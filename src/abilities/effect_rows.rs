//! `data/ability_effects.csv`: extra effect entries for any ability, interpreted
//! without ability-specific code. Rows are appended to the ability's definition in
//! `effect_index` order.
//!
//! Columns: `ability_id, effect_index, trigger, effect_target, effect_type, effect_id,
//! value, damage_type, duration, radius, amount, scaling_type, scaling_value`.

use std::sync::LazyLock;

use super::definition::{
    AbilityEffect, AbilityEffectEntry, AbilityTrigger, AreaCenter, AreaRadius, EffectScaling,
    EffectTarget, RankValue, StatusSpec, TargetTeam,
};
use crate::components::{AbilityId, DamageType};
use crate::items::StatusKind;

const COLUMNS: [&str; 13] = [
    "ability_id",
    "effect_index",
    "trigger",
    "effect_target",
    "effect_type",
    "effect_id",
    "value",
    "damage_type",
    "duration",
    "radius",
    "amount",
    "scaling_type",
    "scaling_value",
];

#[derive(Debug, Clone, PartialEq)]
pub struct EffectRow {
    pub ability_id: &'static str,
    pub effect_index: u32,
    pub entry: AbilityEffectEntry,
}

static ROWS: LazyLock<Vec<EffectRow>> = LazyLock::new(|| {
    parse_effect_rows(include_str!("../../data/ability_effects.csv"))
        .unwrap_or_else(|err| panic!("data/ability_effects.csv: {err}"))
});

/// Effect entries from `data/ability_effects.csv` for `id`, in `effect_index` order.
/// `ability_id` may be the Rust variant (`SeismicSlam`) or the display name (`Seismic Slam`).
pub fn csv_effects_for(id: AbilityId) -> Vec<AbilityEffectEntry> {
    effects_for(&ROWS, id)
}

pub fn effects_for(rows: &[EffectRow], id: AbilityId) -> Vec<AbilityEffectEntry> {
    let variant = normalize(&format!("{id:?}"));
    let display = normalize(id.display_name());
    let mut matching: Vec<&EffectRow> = rows
        .iter()
        .filter(|row| {
            let name = normalize(row.ability_id);
            name == variant || name == display
        })
        .collect();
    matching.sort_by_key(|row| row.effect_index);
    matching.into_iter().map(|row| row.entry).collect()
}

