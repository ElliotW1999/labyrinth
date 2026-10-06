//! Static, data-driven ability definitions. Nothing here is mutated at runtime;
//! per-hero state lives in [`crate::components::AbilityState`].

use crate::components::{AbilityId, AbilityType, DamageType};
use crate::items::{StatusEffect, StatusKind};

/// A value that scales with ability level: `clamp(base + per_rank × rank, min, max)`.
/// `rank` is clamped to at least 1 so unlearned abilities report their level-1 value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankValue {
    pub base: f32,
    pub per_rank: f32,
    pub min: f32,
    pub max: f32,
}

impl RankValue {
    pub const ZERO: Self = Self::fixed(0.0);

    pub const fn fixed(value: f32) -> Self {
        Self::linear(value, 0.0)
    }

    pub const fn linear(base: f32, per_rank: f32) -> Self {
        Self {
            base,
            per_rank,
            min: f32::NEG_INFINITY,
            max: f32::INFINITY,
        }
    }

    /// `level_1 + per_rank × (rank − 1)`, the convention used by AbilityGenerator.
    pub const fn from_level_1(level_1: f32, per_rank: f32) -> Self {
        Self::linear(level_1 - per_rank, per_rank)
    }

    pub const fn at_least(mut self, min: f32) -> Self {
        self.min = min;
        self
    }

