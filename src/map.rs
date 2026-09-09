//! Three-lane battlefield: bases, river, jungle pockets, and lane paths.

use bevy::prelude::*;

use crate::components::{Ground, Lane, Obstacle, Team};
use crate::resources::{MatchConfig, SharedAssets};
use crate::scale;
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
        scale::v(-40.0, 0.02, 40.0),
        scale::v(40.0, 0.02, -40.0),
        scale::map(6.0),
        "Mid Lane",
    );
    spawn_lane_strip(
        &mut commands,
        &mut meshes,
        &assets,
        scale::v(-50.0, 0.02, -20.0),
        scale::v(20.0, 0.02, 50.0),
        scale::map(5.5),
        "Top Lane",
    );
    spawn_lane_strip(
        &mut commands,
        &mut meshes,
        &assets,
        scale::v(-20.0, 0.02, -50.0),
        scale::v(50.0, 0.02, 20.0),
        scale::map(5.5),
        "Bot Lane",
    );

    spawn_lane_strip(
        &mut commands,
        &mut meshes,
        &assets,
        scale::v(-55.0, 0.03, -55.0),
        scale::v(55.0, 0.03, 55.0),
        scale::map(8.0),
        "River",
    );
    commands.spawn((
        Name::new("River Surface"),
        Mesh3d(meshes.add(Cuboid::new(
            scale::map(90.0),
            scale::body(0.05),
            scale::map(8.0),
        ))),
        MeshMaterial3d(assets.river_mat.clone()),
        Transform::from_xyz(0.0, 0.04, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
    ));

    for (pos, name) in [
        (scale::ground(-25.0, 10.0), "Radiant Jungle NW"),
        (scale::ground(-10.0, 25.0), "Radiant Jungle NE"),
        (scale::ground(25.0, -10.0), "Dire Jungle SE"),
        (scale::ground(10.0, -25.0), "Dire Jungle SW"),
    ] {
        commands.spawn((
            Name::new(name),
            Mesh3d(meshes.add(Cuboid::new(
                scale::map(10.0),
                scale::body(0.1),
                scale::map(10.0),
            ))),
            MeshMaterial3d(assets.jungle_mat.clone()),
            Transform::from_translation(pos + Vec3::Y * scale::body(0.05)),
        ));
    }

    spawn_tree_obstacles(&mut commands, &assets);

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(scale::map(30.0), scale::body(80.0), scale::map(20.0))
            .looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.7, 0.75, 0.85),
        brightness: 120.0,
        ..default()
    });

    spawn_ancient(
        &mut commands,
        &assets,
        Team::Radiant,
        scale::v(-48.0, 1.25, -48.0),
    );
    spawn_ancient(&mut commands, &assets, Team::Dire, scale::v(48.0, 1.25, 48.0));

    spawn_tower(
        &mut commands,
        &assets,
        Team::Radiant,
        Lane::Mid,
        scale::v(-32.0, 1.6, -32.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Radiant,
        Lane::Mid,
        scale::v(-16.0, 1.6, -16.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Dire,
        Lane::Mid,
        scale::v(16.0, 1.6, 16.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Dire,
        Lane::Mid,
        scale::v(32.0, 1.6, 32.0),
    );

    spawn_tower(
        &mut commands,
        &assets,
        Team::Radiant,
        Lane::Top,
        scale::v(-45.0, 1.6, -10.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Dire,
        Lane::Top,
        scale::v(10.0, 1.6, 45.0),
    );

    spawn_tower(
        &mut commands,
        &assets,
        Team::Radiant,
        Lane::Bot,
        scale::v(-10.0, 1.6, -45.0),
    );
    spawn_tower(
        &mut commands,
        &assets,
        Team::Dire,
        Lane::Bot,
        scale::v(45.0, 1.6, 10.0),
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
    let delta = to - from;
    let length = delta.length();
    let mid = (from + to) * 0.5;
    let angle = delta.x.atan2(delta.z);
    commands.spawn((
        Name::new(name.to_string()),
        Mesh3d(meshes.add(Cuboid::new(width, scale::body(0.08), length))),
        MeshMaterial3d(assets.lane_mat.clone()),
        Transform::from_translation(mid).with_rotation(Quat::from_rotation_y(angle)),
    ));
}

/// Jungle trees — AABB collision 128×128, model smaller. Kept off the lane corridors.
fn spawn_tree_obstacles(commands: &mut Commands, assets: &SharedAssets) {
    let trunk_y = scale::TREE_MODEL_HEIGHT * 0.5;
    let canopy_y = scale::TREE_MODEL_HEIGHT * 0.55;
    // Positions in jungle pockets, well clear of mid/top/bot lane strips.
    let trees = [
        // Radiant NW jungle
        scale::ground(-30.0, 14.0),
        scale::ground(-26.0, 20.0),
        scale::ground(-34.0, 22.0),
        scale::ground(-22.0, 16.0),
        scale::ground(-28.0, 26.0),
        // Radiant NE jungle (toward top)
        scale::ground(-12.0, 30.0),
        scale::ground(-6.0, 34.0),
        scale::ground(-16.0, 36.0),
        scale::ground(-8.0, 28.0),
        // Dire SE jungle
        scale::ground(30.0, -14.0),
        scale::ground(26.0, -20.0),
        scale::ground(34.0, -22.0),
        scale::ground(22.0, -16.0),
        scale::ground(28.0, -26.0),
        // Dire SW jungle (toward bot)
        scale::ground(12.0, -30.0),
        scale::ground(6.0, -34.0),
        scale::ground(16.0, -36.0),
        scale::ground(8.0, -28.0),
        // Extra pocket fillers away from river/lanes
        scale::ground(-36.0, 8.0),
        scale::ground(36.0, -8.0),
        scale::ground(-8.0, 38.0),
        scale::ground(8.0, -38.0),
    ];

    for (i, pos) in trees.into_iter().enumerate() {
        let mut at = pos;
        at.y = trunk_y;
        commands
            .spawn((
                Name::new(format!("Tree {i}")),
                Mesh3d(assets.tree_mesh.clone()),
                MeshMaterial3d(assets.tree_mat.clone()),
                Transform::from_translation(at),
                Obstacle::aabb(scale::TREE_COLLISION_HALF, scale::TREE_COLLISION_HALF),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Mesh3d(assets.tree_canopy_mesh.clone()),
                    MeshMaterial3d(assets.tree_canopy_mat.clone()),
                    Transform::from_xyz(0.0, canopy_y - trunk_y, 0.0),
                ));
            });
    }
}

/// Waypoints creeps follow down each lane toward the enemy ancient.
pub fn lane_path(team: Team, lane: Lane) -> Vec<Vec3> {
    let radiant_paths = match lane {
        Lane::Mid => vec![
            scale::ground(-45.0, -45.0),
            scale::ground(-30.0, -30.0),
            scale::ground(-15.0, -15.0),
            scale::ground(0.0, 0.0),
            scale::ground(15.0, 15.0),
            scale::ground(30.0, 30.0),
            scale::ground(45.0, 45.0),
        ],
        Lane::Top => vec![
            scale::ground(-48.0, -40.0),
            scale::ground(-48.0, -10.0),
            scale::ground(-30.0, 20.0),
            scale::ground(0.0, 40.0),
            scale::ground(30.0, 48.0),
            scale::ground(45.0, 48.0),
        ],
        Lane::Bot => vec![
            scale::ground(-40.0, -48.0),
            scale::ground(-10.0, -48.0),
            scale::ground(20.0, -30.0),
            scale::ground(40.0, 0.0),
            scale::ground(48.0, 30.0),
            scale::ground(48.0, 45.0),
        ],
    };

    match team {
        Team::Radiant => radiant_paths,
        Team::Dire => radiant_paths.into_iter().rev().collect(),
    }
}
