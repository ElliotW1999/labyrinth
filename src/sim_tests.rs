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

/// The match lifecycle plugins (hero select → map generation → reset) on top of the
/// gameplay sim, with the asset stores the map spawners write to.
fn match_app() -> App {
    let mut app = sim_app();
    app.insert_resource(crate::net::NetConfig {
        mode: crate::net::NetMode::Offline,
        addr: "127.0.0.1:7777".parse().unwrap(),
    })
    .init_resource::<crate::net::NetSession>()
    .init_resource::<crate::resources::MatchConfig>()
    .init_resource::<crate::fog::FogFreeZones>()
    .init_resource::<crate::picking::HoveredUnit>()
    .init_resource::<crate::picking::SelectedUnit>()
    .init_resource::<crate::items::ShopUndoLog>()
    .init_resource::<crate::items::InventoryContextMenu>()
    .init_resource::<Assets<Mesh>>()
    .init_resource::<Assets<StandardMaterial>>()
    .add_plugins((
        crate::map::MapPlugin,
        crate::heroes::HeroesPlugin,
        crate::waves::WavesPlugin,
        crate::obstacle_course::ObstacleCoursePlugin,
    ));
    app
}

fn count<C: Component>(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<C>>()
        .iter(app.world())
        .count()
}

fn pick_hero(app: &mut App, hero: HeroId) {
    app.world_mut()
        .resource_mut::<crate::heroes::LocalHeroChoice>()
        .hero = Some(hero);
    for _ in 0..3 {
        app.update();
    }
}

#[test]
fn map_is_generated_only_after_a_hero_is_picked_and_resets_on_change_hero() {
    use crate::components::{Ancient, Creep, Obstacle, PlayerHero, Tower};
    use crate::heroes::{HeroKind, HeroSelectRoot};
    use crate::map::{MapState, ResetMatchRequest};

    let mut app = match_app();
    for _ in 0..3 {
        app.update();
    }
    // Hero select is up and nothing of the battlefield exists yet.
    assert_eq!(count::<HeroSelectRoot>(&mut app), 1);
    assert!(!app.world().resource::<MapState>().generated);
    assert_eq!(count::<Tower>(&mut app), 0);
    assert_eq!(count::<Ancient>(&mut app), 0);
    assert_eq!(count::<Obstacle>(&mut app), 0);
    assert_eq!(count::<Creep>(&mut app), 0);

    pick_hero(&mut app, HeroId::Vanguard);
    assert!(app.world().resource::<MapState>().generated);
    assert_eq!(count::<HeroSelectRoot>(&mut app), 0);
    assert_eq!(count::<PlayerHero>(&mut app), 1);
    let towers = count::<Tower>(&mut app);
    let obstacles = count::<Obstacle>(&mut app);
    let creeps = count::<Creep>(&mut app);
    assert!(towers > 0 && obstacles > 0 && creeps > 0);
    assert_eq!(count::<Ancient>(&mut app), 2);

    // Change Hero: everything is torn down and hero select comes back.
    app.world_mut()
        .write_message(ResetMatchRequest { change_hero: true });
    app.update();
    app.update();
    assert!(!app.world().resource::<MapState>().generated);
    assert_eq!(count::<HeroSelectRoot>(&mut app), 1);
    assert_eq!(count::<PlayerHero>(&mut app), 0);
    assert_eq!(count::<Tower>(&mut app), 0);
    assert_eq!(count::<Obstacle>(&mut app), 0);
    assert_eq!(count::<Creep>(&mut app), 0);

    // Picking again rebuilds the map from its initial state, with the new hero.
    pick_hero(&mut app, HeroId::Sniper);
    assert_eq!(count::<Tower>(&mut app), towers);
    assert_eq!(count::<Obstacle>(&mut app), obstacles);
    assert_eq!(count::<Creep>(&mut app), creeps);
    assert_eq!(count::<Ancient>(&mut app), 2);
    let kinds: Vec<HeroId> = app
        .world_mut()
        .query_filtered::<&HeroKind, With<PlayerHero>>()
        .iter(app.world())
        .map(|kind| kind.0)
        .collect();
    assert_eq!(kinds, vec![HeroId::Sniper]);

    // New Game keeps the hero and regenerates the map without the select screen.
    app.world_mut()
        .write_message(ResetMatchRequest { change_hero: false });
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(count::<HeroSelectRoot>(&mut app), 0);
    assert_eq!(count::<PlayerHero>(&mut app), 1);
    assert_eq!(count::<Tower>(&mut app), towers);
}
