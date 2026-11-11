//! Hover text for an [`AbilityDefinition`](super::definition::AbilityDefinition).
//!
//! Per-rank numbers are separate spans so the HUD can paint learned ranks yellow.

use super::definition::{
    AbilityBehavior, AbilityDefinition, AbilityEffect, AbilityEffectEntry, AbilityMechanic,
    AbilityTrigger, AreaRadius, Attribute, DisplacementKind, EffectScaling, EffectTarget,
    RankValue, StatusSpec, TargetTeam, TargetType,
};
use crate::components::DamageType;
use crate::status::StatusKind;

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

    push_value(
        &mut bits,
        "Cast range",
        def.cast_range,
        max_rank,
        learned_rank,
    );
    push_value(
        &mut bits,
        "Cast range buffer",
        def.cast_range_buffer,
        max_rank,
        learned_rank,
    );
    push_value(&mut bits, "Radius", def.aoe_radius, max_rank, learned_rank);
    for mechanic in &def.mechanics {
        if let AbilityMechanic::Projectile {
            skillshot: true,
            end_radius: Some(end_radius),
            ..
        } = mechanic
        {
            push_value(&mut bits, "End radius", *end_radius, max_rank, learned_rank);
        }
    }
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
        let tag = trigger_tag(entry.trigger);
        let (rows, notes) = describe_effect(entry);
        for row in rows {
            let label = format!("{} ({tag})", row.label);
            match row.value {
                Some(value) => push_value(&mut bits, &label, value, max_rank, learned_rank),
                None => plain(&mut bits, format!("{label}\n")),
            }
        }
        for note in notes {
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
            TargetType::SelfTarget => "Self target",
            TargetType::Unit => "Unit target",
            TargetType::Point => "Point target",
            TargetType::Area => "Area target",
            TargetType::Direction => "Direction",
        },
        match def.target_team {
            TargetTeam::Enemy => "Enemies",
            TargetTeam::Ally => "Allies",
            TargetTeam::Any => "Any unit",
            TargetTeam::SelfOnly => "Self",
        },
    ];
    if def.is_ultimate {
        parts.push("Ultimate");
    }
    for mechanic in &def.mechanics {
        let tag = match mechanic {
            AbilityMechanic::Dash { .. } => "Dash",
            AbilityMechanic::Projectile {
                skillshot: true, ..
            } => "Skillshot",
            AbilityMechanic::Projectile { .. } => "Projectile",
            AbilityMechanic::ProjectileToEach { team, .. } => match team {
                TargetTeam::Enemy => "Projectile at each enemy in radius",
                TargetTeam::Ally | TargetTeam::SelfOnly => "Projectile at each ally in radius",
                TargetTeam::Any => "Projectile at each unit in radius",
            },
            AbilityMechanic::AttackUnitTarget => "Attack",
            AbilityMechanic::RingFx { .. } => continue,
        };
        if !parts.contains(&tag) {
            parts.push(tag);
        }
    }
    parts.join(" · ")
}

fn push_value(
    bits: &mut Vec<TipBit>,
    label: &str,
    value: RankValue,
    max_rank: u32,
    learned_rank: u32,
) {
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

pub(crate) fn fmt_num(value: f32) -> String {
    if !value.is_finite() {
        return "0".into();
    }
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i32)
    } else {
        format!("{value:.1}")
    }
}

