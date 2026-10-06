//! Static navigation grid: a clearance field over the map plus radius-aware A*.
//!
//! Each cell stores the distance from its center to the nearest static blocker
//! (map edge, tree, tower, ancient). A cell is walkable for a unit of radius `r`
//! when that clearance is at least `r + NAV_MARGIN`, so one grid serves every
//! unit size. Paths are string-pulled with exact segment-vs-shape tests, so the
//! final waypoints keep a full collision radius away from obstacle edges.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use bevy::prelude::*;

/// Grid cell size in world units. Half a tree width, so a gap that physically fits
/// a unit always has a row of cell centers close enough to its middle.
pub const NAV_CELL: f32 = 32.0;
/// Extra clearance required on top of the unit radius for a cell to be walkable.
/// Covers the gap between cell-center samples so raw grid edges don't clip corners.
pub const NAV_MARGIN: f32 = 6.0;
/// Clearance values are capped here; no unit plans with a larger radius.
const MAX_CLEARANCE: f32 = 256.0;
/// A* gives up (returning the closest node found) after this many expansions.
pub const MAX_EXPANSIONS: usize = 60_000;
/// How far (in cells) string-pulling looks ahead from each anchor.
const SMOOTH_WINDOW: usize = 96;
/// How far (in cells) [`NavGrid::nearest_walkable`] searches.
const NEAREST_SEARCH_CELLS: i32 = 40;

const SQRT2: f32 = std::f32::consts::SQRT_2;

/// A static blocker on the ground plane (XZ stored as `Vec2(x, z)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavShape {
    Circle { center: Vec2, radius: f32 },
    /// Collides like the movement code: the box is expanded by the unit radius on both axes.
    Aabb { center: Vec2, half: Vec2 },
}

impl NavShape {
    /// Clearance from `p` to the shape surface, in the metric collision uses
    /// (Euclidean for circles, Chebyshev for boxes). Negative inside.
    pub fn distance(&self, p: Vec2) -> f32 {
        match *self {
            NavShape::Circle { center, radius } => p.distance(center) - radius,
            NavShape::Aabb { center, half } => {
                let d = (p - center).abs() - half;
                d.x.max(d.y)
            }
        }
    }

    fn reach(&self) -> Vec2 {
        match *self {
            NavShape::Circle { radius, .. } => Vec2::splat(radius),
            NavShape::Aabb { half, .. } => half,
        }
    }

    fn center(&self) -> Vec2 {
        match *self {
            NavShape::Circle { center, .. } | NavShape::Aabb { center, .. } => center,
        }
    }

    /// True when a circle of radius `r` sweeping from `a` to `b` stays strictly outside.
    pub fn segment_clear(&self, a: Vec2, b: Vec2, r: f32) -> bool {
        match *self {
            NavShape::Circle { center, radius } => {
                segment_point_distance(a, b, center) >= radius + r
            }
            NavShape::Aabb { center, half } => {
                !segment_hits_box(a, b, center - half - Vec2::splat(r), center + half + Vec2::splat(r))
            }
        }
    }
}

/// Cheap total ordering so the shape list can be compared frame to frame.
pub fn sort_shapes(shapes: &mut [NavShape]) {
    shapes.sort_by(|a, b| {
        let (ca, cb) = (a.center(), b.center());
        ca.x.total_cmp(&cb.x)
            .then(ca.y.total_cmp(&cb.y))
            .then(a.reach().x.total_cmp(&b.reach().x))
    });
}

