//! Health and mana rules shared by every heal, regeneration, leech and conversion path.
//!
//! This module owns the *rules* (heal factor, resource-modifier lookup, overheal clamp).
//! Effects only emit messages and `combat::apply_damage_events` stays the one place where
//! damage changes health; every health *increase* goes through [`apply_heal`].

// Some items are consumed only by later pipeline stages.
#![allow(dead_code)]

use bevy::prelude::*;

use crate::components::{CombatStats, Health, Mana};
use crate::status::StatusEffects;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Health,
    Mana,
}

/// Timed (status) or permanent (`CombatStats`) resource modifiers. Amps and ratios are fractions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceStat {
    Lifesteal,
    SpellLifesteal,
    /// Healing provided.
    HealAmp,
    /// Healing received.
    HealReceivedAmp,
    HealthRegen,
    ManaRegen,
    MaxHealth,
    MaxMana,
    /// -0.25 means 25% cheaper.
    ManaCostAmp,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ManaShield {
    /// Fraction (0..1) of post-mitigation damage diverted to mana.
    pub absorb: f32,
    /// Damage absorbed per point of mana spent (> 0).
    pub damage_per_mana: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealKind {
    Direct,
    OverTime,
    Leech,
    Regen,
    Conversion,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HealOutcome {
    pub applied: f32,
    pub overheal: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxChangeRule {
    /// Existing item rule: a gain also adds to current, a loss only clamps.
    KeepMissing,
    /// Current keeps its fraction of max.
    PreserveRatio,
}

/// Permanent (`CombatStats`) plus the sum of active statuses carrying `stat`.
/// The one lookup for every resource modifier.
pub fn resource_stat(
    stats: Option<&CombatStats>,
    statuses: Option<&StatusEffects>,
    stat: ResourceStat,
) -> f32 {
    let permanent = stats
        .map(|s| match stat {
            ResourceStat::Lifesteal => s.lifesteal,
            ResourceStat::SpellLifesteal => s.spell_lifesteal,
            ResourceStat::HealAmp => s.heal_amp,
            ResourceStat::HealReceivedAmp => s.heal_received_amp,
            ResourceStat::ManaCostAmp => s.mana_cost_amp,
            // Regen and max live on `Health`/`Mana`, not on `CombatStats`.
            ResourceStat::HealthRegen
            | ResourceStat::ManaRegen
            | ResourceStat::MaxHealth
            | ResourceStat::MaxMana => 0.0,
        })
        .unwrap_or(0.0);
    permanent + statuses.map(|s| s.resource_sum(stat)).unwrap_or(0.0)
}

/// Display name for tooltips.
pub fn stat_label(stat: ResourceStat) -> &'static str {
    match stat {
        ResourceStat::Lifesteal => "Lifesteal",
        ResourceStat::SpellLifesteal => "Spell lifesteal",
        ResourceStat::HealAmp => "Heal amp",
        ResourceStat::HealReceivedAmp => "Heal received amp",
        ResourceStat::HealthRegen => "Health regen",
        ResourceStat::ManaRegen => "Mana regen",
        ResourceStat::MaxHealth => "Max health",
        ResourceStat::MaxMana => "Max mana",
        ResourceStat::ManaCostAmp => "Mana cost",
    }
}

/// Healing the unit provides (additive bucket).
pub fn outgoing_heal_amp(stats: Option<&CombatStats>, statuses: Option<&StatusEffects>) -> f32 {
    resource_stat(stats, statuses, ResourceStat::HealAmp)
}

/// Multiplier applied to a raw heal on the target.
///
/// Order: prevention, then amplification (additive: healer's amp + target's received amp,
/// none for regeneration), then the strongest reduction (multiplicative).
pub fn heal_factor(
    kind: HealKind,
    outgoing_amp: f32,
    target_stats: Option<&CombatStats>,
    target_statuses: Option<&StatusEffects>,
) -> f32 {
    if target_statuses.is_some_and(|s| s.is_heal_blocked()) {
        return 0.0;
    }
    let amp = if kind == HealKind::Regen {
        0.0
    } else {
        outgoing_amp + resource_stat(target_stats, target_statuses, ResourceStat::HealReceivedAmp)
    };
    let reduction = target_statuses
        .map(|s| s.heal_reduction())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    (1.0 + amp).max(0.0) * (1.0 - reduction)
}

/// The only place health is increased. Dead units are never healed.
pub fn apply_heal(health: &mut Health, raw: f32, factor: f32) -> HealOutcome {
    if !health.is_alive() || raw <= 0.0 || factor <= 0.0 {
        return HealOutcome::default();
    }
    let amount = raw * factor;
    let applied = amount.min(health.max - health.current).max(0.0);
    health.current += applied;
    HealOutcome {
        applied,
        overheal: amount - applied,
    }
}

/// Timed max-health / max-mana modifiers already folded into `Health::max` / `Mana::max`.
/// Inserted lazily by [`sync_status_resource_caps`].
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct AppliedStatusCaps {
    pub health: f32,
    pub mana: f32,
}

/// Reconcile `Health::max` / `Mana::max` with the timed max modifiers every frame.
/// Applies the difference from what was last folded in using [`MaxChangeRule::PreserveRatio`],
/// so no removal path (expiry, dispel, refresh, immunity purge) can leak or lose max.
pub fn sync_status_resource_caps(
    mut commands: Commands,
    mut units: Query<(
        Entity,
        &mut Health,
        Option<&mut Mana>,
        &StatusEffects,
        Option<&mut AppliedStatusCaps>,
    )>,
) {
    for (entity, mut health, mana, statuses, applied) in &mut units {
        let want_health = statuses.resource_sum(ResourceStat::MaxHealth);
        let want_mana = statuses.resource_sum(ResourceStat::MaxMana);
        let Some(mut applied) = applied else {
            if want_health == 0.0 && want_mana == 0.0 {
                continue;
            }
            let mut caps = AppliedStatusCaps::default();
            fold_caps(&mut caps, &mut health, mana, want_health, want_mana);
            commands.entity(entity).insert(caps);
            continue;
        };
        fold_caps(&mut applied, &mut health, mana, want_health, want_mana);
    }
}

fn fold_caps(
    applied: &mut AppliedStatusCaps,
    health: &mut Health,
    mana: Option<Mut<Mana>>,
    want_health: f32,
    want_mana: f32,
) {
    let d_health = want_health - applied.health;
    if d_health != 0.0 {
        health.change_max(d_health, MaxChangeRule::PreserveRatio);
        applied.health = want_health;
    }
    let d_mana = want_mana - applied.mana;
    if d_mana != 0.0 {
        if let Some(mut mana) = mana {
            mana.change_max(d_mana, MaxChangeRule::PreserveRatio);
        }
        applied.mana = want_mana;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::StatusEffect;

    fn hp(current: f32, max: f32) -> Health {
        Health {
            current,
            max,
            regen_per_sec: 0.0,
        }
    }

    #[test]
    fn heal_does_not_revive_dead_units() {
        let mut dead = hp(0.0, 100.0);
        let out = apply_heal(&mut dead, 50.0, 1.0);
        assert_eq!(out, HealOutcome::default());
        assert_eq!(dead.current, 0.0);
    }

    #[test]
    fn heal_clamps_at_max_and_reports_overheal() {
        let mut h = hp(90.0, 100.0);
        let out = apply_heal(&mut h, 30.0, 1.0);
        assert_eq!(h.current, 100.0);
        assert_eq!(out.applied, 10.0);
        assert_eq!(out.overheal, 20.0);
    }

    #[test]
    fn heal_block_is_factor_zero() {
        let mut statuses = StatusEffects::default();
        statuses.effects.push(StatusEffect {
            heal_blocked: true,
            ..StatusEffect::debuff("heal_block", 5.0)
        });
        assert_eq!(
            heal_factor(HealKind::Direct, 1.0, None, Some(&statuses)),
            0.0
        );
        let mut h = hp(10.0, 100.0);
        apply_heal(&mut h, 50.0, 0.0);
        assert_eq!(h.current, 10.0);
    }

    fn with_status(effect: StatusEffect) -> StatusEffects {
        let mut statuses = StatusEffects::default();
        statuses.effects.push(effect);
        statuses
    }

    #[test]
    fn heal_amp_outgoing_and_incoming_add() {
        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
        stats.heal_received_amp = 0.25;
        let factor = heal_factor(HealKind::Direct, 0.5, Some(&stats), None);
        assert!(
            (factor - 1.75).abs() < 1e-6,
            "1 + 0.5 + 0.25, not 1.5 x 1.25"
        );
    }

    #[test]
    fn timed_heal_amp_is_summed_with_permanent() {
        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
        stats.heal_amp = 0.1;
        let statuses = with_status(StatusEffect::resource_modifier(
            "amp",
            crate::status::StatusKind::Buff,
            ResourceStat::HealAmp,
            0.2,
            5.0,
        ));
        let amp = outgoing_heal_amp(Some(&stats), Some(&statuses));
        assert!((amp - 0.3).abs() < 1e-6);
    }

    #[test]
    fn strongest_heal_reduction_wins_and_multiplies_after_amp() {
        let mut statuses = with_status(StatusEffect::heal_reduction(0.25, 5.0));
        statuses.effects.push(StatusEffect {
            id: "other_reduction",
            ..StatusEffect::heal_reduction(0.5, 5.0)
        });
        assert_eq!(statuses.heal_reduction(), 0.5, "they do not stack");
        let factor = heal_factor(HealKind::Direct, 1.0, None, Some(&statuses));
        assert!((factor - 1.0).abs() < 1e-6, "(1 + 1.0) x (1 - 0.5)");
    }

    #[test]
    fn regen_ignores_amp_but_respects_reduction() {
        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
        stats.heal_received_amp = 1.0;
        let statuses = with_status(StatusEffect::heal_reduction(0.5, 5.0));
        let regen = heal_factor(HealKind::Regen, 1.0, Some(&stats), Some(&statuses));
        let direct = heal_factor(HealKind::Direct, 0.0, Some(&stats), Some(&statuses));
        assert!((regen - 0.5).abs() < 1e-6);
        assert!((direct - 1.0).abs() < 1e-6, "(1 + 1.0) x 0.5");
    }

    #[test]
    fn plain_factor_is_one() {
        assert_eq!(heal_factor(HealKind::Direct, 0.0, None, None), 1.0);
    }
}