pub(crate) fn trigger_tag(trigger: AbilityTrigger) -> &'static str {
    match trigger {
        AbilityTrigger::OnCast => "on cast",
        AbilityTrigger::OnProjectileHit => "on hit",
        AbilityTrigger::OnUnitContact => "on contact",
        AbilityTrigger::OnMovementEnd => "movement end",
        AbilityTrigger::OnImpact => "on impact",
        AbilityTrigger::OnChannelTick => "while channelling",
        AbilityTrigger::OnChannelEnd => "channel end",
        AbilityTrigger::OnChannelInterrupted => "channel interrupted",
        AbilityTrigger::OnExpire => "on expire",
        AbilityTrigger::WhileToggled => "while toggled",
        AbilityTrigger::Passive => "passive",
        AbilityTrigger::OnUse => "use",
        AbilityTrigger::OnAttack => "on attack",
        AbilityTrigger::OnAttackHit => "on attack hit",
        AbilityTrigger::OnDamageDealt => "on dealing damage",
        AbilityTrigger::OnDamageTaken => "when damaged",
        AbilityTrigger::OnHealthBelow { .. } => "at low health",
        AbilityTrigger::OnStatusApplied(_) => "when afflicted",
        AbilityTrigger::OnStatusRemoved(_) => "when a status ends",
        AbilityTrigger::While(_) => "while active",
        AbilityTrigger::OnMove => "while moving",
        AbilityTrigger::OnEnemyEnter { .. } => "when an enemy approaches",
        AbilityTrigger::OnEnemyLeave { .. } => "when an enemy leaves",
        AbilityTrigger::OnCooldownReady => "when ready",
        AbilityTrigger::OnInterval { .. } => "periodically",
        AbilityTrigger::OnSpellCast => "on spell cast",
        AbilityTrigger::OnSpellHit => "on spell hit",
        AbilityTrigger::OnKill => "on kill",
        AbilityTrigger::OnDeath => "on death",
        AbilityTrigger::Custom(_) => "custom",
    }
}

/// One line of an effect's description: a label with no trigger tag, and the per-rank
/// number when it has one ("Physical damage" / 80..., "Teleports to the target point" / none).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EffectRow {
    pub label: String,
    pub value: Option<RankValue>,
    /// The value is a duration in seconds (item tooltips print a unit; ability tooltips
    /// keep their bare numbers).
    pub seconds: bool,
}

fn row(label: impl Into<String>, value: RankValue) -> EffectRow {
    EffectRow {
        label: label.into(),
        value: Some(value),
        seconds: false,
    }
}

fn seconds_row(label: impl Into<String>, value: RankValue) -> EffectRow {
    EffectRow {
        seconds: true,
        ..row(label, value)
    }
}

fn text_row(label: impl Into<String>) -> EffectRow {
    EffectRow {
        label: label.into(),
        value: None,
        seconds: false,
    }
}

/// The one description of an effect entry, shared by ability and item tooltips. Rows
/// carry no trigger tag (the caller adds "(on cast)" or "Use:"); the second list holds
/// untagged notes such as scaling.
pub(crate) fn describe_effect(entry: &AbilityEffectEntry) -> (Vec<EffectRow>, Vec<String>) {
    let mut rows = effect_rows(&entry.effect);
    rows.extend(effect_text_rows(&entry.effect));
    let notes = match entry.effect {
        AbilityEffect::Damage { scaling, .. }
        | AbilityEffect::DrainHealth { scaling, .. }
        | AbilityEffect::Heal { scaling, .. } => scaling_note(scaling).into_iter().collect(),
        _ => Vec::new(),
    };
    (rows, notes)
}

/// Where an area effect lands, for item tooltips ("enemies within 143").
pub(crate) fn area_note(target: &EffectTarget) -> Option<String> {
    let EffectTarget::UnitsInRadius { radius, team, .. } = target else {
        return None;
    };
    let who = match team {
        TargetTeam::Enemy => "enemies",
        TargetTeam::Ally => "allies",
        TargetTeam::Any => "units",
        TargetTeam::SelfOnly => "self",
    };
    Some(match radius {
        AreaRadius::Ability => format!("{who} in the area"),
        AreaRadius::Fixed(value) => format!("{who} within {}", fmt_num(value.at(1))),
    })
}

