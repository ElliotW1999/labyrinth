//! Built-in ability definitions and the [`AbilityDefinitions`] registry.
//!
//! Abilities are plain data: targeting, costs, mechanics, and trigger-tagged effects.
//! Rows from `data/ability_effects.csv` are appended to every definition. Abilities
//! with unique mechanics register a custom behavior in `custom.rs`.

use std::collections::HashMap;

use bevy::prelude::*;

use super::definition::{
    classify, AbilityBehavior, AbilityDefinition, AbilityEffect, AbilityMechanic,
    AbilityTrigger, AreaCenter, AreaRadius, EffectScaling, EffectTarget, RankValue, RingStyle,
    StatusSpec, TargetType,
};
use super::effect_rows::csv_effects_for;
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

const fn nova_ring() -> AbilityMechanic {
    AbilityMechanic::RingFx {
        center: AreaCenter::Aim,
        radius: AreaRadius::Ability,
        style: RingStyle::Nova,
        lifetime: 0.55,
    }
}

const fn shockwave_ring() -> AbilityMechanic {
    AbilityMechanic::RingFx {
        center: AreaCenter::Caster,
        radius: AreaRadius::Ability,
        style: RingStyle::Shockwave,
        lifetime: 0.45,
    }
}

const PROJECTILE: AbilityMechanic = AbilityMechanic::Projectile {
    splash_radius: AreaRadius::Ability,
};

const fn damage(amount: RankValue, damage_type: DamageType, scaling: EffectScaling) -> AbilityEffect {
    AbilityEffect::Damage {
        amount,
        damage_type,
        scaling,
    }
}

const fn magic_damage(amount: RankValue) -> AbilityEffect {
    damage(amount, DamageType::Magical, EffectScaling::None)
}

const fn status(status: StatusSpec, duration: RankValue) -> AbilityEffect {
    AbilityEffect::ApplyStatus { status, duration }
}

/// Splash victims of a projectile that struck its target take this share of the damage.
const SPLASH_DAMAGE_RATIO: f32 = 0.45;

/// Projectile damage: full to the struck unit, reduced splash around it, and full
/// splash when the projectile lands without striking a unit.
fn projectile_damage(
    def: AbilityDefinition,
    amount: RankValue,
    damage_type: DamageType,
    scaling: EffectScaling,
) -> AbilityDefinition {
    def.on(
        AbilityTrigger::OnProjectileHit,
        EffectTarget::TriggerUnit,
        damage(amount, damage_type, scaling),
    )
    .on(
        AbilityTrigger::OnProjectileHit,
        EffectTarget::AffectedUnits,
        damage(amount.scaled(SPLASH_DAMAGE_RATIO), damage_type, scaling),
    )
    .on(
        AbilityTrigger::OnImpact,
        EffectTarget::AffectedUnits,
        damage(amount, damage_type, scaling),
    )
}

/// Unit-targeted spells use a small fixed indicator / splash radius.
const UNIT_SPELL_RADIUS: RankValue = RankValue::fixed(20.0);

pub const EXECUTE_DAMAGE: RankValue = RankValue::linear(110.0, 40.0);
pub const EXECUTE_THRESHOLD: f32 = 0.35;
/// Total damage multiplier against targets below [`EXECUTE_THRESHOLD`].
pub const EXECUTE_MULTIPLIER: f32 = 1.55;

pub fn builtin(id: AbilityId) -> AbilityDefinition {
    let mut def = match id.generated() {
        Some(generated) => from_generated(id, generated),
        None => handwritten(id),
    };
    def.effects.extend(csv_effects_for(id));
    def
}

