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
        .write_message(ResetMatchRequest::classic(true));
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
        .write_message(ResetMatchRequest::classic(false));
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(count::<HeroSelectRoot>(&mut app), 0);
    assert_eq!(count::<PlayerHero>(&mut app), 1);
    assert_eq!(count::<Tower>(&mut app), towers);
}

#[test]
fn classic_new_game_is_unchanged_with_story_plugin() {
    use crate::components::{Ancient, Creep, PlayerHero, Tower};
    use crate::map::GameMode;

    let mut app = match_app();
    app.add_plugins(crate::story::StoryPlugin);
    pick_hero(&mut app, HeroId::Vanguard);
    assert_eq!(*app.world().resource::<GameMode>(), GameMode::Classic);
    assert!(count::<Tower>(&mut app) > 0);
    assert_eq!(count::<Ancient>(&mut app), 2);
    assert!(count::<Creep>(&mut app) > 0);
    assert_eq!(count::<PlayerHero>(&mut app), 1);
}

#[test]
fn story_mode_does_not_build_the_classic_map() {
    use crate::components::{Ancient, Creep, Obstacle, PlayerHero, Tower};
    use crate::map::{GameMode, MapState, ResetMatchRequest};

    let mut app = match_app();
    app.add_plugins(crate::story::StoryPlugin);
    app.world_mut().write_message(ResetMatchRequest {
        change_hero: false,
        mode: GameMode::Story,
    });
    for _ in 0..2 {
        app.update();
    }
    pick_hero(&mut app, HeroId::Vanguard);
    assert_eq!(*app.world().resource::<GameMode>(), GameMode::Story);
    assert!(app.world().resource::<MapState>().generated);
    assert_eq!(count::<Tower>(&mut app), 0);
    assert_eq!(count::<Ancient>(&mut app), 0);
    // Only the safe camp's walls exist: no trees, no lane creeps.
    assert_eq!(
        count::<Obstacle>(&mut app),
        crate::story::layouts::RoomLayoutId::Camp
            .layout()
            .obstacles()
            .len()
    );
    assert_eq!(count::<Creep>(&mut app), 0);
    assert_eq!(count::<PlayerHero>(&mut app), 1);
    // The hero starts in the safe room, not at the classic base.
    let pos = app
        .world_mut()
        .query_filtered::<&Transform, With<PlayerHero>>()
        .single(app.world())
        .unwrap()
        .translation;
    assert!(pos.xz().distance(crate::story::SAFE_ENTRY.xz()) < 1.0);
    // No lane waves arrive in Story Mode either.
    for _ in 0..(31 * 60) {
        app.update();
    }
    assert_eq!(count::<Creep>(&mut app), 0);
}

#[test]
fn change_hero_keeps_the_current_mode_and_new_game_returns_to_classic() {
    use crate::components::{PlayerHero, Tower};
    use crate::map::{GameMode, ResetMatchRequest};

    let mut app = match_app();
    app.add_plugins(crate::story::StoryPlugin);
    app.world_mut().write_message(ResetMatchRequest {
        change_hero: false,
        mode: GameMode::Story,
    });
    for _ in 0..2 {
        app.update();
    }
    pick_hero(&mut app, HeroId::Vanguard);

    // Change Hero in Story keeps Story.
    let current = *app.world().resource::<GameMode>();
    app.world_mut().write_message(ResetMatchRequest {
        change_hero: true,
        mode: current,
    });
    app.update();
    app.update();
    assert_eq!(*app.world().resource::<GameMode>(), GameMode::Story);
    pick_hero(&mut app, HeroId::Sniper);
    assert_eq!(count::<Tower>(&mut app), 0);

    // New Game returns to the classic map with the same hero.
    app.world_mut()
        .write_message(ResetMatchRequest::classic(false));
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(*app.world().resource::<GameMode>(), GameMode::Classic);
    assert!(count::<Tower>(&mut app) > 0);
    assert_eq!(count::<PlayerHero>(&mut app), 1);
}

// --- Story Mode: rooms, lives, rewards ---

mod story {
    use super::*;
    use crate::components::{
        AttackTarget, CastTarget, Creep, Downed, Ground, HeroProgress, Mana, MoveTarget,
        PlayerHero, PlayerWallet,
    };
    use crate::items::{BOTTLE_SLOT, Inventory, ItemId, ItemInstance, ItemShop, NEUTRAL_SLOT};
    use crate::map::{GameMode, ResetMatchRequest};
    use crate::navigation::NavPath;
    use crate::story::lives::HeroLives;
    use crate::story::pickups::ItemPickup;
    use crate::story::rooms::{FINAL_DEPTH, RoomId};
    use crate::story::{
        ExitPad, RoomCompletedEvent, RoomHostile, StoryPhase, StoryRun, StoryRunEndedEvent,
        StorySeed,
    };

    fn story_app() -> App {
        let mut app = match_app();
        app.add_plugins((crate::items::ItemsPlugin, crate::story::StoryPlugin));
        app.insert_resource(StorySeed(Some(7)));
        app.world_mut().write_message(ResetMatchRequest {
            change_hero: false,
            mode: GameMode::Story,
        });
        for _ in 0..2 {
            app.update();
        }
        pick_hero(&mut app, HeroId::Vanguard);
        app
    }

    fn run(app: &mut App, frames: u32) {
        for _ in 0..frames {
            app.update();
        }
    }

    fn hero(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<PlayerHero>>()
            .single(app.world())
            .unwrap()
    }

    fn phase(app: &App) -> StoryPhase {
        app.world().resource::<StoryRun>().phase
    }

    fn pos_of(app: &App, entity: Entity) -> Vec3 {
        app.world().get::<Transform>(entity).unwrap().translation
    }

    fn put_at(app: &mut App, entity: Entity, xz: Vec2) {
        let mut tf = app.world_mut().get_mut::<Transform>(entity).unwrap();
        tf.translation.x = xz.x;
        tf.translation.z = xz.y;
    }

    /// Pad centres of the current room as (slot, armed, position).
    fn pads(app: &mut App) -> Vec<(usize, bool, Vec3)> {
        let mut found: Vec<_> = app
            .world_mut()
            .query::<(&ExitPad, &Transform)>()
            .iter(app.world())
            .map(|(pad, tf)| (pad.slot, pad.armed, tf.translation))
            .collect();
        found.sort_by_key(|(slot, ..)| *slot);
        found
    }

    /// Jump straight into `room` as if its exit had been taken (the draw is seeded, but the
    /// tests below want a specific room).
    fn enter_room(app: &mut App, room: RoomId) {
        app.world_mut().resource_mut::<StoryRun>().phase = StoryPhase::Transition(room);
        run(app, 3);
    }

    fn kill_hostiles(app: &mut App) {
        let hostiles: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<RoomHostile>>()
            .iter(app.world())
            .collect();
        for hostile in hostiles {
            app.world_mut().get_mut::<Health>(hostile).unwrap().current = 0.0;
        }
    }

