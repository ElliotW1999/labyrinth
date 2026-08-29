//! Wire protocol for Labyrinth multiplayer (bincode over UDP).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientToServer {
    Hello { name: String, hero_kind: u8 },
    Heartbeat,
    MoveTo { x: f32, y: f32, z: f32 },
    AttackMove { x: f32, y: f32, z: f32 },
    AttackNet { target: u32 },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerToClient {
    Welcome {
        peer_id: u32,
        hero_id: u32,
        team: u8,
        /// Client's selected hero kit.
        hero_kind: u8,
        /// Host / opponent hero kit.
        opponent_kind: u8,
    },
    Snapshot { tick: u32, heroes: Vec<HeroSnap> },
    Reject { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeroSnap {
    pub id: u32,
    pub team: u8,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub hp: f32,
    pub hp_max: f32,
}

pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).unwrap_or_default()
}

pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, bincode::Error> {
    bincode::deserialize(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_hello() {
        let msg = ClientToServer::Hello {
            name: "tester".into(),
            hero_kind: 1,
        };
        let bytes = encode(&msg);
        let back: ClientToServer = decode(&bytes).unwrap();
        match back {
            ClientToServer::Hello { name, hero_kind } => {
                assert_eq!(name, "tester");
                assert_eq!(hero_kind, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn roundtrip_snapshot() {
        let msg = ServerToClient::Snapshot {
            tick: 9,
            heroes: vec![HeroSnap {
                id: 1,
                team: 0,
                x: 1.0,
                y: 2.0,
                z: 3.0,
                hp: 100.0,
                hp_max: 200.0,
            }],
        };
        let bytes = encode(&msg);
        let back: ServerToClient = decode(&bytes).unwrap();
        match back {
            ServerToClient::Snapshot { tick, heroes } => {
                assert_eq!(tick, 9);
                assert_eq!(heroes.len(), 1);
                assert_eq!(heroes[0].id, 1);
            }
            _ => panic!("wrong variant"),
        }
    }
}
