//! Shared gameplay components for Labyrinth's MOBA loop.

use bevy::prelude::*;

use crate::vitals::MaxChangeRule;

/// Faction affiliation. Friendly fire is disabled across systems.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Team {
    Radiant,
    Dire,
}

impl Team {
    pub fn enemy(self) -> Self {
        match self {
            Team::Radiant => Team::Dire,
            Team::Dire => Team::Radiant,
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Health {
    pub current: f32,
    pub max: f32,
    pub regen_per_sec: f32,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self {
            current: max,
            max,
            regen_per_sec: 0.0,
        }
    }

    pub fn is_alive(self) -> bool {
        self.current > 0.0
    }

    /// Change `max` by `delta` under `rule`. Never kills a living unit and never revives
    /// a dead one; max stays at least 1.
    pub fn change_max(&mut self, delta: f32, rule: MaxChangeRule) {
        let old_max = self.max;
        self.max = (self.max + delta).max(1.0);
        if !self.is_alive() {
            self.current = self.current.min(self.max);
            return;
        }
        let next = rescale_current(self.current, old_max, self.max, delta, rule);
        self.current = match rule {
            MaxChangeRule::KeepMissing => next,
            // A ratio can round a nearly empty unit down to nothing; keep it alive.
            MaxChangeRule::PreserveRatio => next.max(1.0f32.min(self.max)),
        };
    }
}

/// The new `current` after a max change; shared by [`Health`] and [`Mana`].
fn rescale_current(
    current: f32,
    old_max: f32,
    new_max: f32,
    delta: f32,
    rule: MaxChangeRule,
) -> f32 {
    let next = match rule {
        MaxChangeRule::KeepMissing => current + delta.max(0.0),
        MaxChangeRule::PreserveRatio if old_max > 0.0 => current / old_max * new_max,
        MaxChangeRule::PreserveRatio => current,
    };
    next.min(new_max)
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Mana {
    pub current: f32,
    pub max: f32,
    pub regen_per_sec: f32,
}

impl Mana {
    /// Change `max` by `delta` under `rule`; max stays at least 0.
    pub fn change_max(&mut self, delta: f32, rule: MaxChangeRule) {
        let old_max = self.max;
        self.max = (self.max + delta).max(0.0);
        self.current = rescale_current(self.current, old_max, self.max, delta, rule);
    }

    pub fn new(max: f32, regen_per_sec: f32) -> Self {
        Self {
            current: max,
            max,
            regen_per_sec,
        }
    }

    pub fn try_spend(&mut self, cost: f32) -> bool {
        if self.current >= cost {
            self.current -= cost;
            true
        } else {
            false
        }
    }
}

/// What a cleave instance belongs to. Instances of one source merge; different sources never do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleaveSource {
    /// Every permanent item passive; item ratios stack additively in this one instance.
    Items,
    /// The ability or item whose timed modifier statuses carry the cleave (e.g. Empower).
    Effect(crate::abilities::effects::EffectSource),
    /// A status with no known owner, keyed by its own id.
    Status(&'static str),
}

/// One cleave: `ratio` x the main hit to enemies in its own shape. A circle of `radius`
/// (default `scale::ATTACK_CLEAVE_RADIUS` when 0) or, when `distance` > 0, a trapezoid whose
/// half-width grows from `radius` to `end_radius` over `distance`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CleaveInstance {
    pub source: CleaveSource,
    pub ratio: f32,
    pub radius: f32,
    pub end_radius: f32,
    pub distance: f32,
    /// How many statuses / item copies feed this instance; it is dropped when none is left.
    contributors: i32,
}

/// Most cleave instances a unit can hold at once (items plus up to five timed sources).
/// Further sources are ignored, which keeps `CombatStats` `Copy`.
pub const MAX_CLEAVE_INSTANCES: usize = 6;

/// The cleave instances of a unit, in the order their sources first appeared.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CleaveSet {
    slots: [Option<CleaveInstance>; MAX_CLEAVE_INSTANCES],
}

impl CleaveSet {
    pub fn iter(&self) -> impl Iterator<Item = &CleaveInstance> {
        self.slots.iter().flatten()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get(&self, source: CleaveSource) -> Option<&CleaveInstance> {
        self.iter().find(|c| c.source == source)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// Add a contribution (ratio and shape deltas, `contributors` more feeders) to the
    /// instance of `source`, creating it if needed. A negative `contributors` removes a
    /// contribution; the instance disappears with its last feeder.
    pub fn adjust(
        &mut self,
        source: CleaveSource,
        ratio: f32,
        radius: f32,
        end_radius: f32,
        distance: f32,
        contributors: i32,
    ) {
        let index = match self
            .slots
            .iter()
            .position(|s| s.is_some_and(|c| c.source == source))
        {
            Some(index) => index,
            None => {
                let Some(index) = self.slots.iter().position(Option::is_none) else {
                    return;
                };
                self.slots[index] = Some(CleaveInstance {
                    source,
                    ratio: 0.0,
                    radius: 0.0,
                    end_radius: 0.0,
                    distance: 0.0,
                    contributors: 0,
                });
                index
            }
        };
        let instance = self.slots[index].as_mut().expect("slot was just filled");
        instance.ratio += ratio;
        instance.radius += radius;
        instance.end_radius += end_radius;
        instance.distance += distance;
        instance.contributors += contributors;
        if instance.contributors <= 0 {
            self.slots[index] = None;
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
#[require(
    crate::abilities::effects::TriggerCounters,
    crate::abilities::reactive_triggers::TriggerTracking
)]
pub struct CombatStats {
    /// Auto-attack damage (physical).
    pub attack_damage: f32,
    pub attack_range: f32,
    /// Base attack-speed rating (typically ~100). Part of the IAS formula.
    pub base_attack_speed: f32,
    /// Flat attack-speed bonuses (items, buffs).
    pub attack_speed_flat: f32,
    /// Multiplicative AS bonus (0.0 ⇒ ×1.0).
    pub attack_speed_mult: f32,
    /// Base attack time in seconds: the attack interval at 100 attack speed.
    /// Interval = BAT × 100 / AS, so APS = (AS / 100) / BAT.
    pub base_attack_time: f32,
    /// Cached attacks-per-second after the IAS formula (used by combat/HUD).
    pub attack_speed: f32,
    /// Base foreswing before the projectile/hit fires, at 100 attack speed
    /// (0..=1.0s, `basic_attack::MAX_ATTACK_SWING`). Scaled by [`Self::effective_attack_point`].
    pub attack_point: f32,
    /// Base backswing after the attack, at 100 attack speed (0..=1.0s,
    /// `basic_attack::MAX_ATTACK_SWING`); cancelled by new player orders. Scaled by
    /// [`Self::effective_attack_backswing`].
    pub attack_backswing: f32,
    /// Yaw turn rate in radians per 0.03 seconds.
    pub turn_rate: f32,
    pub armor: f32,
    pub magic_resist: f32,
    pub move_speed: f32,
    /// Chance to crit on flagged hits (0..=1).
    pub crit_chance: f32,
    /// Multiplier applied on crit (default 2.0).
    pub crit_multiplier: f32,
    /// Additive bonus: physical damage × (1 + amp).
    pub outgoing_physical_amp: f32,
    /// Additive bonus: magical spell damage × (1 + amp).
    pub outgoing_spell_amp: f32,
    /// Additive bonus applied after mitigation on incoming damage.
    pub incoming_damage_amp: f32,
    /// Flat damage subtracted once per hit (after shields).
    pub damage_block: f32,
    /// Fraction of mitigated damage returned to the attacker.
    pub damage_reflect: f32,
    /// Independent cleave instances, one per source (see [`CleaveSet`]). Each deals its own
    /// ratio × the main hit to its own area; a unit inside two areas takes both.
    pub cleaves: CleaveSet,
    /// Chance (0..=1) that a basic attack against this unit misses. Stacks with statuses.
    pub evasion: f32,
    /// Chance (0..=1) that an attack which would miss lands anyway. 1.0 is true strike.
    pub accuracy: f32,
    /// Enemies within this radius of the struck unit take `splash_damage_ratio` of the
    /// attack's damage. Zero uses [`crate::scale::ATTACK_SPLASH_RADIUS`].
    pub splash_radius: f32,
    pub splash_damage_ratio: f32,
    /// Part of `attack_range` that came from modifiers; `attack_range - bonus` is the base
    /// range, which decides melee versus ranged.
    pub attack_range_bonus: f32,
    /// Added to the attack projectile speed (units / second).
    pub attack_projectile_speed_bonus: f32,
    /// Fraction of basic-attack damage dealt that heals the attacker.
    pub lifesteal: f32,
    /// Fraction of spell damage dealt that heals the caster.
    pub spell_lifesteal: f32,
    /// Additive bonus to healing this unit provides.
    pub heal_amp: f32,
    /// Additive bonus to healing this unit receives.
    pub heal_received_amp: f32,
    /// Additive change to mana costs (-0.25 = 25% cheaper).
    pub mana_cost_amp: f32,
}

/// Unified HP shield consumed before health (all damage types unless pierced).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct ShieldPool {
    pub amount: f32,
}

impl CombatStats {
    /// Simple kit for creeps/towers/tests (no hero agility).
    pub fn simple(
        attack_damage: f32,
        attack_range: f32,
        attacks_per_sec: f32,
        armor: f32,
        magic_resist: f32,
        move_speed: f32,
    ) -> Self {
        // The rate lives in BAT at a neutral 100 attack speed, so the attack point and
        // backswing keep their base durations: interval = BAT × 100 / AS = 1 / attacks_per_sec.
        let bat = if attacks_per_sec > 0.0 {
            1.0 / attacks_per_sec
        } else {
            1.0
        };
        let mut stats = Self {
            attack_damage,
            attack_range,
            base_attack_speed: 100.0,
            attack_speed_flat: 0.0,
            attack_speed_mult: 0.0,
            base_attack_time: bat,
            attack_speed: attacks_per_sec,
            attack_point: 0.2,
            attack_backswing: 0.25,
            turn_rate: crate::facing::CREEP_TURN_RATE,
            armor,
            magic_resist,
            move_speed,
            crit_chance: 0.0,
            crit_multiplier: 2.0,
            outgoing_physical_amp: 0.0,
            outgoing_spell_amp: 0.0,
            incoming_damage_amp: 0.0,
            damage_block: 0.0,
            damage_reflect: 0.0,
            cleaves: CleaveSet::default(),
            evasion: 0.0,
            accuracy: 0.0,
            splash_radius: 0.0,
            splash_damage_ratio: 0.0,
            attack_range_bonus: 0.0,
            attack_projectile_speed_bonus: 0.0,
            lifesteal: 0.0,
            spell_lifesteal: 0.0,
            heal_amp: 0.0,
            heal_received_amp: 0.0,
            mana_cost_amp: 0.0,
        };
        stats.recompute_attack_speed(0.0);
        stats
    }

    /// Attack range before range modifiers (items, statuses).
    pub fn base_attack_range(&self) -> f32 {
        self.attack_range - self.attack_range_bonus
    }

    /// Melee units cleave and swing a slash; ranged ones fire a projectile. Decided from the
    /// base range so a range bonus never turns a melee unit into a ranged one.
    pub fn is_melee(&self) -> bool {
        crate::scale::is_melee_attack_range(self.base_attack_range())
    }

    /// `(base + agi + flat) × (1 + mult)`, clamped to 20..=700.
    pub fn attack_speed_rating(&self, agility: f32) -> f32 {
        ((self.base_attack_speed + agility + self.attack_speed_flat)
            * (1.0 + self.attack_speed_mult))
            .clamp(20.0, 700.0)
    }

    /// Refresh cached APS from the IAS formula.
    pub fn recompute_attack_speed(&mut self, agility: f32) {
        let ias = self.attack_speed_rating(agility);
        self.attack_speed = (ias / 100.0) / self.base_attack_time.max(0.01);
    }

    /// Move speed used for walking: the stat with every status modifier applied, but
    /// never below [`crate::scale::MIN_MOVE_SPEED`] so stacked slows cannot silently
    /// root a unit. There is no upper cap except a status cap (hex, see
    /// [`crate::status::StatusEffects::move_speed_cap`]). Units whose base speed is zero (wards and
    /// other stationary units) stay stationary whatever bonuses they receive.
    pub fn effective_move_speed(&self, statuses: Option<&crate::status::StatusEffects>) -> f32 {
        let from_statuses: f32 =
            statuses.map_or(0.0, |s| s.effects.iter().map(|e| e.move_speed).sum());
        let base = self.move_speed - from_statuses;
        if base <= 0.0 {
            return 0.0;
        }
        let speed = self.move_speed.max(crate::scale::MIN_MOVE_SPEED);
        match statuses.and_then(|s| s.move_speed_cap()) {
            Some(cap) => speed.min(cap),
            None => speed,
        }
    }

    /// Current attack speed as a multiple of 100 (`AS / 100`), recovered from the cached
    /// attacks-per-second: `APS × BAT`. 1.0 at 100 attack speed, 5.0 at 500.
    pub fn attack_speed_ratio(&self) -> f32 {
        (self.attack_speed * self.base_attack_time).max(0.2)
    }

    /// Seconds from the start of one attack to the start of the next: `BAT × 100 / AS`.
    pub fn attack_interval(&self) -> f32 {
        1.0 / self.attack_speed.max(0.05)
    }

    /// Basic-attack foreswing: the base attack point (the lowest live status override, see
    /// [`crate::status::StatusEffects::attack_point_override`], else `attack_point`, clamped to
    /// [`crate::basic_attack::MAX_ATTACK_SWING`]) scaled by attack speed:
    /// `base × 100 / AS`. The one place the override is read.
    pub fn effective_attack_point(&self, statuses: Option<&crate::status::StatusEffects>) -> f32 {
        statuses
            .and_then(|s| s.attack_point_override())
            .unwrap_or(self.attack_point)
            .clamp(0.0, crate::basic_attack::MAX_ATTACK_SWING)
            / self.attack_speed_ratio()
    }

    /// Basic-attack backswing scaled by attack speed like the attack point: `base × 100 / AS`.
    pub fn effective_attack_backswing(&self) -> f32 {
        self.attack_backswing
            .clamp(0.0, crate::basic_attack::MAX_ATTACK_SWING)
            / self.attack_speed_ratio()
    }

    /// Turn rate after stacking status bonuses and slow on top of the hero/creep base in
    /// `turn_rate`: `base × (1 + Σ bonus / 100) × (1 − strongest slow / 100)`.
    pub fn effective_turn_rate(&self, statuses: Option<&crate::status::StatusEffects>) -> f32 {
        let base = self.turn_rate.max(0.01);
        let bonus_pct: f32 = statuses.map_or(0.0, |s| {
            s.effects
                .iter()
                .filter(|e| e.remaining > 0.0)
                .map(|e| e.turn_rate_bonus_percent)
                .sum()
        });
        let base = base * (1.0 + bonus_pct.max(0.0) / 100.0);
        let slow_pct = statuses
            .map(|s| {
                s.effects
                    .iter()
                    .filter(|e| e.remaining > 0.0)
                    .fold(0.0_f32, |acc, e| acc.max(e.turn_rate_slow_percent))
            })
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        base * (1.0 - slow_pct / 100.0).max(0.0)
    }
}

/// In-progress auto-attack foreswing / backswing.
#[derive(Component, Debug, Clone, Copy)]
pub enum AttackSwing {
    Windup {
        remaining: f32,
        target: Entity,
        damage: f32,
    },
    Backswing {
        remaining: f32,
    },
}

/// What an ability cast is aimed at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CastTarget {
    None,
    Unit(Entity),
    Point(Vec3),
}

impl CastTarget {
    pub fn unit(self) -> Option<Entity> {
        match self {
            CastTarget::Unit(entity) => Some(entity),
            _ => None,
        }
    }
}

/// Primary hero attributes. Level-ups raise these; each point feeds derived combat stats.
#[derive(Component, Debug, Clone, Copy)]
pub struct HeroAttributes {
    pub strength: f32,
    pub agility: f32,
    pub intelligence: f32,
    pub str_per_level: f32,
    pub agi_per_level: f32,
    pub int_per_level: f32,
    /// The primary attribute: each point adds one attack damage. `None` for attribute sets
    /// without a primary (the test starter).
    pub primary: Option<crate::abilities::definition::Attribute>,
}

impl HeroAttributes {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn starter() -> Self {
        Self {
            strength: 20.0,
            agility: 18.0,
            intelligence: 18.0,
            str_per_level: 2.2,
            agi_per_level: 2.0,
            int_per_level: 2.4,
            primary: None,
        }
    }

