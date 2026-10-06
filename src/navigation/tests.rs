//! Simulated scenarios: the real movement / attack-positioning / command systems
//! on the full-size map with hand-placed obstacles.

use std::time::Duration;

use bevy::prelude::*;

use super::attack_position::assign_attack_positions;
use super::NavPath;
use crate::abilities::casting::{
    resolve_queued_ability_casts, tick_ability_casting, validate_cast_requests, AbilityCastEvent,
    AbilityCastRequest,
};
use crate::abilities::catalog::{register_loadout_definitions, AbilityDefinitions};
use crate::components::{
    AbilityCasting, AbilityId, AbilityLoadout, AttackTarget, BoundRadius, CastTarget,
    CollisionRadius, CombatStats, Health, Mana, MoveTarget, Obstacle, Team, Tower,
};
use crate::dimensions::{center_distance, edge_distance};
use crate::items::StatusEffects;
use crate::movement::{MovementPlugin, SimSet};
use crate::unit_commands::{CommandQueue, IssueCommand, UnitCommand, UnitCommandsPlugin};

const DT: f32 = 1.0 / 30.0;
const HERO_R: f32 = 40.0;
const CREEP_R: f32 = 36.0;

fn sim_app() -> App {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<AbilityDefinitions>()
        .add_message::<AbilityCastRequest>()
        .add_message::<AbilityCastEvent>()
        .add_plugins((MovementPlugin, UnitCommandsPlugin))
        .add_systems(Update, assign_attack_positions.in_set(SimSet::Ai))
        .add_systems(
            Update,
            (
                register_loadout_definitions,
                resolve_queued_ability_casts,
                validate_cast_requests,
                tick_ability_casting,
            )
                .chain()
                .after(SimSet::Commands),
        );
    app
}

fn step(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(DT));
    app.update();
}

/// Run until `done` holds (checked every frame) or `seconds` elapse; `each` sees every frame.
fn run_until(app: &mut App, seconds: f32, mut each: impl FnMut(&World), done: impl Fn(&World) -> bool) -> bool {
    for _ in 0..(seconds / DT) as usize {
        step(app);
        each(app.world());
        if done(app.world()) {
            return true;
        }
    }
    false
}

fn spawn_unit(app: &mut App, team: Team, pos: Vec3, radius: f32, bounds: f32, range: f32, speed: f32) -> Entity {
    app.world_mut()
        .spawn((
            Transform::from_translation(pos),
            team,
            Health::new(500.0),
            CombatStats::simple(10.0, range, 1.0, 0.0, 0.0, speed),
            CollisionRadius(radius),
            BoundRadius(bounds),
            StatusEffects::default(),
        ))
        .id()
}

fn spawn_hero(app: &mut App, pos: Vec3) -> Entity {
    let hero = spawn_unit(app, Team::Radiant, pos, HERO_R, 36.0, 150.0, 300.0);
    let mut loadout = AbilityLoadout::from_abilities([
        AbilityId::Shockwave,
        AbilityId::Bolt,
        AbilityId::Execute,
        AbilityId::Bulwark,
    ]);
    loadout.slots[0].rank = 1;
    loadout.slots[1].rank = 1;
    app.world_mut()
        .entity_mut(hero)
        .insert((CommandQueue::default(), loadout, Mana::new(1000.0, 0.0)));
    hero
}

fn spawn_tower(app: &mut App, pos: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            Transform::from_translation(pos),
            Team::Radiant,
            Tower,
            Health::new(1800.0),
            CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0),
            CollisionRadius(100.0),
            BoundRadius(96.0),
        ))
        .id()
}

fn spawn_tree(app: &mut App, x: f32, z: f32) {
    app.world_mut()
        .spawn((Transform::from_xyz(x, 0.0, z), Obstacle::aabb(64.0, 64.0)));
}

fn pos(world: &World, e: Entity) -> Vec3 {
    world.get::<Transform>(e).unwrap().translation
}

fn issue(app: &mut App, unit: Entity, command: UnitCommand, queue: bool) {
    app.world_mut().write_message(IssueCommand { unit, command, queue });
}

/// Smallest gap between a unit circle and a tree box, in the collision metric.
fn tree_gap(p: Vec3, tree: Vec2, r: f32) -> f32 {
    let d = (Vec2::new(p.x, p.z) - tree).abs() - Vec2::splat(64.0);
    d.x.max(d.y) - r
}

