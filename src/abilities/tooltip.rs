//! Hover text for an [`AbilityDefinition`](super::definition::AbilityDefinition).
//!
//! Per-rank numbers are separate spans so the HUD can paint learned ranks yellow.

use super::definition::{
    AbilityBehavior, AbilityDefinition, AbilityEffect, AbilityMechanic, AbilityTrigger,
    Attribute, DisplacementKind, EffectScaling, RankValue, StatusSpec, TargetTeam, TargetType,
};
use crate::components::DamageType;
use crate::items::StatusKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipTone {
    Plain,
    /// A rank the hero has already learned.
    Learned,
    /// A rank past the current level.
    Unlearned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TipBit {
    pub text: String,
    pub tone: TipTone,
}

/// Flat span list, including newlines. `learned_rank` is how many ranks are trained
/// (0 = unlearned). Each per-level number at index `i` (0-based) is [`TipTone::Learned`]
/// when `i < learned_rank`.
pub fn ability_tooltip_bits(def: &AbilityDefinition, learned_rank: u32) -> Vec<TipBit> {
    let mut bits = Vec::new();
    plain(&mut bits, format!("{}\n", def.name));
    plain(&mut bits, format!("{}\n", kind_line(def)));
    if !def.description.is_empty() {
        plain(&mut bits, format!("{}\n", def.description));
    }
    plain(&mut bits, "\n");

    let max_rank = def.max_rank.max(1);
    let levels: Vec<f32> = (1..=max_rank).map(|rank| rank as f32).collect();
    push_series(&mut bits, "Level", &levels, learned_rank);

    push_value(&mut bits, "Cast range", def.cast_range, max_rank, learned_rank);
    push_value(&mut bits, "Radius", def.aoe_radius, max_rank, learned_rank);
    push_value(&mut bits, "Cooldown", def.cooldown, max_rank, learned_rank);
    push_value(&mut bits, "Mana", def.mana_cost, max_rank, learned_rank);
    if def.cast_time > 0.0 {
        push_series(&mut bits, "Cast time", &[def.cast_time], learned_rank);
    }
    if def.channel_time > 0.0 {
        push_series(&mut bits, "Channel", &[def.channel_time], learned_rank);
    }
    if let Some((charges, restore)) = def.charges {
        plain(
            &mut bits,
            format!("Charges: {charges}  (restore {restore:.0}s)\n"),
        );
    }

    for entry in &def.effects {
        for (label, value) in effect_rows(entry.trigger, &entry.effect) {
            push_value(&mut bits, &label, value, max_rank, learned_rank);
        }
        for note in effect_notes(entry.trigger, &entry.effect) {
            plain(&mut bits, format!("{note}\n"));
        }
    }
    bits
}

fn kind_line(def: &AbilityDefinition) -> String {
    let mut parts = vec![
        match def.behavior {
            AbilityBehavior::Active => "Active",
            AbilityBehavior::Passive => "Passive",
            AbilityBehavior::Toggle => "Toggle",
        },
        match def.target_type {
            TargetType::NoTarget => "No target",
            TargetType::Unit => "Unit target",
            TargetType::Point => "Point target",
            TargetType::Area => "Area target",
        },
        match def.target_team {
            TargetTeam::Enemy => "Enemies",
            TargetTeam::Ally => "Allies",
            TargetTeam::Any => "Any unit",
        },
    ];
    if def.is_ultimate {
        parts.push("Ultimate");
    }
    for mechanic in &def.mechanics {
        let tag = match mechanic {
            AbilityMechanic::Dash { .. } => "Dash",
            AbilityMechanic::Projectile { skillshot: true, .. } => "Skillshot",
            AbilityMechanic::Projectile { .. } => "Projectile",
            AbilityMechanic::AttackUnitTarget => "Attack",
            AbilityMechanic::RingFx { .. } => continue,
        };
        if !parts.contains(&tag) {
            parts.push(tag);
        }
    }
    parts.join(" · ")
}

fn push_value(bits: &mut Vec<TipBit>, label: &str, value: RankValue, max_rank: u32, learned_rank: u32) {
    let series = series_of(value, max_rank);
    if series.iter().all(|v| v.abs() < 0.05) {
        return;
    }
    push_series(bits, label, &series, learned_rank);
}

fn series_of(value: RankValue, max_rank: u32) -> Vec<f32> {
    let max_rank = max_rank.max(1);
    let values: Vec<f32> = (1..=max_rank).map(|rank| value.at(rank)).collect();
    let first = values[0];
    if values.iter().all(|v| (v - first).abs() < 0.05) {
        vec![first]
    } else {
        values
    }
}

fn push_series(bits: &mut Vec<TipBit>, label: &str, values: &[f32], learned_rank: u32) {
    plain(bits, format!("{label}: "));
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            plain(bits, " / ");
        }
        let learned = (index as u32) < learned_rank;
        bits.push(TipBit {
            text: fmt_num(*value),
            tone: if learned {
                TipTone::Learned
            } else {
                TipTone::Unlearned
            },
        });
    }
    plain(bits, "\n");
}