    fn completions(app: &App) -> usize {
        app.world()
            .resource::<Messages<RoomCompletedEvent>>()
            .iter_current_update_messages()
            .count()
    }

    fn gold(app: &App, hero: Entity) -> u32 {
        app.world().get::<PlayerWallet>(hero).unwrap().gold
    }

    #[test]
    fn story_start_builds_safe_room() {
        let mut app = story_app();
        let hero = hero(&mut app);
        assert_eq!(phase(&app), StoryPhase::Cleared);
        assert_eq!(count::<Ground>(&mut app), 1);
        assert_eq!(count::<ItemShop>(&mut app), 1);
        assert_eq!(count::<RoomHostile>(&mut app), 0);
        let pads = pads(&mut app);
        assert_eq!(pads.len(), 2);
        assert!(pads.iter().all(|(_, armed, _)| *armed), "both exits armed");
        let run = app.world().resource::<StoryRun>();
        assert!(run.exits.iter().all(Option::is_some));
        assert_ne!(run.exits[0], run.exits[1], "two different rooms");

        let at = pos_of(&app, hero);
        assert!(at.xz().distance(crate::story::SAFE_ENTRY.xz()) < 1.0);
        assert_eq!(app.world().get::<HeroLives>(hero).unwrap().remaining, 4);
        let inventory = app.world().get::<Inventory>(hero).unwrap();
        let bottle = inventory.slots[BOTTLE_SLOT].expect("the bottle is granted");
        assert_eq!(bottle.id, ItemId::Bottle);
        assert_eq!(bottle.cooldown.charges, Some(3));
        assert!(inventory.slots[NEUTRAL_SLOT].is_none());
        assert_eq!(gold(&app, hero), crate::units::STARTING_GOLD);
        assert_eq!(
            app.world().get::<HeroProgress>(hero).unwrap().level,
            crate::units::STARTING_HERO_LEVEL
        );
        // A safe room never completes.
        assert_eq!(completions(&app), 0);
    }

    #[test]
    fn dwelling_on_exit_pad_loads_next_room_and_tears_down_previous() {
        let mut app = story_app();
        let hero = hero(&mut app);
        let old_floor = app
            .world_mut()
            .query_filtered::<Entity, With<Ground>>()
            .single(app.world())
            .unwrap();
        let old_shop = app
            .world_mut()
            .query_filtered::<Entity, With<ItemShop>>()
            .single(app.world())
            .unwrap();
        let old_pads: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<ExitPad>>()
            .iter(app.world())
            .collect();
        let target = app.world().resource::<StoryRun>().exits[0].unwrap();
        let (_, _, pad_pos) = pads(&mut app)[0];

        put_at(&mut app, hero, pad_pos.xz());
        run(&mut app, 50);

        assert!(app.world().get_entity(old_floor).is_err());
        assert!(app.world().get_entity(old_shop).is_err());
        assert!(old_pads.iter().all(|e| app.world().get_entity(*e).is_err()));
        let run_state = app.world().resource::<StoryRun>().clone();
        assert_eq!(run_state.depth, 1);
        assert_eq!(run_state.room, Some(target));
        assert_eq!(phase(&app), StoryPhase::Active);
        assert_eq!(
            count::<RoomHostile>(&mut app) as u32,
            target.definition().hostile_count()
        );
        assert_eq!(count::<Ground>(&mut app), 1, "exactly one floor");
        assert_eq!(count::<ItemShop>(&mut app), 0);
        assert_eq!(count::<ExitPad>(&mut app), 2);
        assert!(pads(&mut app).iter().all(|(_, armed, _)| !armed));
        assert_eq!(count::<Creep>(&mut app), count::<RoomHostile>(&mut app));

        let at = pos_of(&app, hero);
        assert!(at.xz().distance(run_state.entry.xz()) < 1.0, "{at:?}");
        assert!(app.world().get::<MoveTarget>(hero).is_none());
        assert!(app.world().get::<NavPath>(hero).is_none());
        // The hero keeps what it had.
        assert_eq!(app.world().get::<HeroLives>(hero).unwrap().remaining, 4);
        assert!(app.world().get::<Inventory>(hero).unwrap().slots[BOTTLE_SLOT].is_some());
    }

    #[test]
    fn brief_pad_touch_does_not_transition() {
        let mut app = story_app();
        let hero = hero(&mut app);
        let (_, _, pad_pos) = pads(&mut app)[0];
        let away = pos_of(&app, hero);
        put_at(&mut app, hero, pad_pos.xz());
        run(&mut app, 10); // well under the dwell time
        put_at(&mut app, hero, away.xz());
        run(&mut app, 60);
        assert_eq!(phase(&app), StoryPhase::Cleared);
        assert_eq!(app.world().resource::<StoryRun>().depth, 0);
        // The dwell restarts: another short touch still does nothing.
        put_at(&mut app, hero, pad_pos.xz());
        run(&mut app, 10);
        put_at(&mut app, hero, away.xz());
        run(&mut app, 10);
        assert_eq!(phase(&app), StoryPhase::Cleared);
    }

    #[test]
    fn exits_stay_inactive_while_hostiles_live() {
        let mut app = story_app();
        let hero = hero(&mut app);
        enter_room(&mut app, RoomId::Skirmish);
        assert_eq!(phase(&app), StoryPhase::Active);
        // Stand on each pad for longer than the dwell time.
        for (_, armed, pad_pos) in pads(&mut app) {
            assert!(!armed);
            put_at(&mut app, hero, pad_pos.xz());
            run(&mut app, 45);
        }
        assert_eq!(phase(&app), StoryPhase::Active);
        assert_eq!(app.world().resource::<StoryRun>().depth, 1);
    }

    #[test]
    fn killing_all_hostiles_completes_room_once() {
        let mut app = story_app();
        let hero = hero(&mut app);
        enter_room(&mut app, RoomId::Skirmish);
        app.world_mut().get_mut::<Inventory>(hero).unwrap().slots[BOTTLE_SLOT]
            .as_mut()
            .unwrap()
            .cooldown
            .charges = Some(0);
        let before = gold(&app, hero);

        assert_eq!(count::<RoomHostile>(&mut app), 6);
        kill_hostiles(&mut app);
        let mut events = 0;
        for _ in 0..10 {
            app.update();
            events += completions(&app);
        }
        assert_eq!(events, 1, "exactly one completion event");
        assert_eq!(phase(&app), StoryPhase::Cleared);
        assert_eq!(
            app.world().get::<Inventory>(hero).unwrap().slots[BOTTLE_SLOT]
                .unwrap()
                .cooldown
                .charges,
            Some(3),
            "the bottle is refilled"
        );
        // 400 gold reward once, plus the 35 gold bounty of each of the six creeps.
        assert_eq!(
            gold(&app, hero),
            before + 400 + 6 * 35,
            "the gold reward, once"
        );
        let pads = pads(&mut app);
        assert!(pads.iter().all(|(_, armed, _)| *armed));
        assert!(
            app.world().resource::<StoryRun>().exits[0].is_some(),
            "exits are drawn for depth 2"
        );
        assert_eq!(count::<ItemPickup>(&mut app), 0, "skirmish pays gold only");
    }

