//! Item pickups: neutral rewards lying in a room. Walking over one equips it and drops the
//! previously equipped neutral item as a new pickup.

use bevy::prelude::*;

use super::StoryVisuals;
use crate::components::{
    CollisionRadius, CombatStats, Downed, Health, HeroAttributes, Mana, WorldTag,
};
use crate::dimensions::{collision_of, rect_touches_circle};
use crate::items::{Inventory, ItemInstance, equip_neutral};
use crate::net::NetworkedHero;

/// Half extent of a pickup's trigger square.
pub const PICKUP_RADIUS: f32 = 100.0;
/// Where a swapped-out item lands relative to the pickup that replaced it: clear of the
/// hero standing on it, so it never re-triggers.
const DROP_OFFSET: Vec3 = Vec3::new(260.0, 0.0, 0.0);

/// An item lying on the floor. It only triggers once `armed`; a pickup arms the first time
/// no hero overlaps it, so a freshly dropped item cannot be swapped straight back.
#[derive(Component, Debug, Clone, Copy)]
pub struct ItemPickup {
    pub instance: ItemInstance,
    pub radius: f32,
    pub armed: bool,
}

/// Spawn a pickup of `instance` at `position` (ground level).
pub fn spawn_item_pickup(
    commands: &mut Commands,
    visuals: &super::StoryHandles,
    position: Vec3,
    instance: ItemInstance,
    armed: bool,
) -> Entity {
    let name = instance.id.name();
    commands
        .spawn((
            Name::new(format!("Pickup: {name}")),
            Mesh3d(visuals.pickup_mesh.clone()),
            MeshMaterial3d(visuals.pickup.clone()),
            Transform::from_xyz(position.x, 60.0, position.z),
            ItemPickup {
                instance,
                radius: PICKUP_RADIUS,
                armed,
            },
            WorldTag(name.to_string()),
        ))
        .id()
}

/// Arm pickups once nobody stands on them; a living hero on an armed pickup equips it
/// (neutral items only) and the previous neutral item drops beside it.
#[allow(clippy::type_complexity)]
pub fn collect_item_pickups(
    mut commands: Commands,
    mut visuals: ResMut<StoryVisuals>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut pickups: Query<(Entity, &Transform, &mut ItemPickup)>,
    mut heroes: Query<
        (
            &Transform,
            Option<&CollisionRadius>,
            &mut Health,
            &mut Inventory,
            &mut Mana,
            &mut CombatStats,
            &mut HeroAttributes,
        ),
        (With<NetworkedHero>, Without<Downed>, Without<ItemPickup>),
    >,
) {
    if pickups.is_empty() {
        return;
    }
    let handles = visuals.ensure(&mut meshes, &mut materials).clone();
    for (entity, pickup_tf, mut pickup) in &mut pickups {
        let half = Vec2::splat(pickup.radius);
        let standing = heroes.iter().position(|(tf, radius, health, ..)| {
            health.is_alive()
                && rect_touches_circle(
                    pickup_tf.translation,
                    half,
                    tf.translation,
                    collision_of(radius),
                )
        });
        let Some(index) = standing else {
            pickup.armed = true;
            continue;
        };
        if !pickup.armed {
            continue;
        }
        let Some((_, _, mut health, mut inventory, mut mana, mut stats, mut attrs)) =
            heroes.iter_mut().nth(index)
        else {
            continue;
        };
        let Ok(previous) = equip_neutral(
            &mut inventory,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            pickup.instance,
        ) else {
            continue;
        };
        commands.entity(entity).despawn();
        if let Some(previous) = previous {
            spawn_item_pickup(
                &mut commands,
                &handles,
                Vec3::new(pickup_tf.translation.x, 0.0, pickup_tf.translation.z) + DROP_OFFSET,
                previous,
                false,
            );
        }
    }
}
