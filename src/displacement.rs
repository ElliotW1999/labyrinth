//! Forced movement (knockback / pull) requested by effects through [`DisplacementEvent`].
//! The unit is rooted for the duration so its own movement doesn't fight the push.

use bevy::prelude::*;

use crate::abilities::definition::DisplacementKind;
use crate::abilities::effects::{StatusChange, StatusEffectEvent};
use crate::components::BoundRadius;
use crate::dimensions::bounds_of;
use crate::items::{StatusEffect, StatusEffects};

#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct DisplacementEvent {
    pub target: Entity,
    pub kind: DisplacementKind,
    pub caster: Entity,
    /// Caster position when the trigger fired (fallback if the caster is gone).
    pub origin: Vec3,
    /// Where the trigger happened; knockbacks push away from it.
    pub point: Vec3,
    /// `None` pulls into contact with the caster.
    pub distance: Option<f32>,
    pub duration: f32,
}

#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct ForcedMovement {
    pub from: Vec3,
    pub to: Vec3,
    pub elapsed: f32,
    pub duration: f32,
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

pub fn apply_displacement_events(
    mut commands: Commands,
    mut events: MessageReader<DisplacementEvent>,
    mut statuses: MessageWriter<StatusEffectEvent>,
    units: Query<(&Transform, Option<&BoundRadius>, Option<&StatusEffects>)>,
) {
    for event in events.read() {
        let Ok((target_tf, target_bound, target_statuses)) = units.get(event.target) else {
            continue;
        };
        if target_statuses.is_some_and(|s| s.has_debuff_immunity()) {
            continue;
        }
        let from = target_tf.translation;
        let travel = match event.kind {
            DisplacementKind::Knockback => {
                let mut away = flat(from - event.point);
                if away.length() < 1.0 {
                    away = flat(from - event.origin);
                }
                away.normalize_or_zero() * event.distance.unwrap_or(0.0)
            }
            DisplacementKind::Pull => {
                let (caster_pos, caster_bound) = units
                    .get(event.caster)
                    .map_or((event.origin, None), |(tf, bound, _)| (tf.translation, bound));
                let toward = flat(caster_pos - from);
                let contact = (toward.length() - bounds_of(caster_bound) - bounds_of(target_bound))
                    .max(0.0);
                let distance = event.distance.map_or(contact, |d| d.min(contact));
                toward.normalize_or_zero() * distance
            }
        };
        if travel.length() < 1e-3 {
            continue;
        }
        let duration = event.duration.max(0.05);
        commands.entity(event.target).insert(ForcedMovement {
            from,
            to: from + travel,
            elapsed: 0.0,
            duration,
        });
        statuses.write(StatusEffectEvent {
            source: Some(event.caster),
            target: event.target,
            change: StatusChange::Apply(StatusEffect {
                rooted: true,
                ..StatusEffect::debuff("displaced", duration)
            }),
        });
    }
}

pub fn tick_forced_movement(
    mut commands: Commands,
    time: Res<Time>,
    mut units: Query<(Entity, &mut Transform, &mut ForcedMovement)>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut forced) in &mut units {
        forced.elapsed += dt;
        let t = (forced.elapsed / forced.duration).min(1.0);
        let pos = forced.from.lerp(forced.to, t);
        transform.translation.x = pos.x;
        transform.translation.z = pos.z;
        if t >= 1.0 {
            commands.entity(entity).remove::<ForcedMovement>();
        }
    }
}
