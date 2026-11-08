//! Forced movement (knockback / pull) requested by effects through [`DisplacementEvent`].
//!
//! ```text
//! AbilityEffect::Displace (ability or item, via the shared EffectRunner)
//!   → DisplacementEvent
//!   → apply_displacement_events: validate, resolve priority, insert ForcedMovement
//!   → tick_forced_movement: the only writer of the Transform of a unit in motion
//!       - stops at trees, buildings, and the map edge
//!       - lands the unit on valid ground and drops its stale navigation state
//! ```
//!
//! A unit in forced motion keeps its orders (`MoveTarget`); `movement.rs` simply does
//! not steer it until the motion ends. It also ignores unit collision while moving.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::abilities::definition::AbilityTrigger;
use crate::abilities::definition::DisplacementKind;
use crate::abilities::effects::{AbilityTriggerEvent, EffectSource, TriggerContext};
use crate::components::{BoundRadius, CollisionRadius, Health, Team};
use crate::dimensions::{bounds_of, center_distance, collision_of};
use crate::navigation::{NavGrid, NavPath, NavSteering, flat};
use crate::scale::{CYCLONE_HEIGHT, CYCLONE_RISE_TIME};
use crate::status::{StatusChange, StatusEffect, StatusEffectEvent, StatusEffects};

#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct DisplacementEvent {
    pub target: Entity,
    pub kind: DisplacementKind,
    pub caster: Entity,
    /// Caster position when the trigger fired (fallback if the caster is gone).
    pub origin: Vec3,
    /// Where the trigger happened; knockbacks push away from it.
    pub point: Vec3,
    /// The cast's aim point.
    pub aim: Vec3,
    /// The unit the ability was cast on.
    pub cast_target: Option<Entity>,
    /// `None` pulls into contact with the caster.
    pub distance: Option<f32>,
    pub duration: f32,
    /// Travel speed for kinds that chase a unit.
    pub speed: Option<f32>,
    /// Trigger identity; self-propelled motion reports contacts and its end with it.
    pub source: Option<MotionSource>,
}

impl DisplacementEvent {
    /// Builds the event for one resolved target of a trigger.
    pub fn from_ctx(
        ctx: &TriggerContext,
        target: Entity,
        kind: DisplacementKind,
        distance: Option<f32>,
        duration: f32,
        speed: Option<f32>,
    ) -> Self {
        Self {
            target,
            kind,
            caster: ctx.caster,
            origin: ctx.origin,
            point: ctx.point,
            aim: ctx.aim,
            cast_target: ctx.cast_target,
            distance,
            duration,
            speed,
            source: Some(MotionSource {
                source: ctx.source,
                caster: ctx.caster,
                team: ctx.team,
                rank: ctx.rank,
                origin: ctx.origin,
                aim: ctx.aim,
                cast_target: ctx.cast_target,
            }),
        }
    }
}

/// How a forced mover travels.
#[derive(Debug, Clone, PartialEq)]
pub enum MotionPath {
    /// Straight segment (knockback, pull to a point, dash).
    Line { from: Vec3, to: Vec3 },
    /// Chase `anchor` at `speed` until within `stop_distance` (center to center) or
    /// `max_travel` is spent (tracking pull / hook, charge). Falls back to the last
    /// known anchor position once the anchor is gone.
    Toward {
        anchor: Entity,
        speed: f32,
        stop_distance: f32,
        max_travel: f32,
        travelled: f32,
        last_anchor: Vec3,
    },
    /// Keep `offset` (XZ) from `anchor` for as long as the anchor itself is in forced
    /// motion (drag).
    Attached { anchor: Entity, offset: Vec2 },
    /// Stay at `at` (`at.y` = ground height), lifted by `height` (cyclone).
    Hold { at: Vec3, height: f32 },
}

/// Who wins when two motions compete for one unit. A new motion replaces the current
/// one only when its priority is at least as high.
#[derive(PartialOrd, Ord, PartialEq, Eq, Clone, Copy, Debug)]
pub enum MotionPriority {
    /// The unit moves itself (dash, charge).
    Own,
    /// Someone else moves the unit (knockback, pull).
    External,
    /// The unit is held out of play (cyclone); it rejects every other motion.
    Suspend,
}

/// Copy-able trigger identity carried by self-propelled motion so it can report
/// contacts and its end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSource {
    pub source: EffectSource,
    pub caster: Entity,
    pub team: Team,
    pub rank: u32,
    pub origin: Vec3,
    pub aim: Vec3,
    pub cast_target: Option<Entity>,
}

impl MotionSource {
    fn context(&self, trigger: AbilityTrigger, point: Vec3) -> TriggerContext {
        TriggerContext::new(
            trigger,
            self.source,
            self.caster,
            self.team,
            self.rank,
            self.origin,
            self.aim,
        )
        .at(point)
        .cast_target(self.cast_target)
    }
}

#[derive(Component, Debug, Clone, PartialEq)]
pub struct ForcedMovement {
    pub path: MotionPath,
    pub elapsed: f32,
    /// Hard cap for every path.
    pub duration: f32,
    pub priority: MotionPriority,
    /// Skip the terrain stop (flying units).
    pub ignore_terrain: bool,
    /// `Some` emits `OnUnitContact` / `OnMovementEnd` for the motion's owner.
    pub source: Option<MotionSource>,
    /// Units already contacted by this motion.
    pub hit: Vec<Entity>,
    /// End early once this status id is gone from the unit (`"cyclone"`).
    pub bound_status: Option<&'static str>,
}

/// Id of the status that mirrors a cyclone motion.
const CYCLONE_STATUS: &str = "cyclone";