    #[test]
    fn neutral_reward_pickup_equips_then_swaps_without_ping_pong() {
        let mut app = story_app();
        let hero = hero(&mut app);
        let base = {
            let e = app.world().entity(hero);
            (
                *e.get::<Health>().unwrap(),
                *e.get::<Mana>().unwrap(),
                *e.get::<crate::components::CombatStats>().unwrap(),
                *e.get::<crate::components::HeroAttributes>().unwrap(),
            )
        };
        let (base_armor, base_damage) = (base.2.armor, base.2.attack_damage);
        enter_room(&mut app, RoomId::Ambush);
        kill_hostiles(&mut app);
        run(&mut app, 5);
        assert_eq!(count::<ItemPickup>(&mut app), 1);
        let (first_id, first_pos) = {
            let mut query = app.world_mut().query::<(&ItemPickup, &Transform)>();
            let (pickup, tf) = query.single(app.world()).unwrap();
            (pickup.instance.id, tf.translation)
        };
        assert_eq!(first_id.definition().slot, crate::items::ItemSlot::Neutral);

        // Walk onto it: equipped, pickup gone.
        put_at(&mut app, hero, first_pos.xz());
        run(&mut app, 3);
        let equipped = app.world().get::<Inventory>(hero).unwrap().slots[NEUTRAL_SLOT];
        assert_eq!(equipped.map(|item| item.id), Some(first_id));
        assert_eq!(count::<ItemPickup>(&mut app), 0);
        let after_first = *app
            .world()
            .get::<crate::components::CombatStats>(hero)
            .unwrap();
        assert!(
            after_first.armor != base_armor
                || after_first.attack_damage != base_damage
                || first_id == ItemId::ArcaneRing
                || first_id == ItemId::FadedBroach,
            "the item's passives apply"
        );

        // A second neutral reward swaps with the first.
        put_at(&mut app, hero, Vec2::new(-900.0, 900.0));
        enter_room(&mut app, RoomId::Ambush);
        kill_hostiles(&mut app);
        run(&mut app, 5);
        let (second_id, second_pos) = {
            let mut query = app.world_mut().query::<(&ItemPickup, &Transform)>();
            let (pickup, tf) = query.single(app.world()).unwrap();
            (pickup.instance.id, tf.translation)
        };
        assert_ne!(
            second_id, first_id,
            "the equipped item is not offered again"
        );
        put_at(&mut app, hero, second_pos.xz());
        run(&mut app, 3);
        let equipped = app.world().get::<Inventory>(hero).unwrap().slots[NEUTRAL_SLOT];
        assert_eq!(equipped.map(|item| item.id), Some(second_id));
        // The old item lies beside it, disarmed while the hero stands on the spot.
        assert_eq!(count::<ItemPickup>(&mut app), 1);
        let dropped = {
            let mut query = app.world_mut().query::<&ItemPickup>();
            *query.single(app.world()).unwrap()
        };
        assert_eq!(dropped.instance.id, first_id);

        // No ping-pong: standing still keeps the second item equipped.
        run(&mut app, 60);
        let equipped = app.world().get::<Inventory>(hero).unwrap().slots[NEUTRAL_SLOT];
        assert_eq!(equipped.map(|item| item.id), Some(second_id));
        assert_eq!(count::<ItemPickup>(&mut app), 1);
        // Passives of the swapped-out item are gone: the stats equal the base stats plus the
        // second item alone.
        let stats = *app
            .world()
            .get::<crate::components::CombatStats>(hero)
            .unwrap();
        let (mut health, mut mana, mut reference, mut attrs) = base;
        let mut inv = Inventory::empty();
        crate::items::equip_neutral(
            &mut inv,
            &mut health,
            &mut mana,
            &mut reference,
            &mut attrs,
            ItemInstance::new(second_id),
        )
        .unwrap();
        assert_eq!(stats.armor, reference.armor);
        assert_eq!(stats.attack_damage, reference.attack_damage);
        assert_eq!(stats.move_speed, reference.move_speed);
        assert_eq!(app.world().get::<Mana>(hero).unwrap().max, mana.max);
    }

    #[test]
    fn hero_death_consumes_a_life_and_respawns_at_entry() {
        use crate::unit_commands::{IssueCommand, UnitCommand};
        let mut app = story_app();
        let hero = hero(&mut app);
        let entry = app.world().resource::<StoryRun>().entry;
        put_at(&mut app, hero, Vec2::new(500.0, 200.0));
        app.world_mut().get_mut::<Mana>(hero).unwrap().current = 0.0;
        app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
        run(&mut app, 2);
        assert_eq!(app.world().get::<HeroLives>(hero).unwrap().remaining, 3);
        assert!(app.world().get::<Downed>(hero).is_some());

        // Downed: orders and the bottle are ignored.
        app.world_mut().write_message(IssueCommand {
            unit: hero,
            command: UnitCommand::Move {
                destination: Vec3::new(0.0, 0.0, 800.0),
            },
            queue: false,
        });
        app.world_mut()
            .write_message(crate::actions::ActivationRequest::item(
                hero,
                BOTTLE_SLOT,
                CastTarget::None,
            ));
        run(&mut app, 30);
        assert!(app.world().get::<MoveTarget>(hero).is_none());
        assert_eq!(
            app.world().get::<Inventory>(hero).unwrap().slots[BOTTLE_SLOT]
                .unwrap()
                .cooldown
                .charges,
            Some(3)
        );
        assert_eq!(app.world().get::<Health>(hero).unwrap().current, 0.0);
        assert_eq!(app.world().get::<HeroLives>(hero).unwrap().remaining, 3);

        // After the respawn delay the hero is back at the entry, full.
        run(&mut app, 5 * 60);
        assert!(app.world().get::<Downed>(hero).is_none());
        let health = app.world().get::<Health>(hero).unwrap();
        assert_eq!(health.current, health.max);
        let mana = app.world().get::<Mana>(hero).unwrap();
        assert_eq!(mana.current, mana.max);
        let at = pos_of(&app, hero);
        assert!(at.xz().distance(entry.xz()) < 1.0, "{at:?} vs {entry:?}");
        assert_eq!(phase(&app), StoryPhase::Cleared);
    }

