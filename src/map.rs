//! Three-lane battlefield: bases, river, jungle pockets, and lane paths.

use bevy::prelude::*;

use crate::components::{Ground, Lane, Obstacle, Team};
use crate::resources::{MatchConfig, SharedAssets};
use crate::units::{spawn_ancient, spawn_tower};

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_map.after(crate::resources::load_shared_assets));
    }
}

fn spawn_map(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    assets: Res<SharedAssets>,
    config: Res<MatchConfig>,
) {
    let extent = config.map_half_extent * 2.0;

    // Playable ground plane (click target for movement orders).
    commands.spawn((
        Name::new("Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(extent, extent))),
        MeshMaterial3d(assets.ground_mat.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
        Ground,
    ));

    spawn_lane_strip(
        &mut commands,
        &mut meshes,
        &assets,
        Vec3::new(-40.0, 0.02, 40.0),
        Vec3::new(40.0, 0.02, -40.0),
        6.0,
        "Mid Lane",
    );
    spawn_lane_strip(
        &mut commands,
        &mut meshes,
        &assets,
        Vec3::new(-50.0, 0.02, -20.0),
        Vec3::new(20.0, 0.02, 50.0),
        5.5,
        "Top Lane",
    );
    spawn_lane_strip(
        &mut commands,
        &mut meshes,
        &assets,
        Vec3::new(-20.0, 0.02, -50.0),
        Vec3::new(50.0, 0.02, 20.0),
        5.5,
        "Bot Lane",
    );

    // River along the anti-diagonal.
    spawn_lane_strip(
        &mut commands,
        &mut meshes,
        &assets,
        Vec3::new(-55.0, 0.03, -55.0),
        Vec3::new(55.0, 0.03, 55.0),
        8.0,
        "River",
    );
    // Override river material by spawning a dedicated strip.
    commands.spawn((
        Name::new("River Surface"),
        Mesh3d(meshes.add(Cuboid::new(90.0, 0.05, 8.0))),
        MeshMaterial3d(assets.river_mat.clone()),
        Transform::from_xyz(0.0, 0.04, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
    ));

    // Jungle pockets.
    for (pos, name) in [
        (Vec3::new(-25.0, 0.05, 10.0), "Radiant Jungle NW"),
        (Vec3::new(-10.0, 0.05, 25.0), "Radiant Jungle NE"),
        (Vec3::new(25.0, 0.05, -10.0), "Dire Jungle SE"),
        (Vec3::new(10.0, 0.05, -25.0), "Dire Jungle SW"),
    ] {
        commands.spawn((
            Name::new(name),
            Mesh3d(meshes.add(Cuboid::new(10.0, 0.1, 10.0))),
            MeshMaterial3d(assets.jungle_mat.clone()),
            Transform::from_translation(pos),
        ));
    }

    spawn_tree_obstacles(&mut commands, &assets);

    // Lighting
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(30.0, 80.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.7, 0.75, 0.85),
        brightness: 120.0,
        ..default()
    });

    // Bases + defenses
    spawn_ancient(&mut commands, &assets, Team::Radiant, Vec3::new(-48.0, 1.25, -48.0));
    spawn_ancient(&mut commands, &assets, Team::Dire, Vec3::new(48.0, 1.25, 48.0));

    // Mid towers
    spawn_tower(
        &mut commands,
        &assets,
        Team::Radiant,
        Lane::Mid,
        Vec3::new(-32.0, 1.6, -32.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Radiant,
        Lane::Mid,
        Vec3::new(-16.0, 1.6, -16.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Dire,
        Lane::Mid,
        Vec3::new(16.0, 1.6, 16.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Dire,
        Lane::Mid,
        Vec3::new(32.0, 1.6, 32.0),
    );

    // Top towers
    spawn_tower(
        &mut commands,
        &assets,
        Team::Radiant,
        Lane::Top,
        Vec3::new(-45.0, 1.6, -10.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Dire,
        Lane::Top,
        Vec3::new(10.0, 1.6, 45.0),
    );

    // Bot towers
    spawn_tower(
        &mut commands,
        &assets,
        Team::Radiant,
        Lane::Bot,
        Vec3::new(-10.0, 1.6, -45.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Dire,
        Lane::Bot,
        Vec3::new(45.0, 1.6, 10.0),
    );
}

fn spawn_lane_strip(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    assets: &SharedAssets,
    from: Vec3,
    to: Vec3,
    width: f32,
    name: &str,
) {
    let mid = (from + to) * 0.5;
    let dir = to - from;
    let length = dir.length().max(1.0);
    let yaw = dir.x.atan2(dir.z);

    commands.spawn((
        Name::new(name.to_string()),
        Mesh3d(meshes.add(Cuboid::new(width, 0.08, length))),
        MeshMaterial3d(assets.lane_mat.clone()),
        Transform::from_translation(mid).with_rotation(Quat::from_rotation_y(yaw)),
    ));
}

/// Placeholder trees blocking travel between lanes (jungle corridors).
fn spawn_tree_obstacles(commands: &mut Commands, assets: &SharedAssets) {
    // Clusters sit off the lane strips so creep paths stay clear.
    let trees = [
        // Between mid and top (NW jungle)
        Vec3::new(-28.0, 1.75, 8.0),
        Vec3::new(-22.0, 1.75, 14.0),
        Vec3::new(-18.0, 1.75, 6.0),
        Vec3::new(-32.0, 1.75, 16.0),
        Vec3::new(-14.0, 1.75, 18.0),
        Vec3::new(-24.0, 1.75, 22.0),
        // Between mid and bot (SW jungle)
        Vec3::new(-8.0, 1.75, -28.0),
        Vec3::new(-14.0, 1.75, -22.0),
        Vec3::new(-6.0, 1.75, -18.0),
        Vec3::new(-16.0, 1.75, -32.0),
        Vec3::new(-18.0, 1.75, -14.0),
        Vec3::new(-22.0, 1.75, -24.0),
        // Between mid and top (SE / dire jungle)
        Vec3::new(28.0, 1.75, -8.0),
        Vec3::new(22.0, 1.75, -14.0),
        Vec3::new(18.0, 1.75, -6.0),
        Vec3::new(32.0, 1.75, -16.0),
        Vec3::new(14.0, 1.75, -18.0),
        Vec3::new(24.0, 1.75, -22.0),
        // Between mid and bot (NE / dire jungle)
        Vec3::new(8.0, 1.75, 28.0),
        Vec3::new(14.0, 1.75, 22.0),
        Vec3::new(6.0, 1.75, 18.0),
        Vec3::new(16.0, 1.75, 32.0),
        Vec3::new(18.0, 1.75, 14.0),
        Vec3::new(22.0, 1.75, 24.0),
        // Extra river-bank blockers
        Vec3::new(-8.0, 1.75, 8.0),
        Vec3::new(8.0, 1.75, -8.0),
        Vec3::new(-12.0, 1.75, -4.0),
        Vec3::new(12.0, 1.75, 4.0),
    ];

    for (i, pos) in trees.into_iter().enumerate() {
        commands.spawn((
            Name::new(format!("Tree {i}")),
            Mesh3d(assets.tree_mesh.clone()),
            MeshMaterial3d(assets.tree_mat.clone()),
            Transform::from_translation(pos),
            Obstacle { radius: 1.1 },
        ));
    }
}

/// Waypoints creeps follow down each lane toward the enemy ancient.
pub fn lane_path(team: Team, lane: Lane) -> Vec<Vec3> {
    let radiant_paths = match lane {
        Lane::Mid => vec![
            Vec3::new(-45.0, 0.0, -45.0),
            Vec3::new(-30.0, 0.0, -30.0),
            Vec3::new(-15.0, 0.0, -15.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(15.0, 0.0, 15.0),
            Vec3::new(30.0, 0.0, 30.0),
            Vec3::new(45.0, 0.0, 45.0),
        ],
        Lane::Top => vec![
            Vec3::new(-48.0, 0.0, -40.0),
            Vec3::new(-48.0, 0.0, -10.0),
            Vec3::new(-30.0, 0.0, 20.0),
            Vec3::new(0.0, 0.0, 40.0),
            Vec3::new(30.0, 0.0, 48.0),
            Vec3::new(45.0, 0.0, 48.0),
        ],
        Lane::Bot => vec![
            Vec3::new(-40.0, 0.0, -48.0),
            Vec3::new(-10.0, 0.0, -48.0),
            Vec3::new(20.0, 0.0, -30.0),
            Vec3::new(40.0, 0.0, 0.0),
            Vec3::new(48.0, 0.0, 30.0),
            Vec3::new(48.0, 0.0, 45.0),
        ],
    };

    match team {
        Team::Radiant => radiant_paths,
        Team::Dire => radiant_paths.into_iter().rev().collect(),
    }
}
