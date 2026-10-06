//! Optional debug gizmos for tuning unit dimensions.
//!
//! Each overlay toggles independently:
//! - F5 — collision radius (orange ring at the unit's base)
//! - F6 — bounds radius (green ring at the unit's base)
//! - F7 — selection volume (yellow wireframe box)
//! - F8 — all three on / off
//! - F9 — navigation: blocked cells around the player hero (for its radius), every
//!   mover's path / current waypoint / requested destination / nav radius /
//!   avoidance direction, attack-position goals, and the player's queued commands
//!
//! `--debug-dims` on the command line starts with the three dimension overlays
//! enabled; `--debug-nav` starts with the navigation overlay enabled.

use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;

use crate::components::{BoundRadius, CollisionRadius, MoveTarget, PlayerHero, SelectionBounds};
use crate::dimensions::collision_of;
use crate::navigation::attack_position::AttackPositionGoal;
use crate::navigation::{flat, NavGrid, NavPath, NavSteering};
use crate::unit_commands::CommandQueue;

/// Ground height for base rings, just above the lane / ground meshes.
const RING_Y: f32 = 4.0;

const COLLISION_COLOR: Color = Color::srgb(1.0, 0.45, 0.15);
const BOUNDS_COLOR: Color = Color::srgb(0.25, 1.0, 0.45);
const SELECTION_COLOR: Color = Color::srgb(1.0, 0.9, 0.2);

const NAV_BLOCKED_COLOR: Color = Color::srgba(1.0, 0.2, 0.2, 0.55);
const NAV_PATH_COLOR: Color = Color::srgb(0.2, 0.85, 1.0);
const NAV_WAYPOINT_COLOR: Color = Color::srgb(1.0, 1.0, 0.3);
const NAV_DESTINATION_COLOR: Color = Color::srgb(1.0, 0.3, 1.0);
const NAV_RADIUS_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.6);
const NAV_AVOID_COLOR: Color = Color::srgb(1.0, 0.55, 0.0);
const NAV_ATTACK_GOAL_COLOR: Color = Color::srgb(1.0, 0.15, 0.15);
const NAV_QUEUE_COLOR: Color = Color::srgb(0.35, 1.0, 0.35);
/// Blocked cells are drawn within this distance of the player hero.
const NAV_BLOCKED_VIEW: f32 = 900.0;
/// Lines sit slightly above the dimension rings.
const NAV_Y: f32 = 8.0;

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
                draw_navigation.run_if(|s: Res<DebugOverlaySettings>| s.nav),
            )
                .chain(),
        );
    }
}

/// Which debug overlays are drawn.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DebugOverlaySettings {
    pub collision: bool,
    pub bounds: bool,
    pub selection: bool,
    pub nav: bool,
}

impl DebugOverlaySettings {
    /// Set the three unit-dimension overlays (navigation is separate).
    pub fn all(enabled: bool) -> Self {
        Self {
            collision: enabled,
            bounds: enabled,
            selection: enabled,
            nav: false,
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
        *settings = DebugOverlaySettings {
            nav: settings.nav,
            ..DebugOverlaySettings::all(!any)
        };
    }
    if keys.just_pressed(KeyCode::F9) {
        settings.nav = !settings.nav;
    }
}

fn at_nav_height(p: Vec3) -> Vec3 {
    Vec3::new(p.x, NAV_Y, p.z)
}

fn cross(gizmos: &mut Gizmos, p: Vec3, size: f32, color: Color) {
    let c = at_nav_height(p);
    gizmos.line(c + Vec3::new(-size, 0.0, -size), c + Vec3::new(size, 0.0, size), color);
    gizmos.line(c + Vec3::new(-size, 0.0, size), c + Vec3::new(size, 0.0, -size), color);
}

type NavDebugUnits<'w, 's> = Query<
    'w,
    's,
    (
        &'static GlobalTransform,
        Option<&'static CollisionRadius>,
        Option<&'static MoveTarget>,
        Option<&'static NavPath>,
        Option<&'static NavSteering>,
        Option<&'static AttackPositionGoal>,
        Option<&'static Visibility>,
    ),
>;

fn draw_navigation(
    grid: Res<NavGrid>,
    units: NavDebugUnits,
    hero: Query<(&GlobalTransform, Option<&CollisionRadius>, Option<&CommandQueue>), With<PlayerHero>>,
    mut gizmos: Gizmos,
) {
    // Non-navigable cells (for the hero's radius) near the hero; everything else is walkable.
    if let Ok((gt, radius, queue)) = hero.single() {
        let half = grid.cell_size() * 0.5 - 2.0;
        for c in grid.blocked_cells_near(flat(gt.translation()), NAV_BLOCKED_VIEW, collision_of(radius)) {
            gizmos.rect(
                Isometry3d::new(Vec3::new(c.x, NAV_Y, c.y), Quat::from_rotation_x(FRAC_PI_2)),
                Vec2::splat(half * 2.0),
                NAV_BLOCKED_COLOR,
            );
        }
        // Queued command chain: current destination, then each queued one in order.
        if let Some(queue) = queue {
            let mut from = at_nav_height(gt.translation());
            for dest in queue.iter().filter_map(|c| c.destination()) {
                let to = at_nav_height(dest);
                gizmos.line(from, to, NAV_QUEUE_COLOR);
                gizmos.circle(base_ring(dest), 24.0, NAV_QUEUE_COLOR);
                from = to;
            }
        }
    }

    for (gt, radius, move_target, path, steering, attack_goal, vis) in &units {
        if !shown(vis) {
            continue;
        }
        let pos = gt.translation();
        if let Some(path) = path {
            gizmos.circle(base_ring(pos), collision_of(radius), NAV_RADIUS_COLOR);
            let mut from = at_nav_height(pos);
            for wp in &path.waypoints[path.index.min(path.waypoints.len())..] {
                let to = at_nav_height(*wp);
                gizmos.line(from, to, NAV_PATH_COLOR);
                from = to;
            }
            if let Some(wp) = path.current() {
                gizmos.sphere(Isometry3d::from_translation(at_nav_height(wp)), 14.0, NAV_WAYPOINT_COLOR);
            }
            if !path.reached {
                cross(&mut gizmos, path.goal, 30.0, NAV_ATTACK_GOAL_COLOR);
            }
        }
        if let Some(target) = move_target {
            cross(&mut gizmos, target.position, 18.0, NAV_DESTINATION_COLOR);
        }
        if let Some(steering) = steering.filter(|s| s.side != 0.0) {
            let start = at_nav_height(pos);
            gizmos.arrow(start, start + steering.avoidance * 90.0, NAV_AVOID_COLOR);
        }
        if let Some(goal) = attack_goal {
            gizmos.circle(base_ring(goal.point), 20.0, NAV_ATTACK_GOAL_COLOR);
            gizmos.line(at_nav_height(goal.anchor), at_nav_height(goal.point), NAV_ATTACK_GOAL_COLOR);
        }
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
