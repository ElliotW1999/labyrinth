//! Factory helpers for heroes, creeps, towers, and ancients.

use bevy::prelude::*;

use crate::components::{
    Ancient, AttackCooldown, CombatStats, Creep, CreepClass, GoldBounty, Health, HeroAttributes,
    Lane, Lifetime, PlayerHero, PlayerWallet, SelectionBounds, Team, Tower, TrainingDummy,
    WorldTag, XpBounty,
};
use crate::dimensions::UnitDimensions;
use crate::heroes::{HeroId, HeroKind};
use crate::items::Inventory;
use crate::net::{NetworkId, NetworkedHero};
use crate::resources::SharedAssets;
use crate::scale;
use crate::status::StatusEffects;

pub struct UnitsPlugin;

impl Plugin for UnitsPlugin {
    fn build(&self, _app: &mut App) {
        // Heroes spawn after local selection (see HeroesPlugin).
    }
}

/// Place a model so it rests on the ground (center at half height).
fn grounded(position: Vec3, model_height: f32) -> Vec3 {
    Vec3::new(position.x, model_height * 0.5, position.z)
}

/// Level every hero starts the match at.
pub const STARTING_HERO_LEVEL: u32 = 6;
/// Gold every hero starts the match with.
pub const STARTING_GOLD: u32 = 25_000;

/// Spawn a playable hero. `local` adds `PlayerHero` (this machine's controlled unit).
pub fn spawn_hero_entity(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    local: bool,
    network_id: u32,
    position: Vec3,
    hero: HeroId,
) -> Entity {
    let def = hero.def();
    let (mut health, mut mana, mut stats, mut attrs) = def.vitals();
    // Heroes enter the match already at `STARTING_HERO_LEVEL`: apply the attribute growth
    // of the levels they skipped, then top them up.
    for _ in 1..STARTING_HERO_LEVEL {
        let before = attrs;
        attrs.level_up();
        HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
    }
    health.current = health.max;
    mana.current = mana.max;

    let mat = match team {
        Team::Radiant => assets.radiant_mat.clone(),
        Team::Dire => assets.dire_mat.clone(),
    };
    let name = if local {
        format!("Local {} ({team:?})", hero.name())
    } else {
        format!("Remote {} ({team:?})", hero.name())
    };

    let mut entity = commands.spawn((
        Name::new(name),
        Mesh3d(assets.hero_mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(grounded(position, scale::HERO_MODEL_HEIGHT)),
        team,
        NetworkedHero,
        NetworkId(network_id),
        HeroKind(hero),
        PlayerWallet {
            gold: STARTING_GOLD,
        },
        health,
        mana,
        stats,
        AttackCooldown(0.0),
        hero.loadout(),
        crate::components::HeroProgress::at_level(STARTING_HERO_LEVEL),
    ));
    entity.insert((
        def.dimensions.components(),
        GoldBounty(0),
        attrs,
        Inventory::empty(),
        StatusEffects::default(),
        crate::unit_commands::CommandQueue::default(),
        crate::abilities::passive::AbilityPassiveSync::default(),
        crate::abilities::passive::AppliedAbilityPassives::default(),
    ));
    if local {
        entity.insert(PlayerHero);
    }
    let id = entity.id();
    attach_mobile_unit_details(commands, id, assets, team, true);
    id
}

/// Stat multipliers and presentation of a [`CreepClass`]. `Normal` multiplies by one and
/// adds nothing, so classic lane creeps keep their exact stats. Captain and Boss values are
/// stubs to be tuned later.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreepClassProfile {
    pub health_mult: f32,
    pub damage_mult: f32,
    pub armor_bonus: f32,
    /// Scales the transform, collision radius, bounds radius and selection bounds.
    pub model_scale: f32,
    pub bounty_mult: f32,
    pub xp_mult: f32,
    /// Hunt radius of lane-less creeps (`ai::Hunter`).
    pub aggro_radius: f32,
    /// World label text, if the class shows one.
    pub tag: Option<&'static str>,
    /// Whether `convert_on_death` may raise the creep.
    pub convertible: bool,
}