fn effect_rows(effect: &AbilityEffect) -> Vec<EffectRow> {
    match *effect {
        AbilityEffect::Damage {
            amount,
            damage_type,
            ..
        } => vec![row(format!("{} damage", damage_name(damage_type)), amount)],
        AbilityEffect::DrainHealth {
            amount,
            damage_type,
            ..
        } => vec![row(
            format!("{} damage (heals you)", damage_name(damage_type)),
            amount,
        )],
        AbilityEffect::Heal { amount, .. } => vec![row("Heal", amount)],
        AbilityEffect::RestoreMana { amount } => vec![row("Mana restore", amount)],
        AbilityEffect::DrainMana {
            amount,
            burn_ratio,
            transfer_ratio,
            ..
        } => {
            let mut label = String::from("Mana removed");
            if burn_ratio > 0.0 {
                label.push_str(" (burned)");
            }
            if transfer_ratio > 0.0 {
                label.push_str(" (drained)");
            }
            vec![row(label, amount)]
        }
        AbilityEffect::SpendResource {
            resource, amount, ..
        } => vec![row(
            match resource {
                crate::vitals::ResourceKind::Health => "Health cost",
                crate::vitals::ResourceKind::Mana => "Mana spent",
            },
            amount,
        )],
        AbilityEffect::ApplyStatus { status, duration } => status_rows(status, duration),
        AbilityEffect::Displace {
            kind,
            distance,
            duration,
            ..
        } => {
            let name = match kind {
                DisplacementKind::Knockback => "Knockback",
                DisplacementKind::Pull | DisplacementKind::PullToPoint => "Pull",
                DisplacementKind::Dash => "Dash",
                DisplacementKind::Charge => "Charge",
                DisplacementKind::Drag => "Drag",
                DisplacementKind::Cyclone => "Cyclone",
            };
            let mut rows = Vec::new();
            if let Some(distance) = distance {
                rows.push(row(format!("{name} distance"), distance));
            }
            rows.push(seconds_row(format!("{name} duration"), duration));
            rows
        }
        AbilityEffect::PassiveStat { stat, amount } => vec![row(passive_name(stat), amount)],
        AbilityEffect::Custom {
            id,
            value,
            amount,
            duration,
        } => {
            let mut rows = Vec::new();
            if value != RankValue::ZERO {
                rows.push(row(id, value));
            }
            if amount != RankValue::ZERO {
                rows.push(row(format!("{id} amount"), amount));
            }
            if duration != RankValue::ZERO {
                rows.push(seconds_row(format!("{id} duration"), duration));
            }
            rows
        }
        AbilityEffect::Teleport {
            max_distance: Some(max),
            ..
        } => vec![row("Max distance", max)],
        AbilityEffect::Dispel { .. }
        | AbilityEffect::Teleport { .. }
        | AbilityEffect::RemoveStatus { .. }
        | AbilityEffect::Proc { .. }
        | AbilityEffect::RequireStatus { .. }
        | AbilityEffect::Counter { .. }
        | AbilityEffect::Chain { .. }
        | AbilityEffect::Zone { .. }
        | AbilityEffect::Accumulate { .. }
        | AbilityEffect::PerformAttack { .. } => Vec::new(),
    }
}

