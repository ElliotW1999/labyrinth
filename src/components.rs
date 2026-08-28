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
    pub attack_speed: f32,
    pub armor: f32,
    pub magic_resist: f32,
    pub move_speed: f32,
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
    /// Attack speed per point of Agility.
    pub const ATTACK_SPEED_PER_AGI: f32 = 0.02;
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
        stats.attack_speed += self.agility * Self::ATTACK_SPEED_PER_AGI;
        mana.max += self.intelligence * Self::MANA_PER_INT;
        mana.current = mana.current.min(mana.max);
        mana.regen_per_sec += self.intelligence * Self::MANA_REGEN_PER_INT;
        stats.magic_resist += self.intelligence * Self::MR_PER_INT;
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
        stats.attack_speed += d_agi * Self::ATTACK_SPEED_PER_AGI;
        let mp = d_int * Self::MANA_PER_INT;
        mana.max += mp;
        mana.current = (mana.current + mp).min(mana.max);
        mana.regen_per_sec += d_int * Self::MANA_REGEN_PER_INT;
        stats.magic_resist += d_int * Self::MR_PER_INT;
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
}

/// How an ability is activated from the hotkey.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityCastKind {
    Instant,
    Targeted { cast_range: f32, aoe_radius: f32 },
}

#[derive(Component, Debug, Clone, Copy)]
pub struct AttackCooldown(pub f32);

#[derive(Component, Debug, Clone, Copy)]
pub struct AttackTarget(pub Entity);

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

#[derive(Component, Debug, Clone, Copy)]
pub struct UnitRadius(pub f32);

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
    pub radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityId {
    Dash,
    Shockwave,
    Bolt,
    Nova,
}

impl AbilityId {
    pub fn hotkey_label(self) -> &'static str {
        match self {
            AbilityId::Dash => "Q",
            AbilityId::Shockwave => "W",
            AbilityId::Bolt => "E",
            AbilityId::Nova => "R",
        }
    }

    pub fn max_rank(self) -> u32 {
        match self {
            AbilityId::Nova => 4,
            _ => 7,
        }
    }

    /// Whether the next rank can be purchased at `hero_level`.
    /// Q/W/E: rank N requires hero level >= 2N-1 (max at 13).
    /// R: ranks unlock at 6 / 12 / 18 / 24.
    pub fn can_rank_up(self, current_rank: u32, hero_level: u32) -> bool {
        let next = current_rank + 1;
        if next > self.max_rank() {
            return false;
        }
        match self {
            AbilityId::Nova => hero_level >= 6 * next,
            _ => hero_level >= 2 * next - 1,
        }
    }

    pub fn placeholder_color(self) -> Color {
        match self {
            AbilityId::Dash => Color::srgb(0.25, 0.65, 1.0),
            AbilityId::Shockwave => Color::srgb(0.2, 0.85, 0.75),
            AbilityId::Bolt => Color::srgb(0.75, 0.35, 1.0),
            AbilityId::Nova => Color::srgb(1.0, 0.75, 0.2),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AbilitySlot {
    pub id: AbilityId,
    /// 0 = unlearned (cannot cast).
    pub rank: u32,
    pub cooldown_remaining: f32,
    pub cooldown: f32,
    pub mana_cost: f32,
}

impl AbilitySlot {
    pub fn fresh(id: AbilityId) -> Self {
        let mut slot = Self {
            id,
            rank: 0,
            cooldown_remaining: 0.0,
            cooldown: 0.0,
            mana_cost: 0.0,
        };
        slot.refresh_stats();
        slot
    }

    pub fn refresh_stats(&mut self) {
        let r = self.rank.max(1);
        match self.id {
            AbilityId::Dash => {
                self.cooldown = (7.5 - r as f32 * 0.35).max(4.0);
                self.mana_cost = 35.0 + r as f32 * 5.0;
            }
            AbilityId::Shockwave => {
                self.cooldown = (9.0 - r as f32 * 0.4).max(5.0);
                self.mana_cost = 50.0 + r as f32 * 8.0;
            }
            AbilityId::Bolt => {
                self.cooldown = (6.0 - r as f32 * 0.3).max(3.0);
                self.mana_cost = 45.0 + r as f32 * 7.0;
            }
            AbilityId::Nova => {
                self.cooldown = (50.0 - r as f32 * 4.0).max(30.0);
                self.mana_cost = 100.0 + r as f32 * 20.0;
            }
        }
    }

    pub fn cast_kind(&self) -> AbilityCastKind {
        let r = self.rank.max(1);
        match self.id {
            AbilityId::Dash | AbilityId::Shockwave => AbilityCastKind::Instant,
            AbilityId::Bolt => AbilityCastKind::Targeted {
                cast_range: 10.0 + r as f32 * 0.8,
                aoe_radius: 1.4 + r as f32 * 0.15,
            },
            AbilityId::Nova => AbilityCastKind::Targeted {
                cast_range: 8.5 + r as f32 * 0.7,
                aoe_radius: 4.5 + r as f32 * 0.5,
            },
        }
    }

    pub fn dash_distance(&self) -> f32 {
        8.0 + self.rank as f32 * 1.2
    }

    pub fn shockwave_damage(&self) -> f32 {
        70.0 + self.rank as f32 * 28.0
    }

    pub fn shockwave_radius(&self) -> f32 {
        6.5 + self.rank as f32 * 0.55
    }

    pub fn bolt_damage(&self) -> f32 {
        90.0 + self.rank as f32 * 30.0
    }

    pub fn nova_damage(&self) -> f32 {
        160.0 + self.rank as f32 * 55.0
    }

    pub fn nova_heal(&self) -> f32 {
        100.0 + self.rank as f32 * 40.0
    }
}

#[derive(Component, Debug, Clone)]
pub struct AbilityLoadout {
    pub slots: [AbilitySlot; 4],
}

impl AbilityLoadout {
    pub fn starter() -> Self {
        Self {
            slots: [
                AbilitySlot::fresh(AbilityId::Dash),
                AbilitySlot::fresh(AbilityId::Shockwave),
                AbilitySlot::fresh(AbilityId::Bolt),
                AbilitySlot::fresh(AbilityId::Nova),
            ],
        }
    }

    pub fn slot_mut(&mut self, index: usize) -> Option<&mut AbilitySlot> {
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
        slot.refresh_stats();
        *skill_points -= 1;
        true
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Projectile {
    pub damage: f32,
    pub speed: f32,
    pub team: Team,
    pub radius: f32,
    pub lifetime: f32,
    pub damage_type: DamageType,
    pub splash_radius: f32,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct ProjectileHome(pub Entity);

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
