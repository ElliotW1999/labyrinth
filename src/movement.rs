//! Locomotion: follow the planned [`NavPath`], sidestep units with local
//! avoidance, then resolve tree, building, and unit collision.
//!
//! Static obstacles are avoided by planning (see [`crate::navigation`]); the
//! collision passes below are a safety net that keeps the hard non-overlap rules,
//! not the mechanism units use to find their way around things.

use bevy::prelude::*;
use std::collections::HashMap;

use crate::actions::{Activation, QueuedActivation};
use crate::components::{
    Ancient, AttackMoveOrder, AttackSwing, AttackTarget, CollisionRadius, CombatStats, MoveTarget,
    Obstacle, ObstacleShape, Tower,
};
use crate::dimensions::{center_distance as flat_distance, collision_of};
use crate::displacement::ForcedMovement;
use crate::facing::turn_toward;
use crate::navigation::avoidance::{Neighbor, avoid};
use crate::navigation::{
    DynamicBlocker, MobileFilter, NavGrid, NavPath, NavSteering, flat, ground, plan_paths,
    sync_nav_grid,
};
use crate::status::{LeashAnchor, StatusEffects};

/// Ordering of the simulation's order → AI → navigation/locomotion stages.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Unit command queues turn player / network commands into orders.
    Commands,
    /// AI and attack positioning write `MoveTarget`s.
    Ai,
    /// Path planning, path following, avoidance, collision.
    Movement,
}

/// Distance at which the final waypoint counts as reached.
const ARRIVE_DIST: f32 = 6.0;
/// Consecutive stuck windows before a unit near its goal accepts where it is.
const GIVE_UP_STUCK: u32 = 4;
/// How far ahead stationary units are considered by avoidance (plus radii).
const AVOID_LOOKAHEAD: f32 = 140.0;

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavGrid>()
            .configure_sets(
                Update,
                (SimSet::Commands, SimSet::Ai, SimSet::Movement).chain(),
            )
            .add_systems(
                Update,
                (
                    sync_nav_grid,
                    plan_paths,
                    apply_move_targets,
                    enforce_leashes,
                    resolve_obstacle_collisions,
                    resolve_building_collisions,
                    resolve_unit_collisions,
                )
                    .chain()
                    .in_set(SimSet::Movement)
                    .run_if(crate::net::is_sim_authority),
            );
    }
}

/// Snapshot of a unit for avoidance.
struct UnitSnap {
    entity: Entity,
    pos: Vec2,
    radius: f32,
    stationary: bool,
}

type MoverQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Transform,
        &'static CombatStats,
        &'static MoveTarget,
        Option<&'static CollisionRadius>,
        Option<&'static StatusEffects>,
        Option<&'static mut NavPath>,
        Option<&'static mut NavSteering>,
    ),
    (MobileFilter, Without<ForcedMovement>),
>;

type SnapQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static CollisionRadius,
        Option<&'static StatusEffects>,
        Has<MoveTarget>,
        Has<ForcedMovement>,
    ),
    (MobileFilter, With<CombatStats>),
>;