fn segment_point_distance(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-8 {
        return a.distance(p);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// Slab test of segment `a→b` against the closed box `[min, max]`.
fn segment_hits_box(a: Vec2, b: Vec2, min: Vec2, max: Vec2) -> bool {
    let d = b - a;
    let mut t0 = 0.0_f32;
    let mut t1 = 1.0_f32;
    for axis in 0..2 {
        let (o, dir, lo, hi) = (a[axis], d[axis], min[axis], max[axis]);
        if dir.abs() < 1e-8 {
            if o < lo || o > hi {
                return false;
            }
            continue;
        }
        let inv = 1.0 / dir;
        let (mut near, mut far) = ((lo - o) * inv, (hi - o) * inv);
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// Ground-plane projection used everywhere in navigation.
#[inline]
pub fn flat(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

#[inline]
pub fn ground(p: Vec2) -> Vec3 {
    Vec3::new(p.x, 0.0, p.y)
}

/// A temporary circular blocker (e.g. a stationary unit) added to one search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicBlocker {
    pub center: Vec2,
    pub radius: f32,
}

/// Result of a path query.
#[derive(Debug, Clone, PartialEq)]
pub struct NavRoute {
    /// Waypoints after the start position, ending at the (resolved) goal.
    pub waypoints: Vec<Vec2>,
    /// False when the goal was unreachable and the route ends at the closest point found.
    pub reached: bool,
    /// True when A* ran (as opposed to a direct line-of-sight route).
    pub searched: bool,
}

impl NavRoute {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn end(&self) -> Option<Vec2> {
        self.waypoints.last().copied()
    }
}

#[derive(Resource, Debug, Clone)]
pub struct NavGrid {
    half_extent: f32,
    cell: f32,
    width: usize,
    clearance: Vec<f32>,
    shapes: Vec<NavShape>,
    /// Bumped on every rebuild; paths planned against an older version replan.
    pub version: u32,
}

impl Default for NavGrid {
    fn default() -> Self {
        Self::new(crate::scale::MAP_HALF, NAV_CELL)
    }
}

impl NavGrid {
    pub fn new(half_extent: f32, cell: f32) -> Self {
        let width = ((half_extent * 2.0) / cell).ceil().max(1.0) as usize;
        let mut grid = Self {
            half_extent,
            cell,
            width,
            clearance: Vec::new(),
            shapes: Vec::new(),
            version: 0,
        };
        grid.rebuild(Vec::new());
        grid
    }

    pub fn shapes(&self) -> &[NavShape] {
        &self.shapes
    }

    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    /// Recompute the clearance field. `shapes` should already be sorted with [`sort_shapes`].
    pub fn rebuild(&mut self, shapes: Vec<NavShape>) {
        let n = self.width * self.width;
        self.clearance = Vec::with_capacity(n);
        for idx in 0..n {
            let c = self.center_of(idx);
            let border = self.half_extent - c.x.abs().max(c.y.abs());
            self.clearance.push(border.min(MAX_CLEARANCE));
        }
        for shape in &shapes {
            let reach = shape.reach() + Vec2::splat(MAX_CLEARANCE);
            let center = shape.center();
            let (x0, z0) = self.clamp_cell(center - reach);
            let (x1, z1) = self.clamp_cell(center + reach);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let idx = z * self.width + x;
                    let d = shape.distance(self.center_of(idx));
                    if d < self.clearance[idx] {
                        self.clearance[idx] = d;
                    }
                }
            }
        }
        self.shapes = shapes;
        self.version = self.version.wrapping_add(1);
    }

    fn clamp_cell(&self, p: Vec2) -> (usize, usize) {
        let max = self.width as i32 - 1;
        let x = (((p.x + self.half_extent) / self.cell).floor() as i32).clamp(0, max);
        let z = (((p.y + self.half_extent) / self.cell).floor() as i32).clamp(0, max);
        (x as usize, z as usize)
    }

    fn cell_xz(&self, p: Vec2) -> Option<(i32, i32)> {
        let x = ((p.x + self.half_extent) / self.cell).floor() as i32;
        let z = ((p.y + self.half_extent) / self.cell).floor() as i32;
        let w = self.width as i32;
        (x >= 0 && z >= 0 && x < w && z < w).then_some((x, z))
    }

    fn index(&self, x: i32, z: i32) -> Option<usize> {
        let w = self.width as i32;
        (x >= 0 && z >= 0 && x < w && z < w).then(|| (z * w + x) as usize)
    }

    fn center_of(&self, idx: usize) -> Vec2 {
        let x = (idx % self.width) as f32;
        let z = (idx / self.width) as f32;
        Vec2::new(
            -self.half_extent + (x + 0.5) * self.cell,
            -self.half_extent + (z + 0.5) * self.cell,
        )
    }

    fn cell_walkable(&self, idx: usize, r: f32, blockers: &[DynamicBlocker]) -> bool {
        if self.clearance[idx] < r + NAV_MARGIN {
            return false;
        }
        if blockers.is_empty() {
            return true;
        }
        let c = self.center_of(idx);
        blockers
            .iter()
            .all(|b| c.distance(b.center) >= b.radius + r)
    }

    /// Exact test: a circle of radius `r` at `p` overlaps no static blocker and is inside the map.
    pub fn point_clear(&self, p: Vec2, r: f32) -> bool {
        if p.x.abs().max(p.y.abs()) > self.half_extent - r {
            return false;
        }
        self.shapes.iter().all(|s| s.distance(p) > r)
    }

    /// Exact sweep test of a circle of radius `r` from `a` to `b` against static
    /// shapes, the map edge, and optional dynamic blockers.
    pub fn segment_clear(&self, a: Vec2, b: Vec2, r: f32, blockers: &[DynamicBlocker]) -> bool {
        let limit = self.half_extent - r;
        if a.x.abs().max(a.y.abs()) > limit || b.x.abs().max(b.y.abs()) > limit {
            return false;
        }
        let lo = a.min(b) - Vec2::splat(r);
        let hi = a.max(b) + Vec2::splat(r);
        let static_clear = self.shapes.iter().all(|s| {
            let (c, reach) = (s.center(), s.reach());
            let disjoint = c.x + reach.x < lo.x
                || c.x - reach.x > hi.x
                || c.y + reach.y < lo.y
                || c.y - reach.y > hi.y;
            disjoint || s.segment_clear(a, b, r)
        });
        static_clear
            && blockers
                .iter()
                .all(|bl| segment_point_distance(a, b, bl.center) >= bl.radius + r)
    }

    /// `p` itself when a unit of radius `r` fits there, otherwise the closest point
    /// that does (searching outward up to ~1500 units). `None` if nothing fits.
    pub fn nearest_walkable(&self, p: Vec2, r: f32) -> Option<Vec2> {
        if self.point_clear(p, r) {
            return Some(p);
        }
        let (cx, cz) = self.cell_xz(p).unwrap_or_else(|| {
            let (x, z) = self.clamp_cell(p);
            (x as i32, z as i32)
        });
        let mut best: Option<(f32, usize)> = None;
        for ring in 0..=NEAREST_SEARCH_CELLS {
            for (x, z) in ring_cells(cx, cz, ring) {
                let Some(idx) = self.index(x, z) else { continue };
                if !self.cell_walkable(idx, r, &[]) {
                    continue;
                }
                let d = self.center_of(idx).distance(p);
                if best.is_none_or(|(bd, bi)| d < bd || (d == bd && idx < bi)) {
                    best = Some((d, idx));
                }
            }
            // A cell in the next ring can still be closer in Euclidean terms; one
            // extra ring past the first hit is enough at this cell size.
            if let Some((d, _)) = best {
                if d <= (ring as f32) * self.cell {
                    break;
                }
            }
        }
        let (_, idx) = best?;
        // Slide from the walkable cell center toward the request until just before contact.
        let mut lo = self.center_of(idx);
        let mut hi = p;
        for _ in 0..10 {
            let mid = (lo + hi) * 0.5;
            if self.point_clear(mid, r + 1.0) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(lo)
    }

    /// Plan a radius-aware route from `start` to `goal`.
    ///
    /// The goal is first resolved to the nearest walkable point. Direct line of
    /// sight skips the search entirely; otherwise 8-connected A* (no corner
    /// cutting) runs over cells with enough clearance and the result is
    /// string-pulled with exact sweep tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn find_path(&self, start: Vec2, goal: Vec2, r: f32, blockers: &[DynamicBlocker]) -> NavRoute {
        self.find_path_with(&mut SearchScratch::default(), start, goal, r, blockers)
    }

    /// [`Self::find_path`] reusing `scratch` buffers between searches.
    pub fn find_path_with(
        &self,
        scratch: &mut SearchScratch,
        start: Vec2,
        goal: Vec2,
        r: f32,
        blockers: &[DynamicBlocker],
    ) -> NavRoute {
        let goal = self.nearest_walkable(goal, r).unwrap_or(goal);
        // A blocker sitting on the goal makes it unreachable by definition; ignore it
        // so the route still leads up to it.
        let blockers: Vec<DynamicBlocker> = blockers
            .iter()
            .copied()
            .filter(|b| goal.distance(b.center) >= b.radius + r)
            .collect();
        if self.segment_clear(start, goal, r, &blockers) {
            return NavRoute {
                waypoints: vec![goal],
                reached: true,
                searched: false,
            };
        }

        let (Some(start_idx), Some(goal_idx)) = (
            self.nearest_walkable_cell(start, r, &blockers),
            self.nearest_walkable_cell(goal, r, &[]),
        ) else {
            return NavRoute {
                waypoints: vec![goal],
                reached: false,
                searched: true,
            };
        };

        let (cells, reached) = self.astar(scratch, start_idx, goal_idx, r, &blockers);
        let mut points: Vec<Vec2> = Vec::with_capacity(cells.len() + 2);
        points.push(start);
        points.extend(cells.iter().skip(1).map(|&idx| self.center_of(idx)));
        if reached {
            if points.len() > 1 {
                points.pop();
            }
            points.push(goal);
        } else if points.len() == 1 {
            points.push(self.center_of(start_idx));
        }
        let waypoints = self.smooth(&points, r, &blockers);
        NavRoute {
            waypoints,
            reached,
            searched: true,
        }
    }

    fn nearest_walkable_cell(&self, p: Vec2, r: f32, blockers: &[DynamicBlocker]) -> Option<usize> {
        let (cx, cz) = self.cell_xz(p)?;
        for ring in 0..=NEAREST_SEARCH_CELLS {
            let best = ring_cells(cx, cz, ring)
                .filter_map(|(x, z)| self.index(x, z))
                .filter(|&idx| self.cell_walkable(idx, r, blockers))
                .min_by(|&a, &b| {
                    self.center_of(a)
                        .distance_squared(p)
                        .total_cmp(&self.center_of(b).distance_squared(p))
                        .then(a.cmp(&b))
                });
            if best.is_some() {
                return best;
            }
        }
        None
    }

    fn astar(
        &self,
        scratch: &mut SearchScratch,
        start: usize,
        goal: usize,
        r: f32,
        blockers: &[DynamicBlocker],
    ) -> (Vec<usize>, bool) {
        let w = self.width as i32;
        scratch.begin(self.clearance.len());
        let goal_xz = ((goal % self.width) as i32, (goal / self.width) as i32);
        let heuristic = |idx: usize| {
            let dx = ((idx % self.width) as i32 - goal_xz.0).abs() as f32;
            let dz = ((idx / self.width) as i32 - goal_xz.1).abs() as f32;
            dx.max(dz) + (SQRT2 - 1.0) * dx.min(dz)
        };

        scratch.relax(start, 0.0, u32::MAX);
        scratch.open.push(OpenNode {
            f: heuristic(start),
            idx: start,
        });
        let mut best = (heuristic(start), start);
        let mut expansions = 0;
        let mut reached = false;

        while let Some(OpenNode { idx, .. }) = scratch.open.pop() {
            if scratch.is_closed(idx) {
                continue;
            }
            scratch.close(idx);
            let h = heuristic(idx);
            if h < best.0 {
                best = (h, idx);
            }
            if idx == goal {
                reached = true;
                break;
            }
            expansions += 1;
            if expansions > MAX_EXPANSIONS {
                break;
            }
            let (x, z) = ((idx % self.width) as i32, (idx / self.width) as i32);
            for (dx, dz) in NEIGHBORS {
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || nx >= w || nz >= w {
                    continue;
                }
                let nidx = (nz * w + nx) as usize;
                if scratch.is_closed(nidx) || !self.cell_walkable(nidx, r, blockers) {
                    continue;
                }
                let diagonal = dx != 0 && dz != 0;
                if diagonal {
                    let side_a = (z * w + nx) as usize;
                    let side_b = (nz * w + x) as usize;
                    if !self.cell_walkable(side_a, r, blockers) || !self.cell_walkable(side_b, r, blockers) {
                        continue;
                    }
                }
                let cost = scratch.g(idx) + if diagonal { SQRT2 } else { 1.0 };
                if cost < scratch.g(nidx) {
                    scratch.relax(nidx, cost, idx as u32);
                    scratch.open.push(OpenNode {
                        f: cost + heuristic(nidx),
                        idx: nidx,
                    });
                }
            }
        }

        let end = if reached { goal } else { best.1 };
        let mut cells = vec![end];
        let mut cur = end;
        while cur != start {
            let p = scratch.parent(cur);
            if p == u32::MAX {
                break;
            }
            cur = p as usize;
            cells.push(cur);
        }
        cells.reverse();
        (cells, reached)
    }

    /// Greedy string-pulling: from each anchor jump to the farthest point it can see.
    fn smooth(&self, points: &[Vec2], r: f32, blockers: &[DynamicBlocker]) -> Vec<Vec2> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < points.len() {
            let far = (i + SMOOTH_WINDOW).min(points.len() - 1);
            let mut next = i + 1;
            for k in (i + 2..=far).rev() {
                if self.segment_clear(points[i], points[k], r, blockers) {
                    next = k;
                    break;
                }
            }
            out.push(points[next]);
            i = next;
        }
        out
    }

    /// Centers of cells that are not walkable for radius `r` near `center` (debug drawing).
    pub fn blocked_cells_near(&self, center: Vec2, radius: f32, r: f32) -> Vec<Vec2> {
        let reach = Vec2::splat(radius);
        let (x0, z0) = self.clamp_cell(center - reach);
        let (x1, z1) = self.clamp_cell(center + reach);
        let mut out = Vec::new();
        for z in z0..=z1 {
            for x in x0..=x1 {
                let idx = z * self.width + x;
                if self.clearance[idx] < r + NAV_MARGIN {
                    out.push(self.center_of(idx));
                }
            }
        }
        out
    }
}

const NEIGHBORS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Cells on the square ring at Chebyshev distance `ring` around `(cx, cz)`.
fn ring_cells(cx: i32, cz: i32, ring: i32) -> impl Iterator<Item = (i32, i32)> {
    (-ring..=ring).flat_map(move |dz| {
        (-ring..=ring).filter_map(move |dx| {
            (dx.abs() == ring || dz.abs() == ring).then_some((cx + dx, cz + dz))
        })
    })
}

/// Reusable A* buffers. Entries are tagged with a search stamp, so starting a new
/// search is O(1) instead of clearing a value per grid cell.
#[derive(Debug, Default)]
pub struct SearchScratch {
    g: Vec<f32>,
    parent: Vec<u32>,
    seen: Vec<u32>,
    closed: Vec<u32>,
    stamp: u32,
    open: BinaryHeap<OpenNode>,
}

impl SearchScratch {
    fn begin(&mut self, cells: usize) {
        if self.seen.len() != cells {
            self.g = vec![0.0; cells];
            self.parent = vec![u32::MAX; cells];
            self.seen = vec![0; cells];
            self.closed = vec![0; cells];
            self.stamp = 0;
        }
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.seen.fill(0);
            self.closed.fill(0);
            self.stamp = 1;
        }
        self.open.clear();
    }

    fn g(&self, idx: usize) -> f32 {
        if self.seen[idx] == self.stamp { self.g[idx] } else { f32::INFINITY }
    }

    fn parent(&self, idx: usize) -> u32 {
        if self.seen[idx] == self.stamp { self.parent[idx] } else { u32::MAX }
    }

    fn relax(&mut self, idx: usize, g: f32, parent: u32) {
        self.seen[idx] = self.stamp;
        self.g[idx] = g;
        self.parent[idx] = parent;
    }

    fn is_closed(&self, idx: usize) -> bool {
        self.closed[idx] == self.stamp
    }

    fn close(&mut self, idx: usize) {
        self.closed[idx] = self.stamp;
    }
}

