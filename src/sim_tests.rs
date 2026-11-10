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

// --- Compulsions (fear, taunt, hypnosis): whole-sim behaviour ---

mod compulsions {
    use super::*;
    use crate::components::{
        AbilityLoadout, AttackTarget, CollisionRadius, Creep, Mana, MoveTarget,
    };
    use crate::status::{StatusEffect, StatusEffects};
    use crate::unit_commands::{CommandQueue, IssueCommand, UnitCommand};

    fn spawn_creep_at(app: &mut App, team: Team, pos: Vec3) -> Entity {
        app.world_mut()
            .resource_scope(|world, assets: Mut<SharedAssets>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                crate::units::spawn_creep(&mut commands, &assets, team, Lane::Mid, pos, false);
                queue.apply(world);
            });
        app.world_mut()
            .query_filtered::<(Entity, &Transform), With<Creep>>()
            .iter(app.world())
            .find(|(_, tf)| flat_dist(tf.translation, pos) < 0.01)
            .map(|(e, _)| e)
            .expect("spawned creep")
    }

    fn spawn_hero_at(app: &mut App, team: Team, pos: Vec3, local: bool) -> Entity {
        app.world_mut()
            .resource_scope(|world, assets: Mut<SharedAssets>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                let hero = crate::units::spawn_hero_entity(
                    &mut commands,
                    &assets,
                    team,
                    local,
                    if local { 0 } else { 1 },
                    pos,
                    HeroId::QueenOfPain,
                );
                queue.apply(world);
                hero
            })
    }

    fn give(app: &mut App, unit: Entity, effect: StatusEffect) {
        app.world_mut()
            .get_mut::<StatusEffects>(unit)
            .unwrap()
            .effects
            .push(effect);
    }

    fn pos(app: &App, unit: Entity) -> Vec3 {
        app.world().get::<Transform>(unit).unwrap().translation
    }

    fn run(app: &mut App, frames: u32) {
        for _ in 0..frames {
            app.update();
        }
    }

    fn health(app: &App, unit: Entity) -> f32 {
        app.world().get::<Health>(unit).unwrap().current
    }

    fn flat_dist(a: Vec3, b: Vec3) -> f32 {
        Vec2::new(a.x - b.x, a.z - b.z).length()
    }

    #[test]
    fn feared_idle_unit_flees_from_source() {
        let mut app = sim_app();
        let source = spawn_hero_at(&mut app, Team::Dire, Vec3::new(-300.0, 0.0, 0.0), true);
        let unit = spawn_hero_at(&mut app, Team::Radiant, Vec3::ZERO, false);
        app.update();
        assert!(app.world().get::<MoveTarget>(unit).is_none(), "idle unit");
        give(&mut app, unit, StatusEffect::fear(10.0, Some(source)));
        run(&mut app, 60);
        let at = pos(&app, unit);
        assert!(at.x > 100.0, "ran away along +x, at {at:?}");
        assert!(at.z.abs() < 60.0, "ran straight away, at {at:?}");
        assert!(flat_dist(at, pos(&app, source)) > 400.0);
    }

    #[test]
    fn fear_from_phased_source_flees_away_not_backward() {
        let mut app = sim_app();
        let source = spawn_hero_at(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0), true);
        give(&mut app, source, StatusEffect::phased(30.0));
        let unit = spawn_hero_at(&mut app, Team::Radiant, Vec3::ZERO, false);
        app.update();
        give(&mut app, unit, StatusEffect::fear(10.0, Some(source)));
        run(&mut app, 60);
        let at = pos(&app, unit);
        assert!(
            at.x < -100.0,
            "ran away from the source along -x, at {at:?}"
        );
        assert!(at.z.abs() < 60.0, "not along its facing, at {at:?}");
    }

    #[test]
    fn hypnotized_unit_walks_to_source_and_stops_at_contact() {
        let mut app = sim_app();
        let source = spawn_hero_at(&mut app, Team::Dire, Vec3::ZERO, true);
        let unit = spawn_hero_at(&mut app, Team::Radiant, Vec3::new(-600.0, 0.0, 0.0), false);
        app.update();
        give(&mut app, unit, StatusEffect::hypnotized(30.0, source));
        let reach = app.world().get::<CollisionRadius>(unit).unwrap().0
            + app.world().get::<CollisionRadius>(source).unwrap().0;
        run(&mut app, 60 * 4);
        let gap = flat_dist(pos(&app, unit), pos(&app, source));
        assert!(
            gap < reach + 30.0,
            "reached contact, gap {gap} reach {reach}"
        );
        assert!(
            pos(&app, unit).x < pos(&app, source).x,
            "did not walk through"
        );
        let before = pos(&app, unit);
        run(&mut app, 30);
        assert!(
            flat_dist(before, pos(&app, unit)) < 5.0,
            "stays put in contact"
        );
        assert_eq!(
            health(&app, source),
            app.world().get::<Health>(source).unwrap().max
        );
    }

    #[test]
    fn unit_stops_running_when_fear_ends() {
        let mut app = sim_app();
        let source = spawn_hero_at(&mut app, Team::Dire, Vec3::new(-300.0, 0.0, 0.0), true);
        let unit = spawn_hero_at(&mut app, Team::Radiant, Vec3::ZERO, false);
        app.update();
        give(&mut app, unit, StatusEffect::fear(0.5, Some(source)));
        run(&mut app, 60);
        assert!(
            !app.world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .has_status("fear")
        );
        let at_end = pos(&app, unit);
        run(&mut app, 60);
        assert!(
            flat_dist(at_end, pos(&app, unit)) < 5.0,
            "the unit keeps running after the fear ended: {at_end:?} -> {:?}",
            pos(&app, unit)
        );
    }

    #[test]
    fn rooted_feared_unit_does_not_move() {
        let mut app = sim_app();
        let source = spawn_hero_at(&mut app, Team::Dire, Vec3::new(-300.0, 0.0, 0.0), true);
        let unit = spawn_hero_at(&mut app, Team::Radiant, Vec3::ZERO, false);
        app.update();
        give(&mut app, unit, StatusEffect::rooted(10.0));
        give(&mut app, unit, StatusEffect::fear(10.0, Some(source)));
        run(&mut app, 60);
        assert!(flat_dist(pos(&app, unit), Vec3::ZERO) < 1.0);
        assert!(app.world().get::<MoveTarget>(unit).is_none());
    }

    #[test]
    fn orders_dropped_while_compelled() {
        let mut app = sim_app();
        let source = spawn_hero_at(&mut app, Team::Dire, Vec3::new(-300.0, 0.0, 0.0), false);
        let hero = spawn_hero_at(&mut app, Team::Radiant, Vec3::ZERO, true);
        app.update();
        give(&mut app, hero, StatusEffect::fear(0.5, Some(source)));
        app.world_mut().write_message(IssueCommand {
            unit: hero,
            command: UnitCommand::Move {
                destination: Vec3::new(0.0, 0.0, 3000.0),
            },
            queue: false,
        });
        run(&mut app, 5);
        let queue = app.world().get::<CommandQueue>(hero).unwrap();
        assert!(
            queue.current.is_none() && queue.queued.is_empty(),
            "dropped"
        );
        assert!(pos(&app, hero).z.abs() < 40.0, "did not head for the order");

        // Once the fear ends, orders work again.
        run(&mut app, 60);
        assert!(
            !app.world()
                .get::<StatusEffects>(hero)
                .unwrap()
                .has_status("fear")
        );
        app.world_mut().write_message(IssueCommand {
            unit: hero,
            command: UnitCommand::Move {
                destination: Vec3::new(0.0, 0.0, 3000.0),
            },
            queue: false,
        });
        run(&mut app, 2);
        assert!(
            app.world()
                .get::<CommandQueue>(hero)
                .unwrap()
                .current
                .is_some()
        );
    }

    #[test]
    fn taunted_creep_attacks_only_source_even_when_others_closer() {
        let mut app = sim_app();
        let creep = spawn_creep_at(&mut app, Team::Dire, Vec3::ZERO);
        let near = spawn_creep_at(&mut app, Team::Radiant, Vec3::new(70.0, 0.0, 0.0));
        let taunter = spawn_hero_at(&mut app, Team::Radiant, Vec3::new(450.0, 0.0, 0.0), true);
        app.update();
        // The near creep would otherwise be the target; keep it alive and in place.
        give(&mut app, near, StatusEffect::rooted(60.0));
        give(&mut app, creep, StatusEffect::taunted(60.0, taunter));
        let near_before = health(&app, near);
        let taunter_before = health(&app, taunter);
        // While it walks to the out-of-range taunter, the creep next to it is left alone.
        run(&mut app, 60);
        assert_eq!(health(&app, near), near_before, "nobody else was hit");
        run(&mut app, 60 * 4);
        assert_eq!(
            app.world().get::<AttackTarget>(creep).map(|t| t.0),
            Some(taunter)
        );
        assert!(health(&app, taunter) < taunter_before, "taunter was hit");
        assert_eq!(health(&app, near), near_before, "nobody else was hit");
    }

    #[test]
    fn taunted_hero_cannot_cast_or_use_items() {
        fn mana_after_cast(taunted: bool) -> f32 {
            let mut app = sim_app();
            let taunter = spawn_creep_at(&mut app, Team::Dire, Vec3::new(2000.0, 0.0, 0.0));
            let hero = spawn_hero_at(&mut app, Team::Radiant, Vec3::ZERO, true);
            app.update();
            for slot in &mut app
                .world_mut()
                .get_mut::<AbilityLoadout>(hero)
                .unwrap()
                .slots
            {
                slot.rank = 1;
            }
            if taunted {
                give(&mut app, hero, StatusEffect::taunted(60.0, taunter));
                let statuses = app.world().get::<StatusEffects>(hero).unwrap();
                assert!(!statuses.can_cast() && statuses.blocks_items());
                assert!(statuses.can_move() && statuses.can_attack());
            }
            let before = app.world().get::<Mana>(hero).unwrap().current;
            app.world_mut()
                .write_message(ActivationRequest::ability(hero, 2, CastTarget::None));
            run(&mut app, 30);
            before - app.world().get::<Mana>(hero).unwrap().current
        }
        assert!(
            mana_after_cast(false) > 1.0,
            "control: the cast spends mana"
        );
        assert!(mana_after_cast(true) < 1.0, "taunted: no cast");
    }

    #[test]
    fn taunt_ends_when_source_dies() {
        let mut app = sim_app();
        let creep = spawn_creep_at(&mut app, Team::Dire, Vec3::ZERO);
        let taunter = spawn_hero_at(&mut app, Team::Radiant, Vec3::new(900.0, 0.0, 0.0), true);
        app.update();
        give(&mut app, creep, StatusEffect::taunted(60.0, taunter));
        run(&mut app, 5);
        assert!(
            app.world()
                .get::<StatusEffects>(creep)
                .unwrap()
                .has_status("taunt")
        );
        app.world_mut().get_mut::<Health>(taunter).unwrap().current = 0.0;
        run(&mut app, 10);
        let statuses = app.world().get::<StatusEffects>(creep).unwrap();
        assert!(
            !statuses.has_status("taunt"),
            "taunt removed with its source"
        );
        assert!(statuses.compulsion().is_none());
        assert!(app.world().get::<AttackTarget>(creep).is_none());
    }

    #[test]
    fn feared_unit_cannot_attack_or_cast() {
        let mut app = sim_app();
        let source = spawn_hero_at(&mut app, Team::Dire, Vec3::new(-2000.0, 0.0, 0.0), true);
        let creep = spawn_creep_at(&mut app, Team::Dire, Vec3::ZERO);
        let victim = spawn_creep_at(&mut app, Team::Radiant, Vec3::new(70.0, 0.0, 0.0));
        app.update();
        give(&mut app, victim, StatusEffect::rooted(60.0));
        give(&mut app, creep, StatusEffect::fear(60.0, Some(source)));
        let statuses = app.world().get::<StatusEffects>(creep).unwrap();
        assert!(!statuses.can_attack() && !statuses.can_cast() && statuses.blocks_items());
        assert!(statuses.can_move());
        let before = health(&app, victim);
        run(&mut app, 60 * 2);
        assert_eq!(
            health(&app, victim),
            before,
            "a feared unit does not attack"
        );
        assert!(pos(&app, creep).x > 200.0, "it ran instead");
    }

    #[test]
    fn taunted_unit_never_falls_back_to_the_nearest_enemy() {
        fn near_damage(taunted: bool) -> f32 {
            let mut app = sim_app();
            let creep = spawn_creep_at(&mut app, Team::Dire, Vec3::ZERO);
            let near = spawn_creep_at(&mut app, Team::Radiant, Vec3::new(70.0, 0.0, 0.0));
            let taunter = spawn_hero_at(&mut app, Team::Radiant, Vec3::new(900.0, 0.0, 0.0), true);
            app.update();
            // Rooted, so the taunter stays out of range and only the fallback could hit `near`.
            give(&mut app, near, StatusEffect::rooted(60.0));
            give(&mut app, creep, StatusEffect::rooted(60.0));
            if taunted {
                give(&mut app, creep, StatusEffect::taunted(60.0, taunter));
            }
            let before = health(&app, near);
            run(&mut app, 120);
            before - health(&app, near)
        }
        assert!(
            near_damage(false) > 1.0,
            "control: it hits the creep next to it"
        );
        assert_eq!(
            near_damage(true),
            0.0,
            "taunted: only the taunter may be hit"
        );
    }

    #[test]
    fn creeps_do_not_acquire_invisible_enemy() {
        fn hero_damage(invisible: bool) -> (f32, bool) {
            let mut app = sim_app();
            let creep = spawn_creep_at(&mut app, Team::Dire, Vec3::ZERO);
            let hero = spawn_hero_at(&mut app, Team::Radiant, Vec3::new(120.0, 0.0, 0.0), true);
            app.update();
            give(&mut app, creep, StatusEffect::rooted(60.0));
            if invisible {
                give(&mut app, hero, StatusEffect::invisibility(60.0, true));
            }
            let before = health(&app, hero);
            run(&mut app, 120);
            (
                before - health(&app, hero),
                app.world().get::<AttackTarget>(creep).is_some(),
            )
        }
        let (damage, targeted) = hero_damage(false);
        assert!(
            damage > 1.0 && targeted,
            "control: the creep attacks the hero"
        );
        let (damage, targeted) = hero_damage(true);
        assert_eq!(damage, 0.0, "invisible hero is left alone");
        assert!(!targeted, "and never acquired");
    }
}
