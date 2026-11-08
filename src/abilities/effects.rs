//! Ability and item consequences: turns [`AbilityTriggerEvent`]s into shared gameplay events.
//!
//! ```text
//! AbilityTriggerEvent(TriggerContext)
//!   → the source's (ability or item) entries whose trigger matches
//!   → resolve EffectTarget against the context (caster, trigger unit, affected units, …)
//!   → AbilityEffect → DamageEvent / HealEvent / ManaRestoreEvent / StatusEffectEvent /
//!                     DisplacementEvent / TeleportEvent (or a custom effect system)
//! ```

use std::collections::HashMap;

use bevy::ecs::system::{SystemId, SystemParam};
use bevy::prelude::*;

use super::catalog::AbilityDefinitions;
use super::definition::{
    AbilityEffect, AbilityEffectEntry, AbilityTrigger, AreaCenter, AreaRadius, Attribute,
    EffectScaling, EffectTarget, StatusBuildContext,
};
use crate::actions::{ActionId, team_allows};
use crate::combat::DamageEvent;
use crate::components::{AbilityId, BoundRadius, CombatStats, Health, HeroAttributes, Mana, Team};
use crate::dimensions::{area_contains, bounds_of};
use crate::displacement::DisplacementEvent;
use crate::items::ItemId;
use crate::status::{StatusChange, StatusEffectEvent, StatusEffects, is_untargetable};
use crate::teleport::TeleportEvent;
use crate::vitals::{self, HealKind};

/// What owns the effect entries a trigger resolves against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectSource {
    Ability(AbilityId),
    Item(ItemId),
}

/// Who and where a trigger involved. Mechanics fill this in once so effects never
/// have to rediscover the entities.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerContext {
    pub trigger: AbilityTrigger,
    /// The casting hero or the item's owner.
    pub caster: Entity,
    pub team: Team,
    pub source: EffectSource,
    /// Ability rank (items use 1).
    pub rank: u32,
    /// Caster position when the trigger fired.
    pub origin: Vec3,
    /// The cast's aim point.
    pub aim: Vec3,
    /// Where the trigger happened.
    pub point: Vec3,
    /// The unit the ability was cast on.
    pub cast_target: Option<Entity>,
    /// The unit that caused the trigger (the cast target for `OnCast`, the attack
    /// target for `OnAttack` / `OnAttackHit`).
    pub trigger_unit: Option<Entity>,
    pub affected_units: Vec<Entity>,
}

impl TriggerContext {
    /// The one constructor for trigger contexts. `point` starts at `aim`; every optional
    /// part starts empty. Fill the rest with the chainable setters.
    pub fn new(
        trigger: AbilityTrigger,
        source: EffectSource,
        caster: Entity,
        team: Team,
        rank: u32,
        origin: Vec3,
        aim: Vec3,
    ) -> Self {
        Self {
            trigger,
            caster,
            team,
            source,
            rank,
            origin,
            aim,
            point: aim,
            cast_target: None,
            trigger_unit: None,
            affected_units: Vec::new(),
        }
    }

    /// Where the trigger happened, when that is not the aim point.
    pub fn at(mut self, point: Vec3) -> Self {
        self.point = point;
        self
    }

    pub fn cast_target(mut self, cast_target: Option<Entity>) -> Self {
        self.cast_target = cast_target;
        self
    }

    pub fn trigger_unit(mut self, trigger_unit: Option<Entity>) -> Self {
        self.trigger_unit = trigger_unit;
        self
    }

    pub fn affected(mut self, affected_units: Vec<Entity>) -> Self {
        self.affected_units = affected_units;
        self
    }
}

#[derive(Message, Debug, Clone, PartialEq)]
pub struct AbilityTriggerEvent(pub TriggerContext);

#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct HealEvent {
    pub source: Option<Entity>,
    pub target: Entity,
    pub amount: f32,
}

#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ManaRestoreEvent {
    pub target: Entity,
    pub amount: f32,
}

