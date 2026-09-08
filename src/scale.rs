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

// --- Ability geometry (tuned vs melee 128 / ranged 500 / tower 700) ---
/// Dash / Blink travel distance (rank 1).
pub const ABILITY_DASH_RANGE: f32 = 350.0;
pub const ABILITY_BLINK_RANGE: f32 = 400.0;
/// Unit-target cast range (Bolt / Execute).
pub const ABILITY_UNIT_CAST_RANGE: f32 = 500.0;
/// Ground-target cast range (Nova / Meteor / missiles).
pub const ABILITY_GROUND_CAST_RANGE: f32 = 550.0;
/// Instant AoE radius around caster (Shockwave / Flurry / Frost).
pub const ABILITY_INSTANT_AOE: f32 = 220.0;
/// Targeted ground AoE radius (Nova / Meteor / Caltrops).
pub const ABILITY_GROUND_AOE: f32 = 250.0;
pub const ABILITY_ULT_AOE: f32 = 320.0;

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

    #[test]
    fn ability_geometry_near_combat_anchors() {
        // Cast/AoE should sit near auto-attack scale — not raw legacy×26 blobs.
        assert!(ABILITY_INSTANT_AOE > MELEE_ATTACK_RANGE);
        assert!(ABILITY_INSTANT_AOE < TOWER_ATTACK_RANGE);
        assert!(ABILITY_UNIT_CAST_RANGE >= RANGED_ATTACK_RANGE);
        assert!(ABILITY_DASH_RANGE > MELEE_ATTACK_RANGE * 2.0);
        assert!(ABILITY_ULT_AOE > ABILITY_GROUND_AOE);
    }
}