    /// Current value of one attribute.
    pub fn get(&self, attribute: crate::abilities::definition::Attribute) -> f32 {
        use crate::abilities::definition::Attribute;
        match attribute {
            Attribute::Strength => self.strength,
            Attribute::Agility => self.agility,
            Attribute::Intelligence => self.intelligence,
        }
    }

    /// Current value of the primary attribute (0 without one).
    pub fn primary_value(&self) -> f32 {
        self.primary.map_or(0.0, |attribute| self.get(attribute))
    }

    /// Attack damage per point of the primary attribute.
    pub const DAMAGE_PER_PRIMARY: f32 = 1.0;

    /// Max HP gained per point of Strength.
    pub const HP_PER_STR: f32 = 18.0;
    /// HP regen per point of Strength.
    pub const HP_REGEN_PER_STR: f32 = 0.12;
    /// Armor per point of Agility.
    pub const ARMOR_PER_AGI: f32 = 0.14;
    /// Max mana per point of Intelligence.
    pub const MANA_PER_INT: f32 = 12.0;
    /// Mana regen per point of Intelligence.
    pub const MANA_REGEN_PER_INT: f32 = 0.05;

    pub fn apply_to(&self, health: &mut Health, mana: &mut Mana, stats: &mut CombatStats) {
        health.max += self.strength * Self::HP_PER_STR;
        health.current = health.current.min(health.max);
        health.regen_per_sec += self.strength * Self::HP_REGEN_PER_STR;
        stats.armor += self.agility * Self::ARMOR_PER_AGI;
        mana.max += self.intelligence * Self::MANA_PER_INT;
        mana.current = mana.current.min(mana.max);
        mana.regen_per_sec += self.intelligence * Self::MANA_REGEN_PER_INT;
        stats.attack_damage += self.primary_value() * Self::DAMAGE_PER_PRIMARY;
        stats.recompute_attack_speed(self.agility);
    }

