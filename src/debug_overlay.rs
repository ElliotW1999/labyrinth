//! Optional debug gizmos for tuning unit dimensions.
//!
//! Each overlay toggles independently:
//! - F5 — collision radius (orange ring at the unit's base)
//! - F6 — bounds radius (green ring at the unit's base)
//! - F7 — selection volume (yellow wireframe box)
//! - F8 — all three on / off
//!
//! `--debug-dims` on the command line starts with all three enabled.

use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;

use crate::components::{BoundRadius, CollisionRadius, SelectionBounds};

/// Ground height for base rings, just above the lane / ground meshes.
const RING_Y: f32 = 4.0;

const COLLISION_COLOR: Color = Color::srgb(1.0, 0.45, 0.15);
const BOUNDS_COLOR: Color = Color::srgb(0.25, 1.0, 0.45);
const SELECTION_COLOR: Color = Color::srgb(1.0, 0.9, 0.2);

pub struct DebugOverlayPlugin;

impl Plugin for DebugOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugOverlaySettings>().add_systems(
            Update,
            (
                toggle_debug_overlays,
                draw_collision_radii.run_if(|s: Res<DebugOverlaySettings>| s.collision),
                draw_bounds_radii.run_if(|s: Res<DebugOverlaySettings>| s.bounds),
                draw_selection_bounds.run_if(|s: Res<DebugOverlaySettings>| s.selection),
            )
                .chain(),
        );
    }
}

/// Which unit-dimension overlays are drawn.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DebugOverlaySettings {
    pub collision: bool,
    pub bounds: bool,
    pub selection: bool,
}

impl DebugOverlaySettings {
    pub fn all(enabled: bool) -> Self {
        Self {
            collision: enabled,
            bounds: enabled,
            selection: enabled,
        }
    }
}

fn toggle_debug_overlays(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<DebugOverlaySettings>) {
    if keys.just_pressed(KeyCode::F5) {
        settings.collision = !settings.collision;
    }
    if keys.just_pressed(KeyCode::F6) {
        settings.bounds = !settings.bounds;
    }
    if keys.just_pressed(KeyCode::F7) {
        settings.selection = !settings.selection;
    }
    if keys.just_pressed(KeyCode::F8) {
        let any = settings.collision || settings.bounds || settings.selection;
        *settings = DebugOverlaySettings::all(!any);
    }
}

fn base_ring(pos: Vec3) -> Isometry3d {
    Isometry3d::new(Vec3::new(pos.x, RING_Y, pos.z), Quat::from_rotation_x(FRAC_PI_2))
}

fn shown(visibility: Option<&Visibility>) -> bool {
    !matches!(visibility, Some(Visibility::Hidden))
}

fn draw_collision_radii(
    units: Query<(&GlobalTransform, &CollisionRadius, Option<&Visibility>)>,
    mut gizmos: Gizmos,
) {
    for (gt, collision, vis) in &units {
        if shown(vis) && collision.0 > 0.0 {
            gizmos.circle(base_ring(gt.translation()), collision.0, COLLISION_COLOR);
        }
    }
}

fn draw_bounds_radii(
    units: Query<(&GlobalTransform, &BoundRadius, Option<&Visibility>)>,
    mut gizmos: Gizmos,
) {
    for (gt, bounds, vis) in &units {
        if shown(vis) && bounds.0 > 0.0 {
            gizmos.circle(base_ring(gt.translation()), bounds.0, BOUNDS_COLOR);
        }
    }
}

fn draw_selection_bounds(
    units: Query<(&GlobalTransform, &SelectionBounds, Option<&Visibility>)>,
    mut gizmos: Gizmos,
) {
    for (gt, selection, vis) in &units {
        if !shown(vis) {
            continue;
        }
        let center = gt.translation() + selection.offset;
        gizmos.cube(
            Transform::from_translation(center).with_scale(selection.half_extents * 2.0),
            SELECTION_COLOR,
        );
    }
}
