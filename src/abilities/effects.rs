//! Ability and item consequences: turns [`AbilityTriggerEvent`]s into shared gameplay events.
//!
//! ```text
//! AbilityTriggerEvent(TriggerContext)
//!   → the source's (ability or item) entries whose trigger matches
//!   → resolve EffectTarget against the context (caster, trigger unit, affected units, …)
//!   → AbilityEffect → DamageEvent / HealEvent / ManaRestoreEvent / ResourceDrainEvent / StatusEffectEvent /
//!                     DisplacementEvent / TeleportEvent (or a custom effect system)
//! ```

use std::collections::{HashMap, VecDeque};

use bevy::ecs::system::{SystemId, SystemParam};
use bevy::prelude::*;

use super::catalog::AbilityDefinitions;
use super::definition::{
    AbilityEffect, AbilityEffectEntry, AbilityTrigger, AreaCenter, AreaRadius, Attribute,
    EffectScaling, EffectTarget, StatusBuildContext,
};
use super::mechanics::{ChainCandidate, chain_hops};
use crate::actions::{ActionId, team_allows};
use crate::basic_attack::ExtraAttackEvent;
use crate::combat::{CombatRng, DamageEvent};
use crate::components::{AbilityId, BoundRadius, CombatStats, Health, HeroAttributes, Mana, Team};
use crate::dimensions::{area_contains, bounds_of};
use crate::displacement::DisplacementEvent;
use crate::items::ItemId;
use crate::status::{
    StatusChange, StatusEffect, StatusEffectEvent, StatusEffects, StatusKind, is_untargetable,
};
use crate::teleport::TeleportEvent;
use crate::vitals::{self, HealKind, ResourceDrainEvent, ResourceKind};

/// What owns the effect entries a trigger resolves against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectSource {
    Ability(AbilityId),
    Item(ItemId),
}

/// Derived triggers (`Proc` / `Counter` / `Accumulate`) may chain at most this deep; the gates beyond it do
/// nothing. A trigger straight from a mechanic has depth 0.
pub const MAX_TRIGGER_DEPTH: u8 = 2;

/// One gate's state is kept per (source, `then` trigger).
type GateKey = (EffectSource, AbilityTrigger);

/// Per-unit state of the stateful gates, keyed by (source, `then` trigger).
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct TriggerCounters {
    /// How many times each `Counter` gate has run.
    pub counts: Vec<(GateKey, u32)>,
    /// Failures in a row of each pseudo-random `Proc` gate.
    pub prd: Vec<(GateKey, u32)>,
    /// Running sum of each `Accumulate` gate.
    pub sums: Vec<(GateKey, f32)>,
}

fn entry_of<T: Default>(list: &mut Vec<(GateKey, T)>, key: GateKey) -> &mut T {
    let index = match list.iter().position(|(k, _)| *k == key) {
        Some(index) => index,
        None => {
            list.push((key, T::default()));
            list.len() - 1
        }
    };
    &mut list[index].1
}

impl TriggerCounters {
    /// Count one execution; true (and the count restarts) on every `every`th.
    fn bump(&mut self, key: GateKey, every: u32) -> bool {
        let count = entry_of(&mut self.counts, key);
        *count += 1;
        if *count >= every.max(1) {
            *count = 0;
            true
        } else {
            false
        }
    }

    /// One pseudo-random roll with nominal rate `chance`: the effective chance grows with
    /// every failure in a row and the streak resets on success.
    fn roll_pseudo(&mut self, key: GateKey, chance: f32, rng: &mut CombatRng) -> bool {
        let failures = entry_of(&mut self.prd, key);
        let effective = (prd_constant(chance) * (*failures as f32 + 1.0)).min(1.0);
        let success = rng.roll(effective);
        *failures = if success {
            0
        } else {
            failures.saturating_add(1)
        };
        success
    }

