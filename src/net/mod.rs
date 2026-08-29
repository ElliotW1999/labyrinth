//! Multiplayer networking: offline by default, optional host/client UDP session.

mod proto;
mod session;

pub use proto::{ClientToServer, HeroSnap, ServerToClient};
pub use session::{
    is_sim_authority, NetConfig, NetMode, NetSession, NetStatus, NetTransport, NetworkId,
    NetworkedHero,
};

use bevy::prelude::*;

use crate::combat::flat_distance;
use crate::components::{
    AttackTarget, CombatStats, Health, MoveTarget, Team, UnitRadius,
};
use crate::movement::{order_attack_move, order_hero_move, order_hero_stop};
use crate::units::spawn_hero_entity;

use proto::{decode, encode};
use session::next_network_id;

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        let config = app
            .world()
            .get_resource::<NetConfig>()
            .cloned()
            .unwrap_or_default();

        app.insert_resource(NetStatus::from_config(&config))
            .insert_resource(NetSession::default());

        match config.mode {
            NetMode::Offline => {}
            NetMode::Host => match NetTransport::bind_host(config.socket_addr()) {
                Ok(transport) => {
                    app.insert_resource(transport);
                    app.add_systems(
                        Update,
                        (host_recv_and_apply, host_broadcast_snapshots).chain(),
                    );
                }
                Err(err) => {
                    bevy::log::error!("Failed to start host on {}: {err}", config.socket_addr());
                    app.insert_resource(NetStatus {
                        mode: NetMode::Offline,
                        detail: format!("Host bind failed: {err} (running offline)"),
                        connected_peers: 0,
                    });
                }
            },
            NetMode::Client => match NetTransport::connect_client(config.socket_addr()) {
                Ok(transport) => {
                    app.insert_resource(transport);
                    app.add_systems(
                        Update,
                        (client_send_hello, client_recv_and_apply).chain(),
                    );
                }
                Err(err) => {
                    bevy::log::error!(
                        "Failed to start client toward {}: {err}",
                        config.socket_addr()
                    );
                    app.insert_resource(NetStatus {
                        mode: NetMode::Offline,
                        detail: format!("Client start failed: {err}"),
                        connected_peers: 0,
                    });
                }
            },
        }
    }
}

fn team_to_u8(team: Team) -> u8 {
    match team {
        Team::Radiant => 0,
        Team::Dire => 1,
    }
}

fn team_from_u8(value: u8) -> Team {
    if value == 1 {
        Team::Dire
    } else {
        Team::Radiant
    }
}