pub fn apply_move_targets(
    time: Res<Time>,
    grid: Res<NavGrid>,
    mut units: ParamSet<(MoverQuery, SnapQuery)>,
    obstacles: Query<(&Transform, &Obstacle), Without<MoveTarget>>,
    buildings: Query<
        (&Transform, &CollisionRadius),
        (Or<(With<Tower>, With<Ancient>)>, Without<MoveTarget>),
    >,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let building_circles: Vec<(Vec3, f32)> = buildings
        .iter()
        .map(|(tf, radius)| (tf.translation, radius.0))
        .collect();
    let circles: Vec<(Vec3, f32)> = building_circles
        .iter()
        .copied()
        .chain(obstacles.iter().filter_map(|(tf, obs)| match obs.shape {
            ObstacleShape::Circle { radius } => Some((tf.translation, radius)),
            ObstacleShape::Aabb { .. } => None,
        }))
        .collect();
    let aabbs: Vec<(Vec3, f32, f32)> = obstacles
        .iter()
        .filter_map(|(tf, obs)| match obs.shape {
            ObstacleShape::Aabb { half_x, half_z } => Some((tf.translation, half_x, half_z)),
            ObstacleShape::Circle { .. } => None,
        })
        .collect();
    let mut fear_origin: HashMap<Entity, Vec2> = HashMap::new();
    let snaps: Vec<UnitSnap> = units
        .p1()
        .iter()
        .filter(|(_, _, c, statuses, _, forced)| {
            // Forced movers phase through units while they travel.
            c.0 > 0.0 && !forced && !statuses.is_some_and(|s| s.is_phased())
        })
        .map(|(entity, tf, c, _, moving, _)| {
            fear_origin.insert(entity, flat(tf.translation));
            UnitSnap {
                entity,
                pos: flat(tf.translation),
                radius: c.0,
                stationary: !moving,
            }
        })
        .collect();

    for (entity, mut transform, stats, target, radius, statuses, mut path, mut steering) in
        &mut units.p0()
    {
        let move_speed = stats.effective_move_speed(statuses);
        if move_speed <= 0.0 {
            continue;
        }
        if statuses.is_some_and(|s| !s.can_move()) {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        let unit_r = collision_of(radius);
        let pos = flat(transform.translation);
        let step = move_speed * dt;

        // Fear: each frame steer toward a point away from the source (`MoveTarget`-like order).
        let fear_waypoint = if statuses.is_some_and(|s| s.is_feared()) {
            let flee_dir = statuses
                .and_then(|s| s.fear_flee_source())
                .and_then(|src| fear_origin.get(&src).copied())
                .map(|other| (pos - other).normalize_or_zero())
                .filter(|d| d.length_squared() > 1e-6)
                .unwrap_or_else(|| {
                    let flat_fwd = flat(*transform.forward());
                    -flat_fwd.normalize_or(Vec2::X)
                });
            Some(ground(pos + flee_dir.normalize() * 900.0))
        } else {
            None
        };
        let feared = fear_waypoint.is_some();

        // Steer at the current waypoint; without a plan yet, head straight for the order.
        let (waypoint, is_final) = if let Some(wp) = fear_waypoint {
            // Keep `is_final` false so proximity to the flee point doesn't drop `MoveTarget`.
            (wp, false)
        } else {
            match path.as_deref_mut() {
                Some(path) => {
                    // Skip ahead on line of sight, but never cut through standing units:
                    // a stuck replan may have routed around them on purpose.
                    if !path.is_final() {
                        let skip_to = flat(path.waypoints[path.index + 1]);
                        let standing: Vec<DynamicBlocker> = snaps
                            .iter()
                            .filter(|s| {
                                s.stationary
                                    && s.entity != entity
                                    && s.pos.distance(pos) < pos.distance(skip_to) + s.radius
                            })
                            .map(|s| DynamicBlocker {
                                center: s.pos,
                                radius: s.radius,
                            })
                            .collect();
                        if grid.segment_clear(pos, skip_to, unit_r, &standing) {
                            path.index += 1;
                        }
                    }
                    while !path.is_final()
                        && flat(path.waypoints[path.index]).distance(pos) <= step.max(16.0)
                    {
                        path.index += 1;
                    }
                    (path.current().unwrap_or(target.position), path.is_final())
                }
                None => (target.position, true),
            }
        };
        let waypoint = flat(waypoint);
        let to_waypoint = waypoint - pos;
        let distance = to_waypoint.length();
        if !feared && is_final && distance < 4.0 {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }
        if distance < 1e-3 {
            continue;
        }
        let desired = to_waypoint / distance;

        let phased = statuses.is_some_and(|s| s.is_phased());
        // Flyers pass over trees and other props, but buildings still stop them.
        let flying = statuses.is_some_and(|s| s.is_flying());
        let (circles, aabbs): (&[(Vec3, f32)], &[(Vec3, f32, f32)]) = if flying {
            (&building_circles, &[])
        } else {
            (&circles, &aabbs)
        };
        let prev_side = steering.as_deref().map_or(0.0, |s| s.side);
        let avoidance = if phased {
            Default::default()
        } else {
            let neighbors: Vec<Neighbor> = snaps
                .iter()
                .filter(|s| {
                    s.entity != entity
                        && s.pos.distance(pos) < AVOID_LOOKAHEAD + s.radius + unit_r + 40.0
                })
                .map(|s| Neighbor {
                    pos: s.pos,
                    radius: s.radius,
                    stationary: s.stationary,
                })
                .collect();
            avoid(
                pos,
                unit_r,
                desired,
                AVOID_LOOKAHEAD.min(distance + 40.0),
                prev_side,
                &neighbors,
            )
        };
        if let Some(steering) = steering.as_deref_mut() {
            steering.avoidance = ground(avoidance.offset);
            steering.side = avoidance.side;
        }
        let mut dir = (desired + avoidance.offset).normalize_or(desired);

        let turn = stats.effective_turn_rate(statuses);
        let facing = turn_toward(&mut transform, ground(dir), turn, dt);
        if !facing {
            continue;
        }

        let travel = if is_final { step.min(distance) } else { step };
        let mut next = pos + dir * travel;
        // Avoidance must never steer into static geometry; fall back to the planned line.
        if avoidance.side != 0.0 && !flying && !grid.point_clear(next, unit_r) {
            dir = desired;
            next = pos + dir * travel;
        }
        let mut next3 = Vec3::new(next.x, transform.translation.y, next.y);
        next3 = separate_from_circles(next3, unit_r, circles);
        next3 = separate_from_aabbs(next3, unit_r, aabbs);

        let goal = if feared {
            target.position
        } else {
            path.as_deref().map_or(target.position, |p| p.goal)
        };
        if flat_distance(goal, next3) > 0.01
            && is_blocked(Vec3::new(goal.x, next3.y, goal.z), unit_r, circles, aabbs)
            && flat_distance(transform.translation, next3) < 1.5
        {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }
        transform.translation = next3;

        if let Some(path) = path.as_deref_mut() {
            if path.stuck.update(transform.translation, dt, travel) {
                path.request_replan();
                let near_goal = flat_distance(transform.translation, goal) < unit_r * 3.0 + 120.0;
                if near_goal && path.stuck.count >= GIVE_UP_STUCK {
                    commands.entity(entity).remove::<MoveTarget>();
                    continue;
                }
            }
        }

        if !feared
            && is_final
            && flat_distance(transform.translation, ground(waypoint)) < ARRIVE_DIST
        {
            commands.entity(entity).remove::<MoveTarget>();
        }
    }
}

/// Clamp leashed units into their circles. Runs after walking and after forced motion
/// (which ticks earlier in the frame), so a knockback stops at the edge too. A leash
/// whose anchor unit is gone does nothing.
pub fn enforce_leashes(
    mut units: ParamSet<(
        Query<(Entity, &Transform, Option<&StatusEffects>)>,
        Query<&mut Transform>,
    )>,
) {
    let mut corrections: Vec<(Entity, Vec3)> = Vec::new();
    {
        let all = units.p0();
        for (entity, transform, statuses) in &all {
            let Some(statuses) = statuses else {
                continue;
            };
            let mut pos = transform.translation;
            let mut moved = false;
            for leash in statuses.leashes() {
                let center = match leash.anchor {
                    LeashAnchor::Point(p) => p,
                    LeashAnchor::Unit(anchor) if anchor == entity => continue,
                    LeashAnchor::Unit(anchor) => match all.get(anchor) {
                        Ok((_, tf, _)) => flat(tf.translation),
                        Err(_) => continue,
                    },
                };
                let offset = flat(pos) - center;
                if offset.length() > leash.radius {
                    let edge = center + offset.normalize_or_zero() * leash.radius;
                    pos.x = edge.x;
                    pos.z = edge.y;
                    moved = true;
                }
            }
            if moved {
                corrections.push((entity, pos));
            }
        }
    }
    let mut writable = units.p1();
    for (entity, pos) in corrections {
        if let Ok(mut tf) = writable.get_mut(entity) {
            tf.translation = pos;
        }
    }
}

fn resolve_obstacle_collisions(
    mut units: Query<
        (&mut Transform, &CollisionRadius, Option<&StatusEffects>),
        (
            With<CollisionRadius>,
            Without<Obstacle>,
            Without<Tower>,
            Without<Ancient>,
        ),
    >,
    obstacles: Query<(&Transform, &Obstacle)>,
) {
    let circles: Vec<(Vec3, f32)> = obstacles
        .iter()
        .filter_map(|(tf, obs)| match obs.shape {
            ObstacleShape::Circle { radius } => Some((tf.translation, radius)),
            ObstacleShape::Aabb { .. } => None,
        })
        .collect();
    let aabbs: Vec<(Vec3, f32, f32)> = obstacles
        .iter()
        .filter_map(|(tf, obs)| match obs.shape {
            ObstacleShape::Aabb { half_x, half_z } => Some((tf.translation, half_x, half_z)),
            ObstacleShape::Circle { .. } => None,
        })
        .collect();

    for (mut transform, radius, statuses) in &mut units {
        if statuses.is_some_and(|s| s.is_flying()) {
            continue;
        }
        let mut pos = separate_from_circles(transform.translation, radius.0, &circles);
        pos = separate_from_aabbs(pos, radius.0, &aabbs);
        transform.translation.x = pos.x;
        transform.translation.z = pos.z;
    }
}

fn resolve_building_collisions(
    mut mobiles: Query<
        (&mut Transform, &CollisionRadius),
        (
            With<CollisionRadius>,
            Without<Tower>,
            Without<Ancient>,
            Without<Obstacle>,
        ),
    >,
    buildings: Query<(&Transform, &CollisionRadius), Or<(With<Tower>, With<Ancient>)>>,
) {
    let snaps: Vec<(Vec3, f32)> = buildings
        .iter()
        .map(|(tf, r)| (tf.translation, r.0))
        .collect();
    for (mut transform, radius) in &mut mobiles {
        let separated = separate_from_circles(transform.translation, radius.0, &snaps);
        transform.translation.x = separated.x;
        transform.translation.z = separated.z;
    }
}

/// Keep mobile units' [`CollisionRadius`] circles from overlapping.
///
/// Units never shove each other: a moving unit that walks into a stationary one is
/// held back and takes the whole correction. Only units in the same state (both
/// moving or both standing) split it. Forceful units keep priority; phased units
/// ignore collision entirely.
fn resolve_unit_collisions(
    mut units: Query<
        (
            Entity,
            &mut Transform,
            &CollisionRadius,
            Option<&StatusEffects>,
            Has<MoveTarget>,
            Has<ForcedMovement>,
        ),
        (
            With<CombatStats>,
            Without<Tower>,
            Without<Ancient>,
            Without<Obstacle>,
        ),
    >,
) {
    let snaps: Vec<CollisionSnap> = units
        .iter()
        .filter(|(_, _, radius, _, _, _)| radius.0 > 0.0)
        .map(
            |(entity, tf, radius, statuses, moving, forced)| CollisionSnap {
                entity,
                pos: tf.translation,
                radius: radius.0,
                phased: forced || statuses.is_some_and(|s| s.is_phased()),
                forceful: statuses.is_some_and(|s| s.can_push_units()),
                moving,
            },
        )
        .collect();

    let mut pushes: Vec<(Entity, Vec3)> = Vec::new();
    for i in 0..snaps.len() {
        for j in (i + 1)..snaps.len() {
            let (a, b) = (&snaps[i], &snaps[j]);
            if a.phased || b.phased {
                continue;
            }
            let min_dist = a.radius + b.radius;
            let dx = a.pos.x - b.pos.x;
            let dz = a.pos.z - b.pos.z;
            let dist = (dx * dx + dz * dz).sqrt();
            if dist >= min_dist {
                continue;
            }
            let (nx, nz) = if dist > 1e-4 {
                (dx / dist, dz / dist)
            } else {
                // Exactly stacked: separate along X, ordered by entity for determinism.
                if a.entity.to_bits() < b.entity.to_bits() {
                    (-1.0, 0.0)
                } else {
                    (1.0, 0.0)
                }
            };
            let overlap = min_dist - dist;
            let (a_w, b_w) = collision_shares(a, b);
            if a_w > 0.0 {
                pushes.push((
                    a.entity,
                    Vec3::new(nx * overlap * a_w, 0.0, nz * overlap * a_w),
                ));
            }
            if b_w > 0.0 {
                pushes.push((
                    b.entity,
                    Vec3::new(-nx * overlap * b_w, 0.0, -nz * overlap * b_w),
                ));
            }
        }
    }

    for (entity, push) in pushes {
        if let Ok((_, mut tf, ..)) = units.get_mut(entity) {
            tf.translation += push;
        }
    }
}

struct CollisionSnap {
    entity: Entity,
    pos: Vec3,
    radius: f32,
    phased: bool,
    forceful: bool,
    moving: bool,
}

/// Fraction of an overlap correction each unit absorbs.
fn collision_shares(a: &CollisionSnap, b: &CollisionSnap) -> (f32, f32) {
    if a.forceful != b.forceful {
        return if a.forceful {
            (0.15, 0.85)
        } else {
            (0.85, 0.15)
        };
    }
    match (a.moving, b.moving) {
        (true, false) => (1.0, 0.0),
        (false, true) => (0.0, 1.0),
        _ => (0.5, 0.5),
    }
}

fn separate_from_circles(mut pos: Vec3, unit_r: f32, circles: &[(Vec3, f32)]) -> Vec3 {
    for _ in 0..3 {
        for (c_pos, c_r) in circles {
            let min_dist = unit_r + *c_r;
            let dx = pos.x - c_pos.x;
            let dz = pos.z - c_pos.z;
            let dist = (dx * dx + dz * dz).sqrt();
            if dist < min_dist && dist > 1e-4 {
                let push = (min_dist - dist) / dist;
                pos.x += dx * push;
                pos.z += dz * push;
            } else if dist <= 1e-4 {
                pos.x += min_dist;
            }
        }
    }
    pos
}

/// Push a circle of radius `unit_r` out of axis-aligned boxes (expanded by unit_r).
fn separate_from_aabbs(mut pos: Vec3, unit_r: f32, boxes: &[(Vec3, f32, f32)]) -> Vec3 {
    for _ in 0..3 {
        for (c_pos, half_x, half_z) in boxes {
            let hx = *half_x + unit_r;
            let hz = *half_z + unit_r;
            let dx = pos.x - c_pos.x;
            let dz = pos.z - c_pos.z;
            if dx.abs() > hx || dz.abs() > hz {
                continue;
            }
            let push_x = hx - dx.abs();
            let push_z = hz - dz.abs();
            if push_x < push_z {
                pos.x += push_x.copysign(if dx >= 0.0 { 1.0 } else { -1.0 });
            } else {
                pos.z += push_z.copysign(if dz >= 0.0 { 1.0 } else { -1.0 });
            }
        }
    }
    pos
}

fn is_blocked(pos: Vec3, unit_r: f32, circles: &[(Vec3, f32)], aabbs: &[(Vec3, f32, f32)]) -> bool {
    for (c_pos, c_r) in circles {
        let min_dist = unit_r + *c_r;
        let dx = pos.x - c_pos.x;
        let dz = pos.z - c_pos.z;
        if dx * dx + dz * dz < min_dist * min_dist {
            return true;
        }
    }
    for (c_pos, half_x, half_z) in aabbs {
        let hx = *half_x + unit_r;
        let hz = *half_z + unit_r;
        if (pos.x - c_pos.x).abs() <= hx && (pos.z - c_pos.z).abs() <= hz {
            return true;
        }
    }
    false
}

/// Issue a ground move for the local hero (used by input).
pub fn order_hero_move(commands: &mut Commands, hero: Entity, position: Vec3) {
    commands
        .entity(hero)
        .insert(MoveTarget { position })
        .remove::<AttackTarget>()
        .remove::<AttackMoveOrder>()
        .remove::<QueuedActivation>()
        .remove::<AttackSwing>()
        .remove::<Activation>();
}

/// Halt movement and cancel the current attack / attack-move / queued cast.
pub fn order_hero_stop(commands: &mut Commands, hero: Entity) {
    commands
        .entity(hero)
        .remove::<MoveTarget>()
        .remove::<AttackTarget>()
        .remove::<AttackMoveOrder>()
        .remove::<QueuedActivation>()
        .remove::<AttackSwing>()
        .remove::<Activation>();
}

/// Order a unit to attack another. Approach (if out of range) is handled by
/// [`crate::navigation::attack_position::assign_attack_positions`], which picks a
/// free spot within range rather than the target's center.
pub fn order_attack_unit(commands: &mut Commands, attacker: Entity, target: Entity) {
    commands
        .entity(attacker)
        .insert(AttackTarget(target))
        .remove::<AttackSwing>()
        .remove::<Activation>()
        .remove::<AttackMoveOrder>()
        .remove::<QueuedActivation>()
        .remove::<MoveTarget>()
        .remove::<crate::navigation::attack_position::AttackPositionGoal>();
}

pub fn order_attack_move(commands: &mut Commands, hero: Entity, destination: Vec3) {
    commands
        .entity(hero)
        .insert(AttackMoveOrder { destination })
        .insert(MoveTarget {
            position: destination,
        })
        .remove::<AttackTarget>()
        .remove::<QueuedActivation>()
        .remove::<AttackSwing>()
        .remove::<Activation>();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(moving: bool, forceful: bool) -> CollisionSnap {
        CollisionSnap {
            entity: Entity::PLACEHOLDER,
            pos: Vec3::ZERO,
            radius: 30.0,
            phased: false,
            forceful,
            moving,
        }
    }

    #[test]
    fn moving_unit_cannot_push_stationary_unit() {
        assert_eq!(
            collision_shares(&snap(true, false), &snap(false, false)),
            (1.0, 0.0)
        );
        assert_eq!(
            collision_shares(&snap(false, false), &snap(true, false)),
            (0.0, 1.0)
        );
    }

    #[test]
    fn same_state_units_split_and_forceful_wins() {
        assert_eq!(
            collision_shares(&snap(true, false), &snap(true, false)),
            (0.5, 0.5)
        );
        assert_eq!(
            collision_shares(&snap(false, true), &snap(true, false)),
            (0.15, 0.85)
        );
    }

    mod leashes {
        use std::time::Duration;

        use super::*;
        use crate::abilities::definition::DisplacementKind;
        use crate::abilities::effects::AbilityTriggerEvent;
        use crate::components::Health;
        use crate::displacement::{
            DisplacementEvent, apply_displacement_events, tick_forced_movement,
        };
        use crate::status::{Leash, StatusEffect, StatusEffectEvent};

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<DisplacementEvent>()
                .add_message::<StatusEffectEvent>()
                .add_message::<crate::status::StatusLifecycleEvent>()
                .add_message::<AbilityTriggerEvent>()
                .add_message::<crate::abilities::effects::SpellHitEvent>()
                .add_message::<crate::abilities::reactive_triggers::CooldownReadyEvent>()
                .add_plugins(MovementPlugin)
                .add_systems(
                    Update,
                    (tick_forced_movement, apply_displacement_events)
                        .chain()
                        .before(SimSet::Ai),
                );
            app
        }

        fn step(app: &mut App, frames: usize) {
            for _ in 0..frames {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(Duration::from_secs_f32(1.0 / 30.0));
                app.update();
            }
        }

        fn unit(app: &mut App, pos: Vec3, effects: Vec<StatusEffect>) -> Entity {
            app.world_mut()
                .spawn((
                    Transform::from_translation(pos),
                    crate::components::Team::Radiant,
                    Health::new(100.0),
                    CombatStats::simple(0.0, 100.0, 1.0, 0.0, 0.0, 300.0),
                    CollisionRadius(20.0),
                    StatusEffects { effects },
                ))
                .id()
        }

        fn x_z(app: &App, e: Entity) -> Vec2 {
            flat(app.world().get::<Transform>(e).unwrap().translation)
        }

        #[test]
        fn leash_clamps_walking_and_knockback_to_the_region() {
            let mut app = app();
            let region = Leash {
                anchor: LeashAnchor::Point(Vec2::ZERO),
                radius: 200.0,
            };
            let walker = unit(
                &mut app,
                Vec3::new(100.0, 0.0, 0.0),
                vec![StatusEffect::leashed(region, 30.0)],
            );
            app.world_mut().entity_mut(walker).insert(MoveTarget {
                position: Vec3::new(1000.0, 0.0, 0.0),
            });
            step(&mut app, 60);
            let p = x_z(&app, walker);
            assert!(p.length() <= 200.0 + 1e-2, "stays in the region: {p}");
            assert!(p.x > 190.0, "walked up to the edge: {p}");

            let pushed = unit(
                &mut app,
                Vec3::new(0.0, 0.0, 100.0),
                vec![StatusEffect::leashed(region, 30.0)],
            );
            let pusher = unit(&mut app, Vec3::new(0.0, 0.0, -500.0), vec![]);
            app.world_mut().write_message(DisplacementEvent {
                target: pushed,
                kind: DisplacementKind::Knockback,
                caster: pusher,
                origin: Vec3::new(0.0, 0.0, -500.0),
                point: Vec3::new(0.0, 0.0, -500.0),
                aim: Vec3::ZERO,
                cast_target: None,
                distance: Some(800.0),
                duration: 0.4,
                speed: None,
                source: None,
            });
            step(&mut app, 30);
            let p = x_z(&app, pushed);
            assert!(
                p.length() <= 200.0 + 1e-2,
                "knockback stops at the edge: {p}"
            );
            assert!(p.y > 190.0, "it was pushed to the edge: {p}");
        }

        #[test]
        fn tether_follows_a_moving_anchor() {
            let mut app = app();
            let anchor = unit(&mut app, Vec3::ZERO, vec![]);
            let tether = Leash {
                anchor: LeashAnchor::Unit(anchor),
                radius: 150.0,
            };
            let tethered = unit(
                &mut app,
                Vec3::new(100.0, 0.0, 0.0),
                vec![StatusEffect::leashed(tether, 30.0)],
            );
            app.world_mut().entity_mut(anchor).insert(MoveTarget {
                position: Vec3::new(0.0, 0.0, 1500.0),
            });
            step(&mut app, 90);
            let (a, t) = (x_z(&app, anchor), x_z(&app, tethered));
            assert!(a.y > 500.0, "the anchor walked away: {a}");
            assert!(
                a.distance(t) <= 150.0 + 1e-2,
                "dragged along by the tether: {a} {t}"
            );
            assert!(t.y > 300.0);

            // A gone anchor releases the unit.
            app.world_mut().despawn(anchor);
            let before = x_z(&app, tethered);
            app.world_mut().entity_mut(tethered).insert(MoveTarget {
                position: Vec3::new(0.0, 0.0, -1500.0),
            });
            step(&mut app, 30);
            assert!(x_z(&app, tethered).distance(before) > 100.0);
        }
    }
}