impl CreepClass {
    pub const fn profile(self) -> CreepClassProfile {
        match self {
            CreepClass::Normal => CreepClassProfile {
                health_mult: 1.0,
                damage_mult: 1.0,
                armor_bonus: 0.0,
                model_scale: 1.0,
                bounty_mult: 1.0,
                xp_mult: 1.0,
                aggro_radius: 1600.0,
                tag: None,
                convertible: true,
            },
            CreepClass::Captain => CreepClassProfile {
                health_mult: 3.0,
                damage_mult: 1.6,
                armor_bonus: 4.0,
                model_scale: 1.35,
                bounty_mult: 3.0,
                xp_mult: 3.0,
                aggro_radius: 1800.0,
                tag: Some("Captain"),
                convertible: false,
            },
            CreepClass::Boss => CreepClassProfile {
                health_mult: 12.0,
                damage_mult: 2.5,
                armor_bonus: 8.0,
                model_scale: 1.9,
                bounty_mult: 10.0,
                xp_mult: 10.0,
                aggro_radius: 2400.0,
                tag: Some("Boss"),
                convertible: false,
            },
        }
    }
}

/// What to spawn: [`spawn_creep_with`]'s one parameter.
#[derive(Debug, Clone, Copy)]
pub struct CreepSpawn {
    pub team: Team,
    pub position: Vec3,
    pub ranged: bool,
    pub class: CreepClass,
    /// `Some`: follows the lane's waypoints. `None`: placed by a game mode.
    pub lane: Option<Lane>,
}

/// Spawn a lane creep of the classic stats ([`CreepClass::Normal`]).
pub fn spawn_creep(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    lane: Lane,
    position: Vec3,
    ranged: bool,
) -> Entity {
    spawn_creep_with(
        commands,
        assets,
        CreepSpawn {
            team,
            position,
            ranged,
            class: CreepClass::Normal,
            lane: Some(lane),
        },
    )
}

