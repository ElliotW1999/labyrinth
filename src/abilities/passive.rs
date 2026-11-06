//! Passive ability bonuses and toggle upkeep (Overcharge-style).

use bevy::prelude::*;

use super::catalog::AbilityDefinitions;
use super::definition::{AbilityBehavior, AbilityEffect, AbilityTrigger, PassiveStat};
use crate::components::{AbilityId, AbilityLoadout, CombatStats, Health, HeroAttributes, Mana};
use crate::items::{ItemPassives, apply_passives, remove_passives};

/// Net passive stat bonuses currently applied from ranked passive abilities.
#[derive(Component, Clone, Default, PartialEq)]
pub struct AppliedAbilityPassives(pub ItemPassives);

/// Last `(ability id, rank)` per slot used to refresh [`AppliedAbilityPassives`].
#[derive(Component, Default)]
pub struct AbilityPassiveSync(pub Option<[(AbilityId, u32); 4]>);

fn loadout_passive_key(loadout: &AbilityLoadout) -> [(AbilityId, u32); 4] {
    [
        (loadout.slots[0].id, loadout.slots[0].rank),
        (loadout.slots[1].id, loadout.slots[1].rank),
        (loadout.slots[2].id, loadout.slots[2].rank),
        (loadout.slots[3].id, loadout.slots[3].rank),
    ]
}

/// Attack-speed and move-speed bonuses applied while a toggle ability is on.
#[derive(Component, Clone, Copy, Default, PartialEq)]
pub struct ToggleStatBonuses {
    pub attack_speed_mult: f32,
    pub move_speed: f32,
}

const OVERCHARGE_MANA_DRAIN_PER_SEC: f32 = 8.0;
const OVERCHARGE_ATTACK_SPEED_MULT: f32 = 0.2;
const OVERCHARGE_MOVE_SPEED: f32 = 10.0;

fn accumulate_passives(total: &mut ItemPassives, add: &ItemPassives) {
    total.strength += add.strength;
    total.agility += add.agility;
    total.intelligence += add.intelligence;
    total.max_health += add.max_health;
    total.health_regen += add.health_regen;
    total.max_mana += add.max_mana;
    total.mana_regen += add.mana_regen;
    total.attack_damage += add.attack_damage;
    total.attack_speed_flat += add.attack_speed_flat;
    total.armor += add.armor;
    total.magic_resist += add.magic_resist;
    total.move_speed += add.move_speed;
    total.spell_amp += add.spell_amp;
    total.outgoing_physical_amp += add.outgoing_physical_amp;
    total.incoming_damage_amp += add.incoming_damage_amp;
    total.damage_block += add.damage_block;
    total.damage_reflect += add.damage_reflect;
}

fn passive_stat_entry(stat: PassiveStat, amount: f32) -> ItemPassives {
    let mut p = ItemPassives::default();
    match stat {
        PassiveStat::MaxHealth => p.max_health = amount,
        PassiveStat::HealthRegen => p.health_regen = amount,
        PassiveStat::MaxMana => p.max_mana = amount,
        PassiveStat::ManaRegen => p.mana_regen = amount,
        PassiveStat::AttackDamage => p.attack_damage = amount,
        PassiveStat::AttackSpeed => p.attack_speed_flat = amount,
        PassiveStat::Armor => p.armor = amount,
        PassiveStat::MagicResist => p.magic_resist = amount,
        PassiveStat::MoveSpeed => p.move_speed = amount,
        PassiveStat::Attribute(attr) => match attr {
            super::definition::Attribute::Strength => p.strength = amount,
            super::definition::Attribute::Agility => p.agility = amount,
            super::definition::Attribute::Intelligence => p.intelligence = amount,
        },
        PassiveStat::SpellAmp => p.spell_amp = amount,
        PassiveStat::OutgoingPhysicalAmp => p.outgoing_physical_amp = amount,
        PassiveStat::IncomingDamageAmp => p.incoming_damage_amp = amount,
        PassiveStat::DamageBlock => p.damage_block = amount,
        PassiveStat::DamageReflect => p.damage_reflect = amount,
    }
    p
}

/// Sum `Passive` trigger stat entries for one learned ability rank.
pub fn passives_at_rank(def: &super::definition::AbilityDefinition, rank: u32) -> ItemPassives {
    if rank == 0 {
        return ItemPassives::default();
    }
    let mut total = ItemPassives::default();
    for entry in def.effects_for(AbilityTrigger::Passive) {
        let AbilityEffect::PassiveStat { stat, amount } = entry.effect else {
            continue;
        };
        let slice = passive_stat_entry(stat, amount.at(rank));
        accumulate_passives(&mut total, &slice);
    }
    total
}

