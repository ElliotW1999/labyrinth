//! Status effects: buffs, debuffs, dispel, shields and damage / heal over time.
//!
//! Owns the [`StatusEffects`] component, the [`StatusEffectEvent`] entry point that every
//! gameplay system uses to add or remove statuses, and the per-frame ticking.

use bevy::prelude::*;

use crate::combat::DamageEvent;
use crate::components::{CombatStats, DamageType, Health, HeroAttributes, ShieldPool, Team};

pub struct StatusPlugin;

impl Plugin for StatusPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<StatusEffectEvent>()
            .add_message::<StatusLifecycleEvent>()
            .add_systems(
                Update,
                tick_status_effects
                    .before(crate::combat::apply_damage_events)
                    .before(crate::vitals::sync_status_resource_caps)
                    // Interrupt checks must see a stun before it expires: a stun shorter than
                    // one frame still cancels a channel, cast point or attack swing.
                    .after(crate::actions::tick_activations)
                    .after(crate::basic_attack::tick_attack_swings),
            );
    }
}

/// Which kinds of action a unit is blocked from, as a small bit set. Derived from status flags
/// by [`StatusEffect::blocks`] / [`StatusEffects::blocks`]; every "can this unit act" check
/// reads it, so the disable matrix lives in one place.
///
/// | flag | MOVE | ATTACK | CAST | ITEMS | ORDERS | PASSIVES |
/// |---|---|---|---|---|---|---|
/// | stunned, asleep | x | x | x | x | | |
/// | rooted | x | | | | | |
/// | silenced | | | x | | | |
/// | muted | | | | x | | |
/// | disarmed | | x | | | | |
/// | breaks_passives | | | | | | x |
/// | compulsion Flee / Approach | | x | x | x | x | |
/// | compulsion Attack (taunt) | | | x | x | x | |
///
/// `ORDERS` drops player and AI orders; `PASSIVES` suppresses ability passives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActionBlocks(u8);

#[allow(dead_code)]
impl ActionBlocks {
    pub const NONE: Self = Self(0);
    pub const MOVE: Self = Self(1);
    pub const ATTACK: Self = Self(1 << 1);
    pub const CAST: Self = Self(1 << 2);
    pub const ITEMS: Self = Self(1 << 3);
    /// Player and AI orders are dropped (compulsions).
    pub const ORDERS: Self = Self(1 << 4);
    /// Ability passives are suppressed (break). Item passives are not (see D2 of the plan).
    pub const PASSIVES: Self = Self(1 << 5);

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Which damage types a status makes its holder immune to, as a small bit set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DamageTypeMask(u8);

#[allow(dead_code)]
impl DamageTypeMask {
    pub const NONE: Self = Self(0);
    pub const PHYSICAL: Self = Self(1);
    pub const MAGICAL: Self = Self(1 << 1);
    pub const PURE: Self = Self(1 << 2);
    pub const ALL: Self = Self(0b111);

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The single-bit mask of one damage type.
    pub const fn of(damage_type: DamageType) -> Self {
        match damage_type {
            DamageType::Physical => Self::PHYSICAL,
            DamageType::Magical => Self::MAGICAL,
            DamageType::Pure => Self::PURE,
        }
    }
}

/// Why a unit is being picked as a target; each intent rejects a different set of statuses
/// (see [`StatusEffects::rejects`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetIntent {
    /// Any unit pick or area membership test: only `untargetable` units are skipped.
    Any,
    /// A basic attack (including auto-acquisition): also skips attack-immune units.
    Attack,
    /// A hostile unit-target spell: also skips spell-immune units.
    HostileSpell,
}

/// Generic buff / debuff container on heroes (and potentially other units later).
#[derive(Component, Debug, Clone, Default)]
pub struct StatusEffects {
    pub effects: Vec<StatusEffect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum StatusKind {
    Buff,
    Debuff,
}

/// How hard this unit’s dispel is when purging matching [`StatusKind`] stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DispelStrength {
    /// Removes [`DispelType::Basic`] and [`DispelType::Strong`] stacks.
    #[default]
    Basic,
    /// Removes only [`DispelType::Strong`] (and Basic) stacks; Strong dispels act as full purge on dispellable effects.
    Strong,
}

/// Per-effect resistance to [`AbilityEffect::Dispel`] (basic vs strong purge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DispelType {
    /// Purgable by basic or strong dispel.
    #[default]
    Basic,
    /// Only strong dispel removes this (and basic debuff immunity purge).
    Strong,
    /// Never removed by dispel (still removed by explicit id / expiry / special rules).
    None,
}

/// What a [`Leash`] is measured from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LeashAnchor {
    /// A fixed spot on the ground (x, z).
    Point(Vec2),
    /// A unit; the leash does nothing once that unit is gone.
    Unit(Entity),
}

/// Keeps the unit inside a circle of `radius` around `anchor` (see `movement::enforce_leashes`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leash {
    pub anchor: LeashAnchor,
    pub radius: f32,
}

/// What a status forces its holder to do, enforced by `ai::enforce_compulsions` (the only
/// order writer for compelled units) and `movement::apply_move_targets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compulsion {
    /// Run away from `from` (fear). With no source (or a despawned one) the unit runs straight
    /// ahead along its facing.
    Flee { from: Option<Entity> },
    /// Walk to `toward` and stop at contact (hypnosis).
    Approach { toward: Entity },
    /// Basic attack only `target` (taunt).
    Attack { target: Entity },
}

#[derive(Debug, Clone)]
pub struct StatusEffect {
    pub id: &'static str,
    pub kind: StatusKind,
    pub remaining: f32,
    pub dispel_type: DispelType,
    /// When true, no dispel removes this (same as [`DispelType::None`] but skips strong purge too).
    pub undispellable: bool,
    /// Flat combat modifiers applied for the duration (removed on expire).
    pub attack_damage: f32,
    pub armor: f32,
    pub magic_resist: f32,
    pub move_speed: f32,
    /// Percent slow as a fraction (0..1) of the holder's base move speed. When non-zero,
    /// [`apply_status`] recomputes `move_speed` as `-fraction x base`, where base is the move
    /// speed without any status deltas, so a refreshed slow never compounds on itself.
    pub move_slow_fraction: f32,
    /// Percent move-speed bonus as a fraction (0..4) of the holder's base move speed; the mirror
    /// of [`Self::move_slow_fraction`]. [`apply_status`] measures `move_speed` as
    /// `(bonus - slow) x base`, so a refreshed bonus never compounds on itself.
    pub move_bonus_fraction: f32,
    /// Flat IAS change from this effect (negative values slow attack speed).
    pub attack_speed_flat: f32,
    /// Multiplicative AS modifier × (1 + `attack_speed_mult`) — negative slows.
    pub attack_speed_mult: f32,
    /// Percent slow applied to turn rate (0..100), see [`CombatStats::turn_rate`].
    pub turn_rate_slow_percent: f32,
    /// Percent added to turn rate (0..=400); bonuses add up.
    pub turn_rate_bonus_percent: f32,
    /// 0..1 miss chance on basic attack damage resolution (`0` = no blind).
    pub blind_miss_chance: f32,
    /// 0..1 chance that attacks against the holder miss. Stacks with other statuses.
    pub evasion: f32,
    /// 0..1 chance that the holder's attacks hit despite a blind or evasion roll
    /// (1.0 is true strike). Stacks with other statuses.
    pub accuracy: f32,
    /// Flat attack range change; also tracked in `CombatStats::attack_range_bonus`.
    pub attack_range: f32,
    /// Cleave ratio contributed to this status's cleave instance while active (melee
    /// attackers only cleave). The four cleave fields sum per `cleave_source` into one
    /// `CombatStats::cleaves` instance.
    pub cleave: f32,
    /// The cleave's starting half-width (circle radius) in that instance.
    pub cleave_radius: f32,
    /// Half-width at `cleave_distance` in that instance.
    pub cleave_end_radius: f32,
    /// Reach of that instance; non-zero makes it a cone.
    pub cleave_distance: f32,
    /// Owner of the cleave instance (the ability whose statuses carry it). `None` keys the
    /// instance by this status's id.
    pub cleave_source: Option<crate::abilities::effects::EffectSource>,
    pub heal_per_sec: f32,
    /// Timed resource modifier, summed when read (see [`crate::vitals::resource_stat`]).
    pub resource: Option<(crate::vitals::ResourceStat, f32)>,
    /// Fraction (0..1) of healing received that is negated. The strongest one applies.
    pub heal_reduction: f32,
    /// Fraction (0..1) of incoming damage removed after mitigation; reductions multiply.
    pub damage_reduction: f32,
    /// Fraction (0..1) by which the durations of new hostile debuffs are shortened;
    /// resistances multiply. Read by [`apply_status_effect_events`] when a debuff lands.
    pub status_resistance: f32,
    /// Basic attacks (released, hit or miss) this status survives; `0` = not charge-limited.
    /// Decremented by [`StatusChange::ConsumeAttackCharge`]; the status ends at zero.
    pub attack_charges: u32,
    /// Stack count of a stacking status (`0` = not a stacking status). Applying the same id
    /// again adds the old count to the new one and refreshes the duration.
    pub stacks: u32,
    /// Prevents all healing while active.
    pub heal_blocked: bool,
    /// Diverts incoming damage to mana while active.
    pub mana_shield: Option<crate::vitals::ManaShield>,
    pub damage_per_sec: f32,
    /// Ticking burn: seconds between ticks (`0` = no ticking; see [`StatusEffect::burn_ticking`]).
    pub dot_interval: f32,
    /// Ticking burn: damage dealt by each tick.
    pub dot_per_tick: f32,
    /// Ticking burn: time accumulated towards the next tick.
    pub dot_timer: f32,
    /// Ticking burn: ticks still owed. Written only by [`tick_status_effects`].
    pub dot_ticks_left: u32,
    pub dot_damage_type: DamageType,
    pub dot_source: Option<Entity>,
    /// Added to [`ShieldPool`] on apply; removed on expire.
    pub shield_amount: f32,
    /// Damage types the holder takes no damage from while active.
    pub immune_damage: DamageTypeMask,
    /// Nobody else may pick the holder as a unit target and area effects skip it.
    pub untargetable: bool,
    /// Applications of this id never replace each other: every one keeps its own instance and
    /// timer ([`apply_status`] skips the same-id refresh). Removal by id, dispel and debuff
    /// immunity still remove every matching instance.
    pub independent: bool,
    /// Targeting only: basic attacks and hostile unit-target spells cannot pick the holder
    /// ([`TargetIntent::Attack`], [`TargetIntent::HostileSpell`]), but area effects, dash contact
    /// and projectiles already in flight still reach it. Unlike [`Self::untargetable`] it
    /// removes the holder from nothing but unit picks. Read only by [`StatusEffects::rejects`].
    pub targeting_blocked: bool,
    /// Basic attacks cannot target the holder ([`TargetIntent::Attack`]).
    pub attack_immune: bool,
    /// Hostile unit-target spells cannot target the holder ([`TargetIntent::HostileSpell`]).
    pub spell_immune: bool,
    /// Drawn for nobody (banished out of the world), whatever the vision.
    pub hidden: bool,
    /// Concealed from enemies (no attack or hostile spell may pick the holder, and fog hides it
    /// even inside vision) unless some live [`StatusEffect::revealed`] is present.
    pub invisible: bool,
    /// True sight: while any live effect has this set, the holder's invisibility is void.
    pub revealed: bool,
    /// With `invisible`: the status is removed when the holder attacks or fires an action.
    pub invis_breaks_on_action: bool,
    /// Negates the next hostile unit-target spell that lands on the holder, then is consumed.
    pub spell_block: bool,
    /// Reflects hostile unit-target spells back at their caster (the original still lands).
    pub spell_reflect: bool,
    /// When true, skip collision with creeps and heroes (buildings/trees still block).
    pub ignore_unit_collision: bool,
    /// When true, this unit claims more separation against overlapping creeps/heroes.
    pub force_unit_push: bool,
    pub silenced: bool,
    pub stunned: bool,
    pub rooted: bool,
    pub disarmed: bool,
    pub muted: bool,
    pub asleep: bool,
    pub invulnerable: bool,
    pub debuff_immune: bool,
    /// Ability passives (stats and triggers) are suppressed while active (break, hex).
    pub breaks_passives: bool,
    /// Caps the holder's walking speed (`0` = no cap). The lowest live cap applies.
    pub move_speed_cap: f32,
    /// Ignores trees and other ground obstacles while moving (buildings still block).
    pub flying: bool,
    /// Confines the unit to a circle (region restriction or tether).
    pub leash: Option<Leash>,
    /// Forced behaviour (fear, taunt, hypnosis) enforced by `ai::enforce_compulsions`.
    pub compulsion: Option<Compulsion>,
    /// Applied by a reaction or held by a condition: expires and is removed silently (it
    /// never fires status triggers).
    pub reactive: bool,
}

impl StatusEffect {
    /// Whether any cleave field is set, so the status feeds a cleave instance.
    pub(crate) fn carries_cleave(&self) -> bool {
        self.cleave != 0.0
            || self.cleave_radius != 0.0
            || self.cleave_end_radius != 0.0
            || self.cleave_distance != 0.0
    }

    fn cleave_key(&self) -> crate::components::CleaveSource {
        match self.cleave_source {
            Some(source) => crate::components::CleaveSource::Effect(source),
            None => crate::components::CleaveSource::Status(self.id),
        }
    }

    /// What this single status blocks, ignoring its duration.
    pub fn blocks(&self) -> ActionBlocks {
        let mut blocks = ActionBlocks::NONE;
        if self.stunned || self.asleep {
            blocks = blocks
                .union(ActionBlocks::MOVE)
                .union(ActionBlocks::ATTACK)
                .union(ActionBlocks::CAST)
                .union(ActionBlocks::ITEMS);
        }
        if self.rooted {
            blocks = blocks.union(ActionBlocks::MOVE);
        }
        if self.silenced {
            blocks = blocks.union(ActionBlocks::CAST);
        }
        if self.muted {
            blocks = blocks.union(ActionBlocks::ITEMS);
        }
        if self.disarmed {
            blocks = blocks.union(ActionBlocks::ATTACK);
        }
        if self.breaks_passives {
            blocks = blocks.union(ActionBlocks::PASSIVES);
        }
        match self.compulsion {
            Some(Compulsion::Flee { .. } | Compulsion::Approach { .. }) => {
                blocks = blocks
                    .union(ActionBlocks::ATTACK)
                    .union(ActionBlocks::CAST)
                    .union(ActionBlocks::ITEMS)
                    .union(ActionBlocks::ORDERS);
            }
            Some(Compulsion::Attack { .. }) => {
                blocks = blocks
                    .union(ActionBlocks::CAST)
                    .union(ActionBlocks::ITEMS)
                    .union(ActionBlocks::ORDERS);
            }
            None => {}
        }
        blocks
    }

    pub fn buff(id: &'static str, duration: f32) -> Self {
        Self {
            id,
            kind: StatusKind::Buff,
            remaining: duration,
            dispel_type: DispelType::Basic,
            undispellable: false,
            attack_damage: 0.0,
            armor: 0.0,
            magic_resist: 0.0,
            move_speed: 0.0,
            move_slow_fraction: 0.0,
            move_bonus_fraction: 0.0,
            attack_speed_flat: 0.0,
            attack_speed_mult: 0.0,
            turn_rate_slow_percent: 0.0,
            turn_rate_bonus_percent: 0.0,
            blind_miss_chance: 0.0,
            evasion: 0.0,
            accuracy: 0.0,
            attack_range: 0.0,
            cleave: 0.0,
            cleave_radius: 0.0,
            cleave_end_radius: 0.0,
            cleave_distance: 0.0,
            cleave_source: None,
            heal_per_sec: 0.0,
            resource: None,
            heal_reduction: 0.0,
            damage_reduction: 0.0,
            status_resistance: 0.0,
            attack_charges: 0,
            stacks: 0,
            heal_blocked: false,
            mana_shield: None,
            damage_per_sec: 0.0,
            dot_interval: 0.0,
            dot_per_tick: 0.0,
            dot_timer: 0.0,
            dot_ticks_left: 0,
            dot_damage_type: DamageType::Magical,
            dot_source: None,
            shield_amount: 0.0,
            immune_damage: DamageTypeMask::NONE,
            untargetable: false,
            independent: false,
            targeting_blocked: false,
            attack_immune: false,
            spell_immune: false,
            hidden: false,
            invisible: false,
            revealed: false,
            invis_breaks_on_action: false,
            spell_block: false,
            spell_reflect: false,
            ignore_unit_collision: false,
            force_unit_push: false,
            silenced: false,
            stunned: false,
            rooted: false,
            disarmed: false,
            muted: false,
            asleep: false,
            invulnerable: false,
            debuff_immune: false,
            breaks_passives: false,
            move_speed_cap: 0.0,
            flying: false,
            leash: None,
            compulsion: None,
            reactive: false,
        }
    }

    pub fn shield(amount: f32, duration: f32) -> Self {
        Self {
            shield_amount: amount,
            ..Self::buff("shield", duration)
        }
    }

    pub fn heal_over_time(hps: f32, duration: f32) -> Self {
        Self {
            heal_per_sec: hps,
            ..Self::buff("heal_over_time", duration)
        }
    }

    /// `fraction` (0..1) of healing received is negated.
    pub fn heal_reduction(fraction: f32, duration: f32) -> Self {
        Self {
            heal_reduction: fraction,
            ..Self::debuff("heal_reduction", duration)
        }
    }

    /// One stack of a per-target stacking debuff; never removed by dispels.
    pub fn stack(id: &'static str, duration: f32) -> Self {
        Self {
            stacks: 1,
            dispel_type: DispelType::None,
            ..Self::debuff(id, duration)
        }
    }

    /// `fraction` (0..1) of incoming damage is removed (a buff).
    pub fn damage_reduction(id: &'static str, fraction: f32, duration: f32) -> Self {
        Self {
            damage_reduction: fraction.clamp(0.0, 1.0),
            ..Self::buff(id, duration)
        }
    }

    /// `fraction` (0..1) of the duration of new hostile debuffs is cut (a buff).
    pub fn status_resistance(id: &'static str, fraction: f32, duration: f32) -> Self {
        Self {
            status_resistance: fraction.clamp(0.0, 1.0),
            ..Self::buff(id, duration)
        }
    }

    /// Multiply the remaining duration by `factor`; a ticking burn keeps only the ticks that
    /// still fit.
    fn scale_duration(&mut self, factor: f32) {
        self.remaining *= factor;
        if self.dot_interval > 0.0 {
            let fit = (self.remaining / self.dot_interval + 1e-4).floor().max(0.0) as u32;
            self.dot_ticks_left = self.dot_ticks_left.min(fit);
        }
    }

    pub fn heal_block(duration: f32) -> Self {
        Self {
            heal_blocked: true,
            ..Self::debuff("heal_block", duration)
        }
    }

    /// A timed resource modifier, summed when read (never stored into stats).
    pub fn resource_modifier(
        id: &'static str,
        kind: StatusKind,
        stat: crate::vitals::ResourceStat,
        value: f32,
        duration: f32,
    ) -> Self {
        let base = match kind {
            StatusKind::Buff => Self::buff(id, duration),
            StatusKind::Debuff => Self::debuff(id, duration),
        };
        Self {
            resource: Some((stat, value)),
            ..base
        }
    }

    pub fn mana_shield(shield: crate::vitals::ManaShield, duration: f32) -> Self {
        Self {
            mana_shield: Some(shield),
            ..Self::buff("mana_shield", duration)
        }
    }

    pub fn burn(dps: f32, damage_type: DamageType, duration: f32) -> Self {
        Self {
            damage_per_sec: dps,
            dot_damage_type: damage_type,
            ..Self::debuff("burn", duration)
        }
    }