/// Input for an [`AbilityEffect::Custom`] system.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CustomEffectInput {
    pub ctx: TriggerContext,
    pub targets: Vec<Entity>,
    pub value: f32,
    pub amount: f32,
    pub duration: f32,
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
    statuses_of: Query<'w, 's, &'static StatusEffects>,
    stats: Query<'w, 's, &'static CombatStats>,
    attributes: Query<'w, 's, &'static HeroAttributes>,
    damage: MessageWriter<'w, DamageEvent>,
    heals: MessageWriter<'w, HealEvent>,
    mana: MessageWriter<'w, ManaRestoreEvent>,
    statuses: MessageWriter<'w, StatusEffectEvent>,
    displacements: MessageWriter<'w, DisplacementEvent>,
    teleports: MessageWriter<'w, TeleportEvent>,
}

impl EffectRunner<'_, '_> {
    /// Alive, and not lifted out of play unless it is the caster itself.
    fn valid_target(&self, ctx: &TriggerContext, entity: Entity) -> bool {
        self.units
            .get(entity)
            .is_ok_and(|(_, _, _, health, _)| health.is_alive())
            && (entity == ctx.caster || !is_untargetable(&self.statuses_of, entity))
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
                .filter(|&e| self.valid_target(ctx, e))
                .into_iter()
                .collect(),
            EffectTarget::TriggerUnit => ctx
                .trigger_unit
                .filter(|&e| self.valid_target(ctx, e))
                .into_iter()
                .collect(),
            EffectTarget::AffectedUnits => ctx
                .affected_units
                .iter()
                .copied()
                .filter(|&e| self.valid_target(ctx, e))
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
                    .filter(|(e, tf, unit_team, health, bound)| {
                        health.is_alive()
                            && (*e == ctx.caster || !is_untargetable(&self.statuses_of, *e))
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
        cast_range: f32,
    ) -> Option<f32> {
        match scaling {
            EffectScaling::None => Some(base),
            EffectScaling::CasterAttackDamage(ratio) => {
                let attack = self.stats.get(ctx.caster).map_or(0.0, |s| s.attack_damage);
                Some(base + ratio * attack)
            }
            EffectScaling::CasterAttribute(attribute, ratio) => {
                let value = self
                    .attributes
                    .get(ctx.caster)
                    .map_or(0.0, |a| match attribute {
                        Attribute::Strength => a.strength,
                        Attribute::Agility => a.agility,
                        Attribute::Intelligence => a.intelligence,
                    });
                Some(base + ratio * value)
            }
            EffectScaling::TargetHealthBelow(threshold) => self
                .units
                .get(target)
                .ok()
                .filter(|(_, _, _, hp, _)| hp.current / hp.max.max(1.0) < threshold)
                .map(|_| base),
            EffectScaling::TargetMissingHealth(ratio) => self
                .units
                .get(target)
                .ok()
                .map(|(_, _, _, hp, _)| base + ratio * (hp.max - hp.current).max(0.0)),
            EffectScaling::TargetMaxHealth(ratio) => self
                .units
                .get(target)
                .ok()
                .map(|(_, _, _, hp, _)| base + ratio * hp.max),
            EffectScaling::TargetCurrentHealth(ratio) => self
                .units
                .get(target)
                .ok()
                .map(|(_, _, _, hp, _)| base + ratio * hp.current),
            EffectScaling::DistanceFromCaster { falloff } => {
                let target_pos = self
                    .units
                    .get(target)
                    .ok()
                    .map(|(_, tf, ..)| tf.translation)?;
                let dist = crate::combat::flat_distance(ctx.origin, target_pos);
                let max = cast_range.max(1.0);
                let t = (dist / max).clamp(0.0, 1.0);
                Some(base * (1.0 - falloff * t))
            }
        }
    }

    pub fn execute(
        &mut self,
        ctx: &TriggerContext,
        effect: &AbilityEffect,
        targets: Vec<Entity>,
        cast_range: f32,
    ) {
        let source = Some(ctx.caster);
        let rank = ctx.rank;
        match *effect {
            AbilityEffect::Damage {
                amount,
                damage_type,
                scaling,
            } => {
                for target in targets {
                    if let Some(amount) =
                        self.scaled_amount(ctx, amount.at(rank), scaling, target, cast_range)
                    {
                        self.damage.write(DamageEvent::spell(
                            ctx.caster,
                            target,
                            amount,
                            damage_type,
                        ));
                    }
                }
            }
            AbilityEffect::DrainHealth {
                amount,
                damage_type,
                scaling,
                transfer_ratio,
            } => {
                for target in targets {
                    if let Some(amount) =
                        self.scaled_amount(ctx, amount.at(rank), scaling, target, cast_range)
                    {
                        self.damage.write(
                            DamageEvent::spell(ctx.caster, target, amount, damage_type)
                                .drained(transfer_ratio),
                        );
                    }
                }
            }
            AbilityEffect::Heal { amount, scaling } => {
                for target in targets {
                    if let Some(amount) =
                        self.scaled_amount(ctx, amount.at(rank), scaling, target, cast_range)
                    {
                        self.heals.write(HealEvent {
                            source,
                            target,
                            amount,
                        });
                    }
                }
            }
            AbilityEffect::RestoreMana { amount } => {
                for target in targets {
                    self.mana.write(ManaRestoreEvent {
                        target,
                        amount: amount.at(rank),
                    });
                }
            }
            AbilityEffect::ApplyStatus { status, duration } => {
                for target in targets {
                    let move_speed = self.stats.get(target).map_or(0.0, |s| s.move_speed);
                    let agility = self
                        .attributes
                        .get(target)
                        .map(|a| a.agility)
                        .unwrap_or(0.0);
                    let fear_source = match status {
                        super::definition::StatusSpec::Fear => Some(ctx.caster),
                        _ => None,
                    };
                    let mut built = status.build(
                        rank,
                        duration.at(rank),
                        StatusBuildContext {
                            move_speed,
                            agility,
                            fear_source,
                            source,
                            point: ctx.point,
                        },
                    );
                    built.dot_source = source;
                    if built.heal_per_sec > 0.0 {
                        // The healer's amp is taken once, here; the tick applies none.
                        let healer_stats = source.and_then(|s| self.stats.get(s).ok());
                        let healer_statuses = source.and_then(|s| self.statuses_of.get(s).ok());
                        let amp = vitals::outgoing_heal_amp(healer_stats, healer_statuses);
                        built.heal_per_sec *= (1.0 + amp).max(0.0);
                    }
                    self.statuses.write(StatusEffectEvent {
                        source,
                        target,
                        change: StatusChange::Apply(built),
                    });
                }
            }
            AbilityEffect::Dispel { kind, strength } => {
                for target in targets {
                    self.statuses.write(StatusEffectEvent {
                        source,
                        target,
                        change: StatusChange::Dispel { kind, strength },
                    });
                }
            }
            AbilityEffect::RemoveStatus { id } => {
                for target in targets {
                    self.statuses.write(StatusEffectEvent {
                        source,
                        target,
                        change: StatusChange::Remove(id),
                    });
                }
            }
            AbilityEffect::Displace {
                kind,
                distance,
                duration,
                speed,
            } => {
                for target in targets {
                    self.displacements.write(DisplacementEvent::from_ctx(
                        ctx,
                        target,
                        kind,
                        distance.map(|d| d.at(rank)),
                        duration.at(rank),
                        speed.map(|s| s.at(rank)),
                    ));
                }
            }
            AbilityEffect::Teleport => {
                for target in targets {
                    self.teleports.write(TeleportEvent {
                        target,
                        caster: ctx.caster,
                        destination: ctx.aim,
                    });
                }
            }
            // Applied when the item is acquired, not when a trigger fires.
            AbilityEffect::PassiveStat { .. } => {}
            AbilityEffect::Custom {
                id,
                value,
                amount,
                duration,
            } => {
                let Some(&system) = self.custom.0.get(id) else {
                    warn!("{:?} uses unregistered custom effect {id:?}", ctx.source);
                    return;
                };
                self.commands.run_system_with(
                    system,
                    CustomEffectInput {
                        ctx: ctx.clone(),
                        targets,
                        value: value.at(rank),
                        amount: amount.at(rank),
                        duration: duration.at(rank),
                    },
                );
            }
        }
    }
}

impl From<ActionId> for EffectSource {
    fn from(action: ActionId) -> Self {
        match action {
            ActionId::Ability(id) => Self::Ability(id),
            ActionId::Item { item, .. } => Self::Item(item),
        }
    }
}

/// What a trigger needs from its source, whether an ability or an item.
pub struct SourceParams<'a> {
    pub entries: &'a [AbilityEffectEntry],
    /// Radius an `AreaRadius::Ability` area resolves to.
    pub aoe_radius: f32,
    /// Range that `EffectScaling::DistanceFromCaster` falls off over.
    pub cast_range: f32,
}

/// One lookup for the effect entries, area radius and cast range of any [`EffectSource`].
/// `None` when an ability has no registered definition.
pub fn source_params<'a>(
    source: EffectSource,
    defs: &'a AbilityDefinitions,
    rank: u32,
) -> Option<SourceParams<'a>> {
    match source {
        EffectSource::Ability(id) => defs.get(id).map(|def| SourceParams {
            entries: &def.effects,
            aoe_radius: def.aoe_radius.at(rank),
            cast_range: def.cast_range.at(rank),
        }),
        EffectSource::Item(id) => {
            let def = id.definition();
            Some(SourceParams {
                entries: def.effects,
                aoe_radius: def.aoe_radius(),
                cast_range: def.activation.cast_range,
            })
        }
    }
}

