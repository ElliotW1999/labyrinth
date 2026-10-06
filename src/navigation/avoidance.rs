//! Local avoidance: a lateral sidestep around units directly ahead.
//!
//! Units are never part of the static grid. Instead each mover looks a short
//! distance along its desired direction and slides sideways past whoever is in
//! the way, committing to one side so it doesn't jitter. Stationary units weigh
//! more than moving ones (a moving unit will usually clear on its own). Hard
//! non-overlap is still enforced afterwards by the collision pass.

use bevy::prelude::*;

/// Extra gap kept between unit edges when passing.
const PASS_PAD: f32 = 6.0;
/// Upper bound on the lateral offset (relative to a unit-length desired direction).
const MAX_OFFSET: f32 = 1.6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neighbor {
    pub pos: Vec2,
    pub radius: f32,
    pub stationary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Avoidance {
    /// Lateral offset to add to the desired direction.
    pub offset: Vec2,
    /// Side committed to (+1 = `perp(desired)`, −1 = opposite, 0 = no avoidance).
    pub side: f32,
}

/// Right-hand perpendicular on the ground plane.
#[inline]
pub fn perp(d: Vec2) -> Vec2 {
    Vec2::new(-d.y, d.x)
}

/// Compute the sidestep for a unit at `pos` (radius `r`) heading along unit vector `desired`.
///
/// `lookahead` is how far ahead stationary units are considered; moving units only
/// matter when they are close. `prev_side` keeps the previous frame's choice while
/// the obstruction is still roughly centered.
pub fn avoid(pos: Vec2, r: f32, desired: Vec2, lookahead: f32, prev_side: f32, neighbors: &[Neighbor]) -> Avoidance {
    let side_axis = perp(desired);
    let mut strongest: Option<(f32, f32)> = None;
    let mut total = 0.0;

    for n in neighbors {
        let rel = n.pos - pos;
        let along = rel.dot(desired);
        if along <= 0.0 {
            continue;
        }
        let reach = if n.stationary { lookahead } else { lookahead * 0.4 };
        if along > reach + n.radius + r {
            continue;
        }
        let lateral = rel.dot(side_axis);
        let min_sep = r + n.radius + PASS_PAD;
        if lateral.abs() >= min_sep {
            continue;
        }
        let closeness = 1.0 - (along / (reach + n.radius + r)).clamp(0.0, 1.0);
        let centered = 1.0 - lateral.abs() / min_sep;
        let weight = if n.stationary { 1.0 } else { 0.45 };
        let urgency = weight * (0.35 + 0.65 * closeness) * (0.4 + 0.6 * centered);
        total += urgency;
        if strongest.is_none_or(|(u, _)| urgency > u) {
            strongest = Some((urgency, lateral));
        }
    }

    let Some((_, lateral)) = strongest else {
        return Avoidance::default();
    };
    // Pass on the side away from the main obstruction; keep the previous choice
    // while it is nearly dead ahead so the unit doesn't flip back and forth.
    let natural = if lateral > 0.0 { -1.0 } else { 1.0 };
    let side = if prev_side != 0.0 && lateral.abs() < (r + PASS_PAD) * 0.75 {
        prev_side
    } else if lateral.abs() < 1e-3 {
        1.0
    } else {
        natural
    };
    Avoidance {
        offset: side_axis * side * total.min(MAX_OFFSET),
        side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(x: f32, y: f32, stationary: bool) -> Neighbor {
        Neighbor {
            pos: Vec2::new(x, y),
            radius: 36.0,
            stationary,
        }
    }

    #[test]
    fn no_neighbors_no_offset() {
        let a = avoid(Vec2::ZERO, 36.0, Vec2::X, 200.0, 0.0, &[]);
        assert_eq!(a, Avoidance::default());
        let behind = avoid(Vec2::ZERO, 36.0, Vec2::X, 200.0, 0.0, &[unit(-80.0, 0.0, true)]);
        assert_eq!(behind.side, 0.0);
    }

    #[test]
    fn sidesteps_away_from_unit_ahead() {
        // Blocker slightly to the +perp side → pass on the −perp side.
        let a = avoid(Vec2::ZERO, 36.0, Vec2::X, 200.0, 0.0, &[unit(100.0, 20.0, true)]);
        assert_eq!(a.side, -1.0);
        assert!(a.offset.y < 0.0);
        let b = avoid(Vec2::ZERO, 36.0, Vec2::X, 200.0, 0.0, &[unit(100.0, -20.0, true)]);
        assert_eq!(b.side, 1.0);
    }

    #[test]
    fn stationary_units_push_harder_and_side_is_sticky() {
        let still = avoid(Vec2::ZERO, 36.0, Vec2::X, 200.0, 0.0, &[unit(60.0, 5.0, true)]);
        let moving = avoid(Vec2::ZERO, 36.0, Vec2::X, 200.0, 0.0, &[unit(60.0, 5.0, false)]);
        assert!(still.offset.length() > moving.offset.length());
        let sticky = avoid(Vec2::ZERO, 36.0, Vec2::X, 200.0, 1.0, &[unit(60.0, 5.0, true)]);
        assert_eq!(sticky.side, 1.0, "keeps the committed side while nearly centered");
    }

    #[test]
    fn units_already_beside_the_path_are_ignored() {
        let a = avoid(Vec2::ZERO, 36.0, Vec2::X, 200.0, 0.0, &[unit(80.0, 90.0, true)]);
        assert_eq!(a.side, 0.0);
    }
}
