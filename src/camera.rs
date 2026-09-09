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
            .add_systems(
                Update,
                (pan_camera_focus, position_camera, sync_camera_visible_x).chain(),
            );
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraRig {
    pub height: f32,
    pub back: f32,
    pub pan_speed: f32,
    pub edge_size: f32,
    /// World units of X visible through the focus plane.
    pub visible_x: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        // Height/back chosen so a ~16:9 view at 45° vfov is near VISIBLE_WORLD_X;
        // `sync_camera_visible_x` then tunes vertical FOV to the live aspect.
        Self {
            height: 2_200.0,
            back: 1_050.0,
            pan_speed: 1_400.0,
            edge_size: 28.0,
            visible_x: scale::VISIBLE_WORLD_X,
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
            position: scale::ground(-44.0, -44.0),
        }
    }
}

#[derive(Component)]
pub struct GameCamera;

fn spawn_camera(mut commands: Commands, rig: Res<CameraRig>, focus: Res<CameraFocus>) {
    let eye = focus.position + Vec3::new(0.0, rig.height, rig.back);
    let projection = PerspectiveProjection {
        near: 2.0,
        far: 80_000.0,
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

/// Keep horizontal coverage at the focus plane ≈ [`CameraRig::visible_x`].
fn sync_camera_visible_x(
    rig: Res<CameraRig>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut camera: Query<&mut Projection, With<GameCamera>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let aspect = (window.width() / window.height().max(1.0)).max(0.1);
    let slant = (rig.height * rig.height + rig.back * rig.back).sqrt().max(1.0);
    // W_x = 2 * R * tan(vfov/2) * aspect  →  vfov = 2 atan(W_x / (2 R aspect))
    let half = (rig.visible_x / (2.0 * slant * aspect)).atan();
    let vfov = (2.0 * half).clamp(0.15, 2.8);

    let Ok(mut projection) = camera.single_mut() else {
        return;
    };
    if let Projection::Perspective(persp) = projection.as_mut() {
        persp.fov = vfov;
        persp.far = 80_000.0;
        persp.near = 2.0;
    }
}
