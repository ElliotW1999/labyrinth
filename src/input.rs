//! Player controls: right-click move / attack, stop, and spell rank hotkeys.

use bevy::prelude::*;

use crate::abilities::{cancel_targeting_if_any, AbilityTargeting};
use crate::combat::flat_distance;
use crate::components::{
    AbilityLoadout, AttackTarget, CombatStats, Ground, Health, HeroProgress, MoveTarget,
    PlayerHero, Team, UnitRadius,
};
use crate::movement::{order_hero_move, order_hero_stop};

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                handle_stop_command,
                handle_point_and_click,
                handle_spell_rank_hotkeys,
            ),
        );
    }
}

fn handle_stop_command(
    keys: Res<ButtonInput<KeyCode>>,
    hero: Query<Entity, With<PlayerHero>>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Space) {
        return;
    }
    let Ok(hero_entity) = hero.single() else {
        return;
    };
    cancel_targeting_if_any(&mut commands, &mut targeting);
    order_hero_stop(&mut commands, hero_entity);
}

fn handle_spell_rank_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mut hero: Query<(&mut AbilityLoadout, &mut HeroProgress), With<PlayerHero>>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if !ctrl {
        return;
    }

    let Ok((mut loadout, mut progress)) = hero.single_mut() else {
        return;
    };

    let picks = [
        (KeyCode::KeyQ, 0usize),
        (KeyCode::KeyW, 1),
        (KeyCode::KeyE, 2),
        (KeyCode::KeyR, 3),
    ];
    for (key, index) in picks {
        if keys.just_pressed(key) {
            loadout.try_rank_up(index, progress.level, &mut progress.skill_points);
        }
    }
}

fn handle_point_and_click(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    hero: Query<(Entity, &Team, &CombatStats, &Transform), With<PlayerHero>>,
    enemies: Query<(Entity, &GlobalTransform, &Team, &Health, Option<&UnitRadius>)>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
) {
    let right = mouse.just_pressed(MouseButton::Right);
    let left = mouse.just_pressed(MouseButton::Left);

    // Targeted spells consume LMB; RMB cancels then issues move/attack.
    if targeting.active.is_some() {
        if right {
            cancel_targeting_if_any(&mut commands, &mut targeting);
        } else {
            return;
        }
    }

    // Movement / attack is right-click only (left is spell confirm / UI).
    if !right {
        let _ = left;
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

    let Ok((hero_entity, hero_team, stats, hero_tf)) = hero.single() else {
        return;
    };

    let clicked_enemy = enemies
        .iter()
        .filter(|(_, _, team, hp, _)| **team == hero_team.enemy() && hp.is_alive())
        .filter(|(_, tf, _, _, radius)| {
            let r = radius.map(|r| r.0).unwrap_or(0.5);
            flat_distance(tf.translation(), hit) < r + 1.2
        })
        .min_by(|a, b| {
            flat_distance(a.1.translation(), hit)
                .partial_cmp(&flat_distance(b.1.translation(), hit))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    if let Some((enemy, enemy_tf, _, _, radius)) = clicked_enemy {
        let reach = stats.attack_range + radius.map(|r| r.0).unwrap_or(0.5);
        let dist = flat_distance(hero_tf.translation, enemy_tf.translation());
        commands.entity(hero_entity).insert(AttackTarget(enemy));
        if dist > reach * 0.9 {
            // Path toward the target when out of range.
            commands.entity(hero_entity).insert(MoveTarget {
                position: Vec3::new(enemy_tf.translation().x, 0.0, enemy_tf.translation().z),
            });
        } else {
            commands.entity(hero_entity).remove::<MoveTarget>();
        }
    } else {
        order_hero_move(&mut commands, hero_entity, hit);
    }
}
