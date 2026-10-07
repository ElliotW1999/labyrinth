//! Static, data-driven ability definitions. Nothing here is mutated at runtime;
//! per-hero state lives in [`crate::components::AbilityState`].

use crate::components::{AbilityId, DamageType};
use crate::items::{StatusEffect, StatusKind};

/// A value that scales with ability level: `clamp(base + per_rank × rank, min, max)`,
/// or an explicit per-level table (`75|150|225|300` in the generator CSVs).
/// `rank` is clamped to at least 1 so unlearned abilities report their level-1 value;
/// tables repeat their last entry past the end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankValue {
    pub base: f32,
    pub per_rank: f32,
    pub min: f32,
    pub max: f32,
    /// Per-level values (index 0 = rank 1). Empty for linear values.
    pub levels: &'static [f32],
    /// Applied after the clamp; see [`RankValue::scaled`].
    pub factor: f32,
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
            levels: &[],
            factor: 1.0,
        }
    }

    #[allow(dead_code)]
    pub const fn levels(levels: &'static [f32]) -> Self {
        Self {
            levels,
            ..Self::ZERO
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

    /// Multiplies every rank's value by `factor`.
    pub const fn scaled(mut self, factor: f32) -> Self {
        self.factor *= factor;
        self
    }

    pub fn at(self, rank: u32) -> f32 {
        let rank = rank.max(1);
        let raw = match self.levels {
            [] => self.base + self.per_rank * rank as f32,
            levels => levels[(rank as usize).min(levels.len()) - 1],
        };
        raw.clamp(self.min, self.max) * self.factor
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

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaCenter {
    /// The caster's position when the trigger fired.
    Caster,
    /// The cast aim (unit position for unit targets, caster position for no-target casts).
    Aim,
    /// Where the trigger happened (the aim for `OnCast`, the impact point for projectiles).
    TriggerPoint,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AreaRadius {
    /// The ability's `aoe_radius` at the cast rank.
    Ability,
    Fixed(RankValue),
}

/// The moment a mechanic reports to the effect layer. Mechanics decide *when* a
/// trigger fires and *who* is involved; effects tagged with the trigger decide *what*
/// happens to them.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AbilityTrigger {
    /// The cast point completed.
    OnCast,
    /// A projectile struck its unit target. Splash units are the affected units.
    OnProjectileHit,
    OnUnitContact,
    /// A projectile ended without striking a unit (ground impact or lost target).
    OnImpact,
    OnChannelTick,
    OnChannelEnd,
    OnExpire,
    /// Item stat bonuses held while the item is owned (applied on purchase, not resolved).
    Passive,
    /// An item was activated from the inventory.
    OnUse,
    /// The owner released a basic attack (trigger unit = the attack target).
    OnAttack,
    /// The owner's basic attack connected (trigger unit = the struck unit).
    OnAttackHit,
    OnDamageTaken,
    OnKill,
    OnDeath,
    /// Emitted by a custom mechanic, e.g. `Custom("on_skewer_end")`.
    Custom(&'static str),
}

/// Hero attribute used by [`EffectScaling::CasterAttribute`] and item stat bonuses.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribute {
    Strength,
    Agility,
    Intelligence,
}

/// Who receives an effect, resolved from the [`AbilityTrigger`]'s context.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectTarget {
    Caster,
    /// The unit the ability was cast on.
    CastTarget,
    /// The unit that caused the trigger (projectile victim, contacted unit, …).
    TriggerUnit,
    /// Every unit the mechanic reported as involved (splash victims, dragged units, …).
    AffectedUnits,
    /// Living units matching `team` within `radius` of `center` when the trigger fires.
    UnitsInRadius {
        center: AreaCenter,
        radius: AreaRadius,
        team: TargetTeam,
    },
}

impl EffectTarget {
    pub const fn enemies_around(center: AreaCenter) -> Self {
        Self::UnitsInRadius {
            center,
            radius: AreaRadius::Ability,
            team: TargetTeam::Enemy,
        }
    }
}

/// Per-target adjustment of an effect's magnitude.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum EffectScaling {
    #[default]
    None,
    /// Adds `ratio × caster attack damage`.
    CasterAttackDamage(f32),
    /// Adds `ratio × caster attribute` (heroes only).
    CasterAttribute(Attribute, f32),
    /// The effect only applies to targets whose health fraction is below the threshold.
    TargetHealthBelow(f32),
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
    /// Reduces the target's current move speed by `percent`.
    Slow { percent: RankValue },
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

    /// `target_move_speed` is only read by percentage modifiers such as [`StatusSpec::Slow`].
    pub fn build(self, rank: u32, duration: f32, target_move_speed: f32) -> StatusEffect {
        match self {
            StatusSpec::Stun => StatusEffect::stunned(duration),
            StatusSpec::Root => StatusEffect::rooted(duration),
            StatusSpec::Silence => StatusEffect::silenced(duration),
            StatusSpec::Disarm => StatusEffect::disarmed(duration),
            StatusSpec::Phased => StatusEffect::phased(duration),
            StatusSpec::Forceful => StatusEffect::forceful(duration),
            StatusSpec::DebuffImmunity => StatusEffect::debuff_immunity(duration),
            StatusSpec::Slow { percent } => StatusEffect {
                move_speed: -target_move_speed * (percent.at(rank) / 100.0).clamp(0.0, 1.0),
                ..StatusEffect::debuff("slow", duration)
            },
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

/// Direction of a forced movement.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplacementKind {
    /// Away from the trigger point (or the caster when they coincide).
    Knockback,
    /// Toward the caster.
    Pull,
}

/// Permanent stat bonus granted by an item's `Passive` entries.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassiveStat {
    MaxHealth,
    HealthRegen,
    MaxMana,
    ManaRegen,
    AttackDamage,
    AttackSpeed,
    Armor,
    MagicResist,
    MoveSpeed,
    Attribute(Attribute),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingStyle {
    Shockwave,
    Nova,
}

/// What happens to each resolved target. Effects never touch `Health` or stats
/// directly: they emit `DamageEvent` / `HealEvent` / `StatusEffectEvent`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityEffect {
    Damage {
        amount: RankValue,
        damage_type: DamageType,
        scaling: EffectScaling,
    },
    Heal {
        amount: RankValue,
        scaling: EffectScaling,
    },
    RestoreMana {
        amount: RankValue,
    },
    ApplyStatus {
        status: StatusSpec,
        duration: RankValue,
    },
    /// Remove every status of `kind` (reverting their stat modifiers).
    Dispel {
        kind: StatusKind,
    },
    /// Remove statuses with this id (e.g. `"stun"`, `"slow"`).
    RemoveStatus {
        id: &'static str,
    },
    /// Forced movement over `duration`. `distance: None` pulls all the way to the caster.
    Displace {
        kind: DisplacementKind,
        distance: Option<RankValue>,
        duration: RankValue,
    },
    /// Permanent bonus while an item is owned; only meaningful under `AbilityTrigger::Passive`.
    PassiveStat {
        stat: PassiveStat,
        amount: RankValue,
    },
    /// Escape hatch: runs the system registered under `id` with
    /// `effects::AbilityEffectAppExt::register_custom_effect`. It still receives
    /// resolved targets and should emit the shared gameplay events.
    Custom {
        id: &'static str,
        value: RankValue,
        amount: RankValue,
        duration: RankValue,
    },
}

/// One row of an ability's effect table: when `trigger` fires, apply `effect` to `target`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityEffectEntry {
    pub trigger: AbilityTrigger,
    pub target: EffectTarget,
    pub effect: AbilityEffect,
}

/// How an ability plays out in the world: movement, projectiles, and presentation.
/// Mechanics report what they encounter as [`AbilityTrigger`]s instead of applying
/// consequences themselves.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityMechanic {
    /// Phased dash toward the aim, capped at `distance`.
    Dash { distance: RankValue },
    /// Spell projectile toward the unit target (homing) or aim point. A unit strike
    /// fires `OnProjectileHit`; ending without one fires `OnImpact`. Enemies within
    /// `splash_radius` of the impact become the trigger's affected units.
    ///
    /// A `skillshot` instead flies the full cast range toward the aim, firing
    /// `OnProjectileHit` for every enemy it passes within `aoe_radius`, then `OnImpact`.
    Projectile {
        splash_radius: AreaRadius,
        /// World units per second; `None` uses the default spell projectile speed.
        speed: Option<f32>,
        skillshot: bool,
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
    /// Seconds the caster channels after the cast point (`0` = not channelled).
    pub channel_time: f32,
    /// Charge-based abilities: maximum stored charges and seconds to restore one.
    pub charges: Option<(u32, f32)>,
    pub description: &'static str,
    /// Run on every successful cast. Abilities with unique mechanics can additionally
    /// register a custom behavior (see `mechanics::AbilityAppExt`).
    pub mechanics: Vec<AbilityMechanic>,
    /// Consequences, keyed by the trigger that applies them.
    pub effects: Vec<AbilityEffectEntry>,
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
            channel_time: 0.0,
            charges: None,
            description: "",
            mechanics: Vec::new(),
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

    pub fn team(mut self, target_team: TargetTeam) -> Self {
        self.target_team = target_team;
        self
    }

    #[allow(dead_code)]
    pub fn channel(mut self, channel_time: f32) -> Self {
        self.channel_time = channel_time.max(0.0);
        self
    }

    #[allow(dead_code)]
    pub fn charges(mut self, max_charges: u32, restore_time: f32) -> Self {
        self.charges = Some((max_charges, restore_time));
        self
    }

    pub fn description(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    pub fn mechanic(mut self, mechanic: AbilityMechanic) -> Self {
        self.mechanics.push(mechanic);
        self
    }

    /// Apply `effect` to `target` whenever `trigger` fires.
    pub fn on(mut self, trigger: AbilityTrigger, target: EffectTarget, effect: AbilityEffect) -> Self {
        self.effects.push(AbilityEffectEntry {
            trigger,
            target,
            effect,
        });
        self
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn effects_for(&self, trigger: AbilityTrigger) -> impl Iterator<Item = &AbilityEffectEntry> {
        self.effects.iter().filter(move |entry| entry.trigger == trigger)
    }

    pub fn is_castable(&self) -> bool {
        self.behavior != AbilityBehavior::Passive
    }
}