#[test]
fn hero_walks_around_a_tower_without_touching_it() {
    let mut app = sim_app();
    spawn_tower(&mut app, Vec3::ZERO);
    let hero = spawn_hero(&mut app, Vec3::new(-800.0, 0.0, 20.0));
    let goal = Vec3::new(800.0, 0.0, -20.0);
    issue(&mut app, hero, UnitCommand::Move { destination: goal }, false);

    step(&mut app);
    step(&mut app);
    let planned = app.world().get::<NavPath>(hero).unwrap().waypoints.clone();
    assert!(planned.len() >= 2, "a detour was planned up front: {planned:?}");

    let mut closest = f32::MAX;
    let mut replanned = false;
    let arrived = run_until(
        &mut app,
        12.0,
        |w| {
            closest = closest.min(center_distance(pos(w, hero), Vec3::ZERO));
            if let Some(path) = w.get::<NavPath>(hero) {
                replanned |= path.waypoints != planned;
            }
        },
        |w| w.get::<MoveTarget>(hero).is_none(),
    );
    assert!(arrived);
    assert!(center_distance(pos(app.world(), hero), goal) < 8.0);
    assert!(closest >= 100.0 + HERO_R - 0.5, "hero clipped the tower: {closest}");
    assert!(!replanned, "static goal must not replan every frame");
}

#[test]
fn hero_threads_multiple_obstacles() {
    let mut app = sim_app();
    // A tree wall with a single gap at z = 256, and a tower right behind the gap.
    let mut trees = Vec::new();
    for i in -6..=6 {
        if i == 2 {
            continue;
        }
        let tree = Vec2::new(0.0, i as f32 * 128.0);
        spawn_tree(&mut app, tree.x, tree.y);
        trees.push(tree);
    }
    spawn_tower(&mut app, Vec3::new(450.0, 0.0, 256.0));
    let hero = spawn_hero(&mut app, Vec3::new(-700.0, 0.0, -300.0));
    let goal = Vec3::new(900.0, 0.0, 300.0);
    issue(&mut app, hero, UnitCommand::Move { destination: goal }, false);

    let mut min_tree_gap = f32::MAX;
    let mut min_tower = f32::MAX;
    let arrived = run_until(
        &mut app,
        15.0,
        |w| {
            let p = pos(w, hero);
            for t in &trees {
                min_tree_gap = min_tree_gap.min(tree_gap(p, *t, HERO_R));
            }
            min_tower = min_tower.min(center_distance(p, Vec3::new(450.0, 0.0, 256.0)));
        },
        |w| w.get::<MoveTarget>(hero).is_none(),
    );
    assert!(arrived);
    assert!(center_distance(pos(app.world(), hero), goal) < 8.0);
    assert!(min_tree_gap >= -0.5, "hero clipped a tree: {min_tree_gap}");
    assert!(min_tower >= 140.0 - 0.5, "hero clipped the tower: {min_tower}");
}

#[test]
fn creep_group_flows_around_a_tower() {
    let mut app = sim_app();
    spawn_tower(&mut app, Vec3::ZERO);
    let mut creeps = Vec::new();
    for row in 0..2 {
        for col in -1..=1 {
            let start = Vec3::new(-900.0 - row as f32 * 90.0, 0.0, col as f32 * 80.0);
            let creep = spawn_unit(&mut app, Team::Radiant, start, CREEP_R, 24.0, 100.0, 195.0);
            let goal = Vec3::new(900.0 + row as f32 * 90.0, 0.0, col as f32 * 80.0);
            app.world_mut().entity_mut(creep).insert(MoveTarget { position: goal });
            creeps.push((creep, goal));
        }
    }

    let mut closest_tower = f32::MAX;
    let mut worst_overlap: f32 = 0.0;
    let ids: Vec<Entity> = creeps.iter().map(|(c, _)| *c).collect();
    let arrived = run_until(
        &mut app,
        20.0,
        |w| {
            for (i, a) in ids.iter().enumerate() {
                closest_tower = closest_tower.min(center_distance(pos(w, *a), Vec3::ZERO));
                for b in &ids[i + 1..] {
                    let overlap = 2.0 * CREEP_R - center_distance(pos(w, *a), pos(w, *b));
                    worst_overlap = worst_overlap.max(overlap);
                }
            }
        },
        |w| ids.iter().all(|c| w.get::<MoveTarget>(*c).is_none()),
    );
    assert!(arrived, "every creep finishes its move");
    for (creep, goal) in &creeps {
        assert!(
            center_distance(pos(app.world(), *creep), *goal) < 160.0,
            "creep ended far from its goal"
        );
    }
    assert!(closest_tower >= 100.0 + CREEP_R - 1.0, "a creep walked into the tower: {closest_tower}");
    assert!(worst_overlap < 6.0, "creeps phased through each other: {worst_overlap}");
}

