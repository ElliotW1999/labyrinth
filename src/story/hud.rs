//! Story Mode HUD: lives, depth and objective at the top centre, and the game over /
//! victory overlay with its two buttons.

use bevy::prelude::*;

use super::lives::HeroLives;
use super::rooms::FINAL_DEPTH;
use super::{RoomHostile, StoryPhase, StoryRun};
use crate::components::{Health, PlayerHero};
use crate::map::{GameMode, MapState, ResetMatchRequest};
use crate::menu::MainMenuState;

#[derive(Component)]
pub struct StoryHudRoot;

#[derive(Component)]
pub struct StoryLivesText;

#[derive(Component)]
pub struct StoryDepthText;

#[derive(Component)]
pub struct StoryObjectiveText;

#[derive(Component)]
pub struct StoryOverlayRoot;

#[derive(Component)]
pub struct StoryOverlayTitle;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoryOverlayButton {
    /// Start a new Story run.
    Retry,
    /// Leave Story Mode for the classic game.
    Classic,
}

impl StoryOverlayButton {
    /// The match each button starts (the current hero is kept).
    pub fn request(self) -> ResetMatchRequest {
        ResetMatchRequest {
            change_hero: false,
            mode: match self {
                StoryOverlayButton::Retry => GameMode::Story,
                StoryOverlayButton::Classic => GameMode::Classic,
            },
        }
    }
}

/// Above the HUD, below the Esc menu (ZIndex 200).
const OVERLAY_Z: i32 = 150;

pub fn spawn_story_hud(mut commands: Commands) {
    commands
        .spawn((
            Name::new("Story HUD"),
            StoryHudRoot,
            Node {
                position_type: PositionType::Absolute,
                top: px(10),
                left: percent(50),
                margin: UiRect::left(px(-170)),
                width: px(340),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(2),
                padding: UiRect::all(px(8)),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.07, 0.1, 0.7)),
            Visibility::Hidden,
        ))
        .with_children(|panel| {
            panel.spawn((
                StoryLivesText,
                Text::new(""),
                TextFont::from_font_size(18.0),
                TextColor(Color::srgb(1.0, 0.8, 0.8)),
            ));
            panel.spawn((
                StoryDepthText,
                Text::new(""),
                TextFont::from_font_size(15.0),
                TextColor(Color::srgb(0.92, 0.94, 0.98)),
            ));
            panel.spawn((
                StoryObjectiveText,
                Text::new(""),
                TextFont::from_font_size(14.0),
                TextColor(Color::srgb(0.7, 0.9, 0.75)),
            ));
        });

    commands
        .spawn((
            Name::new("Story Overlay"),
            StoryOverlayRoot,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                row_gap: px(14),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.78)),
            Visibility::Hidden,
            ZIndex(OVERLAY_Z),
        ))
        .with_children(|root| {
            root.spawn((
                StoryOverlayTitle,
                Text::new(""),
                TextFont::from_font_size(44.0),
                TextColor(Color::srgb(0.95, 0.9, 0.7)),
            ));
            spawn_overlay_button(root, StoryOverlayButton::Retry, "Retry Story");
            spawn_overlay_button(root, StoryOverlayButton::Classic, "Classic Game");
        });
}

fn spawn_overlay_button(parent: &mut ChildSpawnerCommands, id: StoryOverlayButton, label: &str) {
    parent
        .spawn((
            Button,
            id,
            Node {
                width: px(220),
                height: px(44),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.2, 0.28, 0.38)),
            BorderColor::all(Color::srgb(0.45, 0.55, 0.7)),
        ))
        .with_children(|button| {
            button.spawn((
                Text::new(label.to_string()),
                TextFont::from_font_size(18.0),
                TextColor(Color::WHITE),
            ));
        });
}

/// The HUD shows only in a running Story match.
pub fn story_hud_visible(mode: GameMode, map_generated: bool) -> bool {
    mode == GameMode::Story && map_generated
}

/// The objective line for the current phase.
pub fn objective_text(phase: StoryPhase, hostiles_left: usize) -> String {
    match phase {
        StoryPhase::Idle | StoryPhase::Loading(_) | StoryPhase::Transition(_) => String::new(),
        StoryPhase::Active => format!("Defeat all enemies ({hostiles_left} left)"),
        StoryPhase::Cleared => "Choose a path".to_string(),
        StoryPhase::GameOver => "Game over".to_string(),
        StoryPhase::Victory => "Victory".to_string(),
    }
}

