//! Delayed, persistent area zones (the Triggers layer).
//!
//! ```text
//! AbilityEffect::Zone (EffectRunner)
//!   → SpawnZoneEvent
//!   → spawn_requested_zones → AbilityZone entity
//!   → tick_zones: after `delay`, every `interval`, `ticks` times
//!   → AbilityTriggerEvent(template + the units inside)   — resolved by `resolve_ability_triggers`
//! ```
//!
//! A zone only reports who stands inside it; damage, slows and the rest are ordinary effect
//! entries on the custom trigger it fires, so abilities and items share one path. A zone
//! outlives the death of its caster and is dropped only when the caster entity is gone.

use bevy::prelude::*;

use super::definition::TargetTeam;
use super::effects::{AbilityTriggerEvent, TriggerContext, unit_in_area};
use crate::components::{BoundRadius, Health, Team};
use crate::resources::SharedAssets;
use crate::status::{StatusEffects, is_untargetable};

/// Tolerance when comparing the running clock with a tick time, so a tick due exactly now is
/// not pushed a frame by float error.
const SLACK: f32 = 1e-4;

/// How long the activation ring is visible when the zone first ticks.
const RING_LIFETIME: f32 = 0.55;

/// Asks for a zone. Written by `EffectRunner` for each `AbilityEffect::Zone` execution.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct SpawnZoneEvent {
    /// The context every tick fires: the spawning context re-fired one hop deeper under the
    /// zone's trigger, without a trigger unit, affected units or attack.
    pub template: TriggerContext,
    pub center: Vec3,
    pub radius: f32,
    /// Which teams (relative to the caster) the zone reports.
    pub team: TargetTeam,
    /// Seconds before the first tick.
    pub delay: f32,
    /// Seconds the zone stays alive after `delay`.
    pub duration: f32,
    /// Seconds between ticks.
    pub interval: f32,
    /// How many ticks the zone fires in total.
    pub ticks: u32,
}

/// A live zone. `elapsed` counts game time since the frame after it was spawned.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct AbilityZone {
    pub template: TriggerContext,
    pub center: Vec3,
    pub radius: f32,
    pub team: TargetTeam,
    pub delay: f32,
    pub duration: f32,
    pub interval: f32,
    pub ticks: u32,
    pub elapsed: f32,
    /// Ticks fired so far; tick `k` (0-based) is due at `delay + k * interval`.
    pub fired: u32,
}

impl AbilityZone {
    fn from_event(event: SpawnZoneEvent) -> Self {
        Self {
            template: event.template,
            center: event.center,
            radius: event.radius,
            team: event.team,
            delay: event.delay,
            duration: event.duration,
            interval: event.interval,
            ticks: event.ticks,
            elapsed: 0.0,
            fired: 0,
        }
    }

    fn next_tick_due(&self) -> f32 {
        self.delay + self.fired as f32 * self.interval
    }
}

pub fn spawn_requested_zones(mut commands: Commands, mut events: MessageReader<SpawnZoneEvent>) {
    for event in events.read() {
        commands.spawn((
            Name::new("Ability Zone"),
            Transform::from_translation(event.center),
            AbilityZone::from_event(event.clone()),
        ));
    }
}

/// Advance every zone: fire the ticks that came due (overdue ones catch up in one frame),
/// then drop zones that ran out or lost their caster.
pub fn tick_zones(
    time: Res<Time>,
    mut commands: Commands,
    assets: Option<Res<SharedAssets>>,
    mut zones: Query<(Entity, &mut AbilityZone)>,
    existing: Query<()>,
    units: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    statuses: Query<&StatusEffects>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    let dt = time.delta_secs();
    for (zone_entity, mut zone) in &mut zones {
        let caster = zone.template.caster;
        if existing.get(caster).is_err() {
            commands.entity(zone_entity).despawn();
            continue;
        }
        zone.elapsed += dt;
        while zone.fired < zone.ticks && zone.elapsed + SLACK >= zone.next_tick_due() {
            if zone.fired == 0
                && let Some(assets) = assets.as_deref()
            {
                super::spawn_expanding_ring(
                    &mut commands,
                    assets,
                    zone.center,
                    zone.radius,
                    assets.nova_mat.clone(),
                    RING_LIFETIME,
                );
            }
            let mut inside: Vec<Entity> = units
                .iter()
                .filter(|(entity, tf, unit_team, health, bound)| {
                    *entity != caster
                        && unit_in_area(
                            zone.team,
                            zone.template.team,
                            **unit_team,
                            health,
                            *bound,
                            tf.translation,
                            is_untargetable(&statuses, *entity),
                            zone.center,
                            zone.radius,
                        )
                })
                .map(|(entity, ..)| entity)
                .collect();
            inside.sort();
            triggers.write(AbilityTriggerEvent(zone.template.clone().affected(inside)));
            zone.fired += 1;
        }
        if zone.elapsed + SLACK >= zone.delay + zone.duration {
            commands.entity(zone_entity).despawn();
        }
    }
}