#[derive(Debug, Clone, Copy)]
struct OpenNode {
    f: f32,
    idx: usize,
}

impl PartialEq for OpenNode {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for OpenNode {}

impl PartialOrd for OpenNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OpenNode {
    /// Min-heap on `f`, ties broken by index so searches are deterministic.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f
            .total_cmp(&self.f)
            .then_with(|| other.idx.cmp(&self.idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HERO_R: f32 = 40.0;

    fn grid_with(shapes: Vec<NavShape>) -> NavGrid {
        let mut grid = NavGrid::new(3000.0, NAV_CELL);
        let mut shapes = shapes;
        sort_shapes(&mut shapes);
        grid.rebuild(shapes);
        grid
    }

    fn tower(x: f32, z: f32) -> NavShape {
        NavShape::Circle {
            center: Vec2::new(x, z),
            radius: 100.0,
        }
    }

    fn tree(x: f32, z: f32) -> NavShape {
        NavShape::Aabb {
            center: Vec2::new(x, z),
            half: Vec2::splat(64.0),
        }
    }

    /// Every segment of the route keeps the unit's full radius off every shape.
    fn assert_route_clear(grid: &NavGrid, start: Vec2, route: &NavRoute, r: f32) {
        let mut from = start;
        for &wp in &route.waypoints {
            assert!(
                grid.segment_clear(from, wp, r - 0.5, &[]),
                "segment {from:?} → {wp:?} clips an obstacle"
            );
            from = wp;
        }
    }

    #[test]
    fn open_ground_is_a_direct_route() {
        let grid = grid_with(vec![]);
        let route = grid.find_path(Vec2::ZERO, Vec2::new(1000.0, 400.0), HERO_R, &[]);
        assert_eq!(route.waypoints, vec![Vec2::new(1000.0, 400.0)]);
        assert!(route.reached && !route.searched);
    }

    #[test]
    fn hero_routes_around_a_tower_without_clipping() {
        let grid = grid_with(vec![tower(0.0, 0.0)]);
        let start = Vec2::new(-800.0, 10.0);
        let goal = Vec2::new(800.0, -10.0);
        let route = grid.find_path(start, goal, HERO_R, &[]);
        assert!(route.reached && route.searched);
        assert!(route.waypoints.len() >= 2, "{route:?}");
        assert_eq!(route.end(), Some(goal));
        assert_route_clear(&grid, start, &route, HERO_R);
        // String-pulling keeps the detour tight: well under the cost of a wide arc.
        let mut length = 0.0;
        let mut from = start;
        for &wp in &route.waypoints {
            length += from.distance(wp);
            from = wp;
        }
        assert!(length < 1600.0 + 250.0, "detour too long: {length}");
    }

    #[test]
    fn hero_threads_multiple_obstacles() {
        // A wall of trees with one gap, then a tower behind the gap.
        let mut shapes: Vec<NavShape> = (-6..=6)
            .filter(|i| *i != 2)
            .map(|i| tree(0.0, i as f32 * 128.0))
            .collect();
        shapes.push(tower(500.0, 256.0));
        let grid = grid_with(shapes);
        let start = Vec2::new(-600.0, -300.0);
        let goal = Vec2::new(900.0, 300.0);
        let route = grid.find_path(start, goal, HERO_R, &[]);
        assert!(route.reached, "{route:?}");
        assert_route_clear(&grid, start, &route, HERO_R);
        // The only way through the wall is the gap at z = 256.
        let crossing = route
            .waypoints
            .iter()
            .any(|wp| wp.x.abs() < 200.0 && (wp.y - 256.0).abs() < 70.0);
        assert!(crossing, "route must use the gap: {route:?}");
    }

    #[test]
    fn gaps_narrower_than_the_unit_are_not_used() {
        // A wall of trees with 70-unit gaps: an 80-wide hero cannot fit, a 40-wide unit can.
        let shapes = (-8..=8)
            .map(|i| tree(0.0, i as f32 * 198.0))
            .collect();
        let grid = grid_with(shapes);
        let (a, b) = (Vec2::new(-600.0, 99.0), Vec2::new(600.0, 99.0));
        assert!(!grid.segment_clear(a, b, HERO_R, &[]));
        let route = grid.find_path(a, b, HERO_R, &[]);
        let mut from = a;
        for &wp in &route.waypoints {
            assert!(grid.segment_clear(from, wp, HERO_R - 0.5, &[]), "hero squeezed through a gap");
            from = wp;
        }
        let small = 20.0;
        assert!(grid.segment_clear(a, b, small, &[]));
    }

    #[test]
    fn non_navigable_destination_resolves_nearby() {
        let grid = grid_with(vec![tree(0.0, 0.0)]);
        let inside = Vec2::new(10.0, 5.0);
        let resolved = grid.nearest_walkable(inside, HERO_R).unwrap();
        assert!(grid.point_clear(resolved, HERO_R));
        assert!(resolved.distance(inside) < 64.0 + HERO_R + 40.0, "{resolved:?}");
        let route = grid.find_path(Vec2::new(-600.0, 0.0), inside, HERO_R, &[]);
        assert_eq!(route.end(), Some(resolved));
    }

    #[test]
    fn dynamic_blockers_only_apply_when_passed() {
        let grid = grid_with(vec![]);
        let start = Vec2::new(-400.0, 0.0);
        let goal = Vec2::new(400.0, 0.0);
        let blocker = DynamicBlocker {
            center: Vec2::ZERO,
            radius: 36.0,
        };
        let direct = grid.find_path(start, goal, HERO_R, &[]);
        assert_eq!(direct.waypoints.len(), 1);
        let around = grid.find_path(start, goal, HERO_R, &[blocker]);
        assert!(around.waypoints.len() >= 2);
        let mut from = start;
        for &wp in &around.waypoints {
            assert!(grid.segment_clear(from, wp, HERO_R - 0.5, &[blocker]));
            from = wp;
        }
    }

    #[test]
    fn unreachable_goal_returns_closest_point() {
        // Goal boxed in by trees on all sides.
        let mut shapes = Vec::new();
        for i in -3..=3 {
            let o = i as f32 * 128.0;
            shapes.push(tree(o, -384.0));
            shapes.push(tree(o, 384.0));
            shapes.push(tree(-384.0, o));
            shapes.push(tree(384.0, o));
        }
        let grid = grid_with(shapes);
        let route = grid.find_path(Vec2::new(-1200.0, 0.0), Vec2::ZERO, HERO_R, &[]);
        assert!(!route.reached);
        let end = route.end().unwrap();
        assert!(end.x < -384.0, "stops outside the box: {end:?}");
    }

    #[test]
    fn segment_tests_match_collision_shapes() {
        let circle = tower(0.0, 0.0);
        assert!(!circle.segment_clear(Vec2::new(-200.0, 130.0), Vec2::new(200.0, 130.0), 40.0));
        assert!(circle.segment_clear(Vec2::new(-200.0, 150.0), Vec2::new(200.0, 150.0), 40.0));
        let tree = tree(0.0, 0.0);
        assert!(!tree.segment_clear(Vec2::new(-200.0, 100.0), Vec2::new(200.0, 100.0), 40.0));
        assert!(tree.segment_clear(Vec2::new(-200.0, 110.0), Vec2::new(200.0, 110.0), 40.0));
        assert!((tree.distance(Vec2::new(100.0, 0.0)) - 36.0).abs() < 1e-4);
    }
}
