//! Player controls: right-click move / attack, attack-move, stop, and spell ranking.

use bevy::prelude::*;

use crate::abilities::{cancel_targeting_if_any, AbilityTargeting};
use crate::combat::flat_distance;
use crate::components::{
    AbilityLoadout, AttackTarget, CombatStats, Ground, Health, HeroProgress, MoveTarget,
    PlayerHero, Team, UnitRadius,
};
use crate::items::ShopUiState;
use crate::movement::{order_hero_move, order_hero_stop};

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                handle_stop_command,
                handle_point_and_click,
                handle_attack_move,
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

fn cursor_ground_hit(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform)>,
    ground: &Query<&GlobalTransform, With<Ground>>,
) -> Option<Vec3> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, cam_transform) = camera.single().ok()?;
    let ray = camera.viewport_to_world(cam_transform, cursor).ok()?;
    let ground_tf = ground.single().ok()?;
    let plane = InfinitePlane3d::new(Dir3::Y);
    let distance = ray.intersect_plane(ground_tf.translation(), plane)?;
    Some(ray.get_point(distance))
}

fn issue_attack_or_move(
    commands: &mut Commands,
    hero_entity: Entity,
    hero_tf: &Transform,
    hero_team: Team,
    stats: &CombatStats,
    hit: Vec3,
    enemies: &Query<(Entity, &GlobalTransform, &Team, &Health, Option<&UnitRadius>)>,
    click_radius: bool,
) {
    let clicked_enemy = enemies
        .iter()
        .filter(|(_, _, team, hp, _)| **team == hero_team.enemy() && hp.is_alive())
        .filter(|(_, tf, _, _, radius)| {
            if !click_radius {
                return true;
            }
            let r = radius.map(|r| r.0).unwrap_or(0.5);
            flat_distance(tf.translation(), hit) < r + 1.2
        })
        .min_by(|a, b| {
            flat_distance(a.1.translation(), hit)
                .partial_cmp(&flat_distance(b.1.translation(), hit))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    if let Some((enemy, enemy_tf, _, _, radius)) = clicked_enemy {
        order_attack_target(commands, hero_entity, hero_tf, stats, enemy, enemy_tf, radius);
    } else if click_radius {
        order_hero_move(commands, hero_entity, hit);
    }
}

fn order_attack_target(
    commands: &mut Commands,
    hero_entity: Entity,
    hero_tf: &Transform,
    stats: &CombatStats,
    enemy: Entity,
    enemy_tf: &GlobalTransform,
    radius: Option<&UnitRadius>,
) {
    let reach = stats.attack_range + radius.map(|r| r.0).unwrap_or(0.5);
    let dist = flat_distance(hero_tf.translation, enemy_tf.translation());
    commands.entity(hero_entity).insert(AttackTarget(enemy));
    if dist > reach * 0.9 {
        commands.entity(hero_entity).insert(MoveTarget {
            position: Vec3::new(enemy_tf.translation().x, 0.0, enemy_tf.translation().z),
        });
    } else {
        commands.entity(hero_entity).remove::<MoveTarget>();
    }
}

fn handle_point_and_click(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    hero: Query<(Entity, &Team, &CombatStats, &Transform), With<PlayerHero>>,
    enemies: Query<(Entity, &GlobalTransform, &Team, &Health, Option<&UnitRadius>)>,
    shop_ui: Res<ShopUiState>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
) {
    let right_pressed = mouse.pressed(MouseButton::Right);
    let right_just = mouse.just_pressed(MouseButton::Right);

    // Targeted spells: RMB cancels then (if still held / just pressed) issues move/attack.
    if targeting.active.is_some() {
        if right_just || right_pressed {
            cancel_targeting_if_any(&mut commands, &mut targeting);
        } else {
            return;
        }
    }

    // Hold-RMB continuously reissues; also fire on the initial press.
    if !right_pressed {
        return;
    }

    // Don't steal world orders while the shop overlay is up (unless RMB to cancel).
    if shop_ui.open && !right_just && !right_pressed {
        return;
    }

    let Some(hit) = cursor_ground_hit(&windows, &camera, &ground) else {
        return;
    };
    let Ok((hero_entity, hero_team, stats, hero_tf)) = hero.single() else {
        return;
    };

    issue_attack_or_move(
        &mut commands,
        hero_entity,
        hero_tf,
        *hero_team,
        stats,
        hit,
        &enemies,
        true,
    );
}

/// Attack-move: attack the living enemy closest to the cursor (hotkey G).
fn handle_attack_move(
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    hero: Query<(Entity, &Team, &CombatStats, &Transform), With<PlayerHero>>,
    enemies: Query<(Entity, &GlobalTransform, &Team, &Health, Option<&UnitRadius>)>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    cancel_targeting_if_any(&mut commands, &mut targeting);

    let Some(hit) = cursor_ground_hit(&windows, &camera, &ground) else {
        return;
    };
    let Ok((hero_entity, hero_team, stats, hero_tf)) = hero.single() else {
        return;
    };

    let closest = enemies
        .iter()
        .filter(|(_, _, team, hp, _)| **team == hero_team.enemy() && hp.is_alive())
        .min_by(|a, b| {
            flat_distance(a.1.translation(), hit)
                .partial_cmp(&flat_distance(b.1.translation(), hit))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    if let Some((enemy, enemy_tf, _, _, radius)) = closest {
        order_attack_target(
            &mut commands,
            hero_entity,
            hero_tf,
            stats,
            enemy,
            enemy_tf,
            radius,
        );
    } else {
        // No enemies: advance toward the cursor like a move order.
        order_hero_move(&mut commands, hero_entity, hit);
    }
}
