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
use super::definition::{
    AbilityMechanic, AbilityTrigger, AreaCenter, AreaRadius, DisplacementKind, RingStyle,
};
use super::effects::{AbilityTriggerEvent, EffectSource, TriggerContext, unit_in_area};
use crate::actions::{ActionFiredEvent, ActionId};
use crate::combat::{ProjectileHitEvent, ProjectilePayload, SpawnProjectileEvent, flat_distance};
use crate::components::{AbilityId, AttackTarget, BoundRadius, Health, Team};
use crate::dimensions::area_contains;
use crate::displacement::DisplacementEvent;
use crate::resources::SharedAssets;
use crate::scale::DASH_SPEED;
use crate::status::{StatusEffects, is_untargetable};

/// A unit a chain may jump to: already filtered for team, life and targetability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainCandidate {
    pub entity: Entity,
    pub position: Vec3,
    /// Bounds radius added to the jump radius when testing reach.
    pub bounds: f32,
}

/// The units a chain visits after `start`, in order. Each jump goes to the nearest candidate
/// within `radius` of the previous unit that was not hit yet (ties are broken by `Entity`),
/// and stops early when none is in reach. `start` itself is never returned.
pub fn chain_hops(
    start: Entity,
    start_position: Vec3,
    candidates: &[ChainCandidate],
    jumps: u32,
    radius: f32,
) -> Vec<ChainCandidate> {
    let mut hit = vec![start];
    let mut origin = start_position;
    let mut hops = Vec::new();
    for _ in 0..jumps {
        let next = candidates
            .iter()
            .filter(|c| {
                !hit.contains(&c.entity) && area_contains(origin, radius, c.position, c.bounds)
            })
            .min_by(|a, b| {
                flat_distance(origin, a.position)
                    .total_cmp(&flat_distance(origin, b.position))
                    .then(a.entity.cmp(&b.entity))
            })
            .copied();
        let Some(next) = next else {
            break;
        };
        hit.push(next.entity);
        origin = next.position;
        hops.push(next);
    }
    hops
}

/// Everything a mechanic or custom behavior needs to know about one cast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastContext {
    pub caster: Entity,
    pub team: Team,
    /// What owns the effect entries this cast resolves against: the casting ability today, an
    /// item once items run mechanics (see [`Self::ability`]).
    pub source: EffectSource,
    pub rank: u32,
    pub origin: Vec3,
    pub aim: Vec3,
    /// Living unit target, if the ability was cast on one.
    pub unit_target: Option<Entity>,
    pub aoe_radius: f32,
    pub cast_range: f32,
}

impl CastContext {
    /// The casting ability, or `None` when an item is the source.
    #[allow(dead_code)]
    pub fn ability(&self) -> Option<AbilityId> {
        match self.source {
            EffectSource::Ability(id) => Some(id),
            EffectSource::Item(_) => None,
        }
    }

    /// A trigger fired by this cast; mechanics fill in who was involved.
    pub fn trigger(&self, trigger: AbilityTrigger, point: Vec3) -> TriggerContext {
        TriggerContext::new(
            trigger,
            self.source,
            self.caster,
            self.team,
            self.rank,
            self.origin,
            self.aim,
        )
        .at(point)
        .cast_target(self.unit_target)
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
    area_units: Query<
        'w,
        's,
        (
            Entity,
            &'static Transform,
            &'static Team,
            &'static Health,
            Option<&'static BoundRadius>,
        ),
    >,
    statuses_of: Query<'w, 's, &'static StatusEffects>,
    displacements: MessageWriter<'w, DisplacementEvent>,
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
                first_hit,
                end_radius,
                origin_offset,
            } => {
                let unit = ctx
                    .unit_target
                    .filter(|_| !skillshot)
                    .and_then(|e| self.units.get(e).ok().map(|(tf, _)| (e, tf.translation)));
                let dir = Vec3::new(ctx.aim.x - ctx.origin.x, 0.0, ctx.aim.z - ctx.origin.z)
                    .normalize_or(Vec3::X);
                // A skillshot may start in front of the caster; it flies the cast range from there.
                let start = match origin_offset {
                    Some(offset) if *skillshot => ctx.origin + dir * offset.at(ctx.rank).max(0.0),
                    _ => ctx.origin,
                };
                let target_pos = match unit {
                    Some((_, pos)) => pos,
                    None if *skillshot => start + dir * ctx.cast_range,
                    None => ctx.aim,
                };
                self.projectiles.write(SpawnProjectileEvent {
                    team: ctx.team,
                    origin: start,
                    target: unit.map(|(e, _)| e),
                    target_pos,
                    speed: *speed,
                    skillshot_width: skillshot.then_some(ctx.aoe_radius),
                    skillshot_end_width: end_radius
                        .filter(|_| *skillshot)
                        .map(|width| width.at(ctx.rank)),
                    skillshot_max_hits: (*skillshot && *first_hit).then_some(1),
                    splash_radius: if *skillshot {
                        0.0
                    } else {
                        Self::radius(ctx, *splash_radius)
                    },
                    payload: ProjectilePayload {
                        caster: ctx.caster,
                        source: ctx.source,
                        rank: ctx.rank,
                        cast_target: ctx.unit_target,
                        aim: ctx.aim,
                    },
                });
            }
            AbilityMechanic::ProjectileToEach {
                center,
                team,
                speed,
            } => self.projectile_to_each(ctx, *center, *team, *speed),
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