fn host_recv_and_apply(
    mut transport: ResMut<NetTransport>,
    mut session: ResMut<NetSession>,
    mut status: ResMut<NetStatus>,
    mut commands: Commands,
    assets: Res<crate::resources::SharedAssets>,
    heroes: Query<(Entity, &NetworkId, &Transform, &CombatStats, Option<&UnitRadius>)>,
    net_targets: Query<(
        Entity,
        &NetworkId,
        &GlobalTransform,
        &Team,
        &Health,
        Option<&UnitRadius>,
    )>,
    _any_enemies: Query<(Entity, &GlobalTransform, &Team, &Health, Option<&UnitRadius>)>,
) {
    let packets = transport.poll();
    for (from, bytes) in packets {
        let Ok(msg) = decode::<ClientToServer>(&bytes) else {
            continue;
        };
        match msg {
            ClientToServer::Hello { name } => {
                if session.client_addr.is_some() {
                    let _ = transport.send_to(
                        &from,
                        &encode(&ServerToClient::Reject {
                            reason: "Match is full (1v1 only)".into(),
                        }),
                    );
                    continue;
                }
                session.client_addr = Some(from);
                session.client_name = name.clone();
                status.connected_peers = 1;
                status.detail = format!("Host: {name} connected from {from}");

                if session.remote_hero_id.is_none() {
                    let id = next_network_id(&mut session);
                    let entity = spawn_hero_entity(
                        &mut commands,
                        &assets,
                        Team::Dire,
                        false,
                        id,
                        Vec3::new(44.0, 0.9, 44.0),
                    );
                    session.remote_hero_id = Some(id);
                    session.remote_hero_entity = Some(entity);
                }

                let hero_id = session.remote_hero_id.unwrap_or(2);
                let _ = transport.send_to(
                    &from,
                    &encode(&ServerToClient::Welcome {
                        peer_id: 2,
                        hero_id,
                        team: team_to_u8(Team::Dire),
                    }),
                );
            }
            ClientToServer::MoveTo { x, y, z } => {
                if let Some(entity) = session.remote_hero_entity {
                    order_hero_move(&mut commands, entity, Vec3::new(x, y, z));
                }
            }
            ClientToServer::Stop => {
                if let Some(entity) = session.remote_hero_entity {
                    order_hero_stop(&mut commands, entity);
                }
            }
            ClientToServer::AttackMove { x, y, z } => {
                let Some(hero_entity) = session.remote_hero_entity else {
                    continue;
                };
                order_attack_move(&mut commands, hero_entity, Vec3::new(x, y, z));
            }
            ClientToServer::AttackNet { target } => {
                let Some(hero_entity) = session.remote_hero_entity else {
                    continue;
                };
                let Some((_, _, hero_tf, stats, _)) =
                    heroes.iter().find(|(e, ..)| *e == hero_entity)
                else {
                    continue;
                };
                if let Some((enemy, _, enemy_tf, _, _, radius)) =
                    net_targets.iter().find(|(_, id, ..)| id.0 == target)
                {
                    apply_attack_order(
                        &mut commands,
                        hero_entity,
                        hero_tf,
                        stats,
                        enemy,
                        enemy_tf,
                        radius,
                    );
                }
            }
            ClientToServer::Heartbeat => {}
        }
    }
}

