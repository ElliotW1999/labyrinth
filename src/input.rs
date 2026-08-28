//! Player controls: point-and-click move / attack. Camera pan lives in `camera`.

use bevy::prelude::*;

use crate::combat::flat_distance;
use crate::components::{AttackTarget, Ground, Health, MoveTarget, PlayerHero, Team};
use crate::movement::order_hero_move;

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, handle_point_and_click);
    }
}

fn handle_point_and_click(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    hero: Query<(Entity, &Team), With<PlayerHero>>,
    enemies: Query<(Entity, &GlobalTransform, &Team, &Health)>,
    mut commands: Commands,
) {
    let right = mouse.just_pressed(MouseButton::Right);
    let left = mouse.just_pressed(MouseButton::Left);
    if !right && !left {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_transform)) = camera.single() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_transform, cursor) else {
        return;
    };

    let Ok(ground_tf) = ground.single() else {
        return;
    };
    let plane_origin = ground_tf.translation();
    let plane = InfinitePlane3d::new(Dir3::Y);
    let Some(distance) = ray.intersect_plane(plane_origin, plane) else {
        return;
    };
    let hit = ray.get_point(distance);

    let Ok((hero_entity, hero_team)) = hero.single() else {
        return;
    };

    if right {
        let clicked_enemy = enemies
            .iter()
            .filter(|(_, _, team, hp)| **team == hero_team.enemy() && hp.is_alive())
            .filter(|(_, tf, _, _)| flat_distance(tf.translation(), hit) < 2.5)
            .min_by(|a, b| {
                flat_distance(a.1.translation(), hit)
                    .partial_cmp(&flat_distance(b.1.translation(), hit))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some((enemy, _, _, _)) = clicked_enemy {
            commands
                .entity(hero_entity)
                .insert(AttackTarget(enemy))
                .remove::<MoveTarget>();
        } else {
            order_hero_move(&mut commands, hero_entity, hit);
        }
    } else if left {
        order_hero_move(&mut commands, hero_entity, hit);
    }
}
