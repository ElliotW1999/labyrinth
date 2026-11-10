//! Static, data-driven ability definitions. Nothing here is mutated at runtime;
//! per-hero state lives in [`crate::components::AbilityState`].

pub use crate::actions::{TargetTeam, TargetType};
use bevy::prelude::{Entity, Vec2, Vec3};

use crate::components::AbilityId;
use crate::components::DamageType;
use crate::status::{DispelType, Leash, LeashAnchor, StatusEffect, StatusKind};
use crate::vitals::{ManaShield, ResourceKind, ResourceStat};

/// Upper bound (seconds) for an ability's cast point (the aim before it fires).
pub const MAX_CAST_POINT: f32 = 2.0;
/// Upper bound (seconds) for an ability's cancellable backswing.
pub const MAX_CAST_BACKSWING: f32 = 2.0;

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
    /// A self-propelled mover (dash, charge) touched an enemy; trigger unit = that enemy.
    OnUnitContact,
    /// A self-propelled forced movement ended (arrival, block, timeout, or the mover was
    /// disabled). Trigger unit = the chased unit if it was reached; affected units =
    /// every unit contacted on the way. Replacement, teleport and death end silently.
    OnMovementEnd,
    /// A projectile ended without striking a unit (ground impact or lost target).
    OnImpact,
    OnChannelTick,
    /// A channel ran its full time. A channel cut short by a disable fires
    /// [`Self::OnChannelInterrupted`] instead; a new order cancels silently.
    OnChannelEnd,
    /// A channel ended early because the caster was disabled.
    OnChannelInterrupted,
    OnExpire,
    /// Held while a toggle ability is on. Only `ApplyStatus` entries are read: the
    /// upkeep system applies them as permanent statuses when the toggle turns on and
    /// removes them when it turns off.
    WhileToggled,
    /// Held while the condition is true: only `ApplyStatus` entries are read. The statuses
    /// are permanent, undispellable and reactive (they fire no status triggers) and are
    /// removed exactly when the condition turns false, the source is lost or the owner dies.
    While(Condition),
    /// Item stat bonuses held while the item is owned (applied on purchase, not resolved).
    Passive,
    /// An item was activated from the inventory.
    OnUse,
    /// The owner released a basic attack (trigger unit = the attack target).
    OnAttack,
    /// The owner's basic attack connected (trigger unit = the struck unit).
    OnAttackHit,
    /// The owner's non-reactive, non-DoT damage reduced a unit's health or shield (trigger
    /// unit = the victim). Resolves one frame after the damage.
    OnDamageDealt,
    /// The owner took non-reactive, non-DoT damage (trigger unit = the attacker, if any).
    OnDamageTaken,
    /// The owner's damage killed a unit (trigger unit = the victim, which may already be
    /// despawned: target the point or a radius).
    OnKill,
    /// The owner died (trigger unit = the killer, if any).
    OnDeath,
    /// One hit took the owner from at or above `percent` of its max health to below it, and
    /// it survived. Rearms when health is back above the line (the check is stateless).
    OnHealthBelow {
        percent: u8,
    },
    /// A status matching the filter was applied to (or refreshed on) the owner. Trigger unit
    /// = the applier, if it is someone else. Reactive and held statuses never fire it.
    OnStatusApplied(StatusFilter),
    /// A status matching the filter left the owner (expired, dispelled, removed or broken).
    OnStatusRemoved(StatusFilter),
    /// The owner moved (walking, dashing or forced movement; teleports do not count).
    /// `TriggerContext::amount` is the distance covered that frame, so entries under it
    /// are meant to be `Accumulate` gates ("every 1000 units").
    OnMove,
    /// A living, targetable enemy came within `radius` of the owner (units already in range
    /// when the source is first held count as entering). Trigger unit = that enemy.
    OnEnemyEnter {
        radius: u32,
    },
    /// A tracked enemy left the radius or became untargetable. An enemy that died or
    /// despawned is dropped silently.
    OnEnemyLeave {
        radius: u32,
    },
    /// This source's cooldown ran out (never for a zero cooldown). Not emitted for charge
    /// restoration. Fires for abilities and active items; copies of one item share it.
    OnCooldownReady,
    /// Every `millis` of the owner's life while it holds the source (copies of an item share
    /// one timer; it pauses while the owner is dead and fires at most once per frame).
    OnInterval {
        millis: u32,
    },
    /// The owner finished casting an ability. Fans out to every other source the owner
    /// holds (never the cast ability itself); item uses are not spell casts.
    OnSpellCast,
    /// An effect of the owner's cast resolved onto an enemy (one per enemy per cast
    /// context, damage or not). Resolves one frame later.
    OnSpellHit,
    /// Emitted by a custom mechanic, e.g. `Custom("on_skewer_end")`.
    Custom(&'static str),
}

/// What a [`AbilityTrigger::While`] entry waits for. Evaluated once per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Condition {
    /// Health is strictly below `percent` of max.
    HealthBelow {
        percent: u8,
    },
    /// The owner moved last frame (walking, dashing or forced; not teleporting).
    Moving,
    Stationary,
    /// At least `at_least` living, targetable enemies are within `radius`.
    EnemiesNear {
        radius: u32,
        at_least: u8,
    },
    /// At least `at_least` other living, targetable allies are within `radius`.
    AlliesNear {
        radius: u32,
        at_least: u8,
    },
    /// The owner has a status with this runtime id.
    HasStatus(&'static str),
}

