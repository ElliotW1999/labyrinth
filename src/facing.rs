//! Shared yaw / turn-rate helpers.

use bevy::prelude::*;

/// Facing cone before move / attack / cast may begin (11.5°).
pub const FACING_TOLERANCE_RAD: f32 = 11.5_f32 * std::f32::consts::PI / 180.0;

/// `CombatStats.turn_rate` is radians turned per this many seconds.
pub const TURN_RATE_PERIOD: f32 = 0.03;

/// Default hero turn rate (radians per [`TURN_RATE_PERIOD`]).
pub const HERO_TURN_RATE: f32 = 0.6;

/// Default creep turn rate (radians per [`TURN_RATE_PERIOD`]).
pub const CREEP_TURN_RATE: f32 = 0.5;

/// Signed yaw delta from current facing toward `desired_dir` on the XZ plane.
pub fn yaw_delta_to(transform: &Transform, desired_dir: Vec3) -> Option<f32> {
    let flat = Vec3::new(desired_dir.x, 0.0, desired_dir.z);
    if flat.length_squared() < 1e-6 {
        return None;
    }
    let desired_yaw = flat.x.atan2(flat.z);
    let (current_yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let mut delta = desired_yaw - current_yaw;
    const PI: f32 = std::f32::consts::PI;
    while delta > PI {
        delta -= 2.0 * PI;
    }
    while delta < -PI {
        delta += 2.0 * PI;
    }
    Some(delta)
}

/// True when the unit already faces `desired_dir` within [`FACING_TOLERANCE_RAD`].
pub fn is_facing(transform: &Transform, desired_dir: Vec3) -> bool {
    match yaw_delta_to(transform, desired_dir) {
        None => true,
        Some(delta) => delta.abs() <= FACING_TOLERANCE_RAD,
    }
}

/// Rotate `transform` toward a world-space direction on the XZ plane.
///
/// `turn_rate` is radians per [`TURN_RATE_PERIOD`] (0.03s).
/// Returns true when the unit is facing within [`FACING_TOLERANCE_RAD`].
pub fn turn_toward(transform: &mut Transform, desired_dir: Vec3, turn_rate: f32, dt: f32) -> bool {
    let Some(delta) = yaw_delta_to(transform, desired_dir) else {
        return true;
    };
    if delta.abs() <= FACING_TOLERANCE_RAD {
        return true;
    }
    if turn_rate <= 0.0 {
        return false;
    }

    let desired_yaw = {
        let flat = Vec3::new(desired_dir.x, 0.0, desired_dir.z);
        flat.x.atan2(flat.z)
    };
    let (current_yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let max_step = turn_rate * (dt / TURN_RATE_PERIOD);
    if delta.abs() <= max_step {
        transform.rotation = Quat::from_rotation_y(desired_yaw);
        true
    } else {
        transform.rotation = Quat::from_rotation_y(current_yaw + delta.signum() * max_step);
        is_facing(transform, desired_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_tolerance_is_eleven_point_five_degrees() {
        assert!((FACING_TOLERANCE_RAD.to_degrees() - 11.5).abs() < 1e-3);
    }

    #[test]
    fn turn_rate_scales_with_period() {
        let mut tf = Transform::from_rotation(Quat::from_rotation_y(0.0));
        // 0.6 rad per 0.03s over 0.03s → up to 0.6 rad of turn.
        let faced = turn_toward(&mut tf, Vec3::new(1.0, 0.0, 0.0), 0.6, 0.03);
        let (yaw, _, _) = tf.rotation.to_euler(EulerRot::YXZ);
        assert!(yaw.abs() > 0.5);
        // Still not within 11.5° of +X (π/2) after one period from +Z.
        assert!(!faced);
    }
}
