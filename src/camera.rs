//! Top-down chase camera with true XZ plane panning.

use bevy::prelude::*;

use crate::components::PlayerHero;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraRig>()
            .init_resource::<CameraFocus>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (update_camera_focus, position_camera).chain());
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraRig {
    /// Height above the focus point.
    pub height: f32,
    /// How far "south" of the focus the camera sits (world +Z).
    pub back: f32,
    pub follow_lag: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            height: 42.0,
            back: 18.0,
            follow_lag: 8.0,
        }
    }
}

/// Ground-plane point the camera looks at. Panning moves this in XZ;
/// following lerps it toward the hero.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraFocus {
    pub position: Vec3,
    /// While true, focus tracks the hero. Arrow-key pan clears this.
    pub follow_hero: bool,
}

impl Default for CameraFocus {
    fn default() -> Self {
        Self {
            position: Vec3::new(-44.0, 0.0, -44.0),
            follow_hero: true,
        }
    }
}

#[derive(Component)]
pub struct GameCamera;

fn spawn_camera(mut commands: Commands, rig: Res<CameraRig>, focus: Res<CameraFocus>) {
    let eye = focus.position + Vec3::new(0.0, rig.height, rig.back);
    commands.spawn((
        Name::new("Game Camera"),
        Camera3d::default(),
        Transform::from_translation(eye).looking_at(focus.position, Vec3::Y),
        GameCamera,
    ));
}

fn update_camera_focus(
    time: Res<Time>,
    rig: Res<CameraRig>,
    keys: Res<ButtonInput<KeyCode>>,
    hero: Query<&Transform, With<PlayerHero>>,
    mut focus: ResMut<CameraFocus>,
) {
    // Arrow keys pan the focus on the XZ plane (not the camera eye).
    let mut pan = Vec3::ZERO;
    if keys.pressed(KeyCode::ArrowLeft) {
        pan.x -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        pan.x += 1.0;
    }
    if keys.pressed(KeyCode::ArrowUp) {
        pan.z -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        pan.z += 1.0;
    }

    if pan != Vec3::ZERO {
        focus.follow_hero = false;
        focus.position += pan.normalize() * 36.0 * time.delta_secs();
        focus.position.y = 0.0;
        return;
    }

    // F re-locks the camera onto the hero.
    if keys.just_pressed(KeyCode::KeyF) {
        focus.follow_hero = true;
    }

    if !focus.follow_hero {
        return;
    }

    let Ok(hero_tf) = hero.single() else {
        return;
    };
    let desired = Vec3::new(hero_tf.translation.x, 0.0, hero_tf.translation.z);
    let t = 1.0 - (-rig.follow_lag * time.delta_secs()).exp();
    focus.position = focus.position.lerp(desired, t);
}

fn position_camera(
    rig: Res<CameraRig>,
    focus: Res<CameraFocus>,
    mut camera: Query<&mut Transform, With<GameCamera>>,
) {
    let Ok(mut cam_tf) = camera.single_mut() else {
        return;
    };
    let eye = focus.position + Vec3::new(0.0, rig.height, rig.back);
    cam_tf.translation = eye;
    cam_tf.look_at(focus.position, Vec3::Y);
}