/// Which statuses an `OnStatusApplied` / `OnStatusRemoved` entry reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusFilter {
    Any,
    Buff,
    Debuff,
    /// One status by its runtime id (e.g. `"stun"`).
    Id(&'static str),
}

impl StatusFilter {
    pub fn matches(self, id: &str, kind: crate::status::StatusKind) -> bool {
        use crate::status::StatusKind;
        match self {
            Self::Any => true,
            Self::Buff => kind == StatusKind::Buff,
            Self::Debuff => kind == StatusKind::Debuff,
            Self::Id(wanted) => wanted == id,
        }
    }
}

impl AbilityTrigger {
    /// Fired by a basic attack (release or landed hit); effects under it run with an
    /// [`super::effects::AttackMeta`] on the context.
    pub fn is_attack(self) -> bool {
        matches!(self, Self::OnAttack | Self::OnAttackHit)
    }

    /// Fired straight from a cast: the context's effects count as spell hits (see
    /// [`AbilityTrigger::OnSpellHit`]) when they resolve onto an enemy at depth 0.
    pub fn is_cast_phase(self) -> bool {
        matches!(
            self,
            Self::OnCast
                | Self::OnProjectileHit
                | Self::OnImpact
                | Self::OnUnitContact
                | Self::OnMovementEnd
                | Self::OnChannelTick
                | Self::OnChannelEnd
                | Self::OnChannelInterrupted
                | Self::Custom(_)
        )
    }