fn handwritten(id: AbilityId) -> AbilityDefinition {
    use AbilityTrigger::OnCast;
    let def = AbilityDefinition::new(id);
    let around_caster = EffectTarget::enemies_around(AreaCenter::Caster);
    let around_aim = EffectTarget::enemies_around(AreaCenter::Aim);
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
                .mechanic(AbilityMechanic::Dash { distance })
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
                .mechanic(shockwave_ring())
                .on(OnCast, around_caster, magic_damage(damage));
            match id {
                AbilityId::Shockwave => def.on(
                    OnCast,
                    EffectTarget::Caster,
                    status(StatusSpec::Forceful, RankValue::fixed(2.0)),
                ),
                AbilityId::FrostNova => def.on(
                    OnCast,
                    around_caster,
                    status(StatusSpec::Root, RankValue::fixed(1.4)),
                ),
                _ => def
                    .on(OnCast, around_caster, status(StatusSpec::Silence, RankValue::fixed(1.2)))
                    .on(OnCast, around_caster, status(StatusSpec::Disarm, RankValue::fixed(1.0))),
            }
        }
        AbilityId::Bolt => projectile_damage(
            def.targeting(
                TargetType::Unit,
                RankValue::linear(ABILITY_UNIT_CAST_RANGE, 25.0),
                UNIT_SPELL_RADIUS,
            )
            .costs(RankValue::linear(6.0, -0.3).at_least(3.0), RankValue::linear(45.0, 7.0))
            .timing(0.2, 0.3)
            .mechanic(PROJECTILE)
            .mechanic(AbilityMechanic::AttackUnitTarget),
            RankValue::linear(90.0, 30.0),
            DamageType::Magical,
            EffectScaling::None,
        ),
        AbilityId::ArcMissile => projectile_damage(
            def.targeting(
                TargetType::Point,
                RankValue::linear(ABILITY_GROUND_CAST_RANGE, 20.0),
                RankValue::linear(ABILITY_PROJECTILE_WIDTH, 8.0),
            )
            .costs(RankValue::linear(6.0, -0.3).at_least(3.0), RankValue::linear(45.0, 7.0))
            .timing(0.2, 0.3)
            .mechanic(PROJECTILE)
            .mechanic(AbilityMechanic::AttackUnitTarget),
            RankValue::linear(85.0, 28.0),
            DamageType::Magical,
            EffectScaling::None,
        ),
        // Base damage on every hit, plus a bonus that only lands on low-health targets.
        AbilityId::Execute => {
            let def = def
                .targeting(
                    TargetType::Unit,
                    RankValue::linear(ABILITY_UNIT_CAST_RANGE, 25.0),
                    UNIT_SPELL_RADIUS,
                )
                .costs(RankValue::linear(50.0, -4.0).at_least(30.0), RankValue::linear(100.0, 20.0))
                .timing(0.3, 0.4)
                .mechanic(PROJECTILE)
                .mechanic(AbilityMechanic::AttackUnitTarget);
            let def = projectile_damage(def, EXECUTE_DAMAGE, DamageType::Magical, EffectScaling::None);
            projectile_damage(
                def,
                EXECUTE_DAMAGE.scaled(EXECUTE_MULTIPLIER - 1.0),
                DamageType::Magical,
                EffectScaling::TargetHealthBelow(EXECUTE_THRESHOLD),
            )
        }
        AbilityId::Caltrops => def
            .targeting(
                TargetType::Area,
                RankValue::linear(ABILITY_GROUND_CAST_RANGE, 15.0),
                RankValue::linear(ABILITY_GROUND_AOE, 15.0),
            )
            .costs(RankValue::linear(8.0, -0.35).at_least(4.5), RankValue::linear(40.0, 6.0))
            .timing(0.15, 0.25)
            .mechanic(nova_ring())
            .on(OnCast, around_aim, magic_damage(RankValue::linear(50.0, 18.0))),
        AbilityId::Barrier => {
            let duration = RankValue::linear(3.5, 0.4);
            def.costs(RankValue::linear(14.0, -0.5).at_least(8.0), RankValue::linear(55.0, 8.0))
                .timing(0.1, 0.2)
                .mechanic(AbilityMechanic::RingFx {
                    center: AreaCenter::Caster,
                    radius: AreaRadius::Fixed(RankValue::fixed(scale::u(2.5))),
                    style: RingStyle::Nova,
                    lifetime: 0.4,
                })
                .on(OnCast, EffectTarget::Caster, status(StatusSpec::DebuffImmunity, duration))
                .on(
                    OnCast,
                    EffectTarget::Caster,
                    status(StatusSpec::armor_buff("barrier", RankValue::linear(6.0, 2.5)), duration),
                )
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
                .mechanic(nova_ring())
                .on(
                    OnCast,
                    EffectTarget::Caster,
                    AbilityEffect::Heal {
                        amount: RankValue::linear(100.0, 40.0),
                    },
                )
                .on(OnCast, around_aim, magic_damage(RankValue::linear(160.0, 55.0)))
            } else {
                ult_costs(def.targeting(
                    TargetType::Area,
                    RankValue::linear(ABILITY_GROUND_CAST_RANGE, 15.0),
                    RankValue::linear(ABILITY_ULT_AOE, 25.0),
                ))
                .mechanic(nova_ring())
                .on(OnCast, around_aim, magic_damage(RankValue::linear(180.0, 60.0)))
                .on(OnCast, around_aim, status(StatusSpec::Stun, RankValue::fixed(1.1)))
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
/// the activation type; pseudocode extras come from `data/ability_effects.csv`.
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

    let amount = RankValue::from_level_1(g.damage_base, g.damage_per_level).at_least(0.0);
    if behavior != AbilityBehavior::Active || g.damage_base <= 0.0 {
        return def;
    }
    let primary = damage(amount, g.damage_type, EffectScaling::None);
    match target_type {
        TargetType::NoTarget => def.mechanic(shockwave_ring()).on(
            AbilityTrigger::OnCast,
            EffectTarget::enemies_around(AreaCenter::Caster),
            primary,
        ),
        TargetType::Area => def.mechanic(nova_ring()).on(
            AbilityTrigger::OnCast,
            EffectTarget::enemies_around(AreaCenter::Aim),
            primary,
        ),
        TargetType::Unit | TargetType::Point => {
            projectile_damage(def.mechanic(PROJECTILE), amount, g.damage_type, EffectScaling::None)
        }
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
        assert!(def.effects.iter().any(|e| matches!(e.effect, AbilityEffect::Damage { .. })));

        assert_eq!(builtin(AbilityId::ArcaneLance).target_type, TargetType::Unit);
        let stone = builtin(AbilityId::StoneSkin);
        assert_eq!(stone.behavior, AbilityBehavior::Passive);
        assert!(!stone.is_castable());
        assert_eq!(builtin(AbilityId::Overcharge).behavior, AbilityBehavior::Toggle);
        assert_eq!(builtin(AbilityId::Cataclysm).max_rank, 4);
    }

    #[test]
    fn csv_effect_rows_extend_definitions() {
        let slam = builtin(AbilityId::SeismicSlam);
        assert!(slam.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Slow { .. },
                ..
            }
        )));
        let lance = builtin(AbilityId::ArcaneLance);
        assert!(lance.effects_for(AbilityTrigger::OnProjectileHit).any(|e| matches!(
            e.effect,
            AbilityEffect::Damage {
                scaling: EffectScaling::TargetHealthBelow(_),
                ..
            }
        )));
    }

    #[test]
    fn mechanics_and_effects_are_separate() {
        let bolt = builtin(AbilityId::Bolt);
        assert!(bolt.mechanics.contains(&PROJECTILE));
        assert_eq!(bolt.effects_for(AbilityTrigger::OnCast).count(), 0);
        assert_eq!(bolt.effects_for(AbilityTrigger::OnProjectileHit).count(), 2);
        assert_eq!(bolt.effects_for(AbilityTrigger::OnImpact).count(), 1);
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