fn status_rows(status: StatusSpec, duration: RankValue) -> Vec<EffectRow> {
    match status {
        StatusSpec::Slow { percent, .. } => {
            vec![
                row("Slow %", percent),
                seconds_row("Slow duration", duration),
            ]
        }
        StatusSpec::MoveSpeedBonus { percent, .. } => {
            vec![
                row("Move speed bonus %", percent),
                seconds_row("Move speed bonus duration", duration),
            ]
        }
        StatusSpec::TauntCastTarget { attack_speed } => vec![
            row("Attack speed while taunted", attack_speed),
            seconds_row("Taunt duration", duration),
        ],
        StatusSpec::AttackPoint { seconds, .. } => {
            vec![
                seconds_row("Attack point", seconds),
                seconds_row("Attack point duration", duration),
            ]
        }
        StatusSpec::Burn {
            amount,
            tick_interval,
            ..
        } => {
            let label = if tick_interval == RankValue::ZERO {
                "Burn damage per second".to_string()
            } else {
                // Intervals are constant across ranks in practice; show rank 1's.
                format!("Burn damage per {}s", tick_interval.at(1))
            };
            vec![row(label, amount), seconds_row("Burn duration", duration)]
        }
        StatusSpec::HealOverTime {
            hps,
            max_health_pct,
        } => {
            let mut rows = Vec::new();
            if hps != RankValue::ZERO {
                rows.push(row("Heal per second", hps));
            }
            if max_health_pct != RankValue::ZERO {
                rows.push(row("Heal % max health per second", max_health_pct));
            }
            rows.push(seconds_row("Heal duration", duration));
            rows
        }
        StatusSpec::Evasion { percent } => vec![
            row("Evasion %", percent),
            seconds_row("Evasion duration", duration),
        ],
        StatusSpec::Accuracy { percent } => vec![
            row("Accuracy %", percent),
            seconds_row("Accuracy duration", duration),
        ],
        StatusSpec::HealReduction { percent } => vec![
            row("Heal reduction %", percent),
            seconds_row("Heal reduction duration", duration),
        ],
        StatusSpec::Stacks { .. } => vec![seconds_row("Stack duration", duration)],
        StatusSpec::DamageReduction { percent, .. } => vec![
            row("Damage reduction %", percent),
            seconds_row("Damage reduction duration", duration),
        ],
        StatusSpec::MagicResistance { percent, .. } => vec![
            row("Magic resistance %", percent),
            seconds_row("Magic resistance duration", duration),
        ],
        StatusSpec::StatusResistance { percent, .. } => vec![
            row("Status resistance %", percent),
            seconds_row("Status resistance duration", duration),
        ],
        StatusSpec::ManaShield {
            absorb_percent,
            damage_per_mana,
        } => vec![
            row("Mana shield absorb %", absorb_percent),
            row("Damage per mana", damage_per_mana),
            seconds_row("Mana shield duration", duration),
        ],
        StatusSpec::ResourceModifier { stat, amount, .. } => vec![
            row(crate::vitals::stat_label(stat), amount),
            seconds_row("Duration", duration),
        ],
        StatusSpec::Leash { radius, .. } => vec![
            row(format!("{} radius", status_name(status)), radius),
            seconds_row(format!("{} duration", status_name(status)), duration),
        ],
        StatusSpec::Modifier {
            armor,
            magic_resist,
            move_speed,
            attack_damage,
            attack_range,
            attack_projectile_speed,
            attack_damage_pct,
            cleave,
            cleave_radius,
            cleave_end_radius,
            cleave_distance,
            attack_charges,
            ..
        } => {
            let mut rows = Vec::new();
            if armor != RankValue::ZERO {
                rows.push(row("Armor", armor));
            }
            if magic_resist != RankValue::ZERO {
                rows.push(row("Magic resist", magic_resist));
            }
            if move_speed != RankValue::ZERO {
                rows.push(row("Move speed", move_speed));
            }
            if attack_damage != RankValue::ZERO {
                rows.push(row("Attack damage", attack_damage));
            }
            if attack_damage_pct != RankValue::ZERO {
                rows.push(row("Attack damage %", attack_damage_pct));
            }
            if cleave != RankValue::ZERO {
                rows.push(row("Cleave", cleave));
            }
            if cleave_radius != RankValue::ZERO {
                rows.push(row("Cleave start radius", cleave_radius));
            }
            if cleave_end_radius != RankValue::ZERO {
                rows.push(row("Cleave end radius", cleave_end_radius));
            }
            if cleave_distance != RankValue::ZERO {
                rows.push(row("Cleave distance", cleave_distance));
            }
            if attack_range != RankValue::ZERO {
                rows.push(row("Attack range", attack_range));
            }
            if attack_projectile_speed != RankValue::ZERO {
                rows.push(row("Attack projectile speed", attack_projectile_speed));
            }
            if attack_charges != RankValue::ZERO {
                rows.push(row("For the next attacks", attack_charges));
            }
            rows.push(seconds_row("Duration", duration));
            rows
        }
        other => vec![seconds_row(status_name(other), duration)],
    }
}