    #[test]
    fn fourth_death_is_game_over_without_respawn() {
        let mut app = story_app();
        let hero = hero(&mut app);
        for expected in [3, 2, 1] {
            app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
            run(&mut app, 2);
            assert_eq!(
                app.world().get::<HeroLives>(hero).unwrap().remaining,
                expected
            );
            assert_eq!(phase(&app), StoryPhase::Cleared, "still playing");
            run(&mut app, 5 * 60 + 5);
            assert!(app.world().get::<Downed>(hero).is_none(), "respawned");
        }
        app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
        let mut ended = Vec::new();
        for _ in 0..5 {
            app.update();
            ended.extend(
                app.world()
                    .resource::<Messages<StoryRunEndedEvent>>()
                    .iter_current_update_messages()
                    .map(|e| e.victory),
            );
        }
        assert_eq!(ended, vec![false], "one defeat event");
        assert_eq!(app.world().get::<HeroLives>(hero).unwrap().remaining, 0);
        assert_eq!(phase(&app), StoryPhase::GameOver);
        run(&mut app, 10 * 60);
        assert_eq!(app.world().get::<Health>(hero).unwrap().current, 0.0);
        assert!(app.world().get::<Downed>(hero).is_some(), "never respawns");
        assert_eq!(phase(&app), StoryPhase::GameOver);
    }

    #[test]
    fn boss_room_clear_is_victory() {
        let mut app = story_app();
        {
            let mut run_state = app.world_mut().resource_mut::<StoryRun>();
            run_state.depth = FINAL_DEPTH - 1;
        }
        enter_room(&mut app, RoomId::WarlordLair);
        assert_eq!(app.world().resource::<StoryRun>().depth, FINAL_DEPTH);
        assert_eq!(phase(&app), StoryPhase::Active);
        kill_hostiles(&mut app);
        let mut ended = Vec::new();
        for _ in 0..6 {
            app.update();
            ended.extend(
                app.world()
                    .resource::<Messages<StoryRunEndedEvent>>()
                    .iter_current_update_messages()
                    .map(|e| e.victory),
            );
        }
        assert_eq!(ended, vec![true]);
        assert_eq!(phase(&app), StoryPhase::Victory);
        assert!(
            app.world()
                .resource::<StoryRun>()
                .exits
                .iter()
                .all(Option::is_none)
        );
        assert!(
            pads(&mut app).iter().all(|(_, armed, _)| !armed),
            "no way on"
        );
    }

    #[test]
    fn transition_cancels_in_flight_activation_and_command_queue() {
        use crate::unit_commands::{CommandQueue, IssueCommand, UnitCommand};
        let mut app = story_app();
        let hero = hero(&mut app);
        enter_room(&mut app, RoomId::Skirmish);
        let enemy = app
            .world_mut()
            .query_filtered::<Entity, With<RoomHostile>>()
            .iter(app.world())
            .next()
            .unwrap();
        for (queue, destination) in [(false, 400.0), (true, 800.0)] {
            app.world_mut().write_message(IssueCommand {
                unit: hero,
                command: UnitCommand::Move {
                    destination: Vec3::new(0.0, 0.0, destination),
                },
                queue,
            });
        }
        app.world_mut()
            .write_message(crate::actions::ActivationRequest::item(
                hero,
                BOTTLE_SLOT,
                CastTarget::None,
            ));
        app.world_mut().entity_mut(hero).insert(AttackTarget(enemy));
        run(&mut app, 2);
        {
            let queue = app.world().get::<CommandQueue>(hero).unwrap();
            assert!(queue.current.is_some() || !queue.queued.is_empty());
        }

        enter_room(&mut app, RoomId::Ambush);
        let queue = app.world().get::<CommandQueue>(hero).unwrap();
        assert!(queue.current.is_none() && queue.queued.is_empty());
        assert!(app.world().get::<MoveTarget>(hero).is_none());
        assert!(app.world().get::<AttackTarget>(hero).is_none());
        assert!(
            app.world()
                .get::<crate::actions::Activation>(hero)
                .is_none()
        );
        assert!(
            app.world()
                .get::<crate::actions::QueuedActivation>(hero)
                .is_none()
        );
    }

    // --- Verification regressions (tester) ---

    fn bottle_charges(app: &App, hero: Entity) -> Option<u32> {
        app.world().get::<Inventory>(hero).unwrap().slots[BOTTLE_SLOT]
            .unwrap()
            .cooldown
            .charges
    }

    fn set_bottle_charges(app: &mut App, hero: Entity, charges: u32) {
        app.world_mut().get_mut::<Inventory>(hero).unwrap().slots[BOTTLE_SLOT]
            .as_mut()
            .unwrap()
            .cooldown
            .charges = Some(charges);
    }

    #[test]
    fn bottle_refills_only_when_a_room_completes_and_never_above_max() {
        let mut app = story_app();
        let hero = hero(&mut app);

        // The safe camp never refills: it has no completion.
        set_bottle_charges(&mut app, hero, 0);
        run(&mut app, 30);
        assert_eq!(bottle_charges(&app, hero), Some(0), "safe room: no refill");

        // Entering a hostile room does not refill either; clearing it does.
        enter_room(&mut app, RoomId::Skirmish);
        run(&mut app, 30);
        assert_eq!(phase(&app), StoryPhase::Active);
        assert_eq!(
            bottle_charges(&app, hero),
            Some(0),
            "active room: no refill"
        );
        set_bottle_charges(&mut app, hero, 1);
        kill_hostiles(&mut app);
        run(&mut app, 5);
        assert_eq!(bottle_charges(&app, hero), Some(3));

        // A full bottle stays at the maximum through the next completion.
        enter_room(&mut app, RoomId::Skirmish);
        kill_hostiles(&mut app);
        run(&mut app, 5);
        assert_eq!(bottle_charges(&app, hero), Some(3), "never above max");
    }

    #[test]
    fn hero_dying_on_the_clearing_frame_still_completes_once_and_costs_a_life() {
        let mut app = story_app();
        let hero = hero(&mut app);
        enter_room(&mut app, RoomId::Skirmish);
        set_bottle_charges(&mut app, hero, 0);
        let before = gold(&app, hero);
        kill_hostiles(&mut app);
        app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
        let mut completed = 0;
        for _ in 0..10 {
            app.update();
            completed += completions(&app);
        }
        assert_eq!(completed, 1, "exactly one completion");
        assert_eq!(app.world().get::<HeroLives>(hero).unwrap().remaining, 3);
        assert!(app.world().get::<Downed>(hero).is_some());
        assert_eq!(phase(&app), StoryPhase::Cleared);
        assert_eq!(bottle_charges(&app, hero), Some(3));
        // Six Skirmish creeps pay 35 gold each; the room reward is 400, paid once.
        assert_eq!(gold(&app, hero), before + 6 * 35 + 400, "reward paid once");
    }

    #[test]
    fn losing_the_last_life_on_the_clearing_frame_is_game_over_without_a_completion() {
        let mut app = story_app();
        let hero = hero(&mut app);
        enter_room(&mut app, RoomId::Skirmish);
        app.world_mut()
            .get_mut::<HeroLives>(hero)
            .unwrap()
            .remaining = 1;
        let before = gold(&app, hero);
        kill_hostiles(&mut app);
        app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
        let mut completed = 0;
        for _ in 0..10 {
            app.update();
            completed += completions(&app);
        }
        assert_eq!(phase(&app), StoryPhase::GameOver);
        assert_eq!(completed, 0);
        assert_eq!(
            gold(&app, hero),
            before + 6 * 35,
            "bounties only, no room reward"
        );
        assert!(
            pads(&mut app).iter().all(|(_, armed, _)| !armed),
            "no exits after the run ended"
        );
    }

