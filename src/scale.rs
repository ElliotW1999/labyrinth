//! World distance scale — gameplay units match MOBA conventions.
//!
//! Anchors (DotA/LoL-style):
//! - Tower attack range ≈ **700**
//! - Melee hero attack range ≈ **150**
//! - Hero base move speed ≈ **300** units/second
//!
//! Body / collision sizes are anchored so trees have radius [`TREE_RADIUS`].
//! Attack, cast, and AoE ranges stay as absolute MOBA numbers (not body-scaled).

/// Melee hero auto-attack range.
pub const MELEE_ATTACK_RANGE: f32 = 150.0;
/// Short-range / hybrid hero auto-attack range (e.g. Skirmisher).
pub const SHORT_ATTACK_RANGE: f32 = 150.0;
/// Ranged hero auto-attack range.
pub const RANGED_ATTACK_RANGE: f32 = 500.0;
/// Tower auto-attack range.
pub const TOWER_ATTACK_RANGE: f32 = 700.0;
/// Melee creep auto-attack range.
pub const CREEP_MELEE_ATTACK_RANGE: f32 = 100.0;
/// Ranged creep auto-attack range.
pub const CREEP_RANGED_ATTACK_RANGE: f32 = 500.0;
/// Alias — older call sites mean melee creeps.
#[allow(dead_code)]
pub const CREEP_ATTACK_RANGE: f32 = CREEP_MELEE_ATTACK_RANGE;

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

// --- Ability geometry (tuned vs melee 150 / ranged 500 / tower 700) ---
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
/// Skillshot / TargetPoint projectile corridor width.
pub const ABILITY_PROJECTILE_WIDTH: f32 = 90.0;

/// Tree trunk / collision cylinder radius (world units).
pub const TREE_RADIUS: f32 = 64.0;
/// Legacy tree radius that maps onto [`TREE_RADIUS`].
const TREE_LEGACY_RADIUS: f32 = 1.1;

/// Multiplier from the pre-rescale prototype world into map-layout units.
/// Chosen so legacy hero MS `11.5` maps to [`HERO_MOVE_SPEED`] (`11.5 * 26 ≈ 299`).
pub const LEGACY: f32 = 26.0;

/// Convert a legacy-world length into current map-layout units.
#[inline]
pub const fn u(legacy: f32) -> f32 {
    legacy * LEGACY
}

/// Convert a legacy body/collision length so trees land at [`TREE_RADIUS`].
/// Use for meshes and hitboxes of units, buildings, and trees — not for
/// attack / cast / AoE ranges.
#[inline]
pub const fn body(legacy: f32) -> f32 {
    legacy * (TREE_RADIUS / TREE_LEGACY_RADIUS)
}

/// Hero collision radius (matches scaled capsule).
pub const HERO_RADIUS: f32 = body(0.5);
/// Melee / default creep collision radius.
pub const CREEP_RADIUS: f32 = body(0.4);
/// Tower collision radius.
pub const TOWER_RADIUS: f32 = body(0.9);
/// Ancient collision radius.
pub const ANCIENT_RADIUS: f32 = body(1.8);

/// True when an auto-attack should use a melee slash instead of a projectile.
#[inline]
pub fn is_melee_attack_range(range: f32) -> bool {
    range > 0.0 && range <= MELEE_ATTACK_RANGE + 0.5
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
        assert!((MELEE_ATTACK_RANGE - 150.0).abs() < f32::EPSILON);
        assert!((CREEP_MELEE_ATTACK_RANGE - 100.0).abs() < f32::EPSILON);
        assert!((CREEP_RANGED_ATTACK_RANGE - 500.0).abs() < f32::EPSILON);
        assert!((TOWER_ATTACK_RANGE - 700.0).abs() < f32::EPSILON);
        assert!((HERO_MOVE_SPEED - 300.0).abs() < f32::EPSILON);
        assert!((TREE_RADIUS - 64.0).abs() < f32::EPSILON);
        assert!((body(1.1) - TREE_RADIUS).abs() < 0.01);
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

    #[test]
    fn melee_range_helper() {
        assert!(is_melee_attack_range(CREEP_MELEE_ATTACK_RANGE));
        assert!(is_melee_attack_range(MELEE_ATTACK_RANGE));
        assert!(!is_melee_attack_range(RANGED_ATTACK_RANGE));
        assert!(!is_melee_attack_range(TOWER_ATTACK_RANGE));
    }
}
