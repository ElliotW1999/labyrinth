//! Status effects: buffs, debuffs, dispel, shields and damage / heal over time.
//!
//! Owns the [`StatusEffects`] component, the [`StatusEffectEvent`] entry point that every
//! gameplay system uses to add or remove statuses, and the per-frame ticking.

use bevy::prelude::*;

use crate::combat::DamageEvent;
use crate::components::{CombatStats, DamageType, Health, HeroAttributes, ShieldPool};

pub struct StatusPlugin;

impl Plugin for StatusPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<StatusEffectEvent>().add_systems(
            Update,
            tick_status_effects
                .before(crate::combat::apply_damage_events)
                .before(crate::vitals::sync_status_resource_caps),
        );
    }
}

/// Generic buff / debuff container on heroes (and potentially other units later).
#[derive(Component, Debug, Clone, Default)]
pub struct StatusEffects {
    pub effects: Vec<StatusEffect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum StatusKind {
    Buff,
    Debuff,
}

/// How hard this unit’s dispel is when purging matching [`StatusKind`] stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DispelStrength {
    /// Removes [`DispelType::Basic`] and [`DispelType::Strong`] stacks.
    #[default]
    Basic,
    /// Removes only [`DispelType::Strong`] (and Basic) stacks; Strong dispels act as full purge on dispellable effects.
    Strong,
}

/// Per-effect resistance to [`AbilityEffect::Dispel`] (basic vs strong purge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DispelType {
    /// Purgable by basic or strong dispel.
    #[default]
    Basic,
    /// Only strong dispel removes this (and basic debuff immunity purge).
    Strong,
    /// Never removed by dispel (still removed by explicit id / expiry / special rules).
    None,
}

/// What a [`Leash`] is measured from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LeashAnchor {
    /// A fixed spot on the ground (x, z).
    Point(Vec2),
    /// A unit; the leash does nothing once that unit is gone.
    Unit(Entity),
}

/// Keeps the unit inside a circle of `radius` around `anchor` (see `movement::enforce_leashes`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leash {
    pub anchor: LeashAnchor,
    pub radius: f32,
}

#[derive(Debug, Clone)]
pub struct StatusEffect {
    pub id: &'static str,
    pub kind: StatusKind,
    pub remaining: f32,
    pub dispel_type: DispelType,
    /// When true, no dispel removes this (same as [`DispelType::None`] but skips strong purge too).
    pub undispellable: bool,
    /// Flat combat modifiers applied for the duration (removed on expire).
    pub attack_damage: f32,
    pub armor: f32,
    pub magic_resist: f32,
    pub move_speed: f32,
    /// Flat IAS change from this effect (negative values slow attack speed).
    pub attack_speed_flat: f32,
    /// Multiplicative AS modifier × (1 + `attack_speed_mult`) — negative slows.
    pub attack_speed_mult: f32,
    /// Percent slow applied to turn rate (0..100), see [`CombatStats::turn_rate`].
    pub turn_rate_slow_percent: f32,
    /// Percent added to turn rate (0..=400); bonuses add up.
    pub turn_rate_bonus_percent: f32,
    /// 0..1 miss chance on basic attack damage resolution (`0` = no blind).
    pub blind_miss_chance: f32,
    pub heal_per_sec: f32,
    /// Timed resource modifier, summed when read (see [`crate::vitals::resource_stat`]).
    pub resource: Option<(crate::vitals::ResourceStat, f32)>,
    /// Fraction (0..1) of healing received that is negated. The strongest one applies.
    pub heal_reduction: f32,
    /// Prevents all healing while active.
    pub heal_blocked: bool,
    /// Diverts incoming damage to mana while active.
    pub mana_shield: Option<crate::vitals::ManaShield>,
    pub damage_per_sec: f32,
    pub dot_damage_type: DamageType,
    pub dot_source: Option<Entity>,
    /// Added to [`ShieldPool`] on apply; removed on expire.
    pub shield_amount: f32,
    pub damage_immune: bool,
    /// When true, skip collision with creeps and heroes (buildings/trees still block).
    pub ignore_unit_collision: bool,
    /// When true, this unit claims more separation against overlapping creeps/heroes.
    pub force_unit_push: bool,
    pub silenced: bool,
    pub stunned: bool,
    pub rooted: bool,
    pub disarmed: bool,
    pub muted: bool,
    pub asleep: bool,
    pub invulnerable: bool,
    pub debuff_immune: bool,
    /// Ignores trees and other ground obstacles while moving (buildings still block).
    pub flying: bool,
    /// Confines the unit to a circle (region restriction or tether).
    pub leash: Option<Leash>,
    /// Forced retreat from this entity while feared (`None` if source despawned).
    pub fear_source: Option<Entity>,
}

impl StatusEffect {
    pub fn buff(id: &'static str, duration: f32) -> Self {
        Self {
            id,
            kind: StatusKind::Buff,
            remaining: duration,
            dispel_type: DispelType::Basic,
            undispellable: false,
            attack_damage: 0.0,
            armor: 0.0,
            magic_resist: 0.0,
            move_speed: 0.0,
            attack_speed_flat: 0.0,
            attack_speed_mult: 0.0,
            turn_rate_slow_percent: 0.0,
            turn_rate_bonus_percent: 0.0,
            blind_miss_chance: 0.0,
            heal_per_sec: 0.0,
            resource: None,
            heal_reduction: 0.0,
            heal_blocked: false,
            mana_shield: None,
            damage_per_sec: 0.0,
            dot_damage_type: DamageType::Magical,
            dot_source: None,
            shield_amount: 0.0,
            damage_immune: false,
            ignore_unit_collision: false,
            force_unit_push: false,
            silenced: false,
            stunned: false,
            rooted: false,
            disarmed: false,
            muted: false,
            asleep: false,
            invulnerable: false,
            debuff_immune: false,
            flying: false,
            leash: None,
            fear_source: None,
        }
    }

    pub fn shield(amount: f32, duration: f32) -> Self {
        Self {
            shield_amount: amount,
            ..Self::buff("shield", duration)
        }
    }