/// Effects that read as a sentence rather than a number.
fn effect_text_rows(effect: &AbilityEffect) -> Vec<EffectRow> {
    match *effect {
        AbilityEffect::Dispel { kind, strength } => {
            let tier = match strength {
                crate::status::DispelStrength::Basic => "basic",
                crate::status::DispelStrength::Strong => "strong",
            };
            match kind {
                StatusKind::Buff => vec![text_row(format!("{tier} dispel buffs"))],
                StatusKind::Debuff => vec![text_row(format!("{tier} dispel debuffs"))],
            }
        }
        AbilityEffect::RemoveStatus { id } => vec![text_row(format!("Remove {id}"))],
        AbilityEffect::Proc { chance, pseudo, .. } => vec![text_row(format!(
            "{}% chance to proc{}",
            fmt_num(chance.at(1) * 100.0),
            if pseudo { " (pseudo-random)" } else { "" }
        ))],
        AbilityEffect::PerformAttack {
            damage_ratio,
            max_targets,
            ..
        } => {
            let hits = match max_targets {
                0 => "Attacks every target".to_string(),
                1 => "Attacks once".to_string(),
                n => format!("Attacks up to {n} targets"),
            };
            vec![text_row(format!(
                "{hits} for {}% damage",
                fmt_num(damage_ratio.at(1) * 100.0)
            ))]
        }
        AbilityEffect::RequireStatus { .. } => {
            vec![text_row("While the effect is active".to_string())]
        }
        AbilityEffect::Counter { every, .. } => {
            vec![text_row(format!("Every {every} attacks"))]
        }
        AbilityEffect::Chain { jumps, falloff, .. } => vec![text_row(format!(
            "Chains up to {jumps} times, {}% per jump",
            fmt_num(falloff * 100.0)
        ))],
        AbilityEffect::Zone {
            radius,
            delay,
            duration,
            interval,
            follow_caster,
            ..
        } => {
            let label = if follow_caster {
                "Zone (follows the caster)"
            } else {
                "Zone"
            };
            let radius = match radius {
                AreaRadius::Ability => "the ability radius".to_string(),
                AreaRadius::Fixed(value) => fmt_num(value.at(1)),
            };
            let delay = match delay.at(1) {
                d if d > 0.0 => format!("after {}s, ", fmt_num(d)),
                _ => String::new(),
            };
            vec![text_row(format!(
                "{label}: radius {radius}, {delay}every {}s for {}s",
                fmt_num(interval.at(1)),
                fmt_num(duration.at(1))
            ))]
        }
        AbilityEffect::Accumulate { threshold, .. } => vec![text_row(format!(
            "Each time {} builds up",
            fmt_num(threshold.at(1))
        ))],
        AbilityEffect::Teleport { min_distance, .. } if min_distance != RankValue::ZERO => {
            vec![text_row(format!(
                "Teleports toward the target point, at least {} away",
                fmt_num(min_distance.at(1))
            ))]
        }
        AbilityEffect::Teleport { .. } => vec![text_row("Teleports to the target point")],
        AbilityEffect::Displace {
            distance: None,
            kind: DisplacementKind::Pull,
            ..
        } => vec![text_row("Pulls to the caster")],
        AbilityEffect::Custom {
            id,
            value,
            amount,
            duration,
        } if value == RankValue::ZERO
            && amount == RankValue::ZERO
            && duration == RankValue::ZERO =>
        {
            vec![text_row(id)]
        }
        _ => Vec::new(),
    }
}