/// "Depth d / FINAL_DEPTH - Room name".
pub fn depth_text(run: &StoryRun) -> String {
    match run.room {
        Some(room) => {
            let def = room.definition();
            let note = if def.description.is_empty() {
                String::new()
            } else {
                format!(" ({})", def.description)
            };
            format!("Depth {} / {FINAL_DEPTH} - {}{note}", run.depth, def.name)
        }
        None => format!("Depth {} / {FINAL_DEPTH}", run.depth),
    }
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn sync_story_hud(
    mode: Option<Res<GameMode>>,
    map: Option<Res<MapState>>,
    run: Res<StoryRun>,
    lives: Query<&HeroLives, With<PlayerHero>>,
    hostiles: Query<&Health, With<RoomHostile>>,
    mut root: Query<&mut Visibility, With<StoryHudRoot>>,
    mut lives_text: Query<&mut Text, With<StoryLivesText>>,
    mut depth: Query<&mut Text, (With<StoryDepthText>, Without<StoryLivesText>)>,
    mut objective: Query<
        &mut Text,
        (
            With<StoryObjectiveText>,
            Without<StoryLivesText>,
            Without<StoryDepthText>,
        ),
    >,
) {
    let visible = story_hud_visible(
        mode.map(|m| *m).unwrap_or_default(),
        map.is_some_and(|m| m.generated),
    );
    if let Ok(mut visibility) = root.single_mut() {
        *visibility = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    if !visible {
        return;
    }
    if let (Ok(mut text), Ok(lives)) = (lives_text.single_mut(), lives.single()) {
        text.0 = format!("Lives: {}", lives.remaining);
    }
    if let Ok(mut text) = depth.single_mut() {
        text.0 = depth_text(&run);
    }
    if let Ok(mut text) = objective.single_mut() {
        let left = hostiles.iter().filter(|health| health.is_alive()).count();
        text.0 = objective_text(run.phase, left);
    }
}

/// Game over and victory cover the screen; the game keeps running behind them.
#[allow(clippy::type_complexity)]
pub fn sync_story_overlay(
    mode: Option<Res<GameMode>>,
    run: Res<StoryRun>,
    mut root: Query<&mut Visibility, With<StoryOverlayRoot>>,
    mut title: Query<&mut Text, With<StoryOverlayTitle>>,
) {
    let story = mode.is_some_and(|m| *m == GameMode::Story);
    let (visible, label) = match run.phase {
        StoryPhase::GameOver if story => (true, "Game Over"),
        StoryPhase::Victory if story => (true, "Victory!"),
        _ => (false, ""),
    };
    if let Ok(mut visibility) = root.single_mut() {
        *visibility = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    if visible && let Ok(mut text) = title.single_mut() {
        text.0 = label.to_string();
    }
}

pub fn handle_overlay_buttons(
    interactions: Query<(&Interaction, &StoryOverlayButton), Changed<Interaction>>,
    menu: Option<Res<MainMenuState>>,
    run: Res<StoryRun>,
    mut reset: MessageWriter<ResetMatchRequest>,
) {
    // Only live while the overlay is up, and never through the Esc menu on top of it.
    if menu.is_some_and(|m| m.open)
        || !matches!(run.phase, StoryPhase::GameOver | StoryPhase::Victory)
    {
        return;
    }
    for (interaction, button) in &interactions {
        if *interaction == Interaction::Pressed {
            reset.write(button.request());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::IntoSystem;

    #[test]
    fn hud_systems_initialize() {
        let mut world = World::new();
        IntoSystem::into_system(sync_story_hud).initialize(&mut world);
        IntoSystem::into_system(sync_story_overlay).initialize(&mut world);
        IntoSystem::into_system(handle_overlay_buttons).initialize(&mut world);
        IntoSystem::into_system(spawn_story_hud).initialize(&mut world);
    }

    #[test]
    fn hud_is_hidden_outside_a_running_story_match() {
        assert!(story_hud_visible(GameMode::Story, true));
        assert!(!story_hud_visible(GameMode::Story, false));
        assert!(!story_hud_visible(GameMode::Classic, true));
    }

    #[test]
    fn objective_text_follows_the_phase() {
        assert_eq!(
            objective_text(StoryPhase::Active, 3),
            "Defeat all enemies (3 left)"
        );
        assert_eq!(objective_text(StoryPhase::Cleared, 0), "Choose a path");
        assert_eq!(objective_text(StoryPhase::Victory, 0), "Victory");
        assert_eq!(objective_text(StoryPhase::GameOver, 0), "Game over");
        assert_eq!(objective_text(StoryPhase::Idle, 0), "");
    }

    #[test]
    fn depth_text_names_the_room() {
        let mut run = StoryRun::default();
        assert_eq!(depth_text(&run), format!("Depth 0 / {FINAL_DEPTH}"));
        run.depth = 2;
        run.room = Some(super::super::rooms::safe_room());
        assert_eq!(
            depth_text(&run),
            format!("Depth 2 / {FINAL_DEPTH} - Safe Camp (Rest and resupply)")
        );
    }

    #[test]
    fn overlay_buttons_start_story_or_classic_with_the_same_hero() {
        let retry = StoryOverlayButton::Retry.request();
        assert_eq!((retry.change_hero, retry.mode), (false, GameMode::Story));
        let classic = StoryOverlayButton::Classic.request();
        assert_eq!(
            (classic.change_hero, classic.mode),
            (false, GameMode::Classic)
        );
    }

    #[test]
    fn overlay_shows_only_after_game_over_or_victory() {
        let mut app = App::new();
        app.insert_resource(GameMode::Story)
            .init_resource::<StoryRun>()
            .add_systems(Update, sync_story_overlay);
        let overlay = app
            .world_mut()
            .spawn((StoryOverlayRoot, Visibility::Hidden))
            .id();
        let title = app
            .world_mut()
            .spawn((StoryOverlayTitle, Text::new("")))
            .id();
        for (phase, shown, label) in [
            (StoryPhase::Active, false, ""),
            (StoryPhase::GameOver, true, "Game Over"),
            (StoryPhase::Victory, true, "Victory!"),
            (StoryPhase::Cleared, false, ""),
        ] {
            app.world_mut().resource_mut::<StoryRun>().phase = phase;
            app.update();
            let visibility = *app.world().get::<Visibility>(overlay).unwrap();
            assert_eq!(visibility != Visibility::Hidden, shown, "{phase:?}");
            if shown {
                assert_eq!(app.world().get::<Text>(title).unwrap().0, label);
            }
        }
        // In classic mode a stale phase never shows the overlay.
        *app.world_mut().resource_mut::<GameMode>() = GameMode::Classic;
        app.world_mut().resource_mut::<StoryRun>().phase = StoryPhase::GameOver;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(overlay).unwrap(),
            Visibility::Hidden
        );
    }
}
