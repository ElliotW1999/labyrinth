//! Shared materials, meshes, and match configuration.

use bevy::prelude::*;

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
            map_half_extent: 60.0,
            creep_wave_interval: 30.0,
            creeps_per_wave: 4,
        }
    }
}

#[derive(Resource, Default)]
pub struct SharedAssets {
    pub unit_mesh: Handle<Mesh>,
    pub tower_mesh: Handle<Mesh>,
    pub ancient_mesh: Handle<Mesh>,
    pub projectile_mesh: Handle<Mesh>,
    pub health_bar_bg_mesh: Handle<Mesh>,
    pub health_bar_fill_mesh: Handle<Mesh>,
    pub radiant_mat: Handle<StandardMaterial>,
    pub dire_mat: Handle<StandardMaterial>,
    pub tower_radiant_mat: Handle<StandardMaterial>,
    pub tower_dire_mat: Handle<StandardMaterial>,
    pub projectile_radiant_mat: Handle<StandardMaterial>,
    pub projectile_dire_mat: Handle<StandardMaterial>,
    pub health_bar_bg_mat: Handle<StandardMaterial>,
    pub health_bar_fill_mat: Handle<StandardMaterial>,
    pub ground_mat: Handle<StandardMaterial>,
    pub lane_mat: Handle<StandardMaterial>,
    pub river_mat: Handle<StandardMaterial>,
    pub jungle_mat: Handle<StandardMaterial>,
    pub tree_mesh: Handle<Mesh>,
    pub tree_mat: Handle<StandardMaterial>,
}

pub(crate) fn load_shared_assets(
    mut assets: ResMut<SharedAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    assets.unit_mesh = meshes.add(Capsule3d::new(0.35, 0.9));
    assets.tower_mesh = meshes.add(Cylinder::new(0.7, 3.2));
    assets.ancient_mesh = meshes.add(Cuboid::new(3.5, 2.5, 3.5));
    // Elongated dart — local forward is -Z after look_to.
    assets.projectile_mesh = meshes.add(Cuboid::new(0.18, 0.18, 0.85));
    assets.health_bar_bg_mesh = meshes.add(Cuboid::new(1.0, 0.12, 0.08));
    assets.health_bar_fill_mesh = meshes.add(Cuboid::new(1.0, 0.1, 0.09));

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
    assets.tree_mesh = meshes.add(Cylinder::new(0.85, 3.5));
    assets.tree_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.38, 0.18),
        perceptual_roughness: 0.9,
        ..default()
    });
}
