//! Ability consequences: turns [`AbilityTriggerEvent`]s into shared gameplay events.
//!
//! ```text
//! AbilityTriggerEvent(TriggerContext)
//!   → definition entries whose trigger matches
//!   → resolve EffectTarget against the context (caster, trigger unit, affected units, …)
//!   → AbilityEffect → DamageEvent / HealEvent / StatusEffectEvent (or a custom effect system)
//! ```

use std::collections::HashMap;

use bevy::ecs::system::{SystemId, SystemParam};
use bevy::prelude::*;

use super::casting::team_allows;
use super::catalog::AbilityDefinitions;
use super::definition::{
    AbilityEffect, AbilityTrigger, AreaCenter, AreaRadius, EffectScaling, EffectTarget,
};
use crate::combat::DamageEvent;
use crate::components::{AbilityId, BoundRadius, CombatStats, Health, Team};
use crate::dimensions::{area_contains, bounds_of};
use crate::items::{StatusEffect, StatusEffects, StatusKind, apply_status};

/// Who and where a trigger involved. Mechanics fill this in once so effects never
/// have to rediscover the entities.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerContext {
    pub trigger: AbilityTrigger,
    pub caster: Entity,
    pub team: Team,
    pub ability: AbilityId,
    pub rank: u32,
    /// Caster position when the trigger fired.
    pub origin: Vec3,
    /// The cast's aim point.
    pub aim: Vec3,
    /// Where the trigger happened.
    pub point: Vec3,
    /// The unit the ability was cast on.
    pub cast_target: Option<Entity>,
    /// The unit that caused the trigger (the cast target for `OnCast`).
    pub trigger_unit: Option<Entity>,
    pub affected_units: Vec<Entity>,
}

#[derive(Message, Debug, Clone, PartialEq)]
pub struct AbilityTriggerEvent(pub TriggerContext);

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

/// Input for an [`AbilityEffect::Custom`] system.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CustomEffectInput {
    pub ctx: TriggerContext,
    pub targets: Vec<Entity>,
    pub value: f32,
}

