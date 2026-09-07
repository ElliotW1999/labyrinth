//! Labyrinth — desktop MOBA / action-RTS foundations.
//!
//! Built on [Bevy](https://bevy.org): native window, ECS for thousands of units,
//! and a plugin layout meant to grow into lanes, jungle, items, and netcode.
//!
//! Default play is fully offline (no server). Optional UDP host/client modes:
//! `cargo run -- --mode host` / `cargo run -- --mode client --addr 127.0.0.1:7777`

mod abilities;
mod ai;
mod camera;
mod combat;
mod components;
mod facing;
mod fog;
mod healthbars;
mod heroes;
mod input;
mod items;
mod map;
mod menu;
mod movement;
mod net;
mod obstacle_course;
mod progression;
mod resources;
mod ui;
mod units;
mod waves;

use std::net::SocketAddr;

use bevy::prelude::*;
use bevy::window::{MonitorSelection, WindowMode};
use clap::Parser;

use abilities::AbilitiesPlugin;
use ai::AiPlugin;
use camera::CameraPlugin;
use combat::CombatPlugin;
use fog::FogPlugin;
use healthbars::HealthBarPlugin;
use heroes::{HeroId, HeroesPlugin, LocalHeroChoice};
use input::InputPlugin;
use items::ItemsPlugin;
use map::MapPlugin;
use menu::MenuPlugin;
use movement::MovementPlugin;
use net::{NetConfig, NetMode, NetPlugin};
use obstacle_course::ObstacleCoursePlugin;
use progression::ProgressionPlugin;
use resources::ResourcesPlugin;
use ui::UiPlugin;
use units::UnitsPlugin;
use waves::WavesPlugin;

#[derive(Parser, Debug)]
#[command(name = "labyrinth", about = "Labyrinth MOBA foundations")]
struct Cli {
    /// Session mode. `offline` needs no server (default).
    #[arg(long, value_enum, default_value_t = NetMode::Offline)]
    mode: NetMode,

    /// Host bind address, or client connect address.
    #[arg(long, default_value = "127.0.0.1:7777")]
    addr: String,

    /// Convenience alias for `--addr 0.0.0.0:<port>` when hosting.
    #[arg(long)]
    port: Option<u16>,

    /// Skip select screen: hero name from the roster (e.g. `vanguard`).
    #[arg(long)]
    hero: Option<String>,
}

fn parse_net_config() -> (NetConfig, LocalHeroChoice) {
    let cli = Cli::parse();
    let addr: SocketAddr = if let Some(port) = cli.port {
        format!("0.0.0.0:{port}")
            .parse()
            .unwrap_or_else(|_| "0.0.0.0:7777".parse().expect("fallback"))
    } else {
        cli.addr
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:7777".parse().expect("fallback"))
    };
    let mut choice = LocalHeroChoice::default();
    if let Some(name) = cli.hero.as_deref() {
        if let Some(hero) = HeroId::from_cli(name) {
            choice.hero = Some(hero);
            choice.from_cli = true;
        } else {
            eprintln!(
                "Unknown --hero `{name}` (use vanguard|skirmisher|arcanist); showing select"
            );
        }
    }
    (
        NetConfig {
            mode: cli.mode,
            addr,
        },
        choice,
    )
}

fn main() {
    let (net_config, hero_choice) = parse_net_config();
    let title = match net_config.mode {
        NetMode::Offline => "Labyrinth".to_string(),
        NetMode::Host => format!("Labyrinth (Host {})", net_config.addr),
        NetMode::Client => format!("Labyrinth (Client → {})", net_config.addr),
    };

    App::new()
        .insert_resource(net_config)
        .insert_resource(hero_choice)
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title,
                    mode: WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins((
            ResourcesPlugin,
            NetPlugin,
            MapPlugin,
            UnitsPlugin,
            HeroesPlugin,
            MovementPlugin,
            CombatPlugin,
            ProgressionPlugin,
        ))
        .add_plugins((
            ItemsPlugin,
            InputPlugin,
            AbilitiesPlugin,
            AiPlugin,
            WavesPlugin,
            CameraPlugin,
            HealthBarPlugin,
            FogPlugin,
            ObstacleCoursePlugin,
            UiPlugin,
            MenuPlugin,
        ))
        .run();
}
