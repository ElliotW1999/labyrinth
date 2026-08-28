//! Factory helpers for heroes, creeps, towers, and ancients.

use bevy::prelude::*;

use crate::components::{
    AbilityLoadout, Ancient, AttackCooldown, CombatStats, Creep, GoldBounty, Health, HeroProgress,
    Lane, Mana, PlayerHero, PlayerWallet, Team, Tower, UnitRadius, XpBounty,
};
use crate::resources::SharedAssets;

pub struct UnitsPlugin;

impl Plugin for UnitsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player_hero.after(crate::resources::load_shared_assets));
    }
}

pub fn spawn_player_hero(mut commands: Commands, assets: Res<SharedAssets>) {
    commands.spawn((
        Name::new("Player Hero"),
        Mesh3d(assets.unit_mesh.clone()),
        MeshMaterial3d(assets.radiant_mat.clone()),
        Transform::from_xyz(-44.0, 0.9, -44.0),
        Team::Radiant,
        PlayerHero,
        PlayerWallet { gold: 600 },
        Health::new(720.0),
        Mana::new(320.0, 12.0),
        CombatStats {
            attack_damage: 55.0,
            attack_range: 8.0,
            attack_speed: 1.1,
            armor: 4.0,
            move_speed: 12.0,
        },
        AttackCooldown(0.0),
        AbilityLoadout::starter(),
        HeroProgress::new(),
        UnitRadius(0.5),
        GoldBounty(0),
    ));
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
            move_speed: 0.0,
        },
        UnitRadius(1.8),
        GoldBounty(0),
        XpBounty(400),
    ));
}
