//! Free camera with arrow-key and screen-edge panning.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::components::PlayerHero;
use crate::scale;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraRig>()
            .init_resource::<CameraFocus>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (pan_camera_focus, position_camera).chain());
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraRig {
    pub height: f32,
    pub back: f32,
    pub pan_speed: f32,
    pub edge_size: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            height: scale::u(42.0),
            back: scale::u(18.0),
            pan_speed: scale::u(36.0),
            edge_size: 28.0, // screen pixels
        }
    }
}

/// Ground-plane point the camera looks at. Never auto-locks to the hero
/// (hero collision jitter near trees was shaking a follow cam).
#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraFocus {
    pub position: Vec3,
}

impl Default for CameraFocus {
    fn default() -> Self {
        Self {
            position: scale::v(-44.0, 0.0, -44.0),
        }
    }
}

#[derive(Component)]
pub struct GameCamera;

fn spawn_camera(mut commands: Commands, rig: Res<CameraRig>, focus: Res<CameraFocus>) {
    let eye = focus.position + Vec3::new(0.0, rig.height, rig.back);
    // Default Bevy far plane is 1000 — too short for the scaled MOBA world
    // (eye↔focus ≈ 1200+). Towers/trees near that boundary flicker as you pan.
    let projection = PerspectiveProjection {
        near: 2.0,
        far: 16_000.0,
        ..default()
    };
    commands.spawn((
        Name::new("Game Camera"),
        Camera3d::default(),
        Projection::from(projection),
        Transform::from_translation(eye).looking_at(focus.position, Vec3::Y),
        GameCamera,
    ));
}

fn pan_camera_focus(
    time: Res<Time>,
    rig: Res<CameraRig>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    hero: Query<&Transform, With<PlayerHero>>,
    mut focus: ResMut<CameraFocus>,
) {
    // F snaps once to the hero without enabling continuous follow.
    if keys.just_pressed(KeyCode::KeyF) {
        if let Ok(hero_tf) = hero.single() {
            focus.position = Vec3::new(hero_tf.translation.x, 0.0, hero_tf.translation.z);
        }
    }

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

    if let Ok(window) = windows.single() {
        if let Some(cursor) = window.cursor_position() {
            let w = window.width();
            let h = window.height();
            let edge = rig.edge_size;
            if cursor.x <= edge {
                pan.x -= 1.0;
            } else if cursor.x >= w - edge {
                pan.x += 1.0;
            }
            if cursor.y <= edge {
                pan.z -= 1.0;
            } else if cursor.y >= h - edge {
                pan.z += 1.0;
            }
        }
    }

    if pan != Vec3::ZERO {
        focus.position += pan.normalize() * rig.pan_speed * time.delta_secs();
        focus.position.y = 0.0;
    }
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