    /// Caused by damage or a status change. Contexts rooted in such a trigger are
    /// reactions: the damage and statuses they produce are `reactive` and never fire
    /// damage or status triggers again.
    pub fn is_reaction(self) -> bool {
        matches!(
            self,
            Self::OnDamageDealt
                | Self::OnDamageTaken
                | Self::OnHealthBelow { .. }
                | Self::OnStatusApplied(_)
                | Self::OnStatusRemoved(_)
                | Self::OnKill
                | Self::OnDeath
        )
    }
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
    /// Adds `ratio × (max − current)` health to the base amount.
    TargetMissingHealth(f32),
    /// Adds `ratio × max` health to the base amount.
    TargetMaxHealth(f32),
    /// Adds `ratio × current` health to the base amount.
    TargetCurrentHealth(f32),
    /// Multiplies the amount by `stacks of status id on the target + offset`. The hit is
    /// skipped when that is zero.
    TargetStacks { id: &'static str, offset: u32 },
    /// At max cast range, damage is multiplied by `(1 − falloff)`; linear in between.
    DistanceFromCaster { falloff: f32 },
}

/// Status templates; durations come from the owning effect.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatusSpec {
    Stun,
    /// A stun with its own status id (generated `<owner>_stun`). Same-id refresh means it never
    /// replaces, shortens or removes a stun from another source.
    OwnedStun {
        id: &'static str,
    },
    Root,
    Silence,
    Disarm,
    Phased,
    /// Encased in ice: stunned and immune to physical damage. A buff, so the holder's debuff
    /// dispels and debuff immunity leave it alone (see [`StatusEffect::frozen`]).
    Frozen,
    Forceful,
    /// Flies: trees and other ground obstacles stop blocking (buildings still do).
    Flying,
    /// Confines the target to `radius` around an anchor: the trigger point or the caster
    /// (a tether). It clamps and never breaks.
    Leash {
        radius: RankValue,
        anchor: LeashAnchorKind,
    },
    /// Blocks new debuffs and purges existing ones on application. Magic damage and hostile
    /// spells still land.
    DebuffImmunity,
    /// Debuff immunity, plus: hostile unit-target spells cannot pick the holder and magical
    /// damage is ignored (area spells still reach it).
    SpellImmunity,
    /// Basic attacks cannot target the holder.
    AttackImmunity,
    /// Targeting only: attacks and hostile unit-target spells cannot pick the holder, but area
    /// effects, dash contact and in-flight projectiles still reach it. Not cyclone's full
    /// untargetable.
    Untargetable,
    /// Disarmed; immune to physical damage and to being attacked; takes `magic_amp_percent`
    /// (0-90) extra magic damage. Buff or debuff by hostility.
    Ethereal {
        magic_amp_percent: RankValue,
    },
    /// Out of the world: stunned, invulnerable, untargetable, hidden and phased. Not
    /// dispellable. Buff or debuff by hostility.
    Banish,
    /// Absorbing shield pool (`amount` at rank).
    Shield {
        amount: RankValue,
    },
    /// Periodic damage while active. `amount` is damage per second when `tick_interval` is 0
    /// (continuous), otherwise damage per tick, one tick every `tick_interval` seconds. `id` is
    /// the status id: hand-written burns use `"burn"`, generated ones `<owner>_burn` so
    /// burns from different sources coexist.
    Burn {
        id: &'static str,
        amount: RankValue,
        damage_type: DamageType,
        tick_interval: RankValue,
    },
    /// Items only; abilities use silence.
    Mute,
    /// Stops acting until damaged or duration ends.
    Sleep,
    /// No damage from any source.
    Invulnerable,
    /// Composite silence + disarm + mute (items) + break, with walking speed capped at
    /// `scale::HEX_MOVE_SPEED`.
    Hex,
    /// Ability passives (stats and triggers) are suppressed. Item passives are not.
    Break,
    /// Concealed from enemies: no attacks, hostile unit-target spells or auto-acquisition pick
    /// the holder, and fog hides it even inside vision. Area effects still hit it. Ends when
    /// the holder attacks or casts if `breaks_on_action`.
    Invisibility {
        breaks_on_action: bool,
    },
    /// True sight: the holder's invisibility is void while this lasts.
    Reveal,
    /// Negates the next hostile unit-target spell that lands on the holder, then is consumed.
    SpellBlock,
    /// Reflects hostile unit-target spells at their caster; the original still lands unless
    /// blocked. A reflected spell is never reflected again.
    SpellReflect,
    /// Percent chance (0–100) that basic attacks miss.
    Blind {
        miss_chance_percent: RankValue,
    },
    /// `percent` (0-100) chance that attacks against the target miss (a buff).
    Evasion {
        percent: RankValue,
    },
    /// `percent` (0-100) chance that the target's attacks land despite blind or evasion.
    Accuracy {
        percent: RankValue,
    },
    /// The target's attacks never miss: ignores evasion and blind.
    TrueStrike,
    /// Run away from `source` (typically the caster); no attacking, casting, items or orders.
    Fear,
    /// Forced to basic attack `source` (the caster) only; no casting, items or orders.
    Taunt,
    /// Forced to walk to `source` (the caster) and stop at contact; no attacking, casting,
    /// items or orders.
    Hypnosis,
    /// Reduces the target's move speed by `percent` of its base speed. `id` is the status id:
    /// hand-written slows use `"slow"`, generated ones `<owner>_slow`, so slows from different
    /// sources coexist and add while the same source refreshes.
    Slow {
        id: &'static str,
        percent: RankValue,
    },
    /// `percent` of attack speed rating removed (same scale as move slow).
    AttackSpeedSlow {
        percent: RankValue,
    },
    /// `percent` reduction applied to base turn rate.
    TurnRateSlow {
        percent: RankValue,
    },
    /// Raises the holder's move speed by `percent` (0..400) of its base speed, a buff. `id` is
    /// the status id (`<owner>_move_speed_bonus` when generated), so bonuses from different
    /// sources coexist and add (and add to slows) while the same source refreshes.
    MoveSpeedBonus {
        id: &'static str,
        percent: RankValue,
    },
    /// Replaces the holder's basic-attack point with `seconds` (a buff; the lowest live override
    /// applies, backswing is unchanged). `id` is `<owner>_attack_point` when generated.
    AttackPoint {
        id: &'static str,
        seconds: RankValue,
    },
    /// `percent` added to base turn rate (a buff; bonuses add up).
    TurnRateBonus {
        percent: RankValue,
    },
    /// Heals `hps` health per second plus `max_health_pct` percent (0..=100) of the holder's max
    /// health per second; the caster's heal amp is folded into both parts on application.
    HealOverTime {
        hps: RankValue,
        max_health_pct: RankValue,
    },
    /// `percent` (0–100) of healing received is negated; the strongest reduction applies.
    HealReduction {
        percent: RankValue,
    },
    /// Prevents all healing.
    HealBlock,
    /// One stack of a per-target stacking debuff (generated `<owner>_stacks`, undispellable).
    /// Applying it again adds a stack and refreshes the duration; the stacks vanish when it
    /// expires. [`EffectScaling::TargetStacks`] reads the count.
    Stacks {
        id: &'static str,
    },
    /// `percent` (0-100) of incoming damage is removed after mitigation (a buff). `id` is
    /// `<owner>_damage_reduction`; reductions from different ids multiply.
    DamageReduction {
        id: &'static str,
        percent: RankValue,
    },
    /// `percent` (0-100) is cut from the duration of hostile debuffs that land on the holder
    /// while it lasts (a buff). `id` is `<owner>_status_resistance`; resistances multiply.
    StatusResistance {
        id: &'static str,
        percent: RankValue,
    },
    /// Diverts `absorb_percent` (0–100) of post-mitigation damage to mana, at
    /// `damage_per_mana` damage per point of mana.
    ManaShield {
        absorb_percent: RankValue,
        damage_per_mana: RankValue,
    },
    /// A timed resource modifier (regen, max, lifesteal, heal amp, cost) summed when read.
    ResourceModifier {
        id: &'static str,
        kind: StatusKind,
        stat: ResourceStat,
        amount: RankValue,
        /// Independent instances: see [`StatusSpec::Modifier::independent`].
        independent: bool,
    },
    Modifier {
        id: &'static str,
        kind: StatusKind,
        armor: RankValue,
        magic_resist: RankValue,
        move_speed: RankValue,
        attack_damage: RankValue,
        attack_speed_flat: RankValue,
        attack_speed_mult: RankValue,
        /// Flat attack range change (does not change melee / ranged classification).
        attack_range: RankValue,
        /// Flat attack projectile speed change (ranged attackers; units per second).
        attack_projectile_speed: RankValue,
        /// Percent (0..=1000) of the target's base attack damage, snapshotted when the status
        /// is built (see [`StatusBuildContext::base_attack_damage`]) and added to the flat
        /// `attack_damage` bonus.
        attack_damage_pct: RankValue,
        /// Cleave ratio (0..=1) added to `CombatStats::cleave_damage_ratio`; only melee
        /// attackers cleave.
        cleave: RankValue,
        /// Added to `CombatStats::cleave_radius`: the cleave's starting half-width.
        cleave_radius: RankValue,
        /// Added to `CombatStats::cleave_end_radius`: half-width at `cleave_distance`.
        cleave_end_radius: RankValue,
        /// Added to `CombatStats::cleave_distance`: a non-zero value makes the cleave a cone.
        cleave_distance: RankValue,
        /// Whole basic attacks (released, hit or miss) the status lasts for; ZERO = not
        /// charge-limited. It still ends on its duration.
        attack_charges: RankValue,
        /// Every application becomes its own instance with its own timer instead of
        /// refreshing the same-id one (`stat_modifier` effect ids with `:independent`).
        independent: bool,
    },
}

