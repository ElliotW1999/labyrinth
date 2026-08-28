//! Hero XP gains, skill points, and level-up stat growth.

use bevy::prelude::*;

use crate::components::{
    CombatStats, Health, HeroProgress, Mana, PlayerHero, xp_required_for_level,
};

pub struct ProgressionPlugin;

impl Plugin for ProgressionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, apply_level_ups);
    }
}

pub fn add_xp(progress: &mut HeroProgress, amount: u32) {
    if progress.level >= 25 {
        progress.xp = progress.xp_to_next.saturating_sub(1);
        return;
    }
    progress.xp = progress.xp.saturating_add(amount);
    while progress.xp >= progress.xp_to_next && progress.level < 25 {
        progress.xp -= progress.xp_to_next;
        progress.level += 1;
        progress.skill_points += 1;
        progress.xp_to_next = xp_required_for_level(progress.level);
    }
    if progress.level >= 25 {
        progress.xp = 0;
        progress.xp_to_next = 0;
    }
}

fn apply_level_ups(
    mut heroes: Query<
        (Entity, &HeroProgress, &mut Health, &mut Mana, &mut CombatStats),
        With<PlayerHero>,
    >,
    mut last_levels: Local<std::collections::HashMap<Entity, u32>>,
) {
    for (entity, progress, mut health, mut mana, mut stats) in &mut heroes {
        let previous = *last_levels.get(&entity).unwrap_or(&1);
        if progress.level > previous {
            for _ in previous..progress.level {
                apply_level_bonus(&mut health, &mut mana, &mut stats);
            }
        }
        last_levels.insert(entity, progress.level);
    }
}

fn apply_level_bonus(health: &mut Health, mana: &mut Mana, stats: &mut CombatStats) {
    let hp_gain = 60.0;
    let mana_gain = 30.0;
    health.max += hp_gain;
    health.current = (health.current + hp_gain).min(health.max);
    mana.max += mana_gain;
    mana.current = (mana.current + mana_gain).min(mana.max);
    stats.attack_damage += 4.0;
    stats.armor += 0.5;
    stats.magic_resist += 0.4;
    stats.move_speed += 0.15;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{AbilityId, AbilityLoadout};

    #[test]
    fn leveling_grants_skill_points() {
        let mut progress = HeroProgress::new();
        assert_eq!(progress.skill_points, 1);
        let need = progress.xp_to_next;
        add_xp(&mut progress, need);
        assert_eq!(progress.level, 2);
        assert_eq!(progress.skill_points, 2);
    }

    #[test]
    fn basic_ability_maxes_at_level_13() {
        assert!(AbilityId::Dash.can_rank_up(0, 1));
        assert!(!AbilityId::Dash.can_rank_up(1, 2));
        assert!(AbilityId::Dash.can_rank_up(1, 3));
        assert!(AbilityId::Dash.can_rank_up(6, 13));
        assert!(!AbilityId::Dash.can_rank_up(7, 25));
    }

    #[test]
    fn ultimate_ranks_at_six_twelve_eighteen_twenty_four() {
        assert!(!AbilityId::Nova.can_rank_up(0, 5));
        assert!(AbilityId::Nova.can_rank_up(0, 6));
        assert!(AbilityId::Nova.can_rank_up(1, 12));
        assert!(AbilityId::Nova.can_rank_up(2, 18));
        assert!(AbilityId::Nova.can_rank_up(3, 24));
        assert!(!AbilityId::Nova.can_rank_up(4, 25));
    }

    #[test]
    fn ranking_consumes_skill_point_and_scales_stats() {
        let mut loadout = AbilityLoadout::starter();
        let mut points = 1u32;
        assert!(loadout.try_rank_up(0, 1, &mut points));
        assert_eq!(points, 0);
        assert_eq!(loadout.slots[0].rank, 1);
        assert!(loadout.slots[0].mana_cost > 0.0);
    }
}
