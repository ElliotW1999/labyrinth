//! Periodic creep wave spawns for all three lanes.

use bevy::prelude::*;

use crate::components::{Lane, Team};
use crate::resources::{MatchConfig, SharedAssets};
use crate::units::spawn_creep;

pub struct WavesPlugin;

impl Plugin for WavesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WaveTimer>()
            .add_systems(
                Startup,
                spawn_opening_wave
                    .after(crate::resources::load_shared_assets)
                    .run_if(crate::net::is_sim_authority),
            )
            .add_systems(Update, spawn_waves.run_if(crate::net::is_sim_authority));
    }
}

#[derive(Resource)]
pub(crate) struct WaveTimer(pub Timer);

impl Default for WaveTimer {
    fn default() -> Self {
        Self(Timer::from_seconds(30.0, TimerMode::Repeating))
    }
}

fn spawn_opening_wave(mut commands: Commands, assets: Res<SharedAssets>, config: Res<MatchConfig>) {
    spawn_wave_set(&mut commands, &assets, config.creeps_per_wave);
}

fn spawn_waves(
    time: Res<Time>,
    mut timer: ResMut<WaveTimer>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    config: Res<MatchConfig>,
) {
    // Keep timer interval in sync if config changes later.
    if (timer.0.duration().as_secs_f32() - config.creep_wave_interval).abs() > 0.1 {
        timer.0.set_duration(std::time::Duration::from_secs_f32(
            config.creep_wave_interval,
        ));
    }

    timer.0.tick(time.delta());
    if timer.0.just_finished() {
        spawn_wave_set(&mut commands, &assets, config.creeps_per_wave);
    }
}

fn spawn_wave_set(commands: &mut Commands, assets: &SharedAssets, count: usize) {
    for lane in [Lane::Top, Lane::Mid, Lane::Bot] {
        for i in 0..count {
            let offset = Vec3::new(i as f32 * 1.2, 0.0, i as f32 * 0.3);
            spawn_creep(
                commands,
                assets,
                Team::Radiant,
                lane,
                radiant_spawn(lane) + offset,
            );
            spawn_creep(
                commands,
                assets,
                Team::Dire,
                lane,
                dire_spawn(lane) - offset,
            );
        }
    }
}

fn radiant_spawn(lane: Lane) -> Vec3 {
    match lane {
        Lane::Mid => Vec3::new(-45.0, 0.0, -45.0),
        Lane::Top => Vec3::new(-48.0, 0.0, -40.0),
        Lane::Bot => Vec3::new(-40.0, 0.0, -48.0),
    }
}

fn dire_spawn(lane: Lane) -> Vec3 {
    match lane {
        Lane::Mid => Vec3::new(45.0, 0.0, 45.0),
        Lane::Top => Vec3::new(45.0, 0.0, 48.0),
        Lane::Bot => Vec3::new(48.0, 0.0, 45.0),
    }
}
