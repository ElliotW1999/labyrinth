//! Built-in ability definitions and the [`AbilityDefinitions`] registry.
//!
//! Most abilities are plain data: targeting, costs, and a list of reusable effects.
//! Abilities with unique logic (e.g. Execute) keep their common data here and
//! register a custom behavior in `custom.rs`.

use std::collections::HashMap;

use bevy::prelude::*;

use super::definition::{
    classify, AbilityBehavior, AbilityDefinition, AbilityEffect, AreaCenter, AreaRadius,
    EffectTargets, RankValue, RingStyle, StatusSpec, TargetType,
};
use crate::components::{AbilityId, AbilityLoadout, DamageType, GeneratedAbilityDef};
use crate::scale::{
    self, ABILITY_BLINK_RANGE, ABILITY_DASH_RANGE, ABILITY_GROUND_AOE, ABILITY_GROUND_CAST_RANGE,
    ABILITY_INSTANT_AOE, ABILITY_PROJECTILE_WIDTH, ABILITY_ULT_AOE, ABILITY_UNIT_CAST_RANGE,
};

/// Lookup from [`AbilityId`] to its definition. Built-ins are registered for every
/// ability that appears in a spawned loadout; games/tests may insert overrides first.
#[derive(Resource, Debug, Default)]
pub struct AbilityDefinitions {
    defs: HashMap<AbilityId, AbilityDefinition>,
}

impl AbilityDefinitions {
    pub fn get(&self, id: AbilityId) -> Option<&AbilityDefinition> {
        self.defs.get(&id)
    }

    #[allow(dead_code)]
    pub fn insert(&mut self, def: AbilityDefinition) {
        self.defs.insert(def.id, def);
    }

    pub fn ensure_builtin(&mut self, id: AbilityId) {
        self.defs.entry(id).or_insert_with(|| builtin(id));
    }
}

pub fn register_loadout_definitions(
    mut defs: ResMut<AbilityDefinitions>,
    loadouts: Query<&AbilityLoadout, Added<AbilityLoadout>>,
) {
    for loadout in &loadouts {
        for slot in &loadout.slots {
            defs.ensure_builtin(slot.id);
        }
    }
}

const fn nova_ring() -> AbilityEffect {
    AbilityEffect::RingFx {
        center: AreaCenter::Aim,
        radius: AreaRadius::Ability,
        style: RingStyle::Nova,
        lifetime: 0.55,
    }
}

const fn shockwave_ring() -> AbilityEffect {
    AbilityEffect::RingFx {
        center: AreaCenter::Caster,
        radius: AreaRadius::Ability,
        style: RingStyle::Shockwave,
        lifetime: 0.45,
    }
}

fn magic_damage(amount: RankValue, targets: EffectTargets) -> AbilityEffect {
    AbilityEffect::Damage {
        amount,
        damage_type: DamageType::Magical,
        targets,
    }
}

fn status(status: StatusSpec, duration: RankValue, targets: EffectTargets) -> AbilityEffect {
    AbilityEffect::ApplyStatus {
        status,
        duration,
        targets,
    }
}

fn bolt(damage: RankValue) -> AbilityEffect {
    AbilityEffect::SpawnProjectile {
        damage,
        damage_type: DamageType::Magical,
        splash_radius: AreaRadius::Ability,
        on_hit: Vec::new(),
    }
}

/// Unit-targeted spells use a small fixed indicator / splash radius.
const UNIT_SPELL_RADIUS: RankValue = RankValue::fixed(20.0);

