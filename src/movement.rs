//! Click-to-move locomotion with circular obstacle collision.

use bevy::prelude::*;

use crate::combat::flat_distance;
use crate::components::{CombatStats, MoveTarget, Obstacle, UnitRadius};

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (apply_move_targets, resolve_obstacle_collisions)
                .chain()
                .run_if(crate::net::is_sim_authority),
        );
    }
}

fn apply_move_targets(
    time: Res<Time>,
    mut movers: Query<(Entity, &mut Transform, &CombatStats, &MoveTarget, Option<&UnitRadius>)>,
    obstacles: Query<(&Transform, &Obstacle), Without<MoveTarget>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let obstacle_snaps: Vec<(Vec3, f32)> = obstacles
        .iter()
        .map(|(tf, obs)| (tf.translation, obs.radius))
        .collect();

    for (entity, mut transform, stats, target, radius) in &mut movers {
        if stats.move_speed <= 0.0 {
            continue;
        }

        let unit_r = radius.map(|r| r.0).unwrap_or(0.5);
        let mut destination = target.position;
        destination.y = transform.translation.y;

        let to_target = destination - transform.translation;
        let distance = to_target.length();
        if distance < 0.15 {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        let step = stats.move_speed * dt;
        let dir = to_target / distance;
        let mut next = if step >= distance {
            destination
        } else {
            transform.translation + dir * step
        };

        // Soft push out of trees; if the destination itself is blocked, stop the order.
        next = separate_from_obstacles(next, unit_r, &obstacle_snaps);
        if flat_distance(destination, next) > 0.01
            && is_blocked(destination, unit_r, &obstacle_snaps)
            && flat_distance(transform.translation, next) < 0.05
        {
            commands.entity(entity).remove::<MoveTarget>();
            continue;
        }

        let moved = next - transform.translation;
        transform.translation = next;
        if moved.length_squared() > 0.0001 {
            let yaw = moved.x.atan2(moved.z);
            transform.rotation = Quat::from_rotation_y(yaw);
        }

        if flat_distance(transform.translation, destination) < 0.2 {
            commands.entity(entity).remove::<MoveTarget>();
        }
    }
}

fn resolve_obstacle_collisions(
    mut units: Query<
        (&mut Transform, Option<&UnitRadius>),
        (
            Without<Obstacle>,
            Without<crate::components::Tower>,
            Without<crate::components::Ancient>,
        ),
    >,
    obstacles: Query<(&Transform, &Obstacle)>,
) {
    let snaps: Vec<(Vec3, f32)> = obstacles
        .iter()
        .map(|(tf, obs)| (tf.translation, obs.radius))
        .collect();

    for (mut transform, radius) in &mut units {
        let unit_r = radius.map(|r| r.0).unwrap_or(0.5);
        let separated = separate_from_obstacles(transform.translation, unit_r, &snaps);
        transform.translation.x = separated.x;
        transform.translation.z = separated.z;
    }
}

fn is_blocked(pos: Vec3, unit_r: f32, obstacles: &[(Vec3, f32)]) -> bool {
    obstacles.iter().any(|(obs_pos, obs_r)| {
        flat_distance(pos, *obs_pos) < unit_r + *obs_r - 0.05
    })
}

fn separate_from_obstacles(mut pos: Vec3, unit_r: f32, obstacles: &[(Vec3, f32)]) -> Vec3 {
    for _ in 0..3 {
        for (obs_pos, obs_r) in obstacles {
            let min_dist = unit_r + *obs_r;
            let dx = pos.x - obs_pos.x;
            let dz = pos.z - obs_pos.z;
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
        .remove::<crate::components::AttackTarget>();
}

/// Halt movement and cancel the current attack order.
pub fn order_hero_stop(commands: &mut Commands, hero: Entity) {
    commands
        .entity(hero)
        .remove::<MoveTarget>()
        .remove::<crate::components::AttackTarget>();
}
