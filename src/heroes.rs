//! Hero roster: selectable kits with unique spells and attribute growth.

use bevy::prelude::*;

use crate::abilities::definition::Attribute;
use crate::basic_attack::MAX_ATTACK_SWING;
use crate::components::{
    AbilityId, AbilityLoadout, CombatStats, Health, HeroAttributes, Mana, Team,
};
use crate::dimensions::UnitDimensions;
use crate::net::{NetConfig, NetMode, NetSession};
use crate::resources::SharedAssets;
use crate::scale;
use crate::units::spawn_hero_entity;

/// Which hero the local player picked (or will pick).
#[derive(Resource, Debug, Clone, Copy)]
pub struct LocalHeroChoice {
    pub hero: Option<HeroId>,
    /// When set via CLI, skip the select screen.
    pub from_cli: bool,
    pub spawned: bool,
}

impl Default for LocalHeroChoice {
    fn default() -> Self {
        Self {
            hero: None,
            from_cli: false,
            spawned: false,
        }
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HeroKind(pub HeroId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeroId {
    // <hero_generator:hero_enum>
    /// Strength bruiser — dash, shockwave, bolt, nova.
    Vanguard,
    /// Agility skirmisher — blink, flurry, caltrops, execute.
    Skirmisher,
    /// Intelligence caster — missile, frost, barrier, meteor.
    Arcanist,

    /// Generated hero — abilities are stubs until implemented.
    Warden,
    /// Generated hero — abilities are stubs until implemented.
    Hexer,
    /// Generated hero — abilities are stubs until implemented.
    QueenOfPain,
    /// Generated hero — abilities are stubs until implemented.
    Magnus,
    /// Generated hero — abilities are stubs until implemented.
    Sniper,
    /// Generated hero — abilities are stubs until implemented.
    Ursa,
    /// Generated hero — abilities are stubs until implemented.
    Slark,
    /// Generated hero — abilities are stubs until implemented.
    WinterWyvern,
    /// Generated hero — abilities are stubs until implemented.
    Shadowheart,
    /// Generated hero — abilities are stubs until implemented.
    Gyrocopter,
    // </hero_generator:hero_enum>
}

impl HeroId {
    pub fn all() -> &'static [HeroId] {
        &[
            // <hero_generator:hero_all>
            HeroId::Vanguard,
            HeroId::Skirmisher,
            HeroId::Arcanist,
            HeroId::Warden,
            HeroId::Hexer,
            HeroId::QueenOfPain,
            HeroId::Magnus,
            HeroId::Sniper,
            HeroId::Ursa,
            HeroId::Slark,
            HeroId::WinterWyvern,
            HeroId::Shadowheart,
            HeroId::Gyrocopter,
            // </hero_generator:hero_all>
        ]
    }

    pub fn from_cli(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase();
        if let Ok(n) = key.parse::<usize>() {
            if (1..=Self::all().len()).contains(&n) {
                return Some(Self::all()[n - 1]);
            }
        }
        Self::all()
            .iter()
            .copied()
            .find(|hero| hero.name().eq_ignore_ascii_case(&key))
    }

    pub fn as_u8(self) -> u8 {
        Self::all().iter().position(|&h| h == self).unwrap_or(0) as u8
    }

    pub fn from_u8(value: u8) -> Self {
        Self::all()
            .get(value as usize)
            .copied()
            .unwrap_or(HeroId::Vanguard)
    }

    pub fn name(self) -> &'static str {
        match self {
            // <hero_generator:hero_name>
            HeroId::Vanguard => "Vanguard",
            HeroId::Skirmisher => "Skirmisher",
            HeroId::Arcanist => "Arcanist",

            HeroId::Warden => "Warden",
            HeroId::Hexer => "Hexer",
            HeroId::QueenOfPain => "Queen of Pain",
            HeroId::Magnus => "Magnus",
            HeroId::Sniper => "Sniper",
            HeroId::Ursa => "Ursa",
            HeroId::Slark => "Slark",
            HeroId::WinterWyvern => "Winter Wyvern",
            HeroId::Shadowheart => "Shadowheart",
            HeroId::Gyrocopter => "Gyrocopter",
            // </hero_generator:hero_name>
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            // <hero_generator:hero_blurb>
            HeroId::Vanguard => "Str bruiser — dash in, shockwave, bolt, nova",
            HeroId::Skirmisher => "Agi duelist — blink, flurry, caltrops, execute",
            HeroId::Arcanist => "Int caster — missile, frost, barrier, meteor",

            HeroId::Warden => "Str tank — stub kit",
            HeroId::Hexer => "Int support — stub kit",
            HeroId::QueenOfPain => {
                "Int assassin — Shadow Strike, QoP Blink, Scream of Pain, Sonic Wave"
            }
            HeroId::Magnus => "Str initiator — Shockwave, Empower, Skewer, Reverse Polarity",
            HeroId::Sniper => "Agi marksman — Shrapnel, Headshot, Take Aim, Assassinate",
            HeroId::Ursa => "Agi brawler — Earthshock, Overpower, Fury Swipes, Enrage",
            HeroId::Slark => "Agi assassin — Dark Pact, Pounce, Essence Shift, Shadow Dance",
            HeroId::WinterWyvern => {
                "Int disabler — Arctic Burn, Splinter Blast, Cold Embrace, Winter's Curse"
            }
            HeroId::Shadowheart => {
                "Str support — Purification, Warding Bond, Spirit Guardians, Sharran Conversion"
            }
            HeroId::Gyrocopter => {
                "Agi gunship — Rocket Barrage, Homing Missile, Flak Cannon, Call Down"
            } // </hero_generator:hero_blurb>
        }
    }

    pub fn primary_label(self) -> &'static str {
        match self {
            // <hero_generator:hero_primary>
            HeroId::Vanguard => "STR",
            HeroId::Skirmisher => "AGI",
            HeroId::Arcanist => "INT",

            HeroId::Warden => "STR",
            HeroId::Hexer => "INT",
            HeroId::QueenOfPain => "INT",
            HeroId::Magnus => "STR",
            HeroId::Sniper => "AGI",
            HeroId::Ursa => "AGI",
            HeroId::Slark => "AGI",
            HeroId::WinterWyvern => "INT",
            HeroId::Shadowheart => "STR",
            HeroId::Gyrocopter => "AGI",
            // </hero_generator:hero_primary>
        }
    }