/// Spawn a creep of any class. The class [`CreepClassProfile`] scales its health, damage,
/// armor, size and bounties; a Captain or Boss also gets a [`WorldTag`] label.
pub fn spawn_creep_with(
    commands: &mut Commands,
    assets: &SharedAssets,
    spawn: CreepSpawn,
) -> Entity {
    let CreepSpawn {
        team,
        position,
        ranged,
        class,
        lane,
    } = spawn;
    let profile = class.profile();
    let mat = match team {
        Team::Radiant => assets.radiant_mat.clone(),
        Team::Dire => assets.dire_mat.clone(),
    };

    let attack_range = if ranged {
        scale::CREEP_RANGED_ATTACK_RANGE
    } else {
        scale::CREEP_MELEE_ATTACK_RANGE
    };
    let kind = if ranged { "Ranged Creep" } else { "Creep" };
    let name = match (lane, profile.tag) {
        (Some(lane), None) => format!("{team:?} {kind} ({lane:?})"),
        (Some(lane), Some(tag)) => format!("{team:?} {tag} {kind} ({lane:?})"),
        (None, None) => format!("{team:?} {kind}"),
        (None, Some(tag)) => format!("{team:?} {tag} {kind}"),
    };
    let base = if ranged {
        UnitDimensions::RANGED_CREEP
    } else {
        UnitDimensions::MELEE_CREEP
    };
    let dimensions = UnitDimensions {
        collision: base.collision * profile.model_scale,
        bounds: base.bounds * profile.model_scale,
        selection: SelectionBounds::new(
            base.selection.offset * profile.model_scale,
            base.selection.half_extents * profile.model_scale,
        ),
    };

    let id = commands
        .spawn((
            Name::new(name),
            Mesh3d(assets.creep_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(grounded(
                position,
                scale::CREEP_MODEL_HEIGHT * profile.model_scale,
            ))
            .with_scale(Vec3::splat(profile.model_scale)),
            team,
            Creep {
                lane,
                ranged,
                class,
            },
            Health::new((if ranged { 240.0 } else { 280.0 }) * profile.health_mult),
            CombatStats::simple(
                (if ranged { 15.0 } else { 18.0 }) * profile.damage_mult,
                attack_range,
                if ranged { 0.85 } else { 0.9 },
                1.0 + profile.armor_bonus,
                0.5,
                scale::CREEP_MOVE_SPEED,
            ),
            AttackCooldown(0.0),
        ))
        .id();
    commands.entity(id).insert((
        dimensions.components(),
        GoldBounty((35.0 * profile.bounty_mult).round() as u32),
        XpBounty((45.0 * profile.xp_mult).round() as u32),
        crate::status::StatusEffects::default(),
    ));
    if let Some(lane) = lane {
        commands.entity(id).insert(crate::ai::LaneFollower {
            waypoints: crate::map::lane_path(team, lane),
            index: 0,
        });
    }
    if lane.is_none() {
        commands.entity(id).insert(crate::ai::Hunter {
            aggro_radius: profile.aggro_radius,
        });
    }
    if let Some(tag) = profile.tag {
        commands.entity(id).insert(WorldTag(tag.to_string()));
    }
    attach_mobile_unit_details(commands, id, assets, team, false);
    id
}

/// How long a creep that rose again through `convert_on_death` lives. It is despawned
/// without a bounty when this runs out; killed before then it pays the normal bounties.
pub const CONVERTED_CREEP_LIFETIME_SECS: f32 = 30.0;

/// Spawns the creeps `combat::despawn_dead` decided to bring back: a fresh creep of the
/// converting unit's team at the death point (full health, normal bounties, the normal creep
/// AI) that resumes its lane forward from there and expires after
/// [`CONVERTED_CREEP_LIFETIME_SECS`]. Skipped without presentation assets.
pub fn spawn_converted_creeps(
    mut commands: Commands,
    assets: Option<Res<SharedAssets>>,
    mut events: MessageReader<crate::combat::CreepConvertedEvent>,
) {
    for event in events.read() {
        let Some(assets) = assets.as_deref() else {
            continue;
        };
        let id = spawn_creep_with(
            &mut commands,
            assets,
            CreepSpawn {
                team: event.team,
                position: event.position,
                ranged: event.ranged,
                class: CreepClass::Normal,
                lane: event.lane,
            },
        );
        if let Some(lane) = event.lane {
            commands
                .entity(id)
                .insert(crate::ai::LaneFollower::resume_from(
                    crate::map::lane_path(event.team, lane),
                    event.position,
                ));
        }
        commands
            .entity(id)
            .insert(Lifetime(CONVERTED_CREEP_LIFETIME_SECS));
    }
}

/// Stationary 1000 HP target beside the Radiant ancient. Enemy team so the player
/// can attack it; no lane follower, no attack, no movement.
pub fn training_dummy_position() -> Vec3 {
    scale::ground(-48.0, -51.0)
}

pub fn spawn_training_dummy(commands: &mut Commands, assets: &SharedAssets) {
    let position = training_dummy_position();
    let id = commands
        .spawn((
            Name::new("Training Dummy"),
            Mesh3d(assets.creep_mesh.clone()),
            MeshMaterial3d(assets.dire_mat.clone()),
            Transform::from_translation(grounded(position, scale::CREEP_MODEL_HEIGHT)),
            Team::Dire,
            TrainingDummy,
            Health::new(1000.0),
            CombatStats::simple(0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            AttackCooldown(0.0),
            UnitDimensions::MELEE_CREEP.components(),
            GoldBounty(0),
            XpBounty(0),
            crate::status::StatusEffects::default(),
        ))
        .id();
    attach_mobile_unit_details(commands, id, assets, Team::Dire, false);
}

pub fn spawn_tower(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    lane: Lane,
    position: Vec3,
) {
    let mat = match team {
        Team::Radiant => assets.tower_radiant_mat.clone(),
        Team::Dire => assets.tower_dire_mat.clone(),
    };
    let accent = match team {
        Team::Radiant => assets.tower_radiant_accent_mat.clone(),
        Team::Dire => assets.tower_dire_accent_mat.clone(),
    };

    let half_h = scale::TOWER_MODEL_HEIGHT * 0.5;
    commands
        .spawn((
            Name::new(format!("{team:?} Tower ({lane:?})")),
            Mesh3d(assets.tower_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(grounded(position, scale::TOWER_MODEL_HEIGHT)),
            team,
            Tower,
            lane,
            Health::new(1800.0),
            CombatStats::simple(90.0, scale::TOWER_ATTACK_RANGE, 0.85, 12.0, 8.0, 0.0),
            AttackCooldown(0.0),
            UnitDimensions::TOWER.components(),
            GoldBounty(120),
            XpBounty(150),
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(assets.tower_base_mesh.clone()),
                MeshMaterial3d(accent.clone()),
                Transform::from_xyz(0.0, -half_h + scale::TOWER_MODEL_HEIGHT * 0.06, 0.0),
            ));
            parent.spawn((
                Mesh3d(assets.tower_cap_mesh.clone()),
                MeshMaterial3d(accent),
                Transform::from_xyz(0.0, half_h + scale::TOWER_MODEL_HEIGHT * 0.08, 0.0),
            ));
        });
}

pub fn spawn_ancient(commands: &mut Commands, assets: &SharedAssets, team: Team, position: Vec3) {
    let mat = match team {
        Team::Radiant => assets.tower_radiant_mat.clone(),
        Team::Dire => assets.tower_dire_mat.clone(),
    };
    let accent = match team {
        Team::Radiant => assets.tower_radiant_accent_mat.clone(),
        Team::Dire => assets.tower_dire_accent_mat.clone(),
    };

    let mut stats = CombatStats::simple(0.0, 0.0, 0.0, 15.0, 12.0, 0.0);
    stats.attack_point = 0.0;
    stats.attack_backswing = 0.0;
    stats.base_attack_speed = 20.0;
    stats.recompute_attack_speed(0.0);

    let offset = scale::body(1.4);
    commands
        .spawn((
            Name::new(format!("{team:?} Ancient")),
            Mesh3d(assets.ancient_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(position),
            team,
            Ancient,
            Health::new(4000.0),
            stats,
            UnitDimensions::ANCIENT.components(),
            GoldBounty(0),
            XpBounty(400),
        ))
        .with_children(|parent| {
            for (x, z) in [
                (-offset, -offset),
                (offset, -offset),
                (-offset, offset),
                (offset, offset),
            ] {
                parent.spawn((
                    Mesh3d(assets.ancient_spire_mesh.clone()),
                    MeshMaterial3d(accent.clone()),
                    Transform::from_xyz(x, scale::body(0.8), z),
                ));
            }
        });
}

/// Nose (+ optional shoulders) so facing / silhouette read clearly from the high camera.
fn attach_mobile_unit_details(
    commands: &mut Commands,
    entity: Entity,
    assets: &SharedAssets,
    team: Team,
    hero: bool,
) {
    let accent = match team {
        Team::Radiant => assets.radiant_accent_mat.clone(),
        Team::Dire => assets.dire_accent_mat.clone(),
    };
    let body = match team {
        Team::Radiant => assets.radiant_mat.clone(),
        Team::Dire => assets.dire_mat.clone(),
    };

    let (nose_y, nose_z, shoulder_y) = if hero {
        (
            scale::HERO_MODEL_HEIGHT * 0.12,
            -scale::HERO_MODEL_DEPTH * 0.42,
            scale::HERO_MODEL_HEIGHT * 0.18,
        )
    } else {
        (
            scale::CREEP_MODEL_HEIGHT * 0.12,
            -scale::CREEP_MODEL_DEPTH * 0.42,
            0.0,
        )
    };

    commands.entity(entity).with_children(|parent| {
        parent.spawn((
            Name::new("FacingNose"),
            Mesh3d(assets.facing_nose_mesh.clone()),
            MeshMaterial3d(accent),
            Transform::from_xyz(0.0, nose_y, nose_z).with_scale(if hero {
                Vec3::ONE
            } else {
                Vec3::new(
                    scale::CREEP_MODEL_WIDTH / scale::HERO_MODEL_WIDTH,
                    scale::CREEP_MODEL_HEIGHT / scale::HERO_MODEL_HEIGHT,
                    scale::CREEP_MODEL_DEPTH / scale::HERO_MODEL_DEPTH,
                )
            }),
        ));
        if hero {
            parent.spawn((
                Name::new("Shoulders"),
                Mesh3d(assets.unit_shoulder_mesh.clone()),
                MeshMaterial3d(body),
                Transform::from_xyz(0.0, shoulder_y, 0.0),
            ));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{BoundRadius, CollisionRadius};
    use crate::obstacle_course::course_bounds;

    /// Spawn into a bare world (assets are default handles).
    fn spawn(spawn: CreepSpawn) -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<SharedAssets>();
        let assets = world.remove_resource::<SharedAssets>().unwrap();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let id = {
            let mut commands = Commands::new(&mut queue, &world);
            spawn_creep_with(&mut commands, &assets, spawn)
        };
        queue.apply(&mut world);
        world.insert_resource(assets);
        (world, id)
    }

    fn lane_spawn(class: CreepClass, ranged: bool) -> CreepSpawn {
        CreepSpawn {
            team: Team::Dire,
            position: Vec3::new(100.0, 0.0, -50.0),
            ranged,
            class,
            lane: Some(Lane::Mid),
        }
    }

    #[test]
    fn normal_class_lane_creep_is_identical_to_before() {
        for ranged in [false, true] {
            let (world, id) = spawn(lane_spawn(CreepClass::Normal, ranged));
            let health = world.get::<Health>(id).unwrap();
            assert_eq!(health.max, if ranged { 240.0 } else { 280.0 });
            assert_eq!(health.current, health.max);
            let stats = world.get::<CombatStats>(id).unwrap();
            assert_eq!(stats.attack_damage, if ranged { 15.0 } else { 18.0 });
            assert_eq!(stats.armor, 1.0);
            assert_eq!(world.get::<GoldBounty>(id).unwrap().0, 35);
            assert_eq!(world.get::<XpBounty>(id).unwrap().0, 45);
            let base = if ranged {
                UnitDimensions::RANGED_CREEP
            } else {
                UnitDimensions::MELEE_CREEP
            };
            assert_eq!(world.get::<CollisionRadius>(id).unwrap().0, base.collision);
            assert_eq!(world.get::<BoundRadius>(id).unwrap().0, base.bounds);
            assert_eq!(*world.get::<SelectionBounds>(id).unwrap(), base.selection);
            let transform = world.get::<Transform>(id).unwrap();
            assert_eq!(transform.scale, Vec3::ONE);
            assert_eq!(transform.translation.y, scale::CREEP_MODEL_HEIGHT * 0.5);
            let creep = world.get::<Creep>(id).unwrap();
            assert_eq!(creep.lane, Some(Lane::Mid));
            assert_eq!(creep.class, CreepClass::Normal);
            assert!(world.get::<crate::ai::LaneFollower>(id).is_some());
            assert!(world.get::<crate::ai::Hunter>(id).is_none());
            assert!(world.get::<WorldTag>(id).is_none());
        }
        // The classic entry point is the same thing.
        let mut world = World::new();
        let assets = SharedAssets::default();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let id = {
            let mut commands = Commands::new(&mut queue, &world);
            spawn_creep(
                &mut commands,
                &assets,
                Team::Dire,
                Lane::Top,
                Vec3::ZERO,
                false,
            )
        };
        queue.apply(&mut world);
        assert_eq!(world.get::<Creep>(id).unwrap().class, CreepClass::Normal);
        assert_eq!(world.get::<Creep>(id).unwrap().lane, Some(Lane::Top));
    }

    #[test]
    fn captain_and_boss_profiles_scale_stats_dimensions_and_bounties() {
        for class in [CreepClass::Captain, CreepClass::Boss] {
            let profile = class.profile();
            let (world, id) = spawn(CreepSpawn {
                lane: None,
                ..lane_spawn(class, false)
            });
            assert_eq!(
                world.get::<Health>(id).unwrap().max,
                280.0 * profile.health_mult
            );
            let stats = world.get::<CombatStats>(id).unwrap();
            assert_eq!(stats.attack_damage, 18.0 * profile.damage_mult);
            assert_eq!(stats.armor, 1.0 + profile.armor_bonus);
            assert_eq!(
                world.get::<GoldBounty>(id).unwrap().0,
                (35.0 * profile.bounty_mult).round() as u32
            );
            assert_eq!(
                world.get::<XpBounty>(id).unwrap().0,
                (45.0 * profile.xp_mult).round() as u32
            );
            let base = UnitDimensions::MELEE_CREEP;
            assert_eq!(
                world.get::<CollisionRadius>(id).unwrap().0,
                base.collision * profile.model_scale
            );
            assert_eq!(
                world.get::<BoundRadius>(id).unwrap().0,
                base.bounds * profile.model_scale
            );
            assert_eq!(
                world.get::<SelectionBounds>(id).unwrap().half_extents,
                base.selection.half_extents * profile.model_scale
            );
            let transform = world.get::<Transform>(id).unwrap();
            assert_eq!(transform.scale, Vec3::splat(profile.model_scale));
            assert_eq!(
                transform.translation.y,
                scale::CREEP_MODEL_HEIGHT * profile.model_scale * 0.5,
                "still rests on the ground"
            );
            let creep = world.get::<Creep>(id).unwrap();
            assert_eq!((creep.lane, creep.class), (None, class));
            assert!(
                world.get::<crate::ai::LaneFollower>(id).is_none(),
                "a lane-less creep follows no lane"
            );
            assert_eq!(
                world.get::<crate::ai::Hunter>(id).unwrap().aggro_radius,
                profile.aggro_radius,
                "it hunts instead"
            );
        }
    }

    #[test]
    fn class_profiles_are_monotonic() {
        let [normal, captain, boss] = [
            CreepClass::Normal.profile(),
            CreepClass::Captain.profile(),
            CreepClass::Boss.profile(),
        ];
        for (low, high) in [(normal, captain), (captain, boss)] {
            assert!(low.health_mult < high.health_mult);
            assert!(low.damage_mult < high.damage_mult);
            assert!(low.armor_bonus < high.armor_bonus);
            assert!(low.model_scale < high.model_scale);
            assert!(low.bounty_mult < high.bounty_mult);
            assert!(low.xp_mult < high.xp_mult);
            assert!(low.aggro_radius < high.aggro_radius);
        }
        assert!(normal.convertible && !captain.convertible && !boss.convertible);
        assert_eq!(normal.tag, None);
        assert_eq!(captain.tag, Some("Captain"));
        assert_eq!(boss.tag, Some("Boss"));
    }

    #[test]
    fn captain_and_boss_get_a_world_tag() {
        for (class, tag) in [(CreepClass::Captain, "Captain"), (CreepClass::Boss, "Boss")] {
            let (world, id) = spawn(lane_spawn(class, true));
            assert_eq!(world.get::<WorldTag>(id), Some(&WorldTag(tag.to_string())));
        }
    }

    #[test]
    fn training_dummy_stands_between_the_ancient_and_the_course() {
        let ancient = scale::ground(-48.0, -48.0);
        let dummy = training_dummy_position();
        let dist = Vec2::new(dummy.x - ancient.x, dummy.z - ancient.z).length();
        assert!(dist < scale::map(6.0), "{dist}");
        let (_min, max_xz) = course_bounds();
        assert!(dummy.z > max_xz.y, "dummy should be north of the course");
        assert_eq!(Health::new(1000.0).max, 1000.0);
    }
}
