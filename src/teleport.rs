//! Instant relocation requested by `AbilityEffect::Teleport` through [`TeleportEvent`].
//!
//! ```text
//! AbilityEffect::Teleport (ability or item, via the shared EffectRunner)
//!   → TeleportEvent { target, caster, destination = the trigger's aim }
//!   → validate: valid mobile target → inside the map → clear of static obstacles
//!               and buildings (NavGrid) → no overlap with units that would block it
//!   → ok:       move the unit, drop its stale orders, TeleportCompletedEvent
//!   → rejected: nothing changes, TeleportRejectedEvent
//! ```
//!
//! Cast time, cooldown, mana, and range belong to the casting / item-use systems; this
//! module only decides whether the target may stand at the destination. Destinations are
//! never nudged to a nearby legal spot, and units are never pushed apart afterwards:
//! overlap follows the same rule as `movement::resolve_unit_collisions`, so a destination
//! that rule would have to shove units around is rejected instead.

use bevy::prelude::*;

use crate::components::{AttackMoveOrder, CollisionRadius, CombatStats, Health, MoveTarget};
use crate::dimensions::collision_of;
use crate::displacement::ForcedMovement;
use crate::items::StatusEffects;
use crate::navigation::attack_position::AttackPositionGoal;
use crate::navigation::{flat, MobileFilter, NavGrid, NavPath, NavSteering};

/// Request to relocate `target` to `destination` (ground plane; Y is ignored).
/// Emitted for every resolved target of `AbilityEffect::Teleport`.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct TeleportEvent {
    pub target: Entity,
    pub caster: Entity,
    pub destination: Vec3,
}

/// The target now stands at `to`; it was at `from`.
#[allow(dead_code)]
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct TeleportCompletedEvent {
    pub target: Entity,
    pub caster: Entity,
    pub from: Vec3,
    pub to: Vec3,
}

/// The destination was refused and the target was left untouched.
#[allow(dead_code)]
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct TeleportRejectedEvent {
    pub target: Entity,
    pub caster: Entity,
    pub destination: Vec3,
    pub reason: TeleportError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeleportError {
    /// Missing, dead, or a static structure rather than a mobile unit.
    InvalidTarget,
    /// The target's collision circle would not fit inside the playable map.
    OutsideMap,
    /// Terrain, trees, or buildings block the target's collision circle.
    Blocked,
    /// Another unit occupies the spot and unit collision would have to separate them.
    Occupied,
}

/// Checks a collision circle of `radius` at `destination` against the map, static
/// geometry, and the `(position, radius)` footprints of units that block it.
pub fn validate_destination(
    grid: &NavGrid,
    destination: Vec2,
    radius: f32,
    blockers: impl IntoIterator<Item = (Vec2, f32)>,
) -> Result<(), TeleportError> {
    if !destination.is_finite() || !grid.contains(destination, radius) {
        return Err(TeleportError::OutsideMap);
    }
    if !grid.point_clear(destination, radius) {
        return Err(TeleportError::Blocked);
    }
    let occupied = blockers
        .into_iter()
        .any(|(center, other)| center.distance(destination) < radius + other);
    if occupied {
        return Err(TeleportError::Occupied);
    }
    Ok(())
}

type TeleportQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Transform,
        Option<&'static CollisionRadius>,
        Option<&'static StatusEffects>,
        Option<&'static Health>,
        Has<CombatStats>,
    ),
    MobileFilter,
>;