    pub fn def(self) -> HeroDef {
        match self {
            // <hero_generator:hero_def>
            HeroId::Vanguard => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 22.0,
                    agility: 16.0,
                    intelligence: 14.0,
                    str_per_level: 2.6,
                    agi_per_level: 1.7,
                    int_per_level: 1.6,
                    primary: Some(Attribute::Strength),
                },
                base_health: 380.0,
                base_health_regen: 0.6,
                base_mana: 90.0,
                base_mana_regen: 0.8,
                combat: hero_combat(
                    36.0,
                    scale::MELEE_ATTACK_RANGE,
                    1.7,
                    0.3,
                    0.35,
                    crate::facing::HERO_TURN_RATE,
                    2.0,
                    6.321839,
                    scale::HERO_MOVE_SPEED,
                    None,
                ),
                abilities: [
                    AbilityId::Dash,
                    AbilityId::Shockwave,
                    AbilityId::Bolt,
                    AbilityId::Nova,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Skirmisher => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 16.0,
                    agility: 24.0,
                    intelligence: 14.0,
                    str_per_level: 1.8,
                    agi_per_level: 2.8,
                    int_per_level: 1.5,
                    primary: Some(Attribute::Agility),
                },
                base_health: 320.0,
                base_health_regen: 0.4,
                base_mana: 100.0,
                base_mana_regen: 0.9,
                combat: hero_combat(
                    28.0,
                    scale::SHORT_ATTACK_RANGE,
                    1.5,
                    0.25,
                    0.3,
                    crate::facing::HERO_TURN_RATE,
                    1.2,
                    6.321839,
                    scale::HERO_MOVE_SPEED_FAST,
                    None,
                ),
                abilities: [
                    AbilityId::Blink,
                    AbilityId::Flurry,
                    AbilityId::Caltrops,
                    AbilityId::Execute,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Arcanist => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 14.0,
                    agility: 14.0,
                    intelligence: 26.0,
                    str_per_level: 1.6,
                    agi_per_level: 1.5,
                    int_per_level: 3.0,
                    primary: Some(Attribute::Intelligence),
                },
                base_health: 300.0,
                base_health_regen: 0.35,
                base_mana: 140.0,
                base_mana_regen: 1.4,
                combat: hero_combat(
                    22.0,
                    scale::RANGED_ATTACK_RANGE,
                    1.6,
                    0.35,
                    0.4,
                    crate::facing::HERO_TURN_RATE,
                    0.8,
                    6.321839,
                    scale::HERO_MOVE_SPEED_SLOW,
                    None,
                ),
                abilities: [
                    AbilityId::ArcMissile,
                    AbilityId::FrostNova,
                    AbilityId::Barrier,
                    AbilityId::Meteor,
                ],
                dimensions: UnitDimensions::HERO,
            },

