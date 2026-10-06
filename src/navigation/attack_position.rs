//! Attack positioning: attackers walk to a free, reachable spot within range of
//! their target instead of to the target's center.
//!
//! Candidates are sampled on rings around the target (no fixed slots). Each is
//! scored by walking distance plus penalties for spots occupied by other units
//! (standing units weigh most) or already reserved by another attacker this frame,
//! so melee attackers fan out around the target instead of queueing behind
//! each other. A goal is kept until it stops being valid, which avoids churn.

use bevy::prelude::*;

use super::grid::{flat, ground, NavGrid};
use crate::components::{
    Ancient, AttackSwing, AttackTarget, BoundRadius, CollisionRadius, CombatStats, Health,
    MoveTarget, Tower,
};
use crate::dimensions::{bounds_of, chase_stop_gap, collision_of, needs_to_close};

/// Target drift (from where the goal was chosen) that forces a new goal.
pub const TARGET_MOVE_REPLAN: f32 = 64.0;
/// Fraction of attack range used as the preferred edge gap (inside the 0.9 stop gap).
const RING_GAP: f32 = 0.75;
const OCCUPIED_STATIONARY: f32 = 900.0;
const OCCUPIED_MOVING: f32 = 150.0;
const RESERVED: f32 = 700.0;
const DETOUR: f32 = 0.6;

/// Where this attacker is heading to attack `target`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct AttackPositionGoal {
    pub target: Entity,
    pub point: Vec3,
    /// Target position when the goal was chosen.
    pub anchor: Vec3,
}

#[derive(Debug, Clone, Copy)]
pub struct AttackGeometry {
    pub attacker: Vec2,
    pub attacker_radius: f32,
    pub attacker_bounds: f32,
    pub range: f32,
    pub target: Vec2,
    pub target_radius: f32,
    pub target_bounds: f32,
}

impl AttackGeometry {
    /// Center distance at which the edge gap equals the preferred ring gap.
    fn ring_radii(&self) -> Vec<f32> {
        let min = self.attacker_radius + self.target_radius + 2.0;
        let outer = (self.attacker_bounds + self.target_bounds + self.range * RING_GAP).max(min);
        let mut radii = vec![outer];
        if outer - min > self.attacker_radius * 2.0 + 8.0 {
            radii.push((outer + min) * 0.5);
        }
        radii
    }

