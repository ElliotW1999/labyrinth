//! Hero roster: selectable kits with unique spells and attribute growth.

use bevy::prelude::*;

use crate::components::{
    AbilityId, AbilityLoadout, AbilitySlot, CombatStats, Health, HeroAttributes, Mana, Team,
};
use crate::net::{NetConfig, NetMode, NetSession};
use crate::resources::SharedAssets;
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
    /// Strength bruiser — dash, shockwave, bolt, nova.
    Vanguard,
    /// Agility skirmisher — blink, flurry, caltrops, execute.
    Skirmisher,
    /// Intelligence caster — missile, frost, barrier, meteor.
    Arcanist,
}

impl HeroId {
    pub fn all() -> &'static [HeroId] {
        &[HeroId::Vanguard, HeroId::Skirmisher, HeroId::Arcanist]
    }

    pub fn from_cli(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "vanguard" | "1" => Some(HeroId::Vanguard),
            "skirmisher" | "2" => Some(HeroId::Skirmisher),
            "arcanist" | "3" => Some(HeroId::Arcanist),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            HeroId::Vanguard => 0,
            HeroId::Skirmisher => 1,
            HeroId::Arcanist => 2,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => HeroId::Skirmisher,
            2 => HeroId::Arcanist,
            _ => HeroId::Vanguard,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            HeroId::Vanguard => "Vanguard",
            HeroId::Skirmisher => "Skirmisher",
            HeroId::Arcanist => "Arcanist",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            HeroId::Vanguard => "Str bruiser — dash in, shockwave, bolt, nova",
            HeroId::Skirmisher => "Agi duelist — blink, flurry, caltrops, execute",
            HeroId::Arcanist => "Int caster — missile, frost, barrier, meteor",
        }
    }

    pub fn primary_label(self) -> &'static str {
        match self {
            HeroId::Vanguard => "STR",
            HeroId::Skirmisher => "AGI",
            HeroId::Arcanist => "INT",
        }
    }

    pub fn def(self) -> HeroDef {
        match self {
            HeroId::Vanguard => HeroDef {
                id: self,
                attributes: HeroAttributes {
                    strength: 22.0,
                    agility: 16.0,
                    intelligence: 14.0,
                    str_per_level: 2.6,
                    agi_per_level: 1.7,
                    int_per_level: 1.6,
                },
                base_health: 380.0,
                base_health_regen: 0.6,
                base_mana: 90.0,
                base_mana_regen: 0.8,
                combat: hero_combat(58.0, 5.5, 1.7, 0.3, 0.35, 7.5, 2.0, 0.7, 11.5),
                abilities: [
                    AbilityId::Dash,
                    AbilityId::Shockwave,
                    AbilityId::Bolt,
                    AbilityId::Nova,
                ],
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
                },
                base_health: 320.0,
                base_health_regen: 0.4,
                base_mana: 100.0,
                base_mana_regen: 0.9,
                combat: hero_combat(52.0, 6.5, 1.5, 0.25, 0.3, 10.0, 1.2, 0.6, 12.8),
                abilities: [
                    AbilityId::Blink,
                    AbilityId::Flurry,
                    AbilityId::Caltrops,
                    AbilityId::Execute,
                ],
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
                },
                base_health: 300.0,
                base_health_regen: 0.35,
                base_mana: 140.0,
                base_mana_regen: 1.4,
                combat: hero_combat(48.0, 9.5, 1.6, 0.35, 0.4, 6.5, 0.8, 1.2, 11.2),
                abilities: [
                    AbilityId::ArcMissile,
                    AbilityId::FrostNova,
                    AbilityId::Barrier,
                    AbilityId::Meteor,
                ],
            },
        }
    }

    pub fn loadout(self) -> AbilityLoadout {
        let abilities = self.def().abilities;
        AbilityLoadout {
            slots: [
                AbilitySlot::fresh(abilities[0]),
                AbilitySlot::fresh(abilities[1]),
                AbilitySlot::fresh(abilities[2]),
                AbilitySlot::fresh(abilities[3]),
            ],
        }
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
}

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
) -> CombatStats {
    CombatStats {
        attack_damage,
        attack_range,
        base_attack_speed: 100.0,
        attack_speed_flat: 0.0,
        attack_speed_mult: 0.0,
        base_attack_time,
        attack_speed: 0.0,
        attack_point: attack_point.clamp(0.0, 0.5),
        attack_backswing: attack_backswing.clamp(0.0, 0.5),
        turn_rate,
        armor,
        magic_resist,
        move_speed,
    }
}