    #[test]
    fn victory_is_not_overwritten_by_a_later_death() {
        let mut app = story_app();
        let hero = hero(&mut app);
        app.world_mut().resource_mut::<StoryRun>().depth = FINAL_DEPTH - 1;
        enter_room(&mut app, RoomId::WarlordLair);
        kill_hostiles(&mut app);
        run(&mut app, 5);
        assert_eq!(phase(&app), StoryPhase::Victory);
        app.world_mut()
            .get_mut::<HeroLives>(hero)
            .unwrap()
            .remaining = 1;
        app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
        run(&mut app, 5);
        assert_eq!(phase(&app), StoryPhase::Victory);
    }

    #[test]
    fn a_downed_hero_on_an_armed_exit_pad_does_not_take_it() {
        let mut app = story_app();
        let hero = hero(&mut app);
        let pad = pads(&mut app)[0].2;
        put_at(&mut app, hero, pad.xz());
        app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
        // The hero is killed before the dwell time elapses and stays dead for 5 s.
        run(&mut app, 3 * 60);
        assert!(app.world().get::<Downed>(hero).is_some());
        assert_eq!(phase(&app), StoryPhase::Cleared, "a corpse takes no exit");
        assert_eq!(app.world().resource::<StoryRun>().depth, 0);
    }

    #[test]
    fn leaving_a_room_removes_its_pickups_and_keeps_heroes_and_their_bars() {
        use crate::components::HealthBar;
        let mut app = story_app();
        let hero = hero(&mut app);
        enter_room(&mut app, RoomId::Ambush);
        kill_hostiles(&mut app);
        run(&mut app, 5);
        assert_eq!(
            count::<ItemPickup>(&mut app),
            1,
            "the reward lies in the room"
        );
        let bar = app
            .world_mut()
            .spawn((HealthBar { owner: hero }, Transform::default()))
            .id();

        enter_room(&mut app, RoomId::Skirmish);
        assert_eq!(count::<ItemPickup>(&mut app), 0, "no stale pickup");
        assert!(app.world().get_entity(hero).is_ok(), "hero survives");
        assert!(app.world().get_entity(bar).is_ok(), "health bar survives");
        assert_eq!(count::<ExitPad>(&mut app), 2, "exactly one pad pair");
        assert_eq!(count::<Ground>(&mut app), 1);
    }

    #[test]
    fn hostiles_do_not_chase_or_target_a_downed_hero() {
        let mut app = story_app();
        let hero = hero(&mut app);
        enter_room(&mut app, RoomId::Skirmish);
        // Down the hero right beside the hostiles (inside their aggro radius).
        put_at(&mut app, hero, Vec2::new(0.0, -300.0));
        app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
        run(&mut app, 3);
        assert!(app.world().get::<Downed>(hero).is_some());
        run(&mut app, 60);
        let chasing = app
            .world_mut()
            .query_filtered::<&AttackTarget, With<RoomHostile>>()
            .iter(app.world())
            .filter(|target| target.0 == hero)
            .count();
        assert_eq!(chasing, 0, "no hostile attacks the corpse");
    }

    #[test]
    fn a_pre_death_damage_over_time_cannot_kill_the_respawned_hero() {
        use crate::status::{StatusChange, StatusEffect, StatusEffectEvent};
        let mut app = story_app();
        let hero = hero(&mut app);
        let mut dot = StatusEffect::debuff("verify_dot", 30.0);
        dot.dot_interval = 1.0;
        dot.dot_timer = 1.0;
        dot.dot_per_tick = 100_000.0;
        dot.dot_ticks_left = 30;
        app.world_mut()
            .write_message(StatusEffectEvent::new(None, hero, StatusChange::Apply(dot)));
        run(&mut app, 3 * 60);
        assert!(
            app.world().get::<Downed>(hero).is_some(),
            "the DoT killed it"
        );
        assert_eq!(app.world().get::<HeroLives>(hero).unwrap().remaining, 3);
        run(&mut app, 3 * 60);
        assert!(app.world().get::<Downed>(hero).is_none(), "respawned");
        run(&mut app, 4 * 60);
        let health = app.world().get::<Health>(hero).unwrap();
        assert!(health.is_alive(), "the DoT must not outlive the death");
        assert_eq!(
            app.world().get::<HeroLives>(hero).unwrap().remaining,
            3,
            "no second life lost to the old DoT"
        );
    }

    #[test]
    fn classic_matches_never_grant_story_gear() {
        let mut app = match_app();
        app.add_plugins((crate::items::ItemsPlugin, crate::story::StoryPlugin));
        pick_hero(&mut app, HeroId::Vanguard);
        let hero = hero(&mut app);
        assert!(app.world().get::<HeroLives>(hero).is_none());
        let inventory = app.world().get::<Inventory>(hero).unwrap();
        assert!(inventory.slots[BOTTLE_SLOT].is_none());
        assert!(inventory.slots[NEUTRAL_SLOT].is_none());
        assert_eq!(phase(&app), StoryPhase::Idle);
        assert_eq!(count::<ExitPad>(&mut app), 0);
        assert_eq!(count::<RoomHostile>(&mut app), 0);
    }
}

// --- Downed heroes take no orders ---

#[test]
fn downed_unit_drops_issued_commands_and_activation_requests() {
    use crate::actions::ActivationRequest;
    use crate::components::{CastTarget, Downed, MoveTarget};
    use crate::items::{Inventory, ItemId, ItemInstance};
    use crate::unit_commands::{IssueCommand, UnitCommand};

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
            queue.apply(world);
            hero
        });
    app.world_mut().get_mut::<Inventory>(hero).unwrap().slots[0] =
        Some(ItemInstance::new(ItemId::VialOfLight));
    app.update();

    app.world_mut()
        .entity_mut(hero)
        .insert(Downed { respawn_in: 5.0 });
    app.world_mut().write_message(IssueCommand {
        unit: hero,
        command: UnitCommand::Move {
            destination: Vec3::new(0.0, 0.0, 2000.0),
        },
        queue: false,
    });
    app.world_mut()
        .write_message(ActivationRequest::item(hero, 0, CastTarget::None));
    for _ in 0..5 {
        app.update();
    }
    assert!(
        app.world().get::<MoveTarget>(hero).is_none(),
        "the move order was dropped"
    );
    let vial = app.world().get::<Inventory>(hero).unwrap().slots[0].unwrap();
    assert_eq!(vial.cooldown.remaining, 0.0, "the item was not used");

    // Once the unit is no longer downed the same inputs work.
    app.world_mut().entity_mut(hero).remove::<Downed>();
    app.world_mut().write_message(IssueCommand {
        unit: hero,
        command: UnitCommand::Move {
            destination: Vec3::new(0.0, 0.0, 2000.0),
        },
        queue: false,
    });
    app.world_mut()
        .write_message(ActivationRequest::item(hero, 0, CastTarget::None));
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().get::<MoveTarget>(hero).is_some());
    let vial = app.world().get::<Inventory>(hero).unwrap().slots[0].unwrap();
    assert!(vial.cooldown.remaining > 0.0, "the item fired");
}