    fn in_stop_range(&self, point: Vec2) -> bool {
        point.distance(self.target) - self.attacker_bounds - self.target_bounds
            <= chase_stop_gap(self.range) - 1.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Occupant {
    pub entity: Entity,
    pub pos: Vec2,
    pub radius: f32,
    pub stationary: bool,
}

fn crowding(point: Vec2, r: f32, me: Entity, target: Entity, occupants: &[Occupant], reserved: &[(Vec2, f32)]) -> f32 {
    let mut penalty = 0.0;
    for o in occupants {
        if o.entity == me || o.entity == target {
            continue;
        }
        let min = r + o.radius;
        let d = point.distance(o.pos);
        if d < min {
            let weight = if o.stationary { OCCUPIED_STATIONARY } else { OCCUPIED_MOVING };
            penalty += weight * (0.3 + (1.0 - d / min));
        }
    }
    for (pos, rr) in reserved {
        let min = r + rr;
        let d = point.distance(*pos);
        if d < min {
            penalty += RESERVED * (0.3 + (1.0 - d / min));
        }
    }
    penalty
}

/// Best free spot to attack from, or `None` if no candidate is walkable.
pub fn choose_attack_position(
    grid: &NavGrid,
    geo: &AttackGeometry,
    me: Entity,
    target: Entity,
    occupants: &[Occupant],
    reserved: &[(Vec2, f32)],
) -> Option<Vec2> {
    let approach = geo.attacker - geo.target;
    let base = if approach.length_squared() > 1e-4 {
        approach.y.atan2(approach.x)
    } else {
        0.0
    };
    let r = geo.attacker_radius;
    let mut best: Option<(f32, Vec2)> = None;
    for (ring_index, radius) in geo.ring_radii().into_iter().enumerate() {
        let count = ((std::f32::consts::TAU * radius) / (2.0 * r + 8.0)).ceil().clamp(8.0, 32.0) as i32;
        let step = std::f32::consts::TAU / count as f32;
        // Alternate around the approach angle so ties favor the near side.
        for k in 0..count {
            let offset = if k % 2 == 0 { (k / 2) as f32 } else { -(((k + 1) / 2) as f32) };
            let angle = base + offset * step;
            let point = geo.target + Vec2::new(angle.cos(), angle.sin()) * radius;
            if !grid.point_clear(point, r) {
                continue;
            }
            let travel = geo.attacker.distance(point);
            let mut score = travel + crowding(point, r, me, target, occupants, reserved);
            if !grid.segment_clear(geo.attacker, point, r, &[]) {
                score += travel * DETOUR;
            }
            score += ring_index as f32 * 30.0;
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, point));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Keep the current goal while it is still in range, walkable, and uncontested.
pub fn goal_still_valid(
    grid: &NavGrid,
    geo: &AttackGeometry,
    goal: Vec2,
    anchor: Vec2,
    me: Entity,
    target: Entity,
    occupants: &[Occupant],
    reserved: &[(Vec2, f32)],
) -> bool {
    let r = geo.attacker_radius;
    if anchor.distance(geo.target) > TARGET_MOVE_REPLAN || !geo.in_stop_range(goal) {
        return false;
    }
    if !grid.point_clear(goal, r) {
        return false;
    }
    let taken_by_standing = occupants.iter().any(|o| {
        o.entity != me && o.entity != target && o.stationary && goal.distance(o.pos) < r + o.radius - 4.0
    });
    let reserved_by_other = reserved
        .iter()
        .any(|(p, rr)| goal.distance(*p) < r + rr - 4.0);
    !taken_by_standing && !reserved_by_other
}

type AttackerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static CombatStats,
        Option<&'static BoundRadius>,
        Option<&'static CollisionRadius>,
        &'static AttackTarget,
        Option<&'static AttackPositionGoal>,
        Option<&'static MoveTarget>,
        Has<AttackSwing>,
    ),
    (Without<Tower>, Without<Ancient>),
>;

type UnitQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        Option<&'static BoundRadius>,
        Option<&'static CollisionRadius>,
        Has<MoveTarget>,
        Has<Tower>,
        Has<Ancient>,
    ),
    With<Health>,
>;

/// Replaces center-chasing for every mobile attacker (creeps, heroes, player):
/// in range ⇒ stand still; otherwise walk to a reserved spot within range.
pub fn assign_attack_positions(
    grid: Res<NavGrid>,
    attackers: AttackerQuery,
    units: UnitQuery,
    stale_goals: Query<Entity, (With<AttackPositionGoal>, Without<AttackTarget>)>,
    mut commands: Commands,
) {
    for entity in &stale_goals {
        commands.entity(entity).remove::<AttackPositionGoal>();
    }

    let occupants: Vec<Occupant> = units
        .iter()
        .filter(|(_, _, _, c, _, tower, ancient)| !tower && !ancient && collision_of(*c) > 0.0)
        .map(|(entity, tf, _, c, moving, _, _)| Occupant {
            entity,
            pos: flat(tf.translation),
            radius: collision_of(c),
            stationary: !moving,
        })
        .collect();

    struct Pending {
        entity: Entity,
        target: Entity,
        geo: AttackGeometry,
        goal: Option<AttackPositionGoal>,
        move_target: Option<Vec3>,
        priority: f32,
    }
    let mut pending = Vec::new();

    for (entity, tf, stats, bound, collision, AttackTarget(target), goal, move_target, swinging) in &attackers {
        if stats.move_speed <= 0.0 {
            continue;
        }
        let Ok((_, target_tf, target_bound, target_collision, _, _, _)) = units.get(*target) else {
            commands
                .entity(entity)
                .remove::<(AttackTarget, AttackPositionGoal, MoveTarget)>();
            continue;
        };
        let (sb, tb) = (bounds_of(bound), bounds_of(target_bound));
        if !needs_to_close(tf.translation, sb, target_tf.translation, tb, stats.attack_range) {
            commands.entity(entity).remove::<(MoveTarget, AttackPositionGoal)>();
            continue;
        }
        if swinging {
            continue;
        }
        let geo = AttackGeometry {
            attacker: flat(tf.translation),
            attacker_radius: collision_of(collision),
            attacker_bounds: sb,
            range: stats.attack_range,
            target: flat(target_tf.translation),
            target_radius: collision_of(target_collision),
            target_bounds: tb,
        };
        pending.push(Pending {
            entity,
            target: *target,
            geo,
            goal: goal.copied().filter(|g| g.target == *target),
            move_target: move_target.map(|m| m.position),
            priority: geo.attacker.distance(geo.target),
        });
    }

    // Closest attackers claim spots first; entity id keeps ties deterministic.
    pending.sort_by(|a, b| {
        a.priority
            .total_cmp(&b.priority)
            .then(a.entity.to_bits().cmp(&b.entity.to_bits()))
    });

    let mut reserved: Vec<(Vec2, f32)> = Vec::with_capacity(pending.len());
    for p in pending {
        let r = p.geo.attacker_radius;
        let kept = p.goal.filter(|g| {
            goal_still_valid(
                &grid,
                &p.geo,
                flat(g.point),
                flat(g.anchor),
                p.entity,
                p.target,
                &occupants,
                &reserved,
            )
        });
        let point = match kept {
            Some(goal) => goal.point,
            None => {
                let chosen = choose_attack_position(&grid, &p.geo, p.entity, p.target, &occupants, &reserved)
                    .unwrap_or(p.geo.target);
                let goal = AttackPositionGoal {
                    target: p.target,
                    point: ground(chosen),
                    anchor: ground(p.geo.target),
                };
                commands.entity(p.entity).insert(goal);
                goal.point
            }
        };
        reserved.push((flat(point), r));
        let needs_order = p
            .move_target
            .is_none_or(|m| flat(m).distance(flat(point)) > 1.0);
        if needs_order && p.geo.attacker.distance(flat(point)) > 6.0 {
            commands.entity(p.entity).insert(MoveTarget { position: point });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(i: u32) -> Entity {
        Entity::from_raw_u32(i).unwrap()
    }

    fn melee(attacker: Vec2, target: Vec2) -> AttackGeometry {
        AttackGeometry {
            attacker,
            attacker_radius: 36.0,
            attacker_bounds: 24.0,
            range: 100.0,
            target,
            target_radius: 40.0,
            target_bounds: 36.0,
        }
    }

    /// Assign spots for several attackers the way the system does (closest first).
    fn assign(grid: &NavGrid, geos: &[AttackGeometry], target: Entity) -> Vec<Vec2> {
        let mut reserved = Vec::new();
        let mut out = Vec::new();
        for (i, geo) in geos.iter().enumerate() {
            let p = choose_attack_position(grid, geo, e(10 + i as u32), target, &[], &reserved).unwrap();
            reserved.push((p, geo.attacker_radius));
            out.push(p);
        }
        out
    }

    #[test]
    fn melee_attackers_spread_around_target() {
        let grid = NavGrid::new(3000.0, 64.0);
        let target = Vec2::ZERO;
        // Five melee creeps arriving in a column from the same side.
        let geos: Vec<_> = (0..5)
            .map(|i| melee(Vec2::new(-400.0 - i as f32 * 80.0, 0.0), target))
            .collect();
        let spots = assign(&grid, &geos, e(1));
        for (i, a) in spots.iter().enumerate() {
            assert!(geos[i].in_stop_range(*a), "spot {a:?} must be within range");
            for b in &spots[i + 1..] {
                assert!(a.distance(*b) >= 72.0 - 0.5, "spots overlap: {a:?} {b:?}");
            }
        }
        // The first attacker takes the near side.
        assert!(spots[0].x < 0.0);
    }

    #[test]
    fn ranged_attackers_get_distinct_spots_within_range() {
        let grid = NavGrid::new(3000.0, 64.0);
        let geos: Vec<_> = (0..4)
            .map(|i| AttackGeometry {
                attacker: Vec2::new(-900.0, i as f32 * 30.0),
                attacker_radius: 28.0,
                attacker_bounds: 16.0,
                range: 500.0,
                target: Vec2::ZERO,
                target_radius: 40.0,
                target_bounds: 36.0,
            })
            .collect();
        let spots = assign(&grid, &geos, e(1));
        for (i, a) in spots.iter().enumerate() {
            assert!(geos[i].in_stop_range(*a));
            for b in &spots[i + 1..] {
                assert!(a.distance(*b) >= 56.0 - 0.5);
            }
        }
    }

    #[test]
    fn standing_units_and_obstacles_are_avoided() {
        let mut grid = NavGrid::new(3000.0, 64.0);
        grid.rebuild(vec![crate::navigation::NavShape::Aabb {
            center: Vec2::new(-200.0, 0.0),
            half: Vec2::splat(64.0),
        }]);
        let geo = melee(Vec2::new(-600.0, 0.0), Vec2::ZERO);
        let blocker = Occupant {
            entity: e(50),
            pos: Vec2::new(-130.0, 60.0),
            radius: 36.0,
            stationary: true,
        };
        let p = choose_attack_position(&grid, &geo, e(10), e(1), &[blocker], &[]).unwrap();
        assert!(grid.point_clear(p, 36.0));
        assert!(p.distance(blocker.pos) >= 72.0);
        assert!(geo.in_stop_range(p));
    }

    #[test]
    fn goal_invalidates_when_target_moves_or_spot_taken() {
        let grid = NavGrid::new(3000.0, 64.0);
        let geo = melee(Vec2::new(-400.0, 0.0), Vec2::ZERO);
        let goal = choose_attack_position(&grid, &geo, e(10), e(1), &[], &[]).unwrap();
        assert!(goal_still_valid(&grid, &geo, goal, Vec2::ZERO, e(10), e(1), &[], &[]));
        let moved = AttackGeometry {
            target: Vec2::new(100.0, 0.0),
            ..geo
        };
        assert!(!goal_still_valid(&grid, &moved, goal, Vec2::ZERO, e(10), e(1), &[], &[]));
        let squatter = Occupant {
            entity: e(60),
            pos: goal,
            radius: 36.0,
            stationary: true,
        };
        assert!(!goal_still_valid(&grid, &geo, goal, Vec2::ZERO, e(10), e(1), &[squatter], &[]));
    }
}