pub fn apply_teleport_events(
    mut events: MessageReader<TeleportEvent>,
    mut completed: MessageWriter<TeleportCompletedEvent>,
    mut rejected: MessageWriter<TeleportRejectedEvent>,
    grid: Res<NavGrid>,
    mut units: TeleportQuery,
    mut commands: Commands,
) {
    for event in events.read() {
        let destination = Vec3::new(event.destination.x, 0.0, event.destination.z);
        let reject = |rejected: &mut MessageWriter<TeleportRejectedEvent>, reason| {
            rejected.write(TeleportRejectedEvent {
                target: event.target,
                caster: event.caster,
                destination: event.destination,
                reason,
            });
        };

        let Ok((_, _, radius, statuses, health, _)) = units.get(event.target) else {
            reject(&mut rejected, TeleportError::InvalidTarget);
            continue;
        };
        if health.is_some_and(|h| !h.is_alive()) {
            reject(&mut rejected, TeleportError::InvalidTarget);
            continue;
        }
        let radius = collision_of(radius);
        let phased = statuses.is_some_and(|s| s.is_phased());

        // Mirrors the participants of `resolve_unit_collisions`.
        let blockers = units
            .iter()
            .filter(|(entity, _, other_radius, other_statuses, _, combatant)| {
                !phased
                    && *entity != event.target
                    && *combatant
                    && collision_of(*other_radius) > 0.0
                    && !other_statuses.is_some_and(|s| s.is_phased())
            })
            .map(|(_, tf, other_radius, ..)| (flat(tf.translation), collision_of(other_radius)));
        if let Err(reason) = validate_destination(&grid, flat(destination), radius, blockers) {
            reject(&mut rejected, reason);
            continue;
        }

        let Ok((_, mut transform_mut, ..)) = units.get_mut(event.target) else {
            continue;
        };
        let from = transform_mut.translation;
        let to = Vec3::new(destination.x, from.y, destination.z);
        transform_mut.translation = to;

        // Orders aimed at the old location would drag the unit back across the map.
        commands.entity(event.target).remove::<(
            MoveTarget,
            NavPath,
            NavSteering,
            AttackMoveOrder,
            AttackPositionGoal,
            ForcedMovement,
        )>();
        completed.write(TeleportCompletedEvent {
            target: event.target,
            caster: event.caster,
            from,
            to,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::components::{Ancient, Mana, Obstacle, Team, Tower};
    use crate::items::{apply_phased, StatusEffects};
    use crate::movement::{MovementPlugin, SimSet};
    use crate::scale;

    const SPEED: f32 = 300.0;

    fn sim_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<TeleportEvent>()
            .add_message::<TeleportCompletedEvent>()
            .add_message::<TeleportRejectedEvent>()
            .add_plugins(MovementPlugin)
            .add_systems(Update, apply_teleport_events.before(SimSet::Ai));
        app
    }

    fn step(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(1.0 / 30.0));
        app.update();
    }

    fn spawn_unit(app: &mut App, pos: Vec3, radius: f32) -> Entity {
        app.world_mut()
            .spawn((
                Transform::from_translation(pos),
                Team::Radiant,
                Health::new(500.0),
                Mana::new(200.0, 0.0),
                CombatStats::simple(10.0, 150.0, 1.0, 0.0, 0.0, SPEED),
                CollisionRadius(radius),
                StatusEffects::default(),
            ))
            .id()
    }

    fn teleport(app: &mut App, target: Entity, destination: Vec3) {
        app.world_mut().write_message(TeleportEvent {
            target,
            caster: target,
            destination,
        });
        step(app);
    }

    fn pos(app: &App, entity: Entity) -> Vec3 {
        app.world().get::<Transform>(entity).unwrap().translation
    }

    fn completed(app: &App) -> Vec<TeleportCompletedEvent> {
        let messages = app.world().resource::<Messages<TeleportCompletedEvent>>();
        messages.iter_current_update_messages().copied().collect()
    }

    fn rejections(app: &App) -> Vec<TeleportError> {
        let messages = app.world().resource::<Messages<TeleportRejectedEvent>>();
        messages.iter_current_update_messages().map(|e| e.reason).collect()
    }

    #[test]
    fn valid_destination_moves_the_unit_at_once() {
        let mut app = sim_app();
        let unit = spawn_unit(&mut app, Vec3::new(0.0, 5.0, 0.0), 40.0);
        let target = Vec3::new(3000.0, 0.0, -2500.0);
        teleport(&mut app, unit, target);

        assert_eq!(pos(&app, unit), Vec3::new(3000.0, 5.0, -2500.0), "height is preserved");
        let events = completed(&app);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].from, Vec3::new(0.0, 5.0, 0.0));
        assert_eq!(events[0].to, pos(&app, unit));
        assert!(rejections(&app).is_empty());
    }

    #[test]
    fn terrain_buildings_and_the_map_edge_reject_the_destination() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(1000.0, 0.0, 0.0), Obstacle::aabb(64.0, 64.0)));
        app.world_mut().spawn((
            Transform::from_xyz(-1000.0, 0.0, 0.0),
            Tower,
            Team::Dire,
            Health::new(1800.0),
            CollisionRadius(100.0),
        ));
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 0.0, 1500.0),
            Ancient,
            Team::Dire,
            Health::new(1800.0),
            CollisionRadius(150.0),
        ));
        let unit = spawn_unit(&mut app, Vec3::ZERO, 40.0);
        step(&mut app);

        let cases = [
            (Vec3::new(1000.0, 0.0, 0.0), TeleportError::Blocked),
            (Vec3::new(-1020.0, 0.0, 30.0), TeleportError::Blocked),
            (Vec3::new(0.0, 0.0, 1480.0), TeleportError::Blocked),
            (Vec3::new(scale::MAP_HALF + 500.0, 0.0, 0.0), TeleportError::OutsideMap),
            (Vec3::new(0.0, 0.0, -scale::MAP_HALF - 1.0), TeleportError::OutsideMap),
            (Vec3::new(f32::NAN, 0.0, 0.0), TeleportError::OutsideMap),
        ];
        for (destination, expected) in cases {
            teleport(&mut app, unit, destination);
            assert_eq!(pos(&app, unit), Vec3::ZERO, "{destination:?} must not move the unit");
            assert_eq!(rejections(&app), vec![expected], "{destination:?}");
            assert!(completed(&app).is_empty());
        }
    }

    #[test]
    fn the_targets_collision_radius_decides_what_fits() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(1000.0, 0.0, 0.0), Obstacle::aabb(64.0, 64.0)));
        let small = spawn_unit(&mut app, Vec3::new(0.0, 0.0, 400.0), 20.0);
        let large = spawn_unit(&mut app, Vec3::new(0.0, 0.0, -400.0), 80.0);
        step(&mut app);

        // 50 units beyond the tree's face: the small unit fits, the large one overlaps it.
        let beside_tree = Vec3::new(1000.0 + 64.0 + 50.0, 0.0, 0.0);
        teleport(&mut app, small, beside_tree);
        assert_eq!(pos(&app, small).x, beside_tree.x);
        teleport(&mut app, large, beside_tree);
        assert_eq!(rejections(&app), vec![TeleportError::Blocked]);
        assert_eq!(pos(&app, large), Vec3::new(0.0, 0.0, -400.0));

        // The same applies to the map edge.
        let edge = Vec3::new(scale::MAP_HALF - 50.0, 0.0, 0.0);
        teleport(&mut app, large, edge);
        assert_eq!(rejections(&app), vec![TeleportError::OutsideMap]);
        let other = spawn_unit(&mut app, Vec3::new(0.0, 0.0, 3000.0), 20.0);
        teleport(&mut app, other, edge);
        assert_eq!(pos(&app, other).x, edge.x);
    }

    #[test]
    fn standing_units_block_the_destination_unless_phased() {
        let mut app = sim_app();
        let blocker = spawn_unit(&mut app, Vec3::new(2000.0, 0.0, 0.0), 40.0);
        let mover = spawn_unit(&mut app, Vec3::ZERO, 40.0);
        let phased = spawn_unit(&mut app, Vec3::new(0.0, 0.0, 500.0), 40.0);
        apply_phased(&mut app.world_mut().get_mut::<StatusEffects>(phased).unwrap(), 5.0);

        teleport(&mut app, mover, Vec3::new(2060.0, 0.0, 0.0));
        assert_eq!(rejections(&app), vec![TeleportError::Occupied]);
        assert_eq!(pos(&app, mover), Vec3::ZERO);
        assert_eq!(pos(&app, blocker), Vec3::new(2000.0, 0.0, 0.0));

        teleport(&mut app, mover, Vec3::new(2081.0, 0.0, 0.0));
        assert!(rejections(&app).is_empty(), "just clear of the other unit");
        assert_eq!(pos(&app, mover).x, 2081.0);
        assert_eq!(pos(&app, blocker).x, 2000.0, "no one is pushed aside");

        teleport(&mut app, phased, Vec3::new(2010.0, 0.0, 0.0));
        assert_eq!(pos(&app, phased).x, 2010.0, "phased units ignore unit collision");

        let ghost = spawn_unit(&mut app, Vec3::new(0.0, 0.0, -500.0), 40.0);
        apply_phased(&mut app.world_mut().get_mut::<StatusEffects>(ghost).unwrap(), 5.0);
        let other = spawn_unit(&mut app, Vec3::new(0.0, 0.0, 900.0), 40.0);
        teleport(&mut app, other, Vec3::new(0.0, 0.0, -490.0));
        assert!(rejections(&app).is_empty(), "phased units do not block others");
        assert_eq!(pos(&app, other).z, -490.0);
    }

    #[test]
    fn invalid_targets_are_rejected() {
        let mut app = sim_app();
        let tower = app
            .world_mut()
            .spawn((Transform::from_xyz(-500.0, 0.0, 0.0), Tower, CollisionRadius(100.0)))
            .id();
        let dead = spawn_unit(&mut app, Vec3::new(500.0, 0.0, 0.0), 40.0);
        app.world_mut().get_mut::<Health>(dead).unwrap().current = 0.0;
        let gone = app.world_mut().spawn_empty().id();
        app.world_mut().despawn(gone);

        for target in [tower, dead, gone] {
            teleport(&mut app, target, Vec3::new(0.0, 0.0, 2000.0));
            assert_eq!(rejections(&app), vec![TeleportError::InvalidTarget]);
        }
        assert_eq!(pos(&app, tower).x, -500.0);
        assert_eq!(pos(&app, dead).x, 500.0);
    }

    #[test]
    fn rejected_teleports_change_nothing() {
        let mut app = sim_app();
        app.world_mut()
            .spawn((Transform::from_xyz(1000.0, 0.0, 0.0), Obstacle::aabb(64.0, 64.0)));
        let unit = spawn_unit(&mut app, Vec3::ZERO, 40.0);
        let order = Vec3::new(0.0, 0.0, 3000.0);
        app.world_mut().entity_mut(unit).insert((
            MoveTarget { position: order },
            AttackMoveOrder { destination: order },
        ));
        for _ in 0..5 {
            step(&mut app);
        }
        let before = (pos(&app, unit), app.world().get::<Health>(unit).unwrap().current);
        assert!(app.world().get::<NavPath>(unit).is_some());

        teleport(&mut app, unit, Vec3::new(1000.0, 0.0, 0.0));
        assert_eq!(rejections(&app), vec![TeleportError::Blocked]);
        let world = app.world();
        assert!(world.get::<MoveTarget>(unit).is_some(), "orders survive a rejected teleport");
        assert!(world.get::<AttackMoveOrder>(unit).is_some());
        assert!(world.get::<NavPath>(unit).is_some());
        assert!(pos(&app, unit).z > before.0.z, "the unit keeps walking its original route");
        assert_eq!(world.get::<Health>(unit).unwrap().current, before.1);
    }

    #[test]
    fn stale_orders_do_not_pull_the_unit_back() {
        let mut app = sim_app();
        let unit = spawn_unit(&mut app, Vec3::ZERO, 40.0);
        let order = Vec3::new(0.0, 0.0, 3000.0);
        app.world_mut().entity_mut(unit).insert((
            MoveTarget { position: order },
            AttackMoveOrder { destination: order },
            ForcedMovement {
                from: Vec3::ZERO,
                to: Vec3::new(500.0, 0.0, 0.0),
                elapsed: 0.0,
                duration: 5.0,
            },
        ));
        for _ in 0..5 {
            step(&mut app);
        }
        assert!(app.world().get::<NavPath>(unit).is_some());

        let destination = Vec3::new(-3000.0, 0.0, -3000.0);
        teleport(&mut app, unit, destination);
        for _ in 0..30 {
            step(&mut app);
        }
        let world = app.world();
        assert!(world.get::<MoveTarget>(unit).is_none());
        assert!(world.get::<NavPath>(unit).is_none());
        assert!(world.get::<NavSteering>(unit).is_none());
        assert!(world.get::<AttackMoveOrder>(unit).is_none());
        assert!(world.get::<ForcedMovement>(unit).is_none());
        assert_eq!(pos(&app, unit), Vec3::new(destination.x, 0.0, destination.z));
    }

    #[test]
    fn teleporting_preserves_unrelated_state() {
        let mut app = sim_app();
        let unit = spawn_unit(&mut app, Vec3::ZERO, 40.0);
        app.world_mut().get_mut::<Health>(unit).unwrap().current = 123.0;
        app.world_mut().get_mut::<Mana>(unit).unwrap().current = 45.0;
        apply_phased(&mut app.world_mut().get_mut::<StatusEffects>(unit).unwrap(), 5.0);
        teleport(&mut app, unit, Vec3::new(2000.0, 0.0, 2000.0));

        let world = app.world();
        assert_eq!(world.get::<Health>(unit).unwrap().current, 123.0);
        assert_eq!(world.get::<Mana>(unit).unwrap().current, 45.0);
        assert!(world.get::<StatusEffects>(unit).unwrap().is_phased());
        assert_eq!(world.get::<CollisionRadius>(unit).unwrap().0, 40.0);
    }

    #[test]
    fn validation_is_pure_and_deterministic() {
        let grid = NavGrid::default();
        let blockers = [(Vec2::new(100.0, 0.0), 30.0)];
        assert_eq!(validate_destination(&grid, Vec2::ZERO, 40.0, blockers), Ok(()));
        assert_eq!(
            validate_destination(&grid, Vec2::new(31.0, 0.0), 40.0, blockers),
            Err(TeleportError::Occupied)
        );
        assert_eq!(validate_destination(&grid, Vec2::new(30.0, 0.0), 40.0, blockers), Ok(()));
    }

    #[test]
    fn teleport_is_ordered_consistently_in_the_real_schedule() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(crate::net::NetConfig {
                mode: crate::net::NetMode::Offline,
                addr: "127.0.0.1:7777".parse().unwrap(),
            })
            .add_plugins((
                crate::movement::MovementPlugin,
                crate::unit_commands::UnitCommandsPlugin,
                crate::combat::CombatPlugin,
                crate::items::ItemsPlugin,
                crate::abilities::AbilitiesPlugin,
                crate::ai::AiPlugin,
                crate::progression::ProgressionPlugin,
            ));
        app.world_mut()
            .schedule_scope(Update, |world, schedule| schedule.initialize(world))
            .expect("the update schedule has no ordering cycle");
    }
}
