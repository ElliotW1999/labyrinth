//! Navigation layer between orders and locomotion.
//!
//! ```text
//! order (MoveTarget) → plan_paths (static NavGrid A*, replanned only when needed)
//!   → NavPath waypoints → movement: steer at the current waypoint, skip ahead on
//!     line of sight, local avoidance around units → final step
//!   → collision resolution (safety net only)
//! ```
//!
//! Static geometry (map edge, trees, towers, ancients) lives in [`NavGrid`]. Units
//! are never baked into the grid; they are handled by local avoidance, and only a
//! stuck unit replans with nearby *stationary* units as temporary blockers.

pub mod attack_position;
pub mod avoidance;
pub mod grid;

use bevy::prelude::*;

use crate::components::{Ancient, CollisionRadius, MoveTarget, Obstacle, ObstacleShape, Tower};
use crate::dimensions::collision_of;
pub use grid::{flat, ground, DynamicBlocker, NavGrid, NavShape};

/// Requested-goal drift that triggers a full replan.
pub const GOAL_REPLAN_DIST: f32 = 48.0;
/// Minimum time between searched (A*) replans of one unit's path for goal drift.
const REPLAN_COOLDOWN: f32 = 0.25;
/// A* searches allowed per frame; extra requests wait a frame.
const SEARCH_BUDGET: usize = 16;
/// Stationary units within this distance become blockers on a stuck replan.
const BLOCKER_SCAN: f32 = 450.0;

/// Mobile units navigate; buildings and props are static geometry.
pub type MobileFilter = (Without<Tower>, Without<Ancient>, Without<Obstacle>);

/// The planned route for the unit's current [`MoveTarget`].
#[derive(Component, Debug, Clone)]
pub struct NavPath {
    /// Ground-level waypoints after the unit's position; the last one is [`Self::goal`].
    pub waypoints: Vec<Vec3>,
    /// Waypoint currently steered toward.
    pub index: usize,
    /// `MoveTarget.position` the route was planned for.
    pub requested: Vec3,
    /// Where the route actually ends (requested point resolved to walkable ground,
    /// or the closest reachable point).
    pub goal: Vec3,
    pub reached: bool,
    version: u32,
    cooldown: f32,
    replan_requested: bool,
    pub stuck: StuckTracker,
}

impl NavPath {
    pub fn current(&self) -> Option<Vec3> {
        self.waypoints.get(self.index).copied()
    }

    pub fn is_final(&self) -> bool {
        self.index + 1 >= self.waypoints.len()
    }

    /// Ask for a replan with nearby stationary units treated as blockers.
    pub fn request_replan(&mut self) {
        self.replan_requested = true;
    }

    fn from_route(route: grid::NavRoute, requested: Vec3, version: u32, stuck: StuckTracker) -> Self {
        let waypoints: Vec<Vec3> = route.waypoints.iter().map(|p| ground(*p)).collect();
        let goal = waypoints.last().copied().unwrap_or(requested);
        Self {
            waypoints,
            index: 0,
            requested,
            goal,
            reached: route.reached,
            version,
            cooldown: if route.searched { REPLAN_COOLDOWN } else { 0.0 },
            replan_requested: false,
            stuck,
        }
    }
}

/// Progress watchdog: compares distance actually covered against what the unit's
/// speed should have covered over short windows.
#[derive(Debug, Clone, Copy, Default)]
pub struct StuckTracker {
    pub timer: f32,
    pub start: Vec3,
    pub expected: f32,
    /// Consecutive windows with too little progress.
    pub count: u32,
}

/// Seconds per progress window.
pub const STUCK_WINDOW: f32 = 0.5;
/// A window is "stuck" when the unit covered less than this fraction of the expected distance.
pub const STUCK_PROGRESS: f32 = 0.25;

impl StuckTracker {
    /// Feed one frame. Returns true when a window just closed with too little progress.
    pub fn update(&mut self, pos: Vec3, dt: f32, attempted: f32) -> bool {
        if self.timer == 0.0 && self.expected == 0.0 {
            self.start = pos;
        }
        self.timer += dt;
        self.expected += attempted;
        if self.timer < STUCK_WINDOW {
            return false;
        }
        let moved = crate::dimensions::center_distance(self.start, pos);
        let stuck = self.expected > 30.0 && moved < self.expected * STUCK_PROGRESS;
        if stuck {
            self.count += 1;
        } else if moved >= self.expected * 0.5 {
            self.count = 0;
        }
        self.timer = 0.0;
        self.expected = 0.0;
        self.start = pos;
        stuck
    }
}

/// Last local-avoidance result, kept for hysteresis and the debug overlay.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct NavSteering {
    /// Lateral offset added to the desired direction this frame (ground plane).
    pub avoidance: Vec3,
    /// Committed sidestep side (+1 / −1, 0 when not avoiding).
    pub side: f32,
}