/// Where a [`StatusSpec::Leash`] is anchored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeashAnchorKind {
    /// The trigger point (a region around where the effect landed).
    TriggerPoint,
    /// The caster (a tether that follows it).
    Caster,
}

/// Inputs needed to construct a runtime [`StatusEffect`] from a [`StatusSpec`].
#[derive(Debug, Clone, Copy)]
pub struct StatusBuildContext {
    pub move_speed: f32,
    pub agility: f32,
    /// The unit that caused the status (the caster).
    pub source: Option<Entity>,
    /// Where the trigger happened.
    pub point: Vec3,
    /// The target's base attack damage, excluding item bonuses and other statuses: the hero
    /// definition's damage for heroes, the unit's current damage otherwise. Percentage attack
    /// damage modifiers are a snapshot of this at build time.
    pub base_attack_damage: f32,
}

impl StatusSpec {
    /// Whether the status is a debuff on a hostile target and a buff on an ally or the caster
    /// (the runner sets the kind from the target's relation to the source).
    pub const fn kind_follows_hostility(self) -> bool {
        matches!(self, StatusSpec::Ethereal { .. } | StatusSpec::Banish)
    }

    pub const fn armor_buff(id: &'static str, armor: RankValue) -> Self {
        Self::Modifier {
            id,
            kind: StatusKind::Buff,
            armor,
            magic_resist: RankValue::ZERO,
            move_speed: RankValue::ZERO,
            attack_damage: RankValue::ZERO,
            attack_speed_flat: RankValue::ZERO,
            attack_speed_mult: RankValue::ZERO,
            attack_range: RankValue::ZERO,
            attack_projectile_speed: RankValue::ZERO,
            attack_damage_pct: RankValue::ZERO,
            cleave: RankValue::ZERO,
            cleave_radius: RankValue::ZERO,
            cleave_end_radius: RankValue::ZERO,
            cleave_distance: RankValue::ZERO,
            attack_charges: RankValue::ZERO,
            independent: false,
        }
    }

