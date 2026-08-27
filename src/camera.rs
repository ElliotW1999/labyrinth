//! Top-down chase camera with optional free pan.

use bevy::prelude::*;

use crate::components::PlayerHero;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraRig>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, follow_hero);
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct CameraRig {
    pub height: f32,
    pub lag: f32,
    pub offset: Vec3,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            height: 42.0,
            lag: 6.0,
            offset: Vec3::new(0.0, 0.0, 18.0),
        }
    }
}

#[derive(Component)]
pub struct GameCamera;

fn spawn_camera(mut commands: Commands, rig: Res<CameraRig>) {
    commands.spawn((
        Name::new("Game Camera"),
        Camera3d::default(),
        Transform::from_xyz(-44.0, rig.height, -26.0).looking_at(Vec3::new(-44.0, 0.0, -44.0), Vec3::Y),
        GameCamera,
    ));
}

fn follow_hero(
    time: Res<Time>,
    rig: Res<CameraRig>,
    hero: Query<&Transform, (With<PlayerHero>, Without<GameCamera>)>,
    mut camera: Query<&mut Transform, With<GameCamera>>,
) {
    let Ok(hero_tf) = hero.single() else {
        return;
    };
    let Ok(mut cam_tf) = camera.single_mut() else {
        return;
    };

    let focus = hero_tf.translation;
    let desired = focus + Vec3::new(rig.offset.x, rig.height, rig.offset.z);
    let t = 1.0 - (-rig.lag * time.delta_secs()).exp();
    cam_tf.translation = cam_tf.translation.lerp(desired, t);
    cam_tf.look_at(focus, Vec3::Y);
}