            HeroId::Warden => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 24.0,
                    agility: 14.0,
                    intelligence: 12.0,
                    str_per_level: 3.0,
                    agi_per_level: 1.4,
                    int_per_level: 1.2,
                    primary: Some(Attribute::Strength),
                },
                base_health: 340.0,
                base_health_regen: 0.5,
                base_mana: 90.0,
                base_mana_regen: 0.8,
                combat: hero_combat(
                    31.0,
                    scale::MELEE_ATTACK_RANGE,
                    1.7,
                    0.3,
                    0.35,
                    crate::facing::HERO_TURN_RATE,
                    2.0,
                    6.321839,
                    scale::HERO_MOVE_SPEED,
                    None,
                ),
                abilities: [
                    AbilityId::Bulwark,
                    AbilityId::ShieldBash,
                    AbilityId::Taunt,
                    AbilityId::Aegis,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Hexer => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 13.0,
                    agility: 15.0,
                    intelligence: 25.0,
                    str_per_level: 1.5,
                    agi_per_level: 1.6,
                    int_per_level: 3.1,
                    primary: Some(Attribute::Intelligence),
                },
                base_health: 300.0,
                base_health_regen: 0.35,
                base_mana: 140.0,
                base_mana_regen: 1.4,
                combat: hero_combat(
                    23.0,
                    scale::RANGED_ATTACK_RANGE,
                    1.7,
                    0.3,
                    0.35,
                    crate::facing::HERO_TURN_RATE,
                    0.8,
                    6.321839,
                    scale::HERO_MOVE_SPEED,
                    None,
                ),
                abilities: [
                    AbilityId::HexBolt,
                    AbilityId::Curse,
                    AbilityId::Ward,
                    AbilityId::Ritual,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::QueenOfPain => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 20.0,
                    agility: 20.0,
                    intelligence: 25.0,
                    str_per_level: 2.4,
                    agi_per_level: 2.5,
                    int_per_level: 3.4,
                    primary: Some(Attribute::Intelligence),
                },
                base_health: 200.0,
                base_health_regen: -0.15,
                base_mana: 75.0,
                base_mana_regen: 0.0,
                combat: hero_combat(
                    29.0,
                    550.0,
                    1.5,
                    0.56,
                    0.41,
                    crate::facing::HERO_TURN_RATE,
                    -2.8,
                    6.321839,
                    290.0,
                    Some(1500.0),
                ),
                abilities: [
                    AbilityId::ShadowStrike,
                    AbilityId::QueenOfPainBlink,
                    AbilityId::ScreamOfPain,
                    AbilityId::SonicWave,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Magnus => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 23.0,
                    agility: 14.0,
                    intelligence: 20.0,
                    str_per_level: 3.1,
                    agi_per_level: 2.2,
                    int_per_level: 2.1,
                    primary: Some(Attribute::Strength),
                },
                base_health: 211.0,
                base_health_regen: 0.04,
                base_mana: 75.0,
                base_mana_regen: 0.5,
                combat: hero_combat(
                    38.0, 200.0, 1.8, 0.5, 0.84, 0.8, -0.96, 6.321839, 305.0, None,
                ),
                abilities: [
                    AbilityId::MagnusShockwave,
                    AbilityId::Empower,
                    AbilityId::Skewer,
                    AbilityId::ReversePolarity,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Sniper => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 19.0,
                    agility: 27.0,
                    intelligence: 15.0,
                    str_per_level: 2.0,
                    agi_per_level: 3.2,
                    int_per_level: 2.6,
                    primary: Some(Attribute::Agility),
                },
                base_health: 196.0,
                base_health_regen: -0.13,
                base_mana: 75.0,
                base_mana_regen: 0.0,
                combat: hero_combat(
                    16.0,
                    550.0,
                    1.7,
                    0.17,
                    0.7,
                    0.7,
                    -3.78,
                    6.321839,
                    285.0,
                    Some(3000.0),
                ),
                abilities: [
                    AbilityId::Shrapnel,
                    AbilityId::Headshot,
                    AbilityId::TakeAim,
                    AbilityId::Assassinate,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Ursa => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 25.0,
                    agility: 18.0,
                    intelligence: 16.0,
                    str_per_level: 2.4,
                    agi_per_level: 2.8,
                    int_per_level: 1.5,
                    primary: Some(Attribute::Agility),
                },
                base_health: 220.0,
                base_health_regen: 0.0,
                base_mana: 75.0,
                base_mana_regen: 0.0,
                combat: hero_combat(
                    26.0, 150.0, 1.7, 0.3, 0.3, 0.6, -1.52, 6.321839, 310.0, None,
                ),
                abilities: [
                    AbilityId::Earthshock,
                    AbilityId::Overpower,
                    AbilityId::FurySwipes,
                    AbilityId::Enrage,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Slark => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 20.0,
                    agility: 21.0,
                    intelligence: 16.0,
                    str_per_level: 2.1,
                    agi_per_level: 1.5,
                    int_per_level: 1.9,
                    primary: Some(Attribute::Agility),
                },
                base_health: 200.0,
                base_health_regen: -0.15,
                base_mana: 75.0,
                base_mana_regen: 0.0,
                combat: CombatStats {
                    base_attack_speed: 120.0,
                    ..hero_combat(
                        40.0, 150.0, 1.7, 0.5, 0.3, 0.7, -2.94, 6.321839, 300.0, None,
                    )
                },
                abilities: [
                    AbilityId::DarkPact,
                    AbilityId::Pounce,
                    AbilityId::EssenceShift,
                    AbilityId::ShadowDance,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::WinterWyvern => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 22.0,
                    agility: 16.0,
                    intelligence: 26.0,
                    str_per_level: 2.5,
                    agi_per_level: 1.7,
                    int_per_level: 3.6,
                    primary: Some(Attribute::Intelligence),
                },
                base_health: 208.0,
                base_health_regen: -0.19,
                base_mana: 75.0,
                base_mana_regen: 0.25,
                combat: hero_combat(
                    20.0,
                    450.0,
                    1.7,
                    0.25,
                    0.8,
                    0.6,
                    -1.24,
                    6.321839,
                    285.0,
                    Some(700.0),
                ),
                abilities: [
                    AbilityId::ArcticBurn,
                    AbilityId::SplinterBlast,
                    AbilityId::ColdEmbrace,
                    AbilityId::WintersCurse,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Shadowheart => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 22.0,
                    agility: 15.0,
                    intelligence: 18.0,
                    str_per_level: 3.1,
                    agi_per_level: 1.7,
                    int_per_level: 2.1,
                    primary: Some(Attribute::Strength),
                },
                base_health: 208.0,
                base_health_regen: -0.19,
                base_mana: 75.0,
                base_mana_regen: 0.0,
                combat: hero_combat(36.0, 150.0, 1.7, 0.4, 0.6, 0.6, -0.1, 6.321839, 310.0, None),
                abilities: [
                    AbilityId::Purification,
                    AbilityId::WardingBond,
                    AbilityId::SpiritGuardians,
                    AbilityId::SharranConversion,
                ],
                dimensions: UnitDimensions::HERO,
            },
            HeroId::Gyrocopter => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 22.0,
                    agility: 25.0,
                    intelligence: 21.0,
                    str_per_level: 2.5,
                    agi_per_level: 3.4,
                    int_per_level: 2.4,
                    primary: Some(Attribute::Agility),
                },
                base_health: 208.0,
                base_health_regen: -0.19,
                base_mana: 75.0,
                base_mana_regen: 0.0,
                combat: CombatStats {
                    base_attack_speed: 115.0,
                    ..hero_combat(
                        21.0,
                        365.0,
                        1.7,
                        0.2,
                        0.1,
                        0.6,
                        -2.5,
                        6.321839,
                        315.0,
                        Some(3000.0),
                    )
                },
                abilities: [
                    AbilityId::RocketBarrage,
                    AbilityId::HomingMissile,
                    AbilityId::FlakCannon,
                    AbilityId::CallDown,
                ],
                dimensions: UnitDimensions::HERO,
            },
            // </hero_generator:hero_def>
        }
    }

    pub fn loadout(self) -> AbilityLoadout {
        let abilities = self.def().abilities;
        AbilityLoadout::from_abilities(abilities)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HeroDef {
    #[allow(dead_code)]
    pub id: HeroId,
    pub attributes: HeroAttributes,
    pub base_health: f32,
    pub base_health_regen: f32,
    pub base_mana: f32,
    pub base_mana_regen: f32,
    pub combat: CombatStats,
    pub abilities: [AbilityId; 4],
    /// Collision size, bounds radius, and selection volume for this hero.
    pub dimensions: UnitDimensions,
}

/// Builds a hero's base [`CombatStats`].
///
/// `attack_projectile_speed` is the hero's absolute ranged-attack projectile speed. Heroes carry
/// it as a base offset in `attack_projectile_speed_bonus` (relative to
/// `basic_attack::projectile_speed_for(attack_range)`), so items stack on top of it. `None`
/// keeps the range-derived default.
fn hero_combat(
    attack_damage: f32,
    attack_range: f32,
    base_attack_time: f32,
    attack_point: f32,
    attack_backswing: f32,
    turn_rate: f32,
    armor: f32,
    magic_resist: f32,
    move_speed: f32,
    attack_projectile_speed: Option<f32>,
) -> CombatStats {
    CombatStats {
        attack_damage,
        attack_range,
        base_attack_speed: 100.0,
        attack_speed_flat: 0.0,
        attack_speed_mult: 0.0,
        base_attack_time,
        attack_speed: 0.0,
        attack_point: attack_point.clamp(0.0, MAX_ATTACK_SWING),
        attack_backswing: attack_backswing.clamp(0.0, MAX_ATTACK_SWING),
        turn_rate,
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
        cleaves: Default::default(),
        evasion: 0.0,
        accuracy: 0.0,
        splash_radius: 0.0,
        splash_damage_ratio: 0.0,
        attack_range_bonus: 0.0,
        attack_projectile_speed_bonus: attack_projectile_speed.map_or(0.0, |speed| {
            speed - crate::basic_attack::projectile_speed_for(attack_range)
        }),
        lifesteal: 0.0,
        spell_lifesteal: 0.0,
        heal_amp: 0.0,
        heal_received_amp: 0.0,
        mana_cost_amp: 0.0,
    }
}

impl HeroDef {
    /// Attack damage of a level-1 hero: the stored base plus one per point of the primary
    /// attribute. `attack_damage_pct` statuses (Empower) snapshot this.
    pub fn level_one_attack_damage(&self) -> f32 {
        self.combat.attack_damage
            + self.attributes.primary_value() * HeroAttributes::DAMAGE_PER_PRIMARY
    }

    pub fn vitals(&self) -> (Health, Mana, CombatStats, HeroAttributes) {
        let attrs = self.attributes;
        let mut health = Health {
            current: self.base_health,
            max: self.base_health,
            regen_per_sec: self.base_health_regen,
        };
        let mut mana = Mana::new(self.base_mana, self.base_mana_regen);
        let mut stats = self.combat;
        attrs.apply_to(&mut health, &mut mana, &mut stats);
        health.current = health.max;
        mana.current = mana.max;
        (health, mana, stats, attrs)
    }
}

#[derive(Component)]
pub(crate) struct HeroSelectRoot;

#[derive(Component, Debug, Clone, Copy)]
struct HeroSelectButton {
    hero: HeroId,
}

/// Hero select input and local hero spawning; the map is generated after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeroSpawnSet;

pub struct HeroesPlugin;

impl Plugin for HeroesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalHeroChoice>()
            .add_systems(
                Startup,
                spawn_hero_select_ui.after(crate::resources::load_shared_assets),
            )
            .add_systems(
                Update,
                (
                    handle_hero_select_input,
                    handle_hero_select_buttons,
                    maybe_spawn_local_hero,
                )
                    .chain()
                    .in_set(HeroSpawnSet),
            );
    }
}

