//! Unit gameplay dimensions and the shared distance / picking rules built on them.
//!
//! Every unit carries three independent sizes, none derived from its rendered mesh:
//! - [`CollisionRadius`] — physical footprint for unit-to-unit and pathing collision.
//! - [`BoundRadius`] — gameplay extent for attack reach, cast range, and AoE inclusion.
//! - [`SelectionBounds`] — mouse picking volume (hover, left-click, right-click target).
//!
//! Systems should use the helpers here instead of hand-rolling distance checks.

use bevy::prelude::*;

use crate::components::{BoundRadius, CollisionRadius, SelectionBounds};

/// Per-unit-type template for the three gameplay dimensions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitDimensions {
    pub collision: f32,
    pub bounds: f32,
    pub selection: SelectionBounds,
}

impl UnitDimensions {
    pub const HERO: Self = Self {
        collision: 40.0,
        bounds: 36.0,
        selection: SelectionBounds::new(Vec3::new(0.0, 15.0, 0.0), Vec3::new(80.0, 135.0, 80.0)),
    };
    pub const MELEE_CREEP: Self = Self {
        collision: 36.0,
        bounds: 24.0,
        selection: SelectionBounds::new(Vec3::new(0.0, 15.0, 0.0), Vec3::new(65.0, 115.0, 65.0)),
    };
    pub const RANGED_CREEP: Self = Self {
        collision: 28.0,
        bounds: 16.0,
        selection: SelectionBounds::new(Vec3::new(0.0, 15.0, 0.0), Vec3::new(65.0, 115.0, 65.0)),
    };
    pub const TOWER: Self = Self {
        collision: 100.0,
        bounds: 96.0,
        selection: SelectionBounds::new(Vec3::new(0.0, 30.0, 0.0), Vec3::new(110.0, 175.0, 110.0)),
    };
    pub const ANCIENT: Self = Self {
        collision: 80.0,
        bounds: 72.0,
        selection: SelectionBounds::new(Vec3::new(0.0, 40.0, 0.0), Vec3::new(90.0, 85.0, 90.0)),
    };

    pub fn components(self) -> (CollisionRadius, BoundRadius, SelectionBounds) {
        (
            CollisionRadius(self.collision),
            BoundRadius(self.bounds),
            self.selection,
        )
    }
}

/// Bounds radius of an optional component; units without one are treated as points.
#[inline]
pub fn bounds_of(bound: Option<&BoundRadius>) -> f32 {
    bound.map_or(0.0, |b| b.0)
}

/// Collision radius of an optional component; units without one do not obstruct.
#[inline]
pub fn collision_of(collision: Option<&CollisionRadius>) -> f32 {
    collision.map_or(0.0, |c| c.0)
}

/// Center-to-center distance on the ground plane (Y ignored).
#[inline]
pub fn center_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

/// Gap between two units' bounds circles: `center − source_bounds − target_bounds`, ≥ 0.
#[inline]
pub fn edge_distance(source: Vec3, source_bounds: f32, target: Vec3, target_bounds: f32) -> f32 {
    (center_distance(source, target) - source_bounds - target_bounds).max(0.0)
}

/// True when the target is within `range` measured edge-to-edge (attacks, unit-target casts).
#[inline]
pub fn within_range(
    source: Vec3,
    source_bounds: f32,
    target: Vec3,
    target_bounds: f32,
    range: f32,
) -> bool {
    edge_distance(source, source_bounds, target, target_bounds) <= range
}

/// True when a ground-centered area of `radius` touches the target's bounds circle.
#[inline]
pub fn area_contains(center: Vec3, radius: f32, target: Vec3, target_bounds: f32) -> bool {
    center_distance(center, target) <= radius + target_bounds
}

/// Edge gap a chasing unit closes to before it stops, kept inside range so small
/// target movement does not immediately drop it out again.
#[inline]
pub fn chase_stop_gap(range: f32) -> f32 {
    range * 0.9
}

/// True when a unit ordered to attack still has to walk toward its target.
#[inline]
pub fn needs_to_close(
    source: Vec3,
    source_bounds: f32,
    target: Vec3,
    target_bounds: f32,
    range: f32,
) -> bool {
    edge_distance(source, source_bounds, target, target_bounds) > chase_stop_gap(range)
}

/// Cast distance: edge-to-edge for unit targets, caster center to aim point otherwise.
#[inline]
pub fn cast_distance(
    caster: Vec3,
    caster_bounds: f32,
    aim: Vec3,
    unit_target_bounds: Option<f32>,
) -> f32 {
    match unit_target_bounds {
        Some(target_bounds) => edge_distance(caster, caster_bounds, aim, target_bounds),
        None => center_distance(caster, aim),
    }
}

/// Where a cursor ray passes through a unit's selection volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionHit {
    /// Ray distance to the volume entry point (camera depth).
    pub t: f32,
    /// How far the ray passes from the volume center, normalized per axis by the
    /// half extents (0 = dead center, ~1 = grazing an edge).
    pub centering: f32,
}

/// Slab-test a ray against a unit's world-aligned selection box.
pub fn ray_selection_hit(ray: Ray3d, unit_origin: Vec3, selection: &SelectionBounds) -> Option<SelectionHit> {
    let center = unit_origin + selection.offset;
    let half = selection.half_extents.max(Vec3::splat(1e-3));
    let min = center - half;
    let max = center + half;
    let origin = ray.origin;
    let dir = *ray.direction;

    let mut t_enter = 0.0_f32;
    let mut t_exit = f32::INFINITY;
    for axis in 0..3 {
        let o = origin[axis];
        let d = dir[axis];
        if d.abs() < 1e-8 {
            if o < min[axis] || o > max[axis] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d;
        let (t0, t1) = {
            let a = (min[axis] - o) * inv;
            let b = (max[axis] - o) * inv;
            if a <= b { (a, b) } else { (b, a) }
        };
        t_enter = t_enter.max(t0);
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return None;
        }
    }

    let t_closest = (center - origin).dot(dir).clamp(t_enter, t_exit);
    let offset = (origin + dir * t_closest - center) / half;
    Some(SelectionHit {
        t: t_enter,
        centering: offset.length(),
    })
}