fn apply_attack_order(
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

fn host_broadcast_snapshots(
    time: Res<Time>,
    transport: ResMut<NetTransport>,
    mut session: ResMut<NetSession>,
    heroes: Query<(&NetworkId, &Team, &Transform, &Health), With<NetworkedHero>>,
) {
    let Some(client) = session.client_addr else {
        return;
    };
    session.snapshot_timer += time.delta_secs();
    if session.snapshot_timer < 0.05 {
        return;
    }
    session.snapshot_timer = 0.0;
    session.tick = session.tick.wrapping_add(1);

    let snaps: Vec<HeroSnap> = heroes
        .iter()
        .map(|(id, team, tf, hp)| HeroSnap {
            id: id.0,
            team: team_to_u8(*team),
            x: tf.translation.x,
            y: tf.translation.y,
            z: tf.translation.z,
            hp: hp.current,
            hp_max: hp.max,
        })
        .collect();

    let _ = transport.send_to(
        &client,
        &encode(&ServerToClient::Snapshot {
            tick: session.tick,
            heroes: snaps,
        }),
    );
}

fn client_send_hello(
    transport: ResMut<NetTransport>,
    mut session: ResMut<NetSession>,
    mut status: ResMut<NetStatus>,
) {
    if session.hello_sent {
        return;
    }
    session.hello_sent = true;
    status.detail = format!("Client: connecting to {}…", transport.peer_hint());
    let _ = transport.send_to_server(&encode(&ClientToServer::Hello {
        name: "DirePlayer".into(),
    }));
}

fn client_recv_and_apply(
    mut transport: ResMut<NetTransport>,
    mut session: ResMut<NetSession>,
    mut status: ResMut<NetStatus>,
    mut commands: Commands,
    assets: Res<crate::resources::SharedAssets>,
    mut heroes: Query<(Entity, &NetworkId, &mut Transform, &mut Health), With<NetworkedHero>>,
) {
    let packets = transport.poll();
    for (_from, bytes) in packets {
        let Ok(msg) = decode::<ServerToClient>(&bytes) else {
            continue;
        };
        match msg {
            ServerToClient::Welcome {
                peer_id,
                hero_id,
                team,
            } => {
                if session.local_hero_entity.is_some() {
                    continue;
                }
                session.local_peer_id = Some(peer_id);
                session.local_hero_id = Some(hero_id);
                status.connected_peers = 1;
                status.detail = format!("Client: joined as peer {peer_id} (hero {hero_id})");

                let team = team_from_u8(team);
                let spawn_pos = match team {
                    Team::Radiant => Vec3::new(-44.0, 0.9, -44.0),
                    Team::Dire => Vec3::new(44.0, 0.9, 44.0),
                };
                let local = spawn_hero_entity(
                    &mut commands,
                    &assets,
                    team,
                    true,
                    hero_id,
                    spawn_pos,
                );
                session.local_hero_entity = Some(local);

                let remote_id = 1;
                let remote = spawn_hero_entity(
                    &mut commands,
                    &assets,
                    Team::Radiant,
                    false,
                    remote_id,
                    Vec3::new(-44.0, 0.9, -44.0),
                );
                session.remote_hero_id = Some(remote_id);
                session.remote_hero_entity = Some(remote);
            }
            ServerToClient::Snapshot { heroes: snaps, .. } => {
                for snap in snaps {
                    if let Some((_, _, mut tf, mut hp)) =
                        heroes.iter_mut().find(|(_, id, ..)| id.0 == snap.id)
                    {
                        tf.translation = Vec3::new(snap.x, snap.y, snap.z);
                        hp.current = snap.hp;
                        hp.max = snap.hp_max.max(1.0);
                    }
                }
            }
            ServerToClient::Reject { reason } => {
                status.detail = format!("Rejected: {reason}");
            }
        }
    }
}

/// Queue a gameplay command from local input when running as a client.
pub fn client_send_command(transport: &mut NetTransport, msg: ClientToServer) {
    let _ = transport.send_to_server(&encode(&msg));
}

pub fn should_send_orders_over_network(config: &NetConfig) -> bool {
    matches!(config.mode, NetMode::Client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn udp_host_client_hello_welcome() {
        let host_addr: std::net::SocketAddr = "127.0.0.1:17881".parse().unwrap();
        let mut host = NetTransport::bind_host(host_addr).expect("bind host");
        let mut client = NetTransport::connect_client(host_addr).expect("client");

        let hello = encode(&ClientToServer::Hello {
            name: "p2".into(),
        });
        client.send_to_server(&hello).unwrap();
        std::thread::sleep(Duration::from_millis(30));

        let packets = host.poll();
        assert!(!packets.is_empty(), "host should receive hello");
        let (from, bytes) = &packets[0];
        let msg: ClientToServer = decode(bytes).unwrap();
        match msg {
            ClientToServer::Hello { name } => assert_eq!(name, "p2"),
            _ => panic!("expected Hello"),
        }

        let welcome = encode(&ServerToClient::Welcome {
            peer_id: 2,
            hero_id: 2,
            team: 1,
        });
        host.send_to(from, &welcome).unwrap();
        std::thread::sleep(Duration::from_millis(30));

        let replies = client.poll();
        assert!(!replies.is_empty(), "client should receive welcome");
        let msg: ServerToClient = decode(&replies[0].1).unwrap();
        match msg {
            ServerToClient::Welcome {
                peer_id,
                hero_id,
                team,
            } => {
                assert_eq!(peer_id, 2);
                assert_eq!(hero_id, 2);
                assert_eq!(team, 1);
            }
            _ => panic!("expected Welcome"),
        }
    }

    #[test]
    fn offline_is_sim_authority_by_default() {
        let config = NetConfig::default();
        assert!(matches!(config.mode, NetMode::Offline));
        assert!(config.is_authority());
    }
}
