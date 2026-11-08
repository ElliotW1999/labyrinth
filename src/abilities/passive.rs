//! Passive ability bonuses and toggle upkeep (mana drain and `WhileToggled` statuses).

use bevy::prelude::*;

use super::catalog::AbilityDefinitions;
use super::definition::{AbilityBehavior, AbilityEffect, AbilityTrigger};
use super::effects::{base_attack_damage, build_status_effect};
use crate::components::{AbilityId, AbilityLoadout, CombatStats, Health, HeroAttributes, Mana};
use crate::heroes::HeroKind;
use crate::items::{ItemPassives, apply_passives, remove_passives};
use crate::status::{DispelType, StatusChange, StatusEffect, StatusEffectEvent, StatusEffects};
use crate::vitals::{effective_mana_cost, outgoing_heal_amp};

/// Make a held status match `wanted`: apply it (permanent, undispellable) when wanted and
/// absent, remove it when unwanted and present, otherwise do nothing. Held statuses are
/// `reactive`, so neither the change nor the status ever fires status triggers. Shared by
/// toggles and `While(Condition)` entries.
pub(crate) fn reconcile_held_status(
    events: &mut MessageWriter<StatusEffectEvent>,
    owner: Entity,
    statuses: &StatusEffects,
    mut built: StatusEffect,
    wanted: bool,
) {
    let present = statuses.effects.iter().any(|e| e.id == built.id);
    if wanted && !present {
        built.undispellable = true;
        built.dispel_type = DispelType::None;
        built.reactive = true;
        events.write(
            StatusEffectEvent::new(Some(owner), owner, StatusChange::Apply(built)).reactive(true),
        );
    } else if !wanted && present {
        events.write(
            StatusEffectEvent::new(Some(owner), owner, StatusChange::Remove(built.id))
                .reactive(true),
        );
    }
}

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
        total.add_stat(stat, amount.at(rank));
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
        total += passives_at_rank(def, slot.rank);
    }
    total
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

