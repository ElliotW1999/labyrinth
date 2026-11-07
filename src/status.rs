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
            tick_status_effects.before(crate::combat::apply_damage_events),
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
    /// 0..1 miss chance on basic attack damage resolution (`0` = no blind).
    pub blind_miss_chance: f32,
    pub heal_per_sec: f32,
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
            blind_miss_chance: 0.0,
            heal_per_sec: 0.0,
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
            fear_source: None,
        }
    }

    pub fn shield(amount: f32, duration: f32) -> Self {
        Self {
            shield_amount: amount,
            ..Self::buff("shield", duration)
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
    pub fn is_phased(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.ignore_unit_collision && e.remaining > 0.0)
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
        let mut i = 0;
        while i < statuses.effects.len() {
            let effect = &mut statuses.effects[i];
            if effect.heal_per_sec > 0.0 {
                health.current = (health.current + effect.heal_per_sec * dt).min(health.max);
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
                if expired.shield_amount > 0.0 {
                    if let Some(pool) = shield.as_deref_mut() {
                        pool.amount = (pool.amount - expired.shield_amount).max(0.0);
                    }
                }
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

fn remove_statuses(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    agility: f32,
    matches: impl Fn(&StatusEffect) -> bool,
) {
    statuses.effects.retain(|effect| {
        if !matches(effect) {
            return true;
        }
        subtract_status_stat_delta(effect, stats, agility);
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
                    remove_statuses(&mut statuses, &mut stats, agility, |e| {
                        e.kind == StatusKind::Debuff
                            && status_dispel_matches(e, StatusKind::Debuff, DispelStrength::Strong)
                    });
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
                let _ = apply_status(&mut statuses, &mut stats, effect.clone(), agility);
                if effect.shield_amount > 0.0 {
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
                remove_statuses(&mut statuses, &mut stats, agility, |e| {
                    status_dispel_matches(e, *kind, *strength)
                });
            }
            StatusChange::Remove(id) => {
                remove_statuses(&mut statuses, &mut stats, agility, |e| e.id == *id);
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
        app.add_message::<StatusEffectEvent>().add_systems(
            Update,
            crate::abilities::effects::apply_status_effect_events,
        );
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
}