fn plain(bits: &mut Vec<TipBit>, text: impl Into<String>) {
    bits.push(TipBit {
        text: text.into(),
        tone: TipTone::Plain,
    });
}

fn fmt_num(value: f32) -> String {
    if !value.is_finite() {
        return "0".into();
    }
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i32)
    } else {
        format!("{value:.1}")
    }
}

fn trigger_tag(trigger: AbilityTrigger) -> &'static str {
    match trigger {
        AbilityTrigger::OnCast => "on cast",
        AbilityTrigger::OnProjectileHit => "on hit",
        AbilityTrigger::OnUnitContact => "on contact",
        AbilityTrigger::OnImpact => "on impact",
        AbilityTrigger::OnChannelTick => "while channelling",
        AbilityTrigger::OnChannelEnd => "channel end",
        AbilityTrigger::OnExpire => "on expire",
        AbilityTrigger::Passive => "passive",
        AbilityTrigger::OnUse => "use",
        AbilityTrigger::OnAttack => "on attack",
        AbilityTrigger::OnAttackHit => "on attack hit",
        AbilityTrigger::OnDamageTaken => "when damaged",
        AbilityTrigger::OnKill => "on kill",
        AbilityTrigger::OnDeath => "on death",
        AbilityTrigger::Custom(_) => "custom",
    }
}

fn effect_rows(trigger: AbilityTrigger, effect: &AbilityEffect) -> Vec<(String, RankValue)> {
    let tag = trigger_tag(trigger);
    match *effect {
        AbilityEffect::Damage { amount, damage_type, .. } => {
            vec![(format!("{} damage ({tag})", damage_name(damage_type)), amount)]
        }
        AbilityEffect::Heal { amount, .. } => vec![(format!("Heal ({tag})"), amount)],
        AbilityEffect::RestoreMana { amount } => vec![(format!("Mana restore ({tag})"), amount)],
        AbilityEffect::ApplyStatus { status, duration } => status_rows(tag, status, duration),
        AbilityEffect::Displace { kind, distance, duration } => {
            let name = match kind {
                DisplacementKind::Knockback => "Knockback",
                DisplacementKind::Pull => "Pull",
            };
            let mut rows = Vec::new();
            if let Some(distance) = distance {
                rows.push((format!("{name} distance ({tag})"), distance));
            } else {
                rows.push((format!("{name} ({tag})"), RankValue::ZERO));
            }
            rows.push((format!("{name} duration ({tag})"), duration));
            rows
        }
        AbilityEffect::PassiveStat { stat, amount } => {
            vec![(format!("{} ({tag})", passive_name(stat)), amount)]
        }
        AbilityEffect::Custom { id, value, amount, duration } => {
            let mut rows = Vec::new();
            if value != RankValue::ZERO {
                rows.push((format!("{id} ({tag})"), value));
            }
            if amount != RankValue::ZERO {
                rows.push((format!("{id} amount ({tag})"), amount));
            }
            if duration != RankValue::ZERO {
                rows.push((format!("{id} duration ({tag})"), duration));
            }
            rows
        }
        AbilityEffect::Dispel { .. } | AbilityEffect::RemoveStatus { .. } => Vec::new(),
    }
}

fn status_rows(tag: &str, status: StatusSpec, duration: RankValue) -> Vec<(String, RankValue)> {
    match status {
        StatusSpec::Slow { percent } => vec![
            (format!("Slow % ({tag})"), percent),
            (format!("Slow duration ({tag})"), duration),
        ],
        StatusSpec::Modifier {
            armor,
            magic_resist,
            move_speed,
            attack_damage,
            ..
        } => {
            let mut rows = Vec::new();
            if armor != RankValue::ZERO {
                rows.push((format!("Armor ({tag})"), armor));
            }
            if magic_resist != RankValue::ZERO {
                rows.push((format!("Magic resist ({tag})"), magic_resist));
            }
            if move_speed != RankValue::ZERO {
                rows.push((format!("Move speed ({tag})"), move_speed));
            }
            if attack_damage != RankValue::ZERO {
                rows.push((format!("Attack damage ({tag})"), attack_damage));
            }
            rows.push((format!("Duration ({tag})"), duration));
            rows
        }
        other => vec![(format!("{} ({tag})", status_name(other)), duration)],
    }
}

