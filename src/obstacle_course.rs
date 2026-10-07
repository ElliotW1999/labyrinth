//! Obstacle-course hazards beside the Radiant ancient: Firebreather and Heartpiercer.

use bevy::prelude::*;

use crate::combat::apply_damage;
use crate::components::{
    BoundRadius, CollisionRadius, CombatStats, Creep, DamageType, Health, Lifetime, PlayerHero,
};
use crate::dimensions::{area_contains, bounds_of, collision_of};
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

/// Original strip extents, in legacy layout units. The strip is recentered on the
/// Radiant ancient (it used to sit on the pre-rescale map's north edge).
const LEGACY_MIN_X: f32 = -28.0;
const LEGACY_MAX_X: f32 = 28.0;
const LEGACY_MIN_Z: f32 = -68.0;
const LEGACY_MAX_Z: f32 = -52.0;

/// World XZ of a point in the course's legacy layout. The near edge (`LEGACY_MAX_Z`)
/// sits just south of the Radiant ancient.
pub fn course_world(legacy_x: f32, legacy_z: f32) -> Vec2 {
    let ancient = scale::ground(-48.0, -48.0);
    let x = ancient.x + scale::u(legacy_x);
    let south_of_near_edge = scale::u(LEGACY_MAX_Z - legacy_z);
    let z = ancient.z - scale::u(20.0) - south_of_near_edge;
    Vec2::new(x, z)
}

pub fn course_bounds() -> (Vec2, Vec2) {
    let a = course_world(LEGACY_MIN_X, LEGACY_MIN_Z);
    let b = course_world(LEGACY_MAX_X, LEGACY_MAX_Z);
    (a.min(b), a.max(b))
}

fn spawn_obstacle_course(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<SharedAssets>,
    mut fog_free: ResMut<FogFreeZones>,
) {
    let (min_xz, max_xz) = course_bounds();
    let pad = scale::u(2.0);
    fog_free.rects.push(FogRect {
        min_x: min_xz.x - pad,
        max_x: max_xz.x + pad,
        min_z: min_xz.y - pad,
        max_z: max_xz.y + pad,
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

    let mid = (min_xz + max_xz) * 0.5;
    let size = max_xz - min_xz;
    let size_x = size.x;
    let size_z = size.y;
    let legacy_mid_z = (LEGACY_MIN_Z + LEGACY_MAX_Z) * 0.5;

    commands.spawn((
        Name::new("Obstacle Course Floor"),
        ObstacleCourseGround,
        Mesh3d(meshes.add(Cuboid::new(size_x, 0.12, size_z))),
        MeshMaterial3d(floor_mat),
        Transform::from_xyz(mid.x, scale::u(0.06), mid.y),
    ));

    // Firebreather — faces +X and only shoots along that facing.
    let fire_at = course_world(-22.0, legacy_mid_z);
    let mut fire_tf = Transform::from_xyz(fire_at.x, scale::u(1.6), fire_at.y)
        .with_scale(Vec3::new(0.7, 0.85, 0.7));
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

    let pierce_at = course_world(22.0, legacy_mid_z);
    let piercer = commands
        .spawn((
            Name::new("Heartpiercer"),
            Mesh3d(assets.tower_mesh.clone()),
            MeshMaterial3d(pierce_mat),
            Transform::from_xyz(pierce_at.x, scale::u(1.6), pierce_at.y)
                .with_scale(Vec3::new(0.7, 0.85, 0.7)),
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

    let plate_at = course_world(8.0, legacy_mid_z);
    commands.spawn((
        Name::new("Heartpiercer Plate"),
        Mesh3d(meshes.add(Cuboid::new(scale::u(4.0), scale::u(0.15), scale::u(4.0)))),
        MeshMaterial3d(plate_mat),
        Transform::from_xyz(plate_at.x, scale::u(0.12), plate_at.y),
        PressurePlate {
            piercer,
            half_x: scale::u(2.0),
            half_z: scale::u(2.0),
        },
    ));

    for (i, (legacy_x, legacy_z)) in [
        (-26.0, LEGACY_MIN_Z + 2.0),
        (26.0, LEGACY_MIN_Z + 2.0),
        (-26.0, LEGACY_MAX_Z - 2.0),
        (26.0, LEGACY_MAX_Z - 2.0),
    ]
    .into_iter()
    .enumerate()
    {
        let xz = course_world(legacy_x, legacy_z);
        let pos = Vec3::new(xz.x, scale::body(3.2) * 0.5, xz.y);
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
            let r = collision_of(radius);
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
        (Entity, &Transform, &mut Health, &CombatStats, Option<&BoundRadius>),
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
            if area_contains(orb_tf.translation, orb.radius, victim_tf.translation, bounds_of(radius)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn course_sits_just_south_of_the_radiant_ancient() {
        let ancient = scale::ground(-48.0, -48.0);
        let (min_xz, max_xz) = course_bounds();
        assert!(max_xz.y < ancient.z - 100.0, "near edge should be south of the ancient");
        assert!((min_xz.x + max_xz.x) * 0.5 - ancient.x < 1.0);
        assert!(ancient.z - max_xz.y < scale::map(8.0));
        assert!(max_xz.x - min_xz.x > 500.0);
    }
}
