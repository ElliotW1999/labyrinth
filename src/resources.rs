//! Shared materials, meshes, and match configuration.

use bevy::prelude::*;

use crate::scale;

pub struct ResourcesPlugin;

impl Plugin for ResourcesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MatchConfig>()
            .init_resource::<SharedAssets>()
            .add_systems(Startup, load_shared_assets);
    }
}

#[derive(Resource, Debug, Clone)]
pub struct MatchConfig {
    pub map_half_extent: f32,
    pub creep_wave_interval: f32,
    pub creeps_per_wave: usize,
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            map_half_extent: scale::u(70.0),
            creep_wave_interval: 30.0,
            creeps_per_wave: 4,
        }
    }
}

#[derive(Resource, Default)]
pub struct SharedAssets {
    pub unit_mesh: Handle<Mesh>,
    pub unit_shoulder_mesh: Handle<Mesh>,
    pub facing_nose_mesh: Handle<Mesh>,
    pub tower_mesh: Handle<Mesh>,
    pub tower_cap_mesh: Handle<Mesh>,
    pub tower_base_mesh: Handle<Mesh>,
    pub ancient_mesh: Handle<Mesh>,
    pub ancient_spire_mesh: Handle<Mesh>,
    pub projectile_mesh: Handle<Mesh>,
    pub melee_slash_mesh: Handle<Mesh>,
    pub health_bar_bg_mesh: Handle<Mesh>,
    pub health_bar_fill_mesh: Handle<Mesh>,
    pub radiant_mat: Handle<StandardMaterial>,
    pub dire_mat: Handle<StandardMaterial>,
    pub radiant_accent_mat: Handle<StandardMaterial>,
    pub dire_accent_mat: Handle<StandardMaterial>,
    pub tower_radiant_mat: Handle<StandardMaterial>,
    pub tower_dire_mat: Handle<StandardMaterial>,
    pub tower_radiant_accent_mat: Handle<StandardMaterial>,
    pub tower_dire_accent_mat: Handle<StandardMaterial>,
    pub projectile_radiant_mat: Handle<StandardMaterial>,
    pub projectile_dire_mat: Handle<StandardMaterial>,
    pub melee_slash_radiant_mat: Handle<StandardMaterial>,
    pub melee_slash_dire_mat: Handle<StandardMaterial>,
    pub health_bar_bg_mat: Handle<StandardMaterial>,
    pub health_bar_fill_mat: Handle<StandardMaterial>,
    pub ground_mat: Handle<StandardMaterial>,
    pub lane_mat: Handle<StandardMaterial>,
    pub river_mat: Handle<StandardMaterial>,
    pub jungle_mat: Handle<StandardMaterial>,
    pub tree_mesh: Handle<Mesh>,
    pub tree_canopy_mesh: Handle<Mesh>,
    pub tree_mat: Handle<StandardMaterial>,
    pub tree_canopy_mat: Handle<StandardMaterial>,
    pub spell_bolt_mesh: Handle<Mesh>,
    pub spell_bolt_mat: Handle<StandardMaterial>,
    pub indicator_range_mat: Handle<StandardMaterial>,
    pub indicator_aoe_mat: Handle<StandardMaterial>,
    pub attack_range_ring_mat: Handle<StandardMaterial>,
    pub indicator_ring_mesh: Handle<Mesh>,
    pub indicator_beam_mesh: Handle<Mesh>,
    pub shockwave_mat: Handle<StandardMaterial>,
    pub nova_mat: Handle<StandardMaterial>,
    pub dash_ghost_mat: Handle<StandardMaterial>,
}

