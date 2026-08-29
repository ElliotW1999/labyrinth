//! Factory helpers for heroes, creeps, towers, and ancients.

use bevy::prelude::*;

use crate::components::{
    Ancient, AttackCooldown, CombatStats, Creep, GoldBounty, Health, Lane, PlayerHero, PlayerWallet,
    Team, Tower, UnitRadius, XpBounty,
};
use crate::heroes::{HeroId, HeroKind};
use crate::items::{Inventory, StatusEffects};
use crate::net::{NetworkId, NetworkedHero};
use crate::resources::SharedAssets;

pub struct UnitsPlugin;

impl Plugin for UnitsPlugin {
    fn build(&self, _app: &mut App) {
        // Heroes spawn after local selection (see HeroesPlugin).
    }
}

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
    let (health, mana, stats, attrs) = def.vitals();

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
        Mesh3d(assets.unit_mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(position),
        team,
        NetworkedHero,
        NetworkId(network_id),
        HeroKind(hero),
        PlayerWallet { gold: 600 },
        health,
        mana,
        stats,
        AttackCooldown(0.0),
        hero.loadout(),
        crate::components::HeroProgress::new(),
    ));
    entity.insert((
        UnitRadius(0.5),
        GoldBounty(0),
        attrs,
        Inventory::empty(),
        StatusEffects::default(),
    ));
    if local {
        entity.insert(PlayerHero);
    }
    entity.id()
}

pub fn spawn_creep(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    lane: Lane,
    position: Vec3,
) {
    let mat = match team {
        Team::Radiant => assets.radiant_mat.clone(),
        Team::Dire => assets.dire_mat.clone(),
    };

    commands.spawn((
        Name::new(format!("{team:?} Creep ({lane:?})")),
        Mesh3d(assets.unit_mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(position + Vec3::Y * 0.7).with_scale(Vec3::splat(0.75)),
        team,
        Creep { lane },
        Health::new(280.0),
        CombatStats {
            attack_damage: 18.0,
            attack_range: 4.5,
            attack_speed: 0.9,
            armor: 1.0,
            magic_resist: 0.5,
            move_speed: 7.5,
        },
        AttackCooldown(0.0),
        UnitRadius(0.4),
        GoldBounty(35),
        XpBounty(45),
        crate::ai::LaneFollower {
            waypoints: crate::map::lane_path(team, lane),
            index: 0,
        },
    ));
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

    commands.spawn((
        Name::new(format!("{team:?} Tower ({lane:?})")),
        Mesh3d(assets.tower_mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(position),
        team,
        Tower,
        lane,
        Health::new(1800.0),
        CombatStats {
            attack_damage: 90.0,
            attack_range: 14.0,
            attack_speed: 0.85,
            armor: 12.0,
            magic_resist: 8.0,
            move_speed: 0.0,
        },
        AttackCooldown(0.0),
        UnitRadius(0.9),
        GoldBounty(120),
        XpBounty(150),
    ));
}

pub fn spawn_ancient(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    position: Vec3,
) {
    let mat = match team {
        Team::Radiant => assets.tower_radiant_mat.clone(),
        Team::Dire => assets.tower_dire_mat.clone(),
    };

    commands.spawn((
        Name::new(format!("{team:?} Ancient")),
        Mesh3d(assets.ancient_mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(position),
        team,
        Ancient,
        Health::new(4000.0),
        CombatStats {
            attack_damage: 0.0,
            attack_range: 0.0,
            attack_speed: 0.0,
            armor: 15.0,
            magic_resist: 12.0,
            move_speed: 0.0,
        },
        UnitRadius(1.8),
        GoldBounty(0),
        XpBounty(400),
    ));
}