    /// Add `amount` (negatives count as zero); true when the sum reached `threshold`, which
    /// is then reduced by it (the remainder carries over).
    fn accumulate(&mut self, key: GateKey, amount: f32, threshold: f32) -> bool {
        if threshold <= 0.0 {
            return false;
        }
        let sum = entry_of(&mut self.sums, key);
        *sum += amount.max(0.0);
        if *sum >= threshold {
            *sum %= threshold;
            true
        } else {
            false
        }
    }
}

/// The per-roll step `C` of a pseudo-random distribution with long-run rate `p`: the chance
/// on the `n`th roll since the last success is `min(1, C * n)`. Solved by bisection on the
/// expected number of rolls per success, `sum over n of prod over k < n of (1 - C k)`.
pub fn prd_constant(p: f32) -> f32 {
    let p = f64::from(p);
    if p <= 0.0 {
        return 0.0;
    }
    if p >= 1.0 {
        return 1.0;
    }
    let rate = |c: f64| {
        let (mut expected, mut survive, mut n) = (0.0, 1.0, 1.0);
        while survive > 0.0 {
            expected += survive;
            survive *= 1.0 - (c * n).min(1.0);
            n += 1.0;
        }
        1.0 / expected
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if rate(mid) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    ((lo + hi) / 2.0) as f32
}

/// What an attack-derived trigger knows about the attack that caused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackMeta {
    /// The attack was an extra attack (instant / double / multi-target).
    pub extra: bool,
    /// The main hit was a critical strike (`OnAttackHit` only).
    pub crit: bool,
}

/// Which status a status trigger (`OnStatusApplied` / `OnStatusRemoved`) is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusMeta {
    pub id: &'static str,
    pub kind: StatusKind,
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
    /// Set for triggers caused by a basic attack. Damage dealt under such a context is
    /// attack bonus damage rather than spell damage.
    pub attack: Option<AttackMeta>,
    /// How many `Proc` / `Counter` / `Chain` hops produced this context (0 = straight from a mechanic).
    pub depth: u8,
    /// Multiplier on the Damage, DrainHealth and Heal amounts of entries run under this
    /// context. 1.0 except for chain hops (falloff per jump); kept by derived contexts.
    pub magnitude: f32,
    /// The size of the occurrence: post-mitigation damage for damage triggers, landed damage
    /// for `OnAttackHit`, distance for `OnMove`, otherwise 1. Read by the `Accumulate` gate.
    pub amount: f32,
    /// Caused by damage or a status change. Effects resolved under a reaction emit
    /// `reactive` damage, drains and statuses, which never fire damage or status triggers
    /// again, and `PerformAttack` does nothing. Inherited by derived contexts.
    pub reaction: bool,
    /// The status a status trigger is about.
    pub status: Option<StatusMeta>,
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
            attack: None,
            depth: 0,
            magnitude: 1.0,
            amount: 1.0,
            reaction: trigger.is_reaction(),
            status: None,
        }
    }

    /// The same context re-fired under `trigger` one hop deeper (by `Proc` / `Counter` / `Chain`).
    fn derived(&self, trigger: AbilityTrigger) -> Self {
        Self {
            trigger,
            depth: self.depth + 1,
            ..self.clone()
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

    pub fn attack(mut self, attack: AttackMeta) -> Self {
        self.attack = Some(attack);
        self
    }

    pub fn amount(mut self, amount: f32) -> Self {
        self.amount = amount;
        self
    }

    pub fn reaction(mut self, reaction: bool) -> Self {
        self.reaction = reaction;
        self
    }

    pub fn status(mut self, status: StatusMeta) -> Self {
        self.status = Some(status);
        self
    }

    pub fn affected(mut self, affected_units: Vec<Entity>) -> Self {
        self.affected_units = affected_units;
        self
    }
}

#[derive(Message, Debug, Clone, PartialEq)]
pub struct AbilityTriggerEvent(pub TriggerContext);

/// A cast's effects resolved onto an enemy (one per cast context and distinct enemy).
/// Written by [`resolve_ability_triggers`]; the `OnSpellHit` dispatcher reads it next frame.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct SpellHitEvent {
    pub caster: Entity,
    pub team: Team,
    pub ability: AbilityId,
    pub target: Entity,
    /// Where the cast's trigger happened.
    pub point: Vec3,
}

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
    ///
    /// The input's `ctx.reaction` is true when the effect runs under a damage- or
    /// status-class trigger. A custom effect must pass it on (`DamageEvent::reactive`,
    /// `StatusEffectEvent::reactive`, `ResourceDrainEvent::reactive`) and must not start
    /// attacks under it, or it bypasses the recursion guard.
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
    drains: MessageWriter<'w, ResourceDrainEvent>,
    statuses: MessageWriter<'w, StatusEffectEvent>,
    displacements: MessageWriter<'w, DisplacementEvent>,
    teleports: MessageWriter<'w, TeleportEvent>,
    extra_attacks: MessageWriter<'w, ExtraAttackEvent>,
    rng: ResMut<'w, CombatRng>,
    counters: Query<'w, 's, &'static mut TriggerCounters>,
    /// Contexts produced by gates since the last [`Self::take_derived`].
    derived: Local<'s, Vec<TriggerContext>>,
}

