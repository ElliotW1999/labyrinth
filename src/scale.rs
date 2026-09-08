//! World distance scale — gameplay units match MOBA conventions.
//!
//! Anchors (DotA/LoL-style):
//! - Tower attack range ≈ **700**
//! - Melee hero attack range ≈ **128**
//! - Hero base move speed ≈ **300** units/second
//!
//! The previous prototype used a ~26× smaller “meter” world. [`u`] converts those
//! legacy lengths into current units so map layout, meshes, and radii stay
//! consistent with the new combat numbers.

/// Melee hero auto-attack range.
pub const MELEE_ATTACK_RANGE: f32 = 128.0;
/// Short-range / hybrid hero auto-attack range (e.g. Skirmisher).
pub const SHORT_ATTACK_RANGE: f32 = 150.0;
/// Ranged hero auto-attack range.
pub const RANGED_ATTACK_RANGE: f32 = 500.0;
/// Tower auto-attack range.
pub const TOWER_ATTACK_RANGE: f32 = 700.0;
/// Creep melee auto-attack range.
pub const CREEP_ATTACK_RANGE: f32 = 110.0;

/// Typical hero base movement speed (units / second).
pub const HERO_MOVE_SPEED: f32 = 300.0;
/// Faster hero MS (Skirmisher-style).
pub const HERO_MOVE_SPEED_FAST: f32 = 335.0;
/// Slightly slower caster MS.
pub const HERO_MOVE_SPEED_SLOW: f32 = 290.0;
/// Lane creep movement speed.
pub const CREEP_MOVE_SPEED: f32 = 195.0;

/// Day vision — heroes / towers see farther than they attack.
pub const HERO_VISION_RANGE: f32 = 1800.0;
pub const TOWER_VISION_RANGE: f32 = 1900.0;
pub const CREEP_VISION_RANGE: f32 = 800.0;

/// Multiplier from the pre-rescale prototype world into current units.
/// Chosen so legacy hero MS `11.5` maps to [`HERO_MOVE_SPEED`] (`11.5 * 26 ≈ 299`).
pub const LEGACY: f32 = 26.0;

/// Convert a legacy-world length into current world units.
#[inline]
pub const fn u(legacy: f32) -> f32 {
    legacy * LEGACY
}

/// Scale an XZ (or XYZ) position from legacy coordinates.
#[inline]
pub fn v(x: f32, y: f32, z: f32) -> bevy::math::Vec3 {
    bevy::math::Vec3::new(u(x), u(y), u(z))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_match_moba_targets() {
        assert!((MELEE_ATTACK_RANGE - 128.0).abs() < f32::EPSILON);
        assert!((TOWER_ATTACK_RANGE - 700.0).abs() < f32::EPSILON);
        assert!((HERO_MOVE_SPEED - 300.0).abs() < f32::EPSILON);
        // Legacy MS 11.5 maps near 300.
        assert!((u(11.5) - 299.0).abs() < 1.0);
    }
}