    /// A burn that deals `per_tick` every `interval` seconds, the first one `interval` after
    /// application. `floor(duration / interval)` ticks are dealt in total, whatever the frame
    /// rate: ticks still owed when the burn expires on its own are flushed (a dispel or a
    /// refresh does not flush). The id is the caller's, so burns from different sources coexist.
    pub fn burn_ticking(
        id: &'static str,
        per_tick: f32,
        interval: f32,
        damage_type: DamageType,
        duration: f32,
    ) -> Self {
        let interval = interval.max(0.0);
        let ticks = if interval > 0.0 {
            (duration / interval + 1e-4).floor().max(0.0) as u32
        } else {
            0
        };
        Self {
            dot_interval: interval,
            dot_per_tick: per_tick,
            dot_ticks_left: ticks,
            dot_damage_type: damage_type,
            ..Self::debuff(id, duration)
        }
    }

    pub fn debuff(id: &'static str, duration: f32) -> Self {
        Self {
            kind: StatusKind::Debuff,
            ..Self::buff(id, duration)
        }
    }

    pub fn phased(duration: f32) -> Self {
        Self {
            ignore_unit_collision: true,
            ..Self::buff("phased", duration)
        }
    }

    /// Temporary soft-separation against other units.
    /// Ground obstacles stop blocking the unit. When it ends over geometry the unit is
    /// set down on the nearest walkable spot.
    pub fn flying(duration: f32) -> Self {
        Self {
            flying: true,
            ..Self::buff("flying", duration)
        }
    }

    /// Confinement to a circle around `leash.anchor`. `id` is `"leash"` for a fixed
    /// region and `"tether"` for a unit anchor.
    pub fn leashed(leash: Leash, duration: f32) -> Self {
        let id = match leash.anchor {
            LeashAnchor::Point(_) => "leash",
            LeashAnchor::Unit(_) => "tether",
        };
        Self {
            leash: Some(leash),
            ..Self::debuff(id, duration)
        }
    }

    pub fn forceful(duration: f32) -> Self {
        Self {
            force_unit_push: true,
            ..Self::buff("forceful", duration)
        }
    }

    pub fn silenced(duration: f32) -> Self {
        Self {
            silenced: true,
            ..Self::debuff("silence", duration)
        }
    }

    pub fn stunned(duration: f32) -> Self {
        Self {
            stunned: true,
            ..Self::debuff("stun", duration)
        }
    }

    pub fn rooted(duration: f32) -> Self {
        Self {
            rooted: true,
            ..Self::debuff("root", duration)
        }
    }

    pub fn disarmed(duration: f32) -> Self {
        Self {
            disarmed: true,
            ..Self::debuff("disarm", duration)
        }
    }

    /// Blocks new debuffs and purges existing ones on application. Not a spell immunity:
    /// magic damage and hostile spells still land (see [`Self::spell_immunity`]).
    pub fn debuff_immunity(duration: f32) -> Self {
        Self {
            debuff_immune: true,
            dispel_type: DispelType::None,
            undispellable: true,
            ..Self::buff("debuff_immunity", duration)
        }
    }

    /// Debuff immunity plus: hostile unit-target spells cannot pick the holder and magical
    /// damage is ignored. Area spells still reach it but deal no magic damage.
    pub fn spell_immunity(duration: f32) -> Self {
        Self {
            id: "spell_immunity",
            spell_immune: true,
            immune_damage: DamageTypeMask::MAGICAL,
            ..Self::debuff_immunity(duration)
        }
    }

    /// Cannot be picked by attacks or hostile unit-target spells. Area effects, dash contact and
    /// in-flight projectiles still reach the holder (see [`StatusEffect::targeting_blocked`]).
    pub fn untargetable(duration: f32) -> Self {
        Self {
            targeting_blocked: true,
            ..Self::buff("untargetable", duration)
        }
    }

    /// Basic attacks cannot target the holder.
    pub fn attack_immunity(duration: f32) -> Self {
        Self {
            attack_immune: true,
            ..Self::buff("attack_immunity", duration)
        }
    }

    /// Disarmed, immune to physical damage and to being attacked, and more vulnerable to
    /// magic: `magic_amp_percent` (0..90) is the extra magic damage taken against a target with
    /// no magic resistance, applied as a magic resistance reduction. `hostile` picks the
    /// debuff kind (blocked by debuff immunity) over the buff kind (self / ally cast).
    pub fn ethereal(duration: f32, magic_amp_percent: f32, hostile: bool) -> Self {
        let amp = (magic_amp_percent / 100.0).clamp(0.0, 0.9);
        // `mitigate` gives factor 1 - 0.06r / (1 + 0.06|r|); r = -amp / (0.06 (1 - amp)) makes
        // that 1 + amp at zero base resistance.
        let resist_points = amp / (0.06 * (1.0 - amp));
        Self {
            disarmed: true,
            attack_immune: true,
            immune_damage: DamageTypeMask::PHYSICAL,
            magic_resist: -resist_points,
            kind: Self::kind_for_hostility(hostile),
            ..Self::buff("ethereal", duration)
        }
    }

    /// Removed from the world: stunned, invulnerable, untargetable, hidden and phased. Not
    /// dispellable; only expiry or an explicit `remove_status banish` ends it. `hostile`
    /// picks the debuff kind over the buff kind, as for [`Self::cyclone`].
    pub fn banish(duration: f32, hostile: bool) -> Self {
        Self {
            stunned: true,
            invulnerable: true,
            untargetable: true,
            hidden: true,
            ignore_unit_collision: true,
            kind: Self::kind_for_hostility(hostile),
            dispel_type: DispelType::None,
            undispellable: true,
            ..Self::buff("banish", duration)
        }
    }

    fn kind_for_hostility(hostile: bool) -> StatusKind {
        if hostile {
            StatusKind::Debuff
        } else {
            StatusKind::Buff
        }
    }

    pub fn mute(duration: f32) -> Self {
        Self {
            muted: true,
            ..Self::debuff("mute", duration)
        }
    }

    pub fn sleep(duration: f32) -> Self {
        Self {
            asleep: true,
            ..Self::debuff("sleep", duration)
        }
    }

    pub fn invulnerable(duration: f32) -> Self {
        Self {
            invulnerable: true,
            kind: StatusKind::Buff,
            dispel_type: DispelType::Strong,
            ..Self::buff("invulnerable", duration)
        }
    }

    /// Lifted out of play: stunned, invulnerable, and untargetable. `hostile` picks the
    /// debuff kind (blocked by debuff immunity) over the buff kind (self / ally cast).
    /// Not purgeable; only expiry or an explicit `remove_status cyclone` ends it.
    pub fn cyclone(duration: f32, hostile: bool) -> Self {
        Self {
            stunned: true,
            invulnerable: true,
            untargetable: true,
            kind: Self::kind_for_hostility(hostile),
            dispel_type: DispelType::None,
            undispellable: true,
            ..Self::buff("cyclone", duration)
        }
    }

    pub fn blind(miss_chance: f32, duration: f32) -> Self {
        Self {
            blind_miss_chance: miss_chance.clamp(0.0, 1.0),
            ..Self::debuff("blind", duration)
        }
    }

    /// Runs away from `source`, and cannot attack, cast, use items or take orders.
    pub fn fear(duration: f32, source: Option<Entity>) -> Self {
        Self {
            compulsion: Some(Compulsion::Flee { from: source }),
            dispel_type: DispelType::Basic,
            ..Self::debuff("fear", duration)
        }
    }

    /// Forced to basic attack `source` only; cannot cast, use items or take orders.
    pub fn taunted(duration: f32, source: Entity) -> Self {
        Self {
            compulsion: Some(Compulsion::Attack { target: source }),
            ..Self::debuff("taunt", duration)
        }
    }

    /// Forced to walk to `source` and stop at contact; cannot attack, cast, use items or take
    /// orders.
    pub fn hypnotized(duration: f32, source: Entity) -> Self {
        Self {
            compulsion: Some(Compulsion::Approach { toward: source }),
            ..Self::debuff("hypnosis", duration)
        }
    }

    /// Concealed from enemies until revealed; with `breaks_on_action` it ends when the holder
    /// attacks or casts.
    pub fn invisibility(duration: f32, breaks_on_action: bool) -> Self {
        Self {
            invisible: true,
            invis_breaks_on_action: breaks_on_action,
            ..Self::buff("invisibility", duration)
        }
    }

    /// True sight: voids the holder's invisibility.
    pub fn reveal(duration: f32) -> Self {
        Self {
            revealed: true,
            ..Self::debuff("reveal", duration)
        }
    }

    /// Negates the next hostile unit-target spell, then is consumed.
    pub fn spell_block(duration: f32) -> Self {
        Self {
            spell_block: true,
            ..Self::buff("spell_block", duration)
        }
    }

    /// Reflects hostile unit-target spells at their caster (Lotus style: the original lands too).
    pub fn spell_reflect(duration: f32) -> Self {
        Self {
            spell_reflect: true,
            ..Self::buff("spell_reflect", duration)
        }
    }

    /// Only ability passives are suppressed (stats and triggers); items keep working.
    pub fn broken(duration: f32) -> Self {
        Self {
            breaks_passives: true,
            ..Self::debuff("break", duration)
        }
    }

    /// Full disable: silence + disarm + mute (items only) + break, and walking speed capped
    /// at [`crate::scale::HEX_MOVE_SPEED`].
    pub fn hexed(duration: f32) -> Self {
        Self {
            id: "hex",
            silenced: true,
            disarmed: true,
            muted: true,
            breaks_passives: true,
            move_speed_cap: crate::scale::HEX_MOVE_SPEED,
            dispel_type: DispelType::Strong,
            ..Self::debuff("hex", duration)
        }
    }
}

impl StatusEffects {
    /// True while any active effect prevents healing.
    pub fn is_heal_blocked(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.heal_blocked && e.remaining > 0.0)
    }

    /// Strongest active healing reduction (0..1); reductions do not stack.
    pub fn heal_reduction(&self) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .map(|e| e.heal_reduction)
            .fold(0.0, f32::max)
            .clamp(0.0, 1.0)
    }

    /// Stacks of the stacking status `id` currently on the unit (0 when absent).
    pub fn stacks(&self, id: &str) -> u32 {
        self.effects
            .iter()
            .filter(|e| e.id == id && e.remaining > 0.0)
            .map(|e| e.stacks)
            .sum()
    }

    /// Product of `(1 - r)` over every active damage reduction: the factor applied to damage
    /// taken after mitigation (1.0 = none).
    pub fn damage_taken_factor(&self) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0 && e.damage_reduction > 0.0)
            .map(|e| 1.0 - e.damage_reduction.clamp(0.0, 1.0))
            .product()
    }

    /// Product of `(1 - r)` over every active status resistance: the factor applied to the
    /// duration of a hostile debuff as it lands (1.0 = none).
    pub fn status_resistance_factor(&self) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0 && e.status_resistance > 0.0)
            .map(|e| 1.0 - e.status_resistance.clamp(0.0, 1.0))
            .product()
    }

    /// The active mana shield with the highest absorb fraction.
    pub fn mana_shield(&self) -> Option<crate::vitals::ManaShield> {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .filter_map(|e| e.mana_shield)
            .max_by(|a, b| a.absorb.total_cmp(&b.absorb))
    }

    /// Sum of every active timed modifier for `stat`.
    pub fn resource_sum(&self, stat: crate::vitals::ResourceStat) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .filter_map(|e| e.resource)
            .filter(|(s, _)| *s == stat)
            .map(|(_, v)| v)
            .sum()
    }

    pub fn is_phased(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.ignore_unit_collision && e.remaining > 0.0)
    }

    /// Active leashes and tethers.
    pub fn leashes(&self) -> impl Iterator<Item = &Leash> {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .filter_map(|e| e.leash.as_ref())
    }

    pub fn is_flying(&self) -> bool {
        self.effects.iter().any(|e| e.flying && e.remaining > 0.0)
    }

    pub fn can_push_units(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.force_unit_push && e.remaining > 0.0)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_stunned(&self) -> bool {
        self.effects.iter().any(|e| e.stunned && e.remaining > 0.0)
    }

    pub fn is_asleep(&self) -> bool {
        self.effects.iter().any(|e| e.asleep && e.remaining > 0.0)
    }

    /// Union of what every live status blocks.
    pub fn blocks(&self) -> ActionBlocks {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .fold(ActionBlocks::NONE, |acc, e| acc.union(e.blocks()))
    }

    /// Cannot move (root, stun, sleep).
    pub fn is_rooted(&self) -> bool {
        self.blocks().contains(ActionBlocks::MOVE)
    }

    /// Cannot basic attack (disarm, stun, sleep).
    pub fn is_disarmed(&self) -> bool {
        self.blocks().contains(ActionBlocks::ATTACK)
    }

    /// Cannot use items (mute, stun, sleep).
    pub fn blocks_items(&self) -> bool {
        self.blocks().contains(ActionBlocks::ITEMS)
    }

    /// The forced behaviour in effect: the most recently applied live compulsion wins.
    pub fn compulsion(&self) -> Option<Compulsion> {
        self.effects
            .iter()
            .rev()
            .filter(|e| e.remaining > 0.0)
            .find_map(|e| e.compulsion)
    }

    /// Ability passives are suppressed (break, hex).
    pub fn breaks_passives(&self) -> bool {
        self.blocks().contains(ActionBlocks::PASSIVES)
    }

    /// The lowest live walking speed cap (`None` = uncapped).
    pub fn move_speed_cap(&self) -> Option<f32> {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0 && e.move_speed_cap > 0.0)
            .map(|e| e.move_speed_cap)
            .min_by(f32::total_cmp)
    }

    pub fn blind_miss_chance(&self) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0)
            .fold(0.0_f32, |acc, e| acc.max(e.blind_miss_chance))
            .clamp(0.0, 1.0)
    }

    /// Evasion of every live status, one entry each (they stack multiplicatively).
    pub fn evasions(&self) -> impl Iterator<Item = f32> + '_ {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0 && e.evasion > 0.0)
            .map(|e| e.evasion)
    }

    /// Accuracy of every live status, one entry each (they stack multiplicatively).
    pub fn accuracies(&self) -> impl Iterator<Item = f32> + '_ {
        self.effects
            .iter()
            .filter(|e| e.remaining > 0.0 && e.accuracy > 0.0)
            .map(|e| e.accuracy)
    }

    pub fn has_debuff_immunity(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.debuff_immune && e.remaining > 0.0)
    }

    /// Hostile displacement and teleports of this unit are refused (debuff immunity).
    pub fn blocks_hostile_relocation(&self) -> bool {
        self.has_debuff_immunity()
    }

    /// Whether a live status makes the holder immune to `damage_type`.
    pub fn is_immune_to(&self, damage_type: DamageType) -> bool {
        let wanted = DamageTypeMask::of(damage_type);
        self.effects
            .iter()
            .any(|e| e.remaining > 0.0 && e.immune_damage.contains(wanted))
    }

    pub fn is_invulnerable(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.invulnerable && e.remaining > 0.0)
    }

    /// Whether a status with this id is active.
    pub fn has_status(&self, id: &str) -> bool {
        self.effects.iter().any(|e| e.id == id && e.remaining > 0.0)
    }

    /// Held out of play (cyclone, banish): no other unit can pick it as a unit target (basic
    /// attacks, unit casts, auto-acquire, homing projectiles) and area effects skip it.
    pub fn is_untargetable(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.untargetable && e.remaining > 0.0)
    }

    /// Invisible and not revealed: enemies cannot pick it and fog hides it.
    pub fn is_concealed(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.invisible && e.remaining > 0.0)
            && !self.effects.iter().any(|e| e.revealed && e.remaining > 0.0)
    }

    /// Whether a live effect reflects hostile unit-target spells.
    pub fn has_spell_reflect(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.spell_reflect && e.remaining > 0.0)
    }

    /// Whether a live effect blocks attacks and hostile spells from picking the holder, while
    /// leaving area effects alone (see [`StatusEffect::targeting_blocked`]).
    fn is_targeting_blocked(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.targeting_blocked && e.remaining > 0.0)
    }

    /// Whether a unit pick made with `intent` must skip this holder.
    pub fn rejects(&self, intent: TargetIntent) -> bool {
        self.is_untargetable()
            || match intent {
                TargetIntent::Any => false,
                TargetIntent::Attack => {
                    self.effects
                        .iter()
                        .any(|e| e.attack_immune && e.remaining > 0.0)
                        || self.is_targeting_blocked()
                        || self.is_concealed()
                }
                TargetIntent::HostileSpell => {
                    self.effects
                        .iter()
                        .any(|e| e.spell_immune && e.remaining > 0.0)
                        || self.is_targeting_blocked()
                        || self.is_concealed()
                }
            }
    }

    /// Drawn for nobody (banish).
    pub fn is_hidden(&self) -> bool {
        self.effects.iter().any(|e| e.hidden && e.remaining > 0.0)
    }

    pub fn can_move(&self) -> bool {
        !self.is_rooted()
    }

    pub fn can_attack(&self) -> bool {
        !self.is_disarmed()
    }

    pub fn can_cast(&self) -> bool {
        !self.blocks().contains(ActionBlocks::CAST)
    }
}

/// Whether `entity` is held out of play (cyclone, banish) and so cannot be picked as a target.
/// The `Any` form of [`is_targetable`], used by area filters.
pub fn is_untargetable(statuses: &Query<&StatusEffects>, entity: Entity) -> bool {
    statuses
        .get(entity)
        .is_ok_and(|statuses| statuses.is_untargetable())
}

/// Whether `entity` may be picked as a unit target for `intent`. The one check behind every
/// "can this unit be targeted" filter that is not an area test.
pub fn is_targetable(
    statuses: &Query<&StatusEffects>,
    entity: Entity,
    intent: TargetIntent,
) -> bool {
    !statuses
        .get(entity)
        .is_ok_and(|statuses| statuses.rejects(intent))
}

/// Whether `effect` is removed by a dispel of `strength` on `kind`.
pub fn status_dispel_matches(
    effect: &StatusEffect,
    kind: StatusKind,
    strength: DispelStrength,
) -> bool {
    if effect.kind != kind {
        return false;
    }
    if effect.undispellable {
        return false;
    }
    match effect.dispel_type {
        DispelType::None => false,
        DispelType::Basic => true,
        DispelType::Strong => matches!(strength, DispelStrength::Strong),
    }
}

#[derive(Debug, Clone)]
pub enum StatusChange {
    Apply(StatusEffect),
    Dispel {
        kind: StatusKind,
        strength: DispelStrength,
    },
    Remove(&'static str),
    /// One of the target's basic attacks was released: every live status with attack
    /// charges loses one, and ends (as `Expired`) when it has none left.
    ConsumeAttackCharge,
}

/// Single entry point for adding/removing statuses so debuff immunity and stat
/// modifiers are handled consistently.
#[derive(Message, Debug, Clone)]
pub struct StatusEffectEvent {
    #[allow(dead_code)]
    pub source: Option<Entity>,
    pub target: Entity,
    pub change: StatusChange,
    /// Caused by a reaction or a held condition: the change fires no status triggers.
    pub reactive: bool,
}

impl StatusEffectEvent {
    pub fn new(source: Option<Entity>, target: Entity, change: StatusChange) -> Self {
        Self {
            source,
            target,
            change,
            reactive: false,
        }
    }

    pub fn reactive(mut self, reactive: bool) -> Self {
        self.reactive = reactive;
        self
    }
}

/// What happened to a status, as reported to the reactive-trigger layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLifecycle {
    /// Applied, or refreshed by a same-id status.
    Applied,
    /// Ran out.
    Expired,
    /// Purged by a dispel or by gaining debuff immunity.
    Dispelled,
    /// Removed by id, or broken (sleep ends on damage).
    Removed,
}

