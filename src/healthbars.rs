//! Billboard health bars floating above living units.

use bevy::prelude::*;

use crate::camera::GameCamera;
use crate::components::{HasHealthBar, Health, HealthBar, HealthBarFill, UnitRadius};
use crate::resources::SharedAssets;

pub struct HealthBarPlugin;

impl Plugin for HealthBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                attach_health_bars,
                sync_health_bars,
                cull_orphan_health_bars,
            )
                .chain(),
        );
    }
}

fn attach_health_bars(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    units: Query<(Entity, Option<&UnitRadius>), (With<Health>, Without<HasHealthBar>)>,
) {
    for (entity, radius) in &units {
        let width = bar_width(radius);

        commands
            .spawn((
                Name::new("HealthBar"),
                HealthBar { owner: entity },
                Transform::default(),
                Visibility::default(),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Mesh3d(assets.health_bar_bg_mesh.clone()),
                    MeshMaterial3d(assets.health_bar_bg_mat.clone()),
                    Transform::from_xyz(0.0, 0.0, -0.01).with_scale(Vec3::new(width, 1.0, 1.0)),
                ));
                parent.spawn((
                    HealthBarFill,
                    Mesh3d(assets.health_bar_fill_mesh.clone()),
                    MeshMaterial3d(assets.health_bar_fill_mat.clone()),
                    Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::new(width, 1.0, 1.0)),
                ));
            });

        commands.entity(entity).insert(HasHealthBar);
    }
}

fn sync_health_bars(
    camera: Query<&GlobalTransform, With<GameCamera>>,
    owners: Query<(&Health, &GlobalTransform, Option<&UnitRadius>), Without<HealthBar>>,
    mut bars: Query<(&HealthBar, &mut Transform, &Children), (With<HealthBar>, Without<GameCamera>)>,
    mut fills: Query<&mut Transform, (With<HealthBarFill>, Without<HealthBar>, Without<GameCamera>)>,
) {
    let Ok(cam_gt) = camera.single() else {
        return;
    };
    let cam_pos = cam_gt.translation();

    let mut fill_updates: Vec<(Entity, f32, f32)> = Vec::new();

    for (bar, mut bar_tf, children) in &mut bars {
        let Ok((health, owner_gt, radius)) = owners.get(bar.owner) else {
            continue;
        };

        let height = bar_height(radius);
        let width = bar_width(radius);
        let owner_pos = owner_gt.translation();
        bar_tf.translation = owner_pos + Vec3::Y * height;

        let mut to_cam = cam_pos - bar_tf.translation;
        to_cam.y = 0.0;
        if to_cam.length_squared() > 0.0001 {
            if let Ok(dir) = Dir3::new(to_cam) {
                bar_tf.look_to(dir, Vec3::Y);
                bar_tf.rotate_y(std::f32::consts::PI);
            }
        }

        let fraction = if health.max <= 0.0 {
            0.0
        } else {
            (health.current / health.max).clamp(0.0, 1.0)
        };

        for child in children.iter() {
            fill_updates.push((child, width, fraction));
        }
    }

    for (child, width, fraction) in fill_updates {
        if let Ok(mut fill_tf) = fills.get_mut(child) {
            fill_tf.scale = Vec3::new(width * fraction, 1.0, 1.0);
            fill_tf.translation.x = -0.5 * width * (1.0 - fraction);
        }
    }
}

fn cull_orphan_health_bars(
    mut commands: Commands,
    bars: Query<(Entity, &HealthBar)>,
    owners: Query<Entity, With<Health>>,
) {
    for (entity, bar) in &bars {
        if owners.get(bar.owner).is_err() {
            commands.entity(entity).despawn();
        }
    }
}

fn bar_width(radius: Option<&UnitRadius>) -> f32 {
    match radius {
        Some(UnitRadius(r)) if *r > 1.2 => 2.4,
        Some(UnitRadius(r)) if *r > 0.7 => 1.6,
        _ => 1.1,
    }
}

fn bar_height(radius: Option<&UnitRadius>) -> f32 {
    match radius {
        Some(UnitRadius(r)) if *r > 1.2 => 3.4,
        Some(UnitRadius(r)) if *r > 0.7 => 3.8,
        _ => 2.2,
    }
}