fn spawn_hero_select_ui(mut commands: Commands, choice: Res<LocalHeroChoice>) {
    if choice.from_cli && choice.hero.is_some() {
        return;
    }
    spawn_hero_select_ui_force(&mut commands);
}

/// Rebuild the hero select overlay (e.g. after New Game).
pub(crate) fn spawn_hero_select_ui_force(commands: &mut Commands) {
    commands
        .spawn((
            Name::new("Hero Select"),
            HeroSelectRoot,
            Node {
                width: percent(100),
                height: percent(100),
                position_type: PositionType::Absolute,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                row_gap: px(18),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.08, 0.88)),
            ZIndex(20),
        ))
        .with_children(|root| {
            root.spawn((
                Text::new("Choose your hero"),
                TextFont::from_font_size(36.0),
                TextColor(Color::srgb(0.95, 0.97, 1.0)),
            ));
            root.spawn((
                Text::new({
                    let n = HeroId::all().len();
                    let keys = (1..=n)
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    format!("Press {keys}  ·  or click a card  ·  Enter to confirm")
                }),
                TextFont::from_font_size(16.0),
                TextColor(Color::srgb(0.7, 0.8, 0.9)),
            ));
            for (label, title, accent) in PRIMARY_GROUPS {
                let members: Vec<(usize, HeroId)> = HeroId::all()
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(_, hero)| hero.primary_label() == label)
                    .collect();
                if members.is_empty() {
                    continue;
                }
                root.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(16),
                    ..default()
                })
                .with_children(|group| {
                    group.spawn((
                        Text::new(title),
                        TextFont::from_font_size(20.0),
                        TextColor(accent),
                        Node {
                            width: px(120),
                            ..default()
                        },
                    ));
                    group
                        .spawn(Node {
                            display: Display::Grid,
                            grid_template_columns: RepeatedGridTrack::px(
                                HERO_GRID_COLUMNS,
                                HERO_CARD_WIDTH,
                            ),
                            grid_auto_rows: vec![GridTrack::px(HERO_CARD_HEIGHT)],
                            column_gap: px(10),
                            row_gap: px(10),
                            ..default()
                        })
                        .with_children(|grid| {
                            for (i, hero) in members {
                                spawn_hero_card(grid, i, hero, accent);
                            }
                        });
                });
            }
        });
}

/// Hero select rows: primary attribute label, row title, accent colour.
const PRIMARY_GROUPS: [(&str, &str, Color); 3] = [
    ("STR", "Strength", Color::srgb(0.95, 0.4, 0.35)),
    ("AGI", "Agility", Color::srgb(0.45, 0.9, 0.45)),
    ("INT", "Intelligence", Color::srgb(0.45, 0.7, 1.0)),
];
const HERO_GRID_COLUMNS: u16 = 5;
const HERO_CARD_WIDTH: f32 = 200.0;
const HERO_CARD_HEIGHT: f32 = 130.0;

fn spawn_hero_card(grid: &mut ChildSpawnerCommands, index: usize, hero: HeroId, accent: Color) {
    let def = hero.def();
    grid.spawn((
        Button,
        HeroSelectButton { hero },
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: px(4),
            padding: UiRect::all(px(8)),
            border: UiRect::all(px(2)),
            border_radius: BorderRadius::all(px(8)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.12, 0.14, 0.18, 0.95)),
        BorderColor::all(Color::srgb(0.35, 0.4, 0.5)),
    ))
    .with_children(|card| {
        card.spawn((
            Text::new(format!("{}  ·  {}", index + 1, hero.name())),
            TextFont::from_font_size(18.0),
            TextColor(Color::WHITE),
        ));
        card.spawn((
            Text::new(format!(
                "Str {:.0}  Agi {:.0}  Int {:.0}",
                def.attributes.strength, def.attributes.agility, def.attributes.intelligence
            )),
            TextFont::from_font_size(12.0),
            TextColor(accent),
        ));
        card.spawn((
            Text::new(hero.blurb()),
            TextFont::from_font_size(11.0),
            TextColor(Color::srgb(0.8, 0.85, 0.9)),
            TextLayout::justify(Justify::Center),
        ));
    });
}

fn handle_hero_select_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut choice: ResMut<LocalHeroChoice>,
    roots: Query<Entity, With<HeroSelectRoot>>,
) {
    if choice.spawned || roots.is_empty() {
        return;
    }
    const DIGITS: [(KeyCode, KeyCode); 9] = [
        (KeyCode::Digit1, KeyCode::Numpad1),
        (KeyCode::Digit2, KeyCode::Numpad2),
        (KeyCode::Digit3, KeyCode::Numpad3),
        (KeyCode::Digit4, KeyCode::Numpad4),
        (KeyCode::Digit5, KeyCode::Numpad5),
        (KeyCode::Digit6, KeyCode::Numpad6),
        (KeyCode::Digit7, KeyCode::Numpad7),
        (KeyCode::Digit8, KeyCode::Numpad8),
        (KeyCode::Digit9, KeyCode::Numpad9),
    ];
    for (i, hero) in HeroId::all().iter().copied().enumerate() {
        if i >= DIGITS.len() {
            break;
        }
        let (digit, numpad) = DIGITS[i];
        if keys.just_pressed(digit) || keys.just_pressed(numpad) {
            choice.hero = Some(hero);
        }
    }
    if keys.just_pressed(KeyCode::Enter) && choice.hero.is_none() {
        choice.hero = Some(HeroId::Vanguard);
    }
}

fn handle_hero_select_buttons(
    interactions: Query<(&Interaction, &HeroSelectButton), Changed<Interaction>>,
    mut choice: ResMut<LocalHeroChoice>,
    roots: Query<Entity, With<HeroSelectRoot>>,
) {
    if choice.spawned || roots.is_empty() {
        return;
    }
    for (interaction, button) in &interactions {
        if *interaction == Interaction::Pressed {
            choice.hero = Some(button.hero);
        }
    }
}