fn attackers_on_one_target(radius: f32, bounds: f32, range: f32, count: usize, start_x: f32) -> (App, Entity, Vec<Entity>) {
    let mut app = sim_app();
    let target = spawn_unit(&mut app, Team::Dire, Vec3::ZERO, HERO_R, 36.0, 0.0, 0.0);
    let attackers = (0..count)
        .map(|i| {
            let start = Vec3::new(start_x - i as f32 * (radius * 2.0 + 10.0), 0.0, (i % 2) as f32 * 10.0);
            let a = spawn_unit(&mut app, Team::Radiant, start, radius, bounds, range, 195.0);
            app.world_mut().entity_mut(a).insert(AttackTarget(target));
            a
        })
        .collect();
    (app, target, attackers)
}

#[test]
fn melee_attackers_surround_instead_of_queueing() {
    let (mut app, target, attackers) = attackers_on_one_target(CREEP_R, 24.0, 100.0, 5, -400.0);
    let settled = run_until(
        &mut app,
        12.0,
        |_| {},
        |w| attackers.iter().all(|a| w.get::<MoveTarget>(*a).is_none()),
    );
    assert!(settled, "attackers stop once in position");
    let world = app.world();
    for a in &attackers {
        let gap = edge_distance(pos(world, *a), 24.0, pos(world, target), 36.0);
        assert!(gap <= 100.0, "attacker stuck out of range (gap {gap})");
    }
    // Spread around the target: the attackers cover a wide arc, not one line.
    let angles: Vec<f32> = attackers
        .iter()
        .map(|a| {
            let p = pos(world, *a);
            p.z.atan2(p.x)
        })
        .collect();
    let spread = angles.iter().cloned().fold(f32::MIN, f32::max) - angles.iter().cloned().fold(f32::MAX, f32::min);
    assert!(spread > 1.2, "melee attackers bunched on one side: {angles:?}");
}

#[test]
fn ranged_attackers_stop_in_range_without_stacking() {
    let (mut app, target, attackers) = attackers_on_one_target(28.0, 16.0, 500.0, 4, -1100.0);
    let settled = run_until(
        &mut app,
        12.0,
        |_| {},
        |w| attackers.iter().all(|a| w.get::<MoveTarget>(*a).is_none()),
    );
    assert!(settled);
    let world = app.world();
    for (i, a) in attackers.iter().enumerate() {
        let gap = edge_distance(pos(world, *a), 16.0, pos(world, target), 36.0);
        assert!(gap <= 500.0, "ranged attacker out of range (gap {gap})");
        assert!(gap > 300.0, "ranged attackers should stop near max range, not walk in (gap {gap})");
        for b in &attackers[i + 1..] {
            assert!(center_distance(pos(world, *a), pos(world, *b)) >= 56.0 - 2.0);
        }
    }
}

