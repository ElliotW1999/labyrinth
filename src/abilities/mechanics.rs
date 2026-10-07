//! Ability mechanics: what happens in the world when an ability fires, and which
//! triggers that produces. Mechanics decide *when* something happens and *who* is
//! involved; they never apply damage, healing, or statuses themselves (see `effects`).
//!
//! ```text
//! AbilityCastEvent ─┬─ definition mechanics (dash, projectile, …)
//!                   ├─ custom behavior (one-shot system) ── may emit Custom triggers
//!                   └─ AbilityTriggerEvent(OnCast)
//! ProjectileHitEvent → AbilityTriggerEvent(OnProjectileHit | OnImpact)
//! ```

use std::collections::HashMap;

use bevy::ecs::system::{SystemId, SystemParam};
use bevy::prelude::*;

use super::casting::AbilityCastEvent;
use super::catalog::AbilityDefinitions;
use super::definition::{AbilityMechanic, AbilityTrigger, AreaCenter, AreaRadius, RingStyle};
use super::effects::{
    AbilityTriggerEvent, EffectSource, StatusChange, StatusEffectEvent, TriggerContext,
};
use crate::combat::{ProjectileHitEvent, ProjectilePayload, SpawnProjectileEvent, flat_distance};
use crate::components::{
    AbilityId, AttackMoveOrder, AttackTarget, CombatStats, Health, MoveTarget, Team,
};
use crate::items::StatusEffect;
use crate::resources::SharedAssets;

/// Everything a mechanic or custom behavior needs to know about one cast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastContext {
    pub caster: Entity,
    pub team: Team,
    pub ability: AbilityId,
    pub rank: u32,
    pub origin: Vec3,
    pub aim: Vec3,
    /// Living unit target, if the ability was cast on one.
    pub unit_target: Option<Entity>,
    pub aoe_radius: f32,
    pub cast_range: f32,
}

impl CastContext {
    /// A trigger fired by this cast; mechanics fill in who was involved.
    pub fn trigger(&self, trigger: AbilityTrigger, point: Vec3) -> TriggerContext {
        TriggerContext {
            trigger,
            caster: self.caster,
            team: self.team,
            source: EffectSource::Ability(self.ability),
            rank: self.rank,
            origin: self.origin,
            aim: self.aim,
            point,
            cast_target: self.unit_target,
            trigger_unit: None,
            affected_units: Vec::new(),
        }
    }
}

/// Custom per-ability mechanics, run after the definition's common mechanics.
#[derive(Resource, Default)]
pub struct CustomAbilityBehaviors(HashMap<AbilityId, SystemId<In<CastContext>>>);

#[allow(dead_code)]
pub trait AbilityAppExt {
    /// Attach a custom mechanic to an ability. The system receives the
    /// [`CastContext`] of each successful cast. It should report what it encounters
    /// by writing [`AbilityTriggerEvent`]s (e.g. `AbilityTrigger::Custom("on_skewer_end")`)
    /// and leave consequences to the ability's effect entries.
    fn register_ability_behavior<M>(
        &mut self,
        ability: AbilityId,
        system: impl IntoSystem<In<CastContext>, (), M> + 'static,
    ) -> &mut Self;
}

impl AbilityAppExt for App {
    fn register_ability_behavior<M>(
        &mut self,
        ability: AbilityId,
        system: impl IntoSystem<In<CastContext>, (), M> + 'static,
    ) -> &mut Self {
        let id = self.world_mut().register_system(system);
        self.world_mut()
            .get_resource_or_init::<CustomAbilityBehaviors>()
            .0
            .insert(ability, id);
        self
    }
}

/// Runs [`AbilityMechanic`]s for one cast.
#[derive(SystemParam)]
pub struct MechanicRunner<'w, 's> {
    pub commands: Commands<'w, 's>,
    assets: Res<'w, SharedAssets>,
    units: Query<'w, 's, (&'static Transform, &'static Health)>,
    stats: Query<'w, 's, &'static CombatStats>,
    statuses: MessageWriter<'w, StatusEffectEvent>,
    projectiles: MessageWriter<'w, SpawnProjectileEvent>,
}

impl MechanicRunner<'_, '_> {
    pub fn living_unit(&self, entity: Entity) -> Option<Entity> {
        self.units
            .get(entity)
            .ok()
            .filter(|(_, health)| health.is_alive())
            .map(|_| entity)
    }

    fn center(ctx: &CastContext, center: AreaCenter) -> Vec3 {
        match center {
            AreaCenter::Caster => ctx.origin,
            AreaCenter::Aim | AreaCenter::TriggerPoint => ctx.aim,
        }
    }

    fn radius(ctx: &CastContext, radius: AreaRadius) -> f32 {
        match radius {
            AreaRadius::Ability => ctx.aoe_radius,
            AreaRadius::Fixed(value) => value.at(ctx.rank),
        }
    }