/// A status appeared on or left `target`. Written by [`apply_status_effect_events`],
/// [`tick_status_effects`] and the sleep break in `combat::apply_damage_events` (the only
/// places statuses change). Never written for reactive changes or reactive statuses, so a
/// reaction cannot trigger another through the status layer.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusLifecycleEvent {
    pub target: Entity,
    pub source: Option<Entity>,
    pub id: &'static str,
    pub kind: StatusKind,
    pub change: StatusLifecycle,
}

/// A status taken off a unit by [`remove_statuses`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct RemovedStatus {
    pub id: &'static str,
    pub kind: StatusKind,
    pub reactive: bool,
}

impl RemovedStatus {
    pub(crate) fn of(effect: &StatusEffect) -> Self {
        Self {
            id: effect.id,
            kind: effect.kind,
            reactive: effect.reactive,
        }
    }
}

/// Report removed statuses (skipping reactive ones and everything under a reactive cause).
pub(crate) fn report_removed(
    lifecycle: &mut MessageWriter<StatusLifecycleEvent>,
    target: Entity,
    source: Option<Entity>,
    cause_is_reactive: bool,
    change: StatusLifecycle,
    removed: impl IntoIterator<Item = RemovedStatus>,
) {
    if cause_is_reactive {
        return;
    }
    for status in removed.into_iter().filter(|status| !status.reactive) {
        lifecycle.write(StatusLifecycleEvent {
            target,
            source,
            id: status.id,
            kind: status.kind,
            change,
        });
    }
}

/// Slack when comparing a ticking burn's accumulated time with its interval (float drift).
const DOT_TICK_EPSILON: f32 = 1e-4;

pub fn tick_status_effects(
    time: Res<Time>,
    mut damage: MessageWriter<DamageEvent>,
    mut lifecycle: MessageWriter<StatusLifecycleEvent>,
    mut units: Query<(
        Entity,
        &mut StatusEffects,
        &mut CombatStats,
        &mut Health,
        Option<&mut ShieldPool>,
        Option<&HeroAttributes>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut statuses, mut stats, mut health, mut shield, attrs) in &mut units {
        let agi = attrs.map(|a| a.agility).unwrap_or(0.0);
        let hot_factor = crate::vitals::heal_factor(
            crate::vitals::HealKind::OverTime,
            0.0,
            Some(&*stats),
            Some(&*statuses),
        );
        let mut i = 0;
        while i < statuses.effects.len() {
            let effect = &mut statuses.effects[i];
            if effect.heal_per_sec > 0.0 {
                // The healer's amp was folded into `heal_per_sec` when the status was applied.
                crate::vitals::apply_heal(&mut health, effect.heal_per_sec * dt, hot_factor);
            }
            if effect.damage_per_sec > 0.0 {
                damage.write(DamageEvent::dot(
                    effect.dot_source,
                    entity,
                    effect.damage_per_sec * dt,
                    effect.dot_damage_type,
                ));
            }
            if effect.dot_interval > 0.0 {
                effect.dot_timer += dt;
                while effect.dot_ticks_left > 0
                    && effect.dot_timer + DOT_TICK_EPSILON >= effect.dot_interval
                {
                    effect.dot_timer -= effect.dot_interval;
                    effect.dot_ticks_left -= 1;
                    damage.write(DamageEvent::dot(
                        effect.dot_source,
                        entity,
                        effect.dot_per_tick,
                        effect.dot_damage_type,
                    ));
                }
            }
            effect.remaining -= dt;
            if effect.remaining <= 0.0 {
                // Natural expiry flushes owed ticks so the total is exact at any frame rate.
                for _ in 0..std::mem::take(&mut effect.dot_ticks_left) {
                    damage.write(DamageEvent::dot(
                        effect.dot_source,
                        entity,
                        effect.dot_per_tick,
                        effect.dot_damage_type,
                    ));
                }
                let expired = statuses.effects.remove(i);
                subtract_status_stat_delta(&expired, &mut stats, agi);
                release_shield(&expired, shield.as_deref_mut());
                report_removed(
                    &mut lifecycle,
                    entity,
                    None,
                    false,
                    StatusLifecycle::Expired,
                    [RemovedStatus::of(&expired)],
                );
            } else {
                i += 1;
            }
        }
    }
}

/// Apply a phased buff (no combat-stat mutation).
#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_phased(statuses: &mut StatusEffects, duration: f32) {
    statuses.effects.retain(|e| e.id != "phased");
    statuses.effects.push(StatusEffect::phased(duration));
}

/// Apply a forceful buff so this unit claims more separation against overlaps.
#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_forceful(statuses: &mut StatusEffects, duration: f32) {
    statuses.effects.retain(|e| e.id != "forceful");
    statuses.effects.push(StatusEffect::forceful(duration));
}

pub(crate) fn subtract_status_stat_delta(
    effect: &StatusEffect,
    stats: &mut CombatStats,
    agility: f32,
) {
    stats.attack_damage -= effect.attack_damage;
    stats.armor -= effect.armor;
    stats.magic_resist -= effect.magic_resist;
    stats.move_speed -= effect.move_speed;
    stats.attack_speed_flat -= effect.attack_speed_flat;
    stats.attack_speed_mult -= effect.attack_speed_mult;
    stats.attack_range -= effect.attack_range;
    stats.attack_range_bonus -= effect.attack_range;
    if effect.carries_cleave() {
        stats.cleaves.adjust(
            effect.cleave_key(),
            -effect.cleave,
            -effect.cleave_radius,
            -effect.cleave_end_radius,
            -effect.cleave_distance,
            -1,
        );
    }
    stats.recompute_attack_speed(agility);
}

pub(crate) fn add_status_stat_delta(effect: &StatusEffect, stats: &mut CombatStats, agility: f32) {
    stats.attack_damage += effect.attack_damage;
    stats.armor += effect.armor;
    stats.magic_resist += effect.magic_resist;
    stats.move_speed += effect.move_speed;
    stats.attack_speed_flat += effect.attack_speed_flat;
    stats.attack_speed_mult += effect.attack_speed_mult;
    stats.attack_range += effect.attack_range;
    stats.attack_range_bonus += effect.attack_range;
    if effect.carries_cleave() {
        stats.cleaves.adjust(
            effect.cleave_key(),
            effect.cleave,
            effect.cleave_radius,
            effect.cleave_end_radius,
            effect.cleave_distance,
            1,
        );
    }
    stats.recompute_attack_speed(agility);
}

