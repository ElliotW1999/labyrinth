//! Health and mana rules shared by every heal, regeneration, leech and conversion path.
//!
//! This module owns the *rules* (heal factor, resource-modifier lookup, overheal clamp).
//! Effects only emit messages and `combat::apply_damage_events` stays the one place where
//! damage changes health; every health *increase* goes through [`apply_heal`].

// Some items are consumed only by later pipeline stages.
#![allow(dead_code)]

use bevy::prelude::*;

use crate::combat::DamageEvent;
use crate::components::{CombatStats, DamageType, Health, Mana};
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

/// `base x (1 + ManaCostAmp)`, never below zero (-0.25 = 25% cheaper).
pub fn effective_mana_cost(
    base: f32,
    stats: Option<&CombatStats>,
    statuses: Option<&StatusEffects>,
) -> f32 {
    base * (1.0 + resource_stat(stats, statuses, ResourceStat::ManaCostAmp)).max(0.0)
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

/// A resource leaving a unit: mana burn, mana drain, sacrifice, or conversion.
/// Emitted by effects, applied in [`apply_resource_drain_events`] before damage resolves.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ResourceDrainEvent {
    pub source: Option<Entity>,
    pub target: Entity,
    pub resource: ResourceKind,
    pub amount: f32,
    /// Mana only: `ratio x removed` is dealt to the target as spell damage (mana burn).
    pub burn: Option<(f32, DamageType)>,
    /// Mana only: `ratio x removed` mana is given to the source (mana drain).
    pub to_source: Option<f32>,
    /// `ratio x removed` of another resource is given to the target (conversion).
    pub convert_to: Option<(ResourceKind, f32)>,
}

