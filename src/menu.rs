//! Esc main menu: New Game, Settings, Quit.

use bevy::prelude::*;

use crate::abilities::{cancel_targeting_if_any, AbilityTargeting};
use crate::components::{Creep, PlayerHero, Projectile};
use crate::heroes::{spawn_hero_select_ui_force, HeroSelectRoot, LocalHeroChoice};
use crate::items::{InventoryContextMenu, ShopUiState};
use crate::ui::InventorySellMenu;
use crate::net::NetworkedHero;
use crate::obstacle_course::HazardOrb;
use crate::waves::WaveTimer;

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MainMenuState>()
            .add_systems(Startup, spawn_main_menu_ui)
            .add_systems(
                Update,
                (
                    toggle_main_menu,
                    handle_main_menu_buttons,
                    sync_main_menu_visibility,
                    sync_settings_visibility,
                )
                    .chain(),
            );
    }
}

#[derive(Resource, Debug, Default, Clone)]
pub struct MainMenuState {
    pub open: bool,
    pub settings_open: bool,
}

#[derive(Component)]
struct MainMenuRoot;

#[derive(Component)]
struct SettingsPanel;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum MainMenuButton {
    NewGame,
    Settings,
    Quit,
    SettingsClose,
}

fn spawn_main_menu_ui(mut commands: Commands) {
    commands
        .spawn((
            Name::new("Main Menu"),
            MainMenuRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.72)),
            Visibility::Hidden,
            ZIndex(200),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(320.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(12.0),
                    padding: UiRect::all(Val::Px(24.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(10.0)),
                    align_items: AlignItems::Stretch,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.08, 0.1, 0.14, 0.96)),
                BorderColor::all(Color::srgb(0.75, 0.8, 0.9)),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Labyrinth"),
                    TextFont::from_font_size(32.0),
                    TextColor(Color::srgb(0.95, 0.9, 0.7)),
                    Node {
                        margin: UiRect::bottom(Val::Px(8.0)),
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                ));
                spawn_menu_button(panel, MainMenuButton::NewGame, "New Game");
                spawn_menu_button(panel, MainMenuButton::Settings, "Settings");
                spawn_menu_button(panel, MainMenuButton::Quit, "Quit");
            });

            root.spawn((
                SettingsPanel,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(360.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(10.0),
                    padding: UiRect::all(Val::Px(20.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(10.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.07, 0.09, 0.12, 0.98)),
                BorderColor::all(Color::srgb(0.6, 0.7, 0.85)),
                Visibility::Hidden,
                ZIndex(210),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Settings"),
                    TextFont::from_font_size(24.0),
                    TextColor(Color::srgb(0.9, 0.95, 1.0)),
                ));
                panel.spawn((
                    Text::new("Display: Borderless fullscreen\nCamera: arrows / edge pan / F snap\nEsc: close menus"),
                    TextFont::from_font_size(14.0),
                    TextColor(Color::srgb(0.75, 0.8, 0.85)),
                ));
                spawn_menu_button(panel, MainMenuButton::SettingsClose, "Back");
            });
        });
}

fn spawn_menu_button(parent: &mut ChildSpawnerCommands, id: MainMenuButton, label: &str) {
    parent
        .spawn((
            Button,
            id,
            Node {
                height: Val::Px(40.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border_radius: BorderRadius::all(Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.2, 0.28, 0.38)),
            BorderColor::all(Color::srgb(0.45, 0.55, 0.7)),
        ))
        .with_children(|btn| {
            btn.spawn((
                Text::new(label.to_string()),
                TextFont::from_font_size(18.0),
                TextColor(Color::WHITE),
            ));
        });
}

fn toggle_main_menu(
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<MainMenuState>,
    mut shop: ResMut<ShopUiState>,
    mut sell: ResMut<InventoryContextMenu>,
    mut sell_vis: Query<&mut Visibility, With<InventorySellMenu>>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
    mut time: ResMut<Time<Virtual>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }

    // Nested closes first.
    if menu.settings_open {
        menu.settings_open = false;
        return;
    }
    if shop.open {
        shop.open = false;
        return;
    }
    if sell.slot.is_some() {
        sell.slot = None;
        if let Ok(mut vis) = sell_vis.single_mut() {
            *vis = Visibility::Hidden;
        }
        return;
    }
    if targeting.active.is_some() {
        cancel_targeting_if_any(&mut commands, &mut targeting);
        return;
    }

    menu.open = !menu.open;
    if menu.open {
        time.pause();
    } else {
        menu.settings_open = false;
        time.unpause();
    }
}

fn sync_main_menu_visibility(
    menu: Res<MainMenuState>,
    mut root: Query<&mut Visibility, (With<MainMenuRoot>, Without<SettingsPanel>)>,
) {
    let Ok(mut vis) = root.single_mut() else {
        return;
    };
    *vis = if menu.open {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

fn sync_settings_visibility(
    menu: Res<MainMenuState>,
    mut panel: Query<&mut Visibility, With<SettingsPanel>>,
) {
    let Ok(mut vis) = panel.single_mut() else {
        return;
    };
    *vis = if menu.open && menu.settings_open {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

fn handle_main_menu_buttons(
    interactions: Query<(&Interaction, &MainMenuButton), Changed<Interaction>>,
    mut menu: ResMut<MainMenuState>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
    mut choice: ResMut<LocalHeroChoice>,
    mut time: ResMut<Time<Virtual>>,
    mut wave: ResMut<WaveTimer>,
    heroes: Query<Entity, Or<(With<PlayerHero>, With<NetworkedHero>)>>,
    creeps: Query<Entity, With<Creep>>,
    projectiles: Query<Entity, Or<(With<Projectile>, With<HazardOrb>)>>,
    select_roots: Query<Entity, With<HeroSelectRoot>>,
) {
    for (interaction, button) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *button {
            MainMenuButton::NewGame => {
                for e in heroes.iter().chain(creeps.iter()).chain(projectiles.iter()) {
                    commands.entity(e).despawn();
                }
                for e in &select_roots {
                    commands.entity(e).despawn();
                }
                choice.hero = None;
                choice.spawned = false;
                choice.from_cli = false;
                wave.0 = Timer::from_seconds(8.0, TimerMode::Repeating);
                spawn_hero_select_ui_force(&mut commands);
                menu.open = false;
                menu.settings_open = false;
                time.unpause();
            }
            MainMenuButton::Settings => {
                menu.settings_open = true;
            }
            MainMenuButton::SettingsClose => {
                menu.settings_open = false;
            }
            MainMenuButton::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
}
