//! Ability execution: turns an [`AbilityCastEvent`] into gameplay events.
//!
//! ```text
//! AbilityCastEvent ─┬─ definition effects  → EffectRunner ─┐
//!                   └─ custom behavior (one-shot system) ──┴→ DamageEvent / HealEvent /
//!                                                             StatusEffectEvent / SpawnProjectileEvent
//! ProjectileHitEvent (with payload) → `on_hit` effects → same events
//! ```

use std::collections::HashMap;

use bevy::ecs::system::{SystemId, SystemParam};
use bevy::prelude::*;

use super::casting::{team_allows, AbilityCastEvent};
use super::catalog::AbilityDefinitions;
use super::definition::{AbilityEffect, AreaCenter, AreaRadius, EffectTargets, RingStyle};
use crate::combat::{
    flat_distance, DamageEvent, ProjectileHitEvent, ProjectilePayload, SpawnProjectileEvent,
};
use crate::components::{
    AbilityId, AttackMoveOrder, AttackTarget, BoundRadius, CombatStats, Health, MoveTarget, Team,
};
use crate::dimensions::{area_contains, bounds_of};
use crate::items::{apply_status, StatusEffect, StatusEffects, StatusKind};
use crate::resources::SharedAssets;

/// Everything an effect or custom behavior needs to know about one cast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastContext {
    pub caster: Entity,
    pub team: Team,
    pub ability: AbilityId,
    pub rank: u32,
    pub origin: Vec3,
    pub aim: Vec3,
    /// Living unit target (or the unit struck by a projectile for `on_hit`).
    pub unit_target: Option<Entity>,
    pub aoe_radius: f32,
}

#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct HealEvent {
    pub source: Option<Entity>,
    pub target: Entity,
    pub amount: f32,
}

#[derive(Debug, Clone)]
pub enum StatusChange {
    Apply(StatusEffect),
    Dispel(StatusKind),
}

/// Single entry point for adding/removing statuses so debuff immunity and stat
/// modifiers are handled consistently.
#[derive(Message, Debug, Clone)]
pub struct StatusEffectEvent {
    #[allow(dead_code)]
    pub source: Option<Entity>,
    pub target: Entity,
    pub change: StatusChange,
}

/// Custom per-ability logic, run after the definition's common effects.
#[derive(Resource, Default)]
pub struct CustomAbilityBehaviors(HashMap<AbilityId, SystemId<In<CastContext>>>);

pub trait AbilityAppExt {
    /// Attach custom execution logic to an ability. The system receives the
    /// [`CastContext`] of each successful cast and should emit the shared gameplay
    /// events (damage, statuses, projectiles) rather than mutating state directly.
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

type UnitQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Team,
        &'static Health,
        Option<&'static BoundRadius>,
    ),
>;

/// Resolves reusable [`AbilityEffect`]s into gameplay events.
#[derive(SystemParam)]
pub struct EffectRunner<'w, 's> {
    pub commands: Commands<'w, 's>,
    assets: Res<'w, SharedAssets>,
    units: UnitQuery<'w, 's>,
    stats: Query<'w, 's, &'static CombatStats>,
    damage: MessageWriter<'w, DamageEvent>,
    heals: MessageWriter<'w, HealEvent>,
    statuses: MessageWriter<'w, StatusEffectEvent>,
    projectiles: MessageWriter<'w, SpawnProjectileEvent>,
}

impl EffectRunner<'_, '_> {
    pub fn living_unit(&self, entity: Entity) -> Option<Entity> {
        self.units
            .get(entity)
            .ok()
            .filter(|(_, _, _, health, _)| health.is_alive())
            .map(|(e, ..)| e)
    }

    fn center(ctx: &CastContext, center: AreaCenter) -> Vec3 {
        match center {
            AreaCenter::Caster => ctx.origin,
            AreaCenter::Aim => ctx.aim,
        }
    }

    fn radius(ctx: &CastContext, radius: AreaRadius) -> f32 {
        match radius {
            AreaRadius::Ability => ctx.aoe_radius,
            AreaRadius::Fixed(value) => value.at(ctx.rank),
        }
    }

    fn targets(&self, ctx: &CastContext, targets: EffectTargets) -> Vec<Entity> {
        match targets {
            EffectTargets::Caster => vec![ctx.caster],
            EffectTargets::UnitTarget => ctx.unit_target.into_iter().collect(),
            EffectTargets::Area {
                center,
                radius,
                team,
            } => {
                let center = Self::center(ctx, center);
                let radius = Self::radius(ctx, radius);
                self.units
                    .iter()
                    .filter(|(_, tf, unit_team, health, bound)| {
                        health.is_alive()
                            && team_allows(team, ctx.team, **unit_team)
                            && area_contains(center, radius, tf.translation, bounds_of(*bound))
                    })
                    .map(|(e, ..)| e)
                    .collect()
            }
        }
    }