    pub fn heal_over_time(hps: f32, duration: f32) -> Self {
        Self {
            heal_per_sec: hps,
            ..Self::buff("heal_over_time", duration)
        }
    }

    /// `fraction` (0..1) of healing received is negated.
    pub fn heal_reduction(fraction: f32, duration: f32) -> Self {
        Self {
            heal_reduction: fraction,
            ..Self::debuff("heal_reduction", duration)
        }
    }

    pub fn heal_block(duration: f32) -> Self {
        Self {
            heal_blocked: true,
            ..Self::debuff("heal_block", duration)
        }
    }

    /// A timed resource modifier, summed when read (never stored into stats).
    pub fn resource_modifier(
        id: &'static str,
        kind: StatusKind,
        stat: crate::vitals::ResourceStat,
        value: f32,
        duration: f32,
    ) -> Self {
        let base = match kind {
            StatusKind::Buff => Self::buff(id, duration),
            StatusKind::Debuff => Self::debuff(id, duration),
        };
        Self {
            resource: Some((stat, value)),
            ..base
        }
    }

    pub fn mana_shield(shield: crate::vitals::ManaShield, duration: f32) -> Self {
        Self {
            mana_shield: Some(shield),
            ..Self::buff("mana_shield", duration)
        }
    }

    pub fn burn(dps: f32, damage_type: DamageType, duration: f32) -> Self {
        Self {
            damage_per_sec: dps,
            dot_damage_type: damage_type,
            ..Self::debuff("burn", duration)
        }
    }

    pub fn debuff(id: &'static str, duration: f32) -> Self {
        Self {
            kind: StatusKind::Debuff,
            ..Self::buff(id, duration)
        }
    }

    pub fn phased(duration: f32) -> Self {
        Self {
            ignore_unit_collision: true,
            ..Self::buff("phased", duration)
        }
    }

    /// Temporary soft-separation against other units.
    /// Ground obstacles stop blocking the unit. When it ends over geometry the unit is
    /// set down on the nearest walkable spot.
    pub fn flying(duration: f32) -> Self {
        Self {
            flying: true,
            ..Self::buff("flying", duration)
        }
    }

    /// Confinement to a circle around `leash.anchor`. `id` is `"leash"` for a fixed
    /// region and `"tether"` for a unit anchor.
    pub fn leashed(leash: Leash, duration: f32) -> Self {
        let id = match leash.anchor {
            LeashAnchor::Point(_) => "leash",
            LeashAnchor::Unit(_) => "tether",
        };
        Self {
            leash: Some(leash),
            ..Self::debuff(id, duration)
        }
    }

    pub fn forceful(duration: f32) -> Self {
        Self {
            force_unit_push: true,
            ..Self::buff("forceful", duration)
        }
    }

    pub fn silenced(duration: f32) -> Self {
        Self {
            silenced: true,
            ..Self::debuff("silence", duration)
        }
    }

    pub fn stunned(duration: f32) -> Self {
        Self {
            stunned: true,
            ..Self::debuff("stun", duration)
        }
    }

    pub fn rooted(duration: f32) -> Self {
        Self {
            rooted: true,
            ..Self::debuff("root", duration)
        }
    }

    pub fn disarmed(duration: f32) -> Self {
        Self {
            disarmed: true,
            ..Self::debuff("disarm", duration)
        }
    }

    pub fn debuff_immunity(duration: f32) -> Self {
        Self {
            debuff_immune: true,
            dispel_type: DispelType::None,
            undispellable: true,
            ..Self::buff("spell_immunity", duration)
        }
    }

    pub fn mute(duration: f32) -> Self {
        Self {
            muted: true,
            ..Self::debuff("mute", duration)
        }
    }

    pub fn sleep(duration: f32) -> Self {
        Self {
            asleep: true,
            ..Self::debuff("sleep", duration)
        }
    }

    pub fn invulnerable(duration: f32) -> Self {
        Self {
            invulnerable: true,
            kind: StatusKind::Buff,
            dispel_type: DispelType::Strong,
            ..Self::buff("invulnerable", duration)
        }
    }

    /// Lifted out of play: stunned, invulnerable, and untargetable. `hostile` picks the
    /// debuff kind (blocked by debuff immunity) over the buff kind (self / ally cast).
    /// Not purgeable; only expiry or an explicit `remove_status cyclone` ends it.
    pub fn cyclone(duration: f32, hostile: bool) -> Self {
        Self {
            stunned: true,
            invulnerable: true,
            kind: if hostile {
                StatusKind::Debuff
            } else {
                StatusKind::Buff
            },
            dispel_type: DispelType::None,
            undispellable: true,
            ..Self::buff("cyclone", duration)
        }
    }

    pub fn blind(miss_chance: f32, duration: f32) -> Self {
        Self {
            blind_miss_chance: miss_chance.clamp(0.0, 1.0),
            ..Self::debuff("blind", duration)
        }
    }

    pub fn fear(duration: f32, source: Option<Entity>) -> Self {
        Self {
            fear_source: source,
            dispel_type: DispelType::Basic,
            ..Self::debuff("fear", duration)
        }
    }

    /// Full disable: silence + disarm + mute (items only), per catalog “composite hex”.
    pub fn hexed(duration: f32) -> Self {
        Self {
            id: "hex",
            silenced: true,
            disarmed: true,
            muted: true,
            dispel_type: DispelType::Strong,
            ..Self::debuff("hex", duration)
        }
    }
}