impl HeroDef {
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
struct HeroSelectRoot;

#[derive(Component, Debug, Clone, Copy)]
struct HeroSelectButton {
    hero: HeroId,
}

pub struct HeroesPlugin;

impl Plugin for HeroesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalHeroChoice>()
            .add_systems(Startup, spawn_hero_select_ui.after(crate::resources::load_shared_assets))
            .add_systems(
                Update,
                (
                    handle_hero_select_input,
                    handle_hero_select_buttons,
                    maybe_spawn_local_hero,
                )
                    .chain(),
            );
    }
}

fn spawn_hero_select_ui(mut commands: Commands, choice: Res<LocalHeroChoice>) {
    if choice.from_cli && choice.hero.is_some() {
        return;
    }

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
                Text::new("Press 1 / 2 / 3  ·  or click a card  ·  Enter to confirm"),
                TextFont::from_font_size(16.0),
                TextColor(Color::srgb(0.7, 0.8, 0.9)),
            ));
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(16),
                ..default()
            })
            .with_children(|row| {
                for (i, hero) in HeroId::all().iter().copied().enumerate() {
                    let def = hero.def();
                    row.spawn((
                        Button,
                        HeroSelectButton { hero },
                        Node {
                            width: px(260),
                            height: px(220),
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            row_gap: px(6),
                            padding: UiRect::all(px(14)),
                            border: UiRect::all(px(2)),
                            border_radius: BorderRadius::all(px(10)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.12, 0.14, 0.18, 0.95)),
                        BorderColor::all(Color::srgb(0.35, 0.4, 0.5)),
                    ))
                    .with_children(|card| {
                        card.spawn((
                            Text::new(format!("{}  ·  {}", i + 1, hero.name())),
                            TextFont::from_font_size(24.0),
                            TextColor(Color::WHITE),
                        ));
                        card.spawn((
                            Text::new(format!(
                                "{}  Str {:.0}  Agi {:.0}  Int {:.0}",
                                hero.primary_label(),
                                def.attributes.strength,
                                def.attributes.agility,
                                def.attributes.intelligence
                            )),
                            TextFont::from_font_size(14.0),
                            TextColor(Color::srgb(0.95, 0.8, 0.55)),
                        ));
                        card.spawn((
                            Text::new(format!(
                                "+{:.1} / +{:.1} / +{:.1} per level",
                                def.attributes.str_per_level,
                                def.attributes.agi_per_level,
                                def.attributes.int_per_level
                            )),
                            TextFont::from_font_size(13.0),
                            TextColor(Color::srgb(0.75, 0.85, 0.95)),
                        ));
                        card.spawn((
                            Text::new(hero.blurb()),
                            TextFont::from_font_size(13.0),
                            TextColor(Color::srgb(0.8, 0.85, 0.9)),
                        ));
                    });
                }
            });
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
    if keys.just_pressed(KeyCode::Digit1) || keys.just_pressed(KeyCode::Numpad1) {
        choice.hero = Some(HeroId::Vanguard);
    } else if keys.just_pressed(KeyCode::Digit2) || keys.just_pressed(KeyCode::Numpad2) {
        choice.hero = Some(HeroId::Skirmisher);
    } else if keys.just_pressed(KeyCode::Digit3) || keys.just_pressed(KeyCode::Numpad3) {
        choice.hero = Some(HeroId::Arcanist);
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
    let confirm = choice.from_cli
        || !roots.is_empty()
            && (choice.hero.is_some());
    if !confirm {
        return;
    }

    match config.mode {
        NetMode::Offline => {
            spawn_hero_entity(
                &mut commands,
                &assets,
                Team::Radiant,
                true,
                1,
                Vec3::new(-44.0, 0.9, -44.0),
                hero,
            );
            choice.spawned = true;
        }
        NetMode::Host => {
            let entity = spawn_hero_entity(
                &mut commands,
                &assets,
                Team::Radiant,
                true,
                1,
                Vec3::new(-44.0, 0.9, -44.0),
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

    for entity in &roots {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