/// Remove resources and fan the result out. Spending health is non-lethal (floors at 1) and
/// bypasses damage entirely (no shields, reflection or leech). Hostile drains are blocked
/// only by invulnerability.
pub fn apply_resource_drain_events(
    mut events: MessageReader<ResourceDrainEvent>,
    mut units: Query<(
        &mut Health,
        Option<&mut Mana>,
        Option<&CombatStats>,
        Option<&StatusEffects>,
    )>,
    mut damage: MessageWriter<DamageEvent>,
) {
    for event in events.read() {
        let hostile = event.source != Some(event.target);
        let removed = {
            let Ok((mut health, mut mana, _, statuses)) = units.get_mut(event.target) else {
                continue;
            };
            if !health.is_alive() || (hostile && statuses.is_some_and(|s| s.is_invulnerable())) {
                continue;
            }
            let wanted = event.amount.max(0.0);
            match event.resource {
                ResourceKind::Health => {
                    let removed = wanted.min((health.current - 1.0).max(0.0));
                    health.current -= removed;
                    removed
                }
                ResourceKind::Mana => {
                    let Some(mana) = mana.as_deref_mut() else {
                        continue;
                    };
                    let removed = wanted.min(mana.current.max(0.0));
                    mana.current -= removed;
                    removed
                }
            }
        };
        if removed <= 0.0 {
            continue;
        }
        if let (Some((ratio, damage_type)), Some(source)) = (event.burn, event.source) {
            if ratio > 0.0 {
                damage.write(DamageEvent::spell(
                    source,
                    event.target,
                    removed * ratio,
                    damage_type,
                ));
            }
        }
        if let (Some(ratio), Some(source)) = (event.to_source, event.source) {
            if let Ok((_, Some(mut mana), _, _)) = units.get_mut(source) {
                if ratio > 0.0 {
                    mana.current = (mana.current + removed * ratio).min(mana.max);
                }
            }
        }
        if let Some((kind, ratio)) = event.convert_to {
            let gained = removed * ratio;
            if let Ok((mut health, mut mana, stats, statuses)) = units.get_mut(event.target) {
                match kind {
                    ResourceKind::Health => {
                        let amp = outgoing_heal_amp(stats, statuses);
                        let factor = heal_factor(HealKind::Conversion, amp, stats, statuses);
                        apply_heal(&mut health, gained, factor);
                    }
                    ResourceKind::Mana => {
                        if let Some(mana) = mana.as_deref_mut() {
                            mana.current = (mana.current + gained).min(mana.max);
                        }
                    }
                }
            }
        }
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

    mod drain {
        use super::*;
        use bevy::ecs::system::RunSystemOnce;

        fn world() -> World {
            let mut world = World::new();
            world.init_resource::<Messages<ResourceDrainEvent>>();
            world.init_resource::<Messages<DamageEvent>>();
            world
        }

        fn unit(world: &mut World, hp: f32, mana: f32, max_mana: f32) -> Entity {
            world
                .spawn((
                    Health {
                        current: hp,
                        max: 1000.0,
                        regen_per_sec: 0.0,
                    },
                    Mana {
                        current: mana,
                        max: max_mana,
                        regen_per_sec: 0.0,
                    },
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0),
                    StatusEffects::default(),
                ))
                .id()
        }

        fn event(
            source: Entity,
            target: Entity,
            resource: ResourceKind,
            amount: f32,
        ) -> ResourceDrainEvent {
            ResourceDrainEvent {
                source: Some(source),
                target,
                resource,
                amount,
                burn: None,
                to_source: None,
                convert_to: None,
            }
        }

        fn run(world: &mut World) {
            world.run_system_once(apply_resource_drain_events).unwrap();
        }

        fn mana(world: &World, e: Entity) -> f32 {
            world.get::<Mana>(e).unwrap().current
        }

        fn hp(world: &World, e: Entity) -> f32 {
            world.get::<Health>(e).unwrap().current
        }

        #[test]
        fn mana_drain_transfers_to_caster_clamped_to_max() {
            let mut world = world();
            let caster = unit(&mut world, 1000.0, 90.0, 100.0);
            let victim = unit(&mut world, 1000.0, 50.0, 100.0);
            world.write_message(ResourceDrainEvent {
                to_source: Some(1.0),
                ..event(caster, victim, ResourceKind::Mana, 40.0)
            });
            run(&mut world);
            assert_eq!(mana(&world, victim), 10.0);
            assert_eq!(mana(&world, caster), 100.0, "90 + 40 clamps at max");
        }

        #[test]
        fn mana_drain_is_capped_by_what_the_target_has() {
            let mut world = world();
            let caster = unit(&mut world, 1000.0, 0.0, 100.0);
            let victim = unit(&mut world, 1000.0, 15.0, 100.0);
            world.write_message(ResourceDrainEvent {
                to_source: Some(0.5),
                ..event(caster, victim, ResourceKind::Mana, 40.0)
            });
            run(&mut world);
            assert_eq!(mana(&world, victim), 0.0);
            assert_eq!(mana(&world, caster), 7.5);
        }

        #[test]
        fn spend_health_is_non_lethal() {
            let mut world = world();
            let unit = unit(&mut world, 100.0, 0.0, 100.0);
            world.write_message(event(unit, unit, ResourceKind::Health, 60.0));
            run(&mut world);
            assert_eq!(hp(&world, unit), 40.0);
            world.insert_resource(Messages::<ResourceDrainEvent>::default());
            world.write_message(event(unit, unit, ResourceKind::Health, 500.0));
            run(&mut world);
            assert_eq!(hp(&world, unit), 1.0, "floors at 1 hp");
        }

        #[test]
        fn health_to_mana_conversion_uses_spent_amount() {
            let mut world = world();
            let unit = unit(&mut world, 300.0, 0.0, 100.0);
            world.write_message(ResourceDrainEvent {
                convert_to: Some((ResourceKind::Mana, 0.5)),
                ..event(unit, unit, ResourceKind::Health, 80.0)
            });
            run(&mut world);
            assert_eq!(hp(&world, unit), 220.0);
            assert_eq!(mana(&world, unit), 40.0);
        }

        #[test]
        fn mana_to_health_conversion_respects_heal_block() {
            let mut world = world();
            let unit = unit(&mut world, 300.0, 100.0, 100.0);
            world
                .get_mut::<StatusEffects>(unit)
                .unwrap()
                .effects
                .push(StatusEffect::heal_block(5.0));
            world.write_message(ResourceDrainEvent {
                convert_to: Some((ResourceKind::Health, 2.0)),
                ..event(unit, unit, ResourceKind::Mana, 50.0)
            });
            run(&mut world);
            assert_eq!(mana(&world, unit), 50.0, "mana was still spent");
            assert_eq!(
                hp(&world, unit),
                300.0,
                "heal block prevents the conversion"
            );
        }

        #[test]
        fn invulnerable_target_ignores_hostile_mana_drain() {
            let mut world = world();
            let caster = unit(&mut world, 1000.0, 0.0, 100.0);
            let victim = unit(&mut world, 1000.0, 50.0, 100.0);
            world
                .get_mut::<StatusEffects>(victim)
                .unwrap()
                .effects
                .push(StatusEffect::invulnerable(5.0));
            world.write_message(ResourceDrainEvent {
                to_source: Some(1.0),
                burn: Some((1.0, DamageType::Magical)),
                ..event(caster, victim, ResourceKind::Mana, 40.0)
            });
            run(&mut world);
            assert_eq!(mana(&world, victim), 50.0);
            assert_eq!(mana(&world, caster), 0.0);
            assert!(world.resource::<Messages<DamageEvent>>().is_empty());
        }

        #[test]
        fn dead_targets_are_ignored() {
            let mut world = world();
            let caster = unit(&mut world, 1000.0, 0.0, 100.0);
            let victim = unit(&mut world, 0.0, 50.0, 100.0);
            world.write_message(event(caster, victim, ResourceKind::Mana, 40.0));
            run(&mut world);
            assert_eq!(mana(&world, victim), 50.0);
        }
    }

    #[test]
    fn plain_factor_is_one() {
        assert_eq!(heal_factor(HealKind::Direct, 0.0, None, None), 1.0);
    }
}