impl StatusEffects {
    /// True while any active effect prevents healing.
    pub fn is_heal_blocked(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.heal_blocked && e.remaining > 0.0)
    }

    /// Strongest active healing reduction (0..1); reductions do not stack.
    pub fn heal_reduction(&self) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .map(|e| e.heal_reduction)
            .fold(0.0, f32::max)
            .clamp(0.0, 1.0)
    }

    /// The active mana shield with the highest absorb fraction.
    pub fn mana_shield(&self) -> Option<crate::vitals::ManaShield> {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .filter_map(|e| e.mana_shield)
            .max_by(|a, b| a.absorb.total_cmp(&b.absorb))
    }

    /// Sum of every active timed modifier for `stat`.
    pub fn resource_sum(&self, stat: crate::vitals::ResourceStat) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .filter_map(|e| e.resource)
            .filter(|(s, _)| *s == stat)
            .map(|(_, v)| v)
            .sum()
    }

    pub fn is_phased(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.ignore_unit_collision && e.remaining > 0.0)
    }

    /// Active leashes and tethers.
    pub fn leashes(&self) -> impl Iterator<Item = &Leash> {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .filter_map(|e| e.leash.as_ref())
    }

    pub fn is_flying(&self) -> bool {
        self.effects.iter().any(|e| e.flying && e.remaining > 0.0)
    }

    pub fn can_push_units(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.force_unit_push && e.remaining > 0.0)
    }

    pub fn is_silenced(&self) -> bool {
        self.effects.iter().any(|e| e.silenced && e.remaining > 0.0)
    }

    pub fn is_stunned(&self) -> bool {
        self.effects.iter().any(|e| e.stunned && e.remaining > 0.0)
    }

    pub fn is_asleep(&self) -> bool {
        self.effects.iter().any(|e| e.asleep && e.remaining > 0.0)
    }

    pub fn is_rooted(&self) -> bool {
        self.effects
            .iter()
            .any(|e| (e.rooted || e.stunned || e.asleep) && e.remaining > 0.0)
    }

    pub fn is_disarmed(&self) -> bool {
        self.effects
            .iter()
            .any(|e| (e.disarmed || e.stunned || e.asleep) && e.remaining > 0.0)
    }

    pub fn is_muted(&self) -> bool {
        self.effects.iter().any(|e| e.muted && e.remaining > 0.0)
    }

    pub fn is_feared(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.id == "fear" && e.remaining > 0.0)
    }

    pub fn fear_flee_source(&self) -> Option<Entity> {
        self.effects
            .iter()
            .filter(|e| e.id == "fear" && e.remaining > 0.0)
            .find_map(|e| e.fear_source)
    }

    pub fn blind_miss_chance(&self) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .fold(0.0_f32, |acc, e| acc.max(e.blind_miss_chance))
            .clamp(0.0, 1.0)
    }

    pub fn has_debuff_immunity(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.debuff_immune && e.remaining > 0.0)
    }

    pub fn has_damage_immunity(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.damage_immune && e.remaining > 0.0)
    }

    pub fn is_invulnerable(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.invulnerable && e.remaining > 0.0)
    }

    /// Whether a status with this id is active.
    pub fn has_status(&self, id: &str) -> bool {
        self.effects.iter().any(|e| e.id == id && e.remaining > 0.0)
    }

    /// In a cyclone: no other unit can pick it as a unit target (basic attacks, unit
    /// casts, auto-acquire, homing projectiles) and area effects skip it.
    pub fn is_untargetable(&self) -> bool {
        self.has_status("cyclone")
    }

    pub fn can_move(&self) -> bool {
        !self.is_rooted()
    }

    pub fn can_attack(&self) -> bool {
        !self.is_disarmed()
    }

    pub fn can_cast(&self) -> bool {
        !self.is_silenced() && !self.is_stunned() && !self.is_asleep()
    }
}

/// Whether `entity` is held out of play (cyclone) and so cannot be picked as a target.
/// The one check behind every "can this unit be targeted" filter.
pub fn is_untargetable(statuses: &Query<&StatusEffects>, entity: Entity) -> bool {
    statuses
        .get(entity)
        .is_ok_and(|statuses| statuses.is_untargetable())
}

/// Whether `effect` is removed by a dispel of `strength` on `kind`.
pub fn status_dispel_matches(
    effect: &StatusEffect,
    kind: StatusKind,
    strength: DispelStrength,
) -> bool {
    if effect.kind != kind {
        return false;
    }
    if effect.undispellable {
        return false;
    }
    match effect.dispel_type {
        DispelType::None => false,
        DispelType::Basic => true,
        DispelType::Strong => matches!(strength, DispelStrength::Strong),
    }
}

#[derive(Debug, Clone)]
pub enum StatusChange {
    Apply(StatusEffect),
    Dispel {
        kind: StatusKind,
        strength: DispelStrength,
    },
    Remove(&'static str),
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

pub fn tick_status_effects(
    time: Res<Time>,
    mut damage: MessageWriter<DamageEvent>,
    mut units: Query<(
        Entity,
        &mut StatusEffects,
        &mut CombatStats,
        &mut Health,
        Option<&mut ShieldPool>,
        Option<&HeroAttributes>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut statuses, mut stats, mut health, mut shield, attrs) in &mut units {
        let agi = attrs.map(|a| a.agility).unwrap_or(0.0);
        let hot_factor = crate::vitals::heal_factor(
            crate::vitals::HealKind::OverTime,
            0.0,
            Some(&*stats),
            Some(&*statuses),
        );
        let mut i = 0;
        while i < statuses.effects.len() {
            let effect = &mut statuses.effects[i];
            if effect.heal_per_sec > 0.0 {
                // The healer's amp was folded into `heal_per_sec` when the status was applied.
                crate::vitals::apply_heal(&mut health, effect.heal_per_sec * dt, hot_factor);
            }
            if effect.damage_per_sec > 0.0 {
                damage.write(DamageEvent::dot(
                    effect.dot_source,
                    entity,
                    effect.damage_per_sec * dt,
                    effect.dot_damage_type,
                ));
            }
            effect.remaining -= dt;
            if effect.remaining <= 0.0 {
                let expired = statuses.effects.remove(i);
                subtract_status_stat_delta(&expired, &mut stats, agi);
                release_shield(&expired, shield.as_deref_mut());
            } else {
                i += 1;
            }
        }
    }
}

/// Apply a phased buff (no combat-stat mutation).
#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_phased(statuses: &mut StatusEffects, duration: f32) {
    statuses.effects.retain(|e| e.id != "phased");
    statuses.effects.push(StatusEffect::phased(duration));
}

/// Apply a forceful buff so this unit claims more separation against overlaps.
#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_forceful(statuses: &mut StatusEffects, duration: f32) {
    statuses.effects.retain(|e| e.id != "forceful");
    statuses.effects.push(StatusEffect::forceful(duration));
}

