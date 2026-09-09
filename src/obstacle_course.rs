//! Obstacle-course hazards (screen-top / world −Z strip): Firebreather and Heartpiercer.

use bevy::prelude::*;

use crate::combat::{apply_damage, flat_distance};
use crate::components::{
    BoundRadius, CollisionRadius, CombatStats, Creep, DamageType, Health, Lifetime, PlayerHero,
};
use crate::fog::{FogFreeZones, FogRect};
use crate::net::NetworkedHero;
use crate::resources::SharedAssets;
use crate::scale;

pub struct ObstacleCoursePlugin;

impl Plugin for ObstacleCoursePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            spawn_obstacle_course.after(crate::resources::load_shared_assets),
        )
        .add_systems(
            Update,
            (
                tick_firebreathers,
                tick_heartpiercer_plates,
                fly_hazard_orbs,
                apply_hazard_hits,
            )
                .chain()
                .run_if(crate::net::is_sim_authority),
        );
    }
}

/// Periodic orb turret — fires straight ahead along its facing.
#[derive(Component, Debug, Clone, Copy)]
pub struct Firebreather {
    pub timer: f32,
    pub interval: f32,
    pub damage: f32,
    pub range: f32,
    pub speed: f32,
}

/// Orb turret that fires when its pressure plate is stepped on.
#[derive(Component, Debug, Clone, Copy)]
pub struct Heartpiercer {
    pub damage: f32,
    pub cooldown: f32,
    pub remaining: f32,
    pub speed: f32,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct PressurePlate {
    pub piercer: Entity,
    pub half_x: f32,
    pub half_z: f32,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct HazardOrb {
    pub damage: f32,
    pub radius: f32,
    pub velocity: Vec3,
}

/// Marker for the training strip ground.
#[derive(Component, Debug, Clone, Copy)]
pub struct ObstacleCourseGround;

// Screen-top from the default camera is world −Z.
pub const COURSE_MIN_X: f32 = scale::u(-28.0);
pub const COURSE_MAX_X: f32 = scale::u(28.0);
pub const COURSE_MIN_Z: f32 = scale::u(-68.0);
pub const COURSE_MAX_Z: f32 = scale::u(-52.0);

fn spawn_obstacle_course(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<SharedAssets>,
    mut fog_free: ResMut<FogFreeZones>,
) {
    fog_free.rects.push(FogRect {
        min_x: COURSE_MIN_X - scale::u(2.0),
        max_x: COURSE_MAX_X + scale::u(2.0),
        min_z: COURSE_MIN_Z - scale::u(2.0),
        max_z: COURSE_MAX_Z + scale::u(2.0),
    });

    let floor_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.35, 0.28, 0.22),
        perceptual_roughness: 0.9,
        ..default()
    });
    let fire_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.25, 0.1),
        emissive: LinearRgba::rgb(4.0, 0.8, 0.1),
        ..default()
    });
    let pierce_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.15, 0.55),
        emissive: LinearRgba::rgb(1.5, 0.2, 2.0),
        ..default()
    });
    let plate_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.7, 0.55, 0.2),
        perceptual_roughness: 0.6,
        ..default()
    });

    let mid_z = (COURSE_MIN_Z + COURSE_MAX_Z) * 0.5;
    let mid_x = (COURSE_MIN_X + COURSE_MAX_X) * 0.5;
    let size_x = COURSE_MAX_X - COURSE_MIN_X;
    let size_z = COURSE_MAX_Z - COURSE_MIN_Z;

    commands.spawn((
        Name::new("Obstacle Course Floor"),
        ObstacleCourseGround,
        Mesh3d(meshes.add(Cuboid::new(size_x, 0.12, size_z))),
        MeshMaterial3d(floor_mat),
        Transform::from_xyz(mid_x, scale::u(0.06), mid_z),
    ));

    // Firebreather — faces +X and only shoots along that facing.
    let mut fire_tf =
        Transform::from_xyz(scale::u(-22.0), scale::u(1.6), mid_z).with_scale(Vec3::new(0.7, 0.85, 0.7));
    fire_tf.look_to(Dir3::X, Vec3::Y);
    commands.spawn((
        Name::new("Firebreather"),
        Mesh3d(assets.tower_mesh.clone()),
        MeshMaterial3d(fire_mat),
        fire_tf,
        Firebreather {
            timer: 0.5,
            interval: 1.8,
            damage: 55.0,
            range: scale::u(30.0),
            speed: scale::u(22.0),
        },
        CollisionRadius(scale::body(0.7)),
        BoundRadius(scale::body(0.5)),
    ));

    let piercer = commands
        .spawn((
            Name::new("Heartpiercer"),
            Mesh3d(assets.tower_mesh.clone()),
            MeshMaterial3d(pierce_mat),
            Transform::from_xyz(scale::u(22.0), scale::u(1.6), mid_z).with_scale(Vec3::new(0.7, 0.85, 0.7)),
            Heartpiercer {
                damage: 80.0,
                cooldown: 1.2,
                remaining: 0.0,
                speed: scale::u(28.0),
            },
            CollisionRadius(scale::body(0.7)),
        BoundRadius(scale::body(0.5)),
        ))
        .id();

    commands.spawn((
        Name::new("Heartpiercer Plate"),
        Mesh3d(meshes.add(Cuboid::new(scale::u(4.0), scale::u(0.15), scale::u(4.0)))),
        MeshMaterial3d(plate_mat),
        Transform::from_xyz(scale::u(8.0), scale::u(0.12), mid_z),
        PressurePlate {
            piercer,
            half_x: scale::u(2.0),
            half_z: scale::u(2.0),
        },
    ));

    for (i, pos) in [
        Vec3::new(scale::u(-26.0), scale::body(3.2) * 0.5, COURSE_MIN_Z + scale::u(2.0)),
        Vec3::new(scale::u(26.0), scale::body(3.2) * 0.5, COURSE_MIN_Z + scale::u(2.0)),
        Vec3::new(scale::u(-26.0), scale::body(3.2) * 0.5, COURSE_MAX_Z - scale::u(2.0)),
        Vec3::new(scale::u(26.0), scale::body(3.2) * 0.5, COURSE_MAX_Z - scale::u(2.0)),
    ]
    .into_iter()
    .enumerate()
    {
        commands.spawn((
            Name::new(format!("Course Marker Tree {i}")),
            Mesh3d(assets.tree_mesh.clone()),
            MeshMaterial3d(assets.tree_mat.clone()),
            Transform::from_translation(pos),
            crate::components::Obstacle::aabb(
                scale::TREE_COLLISION_HALF,
                scale::TREE_COLLISION_HALF,
            ),
        ));
    }
}