// --- Hunter: lane-less creeps that walk to nearby enemies ---

mod hunter {
    use super::*;
    use crate::ai::Hunter;
    use crate::components::{AttackTarget, Creep, CreepClass, MoveTarget};
    use crate::status::{StatusEffect, StatusEffects};

    fn spawn_hunter(app: &mut App, pos: Vec3) -> Entity {
        app.world_mut()
            .resource_scope(|world, assets: Mut<SharedAssets>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                let id = crate::units::spawn_creep_with(
                    &mut commands,
                    &assets,
                    crate::units::CreepSpawn {
                        team: Team::Dire,
                        position: pos,
                        ranged: false,
                        class: CreepClass::Normal,
                        lane: None,
                    },
                );
                queue.apply(world);
                id
            })
    }

    fn spawn_target_hero(app: &mut App, pos: Vec3) -> Entity {
        app.world_mut()
            .resource_scope(|world, assets: Mut<SharedAssets>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                let hero = crate::units::spawn_hero_entity(
                    &mut commands,
                    &assets,
                    Team::Radiant,
                    false,
                    1,
                    pos,
                    HeroId::QueenOfPain,
                );
                queue.apply(world);
                hero
            })
    }

    fn at(app: &App, unit: Entity) -> Vec3 {
        app.world().get::<Transform>(unit).unwrap().translation
    }

    fn flat(a: Vec3, b: Vec3) -> f32 {
        Vec2::new(a.x - b.x, a.z - b.z).length()
    }

    fn run(app: &mut App, frames: u32) {
        for _ in 0..frames {
            app.update();
        }
    }

    #[test]
    fn hunter_walks_to_enemy_hero_in_aggro_radius_and_attacks() {
        let mut app = sim_app();
        let hero = spawn_target_hero(&mut app, Vec3::ZERO);
        let hunter = spawn_hunter(&mut app, Vec3::new(1000.0, 0.0, 0.0));
        let full = app.world().get::<Health>(hero).unwrap().current;
        let start = flat(at(&app, hunter), at(&app, hero));
        run(&mut app, 60);
        assert!(
            flat(at(&app, hunter), at(&app, hero)) < start - 100.0,
            "the hunter closed in"
        );
        // The hero fights back, so watch for the exchange rather than the end state.
        let (mut attacked, mut hero_hurt) = (false, false);
        for _ in 0..600 {
            app.update();
            let Some(health) = app.world().get::<Health>(hero) else {
                break;
            };
            hero_hurt |= health.current < full;
            attacked |= app.world().get::<AttackTarget>(hunter).is_some();
        }
        assert!(attacked, "the hunter targeted the hero once in range");
        assert!(hero_hurt, "and landed hits");
    }

    #[test]
    fn hunter_ignores_enemies_outside_radius_or_invisible() {
        let mut app = sim_app();
        let radius = CreepClass::Normal.profile().aggro_radius;
        let far = spawn_target_hero(&mut app, Vec3::new(-radius - 400.0, 0.0, 0.0));
        let hunter = spawn_hunter(&mut app, Vec3::ZERO);
        let start = at(&app, hunter);
        run(&mut app, 120);
        assert_eq!(at(&app, hunter), start, "out of range: it idles");
        assert!(app.world().get::<MoveTarget>(hunter).is_none());

        // An untargetable enemy inside the radius is ignored too.
        app.world_mut()
            .get_mut::<StatusEffects>(far)
            .unwrap()
            .effects
            .push(StatusEffect::untargetable(60.0));
        app.world_mut()
            .get_mut::<Transform>(far)
            .unwrap()
            .translation = Vec3::new(-600.0, 0.0, 0.0);
        run(&mut app, 120);
        assert_eq!(at(&app, hunter), start, "untargetable: it idles");
        assert!(app.world().get::<MoveTarget>(hunter).is_none());
        assert!(app.world().get::<Hunter>(hunter).is_some());
        assert!(app.world().get::<Creep>(hunter).unwrap().lane.is_none());
    }

    #[test]
    fn hunter_does_not_override_compulsions() {
        let mut app = sim_app();
        let hero = spawn_target_hero(&mut app, Vec3::new(-500.0, 0.0, 0.0));
        let hunter = spawn_hunter(&mut app, Vec3::ZERO);
        app.update();
        app.world_mut()
            .get_mut::<StatusEffects>(hunter)
            .unwrap()
            .effects
            .push(StatusEffect::fear(10.0, Some(hero)));
        run(&mut app, 120);
        let now = at(&app, hunter);
        // Hunting would have taken it toward the hero (-x); fear sends it the other way.
        assert!(
            now.x > 300.0,
            "the feared hunter ran away from the hero: {now:?}"
        );
    }
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
    fn taunt_target_forces_attack_on_cast_target_not_caster() {
        let mut app = sim_app();
        // The forced attacker, its ally (the unit the taunt names) and the enemy caster.
        let attacker = spawn_creep_at(&mut app, Team::Dire, Vec3::ZERO);
        let cursed = spawn_creep_at(&mut app, Team::Dire, Vec3::new(120.0, 0.0, 0.0));
        let caster = spawn_hero_at(&mut app, Team::Radiant, Vec3::new(0.0, 0.0, 150.0), true);
        // Taunted before the first frame, so no attack on the caster is already under way.
        // The cursed unit neither walks off nor fights back, so only the taunt is at work.
        give(&mut app, cursed, StatusEffect::rooted(60.0));
        give(&mut app, cursed, StatusEffect::disarmed(60.0));
        give(&mut app, attacker, StatusEffect::taunted(60.0, cursed));
        let cursed_before = health(&app, cursed);
        let caster_before = health(&app, caster);
        run(&mut app, 60 * 3);
        assert_eq!(
            app.world().get::<AttackTarget>(attacker).map(|t| t.0),
            Some(cursed)
        );
        assert!(
            health(&app, cursed) < cursed_before,
            "the attacker hit its own ally"
        );
        assert_eq!(
            health(&app, caster),
            caster_before,
            "the caster is left alone"
        );
    }

    #[test]
    fn taunt_target_ends_when_target_dies() {
        let mut app = sim_app();
        let attacker = spawn_creep_at(&mut app, Team::Dire, Vec3::ZERO);
        let cursed = spawn_creep_at(&mut app, Team::Dire, Vec3::new(120.0, 0.0, 0.0));
        app.update();
        give(&mut app, attacker, StatusEffect::taunted(60.0, cursed));
        run(&mut app, 3);
        assert!(
            app.world()
                .get::<StatusEffects>(attacker)
                .unwrap()
                .has_status("taunt")
        );
        app.world_mut().get_mut::<Health>(cursed).unwrap().current = 0.0;
        run(&mut app, 3);
        assert!(
            !app.world()
                .get::<StatusEffects>(attacker)
                .unwrap()
                .has_status("taunt"),
            "the taunt ends with its target"
        );
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

// --- Sharran Conversion (`convert_on_death`): enemy creeps rising again as allies ---

mod conversion {
    use super::*;
    use crate::components::{AttackTarget, Creep, PlayerHero, PlayerWallet, Tower};
    use crate::status::{StatusEffect, StatusEffects};

    fn spawn_creep_of(app: &mut App, team: Team, lane: Lane, ranged: bool, pos: Vec3) -> Entity {
        app.world_mut()
            .resource_scope(|world, assets: Mut<SharedAssets>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                let id = crate::units::spawn_creep(&mut commands, &assets, team, lane, pos, ranged);
                queue.apply(world);
                id
            })
    }

    /// A local (player) hero carrying a conversion aura of `radius`.
    fn spawn_holder(app: &mut App, team: Team, pos: Vec3, radius: f32, seconds: f32) -> Entity {
        let hero = app
            .world_mut()
            .resource_scope(|world, assets: Mut<SharedAssets>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                let hero = crate::units::spawn_hero_entity(
                    &mut commands,
                    &assets,
                    team,
                    true,
                    0,
                    pos,
                    HeroId::QueenOfPain,
                );
                queue.apply(world);
                hero
            });
        app.world_mut()
            .get_mut::<StatusEffects>(hero)
            .unwrap()
            .effects
            .push(StatusEffect::convert_on_death(radius, seconds));
        hero
    }

    fn run(app: &mut App, frames: u32) {
        for _ in 0..frames {
            app.update();
        }
    }

    fn kill(app: &mut App, unit: Entity) {
        app.world_mut().get_mut::<Health>(unit).unwrap().current = 0.0;
    }

    fn creeps_of(app: &mut App, team: Team) -> Vec<Entity> {
        app.world_mut()
            .query_filtered::<(Entity, &Team), With<Creep>>()
            .iter(app.world())
            .filter(|(_, t)| **t == team)
            .map(|(e, _)| e)
            .collect()
    }

    fn gold(app: &App, hero: Entity) -> u32 {
        app.world().get::<PlayerWallet>(hero).unwrap().gold
    }

    #[test]
    fn enemy_creep_dying_near_holder_returns_as_ally_with_full_health() {
        let mut app = sim_app();
        let _holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
        let victim = spawn_creep_of(
            &mut app,
            Team::Dire,
            Lane::Top,
            true,
            Vec3::new(200.0, 0.0, 0.0),
        );
        app.update();
        kill(&mut app, victim);
        run(&mut app, 3);
        assert!(
            creeps_of(&mut app, Team::Dire).is_empty(),
            "the corpse is gone"
        );
        let risen = creeps_of(&mut app, Team::Radiant);
        assert_eq!(risen.len(), 1);
        let creep = *app.world().get::<Creep>(risen[0]).unwrap();
        assert_eq!(creep.lane, Some(Lane::Top));
        assert!(creep.ranged, "same creep type");
        let health = app.world().get::<Health>(risen[0]).unwrap();
        assert_eq!(health.current, health.max);
        assert_eq!(health.max, 240.0, "a ranged creep's full health");
    }

    fn spawn_class_creep(
        app: &mut App,
        team: Team,
        class: crate::components::CreepClass,
        lane: Option<Lane>,
        pos: Vec3,
    ) -> Entity {
        app.world_mut()
            .resource_scope(|world, assets: Mut<SharedAssets>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                let id = crate::units::spawn_creep_with(
                    &mut commands,
                    &assets,
                    crate::units::CreepSpawn {
                        team,
                        position: pos,
                        ranged: false,
                        class,
                        lane,
                    },
                );
                queue.apply(world);
                id
            })
    }

    #[test]
    fn captains_and_bosses_never_convert_on_death() {
        use crate::components::CreepClass;
        for class in [CreepClass::Captain, CreepClass::Boss] {
            let mut app = sim_app();
            let _holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
            let victim = spawn_class_creep(
                &mut app,
                Team::Dire,
                class,
                Some(Lane::Mid),
                Vec3::new(200.0, 0.0, 0.0),
            );
            app.update();
            kill(&mut app, victim);
            run(&mut app, 3);
            assert!(creeps_of(&mut app, Team::Dire).is_empty(), "{class:?} died");
            assert!(
                creeps_of(&mut app, Team::Radiant).is_empty(),
                "{class:?} must not be raised"
            );
        }
        // A Normal creep next to them still converts (the rule is per class).
        let mut app = sim_app();
        let _holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
        let victim = spawn_class_creep(
            &mut app,
            Team::Dire,
            CreepClass::Normal,
            Some(Lane::Mid),
            Vec3::new(200.0, 0.0, 0.0),
        );
        app.update();
        kill(&mut app, victim);
        run(&mut app, 3);
        assert_eq!(creeps_of(&mut app, Team::Radiant).len(), 1);
    }

    #[test]
    fn lane_less_converted_creep_hunts() {
        use crate::components::CreepClass;
        let mut app = sim_app();
        let _holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
        let victim = spawn_class_creep(
            &mut app,
            Team::Dire,
            CreepClass::Normal,
            None,
            Vec3::new(200.0, 0.0, 0.0),
        );
        app.update();
        kill(&mut app, victim);
        run(&mut app, 3);
        let risen = creeps_of(&mut app, Team::Radiant);
        assert_eq!(risen.len(), 1);
        let creep = *app.world().get::<Creep>(risen[0]).unwrap();
        assert_eq!(creep.lane, None);
        assert!(app.world().get::<crate::ai::Hunter>(risen[0]).is_some());
        assert!(
            app.world()
                .get::<crate::ai::LaneFollower>(risen[0])
                .is_none()
        );
    }

    #[test]
    fn bounty_still_paid_on_converted_death() {
        let mut app = sim_app();
        let holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
        let victim = spawn_creep_of(
            &mut app,
            Team::Dire,
            Lane::Mid,
            false,
            Vec3::new(100.0, 0.0, 0.0),
        );
        app.update();
        let before = gold(&app, holder);
        kill(&mut app, victim);
        run(&mut app, 3);
        assert_eq!(gold(&app, holder), before + 35);
        assert_eq!(creeps_of(&mut app, Team::Radiant).len(), 1);
    }

    #[test]
    fn creep_dying_outside_radius_or_after_expiry_is_not_converted() {
        let mut app = sim_app();
        let _holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
        let far = spawn_creep_of(
            &mut app,
            Team::Dire,
            Lane::Mid,
            false,
            Vec3::new(700.0, 0.0, 0.0),
        );
        app.update();
        kill(&mut app, far);
        run(&mut app, 3);
        assert!(
            creeps_of(&mut app, Team::Radiant).is_empty(),
            "outside the radius"
        );

        // After the aura expired.
        let mut app = sim_app();
        let _holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 0.5);
        let victim = spawn_creep_of(
            &mut app,
            Team::Dire,
            Lane::Mid,
            false,
            Vec3::new(100.0, 0.0, 0.0),
        );
        run(&mut app, 90);
        kill(&mut app, victim);
        run(&mut app, 3);
        assert!(creeps_of(&mut app, Team::Radiant).is_empty(), "expired");
    }

    #[test]
    fn dead_holder_converts_nothing() {
        let mut app = sim_app();
        let holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
        let victim = spawn_creep_of(
            &mut app,
            Team::Dire,
            Lane::Mid,
            false,
            Vec3::new(100.0, 0.0, 0.0),
        );
        app.update();
        kill(&mut app, holder);
        kill(&mut app, victim);
        run(&mut app, 3);
        assert!(creeps_of(&mut app, Team::Radiant).is_empty());
        assert!(app.world().get::<PlayerHero>(holder).is_some());
    }

    #[test]
    fn heroes_towers_and_dummy_never_convert() {
        let mut app = sim_app();
        let dummy_at = crate::units::training_dummy_position();
        let _holder = spawn_holder(&mut app, Team::Radiant, dummy_at, 600.0, 10.0);
        app.world_mut()
            .resource_scope(|world, assets: Mut<SharedAssets>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                crate::units::spawn_training_dummy(&mut commands, &assets);
                crate::units::spawn_tower(
                    &mut commands,
                    &assets,
                    Team::Dire,
                    Lane::Mid,
                    dummy_at + Vec3::new(150.0, 0.0, 0.0),
                );
                crate::units::spawn_hero_entity(
                    &mut commands,
                    &assets,
                    Team::Dire,
                    false,
                    1,
                    dummy_at + Vec3::new(0.0, 0.0, 150.0),
                    HeroId::QueenOfPain,
                );
                queue.apply(world);
            });
        app.update();
        let doomed: Vec<Entity> = app
            .world_mut()
            .query::<(Entity, &Team)>()
            .iter(app.world())
            .filter(|(_, team)| **team == Team::Dire)
            .map(|(e, _)| e)
            .collect();
        assert!(doomed.len() >= 3, "dummy, tower and hero: {doomed:?}");
        assert!(
            app.world_mut()
                .query_filtered::<(), With<Tower>>()
                .iter(app.world())
                .count()
                >= 1
        );
        for unit in doomed {
            if app.world().get::<Health>(unit).is_some() {
                kill(&mut app, unit);
            }
        }
        run(&mut app, 3);
        assert!(creeps_of(&mut app, Team::Radiant).is_empty());
        assert!(creeps_of(&mut app, Team::Dire).is_empty());
    }

    #[test]
    fn allied_creep_dying_near_holder_is_not_converted() {
        let mut app = sim_app();
        let _holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
        let ally = spawn_creep_of(
            &mut app,
            Team::Radiant,
            Lane::Mid,
            false,
            Vec3::new(100.0, 0.0, 0.0),
        );
        app.update();
        kill(&mut app, ally);
        run(&mut app, 3);
        assert!(creeps_of(&mut app, Team::Radiant).is_empty());
        assert!(creeps_of(&mut app, Team::Dire).is_empty());
    }

    #[test]
    fn converted_creep_attacks_nearby_enemy() {
        let mut app = sim_app();
        let _holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 10.0);
        let victim = spawn_creep_of(
            &mut app,
            Team::Dire,
            Lane::Mid,
            false,
            Vec3::new(250.0, 0.0, 0.0),
        );
        let target = spawn_creep_of(
            &mut app,
            Team::Dire,
            Lane::Mid,
            false,
            Vec3::new(370.0, 0.0, 0.0),
        );
        app.world_mut()
            .get_mut::<StatusEffects>(target)
            .unwrap()
            .effects
            .push(StatusEffect::rooted(60.0));
        app.update();
        kill(&mut app, victim);
        let before = app.world().get::<Health>(target).unwrap().current;
        run(&mut app, 240);
        let risen = creeps_of(&mut app, Team::Radiant);
        assert_eq!(risen.len(), 1);
        assert!(
            app.world().get::<Health>(target).unwrap().current < before,
            "the risen creep hit the enemy"
        );
        assert!(app.world().get::<AttackTarget>(risen[0]).is_some());
    }

    #[test]
    fn converted_creep_resumes_lane_forward() {
        let mut app = sim_app();
        let death = crate::scale::ground(10.0, 10.0);
        let _holder = spawn_holder(&mut app, Team::Radiant, death, 300.0, 10.0);
        let victim = spawn_creep_of(&mut app, Team::Dire, Lane::Mid, false, death);
        app.update();
        kill(&mut app, victim);
        run(&mut app, 2);
        let risen = creeps_of(&mut app, Team::Radiant);
        assert_eq!(risen.len(), 1);
        let follower = app
            .world()
            .get::<crate::ai::LaneFollower>(risen[0])
            .unwrap();
        let end = *follower.waypoints.last().unwrap();
        let aimed = follower.waypoints[follower.index];
        assert!(
            Vec2::new(aimed.x - end.x, aimed.z - end.z).length()
                < Vec2::new(death.x - end.x, death.z - end.z).length(),
            "aims ahead toward the enemy base, not back toward its own"
        );
        let start = app.world().get::<Transform>(risen[0]).unwrap().translation;
        run(&mut app, 120);
        let later = app.world().get::<Transform>(risen[0]).unwrap().translation;
        assert!(
            Vec2::new(later.x - end.x, later.z - end.z).length()
                < Vec2::new(start.x - end.x, start.z - end.z).length() - 100.0,
            "walked forward: {start:?} -> {later:?}"
        );
    }

    #[test]
    fn converted_creep_expires_after_30s_without_bounty() {
        let mut app = sim_app();
        let holder = spawn_holder(&mut app, Team::Radiant, Vec3::ZERO, 300.0, 5.0);
        let victim = spawn_creep_of(
            &mut app,
            Team::Dire,
            Lane::Mid,
            false,
            Vec3::new(100.0, 0.0, 0.0),
        );
        app.update();
        kill(&mut app, victim);
        run(&mut app, 3);
        assert_eq!(creeps_of(&mut app, Team::Radiant).len(), 1);
        let gold_after_death = gold(&app, holder);
        run(&mut app, 29 * 60);
        assert_eq!(
            creeps_of(&mut app, Team::Radiant).len(),
            1,
            "still alive at 29s"
        );
        run(&mut app, 2 * 60);
        assert!(
            creeps_of(&mut app, Team::Radiant).is_empty(),
            "gone after 30s"
        );
        assert_eq!(
            gold(&app, holder),
            gold_after_death,
            "no bounty for an expiry"
        );
    }
}