/// Upkeep of toggle abilities. Runs at the front of the abilities chain.
///
/// 1. A toggled slot drains `toggle_mana_per_sec` and switches itself off at 0 mana (no
///    cooldown).
/// 2. The `WhileToggled` `ApplyStatus` entries are reconciled with the caster's statuses:
///    applied (permanent, undispellable) once the toggle is on, removed once it is off.
///    The status events land later in the same chain, so the bonus appears and
///    disappears in the frame the toggle changes. A rank-up while on keeps the old
///    values until the toggle is cycled.
pub fn tick_toggle_upkeep(
    time: Res<Time>,
    defs: Res<AbilityDefinitions>,
    mut heroes: Query<(
        Entity,
        &mut AbilityLoadout,
        &mut Mana,
        &StatusEffects,
        &CombatStats,
        Option<&HeroAttributes>,
        Option<&HeroKind>,
        Option<&Transform>,
    )>,
    mut status_events: MessageWriter<StatusEffectEvent>,
) {
    let dt = time.delta_secs();
    for (entity, mut loadout, mut mana, statuses, stats, attrs, kind, transform) in &mut heroes {
        let point = transform.map_or(Vec3::ZERO, |tf| tf.translation);
        let healer_amp = outgoing_heal_amp(Some(stats), Some(statuses));
        for slot in &mut loadout.slots {
            let Some(def) = defs.get(slot.id) else {
                continue;
            };
            if def.behavior != AbilityBehavior::Toggle {
                continue;
            }
            if slot.toggled && slot.rank > 0 {
                let upkeep = effective_mana_cost(
                    def.toggle_mana_per_sec.at(slot.rank),
                    Some(stats),
                    Some(statuses),
                );
                mana.current -= upkeep * dt;
                if mana.current <= 0.0 {
                    mana.current = 0.0;
                    slot.toggled = false;
                }
            }
            let rank = slot.rank.max(1);
            for entry in def.effects_for(AbilityTrigger::WhileToggled) {
                let AbilityEffect::ApplyStatus { status, .. } = entry.effect else {
                    continue;
                };
                let built = build_status_effect(
                    status,
                    rank,
                    f32::INFINITY,
                    Some(entity),
                    point,
                    Some(stats),
                    attrs,
                    base_attack_damage(kind, Some(stats)),
                    healer_amp,
                );
                reconcile_held_status(&mut status_events, entity, statuses, built, slot.toggled);
            }
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

    mod toggles {
        use std::time::Duration;

        use super::*;
        use crate::status::{DispelStrength, StatusKind, apply_status_effect_events};

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .init_resource::<AbilityDefinitions>()
                .add_message::<StatusEffectEvent>()
                .add_message::<crate::status::StatusLifecycleEvent>()
                .add_systems(
                    Update,
                    (tick_toggle_upkeep, apply_status_effect_events).chain(),
                );
            app.world_mut()
                .resource_mut::<AbilityDefinitions>()
                .ensure_builtin(AbilityId::Overcharge);
            app
        }

        fn hero(app: &mut App, mana: f32) -> Entity {
            let mut loadout = AbilityLoadout::from_abilities([
                AbilityId::Overcharge,
                AbilityId::Dash,
                AbilityId::Bolt,
                AbilityId::Nova,
            ]);
            loadout.slots[0].rank = 1;
            app.world_mut()
                .spawn((
                    loadout,
                    Mana::new(mana, 0.0),
                    StatusEffects::default(),
                    CombatStats::simple(10.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    HeroAttributes::starter(),
                ))
                .id()
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn set_toggled(app: &mut App, hero: Entity, on: bool) {
            app.world_mut()
                .get_mut::<AbilityLoadout>(hero)
                .unwrap()
                .slots[0]
                .toggled = on;
        }

        fn stats(app: &App, hero: Entity) -> CombatStats {
            *app.world().get::<CombatStats>(hero).unwrap()
        }

        #[test]
        fn overcharge_on_adds_attack_speed_and_move_speed_and_drains_mana() {
            let mut app = app();
            let hero = hero(&mut app, 100.0);
            let before = stats(&app, hero);
            set_toggled(&mut app, hero, true);
            step(&mut app, 0.5);
            let after = stats(&app, hero);
            assert!((after.attack_speed_mult - before.attack_speed_mult - 0.2).abs() < 1e-5);
            assert!((after.move_speed - before.move_speed - 10.0).abs() < 1e-5);
            let mana = app.world().get::<Mana>(hero).unwrap().current;
            assert!(
                (mana - (100.0 - 8.0 * 0.5)).abs() < 1e-4,
                "8 mana per second, got {mana}"
            );
            step(&mut app, 0.5);
            let again = stats(&app, hero);
            assert_eq!(
                again.move_speed, after.move_speed,
                "applied once, not stacked"
            );
        }

        #[test]
        fn while_toggled_status_has_expected_fields() {
            let mut app = app();
            let hero = hero(&mut app, 100.0);
            set_toggled(&mut app, hero, true);
            step(&mut app, 0.1);
            let statuses = app.world().get::<StatusEffects>(hero).unwrap();
            let speed = statuses
                .effects
                .iter()
                .find(|e| e.id == "overcharge_attack_speed_mult")
                .expect("attack speed status");
            assert_eq!(speed.kind, StatusKind::Buff);
            assert!(speed.remaining.is_infinite());
            assert!(speed.undispellable);
            assert_eq!(speed.dispel_type, DispelType::None);
            assert!((speed.attack_speed_mult - 0.2).abs() < 1e-6);
            assert_eq!(
                speed.dot_source,
                Some(hero),
                "toggle statuses are sourced like cast ones"
            );
            assert_eq!(speed.heal_per_sec, 0.0);
            assert_eq!(statuses.effects.len(), 2);
            let ids: Vec<_> = statuses.effects.iter().map(|e| e.id).collect();
            assert!(
                ids.iter().all(|id| id.starts_with("overcharge_")),
                "{ids:?}"
            );
            let movement = statuses
                .effects
                .iter()
                .find(|e| e.id != "overcharge_attack_speed_mult")
                .unwrap();
            assert!((movement.move_speed - 10.0).abs() < 1e-6);
            assert!(movement.remaining.is_infinite() && movement.undispellable);
        }

        #[test]
        fn overcharge_turns_off_at_zero_mana_and_reverts_stats() {
            let mut app = app();
            let hero = hero(&mut app, 1.0);
            let before = stats(&app, hero);
            set_toggled(&mut app, hero, true);
            step(&mut app, 0.05);
            assert_ne!(stats(&app, hero).move_speed, before.move_speed);
            step(&mut app, 0.5);
            let loadout = app.world().get::<AbilityLoadout>(hero).unwrap();
            assert!(!loadout.slots[0].toggled, "out of mana");
            assert_eq!(app.world().get::<Mana>(hero).unwrap().current, 0.0);
            let after = stats(&app, hero);
            assert!((after.move_speed - before.move_speed).abs() < 1e-5);
            assert!((after.attack_speed_mult - before.attack_speed_mult).abs() < 1e-5);
            assert!(
                app.world()
                    .get::<StatusEffects>(hero)
                    .unwrap()
                    .effects
                    .is_empty()
            );
        }

        #[test]
        fn overcharge_bonus_survives_strong_dispel() {
            let mut app = app();
            let hero = hero(&mut app, 100.0);
            set_toggled(&mut app, hero, true);
            step(&mut app, 0.1);
            let on = stats(&app, hero);
            app.world_mut().write_message(StatusEffectEvent {
                reactive: false,
                source: None,
                target: hero,
                change: StatusChange::Dispel {
                    kind: StatusKind::Buff,
                    strength: DispelStrength::Strong,
                },
            });
            step(&mut app, 0.1);
            assert_eq!(stats(&app, hero).move_speed, on.move_speed);
            assert_eq!(
                app.world()
                    .get::<StatusEffects>(hero)
                    .unwrap()
                    .effects
                    .len(),
                2,
                "both bonus statuses are still there"
            );
        }

        #[test]
        fn toggle_upkeep_uses_effective_cost() {
            let mut app = app();
            let hero = hero(&mut app, 100.0);
            app.world_mut()
                .get_mut::<CombatStats>(hero)
                .unwrap()
                .mana_cost_amp = -0.5;
            set_toggled(&mut app, hero, true);
            step(&mut app, 1.0);
            let mana = app.world().get::<Mana>(hero).unwrap().current;
            assert!(
                (mana - (100.0 - 4.0)).abs() < 1e-4,
                "8 mana per second at half cost, got {mana}"
            );
        }

        #[test]
        fn mana_shield_toggle_applies_and_removes() {
            use crate::abilities::definition::{
                AbilityDefinition, EffectTarget, RankValue, StatusSpec,
            };
            let mut app = app();
            let shield = AbilityDefinition::new(AbilityId::Overcharge)
                .behavior(AbilityBehavior::Toggle)
                .toggle_upkeep(RankValue::fixed(1.0))
                .on(
                    AbilityTrigger::WhileToggled,
                    EffectTarget::Caster,
                    AbilityEffect::ApplyStatus {
                        status: StatusSpec::ManaShield {
                            absorb_percent: RankValue::fixed(50.0),
                            damage_per_mana: RankValue::fixed(2.0),
                        },
                        duration: RankValue::ZERO,
                    },
                );
            app.world_mut()
                .resource_mut::<AbilityDefinitions>()
                .insert(shield);
            let hero = hero(&mut app, 100.0);
            let shield_of = |app: &App| {
                app.world()
                    .get::<StatusEffects>(hero)
                    .unwrap()
                    .mana_shield()
            };
            assert!(shield_of(&app).is_none());
            set_toggled(&mut app, hero, true);
            step(&mut app, 0.1);
            let on = shield_of(&app).expect("shield while toggled");
            assert_eq!((on.absorb, on.damage_per_mana), (0.5, 2.0));
            step(&mut app, 0.1);
            assert_eq!(
                app.world()
                    .get::<StatusEffects>(hero)
                    .unwrap()
                    .effects
                    .len(),
                1,
                "applied once"
            );
            set_toggled(&mut app, hero, false);
            step(&mut app, 0.1);
            assert!(shield_of(&app).is_none());
        }

        #[test]
        fn toggle_off_cast_removes_the_bonus() {
            let mut app = app();
            let hero = hero(&mut app, 100.0);
            let before = stats(&app, hero);
            set_toggled(&mut app, hero, true);
            step(&mut app, 0.1);
            // A second cast flips the toggle back (`commit_ability`).
            set_toggled(&mut app, hero, false);
            step(&mut app, 0.1);
            let after = stats(&app, hero);
            assert!((after.move_speed - before.move_speed).abs() < 1e-5);
            assert!((after.attack_speed_mult - before.attack_speed_mult).abs() < 1e-5);
        }
    }
}
