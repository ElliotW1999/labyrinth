//! Locomotion with tree, building, and unit collision.

use bevy::prelude::*;

use crate::components::{
    AbilityCasting, Ancient, AttackMoveOrder, AttackSwing, AttackTarget, CollisionRadius,
    CombatStats, MoveTarget, Obstacle, ObstacleShape, QueuedAbilityCast, Tower,
};
use crate::dimensions::{center_distance as flat_distance, collision_of, needs_to_close};
use crate::facing::turn_toward;
use crate::items::StatusEffects;

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                apply_move_targets,
                resolve_obstacle_collisions,
                resolve_building_collisions,
                resolve_unit_collisions,
            )
                .chain()
                .run_if(crate::net::is_sim_authority),
        );
    }
}

fn apply_move_targets(
    time: Res<Time>,
    mut movers: Query<
        (
            Entity,
            &mut Transform,
            &CombatStats,
            &MoveTarget,
            Option<&CollisionRadius>,
            Option<&StatusEffects>,
        ),
        (Without<Tower>, Without<Ancient>, Without<Obstacle>),
    >,
    obstacles: Query<(&Transform, &Obstacle), Without<MoveTarget>>,
    buildings: Query<
        (&Transform, &CollisionRadius),
        (Or<(With<Tower>, With<Ancient>)>, Without<MoveTarget>),
    >,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let circles: Vec<(Vec3, f32)> = buildings
        .iter()
        .map(|(tf, radius)| (tf.translation, radius.0))
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

    for (entity, mut transform, stats, target, radius, statuses) in &mut movers {
        if stats.move_speed <= 0.0 {
            continue;
        }
        if statuses.is_some_and(|s| !s.can_move()) {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        let unit_r = collision_of(radius);
        let mut destination = target.position;
        destination.y = transform.translation.y;

        let to_target = destination - transform.translation;
        let distance = to_target.length();
        if distance < 4.0 {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        let dir = to_target / distance;
        let facing = turn_toward(&mut transform, dir, stats.turn_rate, dt);
        if !facing {
            continue;
        }

        let step = stats.move_speed * dt;
        let mut next = if step >= distance {
            destination
        } else {
            transform.translation + dir * step
        };

        next = separate_from_circles(next, unit_r, &circles);
        next = separate_from_aabbs(next, unit_r, &aabbs);
        if flat_distance(destination, next) > 0.01
            && is_blocked(destination, unit_r, &circles, &aabbs)
            && flat_distance(transform.translation, next) < 1.5
        {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        transform.translation = next;

        if flat_distance(transform.translation, destination) < 6.0 {
            commands.entity(entity).remove::<MoveTarget>();
        }
    }
}

fn resolve_obstacle_collisions(
    mut units: Query<
        (&mut Transform, &CollisionRadius),
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

    for (mut transform, radius) in &mut units {
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
        .filter(|(_, _, radius, _, _)| radius.0 > 0.0)
        .map(|(entity, tf, radius, statuses, moving)| CollisionSnap {
            entity,
            pos: tf.translation,
            radius: radius.0,
            phased: statuses.is_some_and(|s| s.is_phased()),
            forceful: statuses.is_some_and(|s| s.can_push_units()),
            moving,
        })
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
                if a.entity.to_bits() < b.entity.to_bits() { (-1.0, 0.0) } else { (1.0, 0.0) }
            };
            let overlap = min_dist - dist;
            let (a_w, b_w) = collision_shares(a, b);
            if a_w > 0.0 {
                pushes.push((a.entity, Vec3::new(nx * overlap * a_w, 0.0, nz * overlap * a_w)));
            }
            if b_w > 0.0 {
                pushes.push((b.entity, Vec3::new(-nx * overlap * b_w, 0.0, -nz * overlap * b_w)));
            }
        }
    }

    for (entity, push) in pushes {
        if let Ok((_, mut tf, _, _, _)) = units.get_mut(entity) {
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
        return if a.forceful { (0.15, 0.85) } else { (0.85, 0.15) };
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

fn is_blocked(
    pos: Vec3,
    unit_r: f32,
    circles: &[(Vec3, f32)],
    aabbs: &[(Vec3, f32, f32)],
) -> bool {
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
        .remove::<QueuedAbilityCast>()
        .remove::<AttackSwing>()
        .remove::<AbilityCasting>();
}

/// Halt movement and cancel the current attack / attack-move / queued cast.
pub fn order_hero_stop(commands: &mut Commands, hero: Entity) {
    commands
        .entity(hero)
        .remove::<MoveTarget>()
        .remove::<AttackTarget>()
        .remove::<AttackMoveOrder>()
        .remove::<QueuedAbilityCast>()
        .remove::<AttackSwing>()
        .remove::<AbilityCasting>();
}

/// Order a unit to attack another, walking until it is within attack range.
pub fn order_attack_unit(
    commands: &mut Commands,
    attacker: Entity,
    attacker_pos: Vec3,
    attacker_bounds: f32,
    attack_range: f32,
    target: Entity,
    target_pos: Vec3,
    target_bounds: f32,
) {
    commands
        .entity(attacker)
        .insert(AttackTarget(target))
        .remove::<AttackSwing>()
        .remove::<AbilityCasting>()
        .remove::<AttackMoveOrder>()
        .remove::<QueuedAbilityCast>();
    if needs_to_close(attacker_pos, attacker_bounds, target_pos, target_bounds, attack_range) {
        commands.entity(attacker).insert(MoveTarget {
            position: Vec3::new(target_pos.x, 0.0, target_pos.z),
        });
    } else {
        commands.entity(attacker).remove::<MoveTarget>();
    }
}

pub fn order_attack_move(commands: &mut Commands, hero: Entity, destination: Vec3) {
    commands
        .entity(hero)
        .insert(AttackMoveOrder { destination })
        .insert(MoveTarget {
            position: destination,
        })
        .remove::<AttackTarget>()
        .remove::<QueuedAbilityCast>()
        .remove::<AttackSwing>()
        .remove::<AbilityCasting>();
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
        assert_eq!(collision_shares(&snap(true, false), &snap(false, false)), (1.0, 0.0));
        assert_eq!(collision_shares(&snap(false, false), &snap(true, false)), (0.0, 1.0));
    }

    #[test]
    fn same_state_units_split_and_forceful_wins() {
        assert_eq!(collision_shares(&snap(true, false), &snap(true, false)), (0.5, 0.5));
        assert_eq!(collision_shares(&snap(false, true), &snap(true, false)), (0.15, 0.85));
    }
}
