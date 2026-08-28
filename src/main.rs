//! Labyrinth — desktop MOBA / action-RTS foundations.
//!
//! Built on [Bevy](https://bevy.org): native window, ECS for thousands of units,
//! and a plugin layout meant to grow into lanes, jungle, items, and netcode.

mod abilities;
mod ai;
mod camera;
mod combat;
mod components;
mod healthbars;
mod input;
mod map;
mod movement;
mod progression;
mod resources;
mod ui;
mod units;
mod waves;

use bevy::prelude::*;

use abilities::AbilitiesPlugin;
use ai::AiPlugin;
use camera::CameraPlugin;
use combat::CombatPlugin;
use healthbars::HealthBarPlugin;
use input::InputPlugin;
use map::MapPlugin;
use movement::MovementPlugin;
use progression::ProgressionPlugin;
use resources::ResourcesPlugin;
use ui::UiPlugin;
use units::UnitsPlugin;
use waves::WavesPlugin;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Labyrinth".into(),
                    resolution: (1600, 900).into(),
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins((
            ResourcesPlugin,
            MapPlugin,
            UnitsPlugin,
            MovementPlugin,
            CombatPlugin,
            ProgressionPlugin,
            InputPlugin,
            AbilitiesPlugin,
            AiPlugin,
            WavesPlugin,
            CameraPlugin,
            HealthBarPlugin,
            UiPlugin,
        ))
        .run();
}