pub fn builtin(id: AbilityId) -> AbilityDefinition {
    if let Some(generated) = id.generated() {
        return from_generated(id, generated);
    }
    let def = AbilityDefinition::new(id);
    let around_caster = EffectTargets::enemies_around(AreaCenter::Caster);
    let around_aim = EffectTargets::enemies_around(AreaCenter::Aim);
    match id {
        AbilityId::Dash | AbilityId::Blink => {
            let distance = if id == AbilityId::Blink {
                RankValue::linear(ABILITY_BLINK_RANGE, 25.0)
            } else {
                RankValue::linear(ABILITY_DASH_RANGE, 30.0)
            };
            def.targeting(TargetType::Point, distance, RankValue::fixed(48.0))
                .costs(RankValue::linear(7.5, -0.35).at_least(4.0), RankValue::linear(35.0, 5.0))
                .timing(0.05, 0.15)
                .effect(AbilityEffect::Dash { distance })
        }
        AbilityId::Shockwave | AbilityId::Flurry | AbilityId::FrostNova => {
            let (damage, radius_per_rank) = match id {
                AbilityId::Flurry => (RankValue::linear(55.0, 22.0), 15.0),
                AbilityId::FrostNova => (RankValue::linear(65.0, 26.0), 20.0),
                _ => (RankValue::linear(70.0, 28.0), 25.0),
            };
            let def = def
                .targeting(
                    TargetType::NoTarget,
                    RankValue::ZERO,
                    RankValue::linear(ABILITY_INSTANT_AOE, radius_per_rank),
                )
                .costs(RankValue::linear(9.0, -0.4).at_least(5.0), RankValue::linear(50.0, 8.0))
                .timing(0.25, 0.35)
                .effect(shockwave_ring())
                .effect(magic_damage(damage, around_caster));
            match id {
                AbilityId::Shockwave => def.effect(status(
                    StatusSpec::Forceful,
                    RankValue::fixed(2.0),
                    EffectTargets::Caster,
                )),
                AbilityId::FrostNova => {
                    def.effect(status(StatusSpec::Root, RankValue::fixed(1.4), around_caster))
                }
                _ => def
                    .effect(status(StatusSpec::Silence, RankValue::fixed(1.2), around_caster))
                    .effect(status(StatusSpec::Disarm, RankValue::fixed(1.0), around_caster)),
            }
        }
        AbilityId::Bolt => def
            .targeting(
                TargetType::Unit,
                RankValue::linear(ABILITY_UNIT_CAST_RANGE, 25.0),
                UNIT_SPELL_RADIUS,
            )
            .costs(RankValue::linear(6.0, -0.3).at_least(3.0), RankValue::linear(45.0, 7.0))
            .timing(0.2, 0.3)
            .effect(bolt(RankValue::linear(90.0, 30.0)))
            .effect(AbilityEffect::AttackUnitTarget),
        AbilityId::ArcMissile => def
            .targeting(
                TargetType::Point,
                RankValue::linear(ABILITY_GROUND_CAST_RANGE, 20.0),
                RankValue::linear(ABILITY_PROJECTILE_WIDTH, 8.0),
            )
            .costs(RankValue::linear(6.0, -0.3).at_least(3.0), RankValue::linear(45.0, 7.0))
            .timing(0.2, 0.3)
            .effect(bolt(RankValue::linear(85.0, 28.0)))
            .effect(AbilityEffect::AttackUnitTarget),
        // Projectile + low-HP bonus damage live in `custom::execute`.
        AbilityId::Execute => def
            .targeting(
                TargetType::Unit,
                RankValue::linear(ABILITY_UNIT_CAST_RANGE, 25.0),
                UNIT_SPELL_RADIUS,
            )
            .costs(RankValue::linear(50.0, -4.0).at_least(30.0), RankValue::linear(100.0, 20.0))
            .timing(0.3, 0.4)
            .effect(AbilityEffect::AttackUnitTarget),
        AbilityId::Caltrops => def
            .targeting(
                TargetType::Area,
                RankValue::linear(ABILITY_GROUND_CAST_RANGE, 15.0),
                RankValue::linear(ABILITY_GROUND_AOE, 15.0),
            )
            .costs(RankValue::linear(8.0, -0.35).at_least(4.5), RankValue::linear(40.0, 6.0))
            .timing(0.15, 0.25)
            .effect(nova_ring())
            .effect(magic_damage(RankValue::linear(50.0, 18.0), around_aim)),
        AbilityId::Barrier => {
            let duration = RankValue::linear(3.5, 0.4);
            def.costs(RankValue::linear(14.0, -0.5).at_least(8.0), RankValue::linear(55.0, 8.0))
                .timing(0.1, 0.2)
                .effect(status(StatusSpec::DebuffImmunity, duration, EffectTargets::Caster))
                .effect(status(
                    StatusSpec::armor_buff("barrier", RankValue::linear(6.0, 2.5)),
                    duration,
                    EffectTargets::Caster,
                ))
                .effect(AbilityEffect::RingFx {
                    center: AreaCenter::Caster,
                    radius: AreaRadius::Fixed(RankValue::fixed(scale::u(2.5))),
                    style: RingStyle::Nova,
                    lifetime: 0.4,
                })
        }
        AbilityId::Nova | AbilityId::Meteor => {
            let ult_costs = |d: AbilityDefinition| {
                d.costs(RankValue::linear(50.0, -4.0).at_least(30.0), RankValue::linear(100.0, 20.0))
                    .timing(0.35, 0.45)
            };
            if id == AbilityId::Nova {
                ult_costs(def.targeting(
                    TargetType::Area,
                    RankValue::linear(ABILITY_GROUND_CAST_RANGE, 20.0),
                    RankValue::linear(ABILITY_GROUND_AOE, 20.0),
                ))
                .effect(AbilityEffect::Heal {
                    amount: RankValue::linear(100.0, 40.0),
                    targets: EffectTargets::Caster,
                })
                .effect(nova_ring())
                .effect(magic_damage(RankValue::linear(160.0, 55.0), around_aim))
            } else {
                ult_costs(def.targeting(
                    TargetType::Area,
                    RankValue::linear(ABILITY_GROUND_CAST_RANGE, 15.0),
                    RankValue::linear(ABILITY_ULT_AOE, 25.0),
                ))
                .effect(nova_ring())
                .effect(magic_damage(RankValue::linear(180.0, 60.0), around_aim))
                .effect(status(StatusSpec::Stun, RankValue::fixed(1.1), around_aim))
            }
        }
        // HeroGenerator stubs: castable, costed, no effects yet.
        _ if id.is_ultimate() => def
            .costs(RankValue::linear(50.0, -4.0).at_least(30.0), RankValue::linear(100.0, 20.0))
            .timing(0.3, 0.35),
        _ => def
            .costs(RankValue::linear(8.0, -0.35).at_least(4.0), RankValue::linear(40.0, 6.0))
            .timing(0.2, 0.25),
    }
}