#[test]
fn units_pass_each_other_in_a_corridor() {
    let mut app = sim_app();
    // Two tree walls leave a 260-wide corridor along X.
    let mut trees = Vec::new();
    for i in -3..=3 {
        for z in [-194.0, 194.0] {
            let t = Vec2::new(i as f32 * 128.0, z);
            spawn_tree(&mut app, t.x, t.y);
            trees.push(t);
        }
    }
    let a = spawn_hero(&mut app, Vec3::new(-800.0, 0.0, 0.0));
    let b = spawn_hero(&mut app, Vec3::new(800.0, 0.0, 0.0));
    let goal_a = Vec3::new(800.0, 0.0, 0.0);
    let goal_b = Vec3::new(-800.0, 0.0, 0.0);
    issue(&mut app, a, UnitCommand::Move { destination: goal_a }, false);
    issue(&mut app, b, UnitCommand::Move { destination: goal_b }, false);

    let mut closest = f32::MAX;
    let mut min_tree_gap = f32::MAX;
    let arrived = run_until(
        &mut app,
        15.0,
        |w| {
            closest = closest.min(center_distance(pos(w, a), pos(w, b)));
            for t in &trees {
                min_tree_gap = min_tree_gap.min(tree_gap(pos(w, a), *t, HERO_R));
                min_tree_gap = min_tree_gap.min(tree_gap(pos(w, b), *t, HERO_R));
            }
        },
        |w| w.get::<MoveTarget>(a).is_none() && w.get::<MoveTarget>(b).is_none(),
    );
    assert!(arrived, "both units get through");
    assert!(center_distance(pos(app.world(), a), goal_a) < 8.0);
    assert!(center_distance(pos(app.world(), b), goal_b) < 8.0);
    assert!(closest >= 2.0 * HERO_R - 4.0, "units overlapped while passing: {closest}");
    assert!(min_tree_gap >= -1.0, "a unit was squeezed into a tree: {min_tree_gap}");
}

#[test]
fn shift_queued_moves_run_in_order() {
    let mut app = sim_app();
    let hero = spawn_hero(&mut app, Vec3::ZERO);
    let points = [
        Vec3::new(400.0, 0.0, 0.0),
        Vec3::new(400.0, 0.0, 400.0),
        Vec3::new(0.0, 0.0, 400.0),
    ];
    issue(&mut app, hero, UnitCommand::Move { destination: points[0] }, false);
    issue(&mut app, hero, UnitCommand::Move { destination: points[1] }, true);
    issue(&mut app, hero, UnitCommand::Move { destination: points[2] }, true);

    let mut visited = Vec::new();
    let done = run_until(
        &mut app,
        12.0,
        |w| {
            let p = pos(w, hero);
            for (i, point) in points.iter().enumerate() {
                if center_distance(p, *point) < 10.0 && visited.last() != Some(&i) {
                    visited.push(i);
                }
            }
        },
        |w| {
            let q = w.get::<CommandQueue>(hero).unwrap();
            q.current.is_none() && q.queued.is_empty()
        },
    );
    assert!(done);
    assert_eq!(visited, vec![0, 1, 2]);

    // A normal (non-Shift) command replaces the whole queue.
    issue(&mut app, hero, UnitCommand::Move { destination: points[0] }, false);
    issue(&mut app, hero, UnitCommand::Move { destination: points[1] }, true);
    step(&mut app);
    issue(&mut app, hero, UnitCommand::Move { destination: Vec3::ZERO }, false);
    step(&mut app);
    let q = app.world().get::<CommandQueue>(hero).unwrap();
    assert!(q.queued.is_empty());
    assert_eq!(q.current.unwrap().command, UnitCommand::Move { destination: Vec3::ZERO });
}

#[test]
fn move_then_ability_then_move() {
    let mut app = sim_app();
    let hero = spawn_hero(&mut app, Vec3::ZERO);
    let first = Vec3::new(400.0, 0.0, 0.0);
    let last = Vec3::new(400.0, 0.0, 400.0);
    issue(&mut app, hero, UnitCommand::Move { destination: first }, false);
    issue(
        &mut app,
        hero,
        UnitCommand::CastAbility {
            slot: 0,
            target: CastTarget::None,
        },
        true,
    );
    issue(&mut app, hero, UnitCommand::Move { destination: last }, true);

    let mut cast_at: Option<Vec3> = None;
    let done = run_until(
        &mut app,
        12.0,
        |w| {
            if cast_at.is_none() && w.get::<AbilityCasting>(hero).is_some() {
                cast_at = Some(pos(w, hero));
            }
        },
        |w| w.get::<CommandQueue>(hero).unwrap().current.is_none(),
    );
    assert!(done);
    let cast_at = cast_at.expect("the queued ability was cast");
    assert!(center_distance(cast_at, first) < 10.0, "cast only after the first move finished");
    assert!(app.world().get::<AbilityLoadout>(hero).unwrap().slots[0].cooldown_remaining > 0.0);
    assert!(center_distance(pos(app.world(), hero), last) < 8.0);
}