pub(crate) fn subtract_status_stat_delta(
    effect: &StatusEffect,
    stats: &mut CombatStats,
    agility: f32,
) {
    stats.attack_damage -= effect.attack_damage;
    stats.armor -= effect.armor;
    stats.magic_resist -= effect.magic_resist;
    stats.move_speed -= effect.move_speed;
    stats.attack_speed_flat -= effect.attack_speed_flat;
    stats.attack_speed_mult -= effect.attack_speed_mult;
    stats.recompute_attack_speed(agility);
}

pub(crate) fn add_status_stat_delta(effect: &StatusEffect, stats: &mut CombatStats, agility: f32) {
    stats.attack_damage += effect.attack_damage;
    stats.armor += effect.armor;
    stats.magic_resist += effect.magic_resist;
    stats.move_speed += effect.move_speed;
    stats.attack_speed_flat += effect.attack_speed_flat;
    stats.attack_speed_mult += effect.attack_speed_mult;
    stats.recompute_attack_speed(agility);
}

/// Apply a timed buff/debuff and immediately add its flat modifiers to combat stats.
/// Debuffs are ignored while the unit has debuff immunity.
pub fn apply_status(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    effect: StatusEffect,
    agility: f32,
) -> bool {
    if effect.kind == StatusKind::Debuff && statuses.has_debuff_immunity() {
        return false;
    }
    // Refresh same-id effects by replacing.
    if let Some(existing) = statuses.effects.iter().position(|e| e.id == effect.id) {
        let old = statuses.effects.remove(existing);
        subtract_status_stat_delta(&old, stats, agility);
    }
    add_status_stat_delta(&effect, stats, agility);
    statuses.effects.push(effect);
    true
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_stun(statuses: &mut StatusEffects, stats: &mut CombatStats, duration: f32) -> bool {
    apply_status(statuses, stats, StatusEffect::stunned(duration), 0.0)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_debuff_immunity(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    duration: f32,
    agility: f32,
) -> bool {
    // Purge existing debuffs when gaining immunity — strong dispel.
    statuses.effects.retain_mut(|e| {
        let purge = e.kind == StatusKind::Debuff
            && status_dispel_matches(e, StatusKind::Debuff, DispelStrength::Strong);
        if purge {
            subtract_status_stat_delta(e, stats, agility);
        }
        !purge
    });
    apply_status(
        statuses,
        stats,
        StatusEffect::debuff_immunity(duration),
        agility,
    )
}

/// Charge `absorbed` points of damage the [`ShieldPool`] soaked against the shield statuses
/// that back it, so each status's `shield_amount` stays what is left of that shield.
///
/// Shields are consumed oldest first: the walk follows `statuses.effects` in vector order,
/// which is application order (a refresh removes and re-pushes, so a refreshed shield becomes
/// the newest). Each shield gives up what it has before the next one is touched.
pub fn consume_shields(statuses: &mut StatusEffects, mut absorbed: f32) {
    for effect in &mut statuses.effects {
        if absorbed <= 0.0 {
            break;
        }
        if effect.shield_amount <= 0.0 {
            continue;
        }
        let taken = effect.shield_amount.min(absorbed);
        effect.shield_amount -= taken;
        absorbed -= taken;
    }
}

/// Take the remaining part of a shield status back out of the pool (expiry, dispel, remove).
/// Because of [`consume_shields`], `shield_amount` is what is left of that shield.
fn release_shield(effect: &StatusEffect, pool: Option<&mut ShieldPool>) {
    if effect.shield_amount > 0.0 {
        if let Some(pool) = pool {
            pool.amount = (pool.amount - effect.shield_amount).max(0.0);
        }
    }
}

fn remove_statuses(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    agility: f32,
    mut pool: Option<&mut ShieldPool>,
    matches: impl Fn(&StatusEffect) -> bool,
) {
    statuses.effects.retain(|effect| {
        if !matches(effect) {
            return true;
        }
        subtract_status_stat_delta(effect, stats, agility);
        release_shield(effect, pool.as_deref_mut());
        false
    });
}

pub fn apply_status_effect_events(
    mut commands: Commands,
    mut events: MessageReader<StatusEffectEvent>,
    mut units: Query<(
        Entity,
        &mut StatusEffects,
        &mut CombatStats,
        Option<&mut ShieldPool>,
        Option<&HeroAttributes>,
    )>,
) {
    for event in events.read() {
        let Ok((entity, mut statuses, mut stats, mut shield, attrs)) = units.get_mut(event.target)
        else {
            continue;
        };
        let agility = attrs.map(|a| a.agility).unwrap_or(0.0);
        match &event.change {
            StatusChange::Apply(effect) => {
                if effect.debuff_immune {
                    remove_statuses(
                        &mut statuses,
                        &mut stats,
                        agility,
                        shield.as_deref_mut(),
                        |e| {
                            e.kind == StatusKind::Debuff
                                && status_dispel_matches(
                                    e,
                                    StatusKind::Debuff,
                                    DispelStrength::Strong,
                                )
                        },
                    );
                }
                let refresh_shield = effect.shield_amount > 0.0;
                let old_shield = if refresh_shield {
                    statuses
                        .effects
                        .iter()
                        .find(|e| e.id == effect.id)
                        .map(|e| e.shield_amount)
                        .unwrap_or(0.0)
                } else {
                    0.0
                };
                let applied = apply_status(&mut statuses, &mut stats, effect.clone(), agility);
                if applied && effect.flying {
                    // When the flight ends, check the ground it lands on.
                    commands
                        .entity(entity)
                        .insert(crate::displacement::SettleOnLanding);
                }
                if applied && effect.shield_amount > 0.0 {
                    if let Some(pool) = shield.as_deref_mut() {
                        pool.amount = (pool.amount - old_shield).max(0.0) + effect.shield_amount;
                    } else {
                        commands.entity(entity).insert(ShieldPool {
                            amount: effect.shield_amount,
                        });
                    }
                }
            }
            StatusChange::Dispel { kind, strength } => {
                remove_statuses(
                    &mut statuses,
                    &mut stats,
                    agility,
                    shield.as_deref_mut(),
                    |e| status_dispel_matches(e, *kind, *strength),
                );
            }
            StatusChange::Remove(id) => {
                remove_statuses(
                    &mut statuses,
                    &mut stats,
                    agility,
                    shield.as_deref_mut(),
                    |e| e.id == *id,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_buff_applies_and_expires_modifiers() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 10.0);
        apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect {
                armor: 8.0,
                ..StatusEffect::buff("test", 1.0)
            },
            0.0,
        );
        assert_eq!(stats.armor, 10.0);
        assert_eq!(statuses.effects.len(), 1);
        statuses.effects[0].remaining = 0.0;
        // Simulate expire path
        let expired = statuses.effects.remove(0);
        stats.armor -= expired.armor;
        assert_eq!(stats.armor, 2.0);
    }

    #[test]
    fn phased_buff_flags_ignore_unit_collision() {
        let mut statuses = StatusEffects::default();
        assert!(!statuses.is_phased());
        apply_phased(&mut statuses, 1.0);
        assert!(statuses.is_phased());
        assert!(statuses.effects.iter().any(|e| e.ignore_unit_collision));
        statuses.effects[0].remaining = 0.0;
        assert!(!statuses.is_phased());
    }

    #[test]
    fn forceful_buff_enables_unit_push() {
        let mut statuses = StatusEffects::default();
        assert!(!statuses.can_push_units());
        apply_forceful(&mut statuses, 2.0);
        assert!(statuses.can_push_units());
        assert!(!statuses.is_phased());
        statuses.effects[0].remaining = 0.0;
        assert!(!statuses.can_push_units());
    }

    #[test]
    fn debuff_immunity_blocks_stun() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 10.0);
        assert!(apply_debuff_immunity(&mut statuses, &mut stats, 5.0, 0.0));
        assert!(!apply_stun(&mut statuses, &mut stats, 2.0));
        assert!(!statuses.is_stunned());
    }

    #[test]
    fn dispel_tiers_remove_basic_but_not_strong_debuffs() {
        let mut app = App::new();
        app.add_message::<StatusEffectEvent>()
            .add_systems(Update, apply_status_effect_events);
        let victim = app
            .world_mut()
            .spawn((
                StatusEffects::default(),
                CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0),
                HeroAttributes::starter(),
            ))
            .id();
        app.world_mut().write_message(StatusEffectEvent {
            source: None,
            target: victim,
            change: StatusChange::Apply(StatusEffect::stunned(2.0)),
        });
        app.world_mut().write_message(StatusEffectEvent {
            source: None,
            target: victim,
            change: StatusChange::Apply(StatusEffect::hexed(2.0)),
        });
        app.update();
        let st = app.world().get::<StatusEffects>(victim).unwrap();
        assert!(st.is_stunned());
        assert!(st.effects.iter().any(|e| e.id == "hex"));
        app.world_mut().write_message(StatusEffectEvent {
            source: None,
            target: victim,
            change: StatusChange::Dispel {
                kind: StatusKind::Debuff,
                strength: super::DispelStrength::Basic,
            },
        });
        app.update();
        let st = app.world().get::<StatusEffects>(victim).unwrap();
        assert!(!st.is_stunned());
        assert!(st.effects.iter().any(|e| e.id == "hex"));
        app.world_mut().write_message(StatusEffectEvent {
            source: None,
            target: victim,
            change: StatusChange::Dispel {
                kind: StatusKind::Debuff,
                strength: super::DispelStrength::Strong,
            },
        });
        app.update();
        let st = app.world().get::<StatusEffects>(victim).unwrap();
        assert!(!st.effects.iter().any(|e| e.id == "hex"));
    }

    #[test]
    fn heal_does_not_revive_dead_units_via_heal_over_time() {
        use std::time::Duration;
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<DamageEvent>()
            .add_systems(Update, tick_status_effects);
        let mut statuses = StatusEffects::default();
        statuses.effects.push(StatusEffect {
            heal_per_sec: 50.0,
            ..StatusEffect::buff("hot", 5.0)
        });
        let dead = app
            .world_mut()
            .spawn((
                statuses,
                CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                Health {
                    current: 0.0,
                    max: 100.0,
                    regen_per_sec: 0.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs(1));
        app.update();
        assert_eq!(app.world().get::<Health>(dead).unwrap().current, 0.0);
    }

    mod max_caps {
        use super::*;
        use crate::components::Mana;
        use crate::vitals::{ResourceStat, sync_status_resource_caps};
        use std::time::Duration;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<DamageEvent>()
                .add_systems(
                    Update,
                    (
                        apply_status_effect_events,
                        tick_status_effects,
                        sync_status_resource_caps,
                    )
                        .chain(),
                );
            app
        }

        fn unit(app: &mut App, current: f32, max: f32) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    Health {
                        current,
                        max,
                        regen_per_sec: 0.0,
                    },
                    Mana {
                        current: 50.0,
                        max: 100.0,
                        regen_per_sec: 0.0,
                    },
                ))
                .id()
        }

        fn buff(id: &'static str, stat: ResourceStat, amount: f32, secs: f32) -> StatusEffect {
            StatusEffect::resource_modifier(id, StatusKind::Buff, stat, amount, secs)
        }

        fn send(app: &mut App, target: Entity, change: StatusChange) {
            app.world_mut().write_message(StatusEffectEvent {
                source: None,
                target,
                change,
            });
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn hp(app: &App, e: Entity) -> (f32, f32) {
            let h = app.world().get::<Health>(e).unwrap();
            (h.current, h.max)
        }

        #[test]
        fn timed_max_health_preserves_ratio_on_apply_expire_and_dispel() {
            let mut app = app();
            let a = unit(&mut app, 500.0, 1000.0);
            send(
                &mut app,
                a,
                StatusChange::Apply(buff("hp_buff", ResourceStat::MaxHealth, 1000.0, 2.0)),
            );
            step(&mut app, 0.0);
            assert_eq!(hp(&app, a), (1000.0, 2000.0), "apply keeps the 50% ratio");
            // Expiry
            step(&mut app, 2.5);
            step(&mut app, 0.0);
            assert_eq!(
                hp(&app, a),
                (500.0, 1000.0),
                "expiry: no free heal, no extra loss"
            );

            // Dispel
            let b = unit(&mut app, 250.0, 1000.0);
            send(
                &mut app,
                b,
                StatusChange::Apply(buff("hp_buff", ResourceStat::MaxHealth, 1000.0, 30.0)),
            );
            step(&mut app, 0.0);
            assert_eq!(hp(&app, b), (500.0, 2000.0));
            send(
                &mut app,
                b,
                StatusChange::Dispel {
                    kind: StatusKind::Buff,
                    strength: DispelStrength::Basic,
                },
            );
            step(&mut app, 0.0);
            assert_eq!(hp(&app, b), (250.0, 1000.0));
        }

        #[test]
        fn refresh_and_remove_do_not_leak_max() {
            let mut app = app();
            let u = unit(&mut app, 1000.0, 1000.0);
            for _ in 0..3 {
                send(
                    &mut app,
                    u,
                    StatusChange::Apply(buff("hp_buff", ResourceStat::MaxHealth, 300.0, 30.0)),
                );
                step(&mut app, 0.0);
            }
            assert_eq!(
                hp(&app, u).1,
                1300.0,
                "refreshing the same id does not stack"
            );
            send(&mut app, u, StatusChange::Remove("hp_buff"));
            step(&mut app, 0.0);
            assert_eq!(hp(&app, u), (1000.0, 1000.0));
        }

        #[test]
        fn timed_max_mana_and_unrelated_stats_are_independent() {
            let mut app = app();
            let u = unit(&mut app, 100.0, 1000.0);
            send(
                &mut app,
                u,
                StatusChange::Apply(buff("mp_buff", ResourceStat::MaxMana, 100.0, 1.0)),
            );
            step(&mut app, 0.0);
            let mana = *app.world().get::<Mana>(u).unwrap();
            assert_eq!((mana.current, mana.max), (100.0, 200.0));
            assert_eq!(hp(&app, u), (100.0, 1000.0));
            step(&mut app, 1.5);
            step(&mut app, 0.0);
            let mana = *app.world().get::<Mana>(u).unwrap();
            assert_eq!((mana.current, mana.max), (50.0, 100.0));
        }

        #[test]
        fn clamped_max_debuff_restores_the_original_max_on_expiry() {
            let mut app = app();
            let u = unit(&mut app, 100.0, 100.0);
            let debuff = StatusEffect::resource_modifier(
                "curse",
                StatusKind::Debuff,
                ResourceStat::MaxHealth,
                -500.0,
                2.0,
            );
            send(&mut app, u, StatusChange::Apply(debuff));
            step(&mut app, 0.0);
            assert_eq!(hp(&app, u).1, 1.0, "max floors at 1");
            step(&mut app, 2.5);
            step(&mut app, 0.0);
            assert_eq!(hp(&app, u).1, 100.0, "expiry must not leak extra max");
        }

        #[test]
        fn a_dead_unit_stays_dead_when_max_changes() {
            let mut app = app();
            let u = unit(&mut app, 0.0, 1000.0);
            send(
                &mut app,
                u,
                StatusChange::Apply(buff("hp_buff", ResourceStat::MaxHealth, 500.0, 5.0)),
            );
            step(&mut app, 0.0);
            assert_eq!(hp(&app, u), (0.0, 1500.0));
        }
    }

    mod heal_statuses {
        use super::*;
        use crate::vitals::{HealKind, apply_heal, heal_factor};
        use std::time::Duration;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<DamageEvent>()
                .add_systems(
                    Update,
                    (apply_status_effect_events, tick_status_effects).chain(),
                );
            app
        }

        fn wounded(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    Health {
                        current: 100.0,
                        max: 1000.0,
                        regen_per_sec: 0.0,
                    },
                ))
                .id()
        }

        fn send(app: &mut App, target: Entity, change: StatusChange) {
            app.world_mut().write_message(StatusEffectEvent {
                source: None,
                target,
                change,
            });
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn factor_of(app: &App, unit: Entity, kind: HealKind) -> f32 {
            let w = app.world();
            heal_factor(
                kind,
                0.5,
                w.get::<CombatStats>(unit),
                w.get::<StatusEffects>(unit),
            )
        }

        #[test]
        fn heal_block_zeroes_every_heal_kind_and_is_dispellable() {
            let mut app = app();
            let unit = wounded(&mut app);
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_block(10.0)),
            );
            step(&mut app, 0.0);
            for kind in [
                HealKind::Direct,
                HealKind::OverTime,
                HealKind::Leech,
                HealKind::Regen,
                HealKind::Conversion,
            ] {
                assert_eq!(factor_of(&app, unit, kind), 0.0, "{kind:?}");
            }
            send(
                &mut app,
                unit,
                StatusChange::Dispel {
                    kind: StatusKind::Debuff,
                    strength: DispelStrength::Basic,
                },
            );
            step(&mut app, 0.0);
            assert!(factor_of(&app, unit, HealKind::Direct) > 0.0);
        }

        #[test]
        fn heal_reduction_blocked_by_debuff_immunity() {
            let mut app = app();
            let unit = wounded(&mut app);
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::debuff_immunity(10.0)),
            );
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_reduction(0.5, 10.0)),
            );
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_block(10.0)),
            );
            step(&mut app, 0.0);
            let statuses = app.world().get::<StatusEffects>(unit).unwrap();
            assert_eq!(statuses.heal_reduction(), 0.0);
            assert!(!statuses.is_heal_blocked());
        }

        #[test]
        fn heal_over_time_ticks_total_over_duration() {
            let mut app = app();
            let unit = wounded(&mut app);
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_over_time(50.0, 4.0)),
            );
            step(&mut app, 0.0);
            for _ in 0..8 {
                step(&mut app, 0.5);
            }
            let health = app.world().get::<Health>(unit).unwrap().current;
            assert!((health - 300.0).abs() < 1e-2, "100 + 50 x 4s, got {health}");
            assert!(
                app.world()
                    .get::<StatusEffects>(unit)
                    .unwrap()
                    .effects
                    .is_empty()
            );
        }

        #[test]
        fn heal_over_time_respects_reduction_and_target_amp_at_tick() {
            let mut app = app();
            let unit = wounded(&mut app);
            app.world_mut()
                .get_mut::<CombatStats>(unit)
                .unwrap()
                .heal_received_amp = 0.5;
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_over_time(100.0, 4.0)),
            );
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_reduction(0.5, 10.0)),
            );
            step(&mut app, 0.0);
            step(&mut app, 1.0);
            let health = app.world().get::<Health>(unit).unwrap().current;
            assert!((health - 175.0).abs() < 1e-2, "100 + 100 x 1.5 x 0.5");
        }

        #[test]
        fn apply_heal_never_exceeds_max() {
            let mut h = Health {
                current: 999.0,
                max: 1000.0,
                regen_per_sec: 0.0,
            };
            assert_eq!(apply_heal(&mut h, 50.0, 1.0).applied, 1.0);
        }
    }

    mod shields {
        use super::*;
        use std::time::Duration;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<DamageEvent>()
                .add_systems(
                    Update,
                    (apply_status_effect_events, tick_status_effects).chain(),
                );
            app
        }

        fn unit(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    Health::new(1000.0),
                    ShieldPool { amount: 0.0 },
                ))
                .id()
        }

        fn send(app: &mut App, target: Entity, change: StatusChange) {
            app.world_mut().write_message(StatusEffectEvent {
                source: None,
                target,
                change,
            });
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn pool(app: &App, unit: Entity) -> f32 {
            app.world().get::<ShieldPool>(unit).unwrap().amount
        }

        fn shield_status(id: &'static str, amount: f32, duration: f32) -> StatusEffect {
            StatusEffect {
                id,
                ..StatusEffect::shield(amount, duration)
            }
        }

        /// What `combat::apply_damage_events` does on a hit that the pool soaks.
        fn absorb(app: &mut App, unit: Entity, amount: f32) {
            let mut e = app.world_mut().entity_mut(unit);
            e.get_mut::<ShieldPool>().unwrap().amount -= amount;
            consume_shields(&mut e.get_mut::<StatusEffects>().unwrap(), amount);
        }

        #[test]
        fn dispelling_a_shield_buff_clears_its_pool() {
            let mut app = app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("shield", 100.0, 10.0)),
            );
            step(&mut app, 0.1);
            assert_eq!(pool(&app, u), 100.0);
            send(
                &mut app,
                u,
                StatusChange::Dispel {
                    kind: StatusKind::Buff,
                    strength: DispelStrength::Strong,
                },
            );
            step(&mut app, 0.1);
            assert_eq!(pool(&app, u), 0.0);
        }

        #[test]
        fn removing_a_shield_by_id_clears_its_pool() {
            let mut app = app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("shield", 100.0, 10.0)),
            );
            step(&mut app, 0.1);
            send(&mut app, u, StatusChange::Remove("shield"));
            step(&mut app, 0.1);
            assert_eq!(pool(&app, u), 0.0);
            assert!(
                app.world()
                    .get::<StatusEffects>(u)
                    .unwrap()
                    .effects
                    .is_empty()
            );
        }

        #[test]
        fn partially_consumed_shield_expiry_leaves_other_shields_intact() {
            let mut app = app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("first", 100.0, 1.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("second", 80.0, 30.0)),
            );
            step(&mut app, 0.1);
            assert_eq!(pool(&app, u), 180.0);
            absorb(&mut app, u, 50.0);
            step(&mut app, 2.0);
            let statuses = app.world().get::<StatusEffects>(u).unwrap();
            assert_eq!(statuses.effects.len(), 1);
            assert_eq!(statuses.effects[0].id, "second");
            assert_eq!(
                pool(&app, u),
                80.0,
                "the second shield keeps its full amount"
            );
        }

        #[test]
        fn shields_are_consumed_oldest_first() {
            let mut statuses = StatusEffects::default();
            statuses.effects.push(shield_status("a", 30.0, 5.0));
            statuses.effects.push(shield_status("b", 40.0, 5.0));
            consume_shields(&mut statuses, 50.0);
            assert_eq!(statuses.effects[0].shield_amount, 0.0);
            assert_eq!(statuses.effects[1].shield_amount, 20.0);
        }

        #[test]
        fn refreshing_a_shield_replaces_only_its_remaining_amount() {
            let mut app = app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("shield", 100.0, 10.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("other", 60.0, 10.0)),
            );
            step(&mut app, 0.1);
            absorb(&mut app, u, 70.0);
            assert_eq!(pool(&app, u), 90.0);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("shield", 100.0, 10.0)),
            );
            step(&mut app, 0.1);
            // "shield" had 30 left and "other" 60: refreshed to 100 gives 100 + 60.
            assert_eq!(pool(&app, u), 160.0);
        }

        // --- End to end through `combat::apply_damage_events` ---

        fn damage_app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<DamageEvent>()
                .add_systems(
                    Update,
                    (
                        apply_status_effect_events,
                        crate::combat::apply_damage_events,
                        tick_status_effects,
                    )
                        .chain(),
                );
            app
        }

        fn hit(app: &mut App, unit: Entity, amount: f32) {
            app.world_mut()
                .write_message(DamageEvent::new(unit, amount, DamageType::Physical));
        }

        fn health(app: &App, unit: Entity) -> f32 {
            app.world().get::<Health>(unit).unwrap().current
        }

        /// `shield_amount` of every status that carries a shield, in vector order.
        fn remaining(app: &App, unit: Entity) -> Vec<(&'static str, f32)> {
            app.world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .effects
                .iter()
                .map(|e| (e.id, e.shield_amount))
                .collect()
        }

        #[test]
        fn damage_consumes_two_shields_oldest_first_and_health_is_untouched() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 180.0);

            hit(&mut app, u, 130.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 50.0);
            assert_eq!(remaining(&app, u), vec![("a", 0.0), ("b", 50.0)]);
            assert_eq!(health(&app, u), 1000.0);
        }

        #[test]
        fn overflow_damage_empties_every_shield_and_hurts_health_once() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 1.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);

            hit(&mut app, u, 200.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 0.0);
            assert_eq!(remaining(&app, u), vec![("a", 0.0), ("b", 0.0)]);
            assert_eq!(health(&app, u), 980.0);

            // Expiring a spent shield must not take anything out of the (empty) pool, and a new
            // shield applied afterwards is not shrunk by the old one.
            step(&mut app, 2.0);
            assert_eq!(pool(&app, u), 0.0);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("c", 40.0, 30.0)),
            );
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 40.0);
        }

        #[test]
        fn fully_consumed_shield_expiring_leaves_the_other_shield_intact() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 1.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);
            hit(&mut app, u, 120.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 60.0);

            step(&mut app, 2.0); // "a" (empty) expires
            assert_eq!(remaining(&app, u), vec![("b", 60.0)]);
            assert_eq!(pool(&app, u), 60.0);

            hit(&mut app, u, 100.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 0.0);
            assert_eq!(health(&app, u), 960.0);
        }

        #[test]
        fn expiring_the_second_shield_after_damage_only_releases_what_is_left() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 1.0)),
            );
            step(&mut app, 0.01);
            hit(&mut app, u, 110.0); // a: 0, b: 70
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 70.0);

            step(&mut app, 2.0); // b expires with 70 left
            assert_eq!(pool(&app, u), 0.0);
            hit(&mut app, u, 25.0);
            step(&mut app, 0.01);
            assert_eq!(health(&app, u), 975.0, "no shield is left to absorb");
        }

        #[test]
        fn dispelling_a_damaged_shield_releases_only_its_remainder() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            step(&mut app, 0.01);
            hit(&mut app, u, 30.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 70.0);

            send(
                &mut app,
                u,
                StatusChange::Dispel {
                    kind: StatusKind::Buff,
                    strength: DispelStrength::Strong,
                },
            );
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 0.0, "only the 70 that was left is released");
            hit(&mut app, u, 25.0);
            step(&mut app, 0.01);
            assert_eq!(
                health(&app, u),
                975.0,
                "a dispelled shield absorbs no further damage"
            );
        }

        #[test]
        fn removing_one_of_two_damaged_shields_keeps_the_other_absorbing() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);
            hit(&mut app, u, 130.0); // a: 0, b: 50
            step(&mut app, 0.01);

            send(&mut app, u, StatusChange::Remove("a"));
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 50.0, "a was spent, nothing to release");

            hit(&mut app, u, 70.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 0.0);
            assert_eq!(health(&app, u), 980.0);
            assert_eq!(remaining(&app, u), vec![("b", 0.0)]);
        }

        #[test]
        fn a_refreshed_shield_becomes_the_newest_and_is_consumed_last() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            step(&mut app, 0.01);
            assert_eq!(remaining(&app, u), vec![("b", 80.0), ("a", 100.0)]);
            assert_eq!(pool(&app, u), 180.0);

            hit(&mut app, u, 90.0);
            step(&mut app, 0.01);
            assert_eq!(remaining(&app, u), vec![("b", 0.0), ("a", 90.0)]);
            assert_eq!(pool(&app, u), 90.0);
        }

        #[test]
        fn each_absorbing_damage_event_is_charged_once() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);

            hit(&mut app, u, 40.0);
            hit(&mut app, u, 40.0);
            step(&mut app, 0.01);
            assert_eq!(remaining(&app, u), vec![("a", 20.0), ("b", 80.0)]);
            assert_eq!(pool(&app, u), 100.0);
            assert_eq!(health(&app, u), 1000.0);
        }

        #[test]
        fn shield_piercing_damage_does_not_consume_status_shields() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            step(&mut app, 0.01);
            app.world_mut().write_message(DamageEvent {
                pierces_shields: true,
                ..DamageEvent::new(u, 40.0, DamageType::Physical)
            });
            step(&mut app, 0.01);
            assert_eq!(remaining(&app, u), vec![("a", 100.0)]);
            assert_eq!(pool(&app, u), 100.0);
            assert_eq!(health(&app, u), 960.0);
        }

        #[test]
        fn status_plugin_ticks_statuses_without_any_other_plugin() {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<DamageEvent>()
                .add_plugins(StatusPlugin);
            let u = unit(&mut app);
            app.world_mut()
                .get_mut::<StatusEffects>(u)
                .unwrap()
                .effects
                .push(shield_status("a", 50.0, 1.0));
            app.world_mut().get_mut::<ShieldPool>(u).unwrap().amount = 50.0;
            step(&mut app, 0.5);
            assert_eq!(remaining(&app, u).len(), 1);
            step(&mut app, 0.6);
            assert!(remaining(&app, u).is_empty());
            assert_eq!(pool(&app, u), 0.0);
        }
    }

    #[test]
    fn leash_is_dispellable_and_blocked_by_debuff_immunity() {
        let leash = StatusEffect::leashed(
            Leash {
                anchor: LeashAnchor::Point(Vec2::ZERO),
                radius: 300.0,
            },
            5.0,
        );
        assert_eq!(leash.id, "leash");
        assert!(status_dispel_matches(
            &leash,
            StatusKind::Debuff,
            DispelStrength::Basic
        ));
        let tether = StatusEffect::leashed(
            Leash {
                anchor: LeashAnchor::Unit(Entity::PLACEHOLDER),
                radius: 300.0,
            },
            5.0,
        );
        assert_eq!(tether.id, "tether");

        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
        let mut statuses = StatusEffects::default();
        assert!(apply_status(&mut statuses, &mut stats, leash.clone(), 0.0));
        assert_eq!(statuses.leashes().count(), 1);
        remove_statuses(&mut statuses, &mut stats, 0.0, None, |e| {
            status_dispel_matches(e, StatusKind::Debuff, DispelStrength::Basic)
        });
        assert_eq!(
            statuses.leashes().count(),
            0,
            "a basic dispel frees the unit"
        );

        let mut immune = StatusEffects::default();
        assert!(apply_debuff_immunity(&mut immune, &mut stats, 5.0, 0.0));
        assert!(!apply_status(&mut immune, &mut stats, leash, 0.0));
        assert_eq!(immune.leashes().count(), 0);
    }
}
