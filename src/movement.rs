//! Locomotion with tree, building, and unit collision.

use bevy::prelude::*;

use crate::combat::flat_distance;
use crate::components::{
    AbilityCasting, Ancient, AttackMoveOrder, AttackSwing, AttackTarget, CombatStats, Creep,
    MoveTarget, Obstacle, PlayerHero, QueuedAbilityCast, Tower, UnitRadius,
};
use crate::facing::turn_toward;
use crate::items::StatusEffects;
use crate::scale;
use crate::net::NetworkedHero;

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
            Option<&UnitRadius>,
            Option<&StatusEffects>,
        ),
        (Without<Tower>, Without<Ancient>, Without<Obstacle>),
    >,
    obstacles: Query<(&Transform, &Obstacle), Without<MoveTarget>>,
    // Without<MoveTarget> keeps this disjoint from `movers` (&mut Transform vs &Transform).
    buildings: Query<
        (&Transform, &UnitRadius),
        (Or<(With<Tower>, With<Ancient>)>, Without<MoveTarget>),
    >,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let mut blockers: Vec<(Vec3, f32)> = obstacles
        .iter()
        .map(|(tf, obs)| (tf.translation, obs.radius))
        .collect();
    for (tf, radius) in &buildings {
        blockers.push((tf.translation, radius.0));
    }

    for (entity, mut transform, stats, target, radius, statuses) in &mut movers {
        if stats.move_speed <= 0.0 {
            continue;
        }
        if statuses.is_some_and(|s| !s.can_move()) {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        let unit_r = radius.map(|r| r.0).unwrap_or(scale::u(0.5));
        let mut destination = target.position;
        destination.y = transform.translation.y;

        let to_target = destination - transform.translation;
        let distance = to_target.length();
        if distance < scale::u(0.15) {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        let dir = to_target / distance;
        let facing = turn_toward(&mut transform, dir, stats.turn_rate, dt);
        // Do not begin moving until the destination is within the facing cone.
        if !facing {
            continue;
        }

        let step = stats.move_speed * dt;
        let mut next = if step >= distance {
            destination
        } else {
            transform.translation + dir * step
        };

        next = separate_from_circles(next, unit_r, &blockers);
        if flat_distance(destination, next) > 0.01
            && is_blocked(destination, unit_r, &blockers)
            && flat_distance(transform.translation, next) < scale::u(0.05)
        {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        transform.translation = next;

        if flat_distance(transform.translation, destination) < scale::u(0.2) {
            commands.entity(entity).remove::<MoveTarget>();
        }
    }
}

fn resolve_obstacle_collisions(
    mut units: Query<
        (&mut Transform, Option<&UnitRadius>),
        (
            Without<Obstacle>,
            Without<Tower>,
            Without<Ancient>,
        ),
    >,
    obstacles: Query<(&Transform, &Obstacle)>,
) {
    let snaps: Vec<(Vec3, f32)> = obstacles
        .iter()
        .map(|(tf, obs)| (tf.translation, obs.radius))
        .collect();

    for (mut transform, radius) in &mut units {
        let unit_r = radius.map(|r| r.0).unwrap_or(scale::u(0.5));
        let separated = separate_from_circles(transform.translation, unit_r, &snaps);
        transform.translation.x = separated.x;
        transform.translation.z = separated.z;
    }
}

fn resolve_building_collisions(
    mut mobiles: Query<
        (&mut Transform, Option<&UnitRadius>),
        (Without<Tower>, Without<Ancient>, Without<Obstacle>),
    >,
    buildings: Query<(&Transform, &UnitRadius), Or<(With<Tower>, With<Ancient>)>>,
) {
    let snaps: Vec<(Vec3, f32)> = buildings
        .iter()
        .map(|(tf, r)| (tf.translation, r.0))
        .collect();
    for (mut transform, radius) in &mut mobiles {
        let unit_r = radius.map(|r| r.0).unwrap_or(scale::u(0.5));
        let separated = separate_from_circles(transform.translation, unit_r, &snaps);
        transform.translation.x = separated.x;
        transform.translation.z = separated.z;
    }
}

/// Soft circular separation between heroes and creeps by `UnitRadius`.
/// Phased units neither push nor are pushed. Forceful units shove harder.
fn resolve_unit_collisions(
    mut units: Query<(
        Entity,
        &mut Transform,
        Option<&UnitRadius>,
        Option<&StatusEffects>,
        Has<PlayerHero>,
        Has<NetworkedHero>,
        Has<Creep>,
    )>,
) {
    let snaps: Vec<(Entity, Vec3, f32, bool, bool)> = units
        .iter()
        .filter(|(_, _, _, _, is_hero, is_net, is_creep)| *is_hero || *is_net || *is_creep)
        .map(|(e, tf, radius, statuses, _, _, _)| {
            (
                e,
                tf.translation,
                radius.map(|r| r.0).unwrap_or(scale::u(0.5)),
                statuses.map(|s| s.is_phased()).unwrap_or(false),
                statuses.map(|s| s.can_push_units()).unwrap_or(false),
            )
        })
        .collect();

    if snaps.len() < 2 {
        return;
    }

    let mut pushes: Vec<(Entity, Vec3)> = Vec::new();
    for i in 0..snaps.len() {
        for j in (i + 1)..snaps.len() {
            let (a_e, a_pos, a_r, a_phase, a_force) = snaps[i];
            let (b_e, b_pos, b_r, b_phase, b_force) = snaps[j];
            if a_phase || b_phase {
                continue;
            }
            let min_dist = a_r + b_r;
            let dx = a_pos.x - b_pos.x;
            let dz = a_pos.z - b_pos.z;
            let dist = (dx * dx + dz * dz).sqrt();
            if dist >= min_dist || dist <= 1e-4 {
                if dist <= 1e-4 {
                    pushes.push((a_e, Vec3::new(min_dist * 0.5, 0.0, 0.0)));
                    pushes.push((b_e, Vec3::new(-min_dist * 0.5, 0.0, 0.0)));
                }
                continue;
            }
            let overlap = min_dist - dist;
            // Forceful units claim more of the separation so they clear space assertively.
            let (a_share, b_share) = match (a_force, b_force) {
                (true, false) => (0.75, 0.25),
                (false, true) => (0.25, 0.75),
                _ => (0.5, 0.5),
            };
            let nx = dx / dist;
            let nz = dz / dist;
            pushes.push((a_e, Vec3::new(nx * overlap * a_share, 0.0, nz * overlap * a_share)));
            pushes.push((b_e, Vec3::new(-nx * overlap * b_share, 0.0, -nz * overlap * b_share)));
        }
    }

    for (entity, push) in pushes {
        if let Ok((_, mut tf, _, _, _, _, _)) = units.get_mut(entity) {
            tf.translation.x += push.x;
            tf.translation.z += push.z;
        }
    }
}

fn is_blocked(pos: Vec3, unit_r: f32, circles: &[(Vec3, f32)]) -> bool {
    circles
        .iter()
        .any(|(c_pos, c_r)| flat_distance(pos, *c_pos) < unit_r + *c_r - 0.05)
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