    /// One homing projectile at each valid unit inside the cast's area (never the caster).
    fn projectile_to_each(
        &mut self,
        ctx: &CastContext,
        center: AreaCenter,
        team: crate::actions::TargetTeam,
        speed: Option<f32>,
    ) {
        let center = Self::center(ctx, center);
        let mut victims: Vec<(Entity, Vec3)> = self
            .area_units
            .iter()
            .filter(|(entity, tf, unit_team, health, bound)| {
                *entity != ctx.caster
                    && unit_in_area(
                        team,
                        ctx.team,
                        **unit_team,
                        health,
                        *bound,
                        tf.translation,
                        is_untargetable(&self.statuses_of, *entity),
                        center,
                        ctx.aoe_radius,
                    )
            })
            .map(|(entity, tf, ..)| (entity, tf.translation))
            .collect();
        victims.sort_by_key(|(entity, _)| *entity);
        for (unit, position) in victims {
            self.projectiles.write(SpawnProjectileEvent {
                team: ctx.team,
                origin: ctx.origin,
                target: Some(unit),
                target_pos: position,
                speed,
                skillshot_width: None,
                skillshot_end_width: None,
                skillshot_max_hits: None,
                splash_radius: 0.0,
                payload: ProjectilePayload {
                    caster: ctx.caster,
                    source: ctx.source,
                    rank: ctx.rank,
                    cast_target: Some(unit),
                    aim: ctx.aim,
                },
            });
        }
    }

    /// The dash itself is forced movement (it phases through units and cannot be
    /// cancelled by new orders); the mechanic only adds the ghost trail.
    fn dash(&mut self, ctx: &CastContext, max_distance: f32) {
        let from = ctx.origin;
        let flat = Vec3::new(ctx.aim.x - from.x, 0.0, ctx.aim.z - from.z);
        let mut dest = Vec3::new(ctx.aim.x, from.y, ctx.aim.z);
        if flat.length() > max_distance && flat.length() > 1e-4 {
            dest = from + flat.normalize() * max_distance;
        } else if flat.length() < 1e-3 {
            // Aimed at the caster: the dash runs along the facing (see `plan_motion`).
            let facing = self
                .units
                .get(ctx.caster)
                .map_or(Vec3::NEG_Z, |(tf, _)| *tf.forward());
            dest = super::effects::forward_point(from, facing, max_distance);
        }
        super::spawn_dash_ghosts(&mut self.commands, &self.assets, from, dest);
        self.displacements.write(DisplacementEvent::from_ctx(
            &ctx.trigger(AbilityTrigger::OnCast, from),
            ctx.caster,
            DisplacementKind::Dash,
            Some(max_distance),
            max_distance / DASH_SPEED,
            None,
        ));
    }
}

/// Items run the same mechanics as abilities: when an item activation fires, its
/// `ItemDefinition::mechanics` start (a projectile, …) with the item as the effect source.
/// The `OnUse` trigger itself comes from `items::resolve_item_uses`.
pub fn execute_item_mechanics(
    mut fired: MessageReader<ActionFiredEvent>,
    casters: Query<(&Transform, &Team)>,
    mut runner: MechanicRunner,
) {
    for event in fired.read() {
        let ActionId::Item { item, .. } = event.action else {
            continue;
        };
        let def = item.definition();
        if def.mechanics.is_empty() {
            continue;
        }
        let Ok((transform, team)) = casters.get(event.caster) else {
            continue;
        };
        let ctx = CastContext {
            caster: event.caster,
            team: *team,
            source: EffectSource::Item(item),
            rank: 1,
            origin: transform.translation,
            aim: event.aim,
            unit_target: event.target.unit().and_then(|e| runner.living_unit(e)),
            aoe_radius: def.aoe_radius(),
            cast_range: def.activation.cast_range,
        };
        for mechanic in def.mechanics {
            runner.run(&ctx, mechanic);
        }
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
            source: EffectSource::Ability(cast.ability),
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
        let trigger = if hit.primary.is_some() {
            AbilityTrigger::OnProjectileHit
        } else {
            AbilityTrigger::OnImpact
        };
        let origin = casters
            .get(payload.caster)
            .map_or(hit.impact, |tf| tf.translation);
        triggers.write(AbilityTriggerEvent(
            TriggerContext::new(
                trigger,
                payload.source,
                payload.caster,
                hit.team,
                payload.rank,
                origin,
                payload.aim,
            )
            .at(hit.impact)
            .cast_target(payload.cast_target)
            .trigger_unit(hit.primary)
            .affected(hit.splashed.clone()),
        ));
    }
}