/// Runs every effect entry whose trigger matches, against the trigger's context.
/// Nothing here depends on which ability or item fired.
pub fn resolve_ability_triggers(
    mut triggers: MessageReader<AbilityTriggerEvent>,
    defs: Res<AbilityDefinitions>,
    mut runner: EffectRunner,
) {
    for AbilityTriggerEvent(ctx) in triggers.read() {
        let Some(params) = source_params(ctx.source, &defs, ctx.rank) else {
            continue;
        };
        for entry in params
            .entries
            .iter()
            .filter(|entry| entry.trigger == ctx.trigger)
        {
            let targets = runner.targets(ctx, entry.target, params.aoe_radius);
            runner.execute(ctx, &entry.effect, targets, params.cast_range);
        }
    }
}

pub fn apply_heal_events(
    mut heals: MessageReader<HealEvent>,
    healers: Query<(Option<&CombatStats>, Option<&StatusEffects>)>,
    mut targets: Query<(&mut Health, Option<&CombatStats>, Option<&StatusEffects>)>,
) {
    for heal in heals.read() {
        // The healer's amp is read before the target is borrowed (the healer may be the target).
        let outgoing = heal
            .source
            .and_then(|source| healers.get(source).ok())
            .map(|(stats, statuses)| vitals::outgoing_heal_amp(stats, statuses))
            .unwrap_or(0.0);
        if let Ok((mut health, stats, statuses)) = targets.get_mut(heal.target) {
            let factor = vitals::heal_factor(HealKind::Direct, outgoing, stats, statuses);
            vitals::apply_heal(&mut health, heal.amount, factor);
        }
    }
}

pub fn apply_mana_restore_events(
    mut events: MessageReader<ManaRestoreEvent>,
    mut targets: Query<&mut Mana>,
) {
    for event in events.read() {
        if let Ok(mut mana) = targets.get_mut(event.target) {
            mana.current = (mana.current + event.amount).min(mana.max);
        }
    }
}
