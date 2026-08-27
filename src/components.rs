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
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self {
            current: max,
            max,
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
    pub attack_damage: f32,
    pub attack_range: f32,
    pub attack_speed: f32,
    pub armor: f32,
    pub move_speed: f32,
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
    /// Lane this creep was spawned into (used by tooling / future AI variants).
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
pub struct PlayerWallet {
    pub gold: u32,
}

#[derive(Component, Debug, Clone)]
pub struct AbilityLoadout {
    pub slots: [AbilitySlot; 4],
}

#[derive(Debug, Clone)]
pub struct AbilitySlot {
    pub id: AbilityId,
    pub cooldown_remaining: f32,
    pub cooldown: f32,
    pub mana_cost: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityId {
    Dash,
    Shockwave,
    Bolt,
    Nova,
}

impl AbilityLoadout {
    pub fn starter() -> Self {
        Self {
            slots: [
                AbilitySlot {
                    id: AbilityId::Dash,
                    cooldown_remaining: 0.0,
                    cooldown: 6.0,
                    mana_cost: 40.0,
                },
                AbilitySlot {
                    id: AbilityId::Shockwave,
                    cooldown_remaining: 0.0,
                    cooldown: 8.0,
                    mana_cost: 60.0,
                },
                AbilitySlot {
                    id: AbilityId::Bolt,
                    cooldown_remaining: 0.0,
                    cooldown: 5.0,
                    mana_cost: 50.0,
                },
                AbilitySlot {
                    id: AbilityId::Nova,
                    cooldown_remaining: 0.0,
                    cooldown: 40.0,
                    mana_cost: 120.0,
                },
            ],
        }
    }

    pub fn slot_mut(&mut self, index: usize) -> Option<&mut AbilitySlot> {
        self.slots.get_mut(index)
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Projectile {
    pub damage: f32,
    pub speed: f32,
    pub team: Team,
    pub radius: f32,
    pub lifetime: f32,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct ProjectileTarget {
    pub position: Vec3,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Lifetime(pub f32);