fn effect_notes(trigger: AbilityTrigger, effect: &AbilityEffect) -> Vec<String> {
    let tag = trigger_tag(trigger);
    match *effect {
        AbilityEffect::Damage { scaling, .. } | AbilityEffect::Heal { scaling, .. } => {
            scaling_note(scaling).into_iter().collect()
        }
        AbilityEffect::Dispel { kind } => match kind {
            StatusKind::Buff => vec![format!("Dispel buffs ({tag})")],
            StatusKind::Debuff => vec![format!("Dispel debuffs ({tag})")],
        },
        AbilityEffect::RemoveStatus { id } => vec![format!("Remove {id} ({tag})")],
        AbilityEffect::Displace { distance: None, kind, .. } => match kind {
            DisplacementKind::Pull => vec![format!("Pulls to the caster ({tag})")],
            DisplacementKind::Knockback => Vec::new(),
        },
        AbilityEffect::Custom { id, value, amount, duration }
            if value == RankValue::ZERO && amount == RankValue::ZERO && duration == RankValue::ZERO =>
        {
            vec![format!("{id} ({tag})")]
        }
        _ => Vec::new(),
    }
}

fn scaling_note(scaling: EffectScaling) -> Option<String> {
    match scaling {
        EffectScaling::None => None,
        EffectScaling::CasterAttackDamage(ratio) => {
            Some(format!("+ {ratio:.2}× attack damage"))
        }
        EffectScaling::CasterAttribute(attribute, ratio) => {
            Some(format!("+ {ratio:.2}× {}", attribute_name(attribute)))
        }
        EffectScaling::TargetHealthBelow(fraction) => {
            Some(format!("Only below {:.0}% health", fraction * 100.0))
        }
    }
}

fn damage_name(damage_type: DamageType) -> &'static str {
    match damage_type {
        DamageType::Physical => "Physical",
        DamageType::Magical => "Magical",
        DamageType::Pure => "Pure",
    }
}

fn status_name(status: StatusSpec) -> &'static str {
    match status {
        StatusSpec::Stun => "Stun",
        StatusSpec::Root => "Root",
        StatusSpec::Silence => "Silence",
        StatusSpec::Disarm => "Disarm",
        StatusSpec::Phased => "Phased",
        StatusSpec::Forceful => "Forceful",
        StatusSpec::DebuffImmunity => "Debuff immunity",
        StatusSpec::Slow { .. } => "Slow",
        StatusSpec::Modifier { .. } => "Modifier",
    }
}

fn passive_name(stat: super::definition::PassiveStat) -> &'static str {
    use super::definition::PassiveStat;
    match stat {
        PassiveStat::MaxHealth => "Health",
        PassiveStat::HealthRegen => "Health regen",
        PassiveStat::MaxMana => "Mana",
        PassiveStat::ManaRegen => "Mana regen",
        PassiveStat::AttackDamage => "Attack damage",
        PassiveStat::AttackSpeed => "Attack speed",
        PassiveStat::Armor => "Armor",
        PassiveStat::MagicResist => "Magic resist",
        PassiveStat::MoveSpeed => "Move speed",
        PassiveStat::Attribute(attribute) => attribute_name(attribute),
    }
}

fn attribute_name(attribute: Attribute) -> &'static str {
    match attribute {
        Attribute::Strength => "Strength",
        Attribute::Agility => "Agility",
        Attribute::Intelligence => "Intelligence",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::catalog::builtin;
    use crate::components::AbilityId;

    fn row_values(bits: &[TipBit], label: &str) -> Vec<(String, bool)> {
        let mut values = Vec::new();
        let mut on_row = false;
        for bit in bits {
            if bit.text.starts_with(&format!("{label}:")) {
                on_row = true;
                continue;
            }
            if !on_row {
                continue;
            }
            if bit.text.contains('\n') {
                break;
            }
            if bit.text.trim() == "/" {
                continue;
            }
            values.push((bit.text.clone(), bit.tone == TipTone::Learned));
        }
        values
    }

    #[test]
    fn learned_ranks_are_yellow_on_every_per_level_row() {
        let slam = builtin(AbilityId::SeismicSlam);
        let bits = ability_tooltip_bits(&slam, 2);
        let text: String = bits.iter().map(|bit| bit.text.as_str()).collect();
        assert!(text.contains("Seismic Slam"));
        assert!(text.contains("Active · No target · Enemies"));
        assert!(text.contains("Slams the ground"));

        let levels = row_values(&bits, "Level");
        assert_eq!(levels.len(), 7);
        assert!(levels[0].1 && levels[1].1);
        assert!(levels[2..].iter().all(|(_, learned)| !learned));

        let damage = row_values(&bits, "Physical damage (on cast)");
        assert!(damage.len() > 2, "{damage:?}");
        assert_eq!(damage[0].0, "80");
        assert!(damage[0].1 && damage[1].1);
        assert!(!damage[2].1);

        let slow = row_values(&bits, "Slow % (on cast)");
        assert_eq!(slow, vec![("30".into(), true)]);
    }

    #[test]
    fn unlearned_ability_leaves_every_rank_unlearned() {
        let slam = builtin(AbilityId::SeismicSlam);
        let bits = ability_tooltip_bits(&slam, 0);
        let levels = row_values(&bits, "Level");
        assert!(levels.iter().all(|(_, learned)| !learned));
    }
}