    pub fn at(self, rank: u32) -> f32 {
        (self.base + self.per_rank * rank.max(1) as f32).clamp(self.min, self.max)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityBehavior {
    Active,
    Passive,
    /// Each cast flips [`crate::components::AbilityState::toggled`].
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType {
    NoTarget,
    /// Must be cast on a unit matching `target_team`.
    Unit,
    /// Ground point / skillshot direction. A hovered unit is accepted as the aim.
    Point,
    /// Ground area centred on the aim. A hovered unit is accepted as the aim.
    Area,
}

/// Which units an ability (or one of its effects) may affect, relative to the caster.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetTeam {
    Enemy,
    /// Same team, including the caster.
    Ally,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaCenter {
    Caster,
    /// The cast aim (unit position for unit targets, impact point for projectile hits).
    Aim,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AreaRadius {
    /// The ability's `aoe_radius` at the cast rank.
    Ability,
    Fixed(RankValue),
}

/// Who receives an effect.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectTargets {
    Caster,
    /// The cast's unit target (or the unit struck by a projectile).
    UnitTarget,
    Area {
        center: AreaCenter,
        radius: AreaRadius,
        team: TargetTeam,
    },
}

impl EffectTargets {
    pub const fn enemies_around(center: AreaCenter) -> Self {
        Self::Area {
            center,
            radius: AreaRadius::Ability,
            team: TargetTeam::Enemy,
        }
    }
}

/// Status templates; durations come from the owning effect.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatusSpec {
    Stun,
    Root,
    Silence,
    Disarm,
    Phased,
    Forceful,
    /// Purges existing debuffs on application.
    DebuffImmunity,
    Modifier {
        id: &'static str,
        kind: StatusKind,
        armor: RankValue,
        magic_resist: RankValue,
        move_speed: RankValue,
        attack_damage: RankValue,
    },
}

impl StatusSpec {
    pub const fn armor_buff(id: &'static str, armor: RankValue) -> Self {
        Self::Modifier {
            id,
            kind: StatusKind::Buff,
            armor,
            magic_resist: RankValue::ZERO,
            move_speed: RankValue::ZERO,
            attack_damage: RankValue::ZERO,
        }
    }

    pub fn build(self, rank: u32, duration: f32) -> StatusEffect {
        match self {
            StatusSpec::Stun => StatusEffect::stunned(duration),
            StatusSpec::Root => StatusEffect::rooted(duration),
            StatusSpec::Silence => StatusEffect::silenced(duration),
            StatusSpec::Disarm => StatusEffect::disarmed(duration),
            StatusSpec::Phased => StatusEffect::phased(duration),
            StatusSpec::Forceful => StatusEffect::forceful(duration),
            StatusSpec::DebuffImmunity => StatusEffect::debuff_immunity(duration),
            StatusSpec::Modifier {
                id,
                kind,
                armor,
                magic_resist,
                move_speed,
                attack_damage,
            } => {
                let base = match kind {
                    StatusKind::Buff => StatusEffect::buff(id, duration),
                    StatusKind::Debuff => StatusEffect::debuff(id, duration),
                };
                StatusEffect {
                    armor: armor.at(rank),
                    magic_resist: magic_resist.at(rank),
                    move_speed: move_speed.at(rank),
                    attack_damage: attack_damage.at(rank),
                    ..base
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingStyle {
    Shockwave,
    Nova,
}

/// Reusable building blocks. Effects only emit gameplay events / commands; the
/// shared damage, status, heal, movement, and projectile systems resolve them.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum AbilityEffect {
    Damage {
        amount: RankValue,
        damage_type: DamageType,
        targets: EffectTargets,
    },
    Heal {
        amount: RankValue,
        targets: EffectTargets,
    },
    ApplyStatus {
        status: StatusSpec,
        duration: RankValue,
        targets: EffectTargets,
    },
    /// Remove every status of `kind` (reverting their stat modifiers).
    Dispel {
        kind: StatusKind,
        targets: EffectTargets,
    },
    /// Phased dash toward the aim, capped at `distance`.
    Dash { distance: RankValue },
    /// Spell projectile toward the unit target (homing) or aim point. `on_hit`
    /// effects resolve against the struck unit, with the impact point as the aim.
    SpawnProjectile {
        damage: RankValue,
        damage_type: DamageType,
        splash_radius: AreaRadius,
        on_hit: Vec<AbilityEffect>,
    },
    /// Caster follows up with auto-attacks on the unit target.
    AttackUnitTarget,
    RingFx {
        center: AreaCenter,
        radius: AreaRadius,
        style: RingStyle,
        lifetime: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AbilityDefinition {
    pub id: AbilityId,
    pub name: &'static str,
    pub behavior: AbilityBehavior,
    pub target_type: TargetType,
    pub target_team: TargetTeam,
    pub max_rank: u32,
    pub is_ultimate: bool,
    pub cast_range: RankValue,
    /// Area radius (area targets) or projectile width (point targets); also drives indicators.
    pub aoe_radius: RankValue,
    /// Cast point: delay before the ability fires (0..=0.5).
    pub cast_time: f32,
    /// Cancellable recovery after the ability fires (0..=0.5).
    pub backswing: f32,
    pub cooldown: RankValue,
    pub mana_cost: RankValue,
    /// Common effects run on every successful cast. Abilities with unique logic can
    /// additionally register a custom behavior (see `effects::AbilityAppExt`).
    pub effects: Vec<AbilityEffect>,
}

impl AbilityDefinition {
    /// Active, instant (no-target) ability with no effects; builders fill in the rest.
    pub fn new(id: AbilityId) -> Self {
        Self {
            id,
            name: id.display_name(),
            behavior: AbilityBehavior::Active,
            target_type: TargetType::NoTarget,
            target_team: TargetTeam::Enemy,
            max_rank: id.max_rank(),
            is_ultimate: id.is_ultimate(),
            cast_range: RankValue::ZERO,
            aoe_radius: RankValue::ZERO,
            cast_time: 0.2,
            backswing: 0.25,
            cooldown: RankValue::ZERO,
            mana_cost: RankValue::ZERO,
            effects: Vec::new(),
        }
    }

    pub fn targeting(mut self, target_type: TargetType, cast_range: RankValue, aoe_radius: RankValue) -> Self {
        self.target_type = target_type;
        self.cast_range = cast_range;
        self.aoe_radius = aoe_radius;
        self
    }

    pub fn costs(mut self, cooldown: RankValue, mana_cost: RankValue) -> Self {
        self.cooldown = cooldown;
        self.mana_cost = mana_cost;
        self
    }

    pub fn timing(mut self, cast_time: f32, backswing: f32) -> Self {
        self.cast_time = cast_time.clamp(0.0, 0.5);
        self.backswing = backswing.clamp(0.0, 0.5);
        self
    }

    pub fn behavior(mut self, behavior: AbilityBehavior) -> Self {
        self.behavior = behavior;
        self
    }

    pub fn effect(mut self, effect: AbilityEffect) -> Self {
        self.effects.push(effect);
        self
    }

    pub fn is_castable(&self) -> bool {
        self.behavior != AbilityBehavior::Passive
    }
}

/// Split AbilityGenerator's combined activation category into behavior + targeting.
pub fn classify(ability_type: AbilityType) -> (AbilityBehavior, TargetType) {
    match ability_type {
        AbilityType::Passive => (AbilityBehavior::Passive, TargetType::NoTarget),
        AbilityType::Toggle => (AbilityBehavior::Toggle, TargetType::NoTarget),
        AbilityType::Untargeted => (AbilityBehavior::Active, TargetType::NoTarget),
        AbilityType::UnitTarget => (AbilityBehavior::Active, TargetType::Unit),
        AbilityType::TargetArea => (AbilityBehavior::Active, TargetType::Area),
        AbilityType::TargetPoint => (AbilityBehavior::Active, TargetType::Point),
    }
}
