//! Click-to-move locomotion and order clearing on new attack commands.

use bevy::prelude::*;

use crate::components::{CombatStats, MoveTarget};

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, apply_move_targets);
    }
}

fn apply_move_targets(
    time: Res<Time>,
    mut movers: Query<(Entity, &mut Transform, &CombatStats, &MoveTarget)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, stats, target) in &mut movers {
        if stats.move_speed <= 0.0 {
            continue;
        }

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
        if step >= distance {
            transform.translation = destination;
            commands.entity(entity).remove::<MoveTarget>();
        } else {
            transform.translation += dir * step;
            let yaw = dir.x.atan2(dir.z);
            transform.rotation = Quat::from_rotation_y(yaw);
        }
    }
}

/// Issue a ground move for the local hero (used by input).
pub fn order_hero_move(commands: &mut Commands, hero: Entity, position: Vec3) {
    commands
        .entity(hero)
        .insert(MoveTarget { position })
        .remove::<crate::components::AttackTarget>();
}