fn maybe_spawn_local_hero(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    config: Res<NetConfig>,
    mut choice: ResMut<LocalHeroChoice>,
    mut session: ResMut<NetSession>,
    mut map: ResMut<crate::map::MapState>,
    mode: Option<Res<crate::map::GameMode>>,
    roots: Query<Entity, With<HeroSelectRoot>>,
    mut buttons: Query<(&HeroSelectButton, &mut BorderColor)>,
) {
    // Highlight selected card.
    for (button, mut border) in &mut buttons {
        let selected = choice.hero == Some(button.hero);
        *border = BorderColor::all(if selected {
            Color::srgb(1.0, 0.85, 0.3)
        } else {
            Color::srgb(0.35, 0.4, 0.5)
        });
    }

    let Some(hero) = choice.hero else {
        return;
    };
    if choice.spawned {
        return;
    }

    // Confirm: CLI auto-confirms; otherwise require Enter or a second click path —
    // for UX, selecting a card (or 1/2/3) immediately confirms.
    let confirm = choice.from_cli || !roots.is_empty() && (choice.hero.is_some());
    if !confirm {
        return;
    }

    // Story matches start in the safe room rather than at the classic Radiant base.
    let start = if mode.is_some_and(|m| *m == crate::map::GameMode::Story) {
        crate::story::SAFE_ENTRY
    } else {
        scale::v(-44.0, 0.9, -44.0)
    };

    match config.mode {
        NetMode::Offline => {
            spawn_hero_entity(&mut commands, &assets, Team::Radiant, true, 1, start, hero);
            choice.spawned = true;
        }
        NetMode::Host => {
            let entity = spawn_hero_entity(
                &mut commands,
                &assets,
                Team::Radiant,
                true,
                1,
                scale::v(-44.0, 0.9, -44.0),
                hero,
            );
            session.local_hero_id = Some(1);
            session.local_hero_entity = Some(entity);
            session.next_id = 2;
            choice.spawned = true;
        }
        NetMode::Client => {
            // Client waits for Welcome; mark choice ready so Hello can include hero kind.
            choice.spawned = true;
        }
    }

    // The battlefield is only built once a hero has been picked.
    map.request();

    for entity in &roots {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hero_keeps_level_one_attack_damage_parity() {
        // Level-1 totals (stored base + primary attribute) from each hero's spec.
        let expected = [
            (HeroId::Vanguard, 58.0),
            (HeroId::Skirmisher, 52.0),
            (HeroId::Arcanist, 48.0),
            (HeroId::Warden, 55.0),
            (HeroId::Hexer, 48.0),
            (HeroId::QueenOfPain, 54.0),
            (HeroId::Magnus, 61.0),
            (HeroId::Sniper, 43.0),
            (HeroId::Ursa, 44.0),
            (HeroId::Slark, 61.0),
            (HeroId::WinterWyvern, 46.0),
            (HeroId::Shadowheart, 58.0),
            (HeroId::Gyrocopter, 46.0),
        ];
        for (hero, total) in expected {
            let def = hero.def();
            let (_, _, stats, _) = def.vitals();
            assert_eq!(stats.attack_damage, total, "{hero:?} vitals damage");
            assert_eq!(
                def.level_one_attack_damage(),
                total,
                "{hero:?} level-one damage"
            );
        }
    }

    #[test]
    fn every_hero_primary_matches_primary_label() {
        for hero in HeroId::all() {
            let primary = hero.def().attributes.primary.expect("hero has a primary");
            let label = match primary {
                Attribute::Strength => "STR",
                Attribute::Agility => "AGI",
                Attribute::Intelligence => "INT",
            };
            assert_eq!(hero.primary_label(), label, "{hero:?}");
        }
    }

    #[test]
    fn every_hero_magic_resist_is_27_5_percent_at_level_1_and_level_6() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        for hero in HeroId::all() {
            let def = hero.def();
            let (mut health, mut mana, mut stats, mut attrs) = def.vitals();
            let at_level_1 = apply_damage(100.0, DamageType::Magical, &stats);
            assert!(
                (at_level_1 - 72.5).abs() < 0.01,
                "{hero:?} level 1: {at_level_1}"
            );
            // Intelligence grants no magic resist: five levels of growth leave it at 27.5%.
            for _ in 1..crate::units::STARTING_HERO_LEVEL {
                let before = attrs;
                attrs.level_up();
                HeroAttributes::apply_delta(&before, &attrs, &mut health, &mut mana, &mut stats);
            }
            let at_level_6 = apply_damage(100.0, DamageType::Magical, &stats);
            assert!(
                (at_level_6 - 72.5).abs() < 0.01,
                "{hero:?} level 6: {at_level_6}"
            );
        }
    }

    #[test]
    fn winter_wyvern_matches_spec() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        assert_eq!(
            HeroId::from_cli("winter wyvern"),
            Some(HeroId::WinterWyvern)
        );
        assert_eq!(HeroId::WinterWyvern.name(), "Winter Wyvern");
        assert_eq!(HeroId::WinterWyvern.primary_label(), "INT");
        let def = HeroId::WinterWyvern.def();
        assert_eq!(
            def.abilities,
            [
                AbilityId::ArcticBurn,
                AbilityId::SplinterBlast,
                AbilityId::ColdEmbrace,
                AbilityId::WintersCurse,
            ]
        );

        let (health, mana, stats, attrs) = def.vitals();
        assert!((health.max - 604.0).abs() < 1e-3, "health {}", health.max);
        assert!((health.regen_per_sec - 2.45).abs() < 1e-4);
        assert!((mana.max - 387.0).abs() < 1e-3, "mana {}", mana.max);
        assert!((mana.regen_per_sec - 1.55).abs() < 1e-4);
        assert!((stats.armor - 1.0).abs() < 1e-3, "armor {}", stats.armor);
        let taken = apply_damage(100.0, DamageType::Magical, &stats);
        assert!(
            (taken - 72.5).abs() < 0.01,
            "27.5% magic resist: took {taken}"
        );
        assert_eq!(
            (attrs.strength, attrs.agility, attrs.intelligence),
            (22.0, 16.0, 26.0)
        );
        assert_eq!(
            (
                attrs.str_per_level,
                attrs.agi_per_level,
                attrs.int_per_level
            ),
            (2.5, 1.7, 3.6)
        );

        // 46 at level 1: the stored 20 plus 26 Intelligence (the primary).
        assert_eq!(stats.attack_damage, 46.0);
        assert_eq!(def.combat.attack_damage, 20.0);
        assert_eq!(stats.attack_range, 450.0);
        assert!(!stats.is_melee());
        assert_eq!(stats.base_attack_time, 1.7);
        assert_eq!(stats.attack_point, 0.25);
        assert_eq!(stats.attack_backswing, 0.8);
        assert_eq!(stats.turn_rate, 0.6);
        assert_eq!(stats.move_speed, 285.0);
        assert_eq!(
            stats.attack_projectile_speed_bonus,
            700.0 - crate::basic_attack::projectile_speed_for(450.0)
        );
        assert!(
            (stats.attack_speed - 116.0 / 100.0 / 1.7).abs() < 1e-4,
            "APS {}",
            stats.attack_speed
        );
        assert!((stats.attack_speed - 0.682353).abs() < 1e-5);
    }

    #[test]
    fn shadowheart_matches_spec() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        assert_eq!(HeroId::from_cli("shadowheart"), Some(HeroId::Shadowheart));
        assert_eq!(HeroId::Shadowheart.name(), "Shadowheart");
        assert_eq!(HeroId::Shadowheart.primary_label(), "STR");
        let def = HeroId::Shadowheart.def();
        assert_eq!(
            def.abilities,
            [
                AbilityId::Purification,
                AbilityId::WardingBond,
                AbilityId::SpiritGuardians,
                AbilityId::SharranConversion,
            ]
        );

        let (health, mana, stats, attrs) = def.vitals();
        assert!((health.max - 604.0).abs() < 1e-3, "health {}", health.max);
        assert!((health.regen_per_sec - 2.45).abs() < 1e-4);
        assert!((mana.max - 291.0).abs() < 1e-3, "mana {}", mana.max);
        assert!((mana.regen_per_sec - 0.9).abs() < 1e-4);
        assert!((stats.armor - 2.0).abs() < 1e-3, "armor {}", stats.armor);
        let taken = apply_damage(100.0, DamageType::Magical, &stats);
        assert!(
            (taken - 72.5).abs() < 0.01,
            "27.5% magic resist: took {taken}"
        );
        assert_eq!(
            (attrs.strength, attrs.agility, attrs.intelligence),
            (22.0, 15.0, 18.0)
        );
        assert_eq!(
            (
                attrs.str_per_level,
                attrs.agi_per_level,
                attrs.int_per_level
            ),
            (3.1, 1.7, 2.1)
        );

        // 58 at level 1: the stored 36 plus 22 Strength (the primary).
        assert_eq!(stats.attack_damage, 58.0);
        assert_eq!(def.combat.attack_damage, 36.0);
        assert_eq!(stats.attack_range, 150.0);
        assert!(stats.is_melee());
        assert_eq!(stats.base_attack_time, 1.7);
        assert_eq!(stats.attack_point, 0.4);
        assert_eq!(stats.attack_backswing, 0.6);
        assert_eq!(stats.turn_rate, 0.6);
        assert_eq!(stats.move_speed, 310.0);
        assert!(
            (stats.attack_speed - 115.0 / 100.0 / 1.7).abs() < 1e-4,
            "APS {}",
            stats.attack_speed
        );
        assert!((stats.attack_speed - 0.676471).abs() < 1e-5);
    }

    #[test]
    fn roster_has_unique_kits_and_growth() {
        let v = HeroId::Vanguard.def();
        let s = HeroId::Skirmisher.def();
        let a = HeroId::Arcanist.def();
        assert_ne!(v.abilities, s.abilities);
        assert_ne!(s.abilities, a.abilities);
        assert!(v.attributes.strength > s.attributes.strength);
        assert!(s.attributes.agility > a.attributes.agility);
        assert!(a.attributes.intelligence > v.attributes.intelligence);
        assert!(a.attributes.int_per_level > v.attributes.int_per_level);
        assert_eq!(HeroId::from_cli("skirmisher"), Some(HeroId::Skirmisher));
    }

    #[test]
    fn loadouts_are_four_abilities() {
        for hero in HeroId::all() {
            let loadout = hero.loadout();
            assert_eq!(loadout.slots.len(), 4);
            assert_eq!(loadout.slots[3].id.is_ultimate(), true);
        }
    }

    #[test]
    fn hero_combat_keeps_attack_point_above_half_a_second() {
        let c = hero_combat(50.0, 550.0, 1.5, 0.56, 0.41, 0.6, 0.0, 0.0, 290.0, None);
        assert_eq!(c.attack_point, 0.56);
        assert_eq!(c.attack_backswing, 0.41);
        let c = hero_combat(50.0, 550.0, 1.5, 5.0, 5.0, 0.6, 0.0, 0.0, 290.0, None);
        assert_eq!(c.attack_point, MAX_ATTACK_SWING);
        assert_eq!(c.attack_backswing, MAX_ATTACK_SWING);
    }

    #[test]
    fn hero_combat_carries_a_per_hero_turn_rate() {
        let c = hero_combat(61.0, 200.0, 1.8, 0.5, 0.84, 0.8, 0.0, 0.0, 305.0, None);
        assert_eq!(c.turn_rate, 0.8);
        let c = hero_combat(
            61.0,
            200.0,
            1.8,
            0.5,
            0.84,
            crate::facing::HERO_TURN_RATE,
            0.0,
            0.0,
            305.0,
            None,
        );
        assert_eq!(c.turn_rate, 0.6);
    }

    #[test]
    fn hero_base_attack_speed_override_feeds_attack_speed() {
        let plain = hero_combat(61.0, 150.0, 1.7, 0.5, 0.3, 0.7, 0.0, 0.0, 300.0, None);
        assert_eq!(plain.base_attack_speed, 100.0);
        let fast = CombatStats {
            base_attack_speed: 120.0,
            ..hero_combat(61.0, 150.0, 1.7, 0.5, 0.3, 0.7, 0.0, 0.0, 300.0, None)
        };
        assert_eq!(fast.base_attack_speed, 120.0);
        assert_eq!(fast.base_attack_time, 1.7);
        let mut stats = fast;
        stats.recompute_attack_speed(21.0);
        assert!((stats.attack_speed - 141.0 / 100.0 / 1.7).abs() < 1e-5);
    }

    #[test]
    fn hero_combat_attack_projectile_speed_override() {
        let c = hero_combat(
            50.0,
            550.0,
            1.5,
            0.3,
            0.3,
            0.6,
            0.0,
            0.0,
            290.0,
            Some(1500.0),
        );
        let speed = (crate::basic_attack::projectile_speed_for(c.base_attack_range())
            + c.attack_projectile_speed_bonus)
            .max(1.0);
        assert!((speed - 1500.0).abs() < 1e-3, "speed {speed}");
        let c = hero_combat(50.0, 550.0, 1.5, 0.3, 0.3, 0.6, 0.0, 0.0, 290.0, None);
        assert_eq!(c.attack_projectile_speed_bonus, 0.0);
    }

    #[test]
    fn queen_of_pain_matches_spec() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        assert_eq!(HeroId::from_cli("queen of pain"), Some(HeroId::QueenOfPain));
        assert_eq!(HeroId::QueenOfPain.primary_label(), "INT");
        let def = HeroId::QueenOfPain.def();
        assert_eq!(
            def.abilities,
            [
                AbilityId::ShadowStrike,
                AbilityId::QueenOfPainBlink,
                AbilityId::ScreamOfPain,
                AbilityId::SonicWave,
            ]
        );

        let (health, mana, stats, attrs) = def.vitals();
        assert_eq!(health.max, 560.0);
        assert!((health.regen_per_sec - 2.25).abs() < 1e-4);
        assert!((mana.max - 375.0).abs() < 1e-3);
        assert!((mana.regen_per_sec - 1.25).abs() < 1e-4);
        assert!(stats.armor.abs() < 1e-4, "armor {}", stats.armor);
        let taken = apply_damage(100.0, DamageType::Magical, &stats);
        assert!(
            (taken - 72.5).abs() < 0.01,
            "27.5% magic resist: took {taken}"
        );
        assert_eq!(
            (attrs.strength, attrs.agility, attrs.intelligence),
            (20.0, 20.0, 25.0)
        );

        assert_eq!(stats.attack_range, 550.0);
        assert_eq!(stats.base_attack_time, 1.5);
        assert_eq!(stats.attack_point, 0.56);
        assert_eq!(stats.attack_backswing, 0.41);
        assert_eq!(stats.turn_rate, crate::facing::HERO_TURN_RATE);
        assert_eq!(stats.turn_rate, 0.6);
        assert_eq!(stats.move_speed, 290.0);
        let projectile_speed = crate::basic_attack::projectile_speed_for(stats.base_attack_range())
            + stats.attack_projectile_speed_bonus;
        assert!((projectile_speed - 1500.0).abs() < 1e-3);
        assert!(
            (stats.attack_speed - 0.8).abs() < 1e-4,
            "APS {}",
            stats.attack_speed
        );
        assert!(!stats.is_melee());
    }

    #[test]
    fn magnus_matches_spec() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        assert_eq!(HeroId::from_cli("magnus"), Some(HeroId::Magnus));
        assert_eq!(HeroId::Magnus.name(), "Magnus");
        assert_eq!(HeroId::Magnus.primary_label(), "STR");
        let def = HeroId::Magnus.def();
        assert_eq!(
            def.abilities,
            [
                AbilityId::MagnusShockwave,
                AbilityId::Empower,
                AbilityId::Skewer,
                AbilityId::ReversePolarity,
            ]
        );

        let (health, mana, stats, attrs) = def.vitals();
        assert_eq!(health.max, 625.0);
        assert!((health.regen_per_sec - 2.8).abs() < 1e-4);
        assert!((mana.max - 315.0).abs() < 1e-3);
        assert!((mana.regen_per_sec - 1.5).abs() < 1e-4);
        assert!((stats.armor - 1.0).abs() < 1e-4, "armor {}", stats.armor);
        let taken = apply_damage(100.0, DamageType::Magical, &stats);
        assert!(
            (taken - 72.5).abs() < 0.01,
            "27.5% magic resist: took {taken}"
        );
        assert_eq!(
            (attrs.strength, attrs.agility, attrs.intelligence),
            (23.0, 14.0, 20.0)
        );
        assert_eq!(
            (
                attrs.str_per_level,
                attrs.agi_per_level,
                attrs.int_per_level
            ),
            (3.1, 2.2, 2.1)
        );

        assert_eq!(stats.attack_damage, 61.0);
        assert_eq!(stats.attack_range, 200.0);
        assert!(stats.is_melee(), "range 200 is still a melee attack");
        assert_eq!(stats.base_attack_time, 1.8);
        assert_eq!(stats.attack_point, 0.5);
        assert_eq!(stats.attack_backswing, 0.84);
        assert_eq!(stats.turn_rate, 0.8);
        assert_eq!(stats.move_speed, 305.0);
        assert!(
            (stats.attack_speed - 114.0 / 100.0 / 1.8).abs() < 1e-4,
            "APS {}",
            stats.attack_speed
        );
    }

    #[test]
    fn sniper_matches_spec() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        assert_eq!(HeroId::from_cli("sniper"), Some(HeroId::Sniper));
        assert_eq!(HeroId::Sniper.name(), "Sniper");
        assert_eq!(HeroId::Sniper.primary_label(), "AGI");
        let def = HeroId::Sniper.def();
        assert_eq!(
            def.abilities,
            [
                AbilityId::Shrapnel,
                AbilityId::Headshot,
                AbilityId::TakeAim,
                AbilityId::Assassinate,
            ]
        );

        let (health, mana, stats, attrs) = def.vitals();
        assert!((health.max - 538.0).abs() < 1e-3, "health {}", health.max);
        assert!((health.regen_per_sec - 2.15).abs() < 1e-4);
        assert!((mana.max - 255.0).abs() < 1e-3, "mana {}", mana.max);
        assert!((mana.regen_per_sec - 0.75).abs() < 1e-4);
        assert!(stats.armor.abs() < 1e-3, "armor {}", stats.armor);
        let taken = apply_damage(100.0, DamageType::Magical, &stats);
        assert!(
            (taken - 72.5).abs() < 0.01,
            "27.5% magic resist: took {taken}"
        );
        assert_eq!(
            (attrs.strength, attrs.agility, attrs.intelligence),
            (19.0, 27.0, 15.0)
        );
        assert_eq!(
            (
                attrs.str_per_level,
                attrs.agi_per_level,
                attrs.int_per_level
            ),
            (2.0, 3.2, 2.6)
        );

        assert_eq!(stats.attack_damage, 43.0);
        assert_eq!(stats.attack_range, 550.0);
        assert!(!stats.is_melee());
        assert_eq!(stats.base_attack_time, 1.7);
        assert_eq!(stats.attack_point, 0.17);
        assert_eq!(stats.attack_backswing, 0.7);
        assert_eq!(stats.turn_rate, 0.7);
        assert_eq!(stats.move_speed, 285.0);
        let projectile_speed = crate::basic_attack::projectile_speed_for(stats.base_attack_range())
            + stats.attack_projectile_speed_bonus;
        assert!((projectile_speed - 3000.0).abs() < 1e-3);
        assert!(
            (stats.attack_speed - 127.0 / 100.0 / 1.7).abs() < 1e-4,
            "APS {}",
            stats.attack_speed
        );
        assert!((stats.attack_speed - 0.747059).abs() < 1e-5);
    }

    #[test]
    fn ursa_matches_spec() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        assert_eq!(HeroId::from_cli("ursa"), Some(HeroId::Ursa));
        assert_eq!(HeroId::Ursa.name(), "Ursa");
        assert_eq!(HeroId::Ursa.primary_label(), "AGI");
        let def = HeroId::Ursa.def();
        assert_eq!(
            def.abilities,
            [
                AbilityId::Earthshock,
                AbilityId::Overpower,
                AbilityId::FurySwipes,
                AbilityId::Enrage,
            ]
        );

        let (health, mana, stats, attrs) = def.vitals();
        assert!((health.max - 670.0).abs() < 1e-3, "health {}", health.max);
        assert!((health.regen_per_sec - 3.0).abs() < 1e-4);
        assert!((mana.max - 267.0).abs() < 1e-3, "mana {}", mana.max);
        assert!((mana.regen_per_sec - 0.8).abs() < 1e-4);
        assert!((stats.armor - 1.0).abs() < 1e-3, "armor {}", stats.armor);
        let taken = apply_damage(100.0, DamageType::Magical, &stats);
        assert!(
            (taken - 72.5).abs() < 0.01,
            "27.5% magic resist: took {taken}"
        );
        assert_eq!(
            (attrs.strength, attrs.agility, attrs.intelligence),
            (25.0, 18.0, 16.0)
        );
        assert_eq!(
            (
                attrs.str_per_level,
                attrs.agi_per_level,
                attrs.int_per_level
            ),
            (2.4, 2.8, 1.5)
        );

        assert_eq!(stats.attack_damage, 44.0);
        assert_eq!(stats.attack_range, 150.0);
        assert!(stats.is_melee());
        assert_eq!(stats.base_attack_time, 1.7);
        assert_eq!(stats.attack_point, 0.3);
        assert_eq!(stats.attack_backswing, 0.3);
        assert_eq!(stats.turn_rate, 0.6);
        assert_eq!(stats.move_speed, 310.0);
        assert!(
            (stats.attack_speed - 118.0 / 100.0 / 1.7).abs() < 1e-4,
            "APS {}",
            stats.attack_speed
        );
        assert!((stats.attack_speed - 0.694118).abs() < 1e-5);
    }

    #[test]
    fn slark_matches_spec() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        assert_eq!(HeroId::from_cli("slark"), Some(HeroId::Slark));
        assert_eq!(HeroId::Slark.name(), "Slark");
        assert_eq!(HeroId::Slark.primary_label(), "AGI");
        let def = HeroId::Slark.def();
        assert_eq!(
            def.abilities,
            [
                AbilityId::DarkPact,
                AbilityId::Pounce,
                AbilityId::EssenceShift,
                AbilityId::ShadowDance,
            ]
        );

        let (health, mana, stats, attrs) = def.vitals();
        assert!((health.max - 560.0).abs() < 1e-3, "health {}", health.max);
        assert!((health.regen_per_sec - 2.25).abs() < 1e-4);
        assert!((mana.max - 267.0).abs() < 1e-3, "mana {}", mana.max);
        assert!((mana.regen_per_sec - 0.8).abs() < 1e-4);
        assert!(stats.armor.abs() < 1e-3, "armor {}", stats.armor);
        let taken = apply_damage(100.0, DamageType::Magical, &stats);
        assert!(
            (taken - 72.5).abs() < 0.01,
            "27.5% magic resist: took {taken}"
        );
        assert_eq!(
            (attrs.strength, attrs.agility, attrs.intelligence),
            (20.0, 21.0, 16.0)
        );
        assert_eq!(
            (
                attrs.str_per_level,
                attrs.agi_per_level,
                attrs.int_per_level
            ),
            (2.1, 1.5, 1.9)
        );

        assert_eq!(stats.attack_damage, 61.0);
        assert_eq!(stats.attack_range, 150.0);
        assert!(stats.is_melee());
        assert_eq!(stats.base_attack_time, 1.7);
        assert_eq!(stats.attack_point, 0.5);
        assert_eq!(stats.attack_backswing, 0.3);
        assert_eq!(stats.turn_rate, 0.7);
        assert_eq!(stats.move_speed, 300.0);
        assert_eq!(stats.base_attack_speed, 120.0);
        assert!(
            (stats.attack_speed - 141.0 / 100.0 / 1.7).abs() < 1e-4,
            "APS {}",
            stats.attack_speed
        );
        assert!((stats.attack_speed - 0.829412).abs() < 1e-5);
    }

    #[test]
    fn gyrocopter_matches_spec() {
        use crate::combat::apply_damage;
        use crate::components::DamageType;

        assert_eq!(HeroId::from_cli("gyrocopter"), Some(HeroId::Gyrocopter));
        assert_eq!(HeroId::Gyrocopter.name(), "Gyrocopter");
        assert_eq!(HeroId::Gyrocopter.primary_label(), "AGI");
        let def = HeroId::Gyrocopter.def();
        assert_eq!(
            def.abilities,
            [
                AbilityId::RocketBarrage,
                AbilityId::HomingMissile,
                AbilityId::FlakCannon,
                AbilityId::CallDown,
            ]
        );

        let (health, mana, stats, attrs) = def.vitals();
        assert!((health.max - 604.0).abs() < 1e-3, "health {}", health.max);
        assert!((health.regen_per_sec - 2.45).abs() < 1e-4);
        assert!((mana.max - 327.0).abs() < 1e-3, "mana {}", mana.max);
        assert!((mana.regen_per_sec - 1.05).abs() < 1e-4);
        assert!((stats.armor - 1.0).abs() < 1e-3, "armor {}", stats.armor);
        let taken = apply_damage(100.0, DamageType::Magical, &stats);
        assert!(
            (taken - 72.5).abs() < 0.01,
            "27.5% magic resist: took {taken}"
        );
        assert_eq!(
            (attrs.strength, attrs.agility, attrs.intelligence),
            (22.0, 25.0, 21.0)
        );
        assert_eq!(
            (
                attrs.str_per_level,
                attrs.agi_per_level,
                attrs.int_per_level
            ),
            (2.5, 3.4, 2.4)
        );

        // 46 at level 1: the stored 21 plus 25 Agility (the primary).
        assert_eq!(stats.attack_damage, 46.0);
        assert_eq!(def.combat.attack_damage, 21.0);
        assert_eq!(stats.attack_range, 365.0);
        assert!(!stats.is_melee());
        let projectile_speed = crate::basic_attack::projectile_speed_for(stats.base_attack_range())
            + stats.attack_projectile_speed_bonus;
        assert!(
            (projectile_speed - 3000.0).abs() < 1e-3,
            "speed {projectile_speed}"
        );
        assert_eq!(stats.base_attack_time, 1.7);
        assert_eq!(stats.attack_point, 0.2);
        assert_eq!(stats.attack_backswing, 0.1);
        assert_eq!(stats.turn_rate, 0.6);
        assert_eq!(stats.move_speed, 315.0);
        assert_eq!(stats.base_attack_speed, 115.0);
        assert!(
            (stats.attack_speed - 140.0 / 100.0 / 1.7).abs() < 1e-4,
            "APS {}",
            stats.attack_speed
        );
        assert!((stats.attack_speed - 0.823529).abs() < 1e-5);
    }
}
