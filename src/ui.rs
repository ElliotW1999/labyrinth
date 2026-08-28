//! Minimal HUD: vitals, gold, level/XP, ability cooldowns, and controls help.

use bevy::prelude::*;

use crate::components::{AbilityLoadout, Health, HeroProgress, Mana, PlayerHero, PlayerWallet};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, refresh_hud);
    }
}

#[derive(Component)]
struct HudRoot;

#[derive(Component)]
struct HudVitals;

#[derive(Component)]
struct HudGold;

#[derive(Component)]
struct HudLevel;

#[derive(Component)]
struct HudAbilities;

fn spawn_hud(mut commands: Commands) {
    commands
        .spawn((
            Name::new("HUD"),
            HudRoot,
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                padding: UiRect::all(px(16)),
                ..default()
            },
        ))
        .with_children(|parent| {
            parent.spawn((
                HudVitals,
                Text::new("HP -- / --   MP -- / --"),
                TextFont::from_font_size(22.0),
                TextColor(Color::srgb(0.92, 0.95, 1.0)),
                Node {
                    margin: UiRect::bottom(px(6)),
                    ..default()
                },
            ));
            parent.spawn((
                HudLevel,
                Text::new("Level 1   XP 0 / 100"),
                TextFont::from_font_size(20.0),
                TextColor(Color::srgb(0.55, 0.9, 1.0)),
                Node {
                    margin: UiRect::bottom(px(6)),
                    ..default()
                },
            ));
            parent.spawn((
                HudGold,
                Text::new("Gold: 0"),
                TextFont::from_font_size(20.0),
                TextColor(Color::srgb(1.0, 0.85, 0.35)),
                Node {
                    margin: UiRect::bottom(px(6)),
                    ..default()
                },
            ));
            parent.spawn((
                HudAbilities,
                Text::new("Q  W  E  R"),
                TextFont::from_font_size(20.0),
                TextColor(Color::srgb(0.85, 0.9, 1.0)),
                Node {
                    margin: UiRect::bottom(px(10)),
                    ..default()
                },
            ));
            parent.spawn((
                Text::new(
                    "RMB: move / attack unit   |   Space: stop   |   QWER abilities   |   Arrows pan  F re-center",
                ),
                TextFont::from_font_size(16.0),
                TextColor(Color::srgba(0.8, 0.85, 0.9, 0.85)),
            ));
        });
}

fn refresh_hud(
    hero: Query<
        (&Health, &Mana, &PlayerWallet, &AbilityLoadout, &HeroProgress),
        With<PlayerHero>,
    >,
    mut vitals: Query<
        &mut Text,
        (
            With<HudVitals>,
            Without<HudGold>,
            Without<HudAbilities>,
            Without<HudLevel>,
        ),
    >,
    mut level: Query<
        &mut Text,
        (
            With<HudLevel>,
            Without<HudVitals>,
            Without<HudGold>,
            Without<HudAbilities>,
        ),
    >,
    mut gold: Query<
        &mut Text,
        (
            With<HudGold>,
            Without<HudVitals>,
            Without<HudAbilities>,
            Without<HudLevel>,
        ),
    >,
    mut abilities: Query<
        &mut Text,
        (
            With<HudAbilities>,
            Without<HudVitals>,
            Without<HudGold>,
            Without<HudLevel>,
        ),
    >,
) {
    let Ok((health, mana, wallet, loadout, progress)) = hero.single() else {
        return;
    };

    if let Ok(mut text) = vitals.single_mut() {
        *text = Text::new(format!(
            "HP {hp:.0} / {hp_max:.0}    MP {mp:.0} / {mp_max:.0}",
            hp = health.current.max(0.0),
            hp_max = health.max,
            mp = mana.current,
            mp_max = mana.max,
        ));
    }

    if let Ok(mut text) = level.single_mut() {
        if progress.level >= 25 {
            *text = Text::new(format!("Level {}   MAX", progress.level));
        } else {
            *text = Text::new(format!(
                "Level {}   XP {} / {}",
                progress.level, progress.xp, progress.xp_to_next
            ));
        }
    }

    if let Ok(mut text) = gold.single_mut() {
        *text = Text::new(format!("Gold: {}", wallet.gold));
    }

    if let Ok(mut text) = abilities.single_mut() {
        let labels = ["Q", "W", "E", "R"];
        let parts: Vec<String> = loadout
            .slots
            .iter()
            .enumerate()
            .map(|(i, slot)| {
                if slot.cooldown_remaining > 0.05 {
                    format!(
                        "{l}({cd:.0})",
                        l = labels[i],
                        cd = slot.cooldown_remaining.ceil()
                    )
                } else {
                    format!("{l} ready", l = labels[i])
                }
            })
            .collect();
        *text = Text::new(parts.join("   "));
    }
}