/// Rebuild the grid whenever the set of static blockers changes (spawned trees,
/// destroyed towers, ...). The shape list is tiny, so comparing it every frame is cheaper
/// than tracking every way an obstacle can appear or disappear.
pub fn sync_nav_grid(
    mut grid: ResMut<NavGrid>,
    obstacles: Query<(&Transform, &Obstacle)>,
    buildings: Query<(&Transform, &CollisionRadius), (Or<(With<Tower>, With<Ancient>)>, Without<Obstacle>)>,
) {
    let mut shapes: Vec<NavShape> = obstacles
        .iter()
        .map(|(tf, obstacle)| match obstacle.shape {
            ObstacleShape::Circle { radius } => NavShape::Circle {
                center: flat(tf.translation),
                radius,
            },
            ObstacleShape::Aabb { half_x, half_z } => NavShape::Aabb {
                center: flat(tf.translation),
                half: Vec2::new(half_x, half_z),
            },
        })
        .chain(buildings.iter().filter(|(_, r)| r.0 > 0.0).map(|(tf, r)| NavShape::Circle {
            center: flat(tf.translation),
            radius: r.0,
        }))
        .collect();
    grid::sort_shapes(&mut shapes);
    if shapes.as_slice() != grid.shapes() {
        grid.rebuild(shapes);
    }
}

/// Plan or refresh [`NavPath`]s. Replans only when the unit has no path, the nav
/// geometry changed, the requested goal drifted past [`GOAL_REPLAN_DIST`], or the
/// movement code flagged the unit as stuck.
pub fn plan_paths(
    time: Res<Time>,
    grid: Res<NavGrid>,
    mut scratch: Local<grid::SearchScratch>,
    mut movers: Query<
        (Entity, &Transform, &MoveTarget, Option<&CollisionRadius>, Option<&mut NavPath>),
        MobileFilter,
    >,
    standing: Query<(&Transform, &CollisionRadius), (Without<MoveTarget>, MobileFilter)>,
    stale: Query<Entity, (Or<(With<NavPath>, With<NavSteering>)>, Without<MoveTarget>)>,
    mut commands: Commands,
) {
    for entity in &stale {
        commands.entity(entity).remove::<(NavPath, NavSteering)>();
    }

    let dt = time.delta_secs();
    let mut searches = 0;
    let mut order: Vec<Entity> = movers.iter().map(|(e, ..)| e).collect();
    order.sort_by_key(|e| e.to_bits());

    for entity in order {
        let Ok((_, tf, target, radius, path)) = movers.get_mut(entity) else {
            continue;
        };
        let pos = flat(tf.translation);
        let r = collision_of(radius);
        let requested = Vec3::new(target.position.x, 0.0, target.position.z);
        let req2 = flat(requested);

        let mut stuck = StuckTracker::default();
        let mut with_blockers = false;
        if let Some(mut path) = path {
            path.cooldown -= dt;
            let shift = flat(path.requested).distance(req2);
            let geometry_changed = path.version != grid.version;
            if !geometry_changed && !path.replan_requested {
                if shift <= GOAL_REPLAN_DIST {
                    // Small drift on the final leg: nudge the end point in place.
                    if shift > 0.5 && path.is_final() && grid.point_clear(req2, r) {
                        let last = path.waypoints.len() - 1;
                        path.waypoints[last] = requested;
                        path.goal = requested;
                        path.requested = requested;
                    }
                    continue;
                }
                if path.cooldown > 0.0 {
                    // Cheap direct update stays responsive; searched replans wait.
                    if grid.point_clear(req2, r) && grid.segment_clear(pos, req2, r, &[]) {
                        *path = NavPath::from_route(
                            grid::NavRoute {
                                waypoints: vec![req2],
                                reached: true,
                                searched: false,
                            },
                            requested,
                            grid.version,
                            StuckTracker::default(),
                        );
                    }
                    continue;
                }
            }
            if path.replan_requested {
                with_blockers = true;
                stuck = StuckTracker {
                    count: path.stuck.count,
                    ..default()
                };
            }
        }

        if searches >= SEARCH_BUDGET {
            continue;
        }
        let blockers: Vec<DynamicBlocker> = if with_blockers {
            standing
                .iter()
                .filter(|(t, c)| c.0 > 0.0 && flat(t.translation).distance(pos) < BLOCKER_SCAN)
                .map(|(t, c)| DynamicBlocker {
                    center: flat(t.translation),
                    radius: c.0,
                })
                .collect()
        } else {
            Vec::new()
        };
        let route = grid.find_path_with(&mut scratch, pos, req2, r, &blockers);
        if route.searched {
            searches += 1;
        }
        let new_path = NavPath::from_route(route, requested, grid.version, stuck);
        match movers.get_mut(entity) {
            Ok((_, _, _, _, Some(mut path))) => *path = new_path,
            _ => {
                commands
                    .entity(entity)
                    .insert((new_path, NavSteering::default()));
            }
        }
    }
}

#[cfg(test)]
mod tests;