impl ForcedMovement {
    pub fn new(path: MotionPath, duration: f32) -> Self {
        Self {
            path,
            elapsed: 0.0,
            duration: duration.max(0.05),
            priority: MotionPriority::External,
            ignore_terrain: false,
            source: None,
            hit: Vec::new(),
            bound_status: None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn line(from: Vec3, to: Vec3, duration: f32) -> Self {
        Self::new(MotionPath::Line { from, to }, duration)
    }
}

fn flat3(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// `pos` itself when a unit of radius `r` fits there, otherwise the closest valid
/// spot. If nothing fits nearby the unit is left where it is.
pub fn reposition_to_valid_terrain(grid: &NavGrid, pos: Vec3, r: f32) -> Vec3 {
    let p = flat(pos);
    if grid.point_clear(p, r) {
        return pos;
    }
    grid.nearest_walkable(p, r)
        .map_or(pos, |q| Vec3::new(q.x, pos.y, q.y))
}

/// A hook that cannot keep up with a fleeing caster gives up after this many times the
/// nominal pull duration.
const PULL_TIMEOUT_FACTOR: f32 = 3.0;

type DisplaceTargets<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        Option<&'static BoundRadius>,
        Option<&'static StatusEffects>,
        Option<&'static Team>,
        Option<&'static Health>,
        Option<&'static ForcedMovement>,
    ),
>;

pub fn apply_displacement_events(
    mut commands: Commands,
    mut events: MessageReader<DisplacementEvent>,
    mut statuses: MessageWriter<StatusEffectEvent>,
    units: DisplaceTargets,
) {
    // Motions inserted earlier this frame are not visible to the query yet.
    let mut pending: HashMap<Entity, MotionPriority> = HashMap::new();
    for event in events.read() {
        let Ok((target_tf, target_bound, target_statuses, target_team, health, current)) =
            units.get(event.target)
        else {
            continue;
        };
        if health.is_some_and(|h| !h.is_alive()) {
            continue;
        }
        let own = event.target == event.caster;
        let caster_team = units.get(event.caster).ok().and_then(|u| u.3);
        let hostile = !own && (target_team != caster_team || target_team.is_none());
        if hostile && target_statuses.is_some_and(|s| s.has_debuff_immunity()) {
            continue;
        }
        let cyclone = event.kind == DisplacementKind::Cyclone;
        let flying = target_statuses.is_some_and(|s| s.is_flying());
        let priority = if cyclone {
            MotionPriority::Suspend
        } else if own {
            MotionPriority::Own
        } else {
            MotionPriority::External
        };
        // Self-propelled motion needs a unit that can move.
        if priority == MotionPriority::Own && target_statuses.is_some_and(|s| !s.can_move()) {
            continue;
        }
        let current_priority = pending
            .get(&event.target)
            .copied()
            .or(current.map(|m| m.priority));
        if current_priority.is_some_and(|c| priority < c) {
            continue;
        }

        let from = target_tf.translation;
        let Some((path, duration)) = plan_motion(event, from, target_bound, &units, &pending)
        else {
            continue;
        };
        let mut motion = ForcedMovement::new(path, duration);
        motion.priority = priority;
        if priority == MotionPriority::Own {
            motion.source = event.source;
        }
        motion.ignore_terrain = flying || cyclone;
        if cyclone {
            motion.bound_status = Some(CYCLONE_STATUS);
            statuses.write(StatusEffectEvent {
                reactive: false,
                source: Some(event.caster),
                target: event.target,
                change: StatusChange::Apply(StatusEffect::cyclone(duration, hostile)),
            });
        }
        pending.insert(event.target, priority);
        commands.entity(event.target).insert(motion);
    }
}

/// The path and duration cap for one request, or `None` when there is nothing to do
/// (no distance, a missing chase target, no speed).
fn plan_motion(
    event: &DisplacementEvent,
    from: Vec3,
    target_bound: Option<&BoundRadius>,
    units: &DisplaceTargets,
    pending: &HashMap<Entity, MotionPriority>,
) -> Option<(MotionPath, f32)> {
    const MIN_TRAVEL: f32 = 1e-3;
    let duration = event.duration.max(0.05);
    let line = |travel: Vec3| {
        (travel.length() >= MIN_TRAVEL).then_some(MotionPath::Line {
            from,
            to: from + travel,
        })
    };
    match event.kind {
        DisplacementKind::Knockback => {
            let mut away = flat3(from - event.point);
            if away.length() < 1.0 {
                away = flat3(from - event.origin);
            }
            let travel = away.normalize_or_zero() * event.distance.unwrap_or(0.0);
            Some((line(travel)?, duration))
        }
        DisplacementKind::PullToPoint => {
            let toward = flat3(event.point - from);
            let travel = toward.normalize_or_zero()
                * event
                    .distance
                    .map_or(toward.length(), |d| d.min(toward.length()));
            Some((line(travel)?, duration))
        }
        DisplacementKind::Dash => {
            // Constant speed: `duration` is the time for the full `distance`.
            let toward = flat3(event.aim - from);
            let max = event.distance.unwrap_or(toward.length());
            let travel = toward.normalize_or_zero() * toward.length().min(max);
            let path = line(travel)?;
            let share = if max > 0.0 {
                travel.length() / max
            } else {
                1.0
            };
            Some((path, (duration * share).max(0.05)))
        }
        DisplacementKind::Cyclone => {
            // A refreshed cyclone keeps the original ground height.
            let at = match units.get(event.target).ok().and_then(|u| u.5) {
                Some(ForcedMovement {
                    path: MotionPath::Hold { at, .. },
                    ..
                }) => *at,
                _ => from,
            };
            Some((
                MotionPath::Hold {
                    at,
                    height: CYCLONE_HEIGHT,
                },
                duration,
            ))
        }
        DisplacementKind::Drag => {
            // Rides along with the caster's own motion; nothing to ride without one.
            if event.target == event.caster {
                return None;
            }
            let (anchor_pos, anchor_bound, _, _, _, anchor_motion) =
                units.get(event.caster).ok()?;
            if anchor_motion.is_none() && !pending.contains_key(&event.caster) {
                return None;
            }
            let reach = bounds_of(anchor_bound) + bounds_of(target_bound);
            let offset = flat3(from - anchor_pos.translation).clamp_length_max(reach);
            let cap = if event.duration > 0.0 {
                event.duration
            } else {
                f32::INFINITY
            };
            Some((
                MotionPath::Attached {
                    anchor: event.caster,
                    offset: Vec2::new(offset.x, offset.z),
                },
                cap,
            ))
        }
        DisplacementKind::Pull | DisplacementKind::Charge => {
            let charge = event.kind == DisplacementKind::Charge;
            let (anchor, speed) = if charge {
                (event.cast_target?, event.speed.filter(|s| *s > 0.0)?)
            } else {
                (event.caster, 0.0)
            };
            let (anchor_pos, anchor_bound, alive) = units.get(anchor).map_or(
                (event.origin, None, !charge),
                |(tf, bound, _, _, health, _)| {
                    (tf.translation, bound, health.is_none_or(|h| h.is_alive()))
                },
            );
            if !alive {
                return None;
            }
            let stop_distance = bounds_of(anchor_bound) + bounds_of(target_bound);
            let contact = (center_distance(anchor_pos, from) - stop_distance).max(0.0);
            if contact < MIN_TRAVEL {
                return None;
            }
            let path = MotionPath::Toward {
                anchor,
                // A hook closes the initial gap in `duration`; a charge has its own speed.
                speed: if charge {
                    speed
                } else {
                    event.distance.map_or(contact, |d| d.min(contact)) / duration
                },
                stop_distance,
                max_travel: event.distance.unwrap_or(f32::INFINITY),
                travelled: 0.0,
                last_anchor: anchor_pos,
            };
            let cap = if charge {
                duration
            } else {
                duration * PULL_TIMEOUT_FACTOR
            };
            Some((path, cap))
        }
    }
}

/// Marks a unit that has flown: once it is neither flying nor in forced motion it is
/// checked against the ground and set down on the nearest walkable spot.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SettleOnLanding;

pub fn settle_grounded_units(
    mut commands: Commands,
    grid: Res<NavGrid>,
    mut units: Query<
        (
            Entity,
            &mut Transform,
            Option<&CollisionRadius>,
            Option<&StatusEffects>,
            Has<ForcedMovement>,
        ),
        With<SettleOnLanding>,
    >,
) {
    for (entity, mut transform, radius, statuses, moving) in &mut units {
        if moving || statuses.is_some_and(|s| s.is_flying()) {
            continue;
        }
        let landed =
            reposition_to_valid_terrain(&grid, transform.translation, collision_of(radius));
        if landed != transform.translation {
            transform.translation = landed;
            commands.entity(entity).remove::<(NavPath, NavSteering)>();
        }
        commands.entity(entity).remove::<SettleOnLanding>();
    }
}

/// Position, team, and size of every unit, taken before any mover is advanced.
struct UnitSnap {
    entity: Entity,
    pos: Vec3,
    team: Option<Team>,
    alive: bool,
    /// Held out of play (cyclone): cannot be contacted or chased.
    untargetable: bool,
    bound: f32,
}

type SnapQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        Option<&'static Team>,
        Option<&'static Health>,
        Option<&'static BoundRadius>,
        Option<&'static StatusEffects>,
    ),
>;

type MoverQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Transform,
        &'static mut ForcedMovement,
        Option<&'static CollisionRadius>,
        Option<&'static BoundRadius>,
        Option<&'static StatusEffects>,
        Option<&'static Health>,
    ),