/// Apply a timed buff/debuff and immediately add its flat modifiers to combat stats.
/// Debuffs are ignored while the unit has debuff immunity.
pub fn apply_status(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    mut effect: StatusEffect,
    agility: f32,
) -> bool {
    if effect.kind == StatusKind::Debuff && statuses.has_debuff_immunity() {
        return false;
    }
    // Refresh same-id effects by replacing, unless each application is its own instance.
    if !effect.independent
        && let Some(existing) = statuses.effects.iter().position(|e| e.id == effect.id)
    {
        let old = statuses.effects.remove(existing);
        subtract_status_stat_delta(&old, stats, agility);
        if effect.stacks > 0 {
            // A stacking status keeps what it had and refreshes its duration.
            effect.stacks += old.stacks;
        }
    }
    if effect.move_slow_fraction > 0.0 || effect.move_bonus_fraction > 0.0 {
        // Measure the slow or bonus against the speed without status deltas, after the
        // refreshed status is gone, so refreshing never shrinks, removes or compounds it.
        let status_speed: f32 = statuses.effects.iter().map(|e| e.move_speed).sum();
        let base = (stats.move_speed - status_speed).max(0.0);
        effect.move_speed = (effect.move_bonus_fraction - effect.move_slow_fraction) * base;
    }
    add_status_stat_delta(&effect, stats, agility);
    statuses.effects.push(effect);
    true
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_stun(statuses: &mut StatusEffects, stats: &mut CombatStats, duration: f32) -> bool {
    apply_status(statuses, stats, StatusEffect::stunned(duration), 0.0)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_debuff_immunity(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    duration: f32,
    agility: f32,
) -> bool {
    // Purge existing debuffs when gaining immunity — strong dispel.
    purge_strong_dispellable_debuffs(statuses, stats, agility, None);
    apply_status(
        statuses,
        stats,
        StatusEffect::debuff_immunity(duration),
        agility,
    )
}

/// Charge `absorbed` points of damage the [`ShieldPool`] soaked against the shield statuses
/// that back it, so each status's `shield_amount` stays what is left of that shield.
///
/// Shields are consumed oldest first: the walk follows `statuses.effects` in vector order,
/// which is application order (a refresh removes and re-pushes, so a refreshed shield becomes
/// the newest). Each shield gives up what it has before the next one is touched.
pub fn consume_shields(statuses: &mut StatusEffects, mut absorbed: f32) {
    for effect in &mut statuses.effects {
        if absorbed <= 0.0 {
            break;
        }
        if effect.shield_amount <= 0.0 {
            continue;
        }
        let taken = effect.shield_amount.min(absorbed);
        effect.shield_amount -= taken;
        absorbed -= taken;
    }
}

/// Take the remaining part of a shield status back out of the pool (expiry, dispel, remove).
/// Because of [`consume_shields`], `shield_amount` is what is left of that shield.
fn release_shield(effect: &StatusEffect, pool: Option<&mut ShieldPool>) {
    if effect.shield_amount > 0.0 {
        if let Some(pool) = pool {
            pool.amount = (pool.amount - effect.shield_amount).max(0.0);
        }
    }
}

fn remove_statuses(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    agility: f32,
    mut pool: Option<&mut ShieldPool>,
    matches: impl Fn(&StatusEffect) -> bool,
) -> Vec<RemovedStatus> {
    let mut removed = Vec::new();
    statuses.effects.retain(|effect| {
        if !matches(effect) {
            return true;
        }
        subtract_status_stat_delta(effect, stats, agility);
        release_shield(effect, pool.as_deref_mut());
        removed.push(RemovedStatus::of(effect));
        false
    });
    removed
}

/// Gaining debuff immunity purges every debuff a strong dispel would remove.
fn purge_strong_dispellable_debuffs(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    agility: f32,
    pool: Option<&mut ShieldPool>,
) -> Vec<RemovedStatus> {
    remove_statuses(statuses, stats, agility, pool, |e| {
        e.kind == StatusKind::Debuff
            && status_dispel_matches(e, StatusKind::Debuff, DispelStrength::Strong)
    })
}

/// Damage breaks sleep: remove every active sleep status and revert its stat modifiers.
pub fn wake_from_sleep(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    agility: f32,
    pool: Option<&mut ShieldPool>,
) -> Vec<RemovedStatus> {
    if !statuses.is_asleep() {
        return Vec::new();
    }
    remove_statuses(statuses, stats, agility, pool, |e| {
        e.asleep && e.remaining > 0.0
    })
}

/// Whether an effect from `caster` is hostile to `target`: it is aimed at another unit that is
/// not on the caster's team (units without a team are always fair game).
pub fn hostile(
    caster: Entity,
    target: Entity,
    caster_team: Option<Team>,
    target_team: Option<Team>,
) -> bool {
    caster != target && (target_team != caster_team || target_team.is_none())
}

/// Spend one attack charge per released basic attack (extra attacks never spend one).
/// Only writes [`StatusEffectEvent`]s; the consumption itself is in
/// [`apply_status_effect_events`].
pub fn consume_attack_charges(
    mut releases: MessageReader<crate::basic_attack::BasicAttackReleaseEvent>,
    holders: Query<&StatusEffects>,
    mut changes: MessageWriter<StatusEffectEvent>,
) {
    for release in releases.read().filter(|r| !r.extra) {
        if holders
            .get(release.attacker)
            .is_ok_and(|s| s.effects.iter().any(|e| e.attack_charges > 0))
        {
            changes.write(StatusEffectEvent::new(
                Some(release.attacker),
                release.attacker,
                StatusChange::ConsumeAttackCharge,
            ));
        }
    }
}

/// An invisible holder that attacks (non-extra release) or fires an activation loses every
/// `invis_breaks_on_action` invisibility. Writes [`StatusChange::Remove`] events only.
pub fn break_invisibility(
    mut releases: MessageReader<crate::basic_attack::BasicAttackReleaseEvent>,
    mut fired: MessageReader<crate::actions::ActionFiredEvent>,
    holders: Query<&StatusEffects>,
    mut changes: MessageWriter<StatusEffectEvent>,
) {
    let actors: Vec<Entity> = releases
        .read()
        .filter(|r| !r.extra)
        .map(|r| r.attacker)
        .chain(fired.read().map(|f| f.caster))
        .collect();
    let mut done: Vec<Entity> = Vec::new();
    for actor in actors {
        if done.contains(&actor) {
            continue;
        }
        let Ok(statuses) = holders.get(actor) else {
            continue;
        };
        let mut ids: Vec<&'static str> = statuses
            .effects
            .iter()
            .filter(|e| e.invisible && e.invis_breaks_on_action && e.remaining > 0.0)
            .map(|e| e.id)
            .collect();
        ids.dedup();
        if ids.is_empty() {
            continue;
        }
        done.push(actor);
        for id in ids {
            changes.write(StatusEffectEvent::new(
                Some(actor),
                actor,
                StatusChange::Remove(id),
            ));
        }
    }
}

pub fn apply_status_effect_events(
    mut commands: Commands,
    mut events: MessageReader<StatusEffectEvent>,
    mut lifecycle: MessageWriter<StatusLifecycleEvent>,
    mut units: Query<(
        Entity,
        &mut StatusEffects,
        &mut CombatStats,
        Option<&mut ShieldPool>,
        Option<&HeroAttributes>,
    )>,
    teams: Query<&Team>,
) {
    for event in events.read() {
        let Ok((entity, mut statuses, mut stats, mut shield, attrs)) = units.get_mut(event.target)
        else {
            continue;
        };
        let agility = attrs.map(|a| a.agility).unwrap_or(0.0);
        match &event.change {
            StatusChange::Apply(effect) => {
                // Status resistance shortens finite debuffs that another unit puts on this
                // one. Only hostile sources count: self-applied, ally-applied, reactive/held and permanent statuses are left alone.
                let mut scaled;
                let effect = if effect.kind == StatusKind::Debuff
                    && !event.reactive
                    && !effect.reactive
                    && effect.remaining.is_finite()
                    && event.source.is_some_and(|source| {
                        hostile(
                            source,
                            entity,
                            teams.get(source).ok().copied(),
                            teams.get(entity).ok().copied(),
                        )
                    })
                    && statuses.status_resistance_factor() < 1.0
                {
                    scaled = effect.clone();
                    scaled.scale_duration(statuses.status_resistance_factor());
                    &scaled
                } else {
                    effect
                };
                if effect.debuff_immune {
                    let purged = purge_strong_dispellable_debuffs(
                        &mut statuses,
                        &mut stats,
                        agility,
                        shield.as_deref_mut(),
                    );
                    report_removed(
                        &mut lifecycle,
                        entity,
                        event.source,
                        event.reactive,
                        StatusLifecycle::Dispelled,
                        purged,
                    );
                }
                let refresh_shield = effect.shield_amount > 0.0;
                let old_shield = if refresh_shield {
                    statuses
                        .effects
                        .iter()
                        .find(|e| e.id == effect.id)
                        .map(|e| e.shield_amount)
                        .unwrap_or(0.0)
                } else {
                    0.0
                };
                let applied = apply_status(&mut statuses, &mut stats, effect.clone(), agility);
                if applied && !event.reactive && !effect.reactive {
                    lifecycle.write(StatusLifecycleEvent {
                        target: entity,
                        source: event.source,
                        id: effect.id,
                        kind: effect.kind,
                        change: StatusLifecycle::Applied,
                    });
                }
                if applied && effect.flying {
                    // When the flight ends, check the ground it lands on.
                    commands
                        .entity(entity)
                        .insert(crate::displacement::SettleOnLanding);
                }
                if applied && effect.shield_amount > 0.0 {
                    if let Some(pool) = shield.as_deref_mut() {
                        pool.amount = (pool.amount - old_shield).max(0.0) + effect.shield_amount;
                    } else {
                        commands.entity(entity).insert(ShieldPool {
                            amount: effect.shield_amount,
                        });
                    }
                }
            }
            StatusChange::Dispel { kind, strength } => {
                let removed = remove_statuses(
                    &mut statuses,
                    &mut stats,
                    agility,
                    shield.as_deref_mut(),
                    |e| status_dispel_matches(e, *kind, *strength),
                );
                report_removed(
                    &mut lifecycle,
                    entity,
                    event.source,
                    event.reactive,
                    StatusLifecycle::Dispelled,
                    removed,
                );
            }
            StatusChange::ConsumeAttackCharge => {
                let mut spent: Vec<&'static str> = Vec::new();
                for effect in statuses.effects.iter_mut().filter(|e| e.attack_charges > 0) {
                    effect.attack_charges -= 1;
                    if effect.attack_charges == 0 {
                        spent.push(effect.id);
                    }
                }
                if spent.is_empty() {
                    continue;
                }
                let removed = remove_statuses(
                    &mut statuses,
                    &mut stats,
                    agility,
                    shield.as_deref_mut(),
                    |e| e.attack_charges == 0 && spent.contains(&e.id),
                );
                report_removed(
                    &mut lifecycle,
                    entity,
                    event.source,
                    event.reactive,
                    StatusLifecycle::Expired,
                    removed,
                );
            }
            StatusChange::Remove(id) => {
                let removed = remove_statuses(
                    &mut statuses,
                    &mut stats,
                    agility,
                    shield.as_deref_mut(),
                    |e| e.id == *id,
                );
                report_removed(
                    &mut lifecycle,
                    entity,
                    event.source,
                    event.reactive,
                    StatusLifecycle::Removed,
                    removed,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::definition::{RankValue, StatusBuildContext, StatusSpec};
    use crate::components::CleaveSource;
    use bevy::ecs::system::RunSystemOnce;

    fn live(effect: StatusEffect) -> StatusEffects {
        StatusEffects {
            effects: vec![effect],
        }
    }

    #[test]
    fn cyclone_untargetable_via_field() {
        let cyclone = live(StatusEffect::cyclone(2.0, true));
        assert!(cyclone.effects[0].untargetable);
        assert!(cyclone.is_untargetable());
        for intent in [
            TargetIntent::Any,
            TargetIntent::Attack,
            TargetIntent::HostileSpell,
        ] {
            assert!(cyclone.rejects(intent), "{intent:?}");
        }
        // The check reads the field, not the status id.
        let by_field = live(StatusEffect {
            untargetable: true,
            ..StatusEffect::buff("anything", 2.0)
        });
        assert!(by_field.is_untargetable());
        let by_id = live(StatusEffect {
            untargetable: false,
            ..StatusEffect::cyclone(2.0, true)
        });
        assert!(!by_id.is_untargetable());
        let mut expired = StatusEffect::cyclone(2.0, true);
        expired.remaining = 0.0;
        assert!(!live(expired).is_untargetable());
    }

    #[test]
    fn targeting_only_untargetable_rejects_attacks_and_enemy_spells() {
        let dance = live(StatusEffect::untargetable(2.0));
        assert!(dance.effects[0].targeting_blocked);
        assert!(dance.rejects(TargetIntent::Attack));
        assert!(dance.rejects(TargetIntent::HostileSpell));
        assert!(!dance.rejects(TargetIntent::Any), "areas still hit it");
        // It is not the full flag, which dash contact and homing projectiles read.
        assert!(!dance.is_untargetable());
        assert!(!dance.effects[0].untargetable);
        // It ends with the status.
        let mut expired = StatusEffect::untargetable(2.0);
        expired.remaining = 0.0;
        assert!(!live(expired).rejects(TargetIntent::Attack));
    }

    #[test]
    fn cyclone_still_fully_untargetable() {
        let cyclone = live(StatusEffect::cyclone(2.0, true));
        assert!(cyclone.is_untargetable());
        assert!(!cyclone.effects[0].targeting_blocked);
        assert!(cyclone.rejects(TargetIntent::Any));
        let banish = live(StatusEffect::banish(2.0, true));
        assert!(banish.is_untargetable());
        assert!(banish.rejects(TargetIntent::Any));
    }

    #[test]
    fn target_intent_rejects_by_immunity() {
        let attack = live(StatusEffect::attack_immunity(2.0));
        assert!(attack.rejects(TargetIntent::Attack));
        assert!(!attack.rejects(TargetIntent::Any));
        assert!(!attack.rejects(TargetIntent::HostileSpell));
        let spell = live(StatusEffect::spell_immunity(2.0));
        assert!(spell.rejects(TargetIntent::HostileSpell));
        assert!(!spell.rejects(TargetIntent::Any));
        assert!(!spell.rejects(TargetIntent::Attack));
        // Ethereal is attack immune, not spell immune.
        let ethereal = live(StatusEffect::ethereal(2.0, 40.0, true));
        assert!(ethereal.rejects(TargetIntent::Attack));
        assert!(!ethereal.rejects(TargetIntent::HostileSpell));
    }

    #[test]
    fn damage_mask_contains() {
        use DamageTypeMask as M;
        assert!(M::ALL.contains(M::PHYSICAL) && M::ALL.contains(M::MAGICAL));
        assert!(M::ALL.contains(M::PURE));
        assert!(!M::PHYSICAL.contains(M::MAGICAL));
        assert!(M::NONE.is_empty() && !M::NONE.contains(M::PHYSICAL));
        assert!(M::PHYSICAL.union(M::MAGICAL).contains(M::MAGICAL));
        assert_eq!(M::of(DamageType::Pure), M::PURE);
        let ethereal = live(StatusEffect::ethereal(2.0, 40.0, true));
        assert!(ethereal.is_immune_to(DamageType::Physical));
        assert!(!ethereal.is_immune_to(DamageType::Magical));
        assert!(!ethereal.is_immune_to(DamageType::Pure));
    }

    #[test]
    fn spell_immunity_rejects_debuffs_and_magic_only() {
        let mut stats = CombatStats::simple(10.0, 100.0, 1.0, 0.0, 0.0, 300.0);
        let mut statuses = StatusEffects::default();
        assert!(apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect::spell_immunity(5.0),
            0.0
        ));
        let immunity = &statuses.effects[0];
        assert_eq!(immunity.id, "spell_immunity");
        assert!(immunity.undispellable && immunity.kind == StatusKind::Buff);
        assert!(!apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect::stunned(2.0),
            0.0
        ));
        assert!(statuses.is_immune_to(DamageType::Magical));
        assert!(!statuses.is_immune_to(DamageType::Physical));
        assert!(!statuses.is_immune_to(DamageType::Pure));
        // Debuff immunity is a different status: no spell or damage immunity.
        let plain = live(StatusEffect::debuff_immunity(5.0));
        assert_eq!(plain.effects[0].id, "debuff_immunity");
        assert!(plain.has_debuff_immunity());
        assert!(!plain.rejects(TargetIntent::HostileSpell));
        assert!(!plain.is_immune_to(DamageType::Magical));
    }

    #[test]
    fn banished_unit_is_untargetable_invulnerable_and_phased() {
        let banished = live(StatusEffect::banish(3.0, true));
        assert!(banished.is_untargetable() && banished.is_invulnerable());
        assert!(banished.is_phased() && banished.is_hidden());
        assert!(banished.is_stunned());
        assert!(
            banished.blocks().contains(
                ActionBlocks::MOVE
                    .union(ActionBlocks::ATTACK)
                    .union(ActionBlocks::CAST)
                    .union(ActionBlocks::ITEMS)
            )
        );
        let effect = &banished.effects[0];
        assert!(effect.undispellable && effect.dispel_type == DispelType::None);
        assert!(!status_dispel_matches(
            effect,
            StatusKind::Debuff,
            DispelStrength::Strong
        ));
        assert_eq!(effect.kind, StatusKind::Debuff);
        assert_eq!(StatusEffect::banish(3.0, false).kind, StatusKind::Buff);
    }

    #[test]
    fn blocks_matrix_matches_documented_table() {
        use ActionBlocks as B;
        let all = B::MOVE.union(B::ATTACK).union(B::CAST).union(B::ITEMS);
        let src = Entity::PLACEHOLDER;
        let compelled = B::ATTACK.union(B::CAST).union(B::ITEMS).union(B::ORDERS);
        let cases: [(StatusEffect, B); 12] = [
            (StatusEffect::stunned(1.0), all),
            (StatusEffect::sleep(1.0), all),
            (StatusEffect::rooted(1.0), B::MOVE),
            (StatusEffect::silenced(1.0), B::CAST),
            (StatusEffect::mute(1.0), B::ITEMS),
            (StatusEffect::disarmed(1.0), B::ATTACK),
            (
                StatusEffect::hexed(1.0),
                B::CAST.union(B::ITEMS).union(B::ATTACK).union(B::PASSIVES),
            ),
            (StatusEffect::broken(1.0), B::PASSIVES),
            (StatusEffect::fear(1.0, Some(src)), compelled),
            (StatusEffect::hypnotized(1.0, src), compelled),
            (
                StatusEffect::taunted(1.0, src),
                B::CAST.union(B::ITEMS).union(B::ORDERS),
            ),
            (StatusEffect::buff("plain", 1.0), B::NONE),
        ];
        for (effect, expected) in cases {
            assert_eq!(effect.blocks(), expected, "{}", effect.id);
            let live = StatusEffects {
                effects: vec![effect.clone()],
            };
            assert_eq!(live.blocks(), expected, "{}", effect.id);
            assert_eq!(
                live.is_rooted(),
                expected.contains(B::MOVE),
                "{}",
                effect.id
            );
            assert_eq!(
                live.is_disarmed(),
                expected.contains(B::ATTACK),
                "{}",
                effect.id
            );
            assert_eq!(
                live.can_cast(),
                !expected.contains(B::CAST),
                "{}",
                effect.id
            );
            assert_eq!(
                live.blocks_items(),
                expected.contains(B::ITEMS),
                "{}",
                effect.id
            );
        }
        // Stun and sleep leave orders and passives alone; expired effects block nothing; effects
        // union.
        assert!(!all.contains(B::ORDERS) && !all.contains(B::PASSIVES));
        let mut expired = StatusEffect::stunned(1.0);
        expired.remaining = 0.0;
        let mixed = StatusEffects {
            effects: vec![expired, StatusEffect::rooted(1.0), StatusEffect::mute(1.0)],
        };
        assert_eq!(mixed.blocks(), B::MOVE.union(B::ITEMS));
    }

    #[test]
    fn newest_live_compulsion_wins_and_expired_ones_are_ignored() {
        let (a, b) = (
            Entity::from_raw_u32(1).unwrap(),
            Entity::from_raw_u32(2).unwrap(),
        );
        assert_eq!(StatusEffects::default().compulsion(), None);
        let mut expired = StatusEffect::fear(1.0, Some(a));
        expired.remaining = 0.0;
        let statuses = StatusEffects {
            effects: vec![
                StatusEffect::fear(1.0, Some(a)),
                StatusEffect::taunted(1.0, b),
                expired,
            ],
        };
        assert_eq!(
            statuses.compulsion(),
            Some(Compulsion::Attack { target: b }),
            "the later live one wins"
        );
        assert_eq!(
            StatusEffect::hypnotized(1.0, a).compulsion,
            Some(Compulsion::Approach { toward: a })
        );
    }

    #[test]
    fn hexed_unit_capped_move_speed_and_broken() {
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 500.0);
        let none = StatusEffects::default();
        assert_eq!(stats.effective_move_speed(Some(&none)), 500.0);

        let hexed = StatusEffects {
            effects: vec![StatusEffect::hexed(2.0)],
        };
        assert!(hexed.breaks_passives());
        assert_eq!(hexed.move_speed_cap(), Some(crate::scale::HEX_MOVE_SPEED));
        assert_eq!(
            stats.effective_move_speed(Some(&hexed)),
            crate::scale::HEX_MOVE_SPEED
        );

        // A haste bonus does not lift the cap; the lowest cap wins; a slower unit is not sped up.
        let haste = StatusEffect {
            move_speed: 200.0,
            ..StatusEffect::buff("haste", 5.0)
        };
        stats.move_speed += 200.0;
        let both = StatusEffects {
            effects: vec![haste, StatusEffect::hexed(2.0)],
        };
        assert_eq!(
            stats.effective_move_speed(Some(&both)),
            crate::scale::HEX_MOVE_SPEED
        );
        let tighter = StatusEffects {
            effects: vec![
                StatusEffect {
                    move_speed_cap: 90.0,
                    ..StatusEffect::debuff("tight", 2.0)
                },
                StatusEffect::hexed(2.0),
            ],
        };
        assert_eq!(tighter.move_speed_cap(), Some(90.0));
        let slow_unit = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 120.0);
        assert_eq!(slow_unit.effective_move_speed(Some(&hexed)), 120.0);

        // An expired hex caps nothing and breaks nothing.
        let mut gone = StatusEffect::hexed(2.0);
        gone.remaining = 0.0;
        let expired = StatusEffects {
            effects: vec![gone],
        };
        assert_eq!(expired.move_speed_cap(), None);
        assert!(!expired.breaks_passives());
        // Break alone neither caps nor disables anything else.
        let broken = StatusEffects {
            effects: vec![StatusEffect::broken(2.0)],
        };
        assert!(broken.breaks_passives() && broken.can_cast() && broken.can_attack());
        assert_eq!(broken.move_speed_cap(), None);
    }

    #[test]
    fn sleep_blocks_items() {
        let asleep = StatusEffects {
            effects: vec![StatusEffect::sleep(2.0)],
        };
        assert!(asleep.blocks_items());
        assert!(crate::actions::DisableRule::MuteOrStun.blocks(Some(&asleep)));
        let silenced = StatusEffects {
            effects: vec![StatusEffect::silenced(2.0)],
        };
        assert!(!crate::actions::DisableRule::MuteOrStun.blocks(Some(&silenced)));
    }

    #[test]
    fn status_buff_applies_and_expires_modifiers() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 10.0);
        apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect {
                armor: 8.0,
                ..StatusEffect::buff("test", 1.0)
            },
            0.0,
        );
        assert_eq!(stats.armor, 10.0);
        assert_eq!(statuses.effects.len(), 1);
        statuses.effects[0].remaining = 0.0;
        // Simulate expire path
        let expired = statuses.effects.remove(0);
        stats.armor -= expired.armor;
        assert_eq!(stats.armor, 2.0);
    }

    fn percent_slow(
        id: &'static str,
        percent: f32,
        duration: f32,
        current_speed: f32,
    ) -> StatusEffect {
        let built = StatusSpec::Slow {
            id: "slow",
            percent: RankValue::fixed(percent),
        }
        .build(
            1,
            duration,
            StatusBuildContext {
                move_speed: current_speed,
                agility: 0.0,
                source: None,
                point: Vec3::ZERO,
                base_attack_damage: 0.0,
            },
        );
        StatusEffect { id, ..built }
    }

    fn percent_bonus(id: &'static str, percent: f32, duration: f32, speed: f32) -> StatusEffect {
        let built = StatusSpec::MoveSpeedBonus {
            id,
            percent: RankValue::fixed(percent),
        }
        .build(
            1,
            duration,
            StatusBuildContext {
                move_speed: speed,
                agility: 0.0,
                source: None,
                point: Vec3::ZERO,
                base_attack_damage: 0.0,
            },
        );
        assert_eq!(built.id, id);
        built
    }

    #[test]
    fn move_speed_bonus_is_percent_of_base_and_reverts() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 300.0);
        let bonus = percent_bonus("dance_move_speed_bonus", 50.0, 4.0, stats.move_speed);
        assert_eq!(bonus.kind, StatusKind::Buff);
        assert!((bonus.move_speed - 150.0).abs() < 1e-4);
        apply_status(&mut statuses, &mut stats, bonus, 0.0);
        assert!((stats.move_speed - 450.0).abs() < 1e-4);
        assert!((stats.effective_move_speed(Some(&statuses)) - 450.0).abs() < 1e-4);
        let expired = statuses.effects.remove(0);
        subtract_status_stat_delta(&expired, &mut stats, 0.0);
        assert!((stats.move_speed - 300.0).abs() < 1e-4);
    }

    #[test]
    fn refreshed_bonus_does_not_compound() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 300.0);
        for _ in 0..10 {
            // Built against the already-hasted speed, as build_status_effect does.
            let bonus = percent_bonus("dance_move_speed_bonus", 20.0, 4.0, stats.move_speed);
            apply_status(&mut statuses, &mut stats, bonus, 0.0);
        }
        assert!((stats.move_speed - 360.0).abs() < 1e-3);
        assert_eq!(statuses.effects.len(), 1);
    }

    #[test]
    fn bonus_and_slow_from_two_ids_add() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 300.0);
        let slow = percent_slow("slow", 30.0, 5.0, stats.move_speed);
        apply_status(&mut statuses, &mut stats, slow, 0.0);
        let bonus = percent_bonus("dance_move_speed_bonus", 50.0, 5.0, stats.move_speed);
        apply_status(&mut statuses, &mut stats, bonus, 0.0);
        // +50% and -30% of the 300 base: 300 + 150 - 90.
        assert!(
            (stats.move_speed - 360.0).abs() < 1e-3,
            "{}",
            stats.move_speed
        );
        assert_eq!(statuses.effects.len(), 2);
        // Refreshing the slow keeps both measured against base.
        let slow = percent_slow("slow", 30.0, 5.0, stats.move_speed);
        apply_status(&mut statuses, &mut stats, slow, 0.0);
        assert!(
            (stats.move_speed - 360.0).abs() < 1e-3,
            "{}",
            stats.move_speed
        );
    }

    #[test]
    fn first_slow_application_is_unchanged() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 300.0);
        let slow = percent_slow("slow", 30.0, 2.0, stats.move_speed);
        assert!((slow.move_speed + 90.0).abs() < 1e-4);
        apply_status(&mut statuses, &mut stats, slow, 0.0);
        assert!((stats.move_speed - 210.0).abs() < 1e-4);
    }

    #[test]
    fn refreshed_slow_stays_at_its_percent_of_base_speed() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 300.0);
        for _ in 0..10 {
            // Built against the already-slowed speed, as build_status_effect does.
            let slow = percent_slow("slow", 30.0, 2.0, stats.move_speed);
            apply_status(&mut statuses, &mut stats, slow, 0.0);
        }
        assert!((stats.move_speed - 210.0).abs() < 1e-3);
        assert_eq!(statuses.effects.len(), 1);
    }

    #[test]
    fn refreshed_full_slow_stays_a_full_slow() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 300.0);
        for _ in 0..3 {
            let slow = percent_slow("slow", 100.0, 2.0, stats.move_speed);
            apply_status(&mut statuses, &mut stats, slow, 0.0);
        }
        assert!(stats.move_speed.abs() < 1e-3);
        assert_eq!(
            stats.effective_move_speed(Some(&statuses)),
            crate::scale::MIN_MOVE_SPEED
        );
    }

    #[test]
    fn slows_from_two_ids_add_floor_and_expire_back_to_base() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 300.0);
        let full = percent_slow("a_slow", 100.0, 1.0, stats.move_speed);
        apply_status(&mut statuses, &mut stats, full, 0.0);
        let partial = percent_slow("b_slow", 48.0, 2.0, stats.move_speed);
        apply_status(&mut statuses, &mut stats, partial, 0.0);
        assert_eq!(
            stats.effective_move_speed(Some(&statuses)),
            crate::scale::MIN_MOVE_SPEED,
            "stacked slows floor at the minimum speed"
        );
        // Refreshing the partial slow while the full one is active keeps both at base-relative values.
        let partial = percent_slow("b_slow", 48.0, 2.0, stats.move_speed);
        apply_status(&mut statuses, &mut stats, partial, 0.0);
        assert!((stats.move_speed - (300.0 - 300.0 - 144.0)).abs() < 1e-3);
        for effect in std::mem::take(&mut statuses.effects) {
            subtract_status_stat_delta(&effect, &mut stats, 0.0);
        }
        assert!(
            (stats.move_speed - 300.0).abs() < 1e-3,
            "both expire back to base"
        );
    }

    #[test]
    fn sleep_wake_releases_stat_deltas() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 100.0);
        let baseline = stats;
        apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect {
                move_speed: -40.0,
                armor: -3.0,
                ..StatusEffect::sleep(5.0)
            },
            0.0,
        );
        apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect {
                armor: 4.0,
                ..StatusEffect::buff("keep", 5.0)
            },
            0.0,
        );
        assert_eq!(stats.move_speed, 60.0);
        assert!(statuses.is_asleep());

        let woken = wake_from_sleep(&mut statuses, &mut stats, 0.0, None);
        assert_eq!(
            woken.iter().map(|r| r.id).collect::<Vec<_>>(),
            ["sleep"],
            "the removed ids are returned for reporting"
        );
        assert!(!woken[0].reactive);

        assert!(!statuses.is_asleep());
        assert_eq!(stats.move_speed, baseline.move_speed);
        assert_eq!(stats.armor, baseline.armor + 4.0, "only sleep is released");
        assert_eq!(statuses.effects.len(), 1);
        // Waking a unit that is not asleep changes nothing and reports nothing.
        assert!(wake_from_sleep(&mut statuses, &mut stats, 0.0, None).is_empty());
        assert_eq!(statuses.effects.len(), 1);
    }

    #[test]
    fn hostile_means_another_unit_not_on_the_casters_team() {
        let mut world = World::new();
        let (a, b) = (world.spawn_empty().id(), world.spawn_empty().id());
        assert!(!hostile(a, a, Some(Team::Radiant), Some(Team::Radiant)));
        assert!(!hostile(a, b, Some(Team::Radiant), Some(Team::Radiant)));
        assert!(hostile(a, b, Some(Team::Radiant), Some(Team::Dire)));
        assert!(hostile(a, b, Some(Team::Radiant), None));
        assert!(hostile(a, b, None, None));
    }

    #[test]
    fn attack_range_modifier_status_reverts_on_expire() {
        use std::time::Duration;
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<Messages<StatusLifecycleEvent>>();
        let mut statuses = StatusEffects::default();
        let mut stats =
            CombatStats::simple(10.0, crate::scale::MELEE_ATTACK_RANGE, 1.0, 0.0, 0.0, 10.0);
        let base = stats.attack_range;
        apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect {
                attack_range: 120.0,
                ..StatusEffect::buff("long_reach", 1.0)
            },
            0.0,
        );
        assert_eq!(stats.attack_range, base + 120.0);
        assert!(
            stats.is_melee(),
            "the bonus does not change the classification"
        );
        let unit = world.spawn((statuses, stats, Health::new(100.0))).id();

        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(2.0));
        world.run_system_once(tick_status_effects).unwrap();

        let stats = world.get::<CombatStats>(unit).unwrap();
        assert_eq!(stats.attack_range, base);
        assert_eq!(stats.attack_range_bonus, 0.0);
        assert!(world.get::<StatusEffects>(unit).unwrap().effects.is_empty());
    }

    #[test]
    fn cleave_modifier_status_adds_and_reverts_exactly() {
        use std::time::Duration;
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<Messages<StatusLifecycleEvent>>();
        let mut statuses = StatusEffects::default();
        let mut stats =
            CombatStats::simple(10.0, crate::scale::MELEE_ATTACK_RANGE, 1.0, 0.0, 0.0, 10.0);
        // A permanent item cleave is a separate instance and must be untouched.
        stats
            .cleaves
            .adjust(CleaveSource::Items, 0.1, 0.0, 0.0, 0.0, 1);
        let empower = || StatusEffect {
            cleave: 0.4,
            cleave_radius: 150.0,
            cleave_end_radius: 360.0,
            cleave_distance: 650.0,
            ..StatusEffect::buff("empower_cleave", 1.0)
        };
        let key = CleaveSource::Status("empower_cleave");
        apply_status(&mut statuses, &mut stats, empower(), 0.0);
        let inst = *stats.cleaves.get(key).unwrap();
        assert!((inst.ratio - 0.4).abs() < 1e-6);
        assert_eq!(
            (inst.radius, inst.end_radius, inst.distance),
            (150.0, 360.0, 650.0)
        );
        assert!((stats.cleaves.get(CleaveSource::Items).unwrap().ratio - 0.1).abs() < 1e-6);
        // Refreshing replaces the old delta instead of stacking it.
        apply_status(&mut statuses, &mut stats, empower(), 0.0);
        assert_eq!(stats.cleaves.get(key), Some(&inst), "a refresh replaces it");
        let unit = world.spawn((statuses, stats, Health::new(100.0))).id();
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(2.0));
        world.run_system_once(tick_status_effects).unwrap();
        let stats = world.get::<CombatStats>(unit).unwrap();
        assert!(
            stats.cleaves.get(key).is_none(),
            "expiry removes the instance"
        );
        assert!((stats.cleaves.get(CleaveSource::Items).unwrap().ratio - 0.1).abs() < 1e-6);
    }

    #[test]
    fn statuses_of_one_source_share_an_instance_until_the_last_ends() {
        use crate::abilities::effects::EffectSource;
        let source = Some(EffectSource::Ability(crate::components::AbilityId::Empower));
        let mut statuses = StatusEffects::default();
        let mut stats =
            CombatStats::simple(10.0, crate::scale::MELEE_ATTACK_RANGE, 1.0, 0.0, 0.0, 10.0);
        apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect {
                cleave: 0.3,
                cleave_source: source,
                ..StatusEffect::buff("a_cleave", 1.0)
            },
            0.0,
        );
        apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect {
                cleave_distance: 500.0,
                cleave_source: source,
                ..StatusEffect::buff("a_cleave_distance", 2.0)
            },
            0.0,
        );
        assert_eq!(stats.cleaves.iter().count(), 1);
        let key = CleaveSource::Effect(source.unwrap());
        assert_eq!(stats.cleaves.get(key).unwrap().distance, 500.0);
        let first = statuses.effects.remove(0);
        subtract_status_stat_delta(&first, &mut stats, 0.0);
        assert_eq!(stats.cleaves.get(key).unwrap().ratio, 0.0);
        assert_eq!(stats.cleaves.iter().count(), 1, "one feeder remains");
        let last = statuses.effects.remove(0);
        subtract_status_stat_delta(&last, &mut stats, 0.0);
        assert!(stats.cleaves.is_empty());
    }

    #[test]
    fn evasion_status_is_basic_dispellable() {
        let evasion = StatusSpec::Evasion {
            percent: RankValue::fixed(40.0),
        }
        .build(
            1,
            5.0,
            StatusBuildContext {
                move_speed: 0.0,
                agility: 0.0,
                source: None,
                point: Vec3::ZERO,
                base_attack_damage: 0.0,
            },
        );
        assert_eq!(evasion.kind, StatusKind::Buff);
        assert!((evasion.evasion - 0.4).abs() < 1e-6);
        assert!(status_dispel_matches(
            &evasion,
            StatusKind::Buff,
            DispelStrength::Basic
        ));
        let mut statuses = StatusEffects::default();
        statuses.effects.push(evasion);
        assert_eq!(statuses.evasions().collect::<Vec<_>>(), vec![0.4]);
    }

    #[test]
    fn phased_buff_flags_ignore_unit_collision() {
        let mut statuses = StatusEffects::default();
        assert!(!statuses.is_phased());
        apply_phased(&mut statuses, 1.0);
        assert!(statuses.is_phased());
        assert!(statuses.effects.iter().any(|e| e.ignore_unit_collision));
        statuses.effects[0].remaining = 0.0;
        assert!(!statuses.is_phased());
    }

    #[test]
    fn forceful_buff_enables_unit_push() {
        let mut statuses = StatusEffects::default();
        assert!(!statuses.can_push_units());
        apply_forceful(&mut statuses, 2.0);
        assert!(statuses.can_push_units());
        assert!(!statuses.is_phased());
        statuses.effects[0].remaining = 0.0;
        assert!(!statuses.can_push_units());
    }

    #[test]
    fn debuff_immunity_blocks_stun() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 10.0);
        assert!(apply_debuff_immunity(&mut statuses, &mut stats, 5.0, 0.0));
        assert!(!apply_stun(&mut statuses, &mut stats, 2.0));
        assert!(!statuses.is_stunned());
    }

    // --- Lifecycle events ---

    fn lifecycle_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<StatusEffectEvent>()
            .add_message::<StatusLifecycleEvent>()
            .add_message::<DamageEvent>()
            .add_message::<crate::combat::DamageAppliedEvent>()
            .add_systems(
                Update,
                (
                    apply_status_effect_events,
                    crate::combat::apply_damage_events,
                    tick_status_effects,
                )
                    .chain(),
            );
        app
    }

    fn lifecycle_unit(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                StatusEffects::default(),
                CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0),
                HeroAttributes::starter(),
                Health::new(1000.0),
            ))
            .id()
    }

    fn lifecycle_frame(app: &mut App, dt: f32) -> Vec<(&'static str, StatusLifecycle)> {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));
        app.update();
        app.world()
            .resource::<Messages<StatusLifecycleEvent>>()
            .iter_current_update_messages()
            .map(|event| (event.id, event.change))
            .collect()
    }

    fn change(app: &mut App, target: Entity, change: StatusChange, reactive: bool) {
        app.world_mut()
            .write_message(StatusEffectEvent::new(None, target, change).reactive(reactive));
    }

    #[test]
    fn lifecycle_events_for_each_removal_cause() {
        use StatusLifecycle::*;
        let mut app = lifecycle_app();
        let unit = lifecycle_unit(&mut app);

        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::stunned(5.0)),
            false,
        );
        assert_eq!(lifecycle_frame(&mut app, 0.01), [("stun", Applied)]);
        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::stunned(5.0)),
            false,
        );
        assert_eq!(
            lifecycle_frame(&mut app, 0.01),
            [("stun", Applied)],
            "refresh"
        );

        change(
            &mut app,
            unit,
            StatusChange::Dispel {
                kind: StatusKind::Debuff,
                strength: DispelStrength::Basic,
            },
            false,
        );
        assert_eq!(lifecycle_frame(&mut app, 0.01), [("stun", Dispelled)]);

        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::stunned(5.0)),
            false,
        );
        lifecycle_frame(&mut app, 0.01);
        change(&mut app, unit, StatusChange::Remove("stun"), false);
        assert_eq!(lifecycle_frame(&mut app, 0.01), [("stun", Removed)]);

        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::buff("haste", 0.5)),
            false,
        );
        lifecycle_frame(&mut app, 0.01);
        assert_eq!(lifecycle_frame(&mut app, 1.0), [("haste", Expired)]);

        // Gaining debuff immunity purges debuffs: they are reported as dispelled.
        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::stunned(5.0)),
            false,
        );
        lifecycle_frame(&mut app, 0.01);
        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::debuff_immunity(5.0)),
            false,
        );
        let events = lifecycle_frame(&mut app, 0.01);
        assert_eq!(events, [("stun", Dispelled), ("debuff_immunity", Applied)]);

        // Sleep breaks on damage.
        let sleeper = lifecycle_unit(&mut app);
        change(
            &mut app,
            sleeper,
            StatusChange::Apply(StatusEffect::sleep(10.0)),
            false,
        );
        lifecycle_frame(&mut app, 0.01);
        app.world_mut()
            .write_message(DamageEvent::new(sleeper, 10.0, DamageType::Physical));
        assert_eq!(lifecycle_frame(&mut app, 0.01), [("sleep", Removed)]);
    }

    /// Damage always breaks sleep, but a reactive hit wakes the unit silently.
    #[test]
    fn sleep_break_reports_removed_unless_reactive() {
        for (reactive, expected) in [
            (false, vec![("sleep", StatusLifecycle::Removed)]),
            (true, vec![]),
        ] {
            let mut app = lifecycle_app();
            let sleeper = lifecycle_unit(&mut app);
            change(
                &mut app,
                sleeper,
                StatusChange::Apply(StatusEffect::sleep(10.0)),
                false,
            );
            lifecycle_frame(&mut app, 0.01);
            app.world_mut().write_message(
                DamageEvent::new(sleeper, 10.0, DamageType::Physical).reactive(reactive),
            );
            assert_eq!(lifecycle_frame(&mut app, 0.01), expected, "{reactive}");
            assert!(
                !app.world()
                    .get::<StatusEffects>(sleeper)
                    .unwrap()
                    .is_asleep(),
                "woken either way (reactive: {reactive})"
            );
        }
    }

    #[test]
    fn rejected_debuff_emits_nothing() {
        let mut app = lifecycle_app();
        let unit = lifecycle_unit(&mut app);
        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::debuff_immunity(5.0)),
            false,
        );
        lifecycle_frame(&mut app, 0.01);

        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::stunned(5.0)),
            false,
        );
        assert!(lifecycle_frame(&mut app, 0.01).is_empty());
        assert!(!app.world().get::<StatusEffects>(unit).unwrap().is_stunned());
    }

    #[test]
    fn reactive_status_expires_silently() {
        let mut app = lifecycle_app();
        let unit = lifecycle_unit(&mut app);

        // A reactive event: applied, but unreported; the status itself is stamped reactive.
        let reactive_stun = StatusEffect {
            reactive: true,
            ..StatusEffect::stunned(0.5)
        };
        change(&mut app, unit, StatusChange::Apply(reactive_stun), true);
        assert!(lifecycle_frame(&mut app, 0.01).is_empty());
        assert!(app.world().get::<StatusEffects>(unit).unwrap().is_stunned());
        assert!(lifecycle_frame(&mut app, 1.0).is_empty(), "silent expiry");
        assert!(!app.world().get::<StatusEffects>(unit).unwrap().is_stunned());

        // A reactive status removed by an ordinary event is silent too.
        let held = StatusEffect {
            reactive: true,
            ..StatusEffect::buff("held", 100.0)
        };
        change(&mut app, unit, StatusChange::Apply(held), true);
        lifecycle_frame(&mut app, 0.01);
        change(&mut app, unit, StatusChange::Remove("held"), false);
        assert!(lifecycle_frame(&mut app, 0.01).is_empty());

        // An ordinary status still reports once the reactive ones are gone.
        change(
            &mut app,
            unit,
            StatusChange::Apply(StatusEffect::buff("plain", 0.5)),
            false,
        );
        lifecycle_frame(&mut app, 0.01);
        assert_eq!(
            lifecycle_frame(&mut app, 1.0),
            [("plain", StatusLifecycle::Expired)]
        );
    }

    #[test]
    fn dispel_tiers_remove_basic_but_not_strong_debuffs() {
        let mut app = App::new();
        app.add_message::<StatusEffectEvent>()
            .add_message::<StatusLifecycleEvent>()
            .add_systems(Update, apply_status_effect_events);
        let victim = app
            .world_mut()
            .spawn((
                StatusEffects::default(),
                CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0),
                HeroAttributes::starter(),
            ))
            .id();
        app.world_mut().write_message(StatusEffectEvent {
            reactive: false,
            source: None,
            target: victim,
            change: StatusChange::Apply(StatusEffect::stunned(2.0)),
        });
        app.world_mut().write_message(StatusEffectEvent {
            reactive: false,
            source: None,
            target: victim,
            change: StatusChange::Apply(StatusEffect::hexed(2.0)),
        });
        app.update();
        let st = app.world().get::<StatusEffects>(victim).unwrap();
        assert!(st.is_stunned());
        assert!(st.effects.iter().any(|e| e.id == "hex"));
        app.world_mut().write_message(StatusEffectEvent {
            reactive: false,
            source: None,
            target: victim,
            change: StatusChange::Dispel {
                kind: StatusKind::Debuff,
                strength: super::DispelStrength::Basic,
            },
        });
        app.update();
        let st = app.world().get::<StatusEffects>(victim).unwrap();
        assert!(!st.is_stunned());
        assert!(st.effects.iter().any(|e| e.id == "hex"));
        app.world_mut().write_message(StatusEffectEvent {
            reactive: false,
            source: None,
            target: victim,
            change: StatusChange::Dispel {
                kind: StatusKind::Debuff,
                strength: super::DispelStrength::Strong,
            },
        });
        app.update();
        let st = app.world().get::<StatusEffects>(victim).unwrap();
        assert!(!st.effects.iter().any(|e| e.id == "hex"));
    }

    #[test]
    fn heal_does_not_revive_dead_units_via_heal_over_time() {
        use std::time::Duration;
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<DamageEvent>()
            .add_message::<crate::combat::DamageAppliedEvent>()
            .add_message::<crate::status::StatusLifecycleEvent>()
            .add_systems(Update, tick_status_effects);
        let mut statuses = StatusEffects::default();
        statuses.effects.push(StatusEffect {
            heal_per_sec: 50.0,
            ..StatusEffect::buff("hot", 5.0)
        });
        let dead = app
            .world_mut()
            .spawn((
                statuses,
                CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                Health {
                    current: 0.0,
                    max: 100.0,
                    regen_per_sec: 0.0,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs(1));
        app.update();
        assert_eq!(app.world().get::<Health>(dead).unwrap().current, 0.0);
    }

    mod max_caps {
        use super::*;
        use crate::components::Mana;
        use crate::vitals::{ResourceStat, sync_status_resource_caps};
        use std::time::Duration;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_message::<crate::status::StatusLifecycleEvent>()
                .add_systems(
                    Update,
                    (
                        apply_status_effect_events,
                        tick_status_effects,
                        sync_status_resource_caps,
                    )
                        .chain(),
                );
            app
        }

        fn unit(app: &mut App, current: f32, max: f32) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    Health {
                        current,
                        max,
                        regen_per_sec: 0.0,
                    },
                    Mana {
                        current: 50.0,
                        max: 100.0,
                        regen_per_sec: 0.0,
                    },
                ))
                .id()
        }

        fn buff(id: &'static str, stat: ResourceStat, amount: f32, secs: f32) -> StatusEffect {
            StatusEffect::resource_modifier(id, StatusKind::Buff, stat, amount, secs)
        }

        fn send(app: &mut App, target: Entity, change: StatusChange) {
            app.world_mut().write_message(StatusEffectEvent {
                reactive: false,
                source: None,
                target,
                change,
            });
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn hp(app: &App, e: Entity) -> (f32, f32) {
            let h = app.world().get::<Health>(e).unwrap();
            (h.current, h.max)
        }

        #[test]
        fn timed_max_health_preserves_ratio_on_apply_expire_and_dispel() {
            let mut app = app();
            let a = unit(&mut app, 500.0, 1000.0);
            send(
                &mut app,
                a,
                StatusChange::Apply(buff("hp_buff", ResourceStat::MaxHealth, 1000.0, 2.0)),
            );
            step(&mut app, 0.0);
            assert_eq!(hp(&app, a), (1000.0, 2000.0), "apply keeps the 50% ratio");
            // Expiry
            step(&mut app, 2.5);
            step(&mut app, 0.0);
            assert_eq!(
                hp(&app, a),
                (500.0, 1000.0),
                "expiry: no free heal, no extra loss"
            );

            // Dispel
            let b = unit(&mut app, 250.0, 1000.0);
            send(
                &mut app,
                b,
                StatusChange::Apply(buff("hp_buff", ResourceStat::MaxHealth, 1000.0, 30.0)),
            );
            step(&mut app, 0.0);
            assert_eq!(hp(&app, b), (500.0, 2000.0));
            send(
                &mut app,
                b,
                StatusChange::Dispel {
                    kind: StatusKind::Buff,
                    strength: DispelStrength::Basic,
                },
            );
            step(&mut app, 0.0);
            assert_eq!(hp(&app, b), (250.0, 1000.0));
        }

        #[test]
        fn refresh_and_remove_do_not_leak_max() {
            let mut app = app();
            let u = unit(&mut app, 1000.0, 1000.0);
            for _ in 0..3 {
                send(
                    &mut app,
                    u,
                    StatusChange::Apply(buff("hp_buff", ResourceStat::MaxHealth, 300.0, 30.0)),
                );
                step(&mut app, 0.0);
            }
            assert_eq!(
                hp(&app, u).1,
                1300.0,
                "refreshing the same id does not stack"
            );
            send(&mut app, u, StatusChange::Remove("hp_buff"));
            step(&mut app, 0.0);
            assert_eq!(hp(&app, u), (1000.0, 1000.0));
        }

        #[test]
        fn timed_max_mana_and_unrelated_stats_are_independent() {
            let mut app = app();
            let u = unit(&mut app, 100.0, 1000.0);
            send(
                &mut app,
                u,
                StatusChange::Apply(buff("mp_buff", ResourceStat::MaxMana, 100.0, 1.0)),
            );
            step(&mut app, 0.0);
            let mana = *app.world().get::<Mana>(u).unwrap();
            assert_eq!((mana.current, mana.max), (100.0, 200.0));
            assert_eq!(hp(&app, u), (100.0, 1000.0));
            step(&mut app, 1.5);
            step(&mut app, 0.0);
            let mana = *app.world().get::<Mana>(u).unwrap();
            assert_eq!((mana.current, mana.max), (50.0, 100.0));
        }

        #[test]
        fn clamped_max_debuff_restores_the_original_max_on_expiry() {
            let mut app = app();
            let u = unit(&mut app, 100.0, 100.0);
            let debuff = StatusEffect::resource_modifier(
                "curse",
                StatusKind::Debuff,
                ResourceStat::MaxHealth,
                -500.0,
                2.0,
            );
            send(&mut app, u, StatusChange::Apply(debuff));
            step(&mut app, 0.0);
            assert_eq!(hp(&app, u).1, 1.0, "max floors at 1");
            step(&mut app, 2.5);
            step(&mut app, 0.0);
            assert_eq!(hp(&app, u).1, 100.0, "expiry must not leak extra max");
        }

        #[test]
        fn a_dead_unit_stays_dead_when_max_changes() {
            let mut app = app();
            let u = unit(&mut app, 0.0, 1000.0);
            send(
                &mut app,
                u,
                StatusChange::Apply(buff("hp_buff", ResourceStat::MaxHealth, 500.0, 5.0)),
            );
            step(&mut app, 0.0);
            assert_eq!(hp(&app, u), (0.0, 1500.0));
        }
    }

    mod independent_instances {
        use super::*;
        use std::time::Duration;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_systems(
                    Update,
                    (apply_status_effect_events, tick_status_effects).chain(),
                );
            app
        }

        fn unit(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(50.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    Health {
                        current: 100.0,
                        max: 100.0,
                        regen_per_sec: 0.0,
                    },
                ))
                .id()
        }

        fn drain(independent: bool, secs: f32) -> StatusEffect {
            StatusEffect {
                attack_damage: -1.0,
                independent,
                ..StatusEffect::debuff("shift_attack_damage", secs)
            }
        }

        fn send(app: &mut App, target: Entity, change: StatusChange) {
            app.world_mut().write_message(StatusEffectEvent {
                reactive: false,
                source: None,
                target,
                change,
            });
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn damage(app: &App, e: Entity) -> f32 {
            app.world().get::<CombatStats>(e).unwrap().attack_damage
        }

        fn instances(app: &App, e: Entity) -> usize {
            app.world()
                .get::<StatusEffects>(e)
                .unwrap()
                .effects
                .iter()
                .filter(|s| s.id == "shift_attack_damage")
                .count()
        }

        #[test]
        fn independent_instances_coexist_and_expire_separately() {
            let mut app = app();
            let u = unit(&mut app);
            send(&mut app, u, StatusChange::Apply(drain(true, 10.0)));
            step(&mut app, 0.0);
            step(&mut app, 5.0);
            send(&mut app, u, StatusChange::Apply(drain(true, 10.0)));
            step(&mut app, 0.0);
            assert_eq!(instances(&app, u), 2);
            // The first one ends at t = 10, the second at t = 15.
            step(&mut app, 5.05);
            assert_eq!(instances(&app, u), 1);
            assert!((damage(&app, u) - 49.0).abs() < 1e-4);
            step(&mut app, 5.0);
            assert_eq!(instances(&app, u), 0);
            assert!((damage(&app, u) - 50.0).abs() < 1e-4);
        }

        #[test]
        fn independent_attack_damage_sums_and_reverts_exactly() {
            let mut app = app();
            let u = unit(&mut app);
            for _ in 0..7 {
                send(&mut app, u, StatusChange::Apply(drain(true, 4.0)));
            }
            step(&mut app, 0.0);
            assert_eq!(instances(&app, u), 7);
            assert!((damage(&app, u) - 43.0).abs() < 1e-4);
            step(&mut app, 4.5);
            assert_eq!(instances(&app, u), 0);
            assert_eq!(damage(&app, u), 50.0);
        }

        #[test]
        fn remove_by_id_and_dispel_remove_every_instance() {
            let mut app = app();
            let u = unit(&mut app);
            for _ in 0..3 {
                send(&mut app, u, StatusChange::Apply(drain(true, 30.0)));
            }
            step(&mut app, 0.0);
            assert_eq!(instances(&app, u), 3);
            send(&mut app, u, StatusChange::Remove("shift_attack_damage"));
            step(&mut app, 0.0);
            assert_eq!(instances(&app, u), 0);
            assert_eq!(damage(&app, u), 50.0);

            for _ in 0..3 {
                send(&mut app, u, StatusChange::Apply(drain(true, 30.0)));
            }
            step(&mut app, 0.0);
            assert_eq!(instances(&app, u), 3);
            send(
                &mut app,
                u,
                StatusChange::Dispel {
                    kind: StatusKind::Debuff,
                    strength: DispelStrength::Basic,
                },
            );
            step(&mut app, 0.0);
            assert_eq!(instances(&app, u), 0);
            assert_eq!(damage(&app, u), 50.0);
        }

        #[test]
        fn non_independent_modifier_still_refreshes() {
            let mut app = app();
            let u = unit(&mut app);
            for _ in 0..3 {
                send(&mut app, u, StatusChange::Apply(drain(false, 30.0)));
            }
            step(&mut app, 0.0);
            assert_eq!(instances(&app, u), 1);
            assert!((damage(&app, u) - 49.0).abs() < 1e-4);
        }
    }

    mod attributes {
        use super::*;
        use crate::components::Mana;
        use crate::vitals::{
            AppliedStatusAttributes, ResourceStat, sync_status_attributes,
            sync_status_resource_caps,
        };
        use std::time::Duration;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_systems(
                    Update,
                    (
                        apply_status_effect_events,
                        tick_status_effects,
                        sync_status_resource_caps,
                        sync_status_attributes,
                    )
                        .chain(),
                );
            app
        }

        /// A hero with starter attributes folded into its vitals the way `HeroDef::vitals` does.
        fn hero(app: &mut App) -> Entity {
            let attrs = HeroAttributes::starter();
            let mut health = Health {
                current: 500.0,
                max: 500.0,
                regen_per_sec: 0.0,
            };
            let mut mana = Mana::new(100.0, 0.0);
            let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
            attrs.apply_to(&mut health, &mut mana, &mut stats);
            app.world_mut()
                .spawn((StatusEffects::default(), attrs, health, mana, stats))
                .id()
        }

        fn modifier(id: &'static str, stat: ResourceStat, amount: f32, secs: f32) -> StatusEffect {
            let kind = if amount >= 0.0 {
                StatusKind::Buff
            } else {
                StatusKind::Debuff
            };
            StatusEffect::resource_modifier(id, kind, stat, amount, secs)
        }

        fn send(app: &mut App, target: Entity, change: StatusChange) {
            app.world_mut().write_message(StatusEffectEvent {
                reactive: false,
                source: None,
                target,
                change,
            });
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn snapshot(app: &App, e: Entity) -> (HeroAttributes, f32, f32, f32, f32, f32) {
            let w = app.world();
            let attrs = *w.get::<HeroAttributes>(e).unwrap();
            let health = w.get::<Health>(e).unwrap();
            let mana = w.get::<Mana>(e).unwrap();
            let stats = w.get::<CombatStats>(e).unwrap();
            (
                attrs,
                health.max,
                mana.max,
                stats.armor,
                stats.magic_resist,
                stats.attack_speed,
            )
        }

        #[test]
        fn timed_agility_raises_attributes_armor_and_attack_speed_then_reverts() {
            let mut app = app();
            let hero = hero(&mut app);
            let base = snapshot(&app, hero);
            send(
                &mut app,
                hero,
                StatusChange::Apply(modifier("t_agility", ResourceStat::Agility, 9.0, 2.0)),
            );
            step(&mut app, 0.0);
            step(&mut app, 0.0);
            let up = snapshot(&app, hero);
            assert_eq!(up.0.agility, base.0.agility + 9.0);
            assert!((up.3 - (base.3 + 9.0 * HeroAttributes::ARMOR_PER_AGI)).abs() < 1e-4);
            assert!(up.5 > base.5, "attack speed {} -> {}", base.5, up.5);
            assert_eq!(up.1, base.1, "strength-fed health is untouched");
            // Expiry puts everything back.
            step(&mut app, 2.5);
            step(&mut app, 0.0);
            let down = snapshot(&app, hero);
            assert_eq!(down.0.agility, base.0.agility);
            assert!((down.3 - base.3).abs() < 1e-4);
            assert!((down.5 - base.5).abs() < 1e-5);
        }

        #[test]
        fn timed_strength_changes_max_health_like_items() {
            let mut app = app();
            let hero = hero(&mut app);
            let base = snapshot(&app, hero);
            send(
                &mut app,
                hero,
                StatusChange::Apply(modifier("t_strength", ResourceStat::Strength, 10.0, 30.0)),
            );
            step(&mut app, 0.0);
            let up = snapshot(&app, hero);
            assert!((up.1 - (base.1 + 10.0 * HeroAttributes::HP_PER_STR)).abs() < 1e-3);
            // A steal (negative) is a debuff and lowers it.
            send(
                &mut app,
                hero,
                StatusChange::Apply(modifier("t_int", ResourceStat::Intelligence, -5.0, 30.0)),
            );
            step(&mut app, 0.0);
            let down = snapshot(&app, hero);
            assert!((down.2 - (base.2 - 5.0 * HeroAttributes::MANA_PER_INT)).abs() < 1e-3);
            assert!((down.4 - (base.4 - 5.0 * HeroAttributes::MR_PER_INT)).abs() < 1e-4);
        }

        #[test]
        fn dispelled_attribute_status_reverts() {
            let mut app = app();
            let hero = hero(&mut app);
            let base = snapshot(&app, hero);
            send(
                &mut app,
                hero,
                StatusChange::Apply(modifier("t_strength", ResourceStat::Strength, 10.0, 30.0)),
            );
            step(&mut app, 0.0);
            assert_ne!(snapshot(&app, hero).1, base.1);
            send(
                &mut app,
                hero,
                StatusChange::Dispel {
                    kind: StatusKind::Buff,
                    strength: DispelStrength::Basic,
                },
            );
            step(&mut app, 0.0);
            let after = snapshot(&app, hero);
            assert_eq!(after.0.strength, base.0.strength);
            assert!((after.1 - base.1).abs() < 1e-3);
            assert_eq!(
                *app.world().get::<AppliedStatusAttributes>(hero).unwrap(),
                AppliedStatusAttributes::default()
            );
        }

        #[test]
        fn attribute_status_on_unit_without_attributes_is_ignored() {
            let mut app = app();
            let unit = app
                .world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(0.0, 0.0, 1.0, 5.0, 0.0, 300.0),
                    Health {
                        current: 100.0,
                        max: 100.0,
                        regen_per_sec: 0.0,
                    },
                    Mana::new(50.0, 0.0),
                ))
                .id();
            send(
                &mut app,
                unit,
                StatusChange::Apply(modifier("t_agility", ResourceStat::Agility, 9.0, 5.0)),
            );
            step(&mut app, 0.0);
            step(&mut app, 0.0);
            let w = app.world();
            assert_eq!(w.get::<CombatStats>(unit).unwrap().armor, 5.0);
            assert_eq!(w.get::<Health>(unit).unwrap().max, 100.0);
            assert!(w.get::<AppliedStatusAttributes>(unit).is_none());
        }

        fn independent_agility(secs: f32) -> StatusEffect {
            StatusEffect {
                independent: true,
                ..modifier("es_agility", ResourceStat::Agility, 3.0, secs)
            }
        }

        #[test]
        fn independent_same_id_agility_stacks_expire_one_by_one_without_drift() {
            let mut app = app();
            let hero = hero(&mut app);
            let base = snapshot(&app, hero);
            send(
                &mut app,
                hero,
                StatusChange::Apply(independent_agility(4.0)),
            );
            step(&mut app, 0.0);
            step(&mut app, 2.0);
            send(
                &mut app,
                hero,
                StatusChange::Apply(independent_agility(4.0)),
            );
            send(
                &mut app,
                hero,
                StatusChange::Apply(independent_agility(4.0)),
            );
            step(&mut app, 0.0);
            step(&mut app, 0.0);
            assert_eq!(snapshot(&app, hero).0.agility, base.0.agility + 9.0);
            // The first stack ends at t = 4, the other two at t = 6.
            step(&mut app, 2.1);
            step(&mut app, 0.0);
            let mid = snapshot(&app, hero);
            assert_eq!(mid.0.agility, base.0.agility + 6.0);
            assert!((mid.3 - (base.3 + 6.0 * HeroAttributes::ARMOR_PER_AGI)).abs() < 1e-4);
            step(&mut app, 2.0);
            step(&mut app, 0.0);
            let down = snapshot(&app, hero);
            assert_eq!(down.0.agility, base.0.agility);
            assert!((down.3 - base.3).abs() < 1e-4);
            assert!((down.5 - base.5).abs() < 1e-5);
        }

        #[test]
        fn many_stack_churn_leaves_no_attribute_drift() {
            let mut app = app();
            let hero = hero(&mut app);
            let base = snapshot(&app, hero);
            for _ in 0..40 {
                send(
                    &mut app,
                    hero,
                    StatusChange::Apply(independent_agility(1.1)),
                );
                step(&mut app, 0.3);
            }
            step(&mut app, 2.0);
            step(&mut app, 0.0);
            let down = snapshot(&app, hero);
            assert!((down.0.agility - base.0.agility).abs() < 1e-3);
            assert!((down.3 - base.3).abs() < 1e-3);
            assert!((down.5 - base.5).abs() < 1e-3);
        }

        #[test]
        fn level_up_during_a_timed_attribute_keeps_both_parts_on_expiry() {
            let mut app = app();
            let hero = hero(&mut app);
            let base = snapshot(&app, hero);
            send(
                &mut app,
                hero,
                StatusChange::Apply(modifier("t_agility", ResourceStat::Agility, 5.0, 2.0)),
            );
            step(&mut app, 0.0);
            step(&mut app, 0.0);
            // A level-up folds its own gain through `apply_delta`, as progression does.
            {
                let w = app.world_mut();
                let mut attrs = *w.get::<HeroAttributes>(hero).unwrap();
                let before = attrs;
                attrs.level_up();
                let mut health = *w.get::<Health>(hero).unwrap();
                let mut mana = *w.get::<Mana>(hero).unwrap();
                let mut stats = *w.get::<CombatStats>(hero).unwrap();
                HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
                *w.get_mut::<HeroAttributes>(hero).unwrap() = attrs;
                *w.get_mut::<Health>(hero).unwrap() = health;
                *w.get_mut::<Mana>(hero).unwrap() = mana;
                *w.get_mut::<CombatStats>(hero).unwrap() = stats;
            }
            let leveled = *app.world().get::<HeroAttributes>(hero).unwrap();
            step(&mut app, 2.5);
            step(&mut app, 0.0);
            let after = snapshot(&app, hero);
            assert!(
                (after.0.agility - leveled.agility + 5.0).abs() < 1e-4,
                "only the status part is removed"
            );
            assert!(after.0.agility > base.0.agility);
        }

        #[test]
        fn dead_holder_attribute_status_still_reverts() {
            let mut app = app();
            let hero = hero(&mut app);
            let base = snapshot(&app, hero);
            send(
                &mut app,
                hero,
                StatusChange::Apply(modifier("t_strength", ResourceStat::Strength, 10.0, 2.0)),
            );
            step(&mut app, 0.0);
            step(&mut app, 0.0);
            app.world_mut().get_mut::<Health>(hero).unwrap().current = 0.0;
            step(&mut app, 2.5);
            step(&mut app, 0.0);
            let down = snapshot(&app, hero);
            assert_eq!(down.0.strength, base.0.strength);
            assert!((down.1 - base.1).abs() < 1e-3);
        }

        #[test]
        fn two_instances_add() {
            let mut app = app();
            let hero = hero(&mut app);
            let base = snapshot(&app, hero);
            send(
                &mut app,
                hero,
                StatusChange::Apply(modifier("a_agility", ResourceStat::Agility, 3.0, 2.0)),
            );
            send(
                &mut app,
                hero,
                StatusChange::Apply(modifier("b_agility", ResourceStat::Agility, 3.0, 6.0)),
            );
            step(&mut app, 0.0);
            assert_eq!(snapshot(&app, hero).0.agility, base.0.agility + 6.0);
            // Only the shorter one expires.
            step(&mut app, 2.5);
            step(&mut app, 0.0);
            assert_eq!(snapshot(&app, hero).0.agility, base.0.agility + 3.0);
            step(&mut app, 4.0);
            step(&mut app, 0.0);
            assert_eq!(snapshot(&app, hero).0.agility, base.0.agility);
        }
    }

    mod heal_statuses {
        use super::*;
        use crate::vitals::{HealKind, apply_heal, heal_factor};
        use std::time::Duration;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_message::<crate::status::StatusLifecycleEvent>()
                .add_systems(
                    Update,
                    (apply_status_effect_events, tick_status_effects).chain(),
                );
            app
        }

        fn wounded(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    Health {
                        current: 100.0,
                        max: 1000.0,
                        regen_per_sec: 0.0,
                    },
                ))
                .id()
        }

        fn send(app: &mut App, target: Entity, change: StatusChange) {
            app.world_mut().write_message(StatusEffectEvent {
                reactive: false,
                source: None,
                target,
                change,
            });
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn factor_of(app: &App, unit: Entity, kind: HealKind) -> f32 {
            let w = app.world();
            heal_factor(
                kind,
                0.5,
                w.get::<CombatStats>(unit),
                w.get::<StatusEffects>(unit),
            )
        }

        #[test]
        fn heal_block_zeroes_every_heal_kind_and_is_dispellable() {
            let mut app = app();
            let unit = wounded(&mut app);
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_block(10.0)),
            );
            step(&mut app, 0.0);
            for kind in [
                HealKind::Direct,
                HealKind::OverTime,
                HealKind::Leech,
                HealKind::Regen,
                HealKind::Conversion,
            ] {
                assert_eq!(factor_of(&app, unit, kind), 0.0, "{kind:?}");
            }
            send(
                &mut app,
                unit,
                StatusChange::Dispel {
                    kind: StatusKind::Debuff,
                    strength: DispelStrength::Basic,
                },
            );
            step(&mut app, 0.0);
            assert!(factor_of(&app, unit, HealKind::Direct) > 0.0);
        }

        #[test]
        fn heal_reduction_blocked_by_debuff_immunity() {
            let mut app = app();
            let unit = wounded(&mut app);
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::debuff_immunity(10.0)),
            );
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_reduction(0.5, 10.0)),
            );
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_block(10.0)),
            );
            step(&mut app, 0.0);
            let statuses = app.world().get::<StatusEffects>(unit).unwrap();
            assert_eq!(statuses.heal_reduction(), 0.0);
            assert!(!statuses.is_heal_blocked());
        }

        #[test]
        fn heal_over_time_ticks_total_over_duration() {
            let mut app = app();
            let unit = wounded(&mut app);
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_over_time(50.0, 4.0)),
            );
            step(&mut app, 0.0);
            for _ in 0..8 {
                step(&mut app, 0.5);
            }
            let health = app.world().get::<Health>(unit).unwrap().current;
            assert!((health - 300.0).abs() < 1e-2, "100 + 50 x 4s, got {health}");
            assert!(
                app.world()
                    .get::<StatusEffects>(unit)
                    .unwrap()
                    .effects
                    .is_empty()
            );
        }

        #[test]
        fn heal_over_time_respects_reduction_and_target_amp_at_tick() {
            let mut app = app();
            let unit = wounded(&mut app);
            app.world_mut()
                .get_mut::<CombatStats>(unit)
                .unwrap()
                .heal_received_amp = 0.5;
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_over_time(100.0, 4.0)),
            );
            send(
                &mut app,
                unit,
                StatusChange::Apply(StatusEffect::heal_reduction(0.5, 10.0)),
            );
            step(&mut app, 0.0);
            step(&mut app, 1.0);
            let health = app.world().get::<Health>(unit).unwrap().current;
            assert!((health - 175.0).abs() < 1e-2, "100 + 100 x 1.5 x 0.5");
        }

        #[test]
        fn apply_heal_never_exceeds_max() {
            let mut h = Health {
                current: 999.0,
                max: 1000.0,
                regen_per_sec: 0.0,
            };
            assert_eq!(apply_heal(&mut h, 50.0, 1.0).applied, 1.0);
        }
    }

    mod shields {
        use super::*;
        use std::time::Duration;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_message::<crate::status::StatusLifecycleEvent>()
                .add_systems(
                    Update,
                    (apply_status_effect_events, tick_status_effects).chain(),
                );
            app
        }

        fn unit(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    Health::new(1000.0),
                    ShieldPool { amount: 0.0 },
                ))
                .id()
        }

        fn send(app: &mut App, target: Entity, change: StatusChange) {
            app.world_mut().write_message(StatusEffectEvent {
                reactive: false,
                source: None,
                target,
                change,
            });
        }

        fn step(app: &mut App, secs: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(secs));
            app.update();
        }

        fn pool(app: &App, unit: Entity) -> f32 {
            app.world().get::<ShieldPool>(unit).unwrap().amount
        }

        fn shield_status(id: &'static str, amount: f32, duration: f32) -> StatusEffect {
            StatusEffect {
                id,
                ..StatusEffect::shield(amount, duration)
            }
        }

        /// What `combat::apply_damage_events` does on a hit that the pool soaks.
        fn absorb(app: &mut App, unit: Entity, amount: f32) {
            let mut e = app.world_mut().entity_mut(unit);
            e.get_mut::<ShieldPool>().unwrap().amount -= amount;
            consume_shields(&mut e.get_mut::<StatusEffects>().unwrap(), amount);
        }

        #[test]
        fn dispelling_a_shield_buff_clears_its_pool() {
            let mut app = app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("shield", 100.0, 10.0)),
            );
            step(&mut app, 0.1);
            assert_eq!(pool(&app, u), 100.0);
            send(
                &mut app,
                u,
                StatusChange::Dispel {
                    kind: StatusKind::Buff,
                    strength: DispelStrength::Strong,
                },
            );
            step(&mut app, 0.1);
            assert_eq!(pool(&app, u), 0.0);
        }

        #[test]
        fn removing_a_shield_by_id_clears_its_pool() {
            let mut app = app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("shield", 100.0, 10.0)),
            );
            step(&mut app, 0.1);
            send(&mut app, u, StatusChange::Remove("shield"));
            step(&mut app, 0.1);
            assert_eq!(pool(&app, u), 0.0);
            assert!(
                app.world()
                    .get::<StatusEffects>(u)
                    .unwrap()
                    .effects
                    .is_empty()
            );
        }

        #[test]
        fn partially_consumed_shield_expiry_leaves_other_shields_intact() {
            let mut app = app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("first", 100.0, 1.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("second", 80.0, 30.0)),
            );
            step(&mut app, 0.1);
            assert_eq!(pool(&app, u), 180.0);
            absorb(&mut app, u, 50.0);
            step(&mut app, 2.0);
            let statuses = app.world().get::<StatusEffects>(u).unwrap();
            assert_eq!(statuses.effects.len(), 1);
            assert_eq!(statuses.effects[0].id, "second");
            assert_eq!(
                pool(&app, u),
                80.0,
                "the second shield keeps its full amount"
            );
        }

        #[test]
        fn shields_are_consumed_oldest_first() {
            let mut statuses = StatusEffects::default();
            statuses.effects.push(shield_status("a", 30.0, 5.0));
            statuses.effects.push(shield_status("b", 40.0, 5.0));
            consume_shields(&mut statuses, 50.0);
            assert_eq!(statuses.effects[0].shield_amount, 0.0);
            assert_eq!(statuses.effects[1].shield_amount, 20.0);
        }

        #[test]
        fn refreshing_a_shield_replaces_only_its_remaining_amount() {
            let mut app = app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("shield", 100.0, 10.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("other", 60.0, 10.0)),
            );
            step(&mut app, 0.1);
            absorb(&mut app, u, 70.0);
            assert_eq!(pool(&app, u), 90.0);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("shield", 100.0, 10.0)),
            );
            step(&mut app, 0.1);
            // "shield" had 30 left and "other" 60: refreshed to 100 gives 100 + 60.
            assert_eq!(pool(&app, u), 160.0);
        }

        // --- End to end through `combat::apply_damage_events` ---

        fn damage_app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_message::<crate::status::StatusLifecycleEvent>()
                .add_systems(
                    Update,
                    (
                        apply_status_effect_events,
                        crate::combat::apply_damage_events,
                        tick_status_effects,
                    )
                        .chain(),
                );
            app
        }

        fn hit(app: &mut App, unit: Entity, amount: f32) {
            app.world_mut()
                .write_message(DamageEvent::new(unit, amount, DamageType::Physical));
        }

        fn health(app: &App, unit: Entity) -> f32 {
            app.world().get::<Health>(unit).unwrap().current
        }

        /// `shield_amount` of every status that carries a shield, in vector order.
        fn remaining(app: &App, unit: Entity) -> Vec<(&'static str, f32)> {
            app.world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .effects
                .iter()
                .map(|e| (e.id, e.shield_amount))
                .collect()
        }

        #[test]
        fn damage_consumes_two_shields_oldest_first_and_health_is_untouched() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 180.0);

            hit(&mut app, u, 130.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 50.0);
            assert_eq!(remaining(&app, u), vec![("a", 0.0), ("b", 50.0)]);
            assert_eq!(health(&app, u), 1000.0);
        }

        #[test]
        fn overflow_damage_empties_every_shield_and_hurts_health_once() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 1.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);

            hit(&mut app, u, 200.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 0.0);
            assert_eq!(remaining(&app, u), vec![("a", 0.0), ("b", 0.0)]);
            assert_eq!(health(&app, u), 980.0);

            // Expiring a spent shield must not take anything out of the (empty) pool, and a new
            // shield applied afterwards is not shrunk by the old one.
            step(&mut app, 2.0);
            assert_eq!(pool(&app, u), 0.0);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("c", 40.0, 30.0)),
            );
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 40.0);
        }

        #[test]
        fn fully_consumed_shield_expiring_leaves_the_other_shield_intact() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 1.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);
            hit(&mut app, u, 120.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 60.0);

            step(&mut app, 2.0); // "a" (empty) expires
            assert_eq!(remaining(&app, u), vec![("b", 60.0)]);
            assert_eq!(pool(&app, u), 60.0);

            hit(&mut app, u, 100.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 0.0);
            assert_eq!(health(&app, u), 960.0);
        }

        #[test]
        fn expiring_the_second_shield_after_damage_only_releases_what_is_left() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 1.0)),
            );
            step(&mut app, 0.01);
            hit(&mut app, u, 110.0); // a: 0, b: 70
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 70.0);

            step(&mut app, 2.0); // b expires with 70 left
            assert_eq!(pool(&app, u), 0.0);
            hit(&mut app, u, 25.0);
            step(&mut app, 0.01);
            assert_eq!(health(&app, u), 975.0, "no shield is left to absorb");
        }

        #[test]
        fn dispelling_a_damaged_shield_releases_only_its_remainder() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            step(&mut app, 0.01);
            hit(&mut app, u, 30.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 70.0);

            send(
                &mut app,
                u,
                StatusChange::Dispel {
                    kind: StatusKind::Buff,
                    strength: DispelStrength::Strong,
                },
            );
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 0.0, "only the 70 that was left is released");
            hit(&mut app, u, 25.0);
            step(&mut app, 0.01);
            assert_eq!(
                health(&app, u),
                975.0,
                "a dispelled shield absorbs no further damage"
            );
        }

        #[test]
        fn removing_one_of_two_damaged_shields_keeps_the_other_absorbing() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);
            hit(&mut app, u, 130.0); // a: 0, b: 50
            step(&mut app, 0.01);

            send(&mut app, u, StatusChange::Remove("a"));
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 50.0, "a was spent, nothing to release");

            hit(&mut app, u, 70.0);
            step(&mut app, 0.01);
            assert_eq!(pool(&app, u), 0.0);
            assert_eq!(health(&app, u), 980.0);
            assert_eq!(remaining(&app, u), vec![("b", 0.0)]);
        }

        #[test]
        fn a_refreshed_shield_becomes_the_newest_and_is_consumed_last() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            step(&mut app, 0.01);
            assert_eq!(remaining(&app, u), vec![("b", 80.0), ("a", 100.0)]);
            assert_eq!(pool(&app, u), 180.0);

            hit(&mut app, u, 90.0);
            step(&mut app, 0.01);
            assert_eq!(remaining(&app, u), vec![("b", 0.0), ("a", 90.0)]);
            assert_eq!(pool(&app, u), 90.0);
        }

        #[test]
        fn each_absorbing_damage_event_is_charged_once() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("b", 80.0, 30.0)),
            );
            step(&mut app, 0.01);

            hit(&mut app, u, 40.0);
            hit(&mut app, u, 40.0);
            step(&mut app, 0.01);
            assert_eq!(remaining(&app, u), vec![("a", 20.0), ("b", 80.0)]);
            assert_eq!(pool(&app, u), 100.0);
            assert_eq!(health(&app, u), 1000.0);
        }

        #[test]
        fn shield_piercing_damage_does_not_consume_status_shields() {
            let mut app = damage_app();
            let u = unit(&mut app);
            send(
                &mut app,
                u,
                StatusChange::Apply(shield_status("a", 100.0, 30.0)),
            );
            step(&mut app, 0.01);
            app.world_mut().write_message(DamageEvent {
                pierces_shields: true,
                ..DamageEvent::new(u, 40.0, DamageType::Physical)
            });
            step(&mut app, 0.01);
            assert_eq!(remaining(&app, u), vec![("a", 100.0)]);
            assert_eq!(pool(&app, u), 100.0);
            assert_eq!(health(&app, u), 960.0);
        }

        #[test]
        fn status_plugin_ticks_statuses_without_any_other_plugin() {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_message::<crate::status::StatusLifecycleEvent>()
                .add_plugins(StatusPlugin);
            let u = unit(&mut app);
            app.world_mut()
                .get_mut::<StatusEffects>(u)
                .unwrap()
                .effects
                .push(shield_status("a", 50.0, 1.0));
            app.world_mut().get_mut::<ShieldPool>(u).unwrap().amount = 50.0;
            step(&mut app, 0.5);
            assert_eq!(remaining(&app, u).len(), 1);
            step(&mut app, 0.6);
            assert!(remaining(&app, u).is_empty());
            assert_eq!(pool(&app, u), 0.0);
        }
    }

    #[test]
    fn leash_is_dispellable_and_blocked_by_debuff_immunity() {
        let leash = StatusEffect::leashed(
            Leash {
                anchor: LeashAnchor::Point(Vec2::ZERO),
                radius: 300.0,
            },
            5.0,
        );
        assert_eq!(leash.id, "leash");
        assert!(status_dispel_matches(
            &leash,
            StatusKind::Debuff,
            DispelStrength::Basic
        ));
        let tether = StatusEffect::leashed(
            Leash {
                anchor: LeashAnchor::Unit(Entity::PLACEHOLDER),
                radius: 300.0,
            },
            5.0,
        );
        assert_eq!(tether.id, "tether");

        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
        let mut statuses = StatusEffects::default();
        assert!(apply_status(&mut statuses, &mut stats, leash.clone(), 0.0));
        assert_eq!(statuses.leashes().count(), 1);
        remove_statuses(&mut statuses, &mut stats, 0.0, None, |e| {
            status_dispel_matches(e, StatusKind::Debuff, DispelStrength::Basic)
        });
        assert_eq!(
            statuses.leashes().count(),
            0,
            "a basic dispel frees the unit"
        );

        let mut immune = StatusEffects::default();
        assert!(apply_debuff_immunity(&mut immune, &mut stats, 5.0, 0.0));
        assert!(!apply_status(&mut immune, &mut stats, leash, 0.0));
        assert_eq!(immune.leashes().count(), 0);
    }

    mod ticking_burn {
        use super::*;
        use crate::combat::DamageKind;
        use std::time::Duration;

        #[derive(Resource, Default)]
        struct Dealt(Vec<DamageEvent>);

        fn collect(mut reader: MessageReader<DamageEvent>, mut dealt: ResMut<Dealt>) {
            dealt.0.extend(reader.read().cloned());
        }

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .init_resource::<Dealt>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_systems(
                    Update,
                    (apply_status_effect_events, tick_status_effects, collect).chain(),
                );
            app
        }

        fn victim(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                    Health::new(1000.0),
                ))
                .id()
        }

        fn burn(
            app: &mut App,
            target: Entity,
            id: &'static str,
            per_tick: f32,
            every: f32,
            secs: f32,
        ) {
            let mut effect =
                StatusEffect::burn_ticking(id, per_tick, every, DamageType::Magical, secs);
            effect.dot_source = None;
            app.world_mut().write_message(StatusEffectEvent::new(
                None,
                target,
                StatusChange::Apply(effect),
            ));
        }

        fn advance(app: &mut App, dt: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(dt));
            app.update();
        }

        fn dealt(app: &App) -> Vec<DamageEvent> {
            app.world().resource::<Dealt>().0.clone()
        }

        #[test]
        fn sixteen_seconds_every_three_deals_exactly_five_ticks() {
            let mut app = app();
            let target = victim(&mut app);
            burn(&mut app, target, "shadow_strike_burn", 20.0, 3.0, 16.0);
            for _ in 0..340 {
                advance(&mut app, 0.05);
            }
            let hits = dealt(&app);
            assert_eq!(hits.len(), 5);
            assert!(
                hits.iter()
                    .all(|h| h.amount == 20.0 && h.kind == DamageKind::Dot)
            );
            assert!(
                app.world()
                    .get::<StatusEffects>(target)
                    .unwrap()
                    .effects
                    .is_empty(),
                "the burn expired"
            );
        }

        #[test]
        fn first_tick_lands_one_interval_after_application() {
            let mut app = app();
            let target = victim(&mut app);
            burn(&mut app, target, "shadow_strike_burn", 20.0, 3.0, 16.0);
            for _ in 0..58 {
                advance(&mut app, 0.05);
            }
            assert!(dealt(&app).is_empty(), "nothing before 3s");
            for _ in 0..4 {
                advance(&mut app, 0.05);
            }
            assert_eq!(dealt(&app).len(), 1);
        }

        #[test]
        fn uneven_frames_still_deal_fourteen_ticks_in_one_point_four_seconds() {
            let mut app = app();
            let target = victim(&mut app);
            burn(&mut app, target, "sonic_wave_burn", 23.0, 0.1, 1.4);
            let steps = [0.013, 0.071, 0.2, 0.0333, 0.119, 0.0071];
            for i in 0..400 {
                advance(&mut app, steps[i % steps.len()]);
                if app
                    .world()
                    .get::<StatusEffects>(target)
                    .unwrap()
                    .effects
                    .is_empty()
                {
                    break;
                }
            }
            let hits = dealt(&app);
            assert_eq!(hits.len(), 14);
            let total: f32 = hits.iter().map(|h| h.amount).sum();
            assert!((total - 322.0).abs() < 1e-3);
        }

        #[test]
        fn a_long_frame_flushes_every_owed_tick_on_expiry() {
            let mut app = app();
            let target = victim(&mut app);
            burn(&mut app, target, "sonic_wave_burn", 23.0, 0.1, 1.4);
            advance(&mut app, 0.016);
            advance(&mut app, 5.0);
            assert_eq!(dealt(&app).len(), 14);
        }

        #[test]
        fn dispel_stops_the_ticks_without_flushing() {
            let mut app = app();
            let target = victim(&mut app);
            burn(&mut app, target, "shadow_strike_burn", 20.0, 3.0, 16.0);
            for _ in 0..70 {
                advance(&mut app, 0.05);
            }
            assert_eq!(dealt(&app).len(), 1);
            app.world_mut().write_message(StatusEffectEvent::new(
                None,
                target,
                StatusChange::Dispel {
                    kind: StatusKind::Debuff,
                    strength: DispelStrength::Basic,
                },
            ));
            for _ in 0..400 {
                advance(&mut app, 0.05);
            }
            assert_eq!(dealt(&app).len(), 1, "no ticks after the dispel");
        }

        #[test]
        fn burns_from_different_owners_coexist() {
            let mut app = app();
            let target = victim(&mut app);
            burn(&mut app, target, "shadow_strike_burn", 20.0, 3.0, 16.0);
            burn(&mut app, target, "sonic_wave_burn", 23.0, 0.1, 1.4);
            advance(&mut app, 0.016);
            let ids: Vec<_> = app
                .world()
                .get::<StatusEffects>(target)
                .unwrap()
                .effects
                .iter()
                .map(|e| e.id)
                .collect();
            assert_eq!(ids, vec!["shadow_strike_burn", "sonic_wave_burn"]);
            advance(&mut app, 3.5);
            let hits = dealt(&app);
            assert_eq!(hits.iter().filter(|h| h.amount == 23.0).count(), 14);
            assert_eq!(hits.iter().filter(|h| h.amount == 20.0).count(), 1);
        }

        #[test]
        fn reapplying_a_ticking_burn_restarts_its_clock_and_tick_count() {
            let mut app = app();
            let target = victim(&mut app);
            burn(&mut app, target, "shadow_strike_burn", 20.0, 3.0, 16.0);
            // 7s in: ticks at 3s and 6s have landed.
            for _ in 0..140 {
                advance(&mut app, 0.05);
            }
            assert_eq!(dealt(&app).len(), 2);
            // Refresh: the old burn (3 owed ticks) is replaced, not stacked or flushed.
            burn(&mut app, target, "shadow_strike_burn", 30.0, 3.0, 16.0);
            advance(&mut app, 0.016);
            let burns = app
                .world()
                .get::<StatusEffects>(target)
                .unwrap()
                .effects
                .iter()
                .filter(|e| e.id == "shadow_strike_burn")
                .count();
            assert_eq!(burns, 1, "same id refreshes by replacing");
            // 2.5s later (< one interval since the refresh) nothing new has landed.
            for _ in 0..50 {
                advance(&mut app, 0.05);
            }
            assert_eq!(dealt(&app).len(), 2, "the clock restarted at the refresh");
            for _ in 0..400 {
                advance(&mut app, 0.05);
            }
            let hits = dealt(&app);
            assert_eq!(hits.len(), 2 + 5, "a full fresh set of five ticks");
            assert!(hits[2..].iter().all(|h| h.amount == 30.0));
        }

        #[test]
        fn tick_count_is_floor_of_duration_over_interval() {
            let ticks = |every, secs| {
                StatusEffect::burn_ticking("b", 1.0, every, DamageType::Magical, secs)
                    .dot_ticks_left
            };
            assert_eq!(ticks(3.0, 16.0), 5);
            assert_eq!(ticks(0.1, 1.4), 14);
            assert_eq!(ticks(3.0, 2.9), 0, "shorter than one interval: no ticks");
            assert_eq!(
                ticks(0.0, 5.0),
                0,
                "no interval: the continuous form is used"
            );
        }

        #[test]
        fn ticks_are_attributed_to_the_burn_source() {
            let mut app = app();
            let target = victim(&mut app);
            let source = app.world_mut().spawn_empty().id();
            let mut effect =
                StatusEffect::burn_ticking("sonic_wave_burn", 23.0, 0.1, DamageType::Pure, 1.4);
            effect.dot_source = Some(source);
            app.world_mut().write_message(StatusEffectEvent::new(
                Some(source),
                target,
                StatusChange::Apply(effect),
            ));
            advance(&mut app, 0.016);
            advance(&mut app, 2.0);
            let hits = dealt(&app);
            assert_eq!(hits.len(), 14);
            assert!(hits.iter().all(|h| h.source == Some(source)
                && h.target == target
                && h.damage_type == DamageType::Pure));
        }

        #[test]
        fn continuous_burn_is_unchanged() {
            let mut app = app();
            let target = victim(&mut app);
            app.world_mut().write_message(StatusEffectEvent::new(
                None,
                target,
                StatusChange::Apply(StatusEffect::burn(10.0, DamageType::Magical, 2.0)),
            ));
            for _ in 0..10 {
                advance(&mut app, 0.1);
            }
            let hits = dealt(&app);
            assert_eq!(hits.len(), 10, "one slice per frame");
            let total: f32 = hits.iter().map(|h| h.amount).sum();
            assert!((total - 10.0).abs() < 1e-3);
        }
    }

    mod status_resistance {
        use super::*;

        fn app() -> App {
            let mut app = App::new();
            app.add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_systems(Update, apply_status_effect_events);
            app
        }

        fn unit(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0),
                    HeroAttributes::starter(),
                ))
                .id()
        }

        fn apply(app: &mut App, source: Option<Entity>, target: Entity, effect: StatusEffect) {
            app.world_mut().write_message(StatusEffectEvent::new(
                source,
                target,
                StatusChange::Apply(effect),
            ));
            app.update();
        }

        fn remaining(app: &App, unit: Entity, id: &str) -> Option<f32> {
            app.world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .effects
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.remaining)
        }

        #[test]
        fn status_resistance_shortens_only_hostile_debuffs() {
            let mut app = app();
            let enemy = unit(&mut app);
            let holder = unit(&mut app);
            apply(
                &mut app,
                Some(holder),
                holder,
                StatusEffect::status_resistance("res", 0.5, 10.0),
            );
            // Hostile debuffs are halved, whatever their dispel tier.
            apply(&mut app, Some(enemy), holder, StatusEffect::stunned(4.0));
            assert_eq!(remaining(&app, holder, "stun"), Some(2.0));
            // A buff from an enemy, a self-applied debuff and a sourceless debuff are not.
            apply(
                &mut app,
                Some(enemy),
                holder,
                StatusEffect::buff("ally_buff", 4.0),
            );
            assert_eq!(remaining(&app, holder, "ally_buff"), Some(4.0));
            apply(
                &mut app,
                Some(holder),
                holder,
                StatusEffect::debuff("self_debuff", 4.0),
            );
            assert_eq!(remaining(&app, holder, "self_debuff"), Some(4.0));
            apply(&mut app, None, holder, StatusEffect::debuff("env", 4.0));
            assert_eq!(remaining(&app, holder, "env"), Some(4.0));
            // Slow amounts are untouched, only the duration shrinks.
            let slow = StatusEffect {
                move_speed: -100.0,
                ..StatusEffect::debuff("slow", 4.0)
            };
            apply(&mut app, Some(enemy), holder, slow);
            assert_eq!(remaining(&app, holder, "slow"), Some(2.0));
            let stats = app.world().get::<CombatStats>(holder).unwrap();
            assert_eq!(stats.move_speed, 10.0 - 100.0);
            // Another unit is unaffected.
            apply(&mut app, Some(holder), enemy, StatusEffect::stunned(4.0));
            assert_eq!(remaining(&app, enemy, "stun"), Some(4.0));
        }

        #[test]
        fn status_resistance_ignores_debuffs_from_allies() {
            let mut app = app();
            let ally = unit(&mut app);
            let enemy = unit(&mut app);
            let holder = unit(&mut app);
            app.world_mut().entity_mut(ally).insert(Team::Radiant);
            app.world_mut().entity_mut(holder).insert(Team::Radiant);
            app.world_mut().entity_mut(enemy).insert(Team::Dire);
            apply(
                &mut app,
                Some(holder),
                holder,
                StatusEffect::status_resistance("res", 0.5, 10.0),
            );
            apply(&mut app, Some(ally), holder, StatusEffect::stunned(4.0));
            assert_eq!(remaining(&app, holder, "stun"), Some(4.0));
            apply(
                &mut app,
                Some(enemy),
                holder,
                StatusEffect::debuff("hex", 4.0),
            );
            assert_eq!(remaining(&app, holder, "hex"), Some(2.0));
        }

        #[test]
        fn two_resistances_multiply() {
            let mut app = app();
            let enemy = unit(&mut app);
            let holder = unit(&mut app);
            for (id, fraction) in [("a", 0.5), ("b", 0.5)] {
                apply(
                    &mut app,
                    Some(holder),
                    holder,
                    StatusEffect::status_resistance(id, fraction, 10.0),
                );
            }
            apply(&mut app, Some(enemy), holder, StatusEffect::stunned(4.0));
            assert_eq!(remaining(&app, holder, "stun"), Some(1.0));
        }

        #[test]
        fn status_resistance_recomputes_ticking_burn_ticks() {
            let mut app = app();
            let enemy = unit(&mut app);
            let holder = unit(&mut app);
            apply(
                &mut app,
                Some(holder),
                holder,
                StatusEffect::status_resistance("res", 0.5, 10.0),
            );
            let burn = StatusEffect::burn_ticking("b", 10.0, 1.0, DamageType::Magical, 8.0);
            assert_eq!(burn.dot_ticks_left, 8);
            apply(&mut app, Some(enemy), holder, burn);
            let status = app
                .world()
                .get::<StatusEffects>(holder)
                .unwrap()
                .effects
                .iter()
                .find(|e| e.id == "b")
                .unwrap()
                .clone();
            assert_eq!(status.remaining, 4.0);
            assert_eq!(status.dot_ticks_left, 4);
        }

        #[test]
        fn status_resistance_has_no_effect_after_it_ends() {
            let mut app = app();
            let enemy = unit(&mut app);
            let holder = unit(&mut app);
            apply(
                &mut app,
                Some(holder),
                holder,
                StatusEffect::status_resistance("res", 0.5, 10.0),
            );
            app.world_mut()
                .get_mut::<StatusEffects>(holder)
                .unwrap()
                .effects[0]
                .remaining = 0.0;
            assert_eq!(
                app.world()
                    .get::<StatusEffects>(holder)
                    .unwrap()
                    .status_resistance_factor(),
                1.0
            );
            apply(&mut app, Some(enemy), holder, StatusEffect::stunned(4.0));
            assert_eq!(remaining(&app, holder, "stun"), Some(4.0));
        }
    }

    mod attack_charges {
        use super::*;
        use crate::basic_attack::BasicAttackReleaseEvent;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_message::<BasicAttackReleaseEvent>()
                .add_systems(
                    Update,
                    (
                        consume_attack_charges,
                        apply_status_effect_events,
                        tick_status_effects,
                    )
                        .chain(),
                );
            app
        }

        fn unit(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0),
                    HeroAttributes::starter(),
                    Health::new(500.0),
                ))
                .id()
        }

        fn charged(charges: u32, duration: f32) -> StatusEffect {
            StatusEffect {
                attack_speed_flat: 400.0,
                attack_charges: charges,
                ..StatusEffect::buff("charged", duration)
            }
        }

        fn give(app: &mut App, unit: Entity, effect: StatusEffect) {
            app.world_mut().write_message(StatusEffectEvent::new(
                Some(unit),
                unit,
                StatusChange::Apply(effect),
            ));
            frame(app, 0.0);
        }

        fn frame(app: &mut App, dt: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(dt));
            app.update();
        }

        fn release(app: &mut App, attacker: Entity, extra: bool) {
            app.world_mut().write_message(BasicAttackReleaseEvent {
                attacker,
                target: attacker,
                damage: 10.0,
                extra,
            });
            frame(app, 0.0);
        }

        fn charges(app: &App, unit: Entity) -> Option<u32> {
            app.world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .effects
                .iter()
                .find(|e| e.id == "charged")
                .map(|e| e.attack_charges)
        }

        fn rating(app: &App, unit: Entity) -> f32 {
            app.world()
                .get::<CombatStats>(unit)
                .unwrap()
                .attack_speed_flat
        }

        #[test]
        fn attack_charges_are_consumed_per_release_and_revert_at_zero() {
            let mut app = app();
            let unit = unit(&mut app);
            let base = rating(&app, unit);
            give(&mut app, unit, charged(3, 20.0));
            assert_eq!(rating(&app, unit), base + 400.0);
            release(&mut app, unit, false);
            release(&mut app, unit, false);
            assert_eq!(charges(&app, unit), Some(1));
            assert_eq!(rating(&app, unit), base + 400.0, "two of three used");
            release(&mut app, unit, false);
            assert_eq!(charges(&app, unit), None, "the third attack ends it");
            assert_eq!(rating(&app, unit), base, "stat delta reverted");
            // A fourth attack finds nothing to consume.
            release(&mut app, unit, false);
            assert_eq!(rating(&app, unit), base);
        }

        #[test]
        fn extra_attacks_do_not_consume_charges() {
            let mut app = app();
            let unit = unit(&mut app);
            give(&mut app, unit, charged(2, 20.0));
            release(&mut app, unit, true);
            release(&mut app, unit, true);
            assert_eq!(charges(&app, unit), Some(2));
        }

        #[test]
        fn recast_resets_attack_charges() {
            let mut app = app();
            let unit = unit(&mut app);
            let base = rating(&app, unit);
            give(&mut app, unit, charged(3, 20.0));
            release(&mut app, unit, false);
            release(&mut app, unit, false);
            assert_eq!(charges(&app, unit), Some(1));
            give(&mut app, unit, charged(3, 20.0));
            assert_eq!(charges(&app, unit), Some(3));
            assert_eq!(rating(&app, unit), base + 400.0, "refreshed, not stacked");
        }

        #[test]
        fn charged_status_still_expires_on_duration() {
            let mut app = app();
            let unit = unit(&mut app);
            let base = rating(&app, unit);
            give(&mut app, unit, charged(5, 1.0));
            release(&mut app, unit, false);
            frame(&mut app, 1.5);
            assert_eq!(charges(&app, unit), None);
            assert_eq!(rating(&app, unit), base);
        }

        #[test]
        fn expiry_by_charges_is_reported_as_expired() {
            let mut app = app();
            let unit = unit(&mut app);
            give(&mut app, unit, charged(1, 20.0));
            app.world_mut()
                .resource_mut::<Messages<StatusLifecycleEvent>>()
                .clear();
            release(&mut app, unit, false);
            let events: Vec<_> = app
                .world()
                .resource::<Messages<StatusLifecycleEvent>>()
                .iter_current_update_messages()
                .map(|e| (e.id, e.change))
                .collect();
            assert_eq!(events, [("charged", StatusLifecycle::Expired)]);
        }

        #[test]
        fn statuses_without_charges_are_not_touched() {
            let mut app = app();
            let unit = unit(&mut app);
            give(&mut app, unit, StatusEffect::buff("plain", 20.0));
            release(&mut app, unit, false);
            assert!(
                app.world()
                    .get::<StatusEffects>(unit)
                    .unwrap()
                    .effects
                    .iter()
                    .any(|e| e.id == "plain")
            );
        }
    }

    mod invisibility {
        use super::*;
        use crate::actions::{ActionFiredEvent, ActionId, ActionSource};
        use crate::basic_attack::BasicAttackReleaseEvent;
        use crate::components::CastTarget;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_message::<BasicAttackReleaseEvent>()
                .add_message::<ActionFiredEvent>()
                .add_systems(
                    Update,
                    (break_invisibility, apply_status_effect_events).chain(),
                );
            app
        }

        fn unit(app: &mut App, effects: Vec<StatusEffect>) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects { effects },
                    CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0),
                    HeroAttributes::starter(),
                    Health::new(500.0),
                ))
                .id()
        }

        fn invisible(app: &App, unit: Entity) -> bool {
            app.world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .is_concealed()
        }

        #[test]
        fn revealed_invisible_is_not_concealed() {
            let mut statuses = StatusEffects {
                effects: vec![StatusEffect::invisibility(5.0, true)],
            };
            assert!(statuses.is_concealed());
            assert!(statuses.rejects(TargetIntent::Attack));
            assert!(statuses.rejects(TargetIntent::HostileSpell));
            assert!(!statuses.rejects(TargetIntent::Any), "areas still hit it");
            statuses.effects.push(StatusEffect::reveal(5.0));
            assert!(!statuses.is_concealed());
            assert!(!statuses.rejects(TargetIntent::Attack));
            // An expired reveal no longer voids it.
            statuses.effects[1].remaining = 0.0;
            assert!(statuses.is_concealed());
            assert!(!StatusEffects::default().is_concealed());
        }

        #[test]
        fn attack_release_breaks_invisibility() {
            let mut app = app();
            let unit = unit(&mut app, vec![StatusEffect::invisibility(30.0, true)]);
            let other = unit_with_nothing(&mut app);
            app.world_mut().write_message(BasicAttackReleaseEvent {
                attacker: other,
                target: unit,
                damage: 1.0,
                extra: false,
            });
            app.update();
            assert!(
                invisible(&app, unit),
                "another unit's attack changes nothing"
            );
            // Extra attacks are not the holder's own action.
            app.world_mut().write_message(BasicAttackReleaseEvent {
                attacker: unit,
                target: other,
                damage: 1.0,
                extra: true,
            });
            app.update();
            assert!(invisible(&app, unit));
            app.world_mut().write_message(BasicAttackReleaseEvent {
                attacker: unit,
                target: other,
                damage: 1.0,
                extra: false,
            });
            app.update();
            assert!(!invisible(&app, unit));
        }

        fn unit_with_nothing(app: &mut App) -> Entity {
            unit(app, vec![])
        }

        fn fire(app: &mut App, caster: Entity) {
            app.world_mut().write_message(ActionFiredEvent {
                caster,
                source: ActionSource::Ability(0),
                action: ActionId::Ability(crate::components::AbilityId::Bolt),
                level: 1,
                target: CastTarget::None,
                aim: Vec3::ZERO,
            });
            app.update();
        }

        #[test]
        fn cast_breaks_invisibility_unless_flagged() {
            let mut app = app();
            let breaks = unit(&mut app, vec![StatusEffect::invisibility(30.0, true)]);
            let keeps = unit(&mut app, vec![StatusEffect::invisibility(30.0, false)]);
            fire(&mut app, breaks);
            fire(&mut app, keeps);
            assert!(!invisible(&app, breaks));
            assert!(invisible(&app, keeps), "breaks_on_action false survives");
        }
    }

    mod stacking {
        use super::*;

        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<Time>()
                .add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_message::<DamageEvent>()
                .add_message::<crate::combat::DamageAppliedEvent>()
                .add_systems(
                    Update,
                    (apply_status_effect_events, tick_status_effects).chain(),
                );
            app
        }

        fn unit(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0),
                    HeroAttributes::starter(),
                    Health::new(500.0),
                ))
                .id()
        }

        fn stack(app: &mut App, target: Entity, duration: f32) {
            app.world_mut().write_message(StatusEffectEvent::new(
                None,
                target,
                StatusChange::Apply(StatusEffect::stack("fury", duration)),
            ));
            frame(app, 0.0);
        }

        fn frame(app: &mut App, dt: f32) {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(dt));
            app.update();
        }

        fn stacks(app: &App, target: Entity) -> u32 {
            app.world()
                .get::<StatusEffects>(target)
                .unwrap()
                .stacks("fury")
        }

        #[test]
        fn stacking_status_adds_and_refreshes() {
            let mut app = app();
            let target = unit(&mut app);
            stack(&mut app, target, 8.0);
            frame(&mut app, 5.0);
            stack(&mut app, target, 8.0);
            assert_eq!(stacks(&app, target), 2);
            let effects = &app.world().get::<StatusEffects>(target).unwrap().effects;
            assert_eq!(effects.len(), 1, "one status carries the count");
            assert!(effects[0].remaining > 7.9, "duration refreshed");
            // Refreshed at t=5 means it survives past the first application's expiry.
            frame(&mut app, 5.0);
            assert_eq!(stacks(&app, target), 2);
            stack(&mut app, target, 8.0);
            assert_eq!(stacks(&app, target), 3);
        }

        #[test]
        fn stacks_are_lost_on_expiry() {
            let mut app = app();
            let target = unit(&mut app);
            stack(&mut app, target, 8.0);
            stack(&mut app, target, 8.0);
            frame(&mut app, 9.0);
            assert_eq!(stacks(&app, target), 0);
            stack(&mut app, target, 8.0);
            assert_eq!(stacks(&app, target), 1, "starts over");
        }

        #[test]
        fn stacks_are_undispellable_and_per_unit() {
            let mut app = app();
            let a = unit(&mut app);
            let b = unit(&mut app);
            stack(&mut app, a, 8.0);
            stack(&mut app, a, 8.0);
            stack(&mut app, b, 8.0);
            app.world_mut().write_message(StatusEffectEvent::new(
                None,
                a,
                StatusChange::Dispel {
                    kind: StatusKind::Debuff,
                    strength: DispelStrength::Strong,
                },
            ));
            frame(&mut app, 0.0);
            assert_eq!(stacks(&app, a), 2);
            assert_eq!(stacks(&app, b), 1);
        }

        #[test]
        fn debuff_immunity_blocks_new_stacks() {
            let mut app = app();
            let target = unit(&mut app);
            stack(&mut app, target, 8.0);
            app.world_mut().write_message(StatusEffectEvent::new(
                None,
                target,
                StatusChange::Apply(StatusEffect::debuff_immunity(5.0)),
            ));
            frame(&mut app, 0.0);
            stack(&mut app, target, 8.0);
            assert_eq!(stacks(&app, target), 1);
        }
    }
    mod new_status_gaps {
        use super::*;

        fn app() -> App {
            let mut app = App::new();
            app.add_message::<StatusEffectEvent>()
                .add_message::<StatusLifecycleEvent>()
                .add_systems(Update, apply_status_effect_events);
            app
        }

        fn unit(app: &mut App) -> Entity {
            app.world_mut()
                .spawn((
                    StatusEffects::default(),
                    CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0),
                    HeroAttributes::starter(),
                ))
                .id()
        }

        fn send(app: &mut App, source: Option<Entity>, target: Entity, change: StatusChange) {
            app.world_mut()
                .write_message(StatusEffectEvent::new(source, target, change));
            app.update();
        }

        fn has(app: &App, unit: Entity, id: &str) -> bool {
            app.world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .effects
                .iter()
                .any(|e| e.id == id)
        }

        fn remaining(app: &App, unit: Entity, id: &str) -> Option<f32> {
            app.world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .effects
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.remaining)
        }

        #[test]
        fn basic_dispel_removes_reveal_taunt_and_fear() {
            let mut app = app();
            let source = unit(&mut app);
            let victim = unit(&mut app);
            for effect in [
                StatusEffect::reveal(5.0),
                StatusEffect::taunted(5.0, source),
                StatusEffect::fear(5.0, Some(source)),
            ] {
                send(&mut app, Some(source), victim, StatusChange::Apply(effect));
            }
            assert!(has(&app, victim, "reveal") && has(&app, victim, "taunt"));
            assert!(has(&app, victim, "fear"));
            send(
                &mut app,
                None,
                victim,
                StatusChange::Dispel {
                    kind: StatusKind::Debuff,
                    strength: DispelStrength::Basic,
                },
            );
            for id in ["reveal", "taunt", "fear"] {
                assert!(!has(&app, victim, id), "{id} survived a basic dispel");
            }
            assert!(
                !app.world()
                    .get::<StatusEffects>(victim)
                    .unwrap()
                    .is_asleep(),
                "sanity"
            );
        }

        #[test]
        fn banish_and_spell_immunity_survive_strong_dispels() {
            let mut app = app();
            let source = unit(&mut app);
            let banished = unit(&mut app);
            let immune = unit(&mut app);
            send(
                &mut app,
                Some(source),
                banished,
                StatusChange::Apply(StatusEffect::banish(5.0, true)),
            );
            send(
                &mut app,
                Some(immune),
                immune,
                StatusChange::Apply(StatusEffect::spell_immunity(5.0)),
            );
            for (target, kind) in [(banished, StatusKind::Debuff), (immune, StatusKind::Buff)] {
                for strength in [DispelStrength::Basic, DispelStrength::Strong] {
                    send(
                        &mut app,
                        None,
                        target,
                        StatusChange::Dispel { kind, strength },
                    );
                }
            }
            assert!(has(&app, banished, "banish"));
            assert!(has(&app, immune, "spell_immunity"));
        }

        #[test]
        fn status_resistance_shortens_taunt_fear_hypnosis_and_reveal() {
            let mut app = app();
            let enemy = unit(&mut app);
            let holder = unit(&mut app);
            send(
                &mut app,
                Some(holder),
                holder,
                StatusChange::Apply(StatusEffect::status_resistance("res", 0.5, 20.0)),
            );
            for effect in [
                StatusEffect::taunted(4.0, enemy),
                StatusEffect::fear(4.0, Some(enemy)),
                StatusEffect::hypnotized(4.0, enemy),
                StatusEffect::reveal(4.0),
            ] {
                let id = effect.id;
                send(&mut app, Some(enemy), holder, StatusChange::Apply(effect));
                assert_eq!(remaining(&app, holder, id), Some(2.0), "{id}");
            }
        }

        #[test]
        fn stun_overlapping_sleep_survives_damage_wake() {
            let mut statuses = StatusEffects {
                effects: vec![StatusEffect::sleep(5.0), StatusEffect::stunned(5.0)],
            };
            let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0);
            let removed = wake_from_sleep(&mut statuses, &mut stats, 0.0, None);
            assert_eq!(removed.len(), 1);
            assert!(!statuses.is_asleep());
            assert!(statuses.is_stunned(), "the stun outlives the sleep");
            let blocks = statuses.blocks();
            for flag in [
                ActionBlocks::MOVE,
                ActionBlocks::ATTACK,
                ActionBlocks::CAST,
                ActionBlocks::ITEMS,
            ] {
                assert!(blocks.contains(flag));
            }
        }
    }
}