fn scaling_note(scaling: EffectScaling) -> Option<String> {
    match scaling {
        EffectScaling::None => None,
        EffectScaling::CasterAttackDamage(ratio) => Some(format!("+ {ratio:.2}× attack damage")),
        EffectScaling::CasterAttribute(attribute, ratio) => {
            Some(format!("+ {ratio:.2}× {}", attribute_name(attribute)))
        }
        EffectScaling::TargetHealthBelow(fraction) => {
            Some(format!("Only below {:.0}% health", fraction * 100.0))
        }
        EffectScaling::TargetMissingHealth(ratio) => Some(format!("+ {ratio:.2}× missing health")),
        EffectScaling::TargetMaxHealth(ratio) => Some(format!("+ {ratio:.2}× max health")),
        EffectScaling::TargetCurrentHealth(ratio) => Some(format!("+ {ratio:.2}× current health")),
        EffectScaling::TargetStacks { offset: 0, .. } => Some("× stacks on the target".to_string()),
        EffectScaling::TargetStacks { offset, .. } => {
            Some(format!("× (stacks on the target + {offset})"))
        }
        EffectScaling::DistanceFromCaster { falloff } => {
            Some(format!("−{:.0}% at max range", falloff * 100.0))
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
        StatusSpec::Stun | StatusSpec::OwnedStun { .. } => "Stun",
        StatusSpec::Root => "Root",
        StatusSpec::Silence => "Silence",
        StatusSpec::Disarm => "Disarm",
        StatusSpec::Phased => "Phased",
        StatusSpec::Frozen => "Frozen",
        StatusSpec::Forceful => "Forceful",
        StatusSpec::Flying => "Flying",
        StatusSpec::Leash {
            anchor: super::definition::LeashAnchorKind::Caster,
            ..
        } => "Tether",
        StatusSpec::Leash { .. } => "Leash",
        StatusSpec::DebuffImmunity => "Debuff immunity",
        StatusSpec::SpellImmunity => "Spell immunity",
        StatusSpec::AttackImmunity => "Attack immunity",
        StatusSpec::Untargetable => "Untargetable",
        StatusSpec::Ethereal { .. } => "Ethereal",
        StatusSpec::Banish => "Banish",
        StatusSpec::Shield { .. } => "Shield",
        StatusSpec::Burn { .. } => "Burn",
        StatusSpec::Mute => "Mute",
        StatusSpec::Sleep => "Sleep",
        StatusSpec::Invulnerable => "Invulnerable",
        StatusSpec::Hex => "Hex",
        StatusSpec::Break => "Break",
        StatusSpec::Invisibility { .. } => "Invisibility",
        StatusSpec::Reveal => "Reveal",
        StatusSpec::SpellBlock => "Spell block",
        StatusSpec::SpellReflect => "Spell reflect",
        StatusSpec::Blind { .. } => "Blind",
        StatusSpec::Evasion { .. } => "Evasion",
        StatusSpec::Accuracy { .. } => "Accuracy",
        StatusSpec::TrueStrike => "True strike",
        StatusSpec::Fear => "Fear",
        StatusSpec::Taunt | StatusSpec::TauntCastTarget { .. } => "Taunt",
        StatusSpec::Hypnosis => "Hypnosis",
        StatusSpec::Slow { .. } => "Slow",
        StatusSpec::MoveSpeedBonus { .. } => "Move speed bonus",
        StatusSpec::AttackPoint { .. } => "Attack point",
        StatusSpec::AttackSpeedSlow { .. } => "Attack speed slow",
        StatusSpec::TurnRateSlow { .. } => "Turn rate slow",
        StatusSpec::TurnRateBonus { .. } => "Turn rate bonus",
        StatusSpec::HealOverTime { .. } => "Heal over time",
        StatusSpec::HealReduction { .. } => "Heal reduction",
        StatusSpec::HealBlock => "Heal block",
        StatusSpec::DamageReduction { .. } => "Damage reduction",
        StatusSpec::Stacks { .. } => "Stacks",
        StatusSpec::MagicResistance { .. } => "Magic resistance",
        StatusSpec::StatusResistance { .. } => "Status resistance",
        StatusSpec::ManaShield { .. } => "Mana shield",
        StatusSpec::ResourceModifier { .. } => "Modifier",
        StatusSpec::Modifier { .. } => "Modifier",
    }
}

pub(crate) fn passive_name(stat: super::definition::PassiveStat) -> &'static str {
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
        PassiveStat::SpellAmp => "Spell amp",
        PassiveStat::OutgoingPhysicalAmp => "Physical amp",
        PassiveStat::IncomingDamageAmp => "Incoming amp",
        PassiveStat::DamageBlock => "Damage block",
        PassiveStat::DamageReflect => "Damage reflect",
        PassiveStat::HealAmp => "Heal amp",
        PassiveStat::HealReceivedAmp => "Heal received amp",
        PassiveStat::Lifesteal => "Lifesteal",
        PassiveStat::SpellLifesteal => "Spell lifesteal",
        PassiveStat::ManaCostAmp => "Mana cost",
        PassiveStat::CritChance => "Crit chance",
        PassiveStat::CritMultiplier => "Crit multiplier",
        PassiveStat::Evasion => "Evasion",
        PassiveStat::Accuracy => "Accuracy",
        PassiveStat::CleaveRatio => "Cleave",
        PassiveStat::SplashRatio => "Splash",
        PassiveStat::AttackRange => "Attack range",
        PassiveStat::AttackProjectileSpeed => "Projectile speed",
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

    #[test]
    fn cast_range_buffer_row_shows_only_when_set() {
        let slam = builtin(AbilityId::SeismicSlam);
        let text = |def: &AbilityDefinition| -> String {
            ability_tooltip_bits(def, 1)
                .iter()
                .map(|bit| bit.text.as_str())
                .collect()
        };
        assert!(!text(&slam).contains("Cast range buffer"));
        let buffered = slam.cast_range_buffer(RankValue::fixed(600.0));
        assert!(text(&buffered).contains("Cast range buffer"));
    }
}
