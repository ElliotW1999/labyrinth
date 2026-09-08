//! Billboard health bars floating above living units.

use bevy::prelude::*;

use crate::components::{HasHealthBar, Health, HealthBar, HealthBarFill, UnitRadius};
use crate::resources::SharedAssets;
use crate::scale;

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
                // Fixed world orientation (no camera billboard).
                Transform::default(),
                Visibility::default(),
            ))
            .with_children(|parent| {
                // Mesh is 1×1 unit; scale X to the desired world width.
                parent.spawn((
                    Mesh3d(assets.health_bar_bg_mesh.clone()),
                    MeshMaterial3d(assets.health_bar_bg_mat.clone()),
                    Transform::from_xyz(0.0, 0.0, -0.02).with_scale(Vec3::new(width, 1.0, 1.0)),
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
    owners: Query<(&Health, &GlobalTransform, Option<&UnitRadius>), Without<HealthBar>>,
    mut bars: Query<(&HealthBar, &mut Transform, &Children), With<HealthBar>>,
    mut fills: Query<&mut Transform, (With<HealthBarFill>, Without<HealthBar>)>,
    mut backgrounds: Query<
        &mut Transform,
        (Without<HealthBarFill>, Without<HealthBar>, With<Mesh3d>),
    >,
) {
    let mut fill_updates: Vec<(Entity, f32, f32)> = Vec::new();
    let mut bg_updates: Vec<(Entity, f32)> = Vec::new();

    for (bar, mut bar_tf, children) in &mut bars {
        let Ok((health, owner_gt, radius)) = owners.get(bar.owner) else {
            continue;
        };

        let height = bar_height(radius);
        let width = bar_width(radius);
        let owner_pos = owner_gt.translation();
        bar_tf.translation = owner_pos + Vec3::Y * height;
        // Keep axis-aligned — do not rotate with the camera.
        bar_tf.rotation = Quat::IDENTITY;

        let fraction = if health.max <= 0.0 {
            0.0
        } else {
            (health.current / health.max).clamp(0.0, 1.0)
        };

        for (i, child) in children.iter().enumerate() {
            if i == 0 {
                bg_updates.push((child, width));
            } else {
                fill_updates.push((child, width, fraction));
            }
        }
    }

    for (child, width) in bg_updates {
        if let Ok(mut bg_tf) = backgrounds.get_mut(child) {
            bg_tf.scale = Vec3::new(width, 1.0, 1.0);
            bg_tf.translation.x = 0.0;
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

/// Bar width ≈ unit diameter so it sits over the body without dwarfing it.
fn bar_width(radius: Option<&UnitRadius>) -> f32 {
    let r = radius.map(|u| u.0).unwrap_or(scale::u(0.5));
    (r * 2.2).clamp(scale::u(0.8), scale::u(4.0))
}

fn bar_height(radius: Option<&UnitRadius>) -> f32 {
    let r = radius.map(|u| u.0).unwrap_or(scale::u(0.5));
    r * 2.5 + scale::u(0.8)
}