/// Resolve which unit is under the cursor.
///
/// Forgiving selection volumes overlap, so the unit whose volume the ray passes
/// closest to the center of wins; camera depth, then entity id, break exact ties so
/// the result never depends on ECS iteration order.
pub fn pick_unit<I>(ray: Ray3d, candidates: I) -> Option<Entity>
where
    I: IntoIterator<Item = (Entity, Vec3, SelectionBounds)>,
{
    const CENTERING_EPSILON: f32 = 1e-3;
    const DEPTH_EPSILON: f32 = 1e-2;

    candidates
        .into_iter()
        .filter_map(|(entity, origin, selection)| {
            ray_selection_hit(ray, origin, &selection).map(|hit| (entity, hit))
        })
        .min_by(|(a_entity, a), (b_entity, b)| {
            if (a.centering - b.centering).abs() > CENTERING_EPSILON {
                return a.centering.total_cmp(&b.centering);
            }
            if (a.t - b.t).abs() > DEPTH_EPSILON {
                return a.t.total_cmp(&b.t);
            }
            a_entity
                .index()
                .cmp(&b_entity.index())
                .then_with(|| a_entity.to_bits().cmp(&b_entity.to_bits()))
        })
        .map(|(entity, _)| entity)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down_ray(x: f32, z: f32) -> Ray3d {
        Ray3d::new(Vec3::new(x, 1_000.0, z), Dir3::NEG_Y)
    }

    #[test]
    fn templates_keep_dimensions_independent() {
        for dims in [
            UnitDimensions::HERO,
            UnitDimensions::MELEE_CREEP,
            UnitDimensions::RANGED_CREEP,
            UnitDimensions::TOWER,
            UnitDimensions::ANCIENT,
        ] {
            assert!(dims.collision >= dims.bounds, "{dims:?}");
            assert!(dims.selection.half_extents.x > dims.collision, "{dims:?}");
        }
    }

    #[test]
    fn edge_distance_subtracts_both_bounds() {
        let a = Vec3::ZERO;
        let b = Vec3::new(300.0, 50.0, 0.0);
        assert!((edge_distance(a, 36.0, b, 24.0) - 240.0).abs() < 1e-4);
        assert_eq!(edge_distance(a, 200.0, b, 200.0), 0.0);
    }

    #[test]
    fn within_range_is_edge_to_edge() {
        let hero = UnitDimensions::HERO.bounds;
        let creep = UnitDimensions::MELEE_CREEP.bounds;
        let target = Vec3::new(150.0 + hero + creep, 0.0, 0.0);
        assert!(within_range(Vec3::ZERO, hero, target, creep, 150.0));
        assert!(!within_range(Vec3::ZERO, hero, target + Vec3::X, creep, 150.0));
    }

    #[test]
    fn area_includes_target_bounds() {
        assert!(area_contains(Vec3::ZERO, 200.0, Vec3::new(220.0, 0.0, 0.0), 24.0));
        assert!(!area_contains(Vec3::ZERO, 200.0, Vec3::new(230.0, 0.0, 0.0), 24.0));
    }

    #[test]
    fn chasing_stops_inside_range() {
        let target = Vec3::new(100.0, 0.0, 0.0);
        assert!(!needs_to_close(Vec3::ZERO, 10.0, target, 10.0, 100.0));
        assert!(needs_to_close(Vec3::ZERO, 10.0, target * 3.0, 10.0, 100.0));
    }

    #[test]
    fn unit_casts_use_edges_ground_casts_use_center() {
        let aim = Vec3::new(500.0, 0.0, 0.0);
        assert!((cast_distance(Vec3::ZERO, 36.0, aim, Some(24.0)) - 440.0).abs() < 1e-4);
        assert!((cast_distance(Vec3::ZERO, 36.0, aim, None) - 500.0).abs() < 1e-4);
    }

    #[test]
    fn ray_hits_tall_selection_volume_above_feet() {
        let selection = UnitDimensions::HERO.selection;
        let unit = Vec3::new(0.0, 120.0, 0.0);
        // Slanted camera ray that only crosses the box near head height.
        let ray = Ray3d::new(Vec3::new(0.0, 2_000.0, 1_000.0), Dir3::new(Vec3::new(0.0, -1.8, -1.0)).unwrap());
        assert!(ray_selection_hit(ray, unit, &selection).is_some());
        assert!(ray_selection_hit(down_ray(200.0, 0.0), unit, &selection).is_none());
    }

    #[test]
    fn pick_prefers_most_centered_then_is_deterministic() {
        let selection = UnitDimensions::MELEE_CREEP.selection;
        let a = Entity::from_raw_u32(1).unwrap();
        let b = Entity::from_raw_u32(2).unwrap();
        let ray = down_ray(10.0, 0.0);
        let near_a = [(a, Vec3::new(0.0, 100.0, 0.0), selection), (b, Vec3::new(60.0, 100.0, 0.0), selection)];
        assert_eq!(pick_unit(ray, near_a), Some(a));
        let reversed = [near_a[1], near_a[0]];
        assert_eq!(pick_unit(ray, reversed), Some(a));

        let stacked = [(b, Vec3::new(0.0, 100.0, 0.0), selection), (a, Vec3::new(0.0, 100.0, 0.0), selection)];
        assert_eq!(pick_unit(down_ray(0.0, 0.0), stacked), Some(a));
    }
}