fn tick_firebreathers(
    time: Res<Time>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut turrets: Query<(&Transform, &mut Firebreather)>,
) {
    let dt = time.delta_secs();
    for (tf, mut gun) in &mut turrets {
        gun.timer -= dt;
        if gun.timer > 0.0 {
            continue;
        }
        gun.timer = gun.interval;

        let forward = *tf.forward();
        let flat = Vec3::new(forward.x, 0.0, forward.z);
        let dir = if flat.length_squared() > 1e-4 {
            flat.normalize()
        } else {
            Vec3::X
        };
        let aim = tf.translation + dir * gun.range;
        spawn_hazard_orb(
            &mut commands,
            &assets,
            tf.translation,
            aim,
            gun.damage,
            gun.speed,
        );
    }
}

fn tick_heartpiercer_plates(
    time: Res<Time>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    plates: Query<(&Transform, &PressurePlate)>,
    mut piercers: Query<(&Transform, &mut Heartpiercer)>,
    walkers: Query<
        (&Transform, Option<&CollisionRadius>),
        Or<(With<PlayerHero>, With<NetworkedHero>, With<Creep>)>,
    >,
) {
    let dt = time.delta_secs();
    for (plate_tf, plate) in &plates {
        let Ok((piercer_tf, mut gun)) = piercers.get_mut(plate.piercer) else {
            continue;
        };
        gun.remaining = (gun.remaining - dt).max(0.0);
        if gun.remaining > 0.0 {
            continue;
        }

        let stepped = walkers.iter().any(|(walker, radius)| {
            let r = radius.map(|u| u.0).unwrap_or(scale::u(0.4));
            let dx = (walker.translation.x - plate_tf.translation.x).abs();
            let dz = (walker.translation.z - plate_tf.translation.z).abs();
            dx <= plate.half_x + r && dz <= plate.half_z + r
        });
        if !stepped {
            continue;
        }

        gun.remaining = gun.cooldown;
        spawn_hazard_orb(
            &mut commands,
            &assets,
            piercer_tf.translation,
            plate_tf.translation,
            gun.damage,
            gun.speed,
        );
    }
}

fn spawn_hazard_orb(
    commands: &mut Commands,
    assets: &SharedAssets,
    origin: Vec3,
    aim: Vec3,
    damage: f32,
    speed: f32,
) {
    let start = origin + Vec3::Y * scale::u(1.4);
    let flat = Vec3::new(aim.x - origin.x, 0.0, aim.z - origin.z);
    let dir = if flat.length_squared() > 1e-4 {
        flat.normalize()
    } else {
        Vec3::X
    };
    let mut transform = Transform::from_translation(start);
    if let Ok(d) = Dir3::new(dir) {
        transform.look_to(d, Vec3::Y);
    }

    commands.spawn((
        Name::new("Hazard Orb"),
        Mesh3d(assets.spell_bolt_mesh.clone()),
        MeshMaterial3d(assets.spell_bolt_mat.clone()),
        transform.with_scale(Vec3::splat(1.35)),
        HazardOrb {
            damage,
            radius: scale::u(0.85),
            velocity: dir * speed,
        },
        Lifetime(2.8),
    ));
}

fn fly_hazard_orbs(time: Res<Time>, mut orbs: Query<(&mut Transform, &HazardOrb)>) {
    let dt = time.delta_secs();
    for (mut tf, orb) in &mut orbs {
        tf.translation += orb.velocity * dt;
    }
}

fn apply_hazard_hits(
    mut commands: Commands,
    orbs: Query<(Entity, &Transform, &HazardOrb, &Lifetime)>,
    mut victims: Query<
        (Entity, &Transform, &mut Health, &CombatStats, Option<&CollisionRadius>),
        Or<(With<PlayerHero>, With<NetworkedHero>, With<Creep>)>,
    >,
) {
    for (orb_entity, orb_tf, orb, life) in &orbs {
        if life.0 <= 0.0 {
            commands.entity(orb_entity).despawn();
            continue;
        }
        let mut hit = false;
        for (_victim, victim_tf, mut health, stats, radius) in &mut victims {
            if !health.is_alive() {
                continue;
            }
            let r = radius.map(|u| u.0).unwrap_or(scale::u(0.5));
            if flat_distance(orb_tf.translation, victim_tf.translation) <= orb.radius + r {
                let amount = apply_damage(orb.damage, DamageType::Magical, stats);
                health.current -= amount;
                hit = true;
            }
        }
        if hit {
            commands.entity(orb_entity).despawn();
        }
    }
}
