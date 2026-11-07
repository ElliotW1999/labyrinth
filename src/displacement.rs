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

use std::collections::HashMap;

use bevy::prelude::*;

use crate::abilities::definition::DisplacementKind;
use crate::abilities::effects::TriggerContext;
use crate::components::{BoundRadius, CollisionRadius, Health, Team};
use crate::dimensions::{bounds_of, collision_of};
use crate::navigation::{NavGrid, NavPath, NavSteering, flat};
use crate::status::StatusEffects;

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
        }
    }
}

/// How a forced mover travels.
#[derive(Debug, Clone, PartialEq)]
pub enum MotionPath {
    /// Straight segment (knockback, fixed pull).
    Line { from: Vec3, to: Vec3 },
}

/// Who wins when two motions compete for one unit. A new motion replaces the current
/// one only when its priority is at least as high.
#[derive(PartialOrd, Ord, PartialEq, Eq, Clone, Copy, Debug)]
pub enum MotionPriority {
    /// The unit moves itself (dash, charge).
    Own,
    /// Someone else moves the unit (knockback, pull).
    External,
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
}

impl ForcedMovement {
    pub fn line(from: Vec3, to: Vec3, duration: f32) -> Self {
        Self {
            path: MotionPath::Line { from, to },
            elapsed: 0.0,
            duration: duration.max(0.05),
            priority: MotionPriority::External,
            ignore_terrain: false,
        }
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
        // Self-propelled motion needs a unit that can move.
        if own && target_statuses.is_some_and(|s| !s.can_move()) {
            continue;
        }
        let priority = if own {
            MotionPriority::Own
        } else {
            MotionPriority::External
        };
        let current_priority = pending
            .get(&event.target)
            .copied()
            .or(current.map(|m| m.priority));
        if current_priority.is_some_and(|c| priority < c) {
            continue;
        }

        let from = target_tf.translation;
        let travel = match event.kind {
            DisplacementKind::Knockback => {
                let mut away = flat3(from - event.point);
                if away.length() < 1.0 {
                    away = flat3(from - event.origin);
                }
                away.normalize_or_zero() * event.distance.unwrap_or(0.0)
            }
            DisplacementKind::Pull => {
                let (caster_pos, caster_bound) = units
                    .get(event.caster)
                    .map_or((event.origin, None), |(tf, bound, ..)| {
                        (tf.translation, bound)
                    });
                let toward = flat3(caster_pos - from);
                let contact =
                    (toward.length() - bounds_of(caster_bound) - bounds_of(target_bound)).max(0.0);
                let distance = event.distance.map_or(contact, |d| d.min(contact));
                toward.normalize_or_zero() * distance
            }
        };
        if travel.length() < 1e-3 {
            continue;
        }
        let mut motion = ForcedMovement::line(from, from + travel, event.duration);
        motion.priority = priority;
        pending.insert(event.target, priority);
        commands.entity(event.target).insert(motion);
    }
}

pub fn tick_forced_movement(
    mut commands: Commands,
    time: Res<Time>,
    grid: Res<NavGrid>,
    mut units: Query<(
        Entity,
        &mut Transform,
        &mut ForcedMovement,
        Option<&CollisionRadius>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut forced, radius) in &mut units {
        let r = collision_of(radius);
        forced.elapsed += dt;
        let t = (forced.elapsed / forced.duration).min(1.0);
        let mut finished = t >= 1.0;
        let MotionPath::Line { from, to } = forced.path;
        let mut next = from.lerp(to, t);

        // Stop at the last clear spot instead of passing through geometry. A unit that
        // already stands inside geometry moves freely so it can get out.
        let here = flat(transform.translation);
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
            }
        }
        transform.translation.x = next.x;
        transform.translation.z = next.z;

        if finished {
            transform.translation = reposition_to_valid_terrain(&grid, transform.translation, r);
            commands
                .entity(entity)
                .remove::<(ForcedMovement, NavPath, NavSteering)>();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::abilities::effects::AbilityTriggerEvent;
    use crate::combat::DamageEvent;
    use crate::components::{CombatStats, MoveTarget, Obstacle};
    use crate::movement::{MovementPlugin, SimSet};
    use crate::scale;
    use crate::status::{StatusEffectEvent, apply_status_effect_events, tick_status_effects};
    use crate::teleport::TeleportEvent;

    const DT: f32 = 1.0 / 30.0;

    fn sim_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<DisplacementEvent>()
            .add_message::<StatusEffectEvent>()
            .add_message::<AbilityTriggerEvent>()
            .add_message::<DamageEvent>()
            .add_message::<TeleportEvent>()
            .add_plugins(MovementPlugin)
            .add_systems(
                Update,
                (
                    tick_forced_movement,
                    apply_displacement_events,
                    apply_status_effect_events,
                    tick_status_effects,
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
}
