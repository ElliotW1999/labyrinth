//! Factory helpers for heroes, creeps, towers, and ancients.

use bevy::prelude::*;

use crate::components::{
    Ancient, AttackCooldown, BoundRadius, CollisionRadius, CombatStats, Creep, GoldBounty, Health,
    Lane, PlayerHero, PlayerWallet, SelectionBox, Team, Tower, XpBounty,
};
use crate::heroes::{HeroId, HeroKind};
use crate::items::{Inventory, StatusEffects};
use crate::net::{NetworkId, NetworkedHero};
use crate::resources::SharedAssets;
use crate::scale;

pub struct UnitsPlugin;

impl Plugin for UnitsPlugin {
    fn build(&self, _app: &mut App) {
        // Heroes spawn after local selection (see HeroesPlugin).
    }
}

/// Capsule mesh diameter used for selection boxes (before per-creep scale).
fn unit_model_diameter() -> f32 {
    scale::body(0.35) * 2.0
}

fn tower_model_diameter() -> f32 {
    scale::body(0.7) * 2.0
}

fn ancient_model_side() -> f32 {
    scale::body(3.5)
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

    let model_d = unit_model_diameter();
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
        BoundRadius(scale::HERO_BOUND),
        CollisionRadius(scale::HERO_COLLISION),
        SelectionBox {
            half_extent: scale::selection_half(model_d, model_d),
        },
        GoldBounty(0),
        attrs,
        Inventory::empty(),
        StatusEffects::default(),
    ));
    if local {
        entity.insert(PlayerHero);
    }
    let id = entity.id();
    attach_mobile_unit_details(commands, id, assets, team, true);
    id
}

pub fn spawn_creep(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    lane: Lane,
    position: Vec3,
    ranged: bool,
) {
    let mat = match team {
        Team::Radiant => assets.radiant_mat.clone(),
        Team::Dire => assets.dire_mat.clone(),
    };

    let attack_range = if ranged {
        scale::CREEP_RANGED_ATTACK_RANGE
    } else {
        scale::CREEP_MELEE_ATTACK_RANGE
    };
    let name = if ranged {
        format!("{team:?} Ranged Creep ({lane:?})")
    } else {
        format!("{team:?} Creep ({lane:?})")
    };
    let mesh_scale = if ranged { 0.7 } else { 0.75 };
    let model_d = unit_model_diameter() * mesh_scale;
    let (bound, collision) = if ranged {
        (scale::RANGED_CREEP_BOUND, scale::RANGED_CREEP_COLLISION)
    } else {
        (scale::MELEE_CREEP_BOUND, scale::MELEE_CREEP_COLLISION)
    };

    let id = commands
        .spawn((
            Name::new(name),
            Mesh3d(assets.unit_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(position + Vec3::Y * scale::body(0.7))
                .with_scale(Vec3::splat(mesh_scale)),
            team,
            Creep { lane },
            Health::new(if ranged { 240.0 } else { 280.0 }),
            CombatStats::simple(
                if ranged { 15.0 } else { 18.0 },
                attack_range,
                if ranged { 0.85 } else { 0.9 },
                1.0,
                0.5,
                scale::CREEP_MOVE_SPEED,
            ),
            AttackCooldown(0.0),
        ))
        .id();
    commands.entity(id).insert((
        BoundRadius(bound),
        CollisionRadius(collision),
        SelectionBox {
            half_extent: scale::selection_half(model_d, model_d),
        },
        GoldBounty(35),
        XpBounty(45),
        crate::items::StatusEffects::default(),
        crate::ai::LaneFollower {
            waypoints: crate::map::lane_path(team, lane),
            index: 0,
        },
    ));
    attach_mobile_unit_details(commands, id, assets, team, false);
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

    let model_d = tower_model_diameter();
    commands
        .spawn((
            Name::new(format!("{team:?} Tower ({lane:?})")),
            Mesh3d(assets.tower_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(position),
            team,
            Tower,
            lane,
            Health::new(1800.0),
            CombatStats::simple(90.0, scale::TOWER_ATTACK_RANGE, 0.85, 12.0, 8.0, 0.0),
            AttackCooldown(0.0),
            BoundRadius(scale::TOWER_BOUND),
            CollisionRadius(scale::TOWER_COLLISION),
            SelectionBox {
                half_extent: scale::selection_half(model_d, model_d),
            },
            GoldBounty(120),
            XpBounty(150),
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(assets.tower_base_mesh.clone()),
                MeshMaterial3d(accent.clone()),
                Transform::from_xyz(0.0, scale::body(-1.4), 0.0),
            ));
            parent.spawn((
                Mesh3d(assets.tower_cap_mesh.clone()),
                MeshMaterial3d(accent),
                Transform::from_xyz(0.0, scale::body(2.0), 0.0),
            ));
        });
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
    let side = ancient_model_side();
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
            BoundRadius(scale::ANCIENT_BOUND),
            CollisionRadius(scale::ANCIENT_COLLISION),
            SelectionBox {
                half_extent: scale::selection_half(side, side),
            },
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

    commands.entity(entity).with_children(|parent| {
        parent.spawn((
            Name::new("FacingNose"),
            Mesh3d(assets.facing_nose_mesh.clone()),
            MeshMaterial3d(accent),
            Transform::from_xyz(0.0, scale::body(0.35), scale::body(-0.55)),
        ));
        if hero {
            parent.spawn((
                Name::new("Shoulders"),
                Mesh3d(assets.unit_shoulder_mesh.clone()),
                MeshMaterial3d(body),
                Transform::from_xyz(0.0, scale::body(0.55), 0.0),
            ));
        }
    });
}