pub fn loadout_passives(loadout: &AbilityLoadout, defs: &AbilityDefinitions) -> ItemPassives {
    let mut total = ItemPassives::default();
    for slot in &loadout.slots {
        let Some(def) = defs.get(slot.id) else {
            continue;
        };
        if def.behavior != AbilityBehavior::Passive {
            continue;
        }
        let slice = passives_at_rank(def, slot.rank);
        accumulate_passives(&mut total, &slice);
    }
    total
}

fn desired_toggle_bonuses(loadout: &AbilityLoadout) -> ToggleStatBonuses {
    let mut out = ToggleStatBonuses::default();
    for slot in &loadout.slots {
        if slot.id == AbilityId::Overcharge && slot.toggled {
            out.attack_speed_mult += OVERCHARGE_ATTACK_SPEED_MULT;
            out.move_speed += OVERCHARGE_MOVE_SPEED;
        }
    }
    out
}

fn apply_toggle_delta(
    before: ToggleStatBonuses,
    after: ToggleStatBonuses,
    stats: &mut CombatStats,
    agility: f32,
) {
    stats.attack_speed_mult += after.attack_speed_mult - before.attack_speed_mult;
    stats.move_speed += after.move_speed - before.move_speed;
    stats.recompute_attack_speed(agility);
}

/// Reconcile passive ability stats when the loadout is new or ranks change.
pub fn sync_ability_passives(
    defs: Res<AbilityDefinitions>,
    mut heroes: Query<(
        &AbilityLoadout,
        &mut AbilityPassiveSync,
        &mut AppliedAbilityPassives,
        &mut Health,
        &mut Mana,
        &mut CombatStats,
        &mut HeroAttributes,
    )>,
) {
    for (loadout, mut sync, mut applied, mut health, mut mana, mut stats, mut attrs) in &mut heroes
    {
        let key = loadout_passive_key(loadout);
        if sync.0.as_ref() == Some(&key) {
            continue;
        }
        sync.0 = Some(key);
        let desired = loadout_passives(loadout, &defs);
        if desired == applied.0 {
            continue;
        }
        if applied.0 != ItemPassives::default() {
            remove_passives(applied.0, 1, &mut health, &mut mana, &mut stats, &mut attrs);
        }
        if desired != ItemPassives::default() {
            apply_passives(desired, &mut health, &mut mana, &mut stats, &mut attrs);
        }
        applied.0 = desired;
    }
}

/// Mana drain and stat bonuses while toggle abilities stay on (Overcharge).
pub fn tick_toggle_upkeep(
    time: Res<Time>,
    mut heroes: Query<(
        &mut AbilityLoadout,
        &mut Mana,
        &mut CombatStats,
        &HeroAttributes,
        &mut ToggleStatBonuses,
    )>,
) {
    let dt = time.delta_secs();
    for (mut loadout, mut mana, mut stats, attrs, mut bonuses) in &mut heroes {
        let before = *bonuses;
        for slot in &mut loadout.slots {
            if slot.id != AbilityId::Overcharge || !slot.toggled {
                continue;
            }
            mana.current -= OVERCHARGE_MANA_DRAIN_PER_SEC * dt;
            if mana.current <= 0.0 {
                mana.current = 0.0;
                slot.toggled = false;
            }
        }
        let after = desired_toggle_bonuses(&loadout);
        if after != before {
            apply_toggle_delta(before, after, &mut stats, attrs.agility);
            *bonuses = after;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::catalog::{AbilityDefinitions, builtin};
    #[test]
    fn stone_skin_passive_scales_with_rank() {
        let def = builtin(AbilityId::StoneSkin);
        assert_eq!(passives_at_rank(&def, 1).armor, 4.0);
        assert_eq!(passives_at_rank(&def, 3).armor, 12.0);
    }

    #[test]
    fn loadout_passives_ignore_unlearned_passives() {
        let defs = AbilityDefinitions::default();
        let mut loadout = AbilityLoadout::from_abilities([
            AbilityId::StoneSkin,
            AbilityId::Dash,
            AbilityId::Bolt,
            AbilityId::Nova,
        ]);
        loadout.slots[0].rank = 2;
        let mut defs = defs;
        defs.ensure_builtin(AbilityId::StoneSkin);
        assert_eq!(loadout_passives(&loadout, &defs).armor, 8.0);
        loadout.slots[0].rank = 0;
        assert_eq!(loadout_passives(&loadout, &defs).armor, 0.0);
    }
}