#[test]
fn invalid_or_dead_targets_do_not_stall_the_queue() {
    let mut app = sim_app();
    let hero = spawn_hero(&mut app, Vec3::ZERO);
    let dead = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0), HERO_R, 36.0, 0.0, 0.0);
    app.world_mut().get_mut::<Health>(dead).unwrap().current = 0.0;
    let gone = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0), HERO_R, 36.0, 0.0, 0.0);
    app.world_mut().despawn(gone);
    let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(600.0, 0.0, 0.0), HERO_R, 36.0, 0.0, 0.0);
    let goal = Vec3::new(0.0, 0.0, 500.0);

    issue(&mut app, hero, UnitCommand::Attack { target: dead }, false);
    issue(
        &mut app,
        hero,
        UnitCommand::CastAbility {
            slot: 1,
            target: CastTarget::Unit(gone),
        },
        true,
    );
    // Not learned: rejected by validation, must fail rather than wait forever.
    issue(
        &mut app,
        hero,
        UnitCommand::CastAbility {
            slot: 3,
            target: CastTarget::None,
        },
        true,
    );
    issue(&mut app, hero, UnitCommand::Attack { target: victim }, true);
    issue(&mut app, hero, UnitCommand::Move { destination: goal }, true);

    // The hero skips straight to attacking the live target and walks toward it.
    let attacking = run_until(&mut app, 2.0, |_| {}, |w| {
        w.get::<AttackTarget>(hero).is_some_and(|t| t.0 == victim)
    });
    assert!(attacking);
    for _ in 0..30 {
        step(&mut app);
    }
    assert_eq!(
        app.world().get::<CommandQueue>(hero).unwrap().current.unwrap().command,
        UnitCommand::Attack { target: victim },
        "an attack lasts while its target lives"
    );

    // The target dies mid-command: the queue moves on to the final move.
    app.world_mut().get_mut::<Health>(victim).unwrap().current = 0.0;
    let done = run_until(&mut app, 10.0, |_| {}, |w| {
        w.get::<CommandQueue>(hero).unwrap().current.is_none()
    });
    assert!(done);
    assert!(center_distance(pos(app.world(), hero), goal) < 8.0);
}

#[test]
fn new_obstacle_on_the_path_triggers_a_replan() {
    let mut app = sim_app();
    let hero = spawn_hero(&mut app, Vec3::new(-800.0, 0.0, 0.0));
    let goal = Vec3::new(800.0, 0.0, 0.0);
    issue(&mut app, hero, UnitCommand::Move { destination: goal }, false);
    for _ in 0..10 {
        step(&mut app);
    }
    assert_eq!(app.world().get::<NavPath>(hero).unwrap().waypoints.len(), 1, "open ground: direct");

    spawn_tree(&mut app, 200.0, 0.0);
    let mut min_gap = f32::MAX;
    let arrived = run_until(
        &mut app,
        10.0,
        |w| min_gap = min_gap.min(tree_gap(pos(w, hero), Vec2::new(200.0, 0.0), HERO_R)),
        |w| w.get::<MoveTarget>(hero).is_none(),
    );
    assert!(arrived);
    assert!(min_gap >= -0.5, "hero walked into the new tree: {min_gap}");
    assert!(center_distance(pos(app.world(), hero), goal) < 8.0);
}

#[test]
fn non_navigable_move_resolves_to_nearby_ground() {
    let mut app = sim_app();
    spawn_tree(&mut app, 600.0, 0.0);
    let hero = spawn_hero(&mut app, Vec3::ZERO);
    issue(
        &mut app,
        hero,
        UnitCommand::Move {
            destination: Vec3::new(600.0, 0.0, 10.0),
        },
        false,
    );
    let done = run_until(&mut app, 6.0, |_| {}, |w| {
        w.get::<CommandQueue>(hero).unwrap().current.is_none()
    });
    assert!(done, "move into a tree completes at the closest walkable point");
    let p = pos(app.world(), hero);
    assert!(tree_gap(p, Vec2::new(600.0, 0.0), HERO_R) >= -0.5);
    assert!(center_distance(p, Vec3::new(600.0, 0.0, 10.0)) < 64.0 + HERO_R + 20.0);
}