impl EffectRunner<'_, '_> {
    /// Alive, and not lifted out of play unless it is the caster itself.
    fn valid_target(&self, ctx: &TriggerContext, entity: Entity) -> bool {
        self.units
            .get(entity)
            .is_ok_and(|(_, _, _, health, _)| health.is_alive())
            && (entity == ctx.caster || !is_untargetable(&self.statuses_of, entity))
    }

    /// Whether `entity` is a living unit of the team opposing the context's.
    fn is_enemy(&self, ctx: &TriggerContext, entity: Entity) -> bool {
        self.units
            .get(entity)
            .is_ok_and(|(_, _, team, health, _)| *team == ctx.team.enemy() && health.is_alive())
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

    /// Contexts that `Proc` / `Counter` gates fired since the last call, in order.
    fn take_derived(&mut self) -> Vec<TriggerContext> {
        std::mem::take(&mut *self.derived)
    }

    /// Whether a gate at `ctx` may fire a derived trigger (one warning at the cap).
    fn gate_open(&self, ctx: &TriggerContext) -> bool {
        if ctx.depth >= MAX_TRIGGER_DEPTH {
            warn!(
                "{:?} {:?}: derived trigger chain stopped at depth {}",
                ctx.source, ctx.trigger, ctx.depth
            );
            return false;
        }
        true
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
                        let amount = amount * ctx.magnitude;
                        self.damage.write(
                            DamageEvent::from_effect(
                                ctx.caster,
                                target,
                                amount,
                                damage_type,
                                ctx.attack.is_some(),
                            )
                            .reactive(ctx.reaction),
                        );
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
                        let amount = amount * ctx.magnitude;
                        self.damage.write(
                            DamageEvent::spell(ctx.caster, target, amount, damage_type)
                                .drained(transfer_ratio)
                                .reactive(ctx.reaction),
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
                            amount: amount * ctx.magnitude,
                        });
                    }
                }
            }
            AbilityEffect::DrainMana {
                amount,
                burn_ratio,
                damage_type,
                transfer_ratio,
            } => {
                for target in targets {
                    self.drains.write(ResourceDrainEvent {
                        source,
                        target,
                        resource: ResourceKind::Mana,
                        amount: amount.at(rank),
                        burn: (burn_ratio > 0.0).then_some((burn_ratio, damage_type)),
                        to_source: (transfer_ratio > 0.0).then_some(transfer_ratio),
                        convert_to: None,
                        from_attack: ctx.attack.is_some(),
                        reactive: ctx.reaction,
                    });
                }
            }
            AbilityEffect::SpendResource {
                resource,
                amount,
                convert_to,
            } => {
                for target in targets {
                    self.drains.write(ResourceDrainEvent {
                        source,
                        target,
                        resource,
                        amount: amount.at(rank),
                        burn: None,
                        to_source: None,
                        convert_to,
                        from_attack: ctx.attack.is_some(),
                        reactive: ctx.reaction,
                    });
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
                // The healer's amp is taken once, here; the tick applies none.
                let healer_amp = vitals::outgoing_heal_amp(
                    source.and_then(|s| self.stats.get(s).ok()),
                    source.and_then(|s| self.statuses_of.get(s).ok()),
                );
                for target in targets {
                    let mut built = build_status_effect(
                        status,
                        rank,
                        duration.at(rank),
                        source,
                        ctx.point,
                        self.stats.get(target).ok(),
                        self.attributes.get(target).ok(),
                        healer_amp,
                    );
                    built.reactive = ctx.reaction;
                    self.statuses.write(
                        StatusEffectEvent::new(source, target, StatusChange::Apply(built))
                            .reactive(ctx.reaction),
                    );
                }
            }
            AbilityEffect::Dispel { kind, strength } => {
                for target in targets {
                    self.statuses.write(
                        StatusEffectEvent::new(
                            source,
                            target,
                            StatusChange::Dispel { kind, strength },
                        )
                        .reactive(ctx.reaction),
                    );
                }
            }
            AbilityEffect::RemoveStatus { id } => {
                for target in targets {
                    self.statuses.write(
                        StatusEffectEvent::new(source, target, StatusChange::Remove(id))
                            .reactive(ctx.reaction),
                    );
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
            AbilityEffect::PerformAttack {
                damage_ratio,
                max_targets,
                exclude_trigger_unit,
            } => {
                // Recursion guards: an extra attack never causes another, and a reaction
                // (damage- or status-class trigger) never starts attacks.
                if ctx.reaction || ctx.attack.is_some_and(|attack| attack.extra) {
                    return;
                }
                let mut chosen: Vec<(f32, Entity)> = targets
                    .into_iter()
                    .filter(|&e| {
                        e != ctx.caster && !(exclude_trigger_unit && ctx.trigger_unit == Some(e))
                    })
                    .filter_map(|e| {
                        let (_, tf, team, health, _) = self.units.get(e).ok()?;
                        (*team == ctx.team.enemy() && health.is_alive())
                            .then(|| (crate::combat::flat_distance(ctx.origin, tf.translation), e))
                    })
                    .collect();
                chosen.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                if max_targets > 0 {
                    chosen.truncate(max_targets as usize);
                }
                for (_, target) in chosen {
                    self.extra_attacks.write(ExtraAttackEvent {
                        attacker: ctx.caster,
                        target,
                        damage_ratio: damage_ratio.at(rank),
                    });
                }
            }
            AbilityEffect::Proc {
                chance,
                then,
                pseudo,
            } => {
                if !self.gate_open(ctx) {
                    return;
                }
                let chance = chance.at(rank);
                let fired = if pseudo {
                    let key = (ctx.source, then);
                    match self.counters.get_mut(ctx.caster) {
                        Ok(mut counters) => counters.roll_pseudo(key, chance, &mut self.rng),
                        // No state to keep, so nothing to bias: a plain roll.
                        Err(_) => self.rng.roll(chance),
                    }
                } else {
                    self.rng.roll(chance)
                };
                if fired {
                    self.derived.push(ctx.derived(then));
                }
            }
            AbilityEffect::Counter { every, then } => {
                if !self.gate_open(ctx) {
                    return;
                }
                let key = (ctx.source, then);
                let fired = self
                    .counters
                    .get_mut(ctx.caster)
                    .is_ok_and(|mut counters| counters.bump(key, every));
                if fired {
                    self.derived.push(ctx.derived(then));
                }
            }
            AbilityEffect::Chain {
                jumps,
                radius,
                falloff,
                team,
                then,
            } => {
                if !self.gate_open(ctx) {
                    return;
                }
                let radius = radius.at(rank);
                let candidates: Vec<ChainCandidate> = self
                    .units
                    .iter()
                    .filter(|(e, _, unit_team, health, _)| {
                        health.is_alive()
                            && (*e == ctx.caster || !is_untargetable(&self.statuses_of, *e))
                            && team_allows(team, ctx.team, **unit_team)
                    })
                    .map(|(entity, tf, _, _, bound)| ChainCandidate {
                        entity,
                        position: tf.translation,
                        bounds: bounds_of(bound),
                    })
                    .collect();
                for start in targets {
                    let Ok((_, start_tf, ..)) = self.units.get(start) else {
                        continue;
                    };
                    let hops = chain_hops(start, start_tf.translation, &candidates, jumps, radius);
                    for (index, hop) in hops.into_iter().enumerate() {
                        let mut next = ctx
                            .derived(then)
                            .trigger_unit(Some(hop.entity))
                            .at(hop.position)
                            .affected(Vec::new());
                        // Chained damage is spell damage, not attack bonus (D1).
                        next.attack = None;
                        next.magnitude = ctx.magnitude * falloff.powi(index as i32 + 1);
                        self.derived.push(next);
                    }
                }
            }
            AbilityEffect::Accumulate { threshold, then } => {
                if !self.gate_open(ctx) {
                    return;
                }
                let key = (ctx.source, then);
                let fired = self.counters.get_mut(ctx.caster).is_ok_and(|mut counters| {
                    counters.accumulate(key, ctx.amount, threshold.at(rank))
                });
                if fired {
                    self.derived.push(ctx.derived(then));
                }
            }
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

/// The one place a [`super::definition::StatusSpec`] becomes a runtime [`StatusEffect`], for
/// timed effects (`ApplyStatus`) and toggle upkeep (`WhileToggled`) alike. `caster` is the unit
/// that causes the status (damage-over-time source, fear origin); `healer_amp` is the caster's
/// outgoing heal amplification, applied once to heal-over-time statuses. Callers set whatever
/// only they know (for example the undispellable flags of a toggle).
pub(crate) fn build_status_effect(
    spec: super::definition::StatusSpec,
    rank: u32,
    duration: f32,
    caster: Option<Entity>,
    point: Vec3,
    target_stats: Option<&CombatStats>,
    target_attrs: Option<&HeroAttributes>,
    healer_amp: f32,
) -> StatusEffect {
    let fear_source = match spec {
        super::definition::StatusSpec::Fear => caster,
        _ => None,
    };
    let mut built = spec.build(
        rank,
        duration,
        StatusBuildContext {
            move_speed: target_stats.map_or(0.0, |s| s.move_speed),
            agility: target_attrs.map_or(0.0, |a| a.agility),
            fear_source,
            source: caster,
            point,
        },
    );
    built.dot_source = caster;
    if built.heal_per_sec > 0.0 {
        built.heal_per_sec *= (1.0 + healer_amp).max(0.0);
    }
    built
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
    mut spell_hits: MessageWriter<SpellHitEvent>,
) {
    // First in, first out: gates append derived contexts behind everything already queued,
    // so the order (and the RNG draw order) is fixed by message order.
    let mut queue: VecDeque<TriggerContext> =
        triggers.read().map(|event| event.0.clone()).collect();
    while let Some(ctx) = queue.pop_front() {
        let Some(params) = source_params(ctx.source, &defs, ctx.rank) else {
            continue;
        };
        // Only a cast's own contexts (not gates' derived ones, not reactions) can hit.
        let spell = match ctx.source {
            EffectSource::Ability(ability) if ctx.depth == 0 && ctx.trigger.is_cast_phase() => {
                Some(ability)
            }
            _ => None,
        };
        let mut enemies_hit: Vec<Entity> = Vec::new();
        for entry in params
            .entries
            .iter()
            .filter(|entry| entry.trigger == ctx.trigger)
        {
            let targets = runner.targets(&ctx, entry.target, params.aoe_radius);
            if spell.is_some() && entry.effect.acts_on_targets() {
                for &target in &targets {
                    if runner.is_enemy(&ctx, target) && !enemies_hit.contains(&target) {
                        enemies_hit.push(target);
                    }
                }
            }
            runner.execute(&ctx, &entry.effect, targets, params.cast_range);
        }
        if let Some(ability) = spell {
            for target in enemies_hit {
                spell_hits.write(SpellHitEvent {
                    caster: ctx.caster,
                    team: ctx.team,
                    ability,
                    target,
                    point: ctx.point,
                });
            }
        }
        queue.extend(runner.take_derived());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &'static str) -> GateKey {
        (
            EffectSource::Item(ItemId::HeartwoodBand),
            AbilityTrigger::Custom(name),
        )
    }

    #[test]
    fn prd_constant_matches_known_values() {
        assert!((prd_constant(0.25) - 0.0847).abs() < 1e-3);
        assert!((prd_constant(0.5) - 0.3021).abs() < 1e-3);
        assert!((prd_constant(0.1) - 0.01475).abs() < 1e-3);
        assert_eq!(prd_constant(0.0), 0.0);
        assert_eq!(prd_constant(1.0), 1.0);
    }

    #[test]
    fn prd_long_run_rate_matches_nominal() {
        let mut rng = CombatRng::default();
        rng.seed(0x5EED);
        let mut counters = TriggerCounters::default();
        let rolls = 20_000;
        let procs = (0..rolls)
            .filter(|_| counters.roll_pseudo(key("on_bash"), 0.25, &mut rng))
            .count();
        let rate = procs as f32 / rolls as f32;
        assert!((rate - 0.25).abs() < 0.01, "long-run rate was {rate}");
    }

    #[test]
    fn prd_failures_reset_on_success_and_are_per_source() {
        let mut rng = CombatRng::default();
        rng.seed(7);
        let mut counters = TriggerCounters::default();
        let failures = |counters: &TriggerCounters, k| {
            counters
                .prd
                .iter()
                .find(|(key, _)| *key == k)
                .map_or(0, |(_, n)| *n)
        };
        let mut streak = 0;
        let mut saw_success = false;
        for _ in 0..200 {
            let success = counters.roll_pseudo(key("on_a"), 0.25, &mut rng);
            streak = if success { 0 } else { streak + 1 };
            assert_eq!(failures(&counters, key("on_a")), streak);
            saw_success |= success;
        }
        assert!(
            saw_success,
            "a 25% pseudo-random proc succeeds within 200 rolls"
        );
        assert_eq!(
            failures(&counters, key("on_b")),
            0,
            "other gates keep their own streak"
        );
        // A guaranteed proc never builds a streak.
        assert!(counters.roll_pseudo(key("on_b"), 1.0, &mut rng));
        assert_eq!(failures(&counters, key("on_b")), 0);
    }

    #[test]
    fn accumulate_fires_at_threshold_and_keeps_remainder() {
        let mut counters = TriggerCounters::default();
        let mut push = |amount| counters.accumulate(key("on_full"), amount, 100.0);
        assert!(!push(60.0));
        assert!(push(60.0), "120 reaches 100");
        assert!(!push(70.0), "20 carried over + 70 = 90");
        assert!(!push(-500.0), "negative amounts count as zero");
        assert!(push(10.0), "exactly 100 again");
        assert!(!push(0.0));
        let mut zero = TriggerCounters::default();
        assert!(
            !zero.accumulate(key("on_never"), 5.0, 0.0),
            "no threshold, no fire"
        );
    }

    #[test]
    fn derived_context_inherits_reaction() {
        let caster = Entity::from_raw_u32(1).unwrap();
        let root = TriggerContext::new(
            AbilityTrigger::OnDamageTaken,
            EffectSource::Item(ItemId::HeartwoodBand),
            caster,
            Team::Radiant,
            1,
            Vec3::ZERO,
            Vec3::ZERO,
        )
        .amount(42.0);
        assert!(root.reaction, "damage taken is a reaction trigger");
        let derived = root.derived(AbilityTrigger::Custom("on_thorns"));
        assert!(derived.reaction);
        assert_eq!(derived.depth, 1);
        assert_eq!(derived.amount, 42.0, "the occurrence size carries over");

        let attack = TriggerContext::new(
            AbilityTrigger::OnAttackHit,
            EffectSource::Item(ItemId::HeartwoodBand),
            caster,
            Team::Radiant,
            1,
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert!(!attack.derived(AbilityTrigger::Custom("on_x")).reaction);
    }
}