#[derive(Resource, Default)]
pub struct CustomAbilityEffects(HashMap<&'static str, SystemId<In<CustomEffectInput>>>);

#[allow(dead_code)]
pub trait AbilityEffectAppExt {
    /// Register the system behind `AbilityEffect::Custom { id, .. }`. It receives the
    /// resolved targets and should emit shared gameplay events rather than mutating
    /// `Health` / stats directly.
    fn register_custom_effect<M>(
        &mut self,
        id: &'static str,
        system: impl IntoSystem<In<CustomEffectInput>, (), M> + 'static,
    ) -> &mut Self;
}

impl AbilityEffectAppExt for App {
    fn register_custom_effect<M>(
        &mut self,
        id: &'static str,
        system: impl IntoSystem<In<CustomEffectInput>, (), M> + 'static,
    ) -> &mut Self {
        let system = self.world_mut().register_system(system);
        self.world_mut()
            .get_resource_or_init::<CustomAbilityEffects>()
            .0
            .insert(id, system);
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

/// Resolves effect targets and executes [`AbilityEffect`]s as gameplay events.
#[derive(SystemParam)]
pub struct EffectRunner<'w, 's> {
    commands: Commands<'w, 's>,
    custom: Res<'w, CustomAbilityEffects>,
    units: UnitQuery<'w, 's>,
    stats: Query<'w, 's, &'static CombatStats>,
    damage: MessageWriter<'w, DamageEvent>,
    heals: MessageWriter<'w, HealEvent>,
    statuses: MessageWriter<'w, StatusEffectEvent>,
}

impl EffectRunner<'_, '_> {
    fn living(&self, entity: Entity) -> bool {
        self.units
            .get(entity)
            .is_ok_and(|(_, _, _, health, _)| health.is_alive())
    }

    pub fn targets(
        &self,
        ctx: &TriggerContext,
        target: EffectTarget,
        aoe_radius: f32,
    ) -> Vec<Entity> {
        match target {
            EffectTarget::Caster => vec![ctx.caster],
            EffectTarget::CastTarget => ctx
                .cast_target
                .filter(|&e| self.living(e))
                .into_iter()
                .collect(),
            EffectTarget::TriggerUnit => ctx
                .trigger_unit
                .filter(|&e| self.living(e))
                .into_iter()
                .collect(),
            EffectTarget::AffectedUnits => ctx
                .affected_units
                .iter()
                .copied()
                .filter(|&e| self.living(e))
                .collect(),
            EffectTarget::UnitsInRadius {
                center,
                radius,
                team,
            } => {
                let center = match center {
                    AreaCenter::Caster => ctx.origin,
                    AreaCenter::Aim => ctx.aim,
                    AreaCenter::TriggerPoint => ctx.point,
                };
                let radius = match radius {
                    AreaRadius::Ability => aoe_radius,
                    AreaRadius::Fixed(value) => value.at(ctx.rank),
                };
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

    /// `None` when the scaling rules the target out.
    fn scaled_amount(
        &self,
        ctx: &TriggerContext,
        base: f32,
        scaling: EffectScaling,
        target: Entity,
    ) -> Option<f32> {
        match scaling {
            EffectScaling::None => Some(base),
            EffectScaling::CasterAttackDamage(ratio) => {
                let attack = self.stats.get(ctx.caster).map_or(0.0, |s| s.attack_damage);
                Some(base + ratio * attack)
            }
            EffectScaling::TargetHealthBelow(threshold) => self
                .units
                .get(target)
                .ok()
                .filter(|(_, _, _, hp, _)| hp.current / hp.max.max(1.0) < threshold)
                .map(|_| base),
        }
    }

    pub fn execute(&mut self, ctx: &TriggerContext, effect: &AbilityEffect, targets: Vec<Entity>) {
        let source = Some(ctx.caster);
        match *effect {
            AbilityEffect::Damage {
                amount,
                damage_type,
                scaling,
            } => {
                for target in targets {
                    if let Some(amount) =
                        self.scaled_amount(ctx, amount.at(ctx.rank), scaling, target)
                    {
                        self.damage.write(DamageEvent {
                            source,
                            target,
                            amount,
                            damage_type,
                        });
                    }
                }
            }
            AbilityEffect::Heal { amount } => {
                for target in targets {
                    self.heals.write(HealEvent {
                        source,
                        target,
                        amount: amount.at(ctx.rank),
                    });
                }
            }
            AbilityEffect::ApplyStatus { status, duration } => {
                for target in targets {
                    let move_speed = self.stats.get(target).map_or(0.0, |s| s.move_speed);
                    self.statuses.write(StatusEffectEvent {
                        source,
                        target,
                        change: StatusChange::Apply(status.build(
                            ctx.rank,
                            duration.at(ctx.rank),
                            move_speed,
                        )),
                    });
                }
            }
            AbilityEffect::Dispel { kind } => {
                for target in targets {
                    self.statuses.write(StatusEffectEvent {
                        source,
                        target,
                        change: StatusChange::Dispel(kind),
                    });
                }
            }
            AbilityEffect::Custom { id, value } => {
                let Some(&system) = self.custom.0.get(id) else {
                    warn!(
                        "ability {:?} uses unregistered custom effect {id:?}",
                        ctx.ability
                    );
                    return;
                };
                self.commands.run_system_with(
                    system,
                    CustomEffectInput {
                        ctx: ctx.clone(),
                        targets,
                        value: value.at(ctx.rank),
                    },
                );
            }
        }
    }
}

/// Runs every effect entry whose trigger matches, against the trigger's context.
/// Nothing here depends on which ability fired.
pub fn resolve_ability_triggers(
    mut triggers: MessageReader<AbilityTriggerEvent>,
    defs: Res<AbilityDefinitions>,
    mut runner: EffectRunner,
) {
    for AbilityTriggerEvent(ctx) in triggers.read() {
        let Some(def) = defs.get(ctx.ability) else {
            continue;
        };
        let aoe_radius = def.aoe_radius.at(ctx.rank);
        for entry in def.effects_for(ctx.trigger) {
            let targets = runner.targets(ctx, entry.target, aoe_radius);
            runner.execute(ctx, &entry.effect, targets);
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