>;

pub fn tick_forced_movement(
    mut commands: Commands,
    time: Res<Time>,
    grid: Res<NavGrid>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
    mut units: ParamSet<(MoverQuery, SnapQuery)>,
) {
    let dt = time.delta_secs();
    let snaps: Vec<UnitSnap> = units
        .p1()
        .iter()
        .map(|(entity, tf, team, health, bound, statuses)| UnitSnap {
            entity,
            pos: tf.translation,
            team: team.copied(),
            alive: health.is_none_or(|h| h.is_alive()),
            untargetable: statuses.is_some_and(|s| s.is_untargetable()),
            bound: bounds_of(bound),
        })
        .collect();
    let snap_of = |entity: Entity| snaps.iter().find(|s| s.entity == entity);

    // Pass 1 moves everything that travels on its own; pass 2 moves the units attached
    // to those travellers, which need the anchors' new positions.
    let mut moved: HashMap<Entity, Vec3> = HashMap::new();
    let mut ended: HashSet<Entity> = HashSet::new();
    for attached_pass in [false, true] {
        for (entity, mut transform, mut forced, radius, bound, statuses, health) in &mut units.p0()
        {
            if matches!(forced.path, MotionPath::Attached { .. }) != attached_pass {
                continue;
            }
            let r = collision_of(radius);
            let own_bound = bounds_of(bound);
            // A dead unit's motion ends silently; the owner's disabling ends it loudly.
            let dead = health.is_some_and(|h| !h.is_alive());
            let interrupted =
                forced.priority == MotionPriority::Own && statuses.is_some_and(|s| !s.can_move());

            forced.elapsed += dt;
            let (elapsed, duration, bound_status) =
                (forced.elapsed, forced.duration, forced.bound_status);
            let mut finished = dead || interrupted || forced.elapsed >= forced.duration;
            let mut reached: Option<Entity> = None;
            let here3 = transform.translation;
            let mut next = here3;

            if !dead && !interrupted {
                let elapsed_t = (forced.elapsed / forced.duration).min(1.0);
                match &mut forced.path {
                    MotionPath::Line { from, to } => {
                        next = from.lerp(*to, elapsed_t);
                        next.y = here3.y;
                        finished = elapsed_t >= 1.0;
                    }
                    MotionPath::Hold { at, height } => {
                        let ramp = (elapsed / CYCLONE_RISE_TIME)
                            .min((duration - elapsed) / CYCLONE_RISE_TIME)
                            .clamp(0.0, 1.0);
                        next = Vec3::new(at.x, at.y + *height * ramp, at.z);
                        let status_gone = bound_status
                            .is_some_and(|id| !statuses.is_some_and(|s| s.has_status(id)));
                        finished = elapsed >= duration || status_gone;
                    }
                    MotionPath::Attached { anchor, offset } => {
                        // Follows the anchor for as long as the anchor itself is in motion.
                        match moved.get(anchor) {
                            Some(anchor_pos) => {
                                next = Vec3::new(
                                    anchor_pos.x + offset.x,
                                    here3.y,
                                    anchor_pos.z + offset.y,
                                );
                                finished = ended.contains(anchor);
                            }
                            None => finished = true,
                        }
                    }
                    MotionPath::Toward {
                        anchor,
                        speed,
                        stop_distance,
                        max_travel,
                        travelled,
                        last_anchor,
                    } => {
                        let live_anchor = snap_of(*anchor).filter(|s| s.alive && !s.untargetable);
                        if let Some(a) = live_anchor {
                            *last_anchor = a.pos;
                        }
                        let gap = center_distance(*last_anchor, here3) - *stop_distance;
                        let step = (*speed * dt)
                            .min(gap.max(0.0))
                            .min(*max_travel - *travelled);
                        if step > 0.0 {
                            let dir = flat3(*last_anchor - here3).normalize_or_zero();
                            next = here3 + dir * step;
                            *travelled += step;
                        }
                        if gap - step <= 1e-3 {
                            finished = true;
                            reached = live_anchor.map(|a| a.entity);
                        }
                        if *travelled >= *max_travel - 1e-3 {
                            finished = true;
                        }
                    }
                }

                // Stop at the last clear spot instead of passing through geometry. A unit
                // that already stands inside geometry moves freely so it can get out.
                let here = flat(here3);
                if !forced.ignore_terrain && grid.point_clear(here, r) {
                    let there = flat(next);
                    if !grid.segment_clear(here, there, r, &[]) {
                        let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
                        for _ in 0..10 {
                            let mid = (lo + hi) * 0.5;
                            if grid.segment_clear(here, here.lerp(there, mid), r, &[]) {
                                lo = mid;
                            } else {
                                hi = mid;
                            }
                        }
                        let stop = here.lerp(there, lo);
                        next = Vec3::new(stop.x, next.y, stop.y);
                        finished = true;
                        reached = None;
                    }
                }
                transform.translation.x = next.x;
                transform.translation.z = next.z;
                if matches!(forced.path, MotionPath::Hold { .. }) {
                    transform.translation.y = next.y;
                }
                // Self-propelled movers face where they are going.
                let moved = flat3(next - here3);
                if forced.priority == MotionPriority::Own && moved.length_squared() > 1e-6 {
                    transform.rotation = Quat::from_rotation_y(moved.x.atan2(moved.z));
                }
            }

            // Self-propelled motion reports each newly touched enemy once.
            if !dead && let Some(src) = forced.source {
                let pos = transform.translation;
                for other in &snaps {
                    if other.entity == entity
                        || !other.alive
                        || other.untargetable
                        || other.team.is_none_or(|t| t == src.team)
                        || forced.hit.contains(&other.entity)
                        || center_distance(pos, other.pos) > own_bound + other.bound
                    {
                        continue;
                    }
                    forced.hit.push(other.entity);
                    triggers.write(AbilityTriggerEvent(
                        src.context(AbilityTrigger::OnUnitContact, pos)
                            .trigger_unit(Some(other.entity)),
                    ));
                }
            }

            moved.insert(entity, transform.translation);
            if finished {
                ended.insert(entity);
            }
            if finished {
                // A cyclone sets the unit back down, however it ended.
                if let MotionPath::Hold { at, .. } = forced.path {
                    transform.translation.y = at.y;
                }
                // Flyers stay where they are; `settle_grounded_units` handles their landing.
                if !statuses.is_some_and(|s| s.is_flying()) {
                    transform.translation =
                        reposition_to_valid_terrain(&grid, transform.translation, r);
                }
                if !dead && let Some(src) = forced.source {
                    triggers.write(AbilityTriggerEvent(
                        src.context(AbilityTrigger::OnMovementEnd, transform.translation)
                            .trigger_unit(reached)
                            .affected(forced.hit.clone()),
                    ));
                }
                commands
                    .entity(entity)
                    .remove::<(ForcedMovement, NavPath, NavSteering)>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::abilities::definition::AbilityTrigger;
    use crate::abilities::effects::AbilityTriggerEvent;
    use crate::abilities::effects::{EffectSource, TriggerContext};
    use crate::combat::DamageEvent;
    use crate::components::AbilityId;
    use crate::components::{CombatStats, MoveTarget, Obstacle};
    use crate::movement::{MovementPlugin, SimSet};
    use crate::scale;
    use crate::status::{StatusEffectEvent, apply_status_effect_events, tick_status_effects};
    use crate::teleport::{
        TeleportCompletedEvent, TeleportEvent, TeleportRejectedEvent, apply_teleport_events,
    };

    const DT: f32 = 1.0 / 30.0;

    fn sim_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<DisplacementEvent>()
            .add_message::<StatusEffectEvent>()
            .add_message::<AbilityTriggerEvent>()
            .add_message::<DamageEvent>()
            .add_message::<crate::combat::DamageAppliedEvent>()
            .add_message::<TeleportEvent>()
            .add_message::<TeleportCompletedEvent>()
            .add_message::<TeleportRejectedEvent>()
            .add_plugins(MovementPlugin)
            .add_systems(
                Update,
                (
                    tick_forced_movement,
                    settle_grounded_units,
                    apply_displacement_events,
                    apply_status_effect_events,
                    tick_status_effects,
                    apply_teleport_events,
                    crate::combat::apply_damage_events,
                )
                    .chain()
                    .before(SimSet::Ai),
            );
        app
    }

    fn step(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(DT));
        app.update();
    }

    fn spawn_unit(app: &mut App, team: Team, pos: Vec3, radius: f32) -> Entity {
        app.world_mut()
            .spawn((
                Transform::from_translation(pos),
                team,
                Health::new(500.0),
                CombatStats::simple(10.0, 150.0, 1.0, 0.0, 0.0, 300.0),
                CollisionRadius(radius),
                BoundRadius(radius),
                StatusEffects::default(),
            ))
            .id()
    }

    fn pos(app: &App, e: Entity) -> Vec3 {
        app.world().get::<Transform>(e).unwrap().translation
    }

    fn push(
        app: &mut App,
        target: Entity,
        caster: Entity,
        kind: DisplacementKind,
        point: Vec3,
        distance: Option<f32>,
        duration: f32,
    ) {
        let origin = pos(app, caster);
        app.world_mut().write_message(DisplacementEvent {
            target,
            kind,
            caster,
            origin,
            point,
            aim: point,
            cast_target: Some(target),
            distance,
            duration,
            speed: None,
            source: None,
        });
    }

    fn knock(app: &mut App, target: Entity, caster: Entity, distance: f32) {
        let origin = pos(app, caster);
        push(
            app,
            target,
            caster,
            DisplacementKind::Knockback,
            origin,
            Some(distance),
            0.3,
        );
    }

    fn run(app: &mut App, frames: usize) {
        for _ in 0..frames {
            step(app);
        }
    }

    #[test]
    fn knockback_stops_at_a_tree_instead_of_passing_through() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(300.0, 0.0, 0.0), Obstacle::circle(50.0)));
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-100.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        step(&mut app);
        knock(&mut app, enemy, caster, 600.0);
        run(&mut app, 30);
        let x = pos(&app, enemy).x;
        assert!(
            x > 100.0 && x <= 300.0 - 50.0 - 30.0 + 1.0,
            "stopped at the tree: {x}"
        );
        assert!(app.world().get::<ForcedMovement>(enemy).is_none());
    }

    #[test]
    fn knockback_stops_at_the_map_edge() {
        let mut app = sim_app();
        let caster = spawn_unit(
            &mut app,
            Team::Radiant,
            Vec3::new(scale::MAP_HALF - 400.0, 0.0, 0.0),
            30.0,
        );
        let enemy = spawn_unit(
            &mut app,
            Team::Dire,
            Vec3::new(scale::MAP_HALF - 300.0, 0.0, 0.0),
            30.0,
        );
        step(&mut app);
        knock(&mut app, enemy, caster, 600.0);
        run(&mut app, 30);
        let x = pos(&app, enemy).x;
        assert!(x <= scale::MAP_HALF - 30.0 + 1e-2, "inside the map: {x}");
        assert!(
            x > scale::MAP_HALF - 300.0,
            "still moved toward the edge: {x}"
        );
    }

    #[test]
    fn unit_starting_inside_an_obstacle_can_still_be_displaced_out() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(0.0, 0.0, 0.0), Obstacle::circle(50.0)));
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-300.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(20.0, 0.0, 0.0), 30.0);
        // Skip the frame that rebuilds the grid so only the forced motion acts first.
        step(&mut app);
        app.world_mut()
            .entity_mut(enemy)
            .insert(Transform::from_xyz(20.0, 0.0, 0.0));
        knock(&mut app, enemy, caster, 300.0);
        run(&mut app, 30);
        let p = pos(&app, enemy);
        assert!(p.x >= 50.0 + 30.0 - 1.0, "escaped the obstacle: {p}");
    }

    #[test]
    fn landing_inside_geometry_relocates_to_nearest_walkable() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(0.0, 0.0, 0.0), Obstacle::circle(60.0)));
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(-300.0, 0.0, 0.0), 30.0);
        step(&mut app);
        // A motion that was inserted already inside the tree ends there; it must land outside.
        app.world_mut().entity_mut(enemy).insert((
            Transform::from_xyz(10.0, 0.0, 0.0),
            ForcedMovement::line(Vec3::new(10.0, 0.0, 0.0), Vec3::new(12.0, 0.0, 0.0), 0.1),
        ));
        run(&mut app, 6);
        let p = pos(&app, enemy);
        assert!(
            flat(p).distance(Vec2::ZERO) >= 60.0 + 30.0 - 1.0,
            "relocated: {p}"
        );
        assert!(app.world().get::<ForcedMovement>(enemy).is_none());
    }

    #[test]
    fn knockback_preserves_move_orders_and_replans_after() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-100.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        let order = Vec3::new(0.0, 0.0, 2000.0);
        app.world_mut()
            .entity_mut(enemy)
            .insert(MoveTarget { position: order });
        run(&mut app, 3);
        assert!(app.world().get::<NavPath>(enemy).is_some());
        knock(&mut app, enemy, caster, 300.0);
        step(&mut app);
        assert!(app.world().get::<ForcedMovement>(enemy).is_some());
        assert!(
            app.world().get::<MoveTarget>(enemy).is_some(),
            "orders survive"
        );
        assert!(!app.world().get::<StatusEffects>(enemy).unwrap().is_rooted());
        let mut reset_seen = false;
        for _ in 0..12 {
            step(&mut app);
            reset_seen |= app.world().get::<ForcedMovement>(enemy).is_none()
                && app.world().get::<MoveTarget>(enemy).is_some();
        }
        assert!(reset_seen, "motion ended with the order intact");
        assert!(app.world().get::<MoveTarget>(enemy).is_some());
        assert!(pos(&app, enemy).z > 0.0, "resumed walking toward the order");
    }

    #[test]
    fn walking_does_not_fight_forced_movement() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-100.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        // The order points back at the caster; the push must still win while it lasts.
        app.world_mut().entity_mut(enemy).insert(MoveTarget {
            position: Vec3::new(-1500.0, 0.0, 0.0),
        });
        step(&mut app);
        knock(&mut app, enemy, caster, 300.0);
        step(&mut app);
        let mut last = pos(&app, enemy).x;
        while app.world().get::<ForcedMovement>(enemy).is_some() {
            step(&mut app);
            let x = pos(&app, enemy).x;
            assert!(
                x >= last - 1e-3,
                "steady push, no walking back: {last} -> {x}"
            );
            last = x;
        }
        assert!(last > 250.0, "travelled the full distance: {last}");
    }

    #[test]
    fn debuff_immune_target_ignores_hostile_but_not_own_displacement() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-100.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(0.0, 0.0, 200.0), 30.0);
        for unit in [caster, enemy, ally] {
            app.world_mut()
                .get_mut::<StatusEffects>(unit)
                .unwrap()
                .effects
                .push(crate::status::StatusEffect::debuff_immunity(10.0));
        }
        step(&mut app);
        knock(&mut app, enemy, caster, 300.0);
        step(&mut app);
        assert!(
            app.world().get::<ForcedMovement>(enemy).is_none(),
            "hostile push blocked"
        );

        let behind = pos(&app, caster) - Vec3::X * 100.0;
        push(
            &mut app,
            caster,
            caster,
            DisplacementKind::Knockback,
            behind,
            Some(300.0),
            0.3,
        );
        step(&mut app);
        assert!(
            app.world().get::<ForcedMovement>(caster).is_some(),
            "own motion allowed"
        );

        let origin = pos(&app, caster);
        push(
            &mut app,
            ally,
            caster,
            DisplacementKind::Knockback,
            origin,
            Some(300.0),
            0.3,
        );
        step(&mut app);
        assert!(
            app.world().get::<ForcedMovement>(ally).is_some(),
            "ally motion allowed"
        );
    }

    #[test]
    fn lower_priority_motion_does_not_replace_higher() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-100.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        step(&mut app);
        // Someone else's push is running; the victim's own motion must not replace it.
        let external = ForcedMovement::line(Vec3::ZERO, Vec3::new(0.0, 0.0, 500.0), 1.0);
        app.world_mut().entity_mut(enemy).insert(external);
        let behind = Vec3::new(100.0, 0.0, 0.0);
        push(
            &mut app,
            enemy,
            enemy,
            DisplacementKind::Knockback,
            behind,
            Some(300.0),
            0.3,
        );
        step(&mut app);
        let current = app.world().get::<ForcedMovement>(enemy).unwrap();
        assert_eq!(current.priority, MotionPriority::External);
        assert!(matches!(current.path, MotionPath::Line { to, .. } if to.z == 500.0));

        // Equal priority: the newest wins.
        knock(&mut app, enemy, caster, 300.0);
        step(&mut app);
        let current = app.world().get::<ForcedMovement>(enemy).unwrap();
        assert!(matches!(current.path, MotionPath::Line { to, .. } if to.x > 100.0));

        // A higher priority replaces a lower one.
        let mut own = ForcedMovement::line(Vec3::ZERO, Vec3::new(0.0, 0.0, 100.0), 1.0);
        own.priority = MotionPriority::Own;
        app.world_mut().entity_mut(enemy).insert(own);
        knock(&mut app, enemy, caster, 300.0);
        step(&mut app);
        let current = app.world().get::<ForcedMovement>(enemy).unwrap();
        assert_eq!(current.priority, MotionPriority::External);
    }

    #[test]
    fn forced_movers_ignore_unit_collision_while_moving() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-100.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        let bystander = spawn_unit(&mut app, Team::Dire, Vec3::new(150.0, 0.0, 0.0), 30.0);
        step(&mut app);
        knock(&mut app, enemy, caster, 300.0);
        let mut passed = false;
        for _ in 0..4 {
            step(&mut app);
            if app.world().get::<ForcedMovement>(enemy).is_some() {
                let d = pos(&app, enemy).distance(pos(&app, bystander));
                assert!(
                    (pos(&app, bystander).x - 150.0).abs() < 1e-3,
                    "bystander not shoved"
                );
                passed |= d < 60.0;
            }
        }
        assert!(
            passed,
            "the mover overlapped the bystander without being separated"
        );
    }

    fn collect(app: &App, into: &mut Vec<TriggerContext>) {
        let messages = app.world().resource::<Messages<AbilityTriggerEvent>>();
        into.extend(messages.iter_current_update_messages().map(|e| e.0.clone()));
    }

    fn own_source(caster: Entity) -> MotionSource {
        MotionSource {
            source: EffectSource::Ability(AbilityId::Bulwark),
            caster,
            team: Team::Radiant,
            rank: 1,
            origin: Vec3::ZERO,
            aim: Vec3::new(600.0, 0.0, 0.0),
            cast_target: None,
        }
    }

    /// A self-propelled line from the caster's spot to `to`, reporting triggers.
    fn start_own_motion(app: &mut App, unit: Entity, to: Vec3, duration: f32) {
        let mut motion = ForcedMovement::line(pos(app, unit), to, duration);
        motion.priority = MotionPriority::Own;
        motion.source = Some(own_source(unit));
        app.world_mut().entity_mut(unit).insert(motion);
    }

    #[test]
    fn pull_tracks_a_moving_caster_until_contact() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(600.0, 0.0, 0.0), 30.0);
        // The caster walks away while the hook is in flight.
        app.world_mut().entity_mut(caster).insert(MoveTarget {
            position: Vec3::new(-3000.0, 0.0, 0.0),
        });
        step(&mut app);
        push(
            &mut app,
            enemy,
            caster,
            DisplacementKind::Pull,
            Vec3::ZERO,
            None,
            0.5,
        );
        let mut gap = None;
        for _ in 0..40 {
            step(&mut app);
            if gap.is_none() && app.world().get::<ForcedMovement>(enemy).is_none() {
                gap = Some(pos(&app, enemy).x - pos(&app, caster).x);
            }
        }
        let gap = gap.expect("the pull ended");
        assert!(pos(&app, caster).x < -50.0, "the caster did walk away");
        // The anchor is read at the start of the frame, so allow one frame of its walk.
        assert!(
            (gap - 60.0).abs() < 11.0,
            "ends in contact with the moved caster: {gap}"
        );
    }

    #[test]
    fn pull_with_distance_stops_after_max_travel() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(600.0, 0.0, 0.0), 30.0);
        step(&mut app);
        push(
            &mut app,
            enemy,
            caster,
            DisplacementKind::Pull,
            Vec3::ZERO,
            Some(100.0),
            0.3,
        );
        run(&mut app, 30);
        assert!(app.world().get::<ForcedMovement>(enemy).is_none());
        assert!(
            (pos(&app, enemy).x - 500.0).abs() < 1.0,
            "{}",
            pos(&app, enemy).x
        );
    }

    #[test]
    fn pull_to_point_ends_at_the_trigger_point() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-800.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 400.0), 30.0);
        step(&mut app);
        let point = Vec3::new(300.0, 0.0, 0.0);
        push(
            &mut app,
            enemy,
            caster,
            DisplacementKind::PullToPoint,
            point,
            None,
            0.3,
        );
        run(&mut app, 30);
        assert!(
            pos(&app, enemy).distance(point) < 1.0,
            "{}",
            pos(&app, enemy)
        );

        let capped = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -400.0), 30.0);
        push(
            &mut app,
            capped,
            caster,
            DisplacementKind::PullToPoint,
            point,
            Some(100.0),
            0.3,
        );
        run(&mut app, 30);
        let travelled = pos(&app, capped).distance(Vec3::new(0.0, 0.0, -400.0));
        assert!(
            (travelled - 100.0).abs() < 1.0,
            "capped by distance: {travelled}"
        );
    }

    #[test]
    fn own_motion_reports_each_contacted_enemy_once() {
        let mut app = sim_app();
        let dasher = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 10.0), 30.0);
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, -10.0), 30.0);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(300.0, 0.0, 0.0), 30.0);
        let off_path = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 500.0), 30.0);
        step(&mut app);
        start_own_motion(&mut app, dasher, Vec3::new(600.0, 0.0, 0.0), 0.5);
        let mut seen = Vec::new();
        for _ in 0..20 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        let contacts: Vec<_> = seen
            .iter()
            .filter(|c| c.trigger == AbilityTrigger::OnUnitContact)
            .map(|c| c.trigger_unit.unwrap())
            .collect();
        assert_eq!(contacts, vec![near, far], "once each, in path order");
        assert!(!contacts.contains(&ally) && !contacts.contains(&off_path));
        let first = seen
            .iter()
            .find(|c| c.trigger == AbilityTrigger::OnUnitContact)
            .unwrap();
        assert_eq!(first.caster, dasher);
        assert_eq!(first.source, EffectSource::Ability(AbilityId::Bulwark));
    }

    #[test]
    fn movement_end_fires_once_with_contacts_as_affected_units() {
        let mut app = sim_app();
        let dasher = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0), 30.0);
        step(&mut app);
        start_own_motion(&mut app, dasher, Vec3::new(500.0, 0.0, 0.0), 0.3);
        let mut seen = Vec::new();
        for _ in 0..20 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        let ends: Vec<_> = seen
            .iter()
            .filter(|c| c.trigger == AbilityTrigger::OnMovementEnd)
            .collect();
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0].affected_units, vec![victim]);
        assert!(ends[0].trigger_unit.is_none(), "a line has no chased unit");
        assert!(ends[0].point.distance(pos(&app, dasher)) < 1.0);

        // A chasing motion reports the unit it reached.
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 800.0), 30.0);
        let mut chase = ForcedMovement::new(
            MotionPath::Toward {
                anchor: target,
                speed: 1500.0,
                stop_distance: 60.0,
                max_travel: f32::INFINITY,
                travelled: 0.0,
                last_anchor: pos(&app, target),
            },
            2.0,
        );
        chase.priority = MotionPriority::Own;
        chase.source = Some(own_source(dasher));
        app.world_mut().entity_mut(dasher).insert(chase);
        seen.clear();
        for _ in 0..40 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        let end = seen
            .iter()
            .find(|c| c.trigger == AbilityTrigger::OnMovementEnd)
            .expect("end trigger");
        assert_eq!(end.trigger_unit, Some(target));
    }

    #[test]
    fn replaced_or_teleported_motion_fires_no_end_trigger() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -600.0), 30.0);
        let dasher = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let other = spawn_unit(&mut app, Team::Radiant, Vec3::new(0.0, 0.0, 1000.0), 30.0);
        step(&mut app);
        let mut seen = Vec::new();

        // Replaced by an external knockback.
        start_own_motion(&mut app, dasher, Vec3::new(600.0, 0.0, 0.0), 1.0);
        step(&mut app);
        knock(&mut app, dasher, caster, 200.0);
        for _ in 0..20 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        // Teleported away mid-motion.
        start_own_motion(&mut app, other, Vec3::new(600.0, 0.0, 1000.0), 1.0);
        step(&mut app);
        app.world_mut().write_message(TeleportEvent {
            target: other,
            caster: other,
            destination: Vec3::new(-2000.0, 0.0, -2000.0),
        });
        for _ in 0..20 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        assert!(app.world().get::<ForcedMovement>(other).is_none());
        assert!(
            seen.iter()
                .all(|c| c.trigger != AbilityTrigger::OnMovementEnd),
            "silent ends: {seen:?}"
        );
    }

    #[test]
    fn charge_without_living_target_does_nothing() {
        let mut app = sim_app();
        let charger = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let dead = spawn_unit(&mut app, Team::Dire, Vec3::new(500.0, 0.0, 0.0), 30.0);
        app.world_mut().get_mut::<Health>(dead).unwrap().current = 0.0;
        let live = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 500.0), 30.0);
        step(&mut app);
        let charge = |cast_target, speed| DisplacementEvent {
            target: charger,
            kind: DisplacementKind::Charge,
            caster: charger,
            origin: Vec3::ZERO,
            point: Vec3::ZERO,
            aim: Vec3::ZERO,
            cast_target,
            distance: None,
            duration: 2.0,
            speed,
            source: Some(own_source(charger)),
        };
        for event in [
            charge(None, Some(900.0)),
            charge(Some(dead), Some(900.0)),
            charge(Some(live), None),
        ] {
            app.world_mut().write_message(event);
            step(&mut app);
            assert!(app.world().get::<ForcedMovement>(charger).is_none());
        }
        assert_eq!(pos(&app, charger), Vec3::ZERO);

        app.world_mut()
            .write_message(charge(Some(live), Some(900.0)));
        step(&mut app);
        assert!(app.world().get::<ForcedMovement>(charger).is_some());
    }

    #[test]
    fn drag_ends_when_the_source_stops() {
        let mut app = sim_app();
        let anchor = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(50.0, 0.0, 0.0), 30.0);
        let loner = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 800.0), 30.0);
        let idle = spawn_unit(&mut app, Team::Radiant, Vec3::new(0.0, 0.0, 700.0), 30.0);
        step(&mut app);
        start_own_motion(&mut app, anchor, Vec3::new(900.0, 0.0, 0.0), 1.0);
        push(
            &mut app,
            victim,
            anchor,
            DisplacementKind::Drag,
            Vec3::ZERO,
            None,
            0.0,
        );
        // Nothing to ride along with: the request is ignored.
        push(
            &mut app,
            loner,
            idle,
            DisplacementKind::Drag,
            Vec3::ZERO,
            None,
            0.0,
        );
        run(&mut app, 2);
        let offset = pos(&app, victim).x - pos(&app, anchor).x;
        run(&mut app, 6);
        assert!(app.world().get::<ForcedMovement>(victim).is_some());
        assert!(app.world().get::<ForcedMovement>(loner).is_none());
        let kept = pos(&app, victim).x - pos(&app, anchor).x;
        assert!(
            (kept - offset).abs() < 1e-2,
            "keeps its offset: {offset} vs {kept}"
        );

        app.world_mut()
            .get_mut::<StatusEffects>(anchor)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::stunned(3.0));
        run(&mut app, 2);
        assert!(app.world().get::<ForcedMovement>(anchor).is_none());
        assert!(
            app.world().get::<ForcedMovement>(victim).is_none(),
            "the drag ends with the anchor's motion"
        );
        let stopped = pos(&app, anchor).x;
        run(&mut app, 5);
        assert_eq!(pos(&app, anchor).x, stopped, "the anchor stays put");
        assert!(app.world().get::<ForcedMovement>(victim).is_none());
    }

    fn cyclone(app: &mut App, target: Entity, caster: Entity, duration: f32) {
        let origin = pos(app, caster);
        app.world_mut().write_message(DisplacementEvent {
            target,
            kind: DisplacementKind::Cyclone,
            caster,
            origin,
            point: origin,
            aim: origin,
            cast_target: Some(target),
            distance: None,
            duration,
            speed: None,
            source: None,
        });
    }

    fn statuses(app: &App, e: Entity) -> &StatusEffects {
        app.world().get::<StatusEffects>(e).unwrap()
    }

    #[test]
    fn cyclone_stuns_lifts_and_restores_height() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-300.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 4.0, 0.0), 30.0);
        step(&mut app);
        cyclone(&mut app, enemy, caster, 1.0);
        run(&mut app, 2);
        let s = statuses(&app, enemy);
        assert!(s.is_stunned() && s.is_invulnerable() && s.is_untargetable());
        let mut peak = 0.0_f32;
        for _ in 0..12 {
            step(&mut app);
            let p = pos(&app, enemy);
            peak = peak.max(p.y);
            assert_eq!((p.x, p.z), (0.0, 0.0), "held in place");
        }
        assert!(
            (peak - (4.0 + scale::CYCLONE_HEIGHT)).abs() < 1e-2,
            "lifted by the cyclone height: {peak}"
        );
        run(&mut app, 25);
        assert!(app.world().get::<ForcedMovement>(enemy).is_none());
        assert_eq!(pos(&app, enemy), Vec3::new(0.0, 4.0, 0.0), "set back down");
        assert!(!statuses(&app, enemy).is_untargetable());
    }

    #[test]
    fn cyclone_blocks_damage_displacement_and_teleport() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-300.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        step(&mut app);
        cyclone(&mut app, enemy, caster, 2.0);
        run(&mut app, 3);

        app.world_mut().write_message(DamageEvent::new(
            enemy,
            100.0,
            crate::components::DamageType::Physical,
        ));
        knock(&mut app, enemy, caster, 400.0);
        app.world_mut().write_message(TeleportEvent {
            target: enemy,
            caster,
            destination: Vec3::new(2000.0, 0.0, 2000.0),
        });
        run(&mut app, 3);
        let motion = app.world().get::<ForcedMovement>(enemy).unwrap();
        assert!(matches!(motion.path, MotionPath::Hold { .. }));
        assert_eq!(motion.priority, MotionPriority::Suspend);
        assert_eq!(app.world().get::<Health>(enemy).unwrap().current, 500.0);
        let p = pos(&app, enemy);
        assert_eq!((p.x, p.z), (0.0, 0.0), "neither pushed nor teleported");

        // Another cyclone refreshes it instead of being rejected.
        cyclone(&mut app, enemy, caster, 3.0);
        step(&mut app);
        let motion = app.world().get::<ForcedMovement>(enemy).unwrap();
        assert_eq!(motion.duration, 3.0);
        assert!(matches!(motion.path, MotionPath::Hold { at, .. } if at.y == 0.0));
    }

    #[test]
    fn cyclone_survives_strong_dispel_and_ends_on_remove() {
        use crate::status::{DispelStrength, StatusChange, StatusKind};
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-300.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        step(&mut app);
        cyclone(&mut app, enemy, caster, 5.0);
        run(&mut app, 8);
        assert!(pos(&app, enemy).y > 0.0);

        app.world_mut().write_message(StatusEffectEvent {
            reactive: false,
            source: Some(caster),
            target: enemy,
            change: StatusChange::Dispel {
                kind: StatusKind::Debuff,
                strength: DispelStrength::Strong,
            },
        });
        run(&mut app, 3);
        assert!(
            statuses(&app, enemy).is_untargetable(),
            "a dispel cannot end it"
        );
        assert!(app.world().get::<ForcedMovement>(enemy).is_some());

        app.world_mut().write_message(StatusEffectEvent {
            reactive: false,
            source: Some(caster),
            target: enemy,
            change: StatusChange::Remove("cyclone"),
        });
        run(&mut app, 2);
        assert!(app.world().get::<ForcedMovement>(enemy).is_none());
        assert_eq!(pos(&app, enemy).y, 0.0, "set back down at once");
        assert!(!statuses(&app, enemy).is_stunned());
    }

    #[test]
    fn flying_expiry_over_a_tree_lands_on_walkable_ground() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(0.0, 0.0, 0.0), Obstacle::circle(80.0)));
        let flyer = spawn_unit(&mut app, Team::Radiant, Vec3::new(500.0, 0.0, 0.0), 30.0);
        step(&mut app);
        app.world_mut().write_message(StatusEffectEvent {
            reactive: false,
            source: None,
            target: flyer,
            change: crate::status::StatusChange::Apply(crate::status::StatusEffect::flying(1.0)),
        });
        step(&mut app);
        assert!(statuses(&app, flyer).is_flying());
        // Over the tree while flying: nothing moves it off the spot.
        app.world_mut()
            .entity_mut(flyer)
            .insert(Transform::from_xyz(10.0, 0.0, 0.0));
        run(&mut app, 10);
        assert_eq!(pos(&app, flyer).x, 10.0, "free to hover over the tree");

        run(&mut app, 30);
        assert!(!statuses(&app, flyer).is_flying());
        let p = pos(&app, flyer);
        assert!(
            flat(p).distance(Vec2::ZERO) >= 80.0 + 30.0 - 1.0,
            "set down outside the tree: {p}"
        );
        assert!(app.world().get::<SettleOnLanding>(flyer).is_none());
    }

    #[test]
    fn forced_motion_of_a_flyer_ignores_terrain() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(300.0, 0.0, 0.0), Obstacle::circle(50.0)));
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::new(-100.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, 30.0);
        app.world_mut()
            .get_mut::<StatusEffects>(enemy)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::flying(10.0));
        step(&mut app);
        knock(&mut app, enemy, caster, 600.0);
        run(&mut app, 30);
        assert!(
            (pos(&app, enemy).x - 600.0).abs() < 1.0,
            "{}",
            pos(&app, enemy)
        );
    }

    fn ends(seen: &[TriggerContext]) -> Vec<&TriggerContext> {
        seen.iter()
            .filter(|c| c.trigger == AbilityTrigger::OnMovementEnd)
            .collect()
    }

    fn start_chase(app: &mut App, unit: Entity, target: Entity) {
        let mut chase = ForcedMovement::new(
            MotionPath::Toward {
                anchor: target,
                speed: 900.0,
                stop_distance: 60.0,
                max_travel: f32::INFINITY,
                travelled: 0.0,
                last_anchor: pos(app, target),
            },
            3.0,
        );
        chase.priority = MotionPriority::Own;
        chase.source = Some(own_source(unit));
        app.world_mut().entity_mut(unit).insert(chase);
    }

    #[test]
    fn charger_death_mid_charge_ends_silently() {
        let mut app = sim_app();
        let charger = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(1500.0, 0.0, 0.0), 30.0);
        step(&mut app);
        start_chase(&mut app, charger, target);
        run(&mut app, 4);
        assert!(app.world().get::<ForcedMovement>(charger).is_some());
        app.world_mut().get_mut::<Health>(charger).unwrap().current = 0.0;
        let mut seen = Vec::new();
        for _ in 0..5 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        assert!(app.world().get::<ForcedMovement>(charger).is_none());
        assert!(ends(&seen).is_empty(), "silent end: {seen:?}");
        let stopped = pos(&app, charger);
        run(&mut app, 5);
        assert_eq!(pos(&app, charger), stopped, "a dead charger stays put");
    }

    #[test]
    fn charge_target_dying_mid_charge_ends_at_its_last_position_without_a_reached_unit() {
        let mut app = sim_app();
        let charger = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(1200.0, 0.0, 0.0), 30.0);
        step(&mut app);
        start_chase(&mut app, charger, target);
        run(&mut app, 4);
        app.world_mut().get_mut::<Health>(target).unwrap().current = 0.0;
        let mut seen = Vec::new();
        for _ in 0..60 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        assert!(app.world().get::<ForcedMovement>(charger).is_none());
        let ends = ends(&seen);
        assert_eq!(ends.len(), 1, "one end trigger: {seen:?}");
        assert_eq!(ends[0].trigger_unit, None, "nothing was reached");
        let x = pos(&app, charger).x;
        assert!((x - (1200.0 - 60.0)).abs() < 2.0, "ended at the spot: {x}");
        assert!(
            seen.iter()
                .all(|c| c.trigger != AbilityTrigger::OnUnitContact),
            "a dead unit is not contacted"
        );
    }

    #[test]
    fn dragged_units_are_released_when_the_dasher_dies_or_vanishes() {
        for despawn in [false, true] {
            let mut app = sim_app();
            let anchor = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
            let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(50.0, 0.0, 0.0), 30.0);
            step(&mut app);
            start_own_motion(&mut app, anchor, Vec3::new(1500.0, 0.0, 0.0), 2.0);
            push(
                &mut app,
                victim,
                anchor,
                DisplacementKind::Drag,
                Vec3::ZERO,
                None,
                0.0,
            );
            run(&mut app, 4);
            assert!(app.world().get::<ForcedMovement>(victim).is_some());
            if despawn {
                app.world_mut().despawn(anchor);
            } else {
                app.world_mut().get_mut::<Health>(anchor).unwrap().current = 0.0;
            }
            let mut seen = Vec::new();
            for _ in 0..3 {
                step(&mut app);
                collect(&app, &mut seen);
            }
            assert!(
                app.world().get::<ForcedMovement>(victim).is_none(),
                "released (despawn: {despawn})"
            );
            assert!(ends(&seen).is_empty(), "silent end (despawn: {despawn})");
            let stopped = pos(&app, victim);
            run(&mut app, 5);
            assert_eq!(pos(&app, victim), stopped);
        }
    }

    #[test]
    fn dragged_victim_dying_mid_drag_does_not_stop_the_dash() {
        let mut app = sim_app();
        let anchor = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(50.0, 0.0, 0.0), 30.0);
        step(&mut app);
        start_own_motion(&mut app, anchor, Vec3::new(900.0, 0.0, 0.0), 1.0);
        push(
            &mut app,
            victim,
            anchor,
            DisplacementKind::Drag,
            Vec3::ZERO,
            None,
            0.0,
        );
        run(&mut app, 4);
        app.world_mut().get_mut::<Health>(victim).unwrap().current = 0.0;
        run(&mut app, 2);
        assert!(app.world().get::<ForcedMovement>(victim).is_none());
        assert!(app.world().get::<ForcedMovement>(anchor).is_some());
    }

    #[test]
    fn dash_into_a_tree_stops_at_it_and_reports_the_end_once() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(300.0, 0.0, 0.0), Obstacle::circle(50.0)));
        let dasher = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        step(&mut app);
        start_own_motion(&mut app, dasher, Vec3::new(600.0, 0.0, 0.0), 0.5);
        let mut seen = Vec::new();
        for _ in 0..30 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        let x = pos(&app, dasher).x;
        assert!(x > 150.0 && x <= 300.0 - 50.0 - 30.0 + 1.0, "stopped: {x}");
        assert!(app.world().get::<ForcedMovement>(dasher).is_none());
        let ends = ends(&seen);
        assert_eq!(ends.len(), 1);
        assert!(ends[0].point.distance(pos(&app, dasher)) < 1.0);
    }

    #[test]
    fn rooted_or_stunned_charger_ends_the_charge_and_reports_it() {
        for stun in [false, true] {
            let mut app = sim_app();
            let charger = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
            let target = spawn_unit(&mut app, Team::Dire, Vec3::new(1500.0, 0.0, 0.0), 30.0);
            step(&mut app);
            start_chase(&mut app, charger, target);
            run(&mut app, 3);
            let effect = if stun {
                crate::status::StatusEffect::stunned(3.0)
            } else {
                crate::status::StatusEffect::rooted(3.0)
            };
            app.world_mut()
                .get_mut::<StatusEffects>(charger)
                .unwrap()
                .effects
                .push(effect);
            let mut seen = Vec::new();
            for _ in 0..3 {
                step(&mut app);
                collect(&app, &mut seen);
            }
            assert!(app.world().get::<ForcedMovement>(charger).is_none());
            let ends = ends(&seen);
            assert_eq!(ends.len(), 1, "stun: {stun}");
            assert_eq!(ends[0].trigger_unit, None);
        }
    }

    #[test]
    fn debuff_immunity_blocks_only_hostile_motion() {
        let mut app = sim_app();
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(-300.0, 0.0, 0.0), 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0), 30.0);
        let immune = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        app.world_mut()
            .get_mut::<StatusEffects>(immune)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::debuff_immunity(10.0));
        step(&mut app);
        // A hostile pull and cyclone are rejected.
        push(
            &mut app,
            immune,
            enemy,
            DisplacementKind::Pull,
            Vec3::ZERO,
            None,
            0.3,
        );
        cyclone(&mut app, immune, enemy, 2.0);
        step(&mut app);
        assert!(app.world().get::<ForcedMovement>(immune).is_none());
        assert!(!statuses(&app, immune).is_untargetable());
        // An ally's pull and cyclone go through.
        push(
            &mut app,
            immune,
            ally,
            DisplacementKind::Pull,
            Vec3::ZERO,
            None,
            0.3,
        );
        step(&mut app);
        assert!(app.world().get::<ForcedMovement>(immune).is_some());
        run(&mut app, 12);
        cyclone(&mut app, immune, ally, 2.0);
        step(&mut app);
        step(&mut app);
        assert!(statuses(&app, immune).is_untargetable(), "ally cyclone");
    }

    #[test]
    fn cyclone_replaces_an_own_motion_silently_and_rejects_dashes_while_held() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -600.0), 30.0);
        let dasher = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        step(&mut app);
        start_own_motion(&mut app, dasher, Vec3::new(900.0, 0.0, 0.0), 1.0);
        run(&mut app, 2);
        cyclone(&mut app, dasher, caster, 1.0);
        let mut seen = Vec::new();
        for _ in 0..3 {
            step(&mut app);
            collect(&app, &mut seen);
        }
        let held = app.world().get::<ForcedMovement>(dasher).unwrap();
        assert_eq!(held.priority, MotionPriority::Suspend);
        assert!(ends(&seen).is_empty(), "silent replacement");
        // Its own dash cannot start while held (it is stunned and Suspend outranks it).
        let before = pos(&app, dasher);
        app.world_mut().write_message(DisplacementEvent {
            target: dasher,
            kind: DisplacementKind::Dash,
            caster: dasher,
            origin: before,
            point: before,
            aim: before + Vec3::new(500.0, 0.0, 0.0),
            cast_target: None,
            distance: Some(500.0),
            duration: 0.4,
            speed: None,
            source: Some(own_source(dasher)),
        });
        run(&mut app, 3);
        assert_eq!(
            app.world().get::<ForcedMovement>(dasher).unwrap().priority,
            MotionPriority::Suspend
        );
        assert!((pos(&app, dasher).x - before.x).abs() < 1.0);
    }

    #[test]
    fn hook_pull_ends_when_the_caster_dies_mid_pull() {
        let mut app = sim_app();
        let caster = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO, 30.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0), 30.0);
        step(&mut app);
        push(
            &mut app,
            enemy,
            caster,
            DisplacementKind::Pull,
            Vec3::ZERO,
            None,
            0.6,
        );
        run(&mut app, 4);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 0.0;
        run(&mut app, 60);
        assert!(app.world().get::<ForcedMovement>(enemy).is_none());
        let x = pos(&app, enemy).x;
        assert!(
            x >= 60.0 - 1.0 && x < 900.0,
            "pulled toward the last known spot, then released: {x}"
        );
    }
}
