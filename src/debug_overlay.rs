//! Hollow debug overlays: bound/collision rings and selection boxes.

use bevy::prelude::*;

use crate::components::{BoundRadius, CollisionRadius, SelectionBox};
use crate::resources::SharedAssets;

pub struct DebugOverlayPlugin;

impl Plugin for DebugOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                ensure_radius_overlays,
                sync_radius_overlays,
                ensure_selection_overlays,
                sync_selection_overlays,
            )
                .chain(),
        );
    }
}

#[derive(Component, Debug, Clone, Copy)]
struct BoundRadiusRing;

#[derive(Component, Debug, Clone, Copy)]
struct CollisionRadiusRing;

#[derive(Component, Debug, Clone, Copy)]
struct SelectionBoxOutline;

#[derive(Component, Debug, Clone, Copy)]
struct HasRadiusOverlays;

#[derive(Component, Debug, Clone, Copy)]
struct HasSelectionOverlay;

fn ensure_radius_overlays(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    units: Query<
        (Entity, &BoundRadius, &CollisionRadius),
        (Without<HasRadiusOverlays>, Or<(With<BoundRadius>, With<CollisionRadius>)>),
    >,
) {
    for (entity, bound, collision) in &units {
        commands.entity(entity).insert(HasRadiusOverlays);
        commands.entity(entity).with_children(|parent| {
            parent.spawn((
                Name::new("Bound Radius Ring"),
                BoundRadiusRing,
                Mesh3d(assets.debug_ring_mesh.clone()),
                MeshMaterial3d(assets.debug_bound_mat.clone()),
                Transform::from_xyz(0.0, 0.2, 0.0)
                    .with_scale(Vec3::new(bound.0, 1.0, bound.0)),
            ));
            parent.spawn((
                Name::new("Collision Radius Ring"),
                CollisionRadiusRing,
                Mesh3d(assets.debug_ring_mesh.clone()),
                MeshMaterial3d(assets.debug_collision_mat.clone()),
                Transform::from_xyz(0.0, 0.35, 0.0)
                    .with_scale(Vec3::new(collision.0, 1.0, collision.0)),
            ));
        });
    }
}

fn sync_radius_overlays(
    bounds: Query<&BoundRadius>,
    collisions: Query<&CollisionRadius>,
    mut bound_rings: Query<(&mut Transform, &ChildOf), With<BoundRadiusRing>>,
    mut collision_rings: Query<
        (&mut Transform, &ChildOf),
        (With<CollisionRadiusRing>, Without<BoundRadiusRing>),
    >,
) {
    for (mut tf, child_of) in &mut bound_rings {
        let Ok(bound) = bounds.get(child_of.parent()) else {
            continue;
        };
        tf.scale = Vec3::new(bound.0, 1.0, bound.0);
        tf.translation.y = 0.2;
    }
    for (mut tf, child_of) in &mut collision_rings {
        let Ok(collision) = collisions.get(child_of.parent()) else {
            continue;
        };
        tf.scale = Vec3::new(collision.0, 1.0, collision.0);
        tf.translation.y = 0.35;
    }
}

fn ensure_selection_overlays(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    units: Query<(Entity, &SelectionBox), Without<HasSelectionOverlay>>,
) {
    for (entity, selection) in &units {
        let side = selection.half_extent * 2.0;
        let thickness = 2.5_f32.max(side * 0.02);
        let y = 0.5;
        commands.entity(entity).insert(HasSelectionOverlay);
        commands.entity(entity).with_children(|parent| {
            // Four thin walls forming a hollow square on the ground.
            for (name, local) in [
                (
                    "Selection N",
                    Transform::from_xyz(0.0, y, selection.half_extent)
                        .with_scale(Vec3::new(side, thickness, thickness)),
                ),
                (
                    "Selection S",
                    Transform::from_xyz(0.0, y, -selection.half_extent)
                        .with_scale(Vec3::new(side, thickness, thickness)),
                ),
                (
                    "Selection E",
                    Transform::from_xyz(selection.half_extent, y, 0.0)
                        .with_scale(Vec3::new(thickness, thickness, side)),
                ),
                (
                    "Selection W",
                    Transform::from_xyz(-selection.half_extent, y, 0.0)
                        .with_scale(Vec3::new(thickness, thickness, side)),
                ),
            ] {
                parent.spawn((
                    Name::new(name),
                    SelectionBoxOutline,
                    Mesh3d(assets.melee_slash_mesh.clone()),
                    MeshMaterial3d(assets.debug_selection_mat.clone()),
                    local,
                ));
            }
        });
    }
}

fn sync_selection_overlays(
    selections: Query<&SelectionBox>,
    children: Query<&Children>,
    mut outlines: Query<(&mut Transform, &ChildOf, &Name), With<SelectionBoxOutline>>,
) {
    for (mut tf, child_of, name) in &mut outlines {
        let Ok(selection) = selections.get(child_of.parent()) else {
            continue;
        };
        let side = selection.half_extent * 2.0;
        let thickness = 2.5_f32.max(side * 0.02);
        let y = 0.5;
        let label = name.as_str();
        *tf = if label.contains("N") {
            Transform::from_xyz(0.0, y, selection.half_extent)
                .with_scale(Vec3::new(side, thickness, thickness))
        } else if label.contains("S") {
            Transform::from_xyz(0.0, y, -selection.half_extent)
                .with_scale(Vec3::new(side, thickness, thickness))
        } else if label.contains("E") {
            Transform::from_xyz(selection.half_extent, y, 0.0)
                .with_scale(Vec3::new(thickness, thickness, side))
        } else {
            Transform::from_xyz(-selection.half_extent, y, 0.0)
                .with_scale(Vec3::new(thickness, thickness, side))
        };
        let _ = children;
    }
}