    pub fn run(&mut self, ctx: &CastContext, mechanic: &AbilityMechanic) {
        match mechanic {
            AbilityMechanic::Dash { distance } => self.dash(ctx, distance.at(ctx.rank)),
            AbilityMechanic::Projectile {
                splash_radius,
                speed,
                skillshot,
            } => {
                let unit = ctx
                    .unit_target
                    .filter(|_| !skillshot)
                    .and_then(|e| self.units.get(e).ok().map(|(tf, _)| (e, tf.translation)));
                let target_pos = match unit {
                    Some((_, pos)) => pos,
                    None if *skillshot => {
                        let dir = Vec3::new(ctx.aim.x - ctx.origin.x, 0.0, ctx.aim.z - ctx.origin.z)
                            .normalize_or(Vec3::X);
                        ctx.origin + dir * ctx.cast_range
                    }
                    None => ctx.aim,
                };
                self.projectiles.write(SpawnProjectileEvent {
                    team: ctx.team,
                    origin: ctx.origin,
                    target: unit.map(|(e, _)| e),
                    target_pos,
                    speed: *speed,
                    skillshot_width: skillshot.then_some(ctx.aoe_radius),
                    splash_radius: if *skillshot {
                        0.0
                    } else {
                        Self::radius(ctx, *splash_radius)
                    },
                    payload: ProjectilePayload {
                        caster: ctx.caster,
                        ability: ctx.ability,
                        rank: ctx.rank,
                        cast_target: ctx.unit_target,
                        aim: ctx.aim,
                    },
                });
            }
            AbilityMechanic::AttackUnitTarget => {
                if let Some(unit) = ctx.unit_target {
                    self.commands.entity(ctx.caster).insert(AttackTarget(unit));
                }
            }
            AbilityMechanic::RingFx {
                center,
                radius,
                style,
                lifetime,
            } => {
                let material = match style {
                    RingStyle::Shockwave => self.assets.shockwave_mat.clone(),
                    RingStyle::Nova => self.assets.nova_mat.clone(),
                };
                super::spawn_expanding_ring(
                    &mut self.commands,
                    &self.assets,
                    Self::center(ctx, *center),
                    Self::radius(ctx, *radius),
                    material,
                    *lifetime,
                );
            }
        }
    }

    /// The phase lasts exactly as long as the travel, so it is part of the mechanic
    /// rather than an effect entry.
    fn dash(&mut self, ctx: &CastContext, max_distance: f32) {
        let from = ctx.origin;
        let flat = Vec3::new(ctx.aim.x - from.x, 0.0, ctx.aim.z - from.z);
        let mut dest = Vec3::new(ctx.aim.x, from.y, ctx.aim.z);
        if flat.length() > max_distance && flat.length() > 1e-4 {
            dest = from + flat.normalize() * max_distance;
        }
        super::spawn_dash_ghosts(&mut self.commands, &self.assets, from, dest);
        let move_speed = self.stats.get(ctx.caster).map_or(1.0, |s| s.move_speed);
        let travel = flat_distance(from, dest) / move_speed.max(1.0);
        self.statuses.write(StatusEffectEvent {
            source: Some(ctx.caster),
            target: ctx.caster,
            change: StatusChange::Apply(StatusEffect::phased(travel + 0.15)),
        });
        self.commands
            .entity(ctx.caster)
            .insert(MoveTarget {
                position: Vec3::new(dest.x, 0.0, dest.z),
            })
            .remove::<(AttackTarget, AttackMoveOrder)>();
    }
}

pub fn execute_ability_casts(
    mut casts: MessageReader<AbilityCastEvent>,
    defs: Res<AbilityDefinitions>,
    behaviors: Res<CustomAbilityBehaviors>,
    casters: Query<(&Transform, &Team)>,
    mut runner: MechanicRunner,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for cast in casts.read() {
        let Ok((transform, team)) = casters.get(cast.caster) else {
            continue;
        };
        let Some(def) = defs.get(cast.ability) else {
            continue;
        };
        let ctx = CastContext {
            caster: cast.caster,
            team: *team,
            ability: cast.ability,
            rank: cast.rank,
            origin: transform.translation,
            aim: cast.aim,
            unit_target: cast.target.unit().and_then(|e| runner.living_unit(e)),
            aoe_radius: def.aoe_radius.at(cast.rank),
            cast_range: def.cast_range.at(cast.rank),
        };
        for mechanic in &def.mechanics {
            runner.run(&ctx, mechanic);
        }
        let mut on_cast = ctx.trigger(AbilityTrigger::OnCast, ctx.aim);
        on_cast.trigger_unit = ctx.unit_target;
        triggers.write(AbilityTriggerEvent(on_cast));
        if let Some(&behavior) = behaviors.0.get(&cast.ability) {
            runner.commands.run_system_with(behavior, ctx);
        }
    }
}

/// A projectile that struck its target fires `OnProjectileHit` (trigger unit = the
/// victim); one that ended elsewhere fires `OnImpact`. Splash victims are the
/// affected units either way.
pub fn projectile_hit_triggers(
    mut hits: MessageReader<ProjectileHitEvent>,
    casters: Query<&Transform>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for hit in hits.read() {
        let payload = hit.payload;
        triggers.write(AbilityTriggerEvent(TriggerContext {
            trigger: if hit.primary.is_some() {
                AbilityTrigger::OnProjectileHit
            } else {
                AbilityTrigger::OnImpact
            },
            caster: payload.caster,
            team: hit.team,
            source: EffectSource::Ability(payload.ability),
            rank: payload.rank,
            origin: casters
                .get(payload.caster)
                .map_or(hit.impact, |tf| tf.translation),
            aim: payload.aim,
            point: hit.impact,
            cast_target: payload.cast_target,
            trigger_unit: hit.primary,
            affected_units: hit.splashed.clone(),
        }));
    }
}
