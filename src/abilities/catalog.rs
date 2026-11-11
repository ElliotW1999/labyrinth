//! Built-in ability definitions and the [`AbilityDefinitions`] registry.
//!
//! Abilities are plain data: targeting, costs, mechanics, and trigger-tagged effects.
//! CSV-authored abilities (`scripts/AbilityGenerator.py` → `generated.rs`) take
//! precedence over the hand-written ones below. Abilities with unique mechanics
//! register a custom behavior in `custom.rs`.

use std::collections::HashMap;

use bevy::prelude::*;

use super::definition::{
    AbilityDefinition, AbilityEffect, AbilityMechanic, AbilityTrigger, AreaCenter, AreaRadius,
    EffectScaling, EffectTarget, RankValue, RingStyle, StatusSpec, TargetTeam, TargetType,
};
use super::generated;
use crate::components::{AbilityId, AbilityLoadout, DamageType};
use crate::scale::{
    self, ABILITY_DASH_RANGE, ABILITY_GROUND_AOE, ABILITY_GROUND_CAST_RANGE, ABILITY_INSTANT_AOE,
    ABILITY_PROJECTILE_WIDTH, ABILITY_ULT_AOE, ABILITY_UNIT_CAST_RANGE,
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
    mut loadouts: Query<&mut AbilityLoadout, Added<AbilityLoadout>>,
) {
    for mut loadout in &mut loadouts {
        for slot in &mut loadout.slots {
            defs.ensure_builtin(slot.id);
            if let Some((max_charges, _)) = defs.get(slot.id).and_then(|def| def.charges) {
                slot.cooldown.init_charges(max_charges);
            }
        }
    }
}

/// Charge-based abilities regain one charge every `restore_time` while below the maximum.
pub fn restore_ability_charges(
    time: Res<Time>,
    defs: Res<AbilityDefinitions>,
    mut loadouts: Query<&mut AbilityLoadout>,
) {
    let dt = time.delta_secs();
    for mut loadout in &mut loadouts {
        for slot in &mut loadout.slots {
            let Some((max_charges, restore_time)) = defs.get(slot.id).and_then(|def| def.charges)
            else {
                continue;
            };
            slot.cooldown.restore_charges(max_charges, restore_time, dt);
        }
    }
}

pub const fn nova_ring() -> AbilityMechanic {
    AbilityMechanic::RingFx {
        center: AreaCenter::Aim,
        radius: AreaRadius::Ability,
        style: RingStyle::Nova,
        lifetime: 0.55,
    }
}

pub const fn shockwave_ring() -> AbilityMechanic {
    AbilityMechanic::RingFx {
        center: AreaCenter::Caster,
        radius: AreaRadius::Ability,
        style: RingStyle::Shockwave,
        lifetime: 0.45,
    }
}

const PROJECTILE: AbilityMechanic = AbilityMechanic::Projectile {
    splash_radius: AreaRadius::Ability,
    speed: None,
    skillshot: false,
    first_hit: false,
    end_radius: None,
    origin_offset: None,
    seek: None,
};

const fn damage(
    amount: RankValue,
    damage_type: DamageType,
    scaling: EffectScaling,
) -> AbilityEffect {
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
    generated::definition(id).unwrap_or_else(|| handwritten(id))
}

