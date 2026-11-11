//! Orbiting hitboxes (the Triggers layer).
//!
//! ```text
//! AbilityEffect::Orbit (EffectRunner)
//!   → SpawnOrbitEvent
//!   → spawn_requested_orbits → AbilityOrbit entity (+ one sphere per orbiter)
//!   → tick_orbits: the hitboxes circle the caster; each new contact
//!   → AbilityTriggerEvent(template + the unit touched)   — resolved by `resolve_ability_triggers`
//! ```
//!
//! An orbit only reports contacts; damage, slows and the rest are ordinary effect entries on
//! the custom trigger it fires, so abilities and items share one path. Every orbiter keeps its
//! own contact memory: a unit fires the trigger when it enters that orbiter's hitbox and fires
//! it again only after it left and re-entered. The orbit follows its caster and ends when the
//! caster dies, its entity is gone, or the duration is up (a dead caster ends it; it does not
//! pause).

use bevy::prelude::*;

use super::definition::TargetTeam;
use super::effects::{AbilityTriggerEvent, TriggerContext, unit_in_area};
use crate::components::{BoundRadius, Health, Team};
use crate::resources::SharedAssets;
use crate::status::{StatusEffects, is_untargetable};

/// Tolerance when comparing the running clock with the duration.
const SLACK: f32 = 1e-4;

/// Height of the orbiting spheres above the caster's position.
const VISUAL_HEIGHT: f32 = 60.0;

/// Asks for an orbit. Written by `EffectRunner` for each `AbilityEffect::Orbit` execution.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct SpawnOrbitEvent {
    /// The context every contact fires: the spawning context re-fired one hop deeper under
    /// the orbit's trigger, without a trigger unit, affected units or attack.
    pub template: TriggerContext,
    /// Number of orbiting hitboxes, spaced evenly around the circle.
    pub count: u32,
    /// Distance of each hitbox from the caster's centre.
    pub radius: f32,
    /// Radius of each hitbox.
    pub hit_radius: f32,
    /// Seconds per revolution.
    pub period: f32,
    /// Seconds the orbit lasts.
    pub duration: f32,
    /// Which teams (relative to the caster) the hitboxes touch.
    pub team: TargetTeam,
    /// Angle of the first hitbox in the XZ plane (`atan2(z, x)`), the caster's facing.
    pub start_angle: f32,
}

/// A live orbit. `elapsed` counts game time since the frame after it was spawned.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct AbilityOrbit {
    pub template: TriggerContext,
    pub count: u32,
    pub radius: f32,
    pub hit_radius: f32,
    pub period: f32,
    pub duration: f32,
    pub team: TargetTeam,
    pub start_angle: f32,
    pub elapsed: f32,
    /// Per orbiter: the units inside its hitbox at the last sub-step (sorted).
    pub touching: Vec<Vec<Entity>>,
    /// The sphere entities, one per orbiter (empty without presentation assets).
    pub visuals: Vec<Entity>,
}

impl AbilityOrbit {
    fn from_event(event: SpawnOrbitEvent) -> Self {
        Self {
            template: event.template,
            count: event.count,
            radius: event.radius,
            hit_radius: event.hit_radius,
            period: event.period,
            duration: event.duration,
            team: event.team,
            start_angle: event.start_angle,
            elapsed: 0.0,
            touching: vec![Vec::new(); event.count as usize],
            visuals: Vec::new(),
        }
    }

    /// Angular speed in radians per second (counter-clockwise in the XZ plane).
    pub fn angular_speed(&self) -> f32 {
        std::f32::consts::TAU / self.period
    }

    /// Offset from the caster of orbiter `index` after `time` seconds of orbiting.
    pub fn offset(&self, index: usize, time: f32) -> Vec3 {
        let angle = self.start_angle
            + self.angular_speed() * time
            + std::f32::consts::TAU * index as f32 / self.count.max(1) as f32;
        Vec3::new(angle.cos(), 0.0, angle.sin()) * self.radius
    }
}

