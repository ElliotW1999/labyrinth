//! Hero XP gains and level-up stat growth.

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
        progress.xp_to_next = xp_required_for_level(progress.level);
    }
    if progress.level >= 25 {
        progress.xp = 0;
        progress.xp_to_next = 0;
    }
}

fn apply_level_ups(
    mut heroes: Query<(Entity, &HeroProgress, &mut Health, &mut Mana, &mut CombatStats), With<PlayerHero>>,
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

    #[test]
    fn leveling_consumes_xp_and_raises_level() {
        let mut progress = HeroProgress::new();
        let need = progress.xp_to_next;
        add_xp(&mut progress, need);
        assert_eq!(progress.level, 2);
        assert_eq!(progress.xp, 0);
        assert_eq!(progress.xp_to_next, xp_required_for_level(2));
    }

    #[test]
    fn excess_xp_carries_over() {
        let mut progress = HeroProgress::new();
        let need = progress.xp_to_next;
        add_xp(&mut progress, need + 25);
        assert_eq!(progress.level, 2);
        assert_eq!(progress.xp, 25);
    }
}