    /// Apply only the delta between `before` and `self` (used on level-up).
    pub fn apply_delta(
        before: &Self,
        after: &Self,
        health: &mut Health,
        mana: &mut Mana,
        stats: &mut CombatStats,
    ) {
        let d_str = after.strength - before.strength;
        let d_agi = after.agility - before.agility;
        let d_int = after.intelligence - before.intelligence;
        let hp = d_str * Self::HP_PER_STR;
        health.max += hp;
        health.current = (health.current + hp).min(health.max);
        health.regen_per_sec += d_str * Self::HP_REGEN_PER_STR;
        stats.armor += d_agi * Self::ARMOR_PER_AGI;
        let mp = d_int * Self::MANA_PER_INT;
        mana.max += mp;
        mana.current = (mana.current + mp).min(mana.max);
        mana.regen_per_sec += d_int * Self::MANA_REGEN_PER_INT;
        stats.attack_damage +=
            (after.primary_value() - before.primary_value()) * Self::DAMAGE_PER_PRIMARY;
        stats.recompute_attack_speed(after.agility);
    }

    pub fn level_up(&mut self) {
        self.strength += self.str_per_level;
        self.agility += self.agi_per_level;
        self.intelligence += self.int_per_level;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageType {
    Physical,
    Magical,
    /// Ignores armor and magic resist.
    #[allow(dead_code)]
    Pure,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct AttackCooldown(pub f32);

#[derive(Component, Debug, Clone, Copy)]
pub struct AttackTarget(pub Entity);

/// Attack-move: path toward `destination` while auto-acquiring enemies in attack range.
#[derive(Component, Debug, Clone, Copy)]
pub struct AttackMoveOrder {
    pub destination: Vec3,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct MoveTarget {
    pub position: Vec3,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerHero;

#[derive(Component, Debug, Clone, Copy)]
pub struct Creep {
    pub lane: Lane,
    /// Ranged (as opposed to melee) lane creep; a converted creep respawns as the same kind.
    pub ranged: bool,
}

/// Practice target near the Radiant ancient. It is not a lane creep: nothing moves it
/// or makes it act.
#[derive(Component, Debug, Clone, Copy)]
pub struct TrainingDummy;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    Top,
    Mid,
    Bot,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Tower;

#[derive(Component, Debug, Clone, Copy)]
pub struct Ancient;

#[derive(Component, Debug, Clone, Copy)]
pub struct Ground;

/// Gameplay extent for range, targeting, and AoE queries (edge-to-edge).
/// Independent of [`CollisionRadius`]; see [`crate::dimensions`].
#[derive(Component, Debug, Clone, Copy)]
pub struct BoundRadius(pub f32);

/// Physical footprint for unit-to-unit and pathing collision.
/// Independent of [`BoundRadius`]; see [`crate::dimensions`].
#[derive(Component, Debug, Clone, Copy)]
pub struct CollisionRadius(pub f32);

/// World-aligned picking box for hover / left-click / right-click targeting,
/// centered at the unit origin plus `offset`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct SelectionBounds {
    pub offset: Vec3,
    pub half_extents: Vec3,
}

impl SelectionBounds {
    pub const fn new(offset: Vec3, half_extents: Vec3) -> Self {
        Self {
            offset,
            half_extents,
        }
    }

    /// Height of the box top above the unit origin.
    pub fn top(&self) -> f32 {
        self.offset.y + self.half_extents.y
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct GoldBounty(pub u32);

#[derive(Component, Debug, Clone, Copy)]
pub struct XpBounty(pub u32);

#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerWallet {
    pub gold: u32,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct HeroProgress {
    pub level: u32,
    pub xp: u32,
    pub xp_to_next: u32,
    /// Unspent ability rank points.
    pub skill_points: u32,
}

impl HeroProgress {
    pub fn new() -> Self {
        Self {
            level: 1,
            xp: 0,
            xp_to_next: xp_required_for_level(1),
            skill_points: 1,
        }
    }

    /// A fresh hero that has already reached `level`, with one unspent skill point per level.
    pub fn at_level(level: u32) -> Self {
        let level = level.clamp(1, 25);
        Self {
            level,
            xp: 0,
            xp_to_next: if level >= 25 {
                0
            } else {
                xp_required_for_level(level)
            },
            skill_points: level,
        }
    }
}

impl Default for HeroProgress {
    fn default() -> Self {
        Self::new()
    }
}

pub fn xp_required_for_level(level: u32) -> u32 {
    100 + (level.saturating_sub(1)) * 40
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Obstacle {
    pub shape: ObstacleShape,
}

impl Obstacle {
    #[allow(dead_code)]
    pub fn circle(radius: f32) -> Self {
        Self {
            shape: ObstacleShape::Circle { radius },
        }
    }

    pub fn aabb(half_x: f32, half_z: f32) -> Self {
        Self {
            shape: ObstacleShape::Aabb { half_x, half_z },
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum ObstacleShape {
    Circle { radius: f32 },
    Aabb { half_x: f32, half_z: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AbilityId {
    // <hero_generator:ability_enum>
    // Vanguard
    Dash,
    Shockwave,
    Bolt,
    Nova,
    // Skirmisher
    Blink,
    Flurry,
    Caltrops,
    Execute,
    // Arcanist
    ArcMissile,
    FrostNova,
    Barrier,
    Meteor,

    // Warden (generated stubs)
    Bulwark,
    ShieldBash,
    Taunt,
    Aegis,
    // Hexer (generated stubs)
    HexBolt,
    Curse,
    Ward,
    Ritual,
    // AbilityGenerator: Seismic Slam
    SeismicSlam,
    // AbilityGenerator: Arcane Lance
    ArcaneLance,
    // AbilityGenerator: Cataclysm
    Cataclysm,
    // AbilityGenerator: Stone Skin
    StoneSkin,
    // AbilityGenerator: Overcharge
    Overcharge,
    // Queen of Pain (generated stubs)
    ShadowStrike,
    QueenOfPainBlink,
    ScreamOfPain,
    SonicWave,
    // Magnus (generated stubs)
    MagnusShockwave,
    Empower,
    Skewer,
    ReversePolarity,
    // Sniper (generated stubs)
    Shrapnel,
    Headshot,
    TakeAim,
    Assassinate,
    // Ursa (generated stubs)
    Earthshock,
    Overpower,
    FurySwipes,
    Enrage,
    // Slark (generated stubs)
    DarkPact,
    Pounce,
    EssenceShift,
    ShadowDance,
    // Winter Wyvern (generated stubs)
    ArcticBurn,
    SplinterBlast,
    ColdEmbrace,
    WintersCurse,
    // Shadowheart (generated stubs)
    Purification,
    WardingBond,
    SpiritGuardians,
    SharranConversion,
    // </hero_generator:ability_enum>
}

impl AbilityId {
    pub fn display_name(self) -> &'static str {
        match self {
            // <hero_generator:ability_display_name>
            AbilityId::Dash => "Dash",
            AbilityId::Shockwave => "Shockwave",
            AbilityId::Bolt => "Bolt",
            AbilityId::Nova => "Nova",
            AbilityId::Blink => "Blink",
            AbilityId::Flurry => "Flurry",
            AbilityId::Caltrops => "Caltrops",
            AbilityId::Execute => "Execute",
            AbilityId::ArcMissile => "Missile",
            AbilityId::FrostNova => "Frost",
            AbilityId::Barrier => "Barrier",
            AbilityId::Meteor => "Meteor",

            AbilityId::Bulwark => "Bulwark",
            AbilityId::ShieldBash => "ShieldBash",
            AbilityId::Taunt => "Taunt",
            AbilityId::Aegis => "Aegis",
            AbilityId::HexBolt => "HexBolt",
            AbilityId::Curse => "Curse",
            AbilityId::Ward => "Ward",
            AbilityId::Ritual => "Ritual",
            AbilityId::SeismicSlam => "Seismic Slam",
            AbilityId::ArcaneLance => "Arcane Lance",
            AbilityId::Cataclysm => "Cataclysm",
            AbilityId::StoneSkin => "Stone Skin",
            AbilityId::Overcharge => "Overcharge",
            AbilityId::ShadowStrike => "Shadow Strike",
            AbilityId::QueenOfPainBlink => "QoP Blink",
            AbilityId::ScreamOfPain => "Scream of Pain",
            AbilityId::SonicWave => "Sonic Wave",
            AbilityId::MagnusShockwave => "Shockwave",
            AbilityId::Empower => "Empower",
            AbilityId::Skewer => "Skewer",
            AbilityId::ReversePolarity => "Reverse Polarity",
            AbilityId::Shrapnel => "Shrapnel",
            AbilityId::Headshot => "Headshot",
            AbilityId::TakeAim => "Take Aim",
            AbilityId::Assassinate => "Assassinate",
            AbilityId::Earthshock => "Earthshock",
            AbilityId::Overpower => "Overpower",
            AbilityId::FurySwipes => "Fury Swipes",
            AbilityId::Enrage => "Enrage",
            AbilityId::DarkPact => "Dark Pact",
            AbilityId::Pounce => "Pounce",
            AbilityId::EssenceShift => "Essence Shift",
            AbilityId::ShadowDance => "Shadow Dance",
            AbilityId::ArcticBurn => "Arctic Burn",
            AbilityId::SplinterBlast => "Splinter Blast",
            AbilityId::ColdEmbrace => "Cold Embrace",
            AbilityId::WintersCurse => "Winter's Curse",
            AbilityId::Purification => "Purification",
            AbilityId::WardingBond => "Warding Bond",
            AbilityId::SpiritGuardians => "Spirit Guardians",
            AbilityId::SharranConversion => "Sharran Conversion",
            // </hero_generator:ability_display_name>
        }
    }

    pub fn is_ultimate(self) -> bool {
        matches!(
            self,
            // <hero_generator:ability_ultimates>
            AbilityId::Nova
                | AbilityId::Execute
                | AbilityId::Meteor
                | AbilityId::Aegis
                | AbilityId::Ritual
                | AbilityId::Cataclysm
                | AbilityId::SonicWave
                | AbilityId::ReversePolarity
                | AbilityId::Assassinate
                | AbilityId::Enrage
                | AbilityId::ShadowDance
                | AbilityId::WintersCurse
                | AbilityId::SharranConversion // </hero_generator:ability_ultimates>
        )
    }

    pub fn max_rank(self) -> u32 {
        match self {
            // <ability_generator:max_rank>
            AbilityId::SeismicSlam => 7,
            AbilityId::ArcaneLance => 7,
            AbilityId::Cataclysm => 4,
            AbilityId::StoneSkin => 4,
            AbilityId::Overcharge => 7,
            AbilityId::Blink => 3,
            AbilityId::ShadowStrike => 7,
            AbilityId::QueenOfPainBlink => 7,
            AbilityId::ScreamOfPain => 7,
            AbilityId::SonicWave => 4,
            AbilityId::MagnusShockwave => 7,
            AbilityId::Empower => 7,
            AbilityId::Skewer => 7,
            AbilityId::ReversePolarity => 4,
            AbilityId::Shrapnel => 7,
            AbilityId::Headshot => 7,
            AbilityId::TakeAim => 7,
            AbilityId::Assassinate => 4,
            AbilityId::Earthshock => 7,
            AbilityId::Overpower => 7,
            AbilityId::FurySwipes => 7,
            AbilityId::Enrage => 4,
            AbilityId::DarkPact => 7,
            AbilityId::Pounce => 7,
            AbilityId::EssenceShift => 7,
            AbilityId::ShadowDance => 4,
            AbilityId::ArcticBurn => 7,
            AbilityId::SplinterBlast => 7,
            AbilityId::ColdEmbrace => 7,
            AbilityId::WintersCurse => 4,
            AbilityId::Purification => 7,
            AbilityId::WardingBond => 7,
            AbilityId::SpiritGuardians => 7,
            AbilityId::SharranConversion => 4,
            // </ability_generator:max_rank>
            _ if self.is_ultimate() => 4,
            _ => 7,
        }
    }

    /// Whether the next rank can be purchased at `hero_level`.
    /// Basics: rank N requires hero level >= 2N-1 (max at 13).
    /// Ultimates: ranks unlock at 6 / 12 / 18 / 24.
    pub fn can_rank_up(self, current_rank: u32, hero_level: u32) -> bool {
        let next = current_rank + 1;
        if next > self.max_rank() {
            return false;
        }
        if self.is_ultimate() {
            hero_level >= 6 * next
        } else {
            hero_level >= 2 * next - 1
        }
    }

    pub fn placeholder_color(self) -> Color {
        match self {
            AbilityId::Dash | AbilityId::Blink => Color::srgb(0.25, 0.65, 1.0),
            AbilityId::Shockwave | AbilityId::Flurry | AbilityId::FrostNova => {
                Color::srgb(0.2, 0.85, 0.75)
            }
            AbilityId::Bolt | AbilityId::Caltrops | AbilityId::ArcMissile => {
                Color::srgb(0.75, 0.35, 1.0)
            }
            AbilityId::Nova | AbilityId::Execute | AbilityId::Meteor => Color::srgb(1.0, 0.75, 0.2),
            AbilityId::Barrier => Color::srgb(0.45, 0.7, 1.0),
            // Generated / unimplemented kits use a neutral placeholder tint.
            _ => Color::srgb(0.55, 0.6, 0.65),
        }
    }
}

/// Mutable per-hero state of one ability. Static data (costs, targeting, effects)
/// lives in `abilities::AbilityDefinition`, looked up by `id`.
#[derive(Debug, Clone)]
pub struct AbilityState {
    pub id: AbilityId,
    /// Ability level; 0 = unlearned (cannot cast).
    pub rank: u32,
    pub cooldown: crate::actions::ActionCooldown,
    pub toggled: bool,
}

impl AbilityState {
    pub fn fresh(id: AbilityId) -> Self {
        Self {
            id,
            rank: 0,
            cooldown: crate::actions::ActionCooldown::default(),
            toggled: false,
        }
    }
}

#[derive(Component, Debug, Clone)]
pub struct AbilityLoadout {
    pub slots: [AbilityState; 4],
}

impl AbilityLoadout {
    #[allow(dead_code)]
    pub fn starter() -> Self {
        // Default kit for tests / fallback — Vanguard.
        Self::from_abilities([
            AbilityId::Dash,
            AbilityId::Shockwave,
            AbilityId::Bolt,
            AbilityId::Nova,
        ])
    }

    pub fn from_abilities(abilities: [AbilityId; 4]) -> Self {
        Self {
            slots: [
                AbilityState::fresh(abilities[0]),
                AbilityState::fresh(abilities[1]),
                AbilityState::fresh(abilities[2]),
                AbilityState::fresh(abilities[3]),
            ],
        }
    }

    pub fn slot_mut(&mut self, index: usize) -> Option<&mut AbilityState> {
        self.slots.get_mut(index)
    }

    pub fn try_rank_up(&mut self, index: usize, hero_level: u32, skill_points: &mut u32) -> bool {
        if *skill_points == 0 {
            return false;
        }
        let Some(slot) = self.slots.get_mut(index) else {
            return false;
        };
        if !slot.id.can_rank_up(slot.rank, hero_level) {
            return false;
        }
        slot.rank += 1;
        *skill_points -= 1;
        true
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Projectile {
    pub speed: f32,
    pub team: Team,
    /// Collision radius against the homing target.
    pub radius: f32,
    pub lifetime: f32,
    /// Enemies this close to the impact are reported as splashed.
    pub splash_radius: f32,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct ProjectileHome {
    pub target: Entity,
    pub last_pos: Vec3,
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectileStyle {
    AutoAttack,
    SpellBolt,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Lifetime(pub f32);

#[derive(Component, Debug, Clone, Copy)]
pub struct SpellFx {
    pub age: f32,
    pub lifetime: f32,
    pub start_scale: f32,
    pub end_scale: f32,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct HealthBar {
    pub owner: Entity,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct HealthBarFill;

#[derive(Component, Debug, Clone, Copy)]
pub struct HasHealthBar;

/// Screen-space label that tracks a hero above their world health bar.
#[derive(Component, Debug, Clone, Copy)]
pub struct WorldHeroNameLabel {
    pub owner: Entity,
}

/// Marker on the HUD root used to parent [`WorldHeroNameLabel`] entities.
#[derive(Component, Debug, Clone, Copy)]
pub struct WorldNameLayer;

#[cfg(test)]
mod ability_generator_tests {
    use super::*;

    #[test]
    fn generated_abilities_register_identity() {
        assert_eq!(AbilityId::SeismicSlam.display_name(), "Seismic Slam");
        assert_eq!(AbilityId::SeismicSlam.max_rank(), 7);
        assert!(AbilityId::Cataclysm.is_ultimate());
        assert_eq!(AbilityId::Cataclysm.max_rank(), 4);
        assert!(!AbilityId::StoneSkin.is_ultimate());
    }
}

#[cfg(test)]
mod movement_stat_tests {
    use super::*;
    use crate::status::{StatusEffect, StatusEffects, apply_status};

    fn with_statuses(base_speed: f32, effects: Vec<StatusEffect>) -> (CombatStats, StatusEffects) {
        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, base_speed);
        let mut statuses = StatusEffects::default();
        for effect in effects {
            assert!(apply_status(&mut statuses, &mut stats, effect, 0.0));
        }
        (stats, statuses)
    }

    fn flat_speed(id: &'static str, amount: f32) -> StatusEffect {
        StatusEffect {
            move_speed: amount,
            ..StatusEffect::debuff(id, 5.0)
        }
    }

    #[test]
    fn turn_rate_bonus_and_slow_combine() {
        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
        stats.turn_rate = 0.5;
        let mut statuses = StatusEffects::default();
        assert_eq!(stats.effective_turn_rate(Some(&statuses)), 0.5);

        statuses.effects.push(StatusEffect {
            turn_rate_bonus_percent: 100.0,
            ..StatusEffect::buff("turn_rate_bonus", 5.0)
        });
        statuses.effects.push(StatusEffect {
            turn_rate_bonus_percent: 50.0,
            ..StatusEffect::buff("turn_rate_bonus_2", 5.0)
        });
        assert!((stats.effective_turn_rate(Some(&statuses)) - 1.25).abs() < 1e-5);

        statuses.effects.push(StatusEffect {
            turn_rate_slow_percent: 40.0,
            ..StatusEffect::debuff("turn_rate_slow", 5.0)
        });
        // 0.5 × (1 + 1.5) × (1 − 0.4)
        assert!((stats.effective_turn_rate(Some(&statuses)) - 0.75).abs() < 1e-5);
    }

    #[test]
    fn stacked_slows_respect_the_move_speed_floor() {
        let (stats, statuses) = with_statuses(
            300.0,
            vec![
                flat_speed("slow_a", -150.0),
                flat_speed("slow_b", -150.0),
                flat_speed("slow_c", -200.0),
            ],
        );
        assert!(stats.move_speed < 0.0, "the raw stat went negative");
        assert_eq!(
            stats.effective_move_speed(Some(&statuses)),
            crate::scale::MIN_MOVE_SPEED
        );
        let (stats, statuses) = with_statuses(300.0, vec![flat_speed("slow_a", -100.0)]);
        assert_eq!(stats.effective_move_speed(Some(&statuses)), 200.0);
    }

    #[test]
    fn haste_is_not_capped() {
        let (stats, statuses) = with_statuses(
            300.0,
            vec![StatusEffect {
                move_speed: 700.0,
                ..StatusEffect::buff("haste", 5.0)
            }],
        );
        assert_eq!(stats.effective_move_speed(Some(&statuses)), 1000.0);
    }

    #[test]
    fn zero_base_speed_units_stay_stationary_with_haste() {
        let (stats, statuses) = with_statuses(
            0.0,
            vec![StatusEffect {
                move_speed: 200.0,
                ..StatusEffect::buff("haste", 5.0)
            }],
        );
        assert_eq!(stats.move_speed, 200.0);
        assert_eq!(stats.effective_move_speed(Some(&statuses)), 0.0);
        let (stats, statuses) = with_statuses(0.0, vec![flat_speed("slow", -50.0)]);
        assert_eq!(stats.effective_move_speed(Some(&statuses)), 0.0);
    }
}

#[cfg(test)]
mod cleave_set_tests {
    use super::*;
    use crate::abilities::effects::EffectSource;

    const EMPOWER: CleaveSource = CleaveSource::Effect(EffectSource::Ability(AbilityId::Empower));

    #[test]
    fn float_drift_never_leaves_a_ghost_instance() {
        let mut set = CleaveSet::default();
        // Ratios that do not cancel exactly in f32.
        let ratios = [0.1_f32, 0.2, 0.3, 0.7, 0.13, 0.37];
        for r in ratios {
            set.adjust(EMPOWER, r, 0.1, 0.3, 0.7, 1);
        }
        for r in ratios.iter().rev() {
            set.adjust(EMPOWER, -r, -0.1, -0.3, -0.7, -1);
        }
        assert!(
            set.is_empty(),
            "removal is by contributor count, not by value"
        );
    }

    #[test]
    fn removing_a_contribution_that_was_never_added_leaves_nothing_behind() {
        let mut set = CleaveSet::default();
        set.adjust(CleaveSource::Items, -0.4, 0.0, 0.0, 0.0, -1);
        assert!(set.is_empty());
    }

    #[test]
    fn items_and_each_source_get_their_own_instance_in_first_seen_order() {
        let mut set = CleaveSet::default();
        set.adjust(CleaveSource::Items, 0.4, 0.0, 0.0, 0.0, 1);
        set.adjust(EMPOWER, 0.16, 150.0, 360.0, 650.0, 1);
        set.adjust(CleaveSource::Items, 0.4, 0.0, 0.0, 0.0, 1);
        let sources: Vec<_> = set.iter().map(|c| c.source).collect();
        assert_eq!(sources, [CleaveSource::Items, EMPOWER]);
        assert!((set.get(CleaveSource::Items).unwrap().ratio - 0.8).abs() < 1e-6);
        assert_eq!(set.get(CleaveSource::Items).unwrap().distance, 0.0);
    }

    /// Documents the silent cap: a seventh simultaneous source is dropped (and its removal is
    /// a harmless no-op), while the six held instances stay intact.
    #[test]
    fn the_cap_drops_a_seventh_source_silently_and_its_removal_is_harmless() {
        let mut set = CleaveSet::default();
        for i in 0..MAX_CLEAVE_INSTANCES {
            let id: &'static str = ["a", "b", "c", "d", "e", "f"][i];
            set.adjust(CleaveSource::Status(id), 0.1, 0.0, 0.0, 0.0, 1);
        }
        set.adjust(EMPOWER, 0.5, 0.0, 0.0, 0.0, 1);
        assert_eq!(set.iter().count(), MAX_CLEAVE_INSTANCES);
        assert!(set.get(EMPOWER).is_none(), "the overflow source is ignored");
        set.adjust(EMPOWER, -0.5, 0.0, 0.0, 0.0, -1);
        assert_eq!(set.iter().count(), MAX_CLEAVE_INSTANCES);
        assert!(set.iter().all(|c| (c.ratio - 0.1).abs() < 1e-6));
    }

    /// Known limitation: if a source was dropped by the cap and a slot frees before its
    /// removal, the removal creates a phantom instance that vanishes at once (contributors
    /// is -1), so nothing leaks. A later same-source add while the first is still ignored
    /// would be under-counted; this pins the no-leak part.
    #[test]
    fn a_dropped_source_removed_after_a_slot_frees_leaks_nothing() {
        let mut set = CleaveSet::default();
        for id in ["a", "b", "c", "d", "e", "f"] {
            set.adjust(CleaveSource::Status(id), 0.1, 0.0, 0.0, 0.0, 1);
        }
        set.adjust(EMPOWER, 0.5, 0.0, 0.0, 0.0, 1); // dropped
        set.adjust(CleaveSource::Status("a"), -0.1, 0.0, 0.0, 0.0, -1); // frees a slot
        set.adjust(EMPOWER, -0.5, 0.0, 0.0, 0.0, -1);
        assert_eq!(set.iter().count(), 5);
        assert!(set.get(EMPOWER).is_none());
    }
}
