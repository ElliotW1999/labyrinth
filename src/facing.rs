//! Shared yaw / turn-rate helpers.

use bevy::prelude::*;

/// Rotate `transform` toward a world-space direction on the XZ plane.
/// Returns true when the unit is facing within ~5°.
pub fn turn_toward(transform: &mut Transform, desired_dir: Vec3, turn_rate: f32, dt: f32) -> bool {
    let flat = Vec3::new(desired_dir.x, 0.0, desired_dir.z);
    if flat.length_squared() < 1e-6 || turn_rate <= 0.0 {
        return true;
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
    let max_step = turn_rate * dt;
    if delta.abs() <= max_step || delta.abs() < 0.08 {
        transform.rotation = Quat::from_rotation_y(desired_yaw);
        true
    } else {
        transform.rotation = Quat::from_rotation_y(current_yaw + delta.signum() * max_step);
        false
    }
}
