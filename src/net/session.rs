//! Session configuration, transport, and network identity.

use std::net::{SocketAddr, UdpSocket};

use bevy::prelude::*;
use clap::ValueEnum;

/// Stable network identity for replicated heroes (Entity ids are not network-safe).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetworkId(pub u32);

/// Marker for heroes that participate in net replication.
#[derive(Component, Debug, Clone, Copy)]
pub struct NetworkedHero;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum NetMode {
    #[default]
    Offline,
    Host,
    Client,
}

#[derive(Resource, Debug, Clone)]
pub struct NetConfig {
    pub mode: NetMode,
    /// Host bind address, or client server address.
    pub addr: SocketAddr,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            mode: NetMode::Offline,
            addr: "127.0.0.1:7777".parse().expect("valid default addr"),
        }
    }
}

impl NetConfig {
    pub fn socket_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn is_authority(&self) -> bool {
        matches!(self.mode, NetMode::Offline | NetMode::Host)
    }
}

pub fn is_sim_authority(config: Option<Res<NetConfig>>) -> bool {
    config.map(|c| c.is_authority()).unwrap_or(true)
}

/// HUD / diagnostics for the current network session.
#[derive(Resource, Debug, Clone)]
#[allow(dead_code)]
pub struct NetStatus {
    pub mode: NetMode,
    pub detail: String,
    pub connected_peers: u32,
}

impl NetStatus {
    pub fn from_config(config: &NetConfig) -> Self {
        let detail = match config.mode {
            NetMode::Offline => "Offline (no server)".into(),
            NetMode::Host => format!("Hosting on {}", config.addr),
            NetMode::Client => format!("Client → {}", config.addr),
        };
        Self {
            mode: config.mode,
            detail,
            connected_peers: 0,
        }
    }
}

#[derive(Resource, Debug, Default)]
pub struct NetSession {
    pub tick: u32,
    pub snapshot_timer: f32,
    pub hello_sent: bool,
    pub client_addr: Option<SocketAddr>,
    pub client_name: String,
    pub local_peer_id: Option<u32>,
    pub local_hero_id: Option<u32>,
    pub local_hero_entity: Option<Entity>,
    pub remote_hero_id: Option<u32>,
    pub remote_hero_entity: Option<Entity>,
    pub next_id: u32,
}

pub fn next_network_id(session: &mut NetSession) -> u32 {
    if session.next_id == 0 {
        session.next_id = 2; // 1 reserved for host Radiant hero
    }
    let id = session.next_id;
    session.next_id += 1;
    id
}

#[derive(Resource)]
pub struct NetTransport {
    socket: UdpSocket,
    /// For clients: the server address. For hosts: unused for send_to_server.
    server_addr: Option<SocketAddr>,
    buf: Vec<u8>,
}

impl NetTransport {
    pub fn bind_host(addr: SocketAddr) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            server_addr: None,
            buf: vec![0; 64 * 1024],
        })
    }

    pub fn connect_client(server: SocketAddr) -> std::io::Result<Self> {
        // Bind ephemeral local port.
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            server_addr: Some(server),
            buf: vec![0; 64 * 1024],
        })
    }

    pub fn peer_hint(&self) -> String {
        self.server_addr
            .map(|a| a.to_string())
            .unwrap_or_else(|| "host".into())
    }

    pub fn poll(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        loop {
            match self.socket.recv_from(&mut self.buf) {
                Ok((len, from)) => out.push((from, self.buf[..len].to_vec())),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        out
    }

    pub fn send_to(&self, addr: &SocketAddr, bytes: &[u8]) -> std::io::Result<usize> {
        self.socket.send_to(bytes, addr)
    }

    pub fn send_to_server(&self, bytes: &[u8]) -> std::io::Result<usize> {
        let Some(addr) = self.server_addr else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "no server address",
            ));
        };
        self.send_to(&addr, bytes)
    }
}