fn handwritten(id: AbilityId) -> AbilityDefinition {
    use AbilityTrigger::OnCast;
    let def = AbilityDefinition::new(id);
    let around_caster = EffectTarget::enemies_around(AreaCenter::Caster);
    let around_aim = EffectTarget::enemies_around(AreaCenter::Aim);
    match id {
        AbilityId::Dash => {
            let distance = RankValue::linear(ABILITY_DASH_RANGE, 30.0);
            def.targeting(TargetType::Point, distance, RankValue::fixed(48.0))
                .costs(
                    RankValue::linear(7.5, -0.35).at_least(4.0),
                    RankValue::linear(35.0, 5.0),
                )
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
                .costs(
                    RankValue::linear(9.0, -0.4).at_least(5.0),
                    RankValue::linear(50.0, 8.0),
                )
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
                    .on(
                        OnCast,
                        around_caster,
                        status(StatusSpec::Silence, RankValue::fixed(1.2)),
                    )
                    .on(
                        OnCast,
                        around_caster,
                        status(StatusSpec::Disarm, RankValue::fixed(1.0)),
                    ),
            }
        }
        AbilityId::Bolt => projectile_damage(
            def.targeting(
                TargetType::Unit,
                RankValue::linear(ABILITY_UNIT_CAST_RANGE, 25.0),
                UNIT_SPELL_RADIUS,
            )
            .costs(
                RankValue::linear(6.0, -0.3).at_least(3.0),
                RankValue::linear(45.0, 7.0),
            )
            .timing(0.2, 0.3)
            .mechanic(PROJECTILE)
            .mechanic(AbilityMechanic::AttackUnitTarget),
            RankValue::linear(90.0, 30.0),
            DamageType::Magical,
            EffectScaling::None,
        )
        .on(
            AbilityTrigger::OnProjectileHit,
            EffectTarget::TriggerUnit,
            AbilityEffect::Chain {
                jumps: 2,
                radius: RankValue::fixed(scale::u(450.0)),
                falloff: 0.75,
                team: TargetTeam::Enemy,
                then: AbilityTrigger::Custom("on_chain_hit"),
            },
        )
        .on(
            AbilityTrigger::Custom("on_chain_hit"),
            EffectTarget::TriggerUnit,
            damage(
                RankValue::linear(45.0, 12.0),
                DamageType::Magical,
                EffectScaling::None,
            ),
        ),
        AbilityId::ArcMissile => projectile_damage(
            def.targeting(
                TargetType::Point,
                RankValue::linear(ABILITY_GROUND_CAST_RANGE, 20.0),
                RankValue::linear(ABILITY_PROJECTILE_WIDTH, 8.0),
            )
            .costs(
                RankValue::linear(6.0, -0.3).at_least(3.0),
                RankValue::linear(45.0, 7.0),
            )
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
                .costs(
                    RankValue::linear(50.0, -4.0).at_least(30.0),
                    RankValue::linear(100.0, 20.0),
                )
                .timing(0.3, 0.4)
                .mechanic(PROJECTILE)
                .mechanic(AbilityMechanic::AttackUnitTarget);
            let def = projectile_damage(
                def,
                EXECUTE_DAMAGE,
                DamageType::Magical,
                EffectScaling::None,
            );
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
            .costs(
                RankValue::linear(8.0, -0.35).at_least(4.5),
                RankValue::linear(40.0, 6.0),
            )
            .timing(0.15, 0.25)
            .mechanic(nova_ring())
            .on(
                OnCast,
                around_aim,
                magic_damage(RankValue::linear(50.0, 18.0)),
            ),
        AbilityId::Barrier => {
            let duration = RankValue::linear(3.5, 0.4);
            def.costs(
                RankValue::linear(14.0, -0.5).at_least(8.0),
                RankValue::linear(55.0, 8.0),
            )
            .timing(0.1, 0.2)
            .mechanic(AbilityMechanic::RingFx {
                center: AreaCenter::Caster,
                radius: AreaRadius::Fixed(RankValue::fixed(scale::u(2.5))),
                style: RingStyle::Nova,
                lifetime: 0.4,
            })
            .on(
                OnCast,
                EffectTarget::Caster,
                status(StatusSpec::DebuffImmunity, duration),
            )
            .on(
                OnCast,
                EffectTarget::Caster,
                status(
                    StatusSpec::armor_buff("barrier", RankValue::linear(6.0, 2.5)),
                    duration,
                ),
            )
            .on(
                OnCast,
                EffectTarget::Caster,
                status(
                    StatusSpec::Shield {
                        amount: RankValue::linear(80.0, 25.0),
                    },
                    duration,
                ),
            )
        }
        AbilityId::Nova | AbilityId::Meteor => {
            let ult_costs = |d: AbilityDefinition| {
                d.costs(
                    RankValue::linear(50.0, -4.0).at_least(30.0),
                    RankValue::linear(100.0, 20.0),
                )
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
                        scaling: EffectScaling::None,
                    },
                )
                .on(
                    OnCast,
                    around_aim,
                    magic_damage(RankValue::linear(160.0, 55.0)),
                )
            } else {
                ult_costs(def.targeting(
                    TargetType::Area,
                    RankValue::linear(ABILITY_GROUND_CAST_RANGE, 15.0),
                    RankValue::linear(ABILITY_ULT_AOE, 25.0),
                ))
                .mechanic(nova_ring())
                .on(
                    OnCast,
                    around_aim,
                    magic_damage(RankValue::linear(180.0, 60.0)),
                )
                .on(
                    OnCast,
                    around_aim,
                    status(StatusSpec::Stun, RankValue::fixed(1.1)),
                )
            }
        }
        // HeroGenerator stubs: castable, costed, no effects yet.
        _ if id.is_ultimate() => def
            .costs(
                RankValue::linear(50.0, -4.0).at_least(30.0),
                RankValue::linear(100.0, 20.0),
            )
            .timing(0.3, 0.35),
        _ => def
            .costs(
                RankValue::linear(8.0, -0.35).at_least(4.0),
                RankValue::linear(40.0, 6.0),
            )
            .timing(0.2, 0.25),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::definition::AbilityBehavior;

    #[test]
    fn timing_clamps_at_max_cast_point_and_backswing() {
        use crate::abilities::definition::{MAX_CAST_BACKSWING, MAX_CAST_POINT};
        assert_eq!(MAX_CAST_POINT, 2.0);
        assert_eq!(MAX_CAST_BACKSWING, 2.0);
        let d = builtin(AbilityId::Dash).timing(0.9, 0.9);
        assert_eq!(d.cast_time, 0.9, "cast points up to 2.0 apply unclamped");
        assert_eq!(d.backswing, 0.9);
        let d = builtin(AbilityId::Dash).timing(2.0, 1.6);
        assert_eq!((d.cast_time, d.backswing), (2.0, 1.6));
        let d = builtin(AbilityId::Dash).timing(5.0, 7.0);
        assert_eq!(d.cast_time, MAX_CAST_POINT);
        assert_eq!(d.backswing, MAX_CAST_BACKSWING);
        let d = builtin(AbilityId::Dash).timing(-1.0, -1.0);
        assert_eq!((d.cast_time, d.backswing), (0.0, 0.0));
    }

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
        assert_eq!(
            builtin(AbilityId::ArcMissile).target_type,
            TargetType::Point
        );
    }

    #[test]
    fn generated_blink_is_a_pure_teleport_definition() {
        let blink = builtin(AbilityId::Blink);
        assert!(blink.mechanics.is_empty());
        assert!(
            blink
                .effects_for(AbilityTrigger::OnCast)
                .any(|e| matches!(e.effect, AbilityEffect::Teleport { .. }))
        );
    }

    #[test]
    fn queen_of_pain_abilities_generate_expected_definitions() {
        use crate::abilities::definition::{AreaCenter, StatusSpec};
        use crate::actions::TargetType;

        let strike = builtin(AbilityId::ShadowStrike);
        assert_eq!(strike.max_rank, 7);
        assert_eq!(strike.target_type, TargetType::Unit);
        let strike_cooldowns: Vec<f32> = (1..=7).map(|r| strike.cooldown.at(r)).collect();
        assert_eq!(strike_cooldowns, [11.0, 9.0, 7.0, 5.0, 3.0, 3.0, 3.0]);
        assert_eq!(strike.cast_range.at(1), 450.0);
        assert_eq!(strike.cast_range.at(7), 750.0);
        let (burn_damage, burn_interval, burn_duration) = strike
            .effects_for(AbilityTrigger::OnProjectileHit)
            .find_map(|e| match e.effect {
                AbilityEffect::ApplyStatus {
                    status:
                        StatusSpec::Burn {
                            id,
                            amount,
                            tick_interval,
                            ..
                        },
                    duration,
                } => {
                    assert_eq!(id, "shadow_strike_burn");
                    Some((amount, tick_interval, duration))
                }
                _ => None,
            })
            .expect("Shadow Strike burns");
        assert_eq!(burn_damage.at(1), 20.0);
        assert_eq!(burn_damage.at(7), 140.0);
        assert_eq!(burn_interval.at(1), 3.0);
        assert_eq!(burn_duration.at(1), 16.0);

        let blink = builtin(AbilityId::QueenOfPainBlink);
        assert_eq!(blink.name, "QoP Blink");
        assert_eq!(blink.max_rank, 7);
        assert!(blink.cast_range.at(1) >= crate::scale::GLOBAL_CAST_RANGE);
        let blink_cooldowns: Vec<f32> = (1..=7).map(|r| blink.cooldown.at(r)).collect();
        assert_eq!(blink_cooldowns, [12.0, 10.0, 8.0, 6.0, 4.0, 2.0, 1.0]);
        let (min, max) = blink
            .effects_for(AbilityTrigger::OnCast)
            .find_map(|e| match e.effect {
                AbilityEffect::Teleport {
                    min_distance,
                    max_distance,
                } => Some((min_distance, max_distance.expect("bounded"))),
                _ => None,
            })
            .expect("a bounded teleport");
        assert_eq!(min.at(1), 200.0);
        assert_eq!(max.at(1), 1075.0);
        assert_eq!(max.at(7), 1525.0);

        let scream = builtin(AbilityId::ScreamOfPain);
        assert_eq!(scream.max_rank, 7);
        assert!(scream.mechanics.iter().any(|m| matches!(
            m,
            AbilityMechanic::ProjectileToEach {
                center: AreaCenter::Caster,
                team: TargetTeam::Enemy,
                speed: Some(speed),
            } if *speed == 900.0
        )));
        assert_eq!(scream.aoe_radius.at(1), 600.0);

        let wave = builtin(AbilityId::SonicWave);
        assert_eq!(wave.max_rank, 4, "ultimates have four ranks");
        assert!(wave.is_ultimate);
        let wave_cooldowns: Vec<f32> = (1..=4).map(|r| wave.cooldown.at(r)).collect();
        assert_eq!(wave_cooldowns, [110.0, 95.0, 80.0, 65.0]);
        let end_radius = wave
            .mechanics
            .iter()
            .find_map(|m| match m {
                AbilityMechanic::Projectile {
                    skillshot: true,
                    end_radius,
                    ..
                } => *end_radius,
                _ => None,
            })
            .expect("an expanding skillshot");
        assert_eq!(wave.aoe_radius.at(1), 100.0);
        assert_eq!(end_radius.at(1), 450.0);
        let wave_burn = wave
            .effects_for(AbilityTrigger::OnProjectileHit)
            .find_map(|e| match e.effect {
                AbilityEffect::ApplyStatus {
                    status:
                        StatusSpec::Burn {
                            id,
                            amount,
                            tick_interval,
                            damage_type,
                        },
                    duration,
                } => Some((id, amount, tick_interval, damage_type, duration)),
                _ => None,
            })
            .expect("Sonic Wave burns");
        assert_eq!(wave_burn.0, "sonic_wave_burn");
        let totals: Vec<f32> = (1..=4).map(|r| wave_burn.1.at(r) * 14.0).collect();
        assert_eq!(totals, [322.0, 476.0, 630.0, 784.0]);
        assert_eq!(wave_burn.2.at(1), 0.1);
        assert_eq!(wave_burn.3, DamageType::Pure);
        assert_eq!(wave_burn.4.at(1), 1.4);
    }

    #[test]
    fn magnus_abilities_generate_expected_definitions() {
        use crate::abilities::definition::{DisplacementKind, StatusSpec};
        use crate::actions::TargetType;

        // Vanguard's hand-written Shockwave is untouched by Magnus' generated Q.
        assert!(
            crate::abilities::generated::definition(AbilityId::Shockwave).is_none(),
            "Magnus' Q must not replace Vanguard's Shockwave"
        );
        assert_eq!(AbilityId::MagnusShockwave.display_name(), "Shockwave");
        assert_eq!(AbilityId::Empower.display_name(), "Empower");
        assert_eq!(AbilityId::Skewer.display_name(), "Skewer");
        assert_eq!(
            AbilityId::ReversePolarity.display_name(),
            "Reverse Polarity"
        );

        // Q: skillshot wave starting 48 in front, pulling toward the wave and slowing.
        let q = builtin(AbilityId::MagnusShockwave);
        assert_eq!(q.name, "Shockwave");
        assert_eq!(q.max_rank, 7);
        assert_eq!(q.target_type, TargetType::Point);
        assert_eq!(q.cast_range.at(1), 1200.0);
        assert_eq!(q.aoe_radius.at(1), 200.0);
        let q_cooldowns: Vec<f32> = (1..=7).map(|r| q.cooldown.at(r)).collect();
        assert_eq!(q_cooldowns, [13.0, 12.0, 11.0, 10.0, 9.0, 8.0, 7.0]);
        assert_eq!(q.backswing, 1.2, "unclamped under the 1.5 cap");
        assert!(q.mechanics.iter().any(|m| matches!(
            m,
            AbilityMechanic::Projectile {
                skillshot: true,
                first_hit: false,
                speed: Some(speed),
                end_radius: None,
                origin_offset: Some(offset),
                ..
            } if *speed == 1200.0 && offset.at(1) == 48.0
        )));
        let hit: Vec<_> = q.effects_for(AbilityTrigger::OnProjectileHit).collect();
        assert!(hit.iter().any(|e| matches!(
            e.effect,
            AbilityEffect::Displace {
                kind: DisplacementKind::PullToPoint,
                distance: Some(distance),
                forward_offset: None,
                ..
            } if distance.at(1) == 150.0
        )));
        assert!(hit.iter().any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Slow { percent, .. },
                duration,
            } if percent.at(1) == 75.0 && duration.at(1) == 0.55 && duration.at(7) == 1.45
        )));
        assert!(hit.iter().any(|e| matches!(
            e.effect,
            AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
                if amount.at(1) == 75.0 && amount.at(7) == 525.0
        )));

        // W: ally unit target; percent damage, cleave ratio and the cone shape.
        let w = builtin(AbilityId::Empower);
        assert_eq!(w.target_type, TargetType::Unit);
        assert_eq!(w.backswing, 1.07);
        assert_eq!(w.cooldown.at(1), 8.0);
        assert_eq!(w.cast_range.at(1), 800.0);
        let modifiers: Vec<_> = w
            .effects_for(AbilityTrigger::OnCast)
            .filter_map(|e| match e.effect {
                AbilityEffect::ApplyStatus {
                    status:
                        StatusSpec::Modifier {
                            id,
                            attack_damage,
                            attack_damage_pct,
                            cleave,
                            cleave_radius,
                            cleave_end_radius,
                            cleave_distance,
                            ..
                        },
                    duration,
                } => Some((
                    id,
                    attack_damage,
                    attack_damage_pct,
                    cleave,
                    cleave_radius,
                    cleave_end_radius,
                    cleave_distance,
                    duration,
                )),
                _ => None,
            })
            .collect();
        assert_eq!(modifiers.len(), 5);
        for m in &modifiers {
            assert_eq!(m.7.at(1), 30.0);
            assert_eq!(m.7.at(7), 48.0);
            assert_eq!(m.1.at(1), 0.0, "no flat attack damage");
        }
        let by_id = |id: &str| modifiers.iter().find(|m| m.0 == id).expect(id);
        assert_eq!(by_id("empower_attack_damage_pct").2.at(1), 16.0);
        assert_eq!(by_id("empower_attack_damage_pct").2.at(7), 64.0);
        assert_eq!(by_id("empower_cleave").3.at(1), 0.16);
        assert!((by_id("empower_cleave").3.at(7) - 0.64).abs() < 1e-6);
        assert_eq!(by_id("empower_cleave_radius").4.at(1), 150.0);
        assert_eq!(by_id("empower_cleave_end_radius").5.at(1), 360.0);
        assert_eq!(by_id("empower_cleave_distance").6.at(7), 650.0);

        // E: global dash with a 145 contact radius, dragging and stunning under its own id.
        let e = builtin(AbilityId::Skewer);
        assert_eq!(e.target_type, TargetType::Point);
        assert_eq!(e.cast_range.at(1), crate::scale::GLOBAL_CAST_RANGE);
        let e_cooldowns: Vec<f32> = (1..=7).map(|r| e.cooldown.at(r)).collect();
        assert_eq!(e_cooldowns, [22.0, 20.0, 18.0, 16.0, 14.0, 12.0, 10.0]);
        assert!(e.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            x.effect,
            AbilityEffect::Displace {
                kind: DisplacementKind::Dash,
                distance: Some(distance),
                duration,
                contact_radius: Some(radius),
                ..
            } if distance.at(1) == 800.0
                && distance.at(7) == 1400.0
                && (duration.at(7) - 1.555556).abs() < 1e-5
                && radius.at(1) == 145.0
        )));
        assert!(
            e.effects_for(AbilityTrigger::OnUnitContact)
                .any(|x| matches!(
                    x.effect,
                    AbilityEffect::Displace {
                        kind: DisplacementKind::Drag,
                        bound_stun: Some("skewer_stun"),
                        ..
                    }
                ))
        );
        let end: Vec<_> = e.effects_for(AbilityTrigger::OnMovementEnd).collect();
        assert_eq!(end.len(), 2, "damage and slow only: no stun-removal row");
        assert!(end.iter().any(|x| matches!(
            x.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Slow { percent, .. },
                duration,
            } if percent.at(1) == 20.0 && percent.at(7) == 80.0 && duration.at(1) == 3.25
        )));
        assert!(
            !e.effects
                .iter()
                .any(|x| matches!(x.effect, AbilityEffect::RemoveStatus { .. })),
            "no remove_status,stun"
        );

        // R: ultimate, radius 430, pull to a point 250 in front, damage and stun.
        let r = builtin(AbilityId::ReversePolarity);
        assert!(r.is_ultimate);
        assert_eq!(r.max_rank, 4);
        assert_eq!(r.target_type, TargetType::NoTarget);
        assert_eq!(r.aoe_radius.at(1), 430.0);
        assert_eq!(r.backswing, 1.3);
        assert_eq!(r.cooldown.at(4), 115.0);
        assert!(r.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            x.effect,
            AbilityEffect::Displace {
                kind: DisplacementKind::PullToPoint,
                distance: None,
                forward_offset: Some(offset),
                ..
            } if offset.at(1) == 250.0
        )));
        assert!(r.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            x.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Stun,
                duration,
            } if duration.at(1) == 2.5 && duration.at(4) == 4.0
        )));
        assert!(r.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            x.effect,
            AbilityEffect::Damage { amount, .. } if amount.at(4) == 400.0
        )));
    }

    #[test]
    fn generated_abilities_build_normal_definitions() {
        let slam = builtin(AbilityId::SeismicSlam);
        assert_eq!(slam.target_type, TargetType::NoTarget);
        assert!((slam.cooldown.at(1) - 10.0).abs() < 1e-4);
        assert!((slam.cooldown.at(7) - 7.6).abs() < 1e-4);
        assert!((slam.mana_cost.at(3) - 71.0).abs() < 1e-4);
        assert!(slam.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Slow { .. },
                ..
            }
        )));

        let lance = builtin(AbilityId::ArcaneLance);
        assert_eq!(lance.target_type, TargetType::Unit);
        assert!(
            lance
                .effects_for(AbilityTrigger::OnProjectileHit)
                .any(|e| matches!(
                    e.effect,
                    AbilityEffect::Damage {
                        scaling: EffectScaling::TargetHealthBelow(_),
                        ..
                    }
                ))
        );

        let stone = builtin(AbilityId::StoneSkin);
        assert_eq!(stone.behavior, AbilityBehavior::Passive);
        assert!(!stone.is_castable());
        assert_eq!(
            builtin(AbilityId::Overcharge).behavior,
            AbilityBehavior::Toggle
        );
        let cataclysm = builtin(AbilityId::Cataclysm);
        assert_eq!(cataclysm.max_rank, 4);
        assert!(cataclysm.is_ultimate);
        assert_eq!(cataclysm.target_type, TargetType::Area);
    }

    #[test]
    fn level_tables_and_linear_values_share_rank_value() {
        let table = RankValue::levels(&[75.0, 150.0, 260.0]);
        assert_eq!(table.at(0), 75.0);
        assert_eq!(table.at(3), 260.0);
        assert_eq!(table.at(9), 260.0);
        assert_eq!(table.scaled(0.5).at(2), 75.0);
        assert_eq!(RankValue::from_level_1(10.0, 2.0).scaled(0.5).at(2), 6.0);
    }

    #[test]
    fn sniper_abilities_generate_expected_definitions() {
        use crate::abilities::definition::{AreaRadius, DisplacementKind, PassiveStat, StatusSpec};
        use crate::actions::TargetType;

        assert_eq!(AbilityId::Shrapnel.display_name(), "Shrapnel");
        assert_eq!(AbilityId::Headshot.display_name(), "Headshot");
        assert_eq!(AbilityId::TakeAim.display_name(), "Take Aim");
        assert_eq!(AbilityId::Assassinate.display_name(), "Assassinate");
        assert!(AbilityId::Assassinate.is_ultimate());
        assert!(!AbilityId::Shrapnel.is_ultimate());

        // Q: ground-targeted zone, 1.2 s delay, then a tick every second for 10 s.
        let q = builtin(AbilityId::Shrapnel);
        assert_eq!(q.max_rank, 7);
        assert_eq!(q.target_type, TargetType::Area);
        assert_eq!(q.cast_range.at(1), 1800.0);
        assert_eq!(q.aoe_radius.at(1), 400.0);
        assert_eq!(q.aoe_radius.at(7), 550.0);
        assert_eq!(q.cast_time, 0.3);
        assert_eq!(q.backswing, 0.0);
        assert_eq!(q.cooldown.at(1), 10.0);
        assert_eq!(q.mana_cost.at(1), 75.0);
        assert!(q.charges.is_none());
        let zone: Vec<_> = q.effects_for(AbilityTrigger::OnCast).collect();
        assert_eq!(zone.len(), 1);
        assert!(matches!(
            zone[0].effect,
            AbilityEffect::Zone {
                radius: AreaRadius::Ability,
                team: TargetTeam::Enemy,
                delay,
                duration,
                interval,
                then: AbilityTrigger::Custom("on_shrapnel_tick"),
                follow_caster: false,
            } if delay.at(1) == 1.2 && duration.at(1) == 10.0 && interval.at(1) == 1.0
        ));
        let tick = AbilityTrigger::Custom("on_shrapnel_tick");
        assert!(q.effects_for(tick).any(|e| matches!(
            e.effect,
            AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
                if amount.at(1) == 30.0 && amount.at(7) == 120.0
        )));
        assert!(q.effects_for(tick).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Slow { id: "shrapnel_slow", percent },
                duration,
            } if percent.at(1) == 12.0 && percent.at(7) == 48.0 && duration.at(1) == 1.1
        )));

        // W: passive 40% pseudo-random proc on attack hit.
        let w = builtin(AbilityId::Headshot);
        assert_eq!(w.behavior, AbilityBehavior::Passive);
        assert!(w.effects_for(AbilityTrigger::OnAttackHit).any(|e| matches!(
            e.effect,
            AbilityEffect::Proc {
                chance,
                then: AbilityTrigger::Custom("on_headshot"),
                pseudo: true,
            } if (chance.at(1) - 0.4).abs() < 1e-6
        )));
        let headshot = AbilityTrigger::Custom("on_headshot");
        assert!(w.effects_for(headshot).any(|e| matches!(
            e.effect,
            AbilityEffect::Damage { amount, damage_type: DamageType::Physical, .. }
                if amount.at(1) == 20.0 && amount.at(7) == 200.0
        )));
        assert!(w.effects_for(headshot).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Slow { id: "headshot_slow", percent },
                duration,
            } if percent.at(1) == 100.0 && duration.at(1) == 0.2 && duration.at(7) == 0.8
        )));
        assert!(w.effects_for(headshot).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Modifier { id: "headshot_attack_speed_flat", attack_speed_flat, .. },
                ..
            } if attack_speed_flat.at(1) == -100.0
        )));
        assert!(w.effects_for(headshot).any(|e| matches!(
            e.effect,
            AbilityEffect::Displace {
                kind: DisplacementKind::Knockback,
                distance: Some(distance),
                duration,
                ..
            } if distance.at(1) == 50.0 && duration.at(1) == 0.05
        )));

        // E: attack range bonus, written as the total minus the 550 base.
        let e = builtin(AbilityId::TakeAim);
        assert!(e.effects_for(AbilityTrigger::Passive).any(|x| matches!(
            x.effect,
            AbilityEffect::PassiveStat { stat: PassiveStat::AttackRange, amount }
                if amount.at(1) == 235.0 && amount.at(7) == 1165.0
        )));

        // R: locked-on homing shot with a 2 s aim and a 600 leash buffer.
        let r = builtin(AbilityId::Assassinate);
        assert!(r.is_ultimate);
        assert_eq!(r.max_rank, 4);
        assert_eq!(r.target_type, TargetType::Unit);
        assert_eq!(r.cast_range.at(1), 3000.0);
        assert_eq!(r.cast_range_buffer.at(1), 600.0);
        assert_eq!(r.cast_time, 2.0);
        assert_eq!(r.backswing, 1.6);
        assert_eq!(r.mana_cost.at(1), 175.0);
        let r_cooldowns: Vec<f32> = (1..=4).map(|rank| r.cooldown.at(rank)).collect();
        assert_eq!(r_cooldowns, [20.0, 15.0, 10.0, 5.0]);
        assert!(r.mechanics.iter().any(|m| matches!(
            m,
            AbilityMechanic::Projectile {
                skillshot: false,
                speed: Some(speed),
                ..
            } if *speed == 1500.0
        )));
        assert!(
            r.effects_for(AbilityTrigger::OnProjectileHit)
                .any(|e| matches!(
                    e.effect,
                    AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
                        if amount.at(1) == 300.0 && amount.at(4) == 600.0
                ))
        );
        assert!(
            r.effects_for(AbilityTrigger::OnProjectileHit)
                .any(|e| matches!(
                    e.effect,
                    AbilityEffect::ApplyStatus {
                        status: StatusSpec::OwnedStun { id: "assassinate_stun" },
                        duration,
                    } if duration.at(1) == 0.1
                ))
        );
    }

    #[test]
    fn ursa_abilities_generate_expected_definitions() {
        use crate::abilities::definition::{
            AbilityEffectEntry, AreaCenter, AreaRadius, DisplacementKind, StatusSpec,
        };
        use crate::actions::TargetType;
        use crate::status::DispelStrength;

        assert_eq!(AbilityId::Earthshock.display_name(), "Earthshock");
        assert_eq!(AbilityId::Overpower.display_name(), "Overpower");
        assert_eq!(AbilityId::FurySwipes.display_name(), "Fury Swipes");
        assert_eq!(AbilityId::Enrage.display_name(), "Enrage");
        assert!(AbilityId::Enrage.is_ultimate());
        assert!(!AbilityId::Earthshock.is_ultimate());

        // Q: no-target leap of 250 over 0.25 s, then a landing-point shock.
        let q = builtin(AbilityId::Earthshock);
        assert_eq!(q.max_rank, 7);
        assert_eq!(q.target_type, TargetType::NoTarget);
        assert_eq!(q.aoe_radius.at(1), 385.0);
        assert_eq!(q.cast_time, 0.0);
        assert_eq!(q.backswing, 0.0);
        assert_eq!(q.mana_cost.at(1), 95.0);
        let cooldowns: Vec<f32> = (1..=7).map(|rank| q.cooldown.at(rank)).collect();
        assert_eq!(cooldowns, [15.0, 13.0, 11.0, 9.0, 7.0, 5.0, 3.0]);
        assert!(q.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            e.effect,
            AbilityEffect::Displace {
                kind: DisplacementKind::Dash,
                distance: Some(distance),
                duration,
                ..
            } if distance.at(1) == 250.0 && duration.at(1) == 0.25
        )));
        let shock = |x: &&AbilityEffectEntry| {
            matches!(
                x.target,
                EffectTarget::UnitsInRadius {
                    center: AreaCenter::TriggerPoint,
                    radius: AreaRadius::Ability,
                    team: TargetTeam::Enemy,
                }
            )
        };
        assert!(
            q.effects_for(AbilityTrigger::OnMovementEnd)
                .filter(shock)
                .any(|e| matches!(
                    e.effect,
                    AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
                        if amount.at(1) == 75.0 && amount.at(7) == 375.0
                ))
        );
        assert!(
            q.effects_for(AbilityTrigger::OnMovementEnd)
                .filter(shock)
                .any(|e| matches!(
                    e.effect,
                    AbilityEffect::ApplyStatus {
                        status: StatusSpec::Slow { id: "earthshock_slow", percent },
                        duration,
                    } if percent.at(1) == 22.0 && percent.at(7) == 58.0 && duration.at(1) == 4.0
                ))
        );

        // W: +400 attack speed for the next 3 .. 9 attacks, 20 s at most.
        let w = builtin(AbilityId::Overpower);
        assert_eq!(w.cast_time, 0.3);
        assert_eq!(w.mana_cost.at(1), 45.0);
        assert_eq!(w.mana_cost.at(7), 75.0);
        assert_eq!(w.cooldown.at(1), 12.0);
        assert_eq!(w.cooldown.at(7), 6.0);
        assert!(w.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Modifier {
                    id: "overpower_attack_speed_flat",
                    attack_speed_flat,
                    attack_charges,
                    ..
                },
                duration,
            } if attack_speed_flat.at(1) == 400.0
                && attack_charges.at(1) == 3.0
                && attack_charges.at(7) == 9.0
                && duration.at(1) == 20.0
        )));

        // E: passive stacking debuff plus stack-scaled physical bonus damage.
        let e = builtin(AbilityId::FurySwipes);
        assert_eq!(e.behavior, AbilityBehavior::Passive);
        assert!(e.effects_for(AbilityTrigger::OnAttackHit).any(|x| matches!(
            x.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Stacks { id: "fury_swipes_stacks" },
                duration,
            } if duration.at(1) == 8.0 && duration.at(7) == 32.0
        )));
        assert!(e.effects_for(AbilityTrigger::OnAttackHit).any(|x| matches!(
            x.effect,
            AbilityEffect::Damage {
                amount,
                damage_type: DamageType::Physical,
                scaling: EffectScaling::TargetStacks { id: "fury_swipes_stacks", offset: 1 },
            } if amount.at(1) == 12.0 && amount.at(7) == 60.0
        )));

        // R: strong dispel on herself, then 80% damage reduction and 50% status resistance.
        let r = builtin(AbilityId::Enrage);
        assert!(r.is_ultimate);
        assert_eq!(r.max_rank, 4);
        assert_eq!(r.mana_cost.at(1), 0.0);
        let r_cooldowns: Vec<f32> = (1..=4).map(|rank| r.cooldown.at(rank)).collect();
        assert_eq!(r_cooldowns, [60.0, 45.0, 30.0, 15.0]);
        let on_cast: Vec<_> = r.effects_for(AbilityTrigger::OnCast).collect();
        assert_eq!(on_cast.len(), 3, "dispel first, then the two buffs");
        assert!(matches!(
            on_cast[0].effect,
            AbilityEffect::Dispel {
                kind: crate::status::StatusKind::Debuff,
                strength: DispelStrength::Strong,
            }
        ));
        assert!(matches!(
            on_cast[1].effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::DamageReduction { id: "enrage_damage_reduction", percent },
                duration,
            } if percent.at(1) == 80.0 && duration.at(1) == 4.0 && duration.at(4) == 5.5
        ));
        assert!(matches!(
            on_cast[2].effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::StatusResistance { id: "enrage_status_resistance", percent },
                duration,
            } if percent.at(1) == 50.0 && duration.at(4) == 5.5
        ));
        assert!(
            on_cast
                .iter()
                .all(|x| matches!(x.target, EffectTarget::Caster))
        );
    }

    #[test]
    fn slark_abilities_generate_expected_definitions() {
        use crate::abilities::definition::{
            AreaCenter, AreaRadius, DisplacementKind, LeashAnchorKind, StatusSpec,
        };
        use crate::actions::TargetType;
        use crate::status::DispelStrength;
        use crate::vitals::ResourceStat;

        assert_eq!(AbilityId::DarkPact.display_name(), "Dark Pact");
        assert_eq!(AbilityId::Pounce.display_name(), "Pounce");
        assert_eq!(AbilityId::EssenceShift.display_name(), "Essence Shift");
        assert_eq!(AbilityId::ShadowDance.display_name(), "Shadow Dance");
        assert!(AbilityId::ShadowDance.is_ultimate());
        assert!(!AbilityId::DarkPact.is_ultimate());

        // Q: a delayed zone that follows Slark, dispelling him and draining enemies each pulse.
        let q = builtin(AbilityId::DarkPact);
        assert_eq!(q.max_rank, 7);
        assert_eq!(q.target_type, TargetType::NoTarget);
        assert_eq!(q.aoe_radius.at(1), 325.0);
        assert_eq!(q.mana_cost.at(1), 65.0);
        assert_eq!(q.cooldown.at(1), 9.0);
        assert_eq!(q.cooldown.at(7), 3.0);
        assert!(q.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            e.effect,
            AbilityEffect::Zone {
                radius: AreaRadius::Ability,
                team: TargetTeam::Enemy,
                delay,
                duration,
                interval,
                then: AbilityTrigger::Custom("on_dark_pact_pulse"),
                follow_caster: true,
            } if delay.at(1) == 1.5 && duration.at(1) == 1.0 && interval.at(1) == 0.1
        )));
        let pulse = AbilityTrigger::Custom("on_dark_pact_pulse");
        assert!(q.effects_for(pulse).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::Caster,
                AbilityEffect::Dispel {
                    kind: crate::status::StatusKind::Debuff,
                    strength: DispelStrength::Basic,
                }
            )
        )));
        assert!(q.effects_for(pulse).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::AffectedUnits,
                AbilityEffect::DrainHealth {
                    amount,
                    damage_type: DamageType::Magical,
                    transfer_ratio,
                    ..
                }
            ) if amount.at(1) == 7.5 && amount.at(7) == 52.5 && transfer_ratio == 0.15
        )));

        // W: a 700 dash with a 120 contact radius, then a leash of 325 at the landing point.
        let w = builtin(AbilityId::Pounce);
        assert_eq!(w.max_rank, 7);
        assert_eq!(w.target_type, TargetType::NoTarget);
        assert_eq!(w.cast_time, 0.0);
        assert_eq!(w.backswing, 0.53);
        assert_eq!(w.mana_cost.at(1), 75.0);
        assert_eq!(w.cooldown.at(1), 24.0);
        assert_eq!(w.cooldown.at(7), 2.0);
        assert!(w.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            e.effect,
            AbilityEffect::Displace {
                kind: DisplacementKind::Dash,
                distance: Some(distance),
                duration,
                contact_radius: Some(contact),
                ..
            } if distance.at(1) == 700.0 && duration.at(1) == 0.75 && contact.at(1) == 120.0
        )));
        assert!(
            w.effects_for(AbilityTrigger::OnUnitContact)
                .any(|e| matches!(
                    (e.target, e.effect),
                    (
                        EffectTarget::TriggerUnit,
                        AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
                    ) if amount.at(1) == 100.0 && amount.at(7) == 460.0
                ))
        );
        assert!(w.effects_for(AbilityTrigger::OnMovementEnd).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::UnitsInRadius {
                    center: AreaCenter::TriggerPoint,
                    radius: AreaRadius::Fixed(area),
                    team: TargetTeam::Enemy,
                },
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::Leash { radius, anchor: LeashAnchorKind::TriggerPoint },
                    duration,
                }
            ) if area.at(1) == 120.0
                && radius.at(1) == 325.0
                && duration.at(1) == 2.5
                && duration.at(7) == 4.0
        )));

        // E: a passive with two independent statuses.
        let e = builtin(AbilityId::EssenceShift);
        assert_eq!(e.behavior, AbilityBehavior::Passive);
        let hit: Vec<_> = e.effects_for(AbilityTrigger::OnAttackHit).collect();
        assert_eq!(hit.len(), 2);
        assert!(matches!(
            (hit[0].target, hit[0].effect),
            (
                EffectTarget::TriggerUnit,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::Modifier {
                        id: "essence_shift_attack_damage",
                        kind: crate::status::StatusKind::Debuff,
                        attack_damage,
                        independent: true,
                        ..
                    },
                    duration,
                }
            ) if attack_damage.at(1) == -1.0 && duration.at(1) == 30.0 && duration.at(7) == 150.0
        ));
        assert!(matches!(
            (hit[1].target, hit[1].effect),
            (
                EffectTarget::Caster,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::ResourceModifier {
                        id: "essence_shift_agility",
                        kind: crate::status::StatusKind::Buff,
                        stat: ResourceStat::Agility,
                        amount,
                        independent: true,
                    },
                    duration,
                }
            ) if amount.at(1) == 3.0 && duration.at(1) == 30.0 && duration.at(7) == 150.0
        ));

        // R: untargetable, move speed bonus and health regeneration.
        let r = builtin(AbilityId::ShadowDance);
        assert_eq!(r.max_rank, 4);
        assert_eq!(r.target_type, TargetType::NoTarget);
        assert_eq!(r.mana_cost.at(1), 100.0);
        assert_eq!(r.cooldown.at(1), 50.0);
        assert_eq!(r.cooldown.at(4), 20.0);
        let on_cast: Vec<_> = r.effects_for(AbilityTrigger::OnCast).collect();
        assert_eq!(on_cast.len(), 3);
        assert!(
            on_cast
                .iter()
                .all(|x| matches!(x.target, EffectTarget::Caster))
        );
        assert!(matches!(
            on_cast[0].effect,
            AbilityEffect::ApplyStatus { status: StatusSpec::Untargetable, duration }
                if duration.at(1) == 4.0 && duration.at(4) == 4.75
        ));
        assert!(matches!(
            on_cast[1].effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::MoveSpeedBonus { id: "shadow_dance_move_speed_bonus", percent },
                ..
            } if percent.at(1) == 20.0 && percent.at(4) == 50.0
        ));
        assert!(matches!(
            on_cast[2].effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::ResourceModifier {
                    id: "shadow_dance_health_regen",
                    stat: ResourceStat::HealthRegen,
                    amount,
                    independent: false,
                    ..
                },
                ..
            } if amount.at(1) == 70.0 && amount.at(4) == 160.0
        ));
    }

    #[test]
    fn mechanics_and_effects_are_separate() {
        let bolt = builtin(AbilityId::Bolt);
        assert!(bolt.mechanics.contains(&PROJECTILE));
        assert_eq!(bolt.effects_for(AbilityTrigger::OnCast).count(), 0);
        assert_eq!(bolt.effects_for(AbilityTrigger::OnProjectileHit).count(), 3);
        assert_eq!(bolt.effects_for(AbilityTrigger::OnImpact).count(), 1);
    }

    #[test]
    fn winter_wyvern_abilities_generate_expected_definitions() {
        use crate::abilities::definition::{AreaRadius, StatusSpec};
        use crate::actions::TargetType;

        assert_eq!(AbilityId::ArcticBurn.display_name(), "Arctic Burn");
        assert_eq!(AbilityId::SplinterBlast.display_name(), "Splinter Blast");
        assert_eq!(AbilityId::ColdEmbrace.display_name(), "Cold Embrace");
        assert_eq!(AbilityId::WintersCurse.display_name(), "Winter's Curse");
        assert!(AbilityId::WintersCurse.is_ultimate());
        assert!(!AbilityId::ArcticBurn.is_ultimate());

        // Q: a no-target self buff; its attack-hit rows are gated on the range status.
        let q = builtin(AbilityId::ArcticBurn);
        assert_eq!(q.max_rank, 7);
        assert_eq!(q.target_type, TargetType::NoTarget);
        assert_eq!(q.mana_cost.at(1), 100.0);
        assert_eq!(q.cooldown.at(1), 29.0);
        assert_eq!(q.cooldown.at(7), 15.5);
        assert!(q.effects_for(AbilityTrigger::OnAttackHit).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::Caster,
                AbilityEffect::RequireStatus {
                    id: "arctic_burn_attack_range",
                    then: AbilityTrigger::Custom("on_arctic_burn_hit"),
                }
            )
        )));
        let hit = AbilityTrigger::Custom("on_arctic_burn_hit");
        assert!(q.effects_for(hit).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Burn { amount, damage_type: DamageType::Magical, .. },
                duration,
            } if amount.at(1) == 15.0 && amount.at(5) == 67.5 && amount.at(7) == 82.5
                && duration.at(1) == 5.0
        )));
        assert!(q.effects_for(hit).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Slow { id: "arctic_burn_slow", percent },
                duration,
            } if percent.at(1) == 15.0 && percent.at(7) == 37.5 && duration.at(1) == 5.0
        )));
        assert!(q.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::AttackPoint { id: "arctic_burn_attack_point", seconds },
                ..
            } if seconds.at(1) == 0.1
        )));
        assert!(q.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            e.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Modifier { attack_projectile_speed, .. },
                ..
            } if attack_projectile_speed.at(1) == 500.0
        )));

        // W: a targeted projectile (speed 650) that splashes 500 around its target.
        let w = builtin(AbilityId::SplinterBlast);
        assert_eq!(w.max_rank, 7);
        assert_eq!(w.target_type, TargetType::Unit);
        assert_eq!(w.aoe_radius.at(1), 500.0);
        assert_eq!(w.mana_cost.at(7), 150.0);
        assert!(w.mechanics.iter().any(|m| matches!(
            m,
            AbilityMechanic::Projectile {
                splash_radius: AreaRadius::Ability,
                speed: Some(650.0),
                skillshot: false,
                ..
            }
        )));
        assert!(
            w.effects_for(AbilityTrigger::OnProjectileHit)
                .any(|e| matches!(
                    (e.target, e.effect),
                    (
                        EffectTarget::AffectedUnits,
                        AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
                    ) if amount.at(1) == 80.0 && amount.at(7) == 440.0
                ))
        );

        // E: freeze plus a heal over time with a flat and a max-health part.
        let e = builtin(AbilityId::ColdEmbrace);
        assert_eq!(e.max_rank, 7);
        assert_eq!(e.target_type, TargetType::Unit);
        assert_eq!(e.cast_range.at(5), 1025.0);
        assert_eq!(e.mana_cost.at(7), 95.0);
        assert!(e.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::CastTarget,
                AbilityEffect::ApplyStatus { status: StatusSpec::Frozen, duration }
            ) if duration.at(1) == 4.0
        )));
        assert!(e.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            x.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::HealOverTime { hps, max_health_pct },
                ..
            } if hps.at(5) == 57.5 && max_health_pct.at(5) == 5.0
        )));

        // R: four ranks; dispel and stun the target, taunt the area onto it.
        let r = builtin(AbilityId::WintersCurse);
        assert_eq!(r.max_rank, 4);
        assert_eq!(r.target_type, TargetType::Unit);
        assert_eq!(r.aoe_radius.at(1), 525.0);
        assert_eq!(r.cooldown.at(4), 75.0);
        assert_eq!(r.mana_cost.at(4), 275.0);
        assert!(r.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::CastTarget,
                AbilityEffect::Dispel {
                    kind: crate::status::StatusKind::Buff,
                    ..
                }
            )
        )));
        assert!(r.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            x.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::OwnedStun {
                    id: "winters_curse_stun"
                },
                ..
            }
        )));
        assert!(r.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            x.effect,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::TauntCastTarget { attack_speed },
                ..
            } if attack_speed.at(1) == 50.0 && attack_speed.at(4) == 87.5
        )));
    }

    #[test]
    fn shadowheart_abilities_generate_expected_definitions() {
        use crate::abilities::definition::{AreaRadius, StatusSpec};
        use crate::actions::TargetType;

        assert_eq!(AbilityId::Purification.display_name(), "Purification");
        assert_eq!(AbilityId::WardingBond.display_name(), "Warding Bond");
        assert_eq!(
            AbilityId::SpiritGuardians.display_name(),
            "Spirit Guardians"
        );
        assert_eq!(
            AbilityId::SharranConversion.display_name(),
            "Sharran Conversion"
        );
        assert!(AbilityId::SharranConversion.is_ultimate());
        assert!(!AbilityId::Purification.is_ultimate());

        // Q: a heal on the ally target plus pure damage to the enemies around it.
        let q = builtin(AbilityId::Purification);
        assert_eq!(q.max_rank, 7);
        assert_eq!(q.target_type, TargetType::Unit);
        assert_eq!(q.target_team, TargetTeam::Ally);
        assert_eq!(q.cast_range.at(1), 700.0);
        assert_eq!(q.aoe_radius.at(1), 260.0);
        assert_eq!(q.cooldown.at(7), 10.5);
        assert_eq!(q.mana_cost.at(5), 142.5);
        assert!(q.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::CastTarget,
                AbilityEffect::Heal { amount, .. }
            ) if amount.at(1) == 90.0 && amount.at(5) == 335.0
        )));
        assert!(q.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::UnitsInRadius {
                    center: AreaCenter::Aim,
                    radius: AreaRadius::Ability,
                    team: TargetTeam::Enemy,
                },
                AbilityEffect::Damage { amount, damage_type: DamageType::Pure, .. }
            ) if amount.at(5) == 335.0
        )));

        // W: armor and percent magic resistance on both units, plus the bond; 10 s at every rank.
        let w = builtin(AbilityId::WardingBond);
        assert_eq!(w.max_rank, 7);
        assert_eq!(w.target_type, TargetType::Unit);
        assert_eq!(w.target_team, TargetTeam::Ally);
        assert_eq!(w.backswing, 1.33);
        let armor = |target: EffectTarget| {
            w.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
                (e.target, e.effect),
                (
                    t,
                    AbilityEffect::ApplyStatus {
                        status: StatusSpec::Modifier { id: "warding_bond_armor", armor, .. },
                        duration,
                    }
                ) if t == target && armor.at(7) == 5.5 && duration.at(1) == 10.0 && duration.at(7) == 10.0
            ))
        };
        assert!(armor(EffectTarget::Caster) && armor(EffectTarget::CastTarget));
        let resist = |target: EffectTarget| {
            w.effects_for(AbilityTrigger::OnCast).any(|e| {
                matches!(
                    (e.target, e.effect),
                    (
                        t,
                        AbilityEffect::ApplyStatus {
                            status: StatusSpec::MagicResistance {
                                id: "warding_bond_magic_resistance",
                                percent,
                            },
                            duration,
                        }
                    ) if t == target && percent.at(4) == 20.0 && duration.at(3) == 10.0
                )
            })
        };
        assert!(resist(EffectTarget::Caster) && resist(EffectTarget::CastTarget));
        assert!(w.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::CastTarget,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::DamageShare { percent },
                    duration,
                }
            ) if percent.at(1) == 100.0 && duration.at(7) == 10.0
        )));

        // E: three spirits (hit 80) circling at 180, one revolution per 1.41 s, pure damage
        // to whatever each touches.
        let e = builtin(AbilityId::SpiritGuardians);
        assert_eq!(e.max_rank, 7);
        assert_eq!(e.target_type, TargetType::NoTarget);
        let touch = AbilityTrigger::Custom("on_spirit_guardians_touch");
        assert!(e.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::Caster,
                AbilityEffect::Orbit {
                    count: 3,
                    radius: AreaRadius::Fixed(radius),
                    hit_radius,
                    period,
                    duration,
                    then,
                    ..
                }
            ) if radius.at(1) == 180.0 && hit_radius == 80.0 && period.at(1) == 1.41
                && duration.at(1) == 3.0 && duration.at(7) == 7.5 && then == touch
        )));
        assert!(e.effects_for(touch).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::TriggerUnit,
                AbilityEffect::Damage { amount, damage_type: DamageType::Pure, .. }
            ) if amount.at(1) == 90.0 && amount.at(7) == 405.0
        )));

        // R: four ranks; a timed conversion aura of radius 300.
        let r = builtin(AbilityId::SharranConversion);
        assert_eq!(r.max_rank, 4);
        assert_eq!(r.target_type, TargetType::NoTarget);
        assert_eq!(r.cooldown.at(4), 75.0);
        assert_eq!(r.mana_cost.at(4), 250.0);
        assert!(r.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::Caster,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::ConvertOnDeath { radius },
                    duration,
                }
            ) if radius.at(1) == 300.0 && duration.at(1) == 5.0 && duration.at(4) == 10.0
        )));
    }

    #[test]
    fn gyrocopter_abilities_generate_expected_definitions() {
        use crate::abilities::definition::{AreaRadius, StatusSpec};
        use crate::actions::TargetType;
        use crate::components::ProjectileSeek;

        assert_eq!(AbilityId::RocketBarrage.display_name(), "Rocket Barrage");
        assert_eq!(AbilityId::HomingMissile.display_name(), "Homing Missile");
        assert_eq!(AbilityId::FlakCannon.display_name(), "Flak Cannon");
        assert_eq!(AbilityId::CallDown.display_name(), "Call Down");
        assert!(AbilityId::CallDown.is_ultimate());
        assert!(!AbilityId::FlakCannon.is_ultimate());

        // Q: ten rockets a second for three seconds around Gyrocopter; each tick strikes one
        // random enemy within 400.
        let q = builtin(AbilityId::RocketBarrage);
        assert_eq!(q.max_rank, 7);
        assert_eq!(q.target_type, TargetType::NoTarget);
        assert_eq!(q.target_team, TargetTeam::Enemy);
        assert_eq!(q.aoe_radius.at(1), 400.0);
        assert_eq!(q.cast_time, 0.0);
        assert_eq!(q.cooldown.at(5), 5.25);
        assert_eq!(q.mana_cost.at(1), 75.0);
        let tick = AbilityTrigger::Custom("on_rocket_barrage_tick");
        let hit = AbilityTrigger::Custom("on_rocket_barrage_hit");
        assert!(q.effects_for(AbilityTrigger::OnCast).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::Caster,
                AbilityEffect::Zone {
                    radius: AreaRadius::Ability,
                    team: TargetTeam::Enemy,
                    duration,
                    interval,
                    then,
                    follow_caster: true,
                    ..
                }
            ) if interval.at(1) == 0.1 && duration.at(1) == 3.0 && then == tick
        )));
        assert!(q.effects_for(tick).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::AffectedUnits,
                AbilityEffect::PickRandom { count: 1, then }
            ) if then == hit
        )));
        assert!(q.effects_for(hit).any(|e| matches!(
            (e.target, e.effect),
            (
                EffectTarget::TriggerUnit,
                AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
            ) if amount.at(1) == 8.0 && amount.at(7) == 35.0
        )));

        // W: a unit-target seeking missile, damage and a Gyrocopter-owned stun on reach.
        let w = builtin(AbilityId::HomingMissile);
        assert_eq!(w.max_rank, 7);
        assert_eq!(w.target_type, TargetType::Unit);
        assert_eq!(w.cast_range.at(1), 1050.0);
        assert_eq!(w.cast_time, 0.0);
        assert_eq!(w.mana_cost.at(7), 135.0);
        assert_eq!(w.cooldown.at(5), 22.5);
        assert!(w.mechanics.iter().any(|m| matches!(
            m,
            AbilityMechanic::Projectile {
                skillshot: false,
                speed: Some(speed),
                seek: Some(ProjectileSeek { delay, acceleration, retarget_radius }),
                ..
            } if *speed == 500.0 && *delay == 0.5 && (acceleration - 14.285714).abs() < 1e-5
                && *retarget_radius == 700.0
        )));
        assert!(
            w.effects_for(AbilityTrigger::OnProjectileHit)
                .any(|e| matches!(
                    (e.target, e.effect),
                    (
                        EffectTarget::TriggerUnit,
                        AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
                    ) if amount.at(1) == 90.0 && amount.at(7) == 495.0
                ))
        );
        assert!(
            w.effects_for(AbilityTrigger::OnProjectileHit)
                .any(|e| matches!(
                    (e.target, e.effect),
                    (
                        EffectTarget::TriggerUnit,
                        AbilityEffect::ApplyStatus {
                            status: StatusSpec::OwnedStun { id: "homing_missile_stun" },
                            duration,
                        }
                    ) if duration.at(1) == 1.25 && duration.at(7) == 2.375
                ))
        );

        // E: a charges-only buff, gated on by each attack, that splits it to extra enemies
        // around Gyrocopter.
        let e = builtin(AbilityId::FlakCannon);
        assert_eq!(e.max_rank, 7);
        assert_eq!(e.target_type, TargetType::NoTarget);
        assert_eq!(e.cast_time, 0.0);
        assert_eq!(e.cooldown.at(1), 25.0);
        assert_eq!(e.mana_cost.at(1), 50.0);
        assert!(e.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::Caster,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::AttackCharges { id: "flak_cannon_attack_charges", charges },
                    duration,
                }
            ) if charges.at(4) == 7.0 && charges.at(7) == 9.0 && duration.at(1) == 10.0
                && duration.at(7) == 10.0
        )));
        let split = AbilityTrigger::Custom("on_flak_cannon_attack");
        assert!(e.effects_for(AbilityTrigger::OnAttack).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::Caster,
                AbilityEffect::RequireStatus { id: "flak_cannon_attack_charges", then }
            ) if then == split
        )));
        assert!(e.effects_for(split).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::UnitsInRadius {
                    center: AreaCenter::Caster,
                    radius: AreaRadius::Fixed(radius),
                    team: TargetTeam::Enemy,
                },
                AbilityEffect::PerformAttack { damage_ratio, max_targets, exclude_trigger_unit: true }
            ) if radius.at(1) == 1250.0 && max_targets.at(7) == 9.0 && max_targets.at(1) == 4.0
                && damage_ratio.at(1) == 1.0
        )));

        // R: four ranks; a ground zone with one impact tick after 2 s that damages and slows.
        let r = builtin(AbilityId::CallDown);
        assert_eq!(r.max_rank, 4);
        assert_eq!(r.target_type, TargetType::Area);
        assert_eq!(r.cast_range.at(1), 1000.0);
        assert_eq!(r.aoe_radius.at(1), 400.0);
        assert_eq!(r.cast_time, 0.3);
        assert_eq!(r.backswing, 0.5);
        assert_eq!(r.cooldown.at(4), 50.0);
        assert_eq!(r.mana_cost.at(4), 275.0);
        let impact = AbilityTrigger::Custom("on_call_down_impact");
        assert!(r.effects_for(AbilityTrigger::OnCast).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::Caster,
                AbilityEffect::Zone {
                    radius: AreaRadius::Ability,
                    delay,
                    duration,
                    interval,
                    then,
                    follow_caster: false,
                    ..
                }
            ) if delay.at(1) == 2.0 && duration.at(1) == 1.0 && interval.at(1) == 1.0 && then == impact
        )));
        assert!(r.effects_for(impact).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::AffectedUnits,
                AbilityEffect::Damage { amount, damage_type: DamageType::Magical, .. }
            ) if amount.at(1) == 200.0 && amount.at(4) == 575.0
        )));
        assert!(r.effects_for(impact).any(|x| matches!(
            (x.target, x.effect),
            (
                EffectTarget::AffectedUnits,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::Slow { id: "call_down_slow", percent },
                    duration,
                }
            ) if percent.at(1) == 60.0 && duration.at(1) == 4.0
        )));
    }

    #[test]
    fn loadouts_register_their_definitions() {
        let mut world = World::new();
        world.init_resource::<AbilityDefinitions>();
        world.spawn(AbilityLoadout::starter());
        world.run_system_once(register_loadout_definitions).unwrap();
        let defs = world.resource::<AbilityDefinitions>();
        assert!(defs.get(AbilityId::Bolt).is_some());
        assert!(defs.get(AbilityId::Meteor).is_none());
    }

    use bevy::ecs::system::RunSystemOnce;
}
