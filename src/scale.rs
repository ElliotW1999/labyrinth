//! World distance scale — gameplay units match MOBA conventions.
//!
//! Anchors (DotA/LoL-style):
//! - Tower attack range ≈ **700**
//! - Melee hero attack range ≈ **150**
//! - Hero base move speed ≈ **300** units/second
//! - Map size **15200×15200**
//!
//! Body meshes use [`body`]. Map layout XZ uses [`map`]. Attack/cast/AoE
//! ranges stay absolute MOBA numbers.

/// Full map width/depth in world units.
pub const MAP_SIZE: f32 = 15_200.0;
/// Half-extent from map center to edge.
pub const MAP_HALF: f32 = MAP_SIZE * 0.5;
/// How many world units of X the camera should show at the focus plane.
pub const VISIBLE_WORLD_X: f32 = 3_600.0;

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

// --- Ability geometry ---
pub const ABILITY_DASH_RANGE: f32 = 350.0;
pub const ABILITY_BLINK_RANGE: f32 = 400.0;
pub const ABILITY_UNIT_CAST_RANGE: f32 = 500.0;
pub const ABILITY_GROUND_CAST_RANGE: f32 = 550.0;
pub const ABILITY_INSTANT_AOE: f32 = 220.0;
pub const ABILITY_GROUND_AOE: f32 = 250.0;
pub const ABILITY_ULT_AOE: f32 = 320.0;
pub const ABILITY_PROJECTILE_WIDTH: f32 = 90.0;

// --- Bound radii (attack reach + spell AoE inclusion) ---
pub const HERO_BOUND: f32 = 24.0;
pub const MELEE_CREEP_BOUND: f32 = 16.0;
pub const RANGED_CREEP_BOUND: f32 = 8.0;
pub const TOWER_BOUND: f32 = 144.0;
pub const ANCIENT_BOUND: f32 = 180.0;

// --- Collision radii (obstruction; circles must not intersect) ---
pub const HERO_COLLISION: f32 = 27.0;
pub const MELEE_CREEP_COLLISION: f32 = 27.0;
pub const RANGED_CREEP_COLLISION: f32 = 18.0;
pub const TOWER_COLLISION: f32 = 144.0;
pub const ANCIENT_COLLISION: f32 = 180.0;

/// Tree axis-aligned collision box side length (128×128).
pub const TREE_COLLISION_SIZE: f32 = 128.0;
pub const TREE_COLLISION_HALF: f32 = TREE_COLLISION_SIZE * 0.5;
/// Visual trunk radius — smaller than the 128×128 collision box.
pub const TREE_MODEL_RADIUS: f32 = 40.0;
pub const TREE_MODEL_HEIGHT: f32 = 160.0;

/// Legacy alias for [`TREE_MODEL_RADIUS`].
#[allow(dead_code)]
pub const TREE_RADIUS: f32 = TREE_MODEL_RADIUS;
const TREE_LEGACY_RADIUS: f32 = 1.1;

/// Multiplier from the pre-rescale prototype world into small-map layout units.
pub const LEGACY: f32 = 26.0;

/// Convert a legacy-world length into the old 26× layout units (Y offsets, etc.).
#[inline]
pub const fn u(legacy: f32) -> f32 {
    legacy * LEGACY
}

/// Map-layout XZ: legacy coords where ±70 was the old half-extent → ±[`MAP_HALF`].
#[inline]
pub const fn map(legacy: f32) -> f32 {
    legacy * (MAP_HALF / 70.0)
}

/// Convert a legacy body mesh length (visuals still use the body scale).
#[inline]
pub const fn body(legacy: f32) -> f32 {
    legacy * (TREE_MODEL_RADIUS / TREE_LEGACY_RADIUS)
}

/// Deprecated aliases kept for transitional call sites.
#[allow(dead_code)]
pub const HERO_RADIUS: f32 = HERO_COLLISION;
#[allow(dead_code)]
pub const CREEP_RADIUS: f32 = MELEE_CREEP_COLLISION;
#[allow(dead_code)]
pub const TOWER_RADIUS: f32 = TOWER_COLLISION;
#[allow(dead_code)]
pub const ANCIENT_RADIUS: f32 = ANCIENT_COLLISION;

/// True when an auto-attack should use a melee slash instead of a projectile.
#[inline]
pub fn is_melee_attack_range(range: f32) -> bool {
    range > 0.0 && range <= MELEE_ATTACK_RANGE + 0.5
}

/// Selection-box half-extent for a model with given XZ width and length
/// (square of side `max(width, length)`).
#[inline]
pub fn selection_half(model_width: f32, model_length: f32) -> f32 {
    model_width.max(model_length) * 0.5
}

/// Attack reach: gap between bound edges ≤ attack_range.
#[inline]
pub fn attack_reach(attacker_bound: f32, attack_range: f32, target_bound: f32) -> f32 {
    attack_range + attacker_bound + target_bound
}

/// Scale an XZ map position (Y uses [`body`] for prop height).
#[inline]
pub fn v(x: f32, y: f32, z: f32) -> bevy::math::Vec3 {
    bevy::math::Vec3::new(map(x), body(y), map(z))
}

/// Flat map position at ground level.
#[inline]
pub fn ground(x: f32, z: f32) -> bevy::math::Vec3 {
    bevy::math::Vec3::new(map(x), 0.0, map(z))
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
        assert!((MAP_SIZE - 15_200.0).abs() < f32::EPSILON);
        assert!((map(70.0) - MAP_HALF).abs() < 0.01);
        assert!((HERO_BOUND - 24.0).abs() < f32::EPSILON);
        assert!((HERO_COLLISION - 27.0).abs() < f32::EPSILON);
        assert!((TREE_COLLISION_SIZE - 128.0).abs() < f32::EPSILON);
        assert!(TREE_MODEL_RADIUS < TREE_COLLISION_HALF);
        assert!((u(11.5) - 299.0).abs() < 1.0);
    }

    #[test]
    fn ability_geometry_near_combat_anchors() {
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

    #[test]
    fn attack_reach_includes_both_bounds() {
        assert!((attack_reach(24.0, 150.0, 16.0) - 190.0).abs() < f32::EPSILON);
    }
}