    /// `ctx.move_speed` is read by percentage move and attack-speed slows.
    pub fn build(self, rank: u32, duration: f32, ctx: StatusBuildContext) -> StatusEffect {
        // Match `CombatStats::attack_speed_rating` defaults for unsourced targets (items, etc.).
        let ias = (100.0 + ctx.agility).clamp(20.0, 700.0);
        match self {
            StatusSpec::Stun => StatusEffect::stunned(duration),
            StatusSpec::OwnedStun { id } => StatusEffect {
                id,
                ..StatusEffect::stunned(duration)
            },
            StatusSpec::Root => StatusEffect::rooted(duration),
            StatusSpec::Silence => StatusEffect::silenced(duration),
            StatusSpec::Disarm => StatusEffect::disarmed(duration),
            StatusSpec::Phased => StatusEffect::phased(duration),
            StatusSpec::Frozen => StatusEffect::frozen(duration),
            StatusSpec::Forceful => StatusEffect::forceful(duration),
            StatusSpec::Flying => StatusEffect::flying(duration),
            StatusSpec::Leash { radius, anchor } => {
                let at_point = LeashAnchor::Point(Vec2::new(ctx.point.x, ctx.point.z));
                let anchor = match (anchor, ctx.source) {
                    (LeashAnchorKind::Caster, Some(source)) => LeashAnchor::Unit(source),
                    // No caster to follow: fall back to the spot where it landed.
                    _ => at_point,
                };
                StatusEffect::leashed(
                    Leash {
                        anchor,
                        radius: radius.at(rank).max(0.0),
                    },
                    duration,
                )
            }
            StatusSpec::DebuffImmunity => StatusEffect::debuff_immunity(duration),
            StatusSpec::SpellImmunity => StatusEffect::spell_immunity(duration),
            StatusSpec::AttackImmunity => StatusEffect::attack_immunity(duration),
            StatusSpec::Untargetable => StatusEffect::untargetable(duration),
            // A hostile cast is a debuff; the runner re-sets the kind from the real relation.
            StatusSpec::Ethereal { magic_amp_percent } => {
                StatusEffect::ethereal(duration, magic_amp_percent.at(rank), true)
            }
            StatusSpec::Banish => StatusEffect::banish(duration, true),
            StatusSpec::Shield { amount } => StatusEffect::shield(amount.at(rank), duration),
            StatusSpec::Burn {
                id,
                amount,
                damage_type,
                tick_interval,
            } => {
                let interval = tick_interval.at(rank);
                if interval > 0.0 {
                    StatusEffect::burn_ticking(id, amount.at(rank), interval, damage_type, duration)
                } else {
                    StatusEffect {
                        id,
                        ..StatusEffect::burn(amount.at(rank), damage_type, duration)
                    }
                }
            }
            StatusSpec::Mute => StatusEffect::mute(duration),
            StatusSpec::Sleep => StatusEffect::sleep(duration),
            StatusSpec::Invulnerable => StatusEffect::invulnerable(duration),
            StatusSpec::Hex => StatusEffect::hexed(duration),
            StatusSpec::Break => StatusEffect::broken(duration),
            StatusSpec::Invisibility { breaks_on_action } => {
                StatusEffect::invisibility(duration, breaks_on_action)
            }
            StatusSpec::Reveal => StatusEffect::reveal(duration),
            StatusSpec::SpellBlock => StatusEffect::spell_block(duration),
            StatusSpec::SpellReflect => StatusEffect::spell_reflect(duration),
            StatusSpec::Blind {
                miss_chance_percent,
            } => StatusEffect::blind(
                (miss_chance_percent.at(rank) / 100.0).clamp(0.0, 1.0),
                duration,
            ),
            StatusSpec::Evasion { percent } => StatusEffect {
                evasion: (percent.at(rank) / 100.0).clamp(0.0, 1.0),
                ..StatusEffect::buff("evasion", duration)
            },
            StatusSpec::Accuracy { percent } => StatusEffect {
                accuracy: (percent.at(rank) / 100.0).clamp(0.0, 1.0),
                ..StatusEffect::buff("accuracy", duration)
            },
            StatusSpec::TrueStrike => StatusEffect {
                accuracy: 1.0,
                ..StatusEffect::buff("true_strike", duration)
            },
            StatusSpec::Fear => StatusEffect::fear(duration, ctx.source),
            // Without a caster to look at or walk to there is nothing to force: a no-op status.
            StatusSpec::Taunt => match ctx.source {
                Some(source) => StatusEffect::taunted(duration, source),
                None => StatusEffect::debuff("taunt", duration),
            },
            StatusSpec::Hypnosis => match ctx.source {
                Some(source) => StatusEffect::hypnotized(duration, source),
                None => StatusEffect::debuff("hypnosis", duration),
            },
            StatusSpec::Slow { id, percent } => {
                let p = (percent.at(rank) / 100.0).clamp(0.0, 1.0);
                StatusEffect {
                    move_speed: -ctx.move_speed * p,
                    move_slow_fraction: p,
                    dispel_type: DispelType::Basic,
                    ..StatusEffect::debuff(id, duration)
                }
            }
            StatusSpec::MoveSpeedBonus { id, percent } => {
                let p = (percent.at(rank) / 100.0).clamp(0.0, 4.0);
                StatusEffect {
                    move_speed: ctx.move_speed * p,
                    move_bonus_fraction: p,
                    dispel_type: DispelType::Basic,
                    ..StatusEffect::buff(id, duration)
                }
            }
            StatusSpec::AttackPoint { id, seconds } => StatusEffect {
                attack_point_override: Some(
                    seconds
                        .at(rank)
                        .clamp(0.0, crate::basic_attack::MAX_ATTACK_SWING),
                ),
                dispel_type: DispelType::Basic,
                ..StatusEffect::buff(id, duration)
            },
            StatusSpec::AttackSpeedSlow { percent } => {
                let p = (percent.at(rank) / 100.0).clamp(0.0, 1.0);
                StatusEffect {
                    attack_speed_flat: -ias * p,
                    dispel_type: DispelType::Basic,
                    ..StatusEffect::debuff("attack_speed_slow", duration)
                }
            }
            StatusSpec::TurnRateSlow { percent } => StatusEffect {
                turn_rate_slow_percent: percent.at(rank).clamp(0.0, 100.0),
                dispel_type: DispelType::Basic,
                ..StatusEffect::debuff("turn_rate_slow", duration)
            },
            StatusSpec::TurnRateBonus { percent } => StatusEffect {
                turn_rate_bonus_percent: percent.at(rank).clamp(0.0, 400.0),
                dispel_type: DispelType::Basic,
                ..StatusEffect::buff("turn_rate_bonus", duration)
            },
            StatusSpec::HealOverTime {
                hps,
                max_health_pct,
            } => StatusEffect {
                heal_max_health_fraction: (max_health_pct.at(rank) / 100.0).clamp(0.0, 1.0),
                ..StatusEffect::heal_over_time(hps.at(rank), duration)
            },
            StatusSpec::HealReduction { percent } => {
                StatusEffect::heal_reduction((percent.at(rank) / 100.0).clamp(0.0, 1.0), duration)
            }
            StatusSpec::HealBlock => StatusEffect::heal_block(duration),
            StatusSpec::Stacks { id } => StatusEffect::stack(id, duration),
            StatusSpec::StatusResistance { id, percent } => StatusEffect::status_resistance(
                id,
                (percent.at(rank) / 100.0).clamp(0.0, 1.0),
                duration,
            ),
            StatusSpec::DamageReduction { id, percent } => StatusEffect::damage_reduction(
                id,
                (percent.at(rank) / 100.0).clamp(0.0, 1.0),
                duration,
            ),
            StatusSpec::ManaShield {
                absorb_percent,
                damage_per_mana,
            } => StatusEffect::mana_shield(
                ManaShield {
                    absorb: (absorb_percent.at(rank) / 100.0).clamp(0.0, 1.0),
                    damage_per_mana: damage_per_mana.at(rank).max(f32::EPSILON),
                },
                duration,
            ),
            StatusSpec::ResourceModifier {
                id,
                kind,
                stat,
                amount,
                independent,
            } => StatusEffect {
                independent,
                ..StatusEffect::resource_modifier(id, kind, stat, amount.at(rank), duration)
            },
            StatusSpec::Modifier {
                id,
                kind,
                armor,
                magic_resist,
                move_speed,
                attack_damage,
                attack_speed_flat,
                attack_speed_mult,
                attack_range,
                attack_projectile_speed,
                attack_damage_pct,
                cleave,
                cleave_radius,
                cleave_end_radius,
                cleave_distance,
                attack_charges,
                independent,
            } => {
                let base = match kind {
                    StatusKind::Buff => StatusEffect::buff(id, duration),
                    StatusKind::Debuff => StatusEffect::debuff(id, duration),
                };
                StatusEffect {
                    armor: armor.at(rank),
                    magic_resist: magic_resist.at(rank),
                    move_speed: move_speed.at(rank),
                    attack_damage: attack_damage.at(rank)
                        + attack_damage_pct.at(rank) / 100.0 * ctx.base_attack_damage,
                    attack_speed_flat: attack_speed_flat.at(rank),
                    attack_speed_mult: attack_speed_mult.at(rank),
                    attack_range: attack_range.at(rank),
                    attack_projectile_speed: attack_projectile_speed.at(rank),
                    cleave: cleave.at(rank),
                    cleave_radius: cleave_radius.at(rank),
                    cleave_end_radius: cleave_end_radius.at(rank),
                    cleave_distance: cleave_distance.at(rank),
                    attack_charges: attack_charges.at(rank).round().max(0.0) as u32,
                    independent,
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
    /// Toward the caster; tracks the caster while pulling.
    Pull,
    /// Toward the trigger point, up to the distance.
    PullToPoint,
    /// The target dashes in a straight line toward the aim point, at most `distance`.
    /// `duration` is the time for the full distance. Phases through units.
    Dash,
    /// The target runs at the cast target at `speed` until contact (or `distance` /
    /// `duration` run out). Needs a living cast target.
    Charge,
    /// The target rides along with the caster's own forced movement (a dash or charge),
    /// keeping its offset, until that movement ends or `duration` (when above zero) runs out.
    Drag,
    /// The target is lifted and held in place for `duration`: stunned, invulnerable,
    /// untargetable, and immune to other displacement and teleports.
    Cyclone,
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
    SpellAmp,
    OutgoingPhysicalAmp,
    IncomingDamageAmp,
    DamageBlock,
    DamageReflect,
    /// Healing the holder provides (fraction).
    HealAmp,
    /// Healing the holder receives (fraction).
    HealReceivedAmp,
    /// Fraction of basic-attack damage dealt returned as health.
    Lifesteal,
    /// Fraction of spell damage dealt returned as health.
    SpellLifesteal,
    /// Additive change to mana costs (-0.25 = 25% cheaper).
    ManaCostAmp,
    /// Chance (fraction) that basic attacks crit.
    CritChance,
    /// Bonus on top of the 2.0 base crit multiplier (0.5 = 2.5x).
    CritMultiplier,
    /// Chance (fraction) that attacks against the holder miss.
    Evasion,
    /// Chance (fraction) that the holder's attacks land despite blind or evasion.
    Accuracy,
    /// Fraction of attack damage dealt to enemies around a melee attacker.
    CleaveRatio,
    /// Fraction of attack damage dealt to enemies around the struck unit.
    SplashRatio,
    /// Flat attack range (does not change melee / ranged classification).
    AttackRange,
    /// Flat attack projectile speed (units / second).
    AttackProjectileSpeed,
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
    /// Spell damage that heals the caster for `transfer_ratio` of the health actually
    /// removed (health transfer).
    DrainHealth {
        amount: RankValue,
        damage_type: DamageType,
        scaling: EffectScaling,
        transfer_ratio: f32,
    },
    RestoreMana {
        amount: RankValue,
    },
    /// Remove mana from each target. `burn_ratio x removed` is dealt as spell damage (mana
    /// burn) and `transfer_ratio x removed` is given to the caster (mana drain).
    DrainMana {
        amount: RankValue,
        burn_ratio: f32,
        damage_type: DamageType,
        transfer_ratio: f32,
    },
    /// The target spends its own resource (health never drops below 1), optionally gaining
    /// `ratio x spent` of another (sacrifice / conversion).
    SpendResource {
        resource: ResourceKind,
        amount: RankValue,
        convert_to: Option<(ResourceKind, f32)>,
    },
    ApplyStatus {
        status: StatusSpec,
        duration: RankValue,
    },
    /// Remove every status of `kind` (reverting their stat modifiers).
    Dispel {
        kind: StatusKind,
        strength: crate::status::DispelStrength,
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
        /// Travel speed; only kinds that chase a unit read it.
        speed: Option<RankValue>,
        /// Dash and charge only: the reach from the mover's centre (plus the other unit's
        /// bound) within which it contacts enemies. `None` is body contact.
        contact_radius: Option<RankValue>,
        /// `PullToPoint` only: pull to a point this far in front of the caster (along its
        /// facing) instead of the trigger point. `None` keeps the trigger point.
        forward_offset: Option<RankValue>,
        /// `Drag` only: each dragged unit is also stunned under this status id (unique to the
        /// source, so no other stun is overwritten or removed) until the drag ends.
        bound_stun: Option<&'static str>,
    },
    /// Instantly relocates each target to the trigger's aim point, validated against the
    /// map, static obstacles, and unit overlap (see `crate::teleport`). Rejected
    /// destinations leave the target untouched.
    ///
    /// With bounds, the distance from the caster to the aim point is clamped to
    /// `min_distance..=max_distance` along the same line (an aim on the caster itself uses
    /// the caster's facing). `ZERO` / `None` leave the destination exactly at the aim point.
    Teleport {
        min_distance: RankValue,
        max_distance: Option<RankValue>,
    },
    /// Permanent bonus while an item is owned; only meaningful under `AbilityTrigger::Passive`.
    PassiveStat {
        stat: PassiveStat,
        amount: RankValue,
    },
    /// Gate: with probability `chance` (0..=1, one [`CombatRng`](crate::combat::CombatRng)
    /// draw per execution) re-fires this trigger's context under the custom trigger `then`.
    /// Entries tagged `then` hold the consequences, so one roll covers all of them.
    ///
    /// With `pseudo` the chance is pseudo-random (PRD): each failure raises the next roll
    /// and a success resets it, so the long-run rate is still `chance` but streaks are
    /// short. The failure count lives in `TriggerCounters`, per (owner, source, `then`).
    Proc {
        chance: RankValue,
        then: AbilityTrigger,
        pseudo: bool,
    },
    /// Gate: while the caster holds a live status with this `id` (see
    /// `StatusEffects::has_status`), re-fires this trigger's context under `then`. No RNG and
    /// no counters: it fires once per execution, so a refreshing status never stacks.
    RequireStatus {
        id: &'static str,
        then: AbilityTrigger,
    },
    /// Gate: every `every`th execution (counted per source and `then`) re-fires this
    /// trigger's context under the trigger `then`.
    Counter {
        every: u32,
        then: AbilityTrigger,
    },
    /// Mechanic gate: from each resolved start unit, jump up to `jumps` times to the nearest
    /// valid unit (team-allowed, alive, targetable, not yet hit) within `radius` of the
    /// previous unit, re-firing this context under `then` per hop
    /// (trigger unit = hop, point = its position, magnitude *= falloff^hop).
    /// The start unit is not re-fired. Emits no gameplay messages.
    Chain {
        jumps: u32,
        radius: RankValue,
        falloff: f32,
        team: TargetTeam,
        then: AbilityTrigger,
    },
    /// Mechanic gate: spawns a persistent area at the trigger point (the aim for `OnCast`).
    /// After `delay` seconds, and then every `interval` seconds for `duration` seconds in
    /// total (`floor(duration / interval)` ticks), it re-fires this context under `then`
    /// with the units of `team` inside `radius` as the affected units (the caster is never
    /// one). The re-fired context has no trigger unit, no affected units from the spawn and
    /// no attack. Emits no gameplay messages itself and runs once per execution.
    ///
    /// With `follow_caster` the zone is re-centred on the caster before every tick (the
    /// re-fired context's point and origin move with it) and ends as soon as the caster dies;
    /// otherwise it stays where it was spawned.
    Zone {
        radius: AreaRadius,
        team: TargetTeam,
        delay: RankValue,
        duration: RankValue,
        interval: RankValue,
        then: AbilityTrigger,
        follow_caster: bool,
    },
    /// Gate: sums `TriggerContext::amount` over executions (damage taken, distance walked, or
    /// 1 per occurrence) and, whenever the sum reaches `threshold`, re-fires this trigger's
    /// context under `then` once and keeps the remainder. The sum is counted per source and
    /// `then`.
    Accumulate {
        threshold: RankValue,
        then: AbilityTrigger,
    },
    /// Each resolved enemy gets an extra basic attack dealing `damage_ratio` (1.0 = 100%) of
    /// the caster's attack damage, ignoring cooldown, swing and range (instant attack, double
    /// attack, multi-target attack). The caster itself is never a target, nor the trigger unit
    /// when `exclude_trigger_unit` is set; at most `max_targets` are attacked, nearest first
    /// (0 = all). Does nothing under a trigger caused by an extra attack, so extra attacks
    /// never chain, nor under a reaction (damage- or status-class trigger).
    PerformAttack {
        damage_ratio: RankValue,
        max_targets: u32,
        exclude_trigger_unit: bool,
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

impl AbilityEffect {
    /// Whether the effect does something to its targets, as opposed to gates (which only
    /// re-fire the context) and passive stat entries.
    pub fn acts_on_targets(&self) -> bool {
        !matches!(
            self,
            Self::Proc { .. }
                | Self::RequireStatus { .. }
                | Self::Counter { .. }
                | Self::Accumulate { .. }
                | Self::Chain { .. }
                | Self::Zone { .. }
                | Self::PassiveStat { .. }
        )
    }
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
    /// With `first_hit` it stops at the first enemy it strikes instead (a hook).
    Projectile {
        splash_radius: AreaRadius,
        /// World units per second; `None` uses the default spell projectile speed.
        speed: Option<f32>,
        skillshot: bool,
        /// Skillshots only: end at the first enemy struck (no `OnImpact` after a hit).
        first_hit: bool,
        /// Skillshots only: the width (like `aoe_radius`, per rank) the wave widens to by the
        /// end of its flight. `None` keeps the width constant.
        end_radius: Option<RankValue>,
        /// Skillshots only: the wave starts this far in front of the caster (towards the aim)
        /// instead of at the caster's centre, and still flies the full cast range from there.
        origin_offset: Option<RankValue>,
    },
    /// One homing spell projectile per living, targetable unit of `team` inside the ability's
    /// `aoe_radius` around `center` (the caster is never one). Each projectile carries the
    /// cast's payload with that unit as its cast target, and its hit fires `OnProjectileHit`
    /// with the struck unit as the trigger unit. No splash.
    ProjectileToEach {
        center: AreaCenter,
        team: TargetTeam,
        /// World units per second; `None` uses the default spell projectile speed.
        speed: Option<f32>,
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
    /// Unit-targeted casts only: the cast is cancelled for free during the cast point if the
    /// target moves further than `cast_range + cast_range_buffer` from the caster. Zero = no limit.
    pub cast_range_buffer: RankValue,
    /// Area radius (area targets) or projectile width (point targets); also drives indicators.
    pub aoe_radius: RankValue,
    /// Cast point: delay before the ability fires (0..=`MAX_CAST_POINT`).
    pub cast_time: f32,
    /// Cancellable recovery after the ability fires (0..=`MAX_CAST_BACKSWING`).
    pub backswing: f32,
    pub cooldown: RankValue,
    pub mana_cost: RankValue,
    /// Seconds the caster channels after the cast point (`0` = not channelled).
    pub channel_time: f32,
    /// Mana drained per second while a toggle ability is on.
    pub toggle_mana_per_sec: RankValue,
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
            cast_range_buffer: RankValue::ZERO,
            aoe_radius: RankValue::ZERO,
            cast_time: 0.2,
            backswing: 0.25,
            cooldown: RankValue::ZERO,
            mana_cost: RankValue::ZERO,
            channel_time: 0.0,
            toggle_mana_per_sec: RankValue::ZERO,
            charges: None,
            description: "",
            mechanics: Vec::new(),
            effects: Vec::new(),
        }
    }

    pub fn targeting(
        mut self,
        target_type: TargetType,
        cast_range: RankValue,
        aoe_radius: RankValue,
    ) -> Self {
        self.target_type = target_type;
        self.cast_range = cast_range;
        self.aoe_radius = aoe_radius;
        self
    }

    /// How far past `cast_range` a unit target may move during the cast point before the
    /// cast is cancelled.
    pub fn cast_range_buffer(mut self, buffer: RankValue) -> Self {
        self.cast_range_buffer = buffer;
        self
    }

    pub fn costs(mut self, cooldown: RankValue, mana_cost: RankValue) -> Self {
        self.cooldown = cooldown;
        self.mana_cost = mana_cost;
        self
    }

    pub fn timing(mut self, cast_time: f32, backswing: f32) -> Self {
        self.cast_time = cast_time.clamp(0.0, MAX_CAST_POINT);
        self.backswing = backswing.clamp(0.0, MAX_CAST_BACKSWING);
        self
    }

    /// Mana per second a toggle ability drains while on.
    pub fn toggle_upkeep(mut self, mana_per_sec: RankValue) -> Self {
        self.toggle_mana_per_sec = mana_per_sec;
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
    pub fn on(
        mut self,
        trigger: AbilityTrigger,
        target: EffectTarget,
        effect: AbilityEffect,
    ) -> Self {
        self.effects.push(AbilityEffectEntry {
            trigger,
            target,
            effect,
        });
        self
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn effects_for(
        &self,
        trigger: AbilityTrigger,
    ) -> impl Iterator<Item = &AbilityEffectEntry> {
        self.effects
            .iter()
            .filter(move |entry| entry.trigger == trigger)
    }

    pub fn is_castable(&self) -> bool {
        self.behavior != AbilityBehavior::Passive
    }
}