pub(crate) fn load_shared_assets(
    mut assets: ResMut<SharedAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Body meshes use `body()` so they track the tree-radius anchor (not attack ranges).
    assets.unit_mesh = meshes.add(Capsule3d::new(scale::body(0.35), scale::body(0.9)));
    // Shoulders / arms — sit beside the capsule to break the silhouette.
    assets.unit_shoulder_mesh =
        meshes.add(Cuboid::new(scale::body(0.85), scale::body(0.28), scale::body(0.28)));
    // Facing marker — elongated along local -Z (Bevy forward after yaw).
    assets.facing_nose_mesh =
        meshes.add(Cuboid::new(scale::body(0.22), scale::body(0.18), scale::body(0.7)));
    assets.tower_mesh = meshes.add(Cylinder::new(scale::body(0.7), scale::body(3.2)));
    assets.tower_cap_mesh = meshes.add(Cone::new(scale::body(0.95), scale::body(1.1)));
    assets.tower_base_mesh = meshes.add(Cylinder::new(scale::body(1.15), scale::body(0.35)));
    assets.ancient_mesh = meshes.add(Cuboid::new(
        scale::body(3.5),
        scale::body(2.5),
        scale::body(3.5),
    ));
    assets.ancient_spire_mesh = meshes.add(Cuboid::new(
        scale::body(0.7),
        scale::body(3.2),
        scale::body(0.7),
    ));
    // Elongated dart — local forward is -Z after look_to.
    assets.projectile_mesh =
        meshes.add(Cuboid::new(scale::body(0.18), scale::body(0.18), scale::body(0.85)));
    // Unit cuboid (X=thickness, Y=height, Z=length). Scaled per-slash to attack range.
    assets.melee_slash_mesh = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    // Health bars / indicators are scaled in world units via Transform — keep mesh size = 1.
    // Y thickness is ~3× the prior readable size for the high MOBA camera.
    assets.health_bar_bg_mesh = meshes.add(Cuboid::new(1.0, 9.6, 0.55));
    assets.health_bar_fill_mesh = meshes.add(Cuboid::new(1.0, 8.1, 0.65));

    assets.radiant_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.55, 0.95),
        perceptual_roughness: 0.7,
        ..default()
    });
    assets.dire_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.9, 0.3, 0.25),
        perceptual_roughness: 0.7,
        ..default()
    });
    assets.radiant_accent_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.85, 1.0),
        emissive: LinearRgba::rgb(0.8, 2.0, 4.0),
        perceptual_roughness: 0.4,
        ..default()
    });
    assets.dire_accent_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.55, 0.25),
        emissive: LinearRgba::rgb(4.0, 1.2, 0.3),
        perceptual_roughness: 0.4,
        ..default()
    });
    assets.tower_radiant_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.15, 0.35, 0.75),
        perceptual_roughness: 0.55,
        metallic: 0.2,
        ..default()
    });
    assets.tower_dire_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.7, 0.18, 0.15),
        perceptual_roughness: 0.55,
        metallic: 0.2,
        ..default()
    });
    assets.tower_radiant_accent_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.35, 0.65, 1.0),
        emissive: LinearRgba::rgb(0.4, 1.2, 3.0),
        perceptual_roughness: 0.35,
        metallic: 0.35,
        ..default()
    });
    assets.tower_dire_accent_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.4, 0.25),
        emissive: LinearRgba::rgb(3.0, 0.8, 0.2),
        perceptual_roughness: 0.35,
        metallic: 0.35,
        ..default()
    });
    assets.projectile_radiant_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.85, 1.0),
        emissive: LinearRgba::rgb(1.5, 4.0, 8.0),
        unlit: true,
        ..default()
    });
    assets.projectile_dire_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.55, 0.25),
        emissive: LinearRgba::rgb(8.0, 2.5, 0.4),
        unlit: true,
        ..default()
    });
    assets.melee_slash_radiant_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.75, 0.9, 1.0, 0.7),
        emissive: LinearRgba::rgb(1.2, 2.5, 4.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    assets.melee_slash_dire_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.7, 0.4, 0.7),
        emissive: LinearRgba::rgb(5.0, 1.8, 0.4),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    assets.health_bar_bg_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.08, 0.08, 0.1),
        unlit: true,
        alpha_mode: AlphaMode::Opaque,
        ..default()
    });
    assets.health_bar_fill_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.85, 0.35),
        unlit: true,
        ..default()
    });
    assets.ground_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.18, 0.28, 0.16),
        perceptual_roughness: 1.0,
        ..default()
    });
    assets.lane_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.35, 0.32, 0.28),
        perceptual_roughness: 0.95,
        ..default()
    });
    assets.river_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.4, 0.55),
        perceptual_roughness: 0.3,
        metallic: 0.1,
        ..default()
    });
    assets.jungle_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.22, 0.1),
        perceptual_roughness: 1.0,
        ..default()
    });
    // Trunk matches collision cylinder radius ([`scale::TREE_RADIUS`]).
    assets.tree_mesh = meshes.add(Cylinder::new(scale::TREE_RADIUS, scale::body(3.2)));
    // Foliar crown — wider short cylinder stacked on the trunk.
    assets.tree_canopy_mesh = meshes.add(Cylinder::new(scale::body(1.8), scale::body(1.2)));
    assets.tree_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.28, 0.18, 0.1),
        perceptual_roughness: 0.95,
        ..default()
    });
    assets.tree_canopy_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.14, 0.42, 0.18),
        perceptual_roughness: 0.9,
        ..default()
    });
    assets.spell_bolt_mesh = meshes.add(Sphere::new(scale::body(0.35)));
    assets.spell_bolt_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.75, 0.35, 1.0),
        emissive: LinearRgba::rgb(6.0, 1.5, 10.0),
        unlit: true,
        ..default()
    });
    assets.indicator_ring_mesh = meshes.add(Cylinder::new(1.0, 0.05));
    // Unit cuboid: X = width, Z = length — scaled for TargetPoint trajectories.
    assets.indicator_beam_mesh = meshes.add(Cuboid::new(1.0, 0.08, 1.0));
    assets.indicator_range_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.3, 0.7, 1.0, 0.22),
        emissive: LinearRgba::rgb(0.2, 0.5, 1.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    assets.indicator_aoe_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.55, 0.2, 0.28),
        emissive: LinearRgba::rgb(1.2, 0.4, 0.1),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    assets.attack_range_ring_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.38),
        emissive: LinearRgba::rgb(1.5, 1.5, 1.5),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    assets.shockwave_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.4, 0.85, 1.0, 0.45),
        emissive: LinearRgba::rgb(1.0, 3.0, 5.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    assets.nova_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.85, 0.3, 0.5),
        emissive: LinearRgba::rgb(5.0, 3.5, 0.5),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    assets.dash_ghost_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.4, 0.7, 1.0, 0.35),
        emissive: LinearRgba::rgb(0.5, 1.5, 3.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
}