pub fn spawn_requested_orbits(
    mut commands: Commands,
    assets: Option<Res<SharedAssets>>,
    mut events: MessageReader<SpawnOrbitEvent>,
) {
    for event in events.read() {
        let mut orbit = AbilityOrbit::from_event(event.clone());
        let origin = orbit.template.origin;
        let mut entity = commands.spawn((
            Name::new("Ability Orbit"),
            Transform::from_translation(origin),
            Visibility::default(),
        ));
        if let Some(assets) = assets.as_deref() {
            let visual_scale = orbit.hit_radius / crate::scale::body(0.35);
            let mut visuals = Vec::new();
            entity.with_children(|parent| {
                for index in 0..orbit.count as usize {
                    let local = orbit.offset(index, 0.0) + Vec3::Y * VISUAL_HEIGHT;
                    visuals.push(
                        parent
                            .spawn((
                                Name::new("Orbiter"),
                                Mesh3d(assets.spell_bolt_mesh.clone()),
                                MeshMaterial3d(assets.spell_bolt_mat.clone()),
                                Transform::from_translation(local)
                                    .with_scale(Vec3::splat(visual_scale)),
                            ))
                            .id(),
                    );
                }
            });
            orbit.visuals = visuals;
        }
        entity.insert(orbit);
    }
}

/// Advance every orbit: move the hitboxes in sub-steps small enough that none can skip over
/// a unit, report new contacts, then drop orbits that ran out or lost their caster.
pub fn tick_orbits(
    time: Res<Time>,
    mut commands: Commands,
    mut orbits: Query<(Entity, &mut AbilityOrbit, &mut Transform)>,
    existing: Query<()>,
    units: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>), Without<AbilityOrbit>>,
    statuses: Query<&StatusEffects>,
    mut visuals: Query<&mut Transform, (Without<AbilityOrbit>, Without<Team>)>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    let dt = time.delta_secs();
    for (orbit_entity, mut orbit, mut orbit_tf) in &mut orbits {
        let caster = orbit.template.caster;
        if existing.get(caster).is_err() {
            commands.entity(orbit_entity).despawn();
            continue;
        }
        // A dead caster ends the orbit (it is not paused).
        let Some(center) = units
            .get(caster)
            .ok()
            .filter(|(_, _, _, health, _)| health.is_alive())
            .map(|(_, tf, ..)| tf.translation)
        else {
            commands.entity(orbit_entity).despawn();
            continue;
        };
        orbit_tf.translation = center;
        orbit.template.origin = center;

        let step_dt = dt.min((orbit.duration - orbit.elapsed).max(0.0));
        if step_dt > 0.0 {
            // Arc travelled by a hitbox this frame, split so each sub-step moves at most one
            // hit radius.
            let arc = orbit.angular_speed() * step_dt * orbit.radius;
            let steps = ((arc / orbit.hit_radius).ceil() as u32).max(1);
            // Candidates once per frame: everything but the caster that could be touched.
            let candidates: Vec<(Entity, Vec3, Team, &Health, Option<&BoundRadius>, bool)> = units
                .iter()
                .filter(|(entity, ..)| *entity != caster)
                .map(|(entity, tf, team, health, bound)| {
                    (
                        entity,
                        tf.translation,
                        *team,
                        health,
                        bound,
                        is_untargetable(&statuses, entity),
                    )
                })
                .collect();
            for step in 1..=steps {
                let at = orbit.elapsed + step_dt * step as f32 / steps as f32;
                for index in 0..orbit.count as usize {
                    let position = center + orbit.offset(index, at);
                    let mut now: Vec<Entity> = candidates
                        .iter()
                        .filter(|(_, pos, team, health, bound, untargetable)| {
                            unit_in_area(
                                orbit.team,
                                orbit.template.team,
                                *team,
                                health,
                                *bound,
                                *pos,
                                *untargetable,
                                position,
                                orbit.hit_radius,
                            )
                        })
                        .map(|(entity, ..)| *entity)
                        .collect();
                    now.sort();
                    for unit in &now {
                        if !orbit.touching[index].contains(unit) {
                            triggers.write(AbilityTriggerEvent(
                                orbit
                                    .template
                                    .clone()
                                    .trigger_unit(Some(*unit))
                                    .at(position),
                            ));
                        }
                    }
                    // A unit re-arms only after it leaves this orbiter's hitbox.
                    orbit.touching[index] = now;
                }
            }
            orbit.elapsed += step_dt;
        }

        for (index, visual) in orbit.visuals.iter().enumerate() {
            if let Ok(mut tf) = visuals.get_mut(*visual) {
                tf.translation = orbit.offset(index, orbit.elapsed) + Vec3::Y * VISUAL_HEIGHT;
            }
        }
        if orbit.elapsed + SLACK >= orbit.duration {
            commands.entity(orbit_entity).despawn();
        }
    }
}