    /// `effect_index` identifies a top-level `SpawnProjectile` so its `on_hit`
    /// effects can be found on impact; pass `None` for nested effects.
    pub fn run(&mut self, ctx: &CastContext, effect: &AbilityEffect, effect_index: Option<usize>) {
        let source = Some(ctx.caster);
        match effect {
            AbilityEffect::Damage {
                amount,
                damage_type,
                targets,
            } => {
                for target in self.targets(ctx, *targets) {
                    self.damage.write(DamageEvent {
                        source,
                        target,
                        amount: amount.at(ctx.rank),
                        damage_type: *damage_type,
                    });
                }
            }
            AbilityEffect::Heal { amount, targets } => {
                for target in self.targets(ctx, *targets) {
                    self.heals.write(HealEvent {
                        source,
                        target,
                        amount: amount.at(ctx.rank),
                    });
                }
            }
            AbilityEffect::ApplyStatus {
                status,
                duration,
                targets,
            } => {
                let effect = status.build(ctx.rank, duration.at(ctx.rank));
                for target in self.targets(ctx, *targets) {
                    self.statuses.write(StatusEffectEvent {
                        source,
                        target,
                        change: StatusChange::Apply(effect.clone()),
                    });
                }
            }
            AbilityEffect::Dispel { kind, targets } => {
                for target in self.targets(ctx, *targets) {
                    self.statuses.write(StatusEffectEvent {
                        source,
                        target,
                        change: StatusChange::Dispel(*kind),
                    });
                }
            }
            AbilityEffect::Dash { distance } => self.dash(ctx, distance.at(ctx.rank)),
            AbilityEffect::SpawnProjectile {
                damage,
                damage_type,
                splash_radius,
                ..
            } => {
                let unit = ctx
                    .unit_target
                    .and_then(|e| self.units.get(e).ok().map(|(e, tf, ..)| (e, tf.translation)));
                self.projectiles.write(SpawnProjectileEvent {
                    team: ctx.team,
                    origin: ctx.origin,
                    target: unit.map(|(e, _)| e),
                    target_pos: unit.map_or(ctx.aim, |(_, pos)| pos),
                    damage: damage.at(ctx.rank),
                    damage_type: *damage_type,
                    splash_radius: Self::radius(ctx, *splash_radius),
                    payload: Some(ProjectilePayload {
                        caster: ctx.caster,
                        ability: ctx.ability,
                        rank: ctx.rank,
                        effect_index,
                    }),
                });
            }
            AbilityEffect::AttackUnitTarget => {
                if let Some(unit) = ctx.unit_target {
                    self.commands.entity(ctx.caster).insert(AttackTarget(unit));
                }
            }
            AbilityEffect::RingFx {
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
    mut runner: EffectRunner,
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
        };
        for (index, effect) in def.effects.iter().enumerate() {
            runner.run(&ctx, effect, Some(index));
        }
        if let Some(&behavior) = behaviors.0.get(&cast.ability) {
            runner.commands.run_system_with(behavior, ctx);
        }
    }
}

/// Primary projectile hits resolve the originating `SpawnProjectile` effect's `on_hit` list.
pub fn apply_projectile_on_hit_effects(
    mut hits: MessageReader<ProjectileHitEvent>,
    defs: Res<AbilityDefinitions>,
    casters: Query<&Transform>,
    mut runner: EffectRunner,
) {
    for hit in hits.read() {
        let Some(payload) = hit.payload.filter(|_| hit.primary) else {
            continue;
        };
        let Some(def) = defs.get(payload.ability) else {
            continue;
        };
        let Some(AbilityEffect::SpawnProjectile { on_hit, .. }) =
            payload.effect_index.and_then(|i| def.effects.get(i))
        else {
            continue;
        };
        let ctx = CastContext {
            caster: payload.caster,
            team: hit.team,
            ability: payload.ability,
            rank: payload.rank,
            origin: casters.get(payload.caster).map_or(hit.impact, |tf| tf.translation),
            aim: hit.impact,
            unit_target: runner.living_unit(hit.target),
            aoe_radius: def.aoe_radius.at(payload.rank),
        };
        for effect in on_hit {
            runner.run(&ctx, effect, None);
        }
    }
}

pub fn apply_heal_events(mut heals: MessageReader<HealEvent>, mut targets: Query<&mut Health>) {
    for heal in heals.read() {
        if let Ok(mut health) = targets.get_mut(heal.target) {
            health.current = (health.current + heal.amount).min(health.max);
        }
    }
}

fn remove_statuses(statuses: &mut StatusEffects, stats: &mut CombatStats, kind: StatusKind) {
    statuses.effects.retain(|effect| {
        if effect.kind != kind {
            return true;
        }
        stats.attack_damage -= effect.attack_damage;
        stats.armor -= effect.armor;
        stats.magic_resist -= effect.magic_resist;
        stats.move_speed -= effect.move_speed;
        false
    });
}

pub fn apply_status_effect_events(
    mut events: MessageReader<StatusEffectEvent>,
    mut units: Query<(&mut StatusEffects, &mut CombatStats)>,
) {
    for event in events.read() {
        let Ok((mut statuses, mut stats)) = units.get_mut(event.target) else {
            continue;
        };
        match &event.change {
            StatusChange::Apply(effect) => {
                if effect.debuff_immune {
                    remove_statuses(&mut statuses, &mut stats, StatusKind::Debuff);
                }
                apply_status(&mut statuses, &mut stats, effect.clone());
            }
            StatusChange::Dispel(kind) => remove_statuses(&mut statuses, &mut stats, *kind),
        }
    }
}
