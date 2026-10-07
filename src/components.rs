//! Shared gameplay components for Labyrinth's MOBA loop.

use bevy::prelude::*;

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
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Mana {
    pub current: f32,
    pub max: f32,
    pub regen_per_sec: f32,
}

impl Mana {
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

#[derive(Component, Debug, Clone, Copy)]
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
    /// Base attack time in seconds. APS = (IAS/100) / BAT.
    pub base_attack_time: f32,
    /// Cached attacks-per-second after the IAS formula (used by combat/HUD).
    pub attack_speed: f32,
    /// Foreswing before the projectile/hit fires (0..=0.5s).
    pub attack_point: f32,
    /// Backswing after the attack (0..=0.5s); cancelled by new player orders.
    pub attack_backswing: f32,
    /// Yaw turn rate in radians per 0.03 seconds.
    pub turn_rate: f32,
    pub armor: f32,
    pub magic_resist: f32,
    pub move_speed: f32,
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
        let bat = 1.0;
        let ias = (attacks_per_sec * bat * 100.0).clamp(20.0, 700.0);
        let mut stats = Self {
            attack_damage,
            attack_range,
            base_attack_speed: ias,
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
        };
        stats.recompute_attack_speed(0.0);
        stats
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

/// In-progress ability cast point / backswing.
#[derive(Component, Debug, Clone, Copy)]
pub struct AbilityCasting {
    pub slot: usize,
    pub ability: AbilityId,
    pub target: CastTarget,
    pub aim: Vec3,
    /// Time left in the cast point (ability fires when this hits 0).
    pub point_remaining: f32,
    /// After fire, remaining backswing (cancellable).
    pub backswing_remaining: f32,
    pub fired: bool,
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
        }
    }

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
    /// Magic resist per point of Intelligence.
    pub const MR_PER_INT: f32 = 0.12;

    pub fn apply_to(&self, health: &mut Health, mana: &mut Mana, stats: &mut CombatStats) {
        health.max += self.strength * Self::HP_PER_STR;
        health.current = health.current.min(health.max);
        health.regen_per_sec += self.strength * Self::HP_REGEN_PER_STR;
        stats.armor += self.agility * Self::ARMOR_PER_AGI;
        mana.max += self.intelligence * Self::MANA_PER_INT;
        mana.current = mana.current.min(mana.max);
        mana.regen_per_sec += self.intelligence * Self::MANA_REGEN_PER_INT;
        stats.magic_resist += self.intelligence * Self::MR_PER_INT;
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
        stats.magic_resist += d_int * Self::MR_PER_INT;
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

/// Validated cast waiting for the caster to walk into range; re-requested once in range.
#[derive(Component, Debug, Clone, Copy)]
pub struct QueuedAbilityCast {
    pub slot: usize,
    pub ability: AbilityId,
    pub target: CastTarget,
    /// Last known aim (tracks a unit target while it lives).
    pub aim: Vec3,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct MoveTarget {
    pub position: Vec3,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerHero;

#[derive(Component, Debug, Clone, Copy)]
pub struct Creep {
    #[allow(dead_code)]
    pub lane: Lane,
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
            // </hero_generator:ability_display_name>
        }
    }

    pub fn is_ultimate(self) -> bool {
        matches!(
            self,
            // <hero_generator:ability_ultimates>
            AbilityId::Nova | AbilityId::Execute | AbilityId::Meteor | AbilityId::Aegis | AbilityId::Ritual | AbilityId::Cataclysm
            // </hero_generator:ability_ultimates>
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
    pub cooldown_remaining: f32,
    /// Remaining charges for charge-based abilities (`None` = not charge-based).
    pub charges: Option<u32>,
    /// Seconds until the next charge is restored (while below the maximum).
    pub charge_restore_remaining: f32,
    pub toggled: bool,
}

impl AbilityState {
    pub fn fresh(id: AbilityId) -> Self {
        Self {
            id,
            rank: 0,
            cooldown_remaining: 0.0,
            charges: None,
            charge_restore_remaining: 0.0,
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
