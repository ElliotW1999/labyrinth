//! Player controls: right-click move / attack, attack-move, stop, and spell ranking.
//!
//! Orders go through [`IssueCommand`]; holding Shift queues them after the
//! current command instead of replacing it.

use bevy::prelude::*;

use crate::abilities::{cancel_targeting_if_any, AbilityTargeting};
use crate::components::{AbilityLoadout, AttackTarget, Ground, HeroProgress, PlayerHero, Team};
use crate::items::ShopUiState;
use crate::menu::MainMenuState;
use crate::movement::SimSet;
use crate::net::{
    client_send_command, should_send_orders_over_network, ClientToServer, NetConfig, NetworkId,
    NetTransport,
};
use crate::picking::{cursor_ground_hit, cursor_ray, ground_hit, pick_under_ray, PickableUnits};
use crate::ui::UiPointerState;
use crate::unit_commands::{IssueCommand, UnitCommand};

/// While right-click is held, the move order follows the cursor once it drifts this far.
const HELD_MOVE_REISSUE: f32 = 24.0;

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
            )
                .before(SimSet::Commands)
                .run_if(|menu: Res<MainMenuState>| !menu.open),
        );
    }
}

/// Shift appends to the command queue instead of replacing it.
pub fn shift_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight)
}

fn handle_stop_command(
    keys: Res<ButtonInput<KeyCode>>,
    hero: Query<Entity, With<PlayerHero>>,
    config: Res<NetConfig>,
    transport: Option<ResMut<NetTransport>>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
    mut issue: MessageWriter<IssueCommand>,
) {
    if !keys.just_pressed(KeyCode::Space) {
        return;
    }
    let Ok(hero_entity) = hero.single() else {
        return;
    };
    cancel_targeting_if_any(&mut commands, &mut targeting);
    if should_send_orders_over_network(&config) {
        if let Some(mut transport) = transport {
            client_send_command(&mut transport, ClientToServer::Stop);
        }
        return;
    }
    issue.write(IssueCommand {
        unit: hero_entity,
        command: UnitCommand::Stop,
        queue: false,
    });
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
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    hero: Query<(Entity, &Team, Option<&AttackTarget>), With<PlayerHero>>,
    pickable: PickableUnits,
    targets: Query<Option<&NetworkId>>,
    shop_ui: Res<ShopUiState>,
    pointer: Res<UiPointerState>,
    config: Res<NetConfig>,
    transport: Option<ResMut<NetTransport>>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
    mut issue: MessageWriter<IssueCommand>,
    mut last_held_move: Local<Option<Vec3>>,
) {
    let right_pressed = mouse.pressed(MouseButton::Right);
    let right_just = mouse.just_pressed(MouseButton::Right);
    let queue = shift_held(&keys);

    if targeting.active.is_some() {
        if right_just || right_pressed {
            cancel_targeting_if_any(&mut commands, &mut targeting);
        } else {
            return;
        }
    }

    // A click can press and release within one (slow) frame: just_pressed still counts.
    if !right_pressed && !right_just {
        *last_held_move = None;
        return;
    }
    // Holding RMB keeps steering a normal move; queued orders are one per click.
    if queue && !right_just {
        return;
    }

    // RMB on UI must not move the hero. Minimap RMB is handled separately.
    if pointer.over_minimap || shop_ui.open || pointer.over_blocking_ui {
        return;
    }

    let Some(ray) = cursor_ray(&windows, &camera) else {
        return;
    };
    let Some(hit) = ground_hit(ray, &ground) else {
        return;
    };
    let Ok((hero_entity, hero_team, current_attack)) = hero.single() else {
        return;
    };
    let enemy_team = hero_team.enemy();
    let clicked_enemy = pick_under_ray(ray, &pickable, |team| team == enemy_team)
        .and_then(|enemy| targets.get(enemy).ok().map(|t| (enemy, t)));

    if should_send_orders_over_network(&config) {
        let Some(mut transport) = transport else {
            return;
        };
        if let Some((_, net_id)) = clicked_enemy {
            if let Some(id) = net_id {
                client_send_command(
                    &mut transport,
                    ClientToServer::AttackNet { target: id.0 },
                );
            } else {
                client_send_command(
                    &mut transport,
                    ClientToServer::AttackMove {
                        x: hit.x,
                        y: hit.y,
                        z: hit.z,
                    },
                );
            }
        } else {
            client_send_command(
                &mut transport,
                ClientToServer::MoveTo {
                    x: hit.x,
                    y: hit.y,
                    z: hit.z,
                },
            );
        }
        return;
    }

    let command = if let Some((enemy, _)) = clicked_enemy {
        if !right_just && current_attack.is_some_and(|a| a.0 == enemy) {
            return;
        }
        *last_held_move = None;
        UnitCommand::Attack { target: enemy }
    } else {
        let drifted = last_held_move.is_none_or(|last| last.distance(hit) > HELD_MOVE_REISSUE);
        if !right_just && !drifted {
            return;
        }
        *last_held_move = Some(hit);
        UnitCommand::Move { destination: hit }
    };
    issue.write(IssueCommand {
        unit: hero_entity,
        command,
        queue,
    });
}

fn handle_attack_move(
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    hero: Query<Entity, With<PlayerHero>>,
    pointer: Res<UiPointerState>,
    config: Res<NetConfig>,
    transport: Option<ResMut<NetTransport>>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
    mut issue: MessageWriter<IssueCommand>,
) {
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    if pointer.over_blocking_ui && !pointer.over_minimap {
        return;
    }
    cancel_targeting_if_any(&mut commands, &mut targeting);

    let Some(hit) = cursor_ground_hit(&windows, &camera, &ground) else {
        return;
    };
    let Ok(hero_entity) = hero.single() else {
        return;
    };

    if should_send_orders_over_network(&config) {
        if let Some(mut transport) = transport {
            client_send_command(
                &mut transport,
                ClientToServer::AttackMove {
                    x: hit.x,
                    y: hit.y,
                    z: hit.z,
                },
            );
        }
        return;
    }

    issue.write(IssueCommand {
        unit: hero_entity,
        command: UnitCommand::AttackMove { destination: hit },
        queue: shift_held(&keys),
    });
}
