//! Health bars floating above living units, plus screen-space hero nameplates.

use bevy::prelude::*;

use crate::camera::GameCamera;
use crate::components::{
    HasHealthBar, Health, HealthBar, HealthBarFill, SelectionBox, WorldHeroNameLabel,
    WorldNameLayer,
};
use crate::heroes::HeroKind;
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
                sync_world_hero_names,
                cull_orphan_health_bars,
                cull_orphan_world_names,
            )
                .chain(),
        );
    }
}

fn attach_health_bars(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    units: Query<
        (Entity, Option<&SelectionBox>, Option<&HeroKind>),
        (With<Health>, Without<HasHealthBar>),
    >,
    layer: Query<Entity, With<WorldNameLayer>>,
) {
    let layer_entity = layer.single().ok();

    for (entity, selection, hero_kind) in &units {
        let width = bar_width(selection);

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

        if let (Some(kind), Some(layer_e)) = (hero_kind, layer_entity) {
            commands.entity(layer_e).with_children(|parent| {
                parent.spawn((
                    Name::new(format!("WorldName:{}", kind.0.name())),
                    WorldHeroNameLabel { owner: entity },
                    Text::new(kind.0.name()),
                    TextFont::from_font_size(14.0),
                    TextColor(Color::srgb(0.95, 0.96, 1.0)),
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(-100.0),
                        top: px(-100.0),
                        padding: UiRect::axes(px(6), px(2)),
                        border_radius: BorderRadius::all(px(3)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.05, 0.07, 0.1, 0.72)),
                    Visibility::Hidden,
                    ZIndex(5),
                ));
            });
        }

        commands.entity(entity).insert(HasHealthBar);
    }
}

fn sync_health_bars(
    owners: Query<(&Health, &GlobalTransform, Option<&SelectionBox>), Without<HealthBar>>,
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
        let Ok((health, owner_gt, selection)) = owners.get(bar.owner) else {
            continue;
        };

        let height = bar_height(selection);
        let width = bar_width(selection);
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

fn sync_world_hero_names(
    camera: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    owners: Query<
        (&GlobalTransform, &HeroKind, Option<&SelectionBox>, &Visibility),
        Without<WorldHeroNameLabel>,
    >,
    mut labels: Query<(&WorldHeroNameLabel, &mut Node, &mut Visibility, &mut Text)>,
) {
    let Ok((camera, cam_gt)) = camera.single() else {
        return;
    };

    for (label, mut node, mut vis, mut text) in &mut labels {
        let Ok((owner_gt, kind, selection, owner_vis)) = owners.get(label.owner) else {
            *vis = Visibility::Hidden;
            continue;
        };
        if matches!(*owner_vis, Visibility::Hidden) {
            *vis = Visibility::Hidden;
            continue;
        }

        let world_pos =
            owner_gt.translation() + Vec3::Y * (bar_height(selection) + scale::HEALTH_BAR_BG_THICKNESS);
        let Ok(screen) = camera.world_to_viewport(cam_gt, world_pos) else {
            *vis = Visibility::Hidden;
            continue;
        };

        *text = Text::new(kind.0.name());
        // Approximate half-width so the label sits centered above the bar.
        node.left = px(screen.x - 36.0);
        node.top = px(screen.y - 16.0);
        *vis = Visibility::Visible;
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

fn cull_orphan_world_names(
    mut commands: Commands,
    labels: Query<(Entity, &WorldHeroNameLabel)>,
    owners: Query<Entity, With<HeroKind>>,
) {
    for (entity, label) in &labels {
        if owners.get(label.owner).is_err() {
            commands.entity(entity).despawn();
        }
    }
}

/// Bar width tracks the unit's selection footprint (≈ model width).
fn bar_width(selection: Option<&SelectionBox>) -> f32 {
    let half = selection
        .map(|s| s.half_extent)
        .unwrap_or(scale::HERO_MODEL_WIDTH * 0.5);
    (half * 2.2).clamp(scale::CREEP_MODEL_WIDTH * 0.8, scale::TOWER_MODEL_WIDTH * 1.4)
}

/// Offset from unit center to just above the model top.
/// Models are 2× as tall as they are wide, so half-height ≈ selection half_extent.
fn bar_height(selection: Option<&SelectionBox>) -> f32 {
    let half = selection
        .map(|s| s.half_extent)
        .unwrap_or(scale::HERO_MODEL_WIDTH * 0.5);
    half * 2.0 + scale::HEALTH_BAR_BG_THICKNESS * 0.75
}


#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::IntoSystem;

    #[test]
    fn sync_health_bars_system_initializes() {
        let mut world = World::new();
        let mut system = IntoSystem::into_system(sync_health_bars);
        system.initialize(&mut world);
    }

    #[test]
    fn sync_world_hero_names_system_initializes() {
        let mut world = World::new();
        let mut system = IntoSystem::into_system(sync_world_hero_names);
        system.initialize(&mut world);
    }

    #[test]
    fn attach_health_bars_system_initializes() {
        let mut world = World::new();
        world.init_resource::<SharedAssets>();
        let mut system = IntoSystem::into_system(attach_health_bars);
        system.initialize(&mut world);
    }
}