fn normalize(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

pub fn parse_effect_rows(text: &'static str) -> Result<Vec<EffectRow>, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty() && !line.trim_start().starts_with('#'));
    let Some((_, header)) = lines.next() else {
        return Ok(Vec::new());
    };
    let header: Vec<&str> = header.split(',').map(str::trim).collect();
    let column = |name: &str| header.iter().position(|h| h.eq_ignore_ascii_case(name));
    let indices = COLUMNS
        .iter()
        .map(|name| column(name).ok_or_else(|| format!("missing column `{name}`")))
        .collect::<Result<Vec<_>, _>>()?;

    lines
        .map(|(number, line)| {
            let cells: Vec<&'static str> = line.split(',').map(str::trim).collect();
            let field = |i: usize| cells.get(indices[i]).copied().unwrap_or("");
            parse_row(&field).map_err(|err| format!("line {}: {err}", number + 1))
        })
        .collect()
}

fn number(cell: &str, name: &str) -> Result<Option<f32>, String> {
    if cell.is_empty() {
        return Ok(None);
    }
    cell.parse()
        .map(Some)
        .map_err(|_| format!("`{name}` is not a number: {cell:?}"))
}

fn parse_row(field: &dyn Fn(usize) -> &'static str) -> Result<EffectRow, String> {
    let [
        ability_id,
        effect_index,
        trigger,
        effect_target,
        effect_type,
        effect_id,
        value,
        damage_type,
        duration,
        radius,
        amount,
        scaling_type,
        scaling_value,
    ] = std::array::from_fn(field);
    if ability_id.is_empty() {
        return Err("`ability_id` is required".into());
    }
    let effect_index = effect_index
        .parse()
        .map_err(|_| format!("`effect_index` is not a whole number: {effect_index:?}"))?;

    let trigger = match trigger {
        "on_cast" => AbilityTrigger::OnCast,
        "on_projectile_hit" => AbilityTrigger::OnProjectileHit,
        "on_unit_contact" => AbilityTrigger::OnUnitContact,
        "on_impact" => AbilityTrigger::OnImpact,
        "on_channel_tick" => AbilityTrigger::OnChannelTick,
        "on_channel_end" => AbilityTrigger::OnChannelEnd,
        "on_expire" => AbilityTrigger::OnExpire,
        custom if custom.starts_with("on_") => AbilityTrigger::Custom(custom),
        other => return Err(format!("unknown trigger {other:?}")),
    };

    let target = match effect_target {
        "caster" => EffectTarget::Caster,
        "cast_target" => EffectTarget::CastTarget,
        "trigger_unit" => EffectTarget::TriggerUnit,
        "affected_units" => EffectTarget::AffectedUnits,
        "units_in_radius" => EffectTarget::UnitsInRadius {
            center: AreaCenter::TriggerPoint,
            radius: number(radius, "radius")?.map_or(AreaRadius::Ability, |r| {
                AreaRadius::Fixed(RankValue::fixed(r))
            }),
            team: TargetTeam::Enemy,
        },
        other => return Err(format!("unknown effect_target {other:?}")),
    };

    let magnitude = RankValue::from_level_1(
        number(value, "value")?.unwrap_or(0.0),
        number(amount, "amount")?.unwrap_or(0.0),
    );
    let duration = RankValue::fixed(number(duration, "duration")?.unwrap_or(0.0));
    let scaling_value = number(scaling_value, "scaling_value")?;
    let scaling = match (scaling_type, scaling_value) {
        ("" | "none", _) => EffectScaling::None,
        ("caster_attack_damage", Some(ratio)) => EffectScaling::CasterAttackDamage(ratio),
        ("target_health_below", Some(threshold)) => EffectScaling::TargetHealthBelow(threshold),
        (kind, None) => return Err(format!("scaling_type {kind:?} needs a scaling_value")),
        (kind, Some(_)) => return Err(format!("unknown scaling_type {kind:?}")),
    };
    if scaling != EffectScaling::None && effect_type != "damage" {
        return Err("scaling is only supported for damage effects".into());
    }

    let effect = match effect_type {
        "damage" => AbilityEffect::Damage {
            amount: magnitude,
            damage_type: match damage_type {
                "" | "magical" => DamageType::Magical,
                "physical" => DamageType::Physical,
                other => return Err(format!("unknown damage_type {other:?}")),
            },
            scaling,
        },
        "heal" => AbilityEffect::Heal { amount: magnitude },
        "apply_status" => AbilityEffect::ApplyStatus {
            status: match effect_id {
                "stun" => StatusSpec::Stun,
                "root" => StatusSpec::Root,
                "silence" => StatusSpec::Silence,
                "disarm" => StatusSpec::Disarm,
                "phased" => StatusSpec::Phased,
                "forceful" => StatusSpec::Forceful,
                "debuff_immunity" => StatusSpec::DebuffImmunity,
                "slow" => StatusSpec::Slow { percent: magnitude },
                other => return Err(format!("unknown status effect_id {other:?}")),
            },
            duration,
        },
        "dispel" => AbilityEffect::Dispel {
            kind: match effect_id {
                "buff" => StatusKind::Buff,
                "debuff" => StatusKind::Debuff,
                other => {
                    return Err(format!(
                        "dispel effect_id must be buff or debuff, got {other:?}"
                    ));
                }
            },
        },
        "custom" if !effect_id.is_empty() => AbilityEffect::Custom {
            id: effect_id,
            value: magnitude,
        },
        "custom" => return Err("custom effects need an effect_id".into()),
        other => return Err(format!("unknown effect_type {other:?}")),
    };

    Ok(EffectRow {
        ability_id,
        effect_index,
        entry: AbilityEffectEntry {
            trigger,
            target,
            effect,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "ability_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,duration,radius,amount,scaling_type,scaling_value";

    fn parse(rows: &'static str) -> Result<Vec<EffectRow>, String> {
        let text: &'static str = Box::leak(format!("{HEADER}\n{rows}").into_boxed_str());
        parse_effect_rows(text)
    }

    #[test]
    fn projectile_hit_damage_row_needs_no_ability_code() {
        let rows =
            parse("Fireball,0,on_projectile_hit,trigger_unit,damage,,120,physical,,,40,,").unwrap();
        assert_eq!(rows.len(), 1);
        let entry = rows[0].entry;
        assert_eq!(entry.trigger, AbilityTrigger::OnProjectileHit);
        assert_eq!(entry.target, EffectTarget::TriggerUnit);
        let AbilityEffect::Damage {
            amount,
            damage_type,
            scaling,
        } = entry.effect
        else {
            panic!("expected damage, got {:?}", entry.effect);
        };
        assert_eq!(damage_type, DamageType::Physical);
        assert_eq!(scaling, EffectScaling::None);
        assert_eq!(amount.at(1), 120.0);
        assert_eq!(amount.at(3), 200.0);
    }

    #[test]
    fn custom_triggers_statuses_and_scaling_parse() {
        let rows = parse(
            "Skewer,1,on_skewer_end,affected_units,apply_status,slow,25,,2,,,,\n\
             Skewer,0,on_skewer_end,units_in_radius,damage,,80,,,150,,target_health_below,0.5",
        )
        .unwrap();
        assert_eq!(
            rows[0].entry.trigger,
            AbilityTrigger::Custom("on_skewer_end")
        );
        assert!(matches!(
            rows[0].entry.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Slow { .. },
                ..
            }
        ));
        assert!(matches!(
            rows[1].entry.target,
            EffectTarget::UnitsInRadius {
                radius: AreaRadius::Fixed(_),
                ..
            }
        ));
        assert!(matches!(
            rows[1].entry.effect,
            AbilityEffect::Damage { scaling: EffectScaling::TargetHealthBelow(t), .. } if t == 0.5
        ));
    }

    #[test]
    fn bad_rows_report_their_line() {
        let err = parse("Fireball,0,projectile_hit,trigger_unit,damage,,1,,,,,,").unwrap_err();
        assert!(
            err.contains("line 2") && err.contains("unknown trigger"),
            "{err}"
        );
        assert!(parse("Fireball,0,on_cast,caster,heal,,1,,,,,caster_attack_damage,1").is_err());
        assert!(parse("Fireball,0,on_cast,nobody,damage,,1,,,,,,").is_err());
    }

    #[test]
    fn rows_match_variant_or_display_name_in_index_order() {
        let rows = parse(
            "Seismic Slam,2,on_cast,caster,heal,,10,,,,,,\n\
             SeismicSlam,1,on_cast,caster,heal,,5,,,,,,\n\
             Bolt,0,on_cast,caster,heal,,1,,,,,,",
        )
        .unwrap();
        let slam = effects_for(&rows, AbilityId::SeismicSlam);
        assert_eq!(slam.len(), 2);
        assert_eq!(
            slam[0].effect,
            AbilityEffect::Heal {
                amount: RankValue::from_level_1(5.0, 0.0)
            }
        );
        assert_eq!(effects_for(&rows, AbilityId::Bolt).len(), 1);
    }

    #[test]
    fn shipped_csv_parses() {
        assert!(!ROWS.is_empty());
    }
}