/// AbilityGenerator output → definition. The primary damage effect is derived from
/// the activation type; pseudocode extras still need hand-written effects.
fn from_generated(id: AbilityId, g: GeneratedAbilityDef) -> AbilityDefinition {
    let (behavior, target_type) = classify(g.ability_type);
    let mut aoe = RankValue::from_level_1(g.aoe_radius_base, g.aoe_radius_per_level).at_least(0.0);
    if target_type == TargetType::Point {
        aoe = aoe.at_least(40.0);
    }
    let mut def = AbilityDefinition::new(id)
        .behavior(behavior)
        .targeting(
            target_type,
            RankValue::from_level_1(g.cast_range_base, g.cast_range_per_level).at_least(0.0),
            aoe,
        )
        .costs(
            RankValue::from_level_1(g.cooldown_base, g.cooldown_per_level).at_least(g.cooldown_min),
            RankValue::from_level_1(g.mana_cost_base, g.mana_cost_per_level).at_least(0.0),
        )
        .timing(g.cast_point, g.cast_backswing);
    def.name = g.display_name;
    def.max_rank = g.max_rank;
    def.is_ultimate = g.is_ultimate;

    let damage = RankValue::from_level_1(g.damage_base, g.damage_per_level).at_least(0.0);
    if behavior != AbilityBehavior::Active || g.damage_base <= 0.0 {
        return def;
    }
    let damage_effect = |center| AbilityEffect::Damage {
        amount: damage,
        damage_type: g.damage_type,
        targets: EffectTargets::enemies_around(center),
    };
    match target_type {
        TargetType::NoTarget => def
            .effect(shockwave_ring())
            .effect(damage_effect(AreaCenter::Caster)),
        TargetType::Area => def.effect(nova_ring()).effect(damage_effect(AreaCenter::Aim)),
        TargetType::Unit | TargetType::Point => def.effect(AbilityEffect::SpawnProjectile {
            damage,
            damage_type: g.damage_type,
            splash_radius: AreaRadius::Ability,
            on_hit: Vec::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::AbilityType;

    #[test]
    fn rank_values_match_legacy_formulas() {
        let dash = builtin(AbilityId::Dash);
        assert_eq!(dash.target_type, TargetType::Point);
        assert!((dash.cooldown.at(3) - (7.5 - 3.0 * 0.35)).abs() < 1e-4);
        assert!((dash.cooldown.at(7) - 5.05).abs() < 1e-4);
        assert!((dash.mana_cost.at(2) - 45.0).abs() < 1e-4);
        assert_eq!(dash.cast_range.at(1), ABILITY_DASH_RANGE + 30.0);

        let execute = builtin(AbilityId::Execute);
        assert_eq!(execute.cooldown.at(7), 30.0);
        assert_eq!(execute.target_type, TargetType::Unit);
        assert_eq!(builtin(AbilityId::Nova).target_type, TargetType::Area);
        assert_eq!(builtin(AbilityId::ArcMissile).target_type, TargetType::Point);
    }

    #[test]
    fn generated_abilities_convert_to_definitions() {
        let slam = AbilityId::SeismicSlam.generated().unwrap();
        assert_eq!(slam.ability_type, AbilityType::Untargeted);
        let def = builtin(AbilityId::SeismicSlam);
        assert_eq!(def.target_type, TargetType::NoTarget);
        assert!((def.cooldown.at(1) - slam.cooldown_at(1)).abs() < 1e-4);
        assert!((def.cooldown.at(7) - slam.cooldown_at(7)).abs() < 1e-4);
        assert!((def.mana_cost.at(3) - slam.mana_cost_at(3)).abs() < 1e-4);
        assert!(def.effects.iter().any(|e| matches!(e, AbilityEffect::Damage { .. })));

        assert_eq!(builtin(AbilityId::ArcaneLance).target_type, TargetType::Unit);
        let stone = builtin(AbilityId::StoneSkin);
        assert_eq!(stone.behavior, AbilityBehavior::Passive);
        assert!(!stone.is_castable());
        assert_eq!(builtin(AbilityId::Overcharge).behavior, AbilityBehavior::Toggle);
        assert_eq!(builtin(AbilityId::Cataclysm).max_rank, 4);
    }

    #[test]
    fn loadouts_register_their_definitions() {
        let mut world = World::new();
        world.init_resource::<AbilityDefinitions>();
        world.spawn(AbilityLoadout::starter());
        world
            .run_system_once(register_loadout_definitions)
            .unwrap();
        let defs = world.resource::<AbilityDefinitions>();
        assert!(defs.get(AbilityId::Bolt).is_some());
        assert!(defs.get(AbilityId::Meteor).is_none());
    }

    use bevy::ecs::system::RunSystemOnce;
}
