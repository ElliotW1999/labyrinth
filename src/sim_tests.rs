//! Whole-simulation regression tests: the real gameplay plugins scheduled together, so
//! cross-plugin ordering (command buffers, despawns) is exercised the way the game runs it.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use crate::actions::ActivationRequest;
use crate::components::CastTarget;
use crate::components::Lane;
use crate::components::{Health, Team};
use crate::heroes::HeroId;
use crate::resources::SharedAssets;

fn sim_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            1.0 / 60.0,
        )))
        .init_resource::<SharedAssets>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<crate::items::ShopUiState>()
        .add_plugins((
            crate::movement::MovementPlugin,
            crate::unit_commands::UnitCommandsPlugin,
            crate::combat::CombatPlugin,
            crate::status::StatusPlugin,
            crate::abilities::AbilitiesPlugin,
            crate::ai::AiPlugin,
        ));
    app
}

/// Spawns a Queen of Pain hero at the origin and a Dire creep wave walking down its lane
/// (no enemy in range, so the creep AI re-orders `MoveTarget` every frame), then casts
/// Scream of Pain on `cast_frame`, and on `kill_frame` zeroes the health of every other
/// creep (as if something else killed them while the projectiles were in flight).
fn run_scream_scenario(cast_frame: u32, kill_frame: u32) {
    let mut app = sim_app();
    let hero = app
        .world_mut()
        .resource_scope(|world, assets: Mut<SharedAssets>| {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            let hero = crate::units::spawn_hero_entity(
                &mut commands,
                &assets,
                Team::Radiant,
                true,
                0,
                Vec3::ZERO,
                HeroId::QueenOfPain,
            );
            for i in 0..6 {
                crate::units::spawn_creep(
                    &mut commands,
                    &assets,
                    Team::Dire,
                    Lane::Mid,
                    Vec3::new(150.0 + 60.0 * i as f32, 0.0, 40.0 * (i % 2) as f32),
                    i % 3 == 0,
                );
            }
            queue.apply(world);
            hero
        });
    app.update();
    for slot in &mut app
        .world_mut()
        .get_mut::<crate::components::AbilityLoadout>(hero)
        .unwrap()
        .slots
    {
        slot.rank = 1;
    }
    let creeps: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Team, &crate::components::Creep)>()
        .iter(app.world())
        .filter(|(_, team, _)| **team == Team::Dire)
        .map(|(entity, ..)| entity)
        .collect();
    // Low enough that one scream projectile (90 magical) kills each of them.
    for &creep in &creeps {
        app.world_mut().get_mut::<Health>(creep).unwrap().current = 30.0;
    }

    for frame in 0..60 {
        if frame == cast_frame {
            app.world_mut()
                .write_message(ActivationRequest::ability(hero, 2, CastTarget::None));
        }
        if frame == kill_frame {
            for &creep in creeps.iter().step_by(2) {
                if let Some(mut health) = app.world_mut().get_mut::<Health>(creep) {
                    health.current = 0.0;
                }
            }
        }
        app.update();
    }
    for creep in creeps {
        assert!(
            app.world().get_entity(creep).is_err(),
            "cast {cast_frame} / kill {kill_frame}: every creep died"
        );
    }
}

/// Regression: casting Scream of Pain at walking lane creeps that die (to the scream or
/// anything else) while its projectiles are in flight panicked with "Entity despawned":
/// `despawn_dead` was unordered with the creep AI, whose queued `MoveTarget` insert then
/// landed on the despawned creep.
#[test]
fn scream_of_pain_at_dying_lane_creeps_does_not_panic() {
    for cast_frame in 0..2 {
        for kill_frame in cast_frame..cast_frame + 10 {
            run_scream_scenario(cast_frame, kill_frame);
        }
    }
}
