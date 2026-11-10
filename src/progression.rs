//! Hero XP gains, skill points, and attribute growth on level-up.

use bevy::prelude::*;

use crate::components::{
    CombatStats, Health, HeroAttributes, HeroProgress, Mana, PlayerHero, xp_required_for_level,
};
use crate::status::StatusEffects;
use crate::vitals::{self, HealKind, ResourceStat};

pub struct ProgressionPlugin;

impl Plugin for ProgressionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (regen_health, apply_level_ups));
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

fn regen_health(
    time: Res<Time>,
    mut query: Query<(&mut Health, Option<&CombatStats>, Option<&StatusEffects>)>,
) {
    let dt = time.delta_secs();
    for (mut health, stats, statuses) in &mut query {
        if !health.is_alive() || health.current >= health.max {
            continue;
        }
        let regen = health.regen_per_sec
            + statuses
                .map(|s| s.resource_sum(ResourceStat::HealthRegen))
                .unwrap_or(0.0);
        if regen <= 0.0 {
            continue;
        }
        let factor = vitals::heal_factor(HealKind::Regen, 0.0, stats, statuses);
        vitals::apply_heal(&mut health, regen * dt, factor);
    }
}

fn apply_level_ups(
    mut heroes: Query<
        (
            Entity,
            &HeroProgress,
            &mut HeroAttributes,
            &mut Health,
            &mut Mana,
            &mut CombatStats,
        ),
        With<PlayerHero>,
    >,
    mut last_levels: Local<std::collections::HashMap<Entity, u32>>,
) {
    for (entity, progress, mut attrs, mut health, mut mana, mut stats) in &mut heroes {
        // A hero spawns with the attributes of its starting level already applied, so the
        // first level seen is the baseline; only levels gained after that grow attributes.
        let previous = *last_levels.entry(entity).or_insert(progress.level);
        if progress.level > previous {
            for _ in previous..progress.level {
                let before = *attrs;
                attrs.level_up();
                HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
            }
        }
        last_levels.insert(entity, progress.level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{AbilityId, AbilityLoadout};
    use bevy::ecs::system::RunSystemOnce;
    use std::time::Duration;

    fn regen_world(health: Health) -> (World, Entity) {
        let mut world = World::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs(1));
        world.insert_resource(time);
        let e = world.spawn(health).id();
        (world, e)
    }

    #[test]
    fn heal_does_not_revive_dead_units_via_regen() {
        let (mut world, e) = regen_world(Health {
            current: 0.0,
            max: 100.0,
            regen_per_sec: 10.0,
        });
        world.run_system_once(regen_health).unwrap();
        assert_eq!(world.get::<Health>(e).unwrap().current, 0.0);
    }

    #[test]
    fn timed_health_regen_modifier_adds_and_expires() {
        use crate::status::{StatusEffect, StatusEffects, StatusKind};
        let (mut world, e) = regen_world(Health {
            current: 50.0,
            max: 1000.0,
            regen_per_sec: 2.0,
        });
        let mut statuses = StatusEffects::default();
        statuses.effects.push(StatusEffect::resource_modifier(
            "regen_buff",
            StatusKind::Buff,
            ResourceStat::HealthRegen,
            8.0,
            5.0,
        ));
        world.entity_mut(e).insert(statuses);
        world.run_system_once(regen_health).unwrap();
        assert_eq!(world.get::<Health>(e).unwrap().current, 60.0);
        world.get_mut::<StatusEffects>(e).unwrap().effects.clear();
        world.run_system_once(regen_health).unwrap();
        assert_eq!(world.get::<Health>(e).unwrap().current, 62.0);
    }

    #[test]
    fn regen_ignores_amp_but_respects_reduction_in_the_system() {
        use crate::status::{StatusEffect, StatusEffects};
        let (mut world, e) = regen_world(Health {
            current: 50.0,
            max: 1000.0,
            regen_per_sec: 10.0,
        });
        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
        stats.heal_received_amp = 1.0;
        let mut statuses = StatusEffects::default();
        statuses
            .effects
            .push(StatusEffect::heal_reduction(0.5, 5.0));
        world.entity_mut(e).insert((stats, statuses));
        world.run_system_once(regen_health).unwrap();
        assert_eq!(world.get::<Health>(e).unwrap().current, 55.0);
    }

    #[test]
    fn regen_heals_and_clamps_at_max() {
        let (mut world, e) = regen_world(Health {
            current: 95.0,
            max: 100.0,
            regen_per_sec: 10.0,
        });
        world.run_system_once(regen_health).unwrap();
        assert_eq!(world.get::<Health>(e).unwrap().current, 100.0);
    }

    #[test]
    fn hero_progress_at_level_has_one_point_per_level() {
        let progress = HeroProgress::at_level(6);
        assert_eq!(progress.level, 6);
        assert_eq!(progress.skill_points, 6);
        assert_eq!(progress.xp, 0);
        assert_eq!(progress.xp_to_next, xp_required_for_level(6));
    }

    #[test]
    fn heroes_spawn_at_starting_level_with_starting_gold_and_full_vitals() {
        use crate::components::PlayerWallet;
        use crate::heroes::HeroId;
        use crate::resources::SharedAssets;
        use crate::units::{STARTING_GOLD, STARTING_HERO_LEVEL, spawn_hero_entity};

        let mut world = World::new();
        world.init_resource::<SharedAssets>();
        let hero = world.resource_scope(|world, assets: Mut<SharedAssets>| {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            let hero = spawn_hero_entity(
                &mut commands,
                &assets,
                crate::components::Team::Radiant,
                true,
                1,
                Vec3::ZERO,
                HeroId::Vanguard,
            );
            queue.apply(world);
            hero
        });
        assert_eq!(STARTING_HERO_LEVEL, 6);
        assert_eq!(STARTING_GOLD, 25_000);
        let progress = *world.get::<HeroProgress>(hero).unwrap();
        assert_eq!(progress.level, STARTING_HERO_LEVEL);
        assert_eq!(progress.skill_points, STARTING_HERO_LEVEL);
        assert_eq!(world.get::<PlayerWallet>(hero).unwrap().gold, STARTING_GOLD);

        // Attributes include five levels of growth over the level-1 definition.
        let base = HeroId::Vanguard.def().attributes;
        let attrs = *world.get::<HeroAttributes>(hero).unwrap();
        let expected = base.strength + 5.0 * base.str_per_level;
        assert!((attrs.strength - expected).abs() < 1e-3);
        let health = *world.get::<Health>(hero).unwrap();
        assert_eq!(health.current, health.max);
        let mana = world.get::<Mana>(hero).unwrap();
        assert_eq!(mana.current, mana.max);

        // The level-up system treats the starting level as the baseline: no double growth.
        world.run_system_once(apply_level_ups).unwrap();
        let after = *world.get::<HeroAttributes>(hero).unwrap();
        assert_eq!(after.strength, attrs.strength);
    }

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
        assert!(AbilityId::Execute.can_rank_up(0, 6));
        assert!(AbilityId::Meteor.can_rank_up(1, 12));
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
        let dash = crate::abilities::catalog::builtin(loadout.slots[0].id);
        assert!(dash.mana_cost.at(loadout.slots[0].rank) > 0.0);
    }

    #[test]
    fn level_up_grows_primary_attributes() {
        let mut attrs = HeroAttributes::starter();
        let mut health = Health::new(400.0);
        let mut mana = Mana::new(200.0, 5.0);
        let mut stats = CombatStats::simple(55.0, 8.0, 1.0, 2.0, 2.0, 12.0);
        attrs.apply_to(&mut health, &mut mana, &mut stats);
        let hp_before = health.max;
        let armor_before = stats.armor;
        let before = attrs;
        attrs.level_up();
        HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
        assert!(attrs.strength > before.strength);
        assert!(attrs.agility > before.agility);
        assert!(attrs.intelligence > before.intelligence);
        assert!(health.max > hp_before);
        assert!(stats.armor > armor_before);
        assert!(stats.attack_speed > 1.0);
    }

    fn attribute_fixture(
        primary: Option<crate::abilities::definition::Attribute>,
    ) -> (HeroAttributes, Health, Mana, CombatStats) {
        let mut attrs = HeroAttributes::starter();
        attrs.primary = primary;
        let mut health = Health::new(400.0);
        let mut mana = Mana::new(200.0, 5.0);
        let mut stats = CombatStats::simple(30.0, 8.0, 1.0, 2.0, 2.0, 12.0);
        attrs.apply_to(&mut health, &mut mana, &mut stats);
        (attrs, health, mana, stats)
    }

    #[test]
    fn primary_attribute_adds_attack_damage_on_apply_and_level_up() {
        use crate::abilities::definition::Attribute;
        let (mut attrs, mut health, mut mana, mut stats) =
            attribute_fixture(Some(Attribute::Agility));
        assert_eq!(stats.attack_damage, 30.0 + attrs.agility);
        let before = attrs;
        attrs.level_up();
        HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
        assert!((stats.attack_damage - (30.0 + attrs.agility)).abs() < 1e-4);
        assert!(
            (stats.attack_damage - (30.0 + before.agility + before.agi_per_level)).abs() < 1e-4
        );
    }

    #[test]
    fn non_primary_attribute_adds_no_damage() {
        use crate::abilities::definition::Attribute;
        let (mut attrs, mut health, mut mana, mut stats) =
            attribute_fixture(Some(Attribute::Strength));
        let damage = stats.attack_damage;
        // Only strength feeds damage; an agility or intelligence delta leaves it untouched.
        let before = attrs;
        attrs.agility += 10.0;
        attrs.intelligence += 10.0;
        HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
        assert_eq!(stats.attack_damage, damage);
    }

    #[test]
    fn starter_attributes_without_primary_add_no_damage() {
        let (mut attrs, mut health, mut mana, mut stats) = attribute_fixture(None);
        assert_eq!(stats.attack_damage, 30.0);
        let before = attrs;
        attrs.level_up();
        HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
        assert_eq!(stats.attack_damage, 30.0);
    }

    #[test]
    fn primary_attribute_bonus_survives_level_up_and_reverts_exactly() {
        use crate::abilities::definition::Attribute;
        let (mut attrs, mut health, mut mana, mut stats) =
            attribute_fixture(Some(Attribute::Strength));
        let start = stats.attack_damage;
        // A +10 Strength bonus is applied, the hero levels up, then the bonus is removed.
        let before = attrs;
        attrs.strength += 10.0;
        HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
        assert!((stats.attack_damage - (start + 10.0)).abs() < 1e-4);
        let before = attrs;
        attrs.level_up();
        HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
        let before = attrs;
        attrs.strength -= 10.0;
        HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
        let expected = start + attrs.str_per_level;
        assert!(
            (stats.attack_damage - expected).abs() < 1e-3,
            "damage {} vs {expected}",
            stats.attack_damage
        );
        assert!((stats.attack_damage - (30.0 + attrs.strength)).abs() < 1e-3);
    }

    #[test]
    fn spawned_hero_damage_includes_five_levels_of_primary_growth() {
        use crate::heroes::HeroId;
        use crate::resources::SharedAssets;
        use crate::units::spawn_hero_entity;

        let mut world = World::new();
        world.init_resource::<SharedAssets>();
        let hero = world.resource_scope(|world, assets: Mut<SharedAssets>| {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            let hero = spawn_hero_entity(
                &mut commands,
                &assets,
                crate::components::Team::Radiant,
                true,
                1,
                Vec3::ZERO,
                HeroId::Vanguard,
            );
            queue.apply(world);
            hero
        });
        // Vanguard: level-1 total 58 (STR 22), plus 5 levels x 2.6 STR.
        let damage = world.get::<CombatStats>(hero).unwrap().attack_damage;
        assert!((damage - (58.0 + 13.0)).abs() < 1e-3, "damage {damage}");
    }
}
