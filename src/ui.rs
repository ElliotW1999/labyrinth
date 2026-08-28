//! HUD: vitals, spell bar with icons/ranks, unspent points, and minimap.

use bevy::prelude::*;

use crate::components::{
    AbilityId, AbilityLoadout, Ancient, Creep, Health, HeroProgress, Mana, PlayerHero,
    PlayerWallet, Team, Tower,
};
use crate::resources::MatchConfig;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(
                Update,
                (
                    refresh_hud_text,
                    refresh_spell_bar,
                    handle_spell_level_clicks,
                    refresh_minimap,
                ),
            );
    }
}

#[derive(Component)]
struct HudRoot;

#[derive(Component)]
struct HudVitals;

#[derive(Component)]
struct HudLevel;

#[derive(Component)]
struct HudGold;

#[derive(Component)]
struct HudSkillPoints;

#[derive(Component)]
struct SpellBarRoot;

#[derive(Component)]
struct SpellIcon {
    index: usize,
}

#[derive(Component)]
struct SpellRankText {
    index: usize,
}

#[derive(Component)]
struct SpellCdText {
    index: usize,
}

#[derive(Component)]
struct SpellLevelButton {
    index: usize,
}

#[derive(Component)]
struct MinimapRoot;

#[derive(Component)]
struct MinimapDot {
    kind: MinimapDotKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MinimapDotKind {
    Hero,
    AllyCreep,
    EnemyCreep,
    AllyTower,
    EnemyTower,
    Ancient,
}

const MINIMAP_SIZE: f32 = 180.0;
const MAX_MINIMAP_DOTS: usize = 64;

fn spawn_hud(mut commands: Commands) {
    commands
        .spawn((
            Name::new("HUD"),
            HudRoot,
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .with_children(|root| {
            // Top-left status panel
            root.spawn(Node {
                position_type: PositionType::Absolute,
                top: px(12),
                left: px(12),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                ..default()
            })
            .with_children(|panel| {
                panel.spawn((
                    HudVitals,
                    Text::new("HP -- / --   MP -- / --"),
                    TextFont::from_font_size(20.0),
                    TextColor(Color::srgb(0.92, 0.95, 1.0)),
                ));
                panel.spawn((
                    HudLevel,
                    Text::new("Level 1   XP 0 / 100"),
                    TextFont::from_font_size(18.0),
                    TextColor(Color::srgb(0.55, 0.9, 1.0)),
                ));
                panel.spawn((
                    HudGold,
                    Text::new("Gold: 0"),
                    TextFont::from_font_size(18.0),
                    TextColor(Color::srgb(1.0, 0.85, 0.35)),
                ));
                panel.spawn((
                    HudSkillPoints,
                    Text::new("Skill Points: 1"),
                    TextFont::from_font_size(18.0),
                    TextColor(Color::srgb(0.95, 0.7, 1.0)),
                ));
            });

            // Bottom-center spell bar
            root.spawn((
                SpellBarRoot,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(18),
                    left: percent(50),
                    margin: UiRect::left(px(-170)),
                    flex_direction: FlexDirection::Row,
                    column_gap: px(10),
                    padding: UiRect::all(px(8)),
                    border_radius: BorderRadius::all(px(8)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.05, 0.07, 0.1, 0.72)),
            ))
            .with_children(|bar| {
                for (i, id) in [
                    AbilityId::Dash,
                    AbilityId::Shockwave,
                    AbilityId::Bolt,
                    AbilityId::Nova,
                ]
                .into_iter()
                .enumerate()
                {
                    spawn_spell_icon(bar, i, id);
                }
            });

            // Help
            root.spawn((
                Text::new(
                    "RMB: move/attack  |  LMB: confirm spell  |  Ctrl+QWER or +: rank up  |  Space: stop",
                ),
                TextFont::from_font_size(14.0),
                TextColor(Color::srgba(0.8, 0.85, 0.9, 0.8)),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(100),
                    left: px(16),
                    ..default()
                },
            ));

            // Minimap
            root.spawn((
                MinimapRoot,
                Node {
                    position_type: PositionType::Absolute,
                    right: px(14),
                    bottom: px(14),
                    width: px(MINIMAP_SIZE),
                    height: px(MINIMAP_SIZE),
                    border: UiRect::all(px(2)),
                    overflow: Overflow::clip(),
                    border_radius: BorderRadius::all(px(6)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.08, 0.12, 0.1, 0.85)),
                BorderColor::all(Color::srgb(0.35, 0.45, 0.4)),
            ))
            .with_children(|map| {
                map.spawn((
                    Text::new("MAP"),
                    TextFont::from_font_size(12.0),
                    TextColor(Color::srgba(0.7, 0.8, 0.75, 0.7)),
                    Node {
                        position_type: PositionType::Absolute,
                        top: px(4),
                        left: px(6),
                        ..default()
                    },
                ));
                for _ in 0..MAX_MINIMAP_DOTS {
                    map.spawn((
                        MinimapDot {
                            kind: MinimapDotKind::AllyCreep,
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            width: px(6),
                            height: px(6),
                            left: px(-20),
                            top: px(-20),
                            border_radius: BorderRadius::all(px(3)),
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                        Visibility::Hidden,
                    ));
                }
            });
        });
}

fn spawn_spell_icon(parent: &mut ChildSpawnerCommands, index: usize, id: AbilityId) {
    parent
        .spawn((
            SpellIcon { index },
            Node {
                width: px(72),
                height: px(72),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(2)),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(id.placeholder_color()),
            BorderColor::all(Color::srgb(0.15, 0.15, 0.18)),
        ))
        .with_children(|icon| {
            icon.spawn((
                Text::new(id.hotkey_label()),
                TextFont::from_font_size(26.0),
                TextColor(Color::WHITE),
            ));
            icon.spawn((
                SpellRankText { index },
                Text::new("Lv 0"),
                TextFont::from_font_size(12.0),
                TextColor(Color::srgb(0.95, 0.95, 1.0)),
            ));
            icon.spawn((
                SpellCdText { index },
                Text::new(""),
                TextFont::from_font_size(11.0),
                TextColor(Color::srgb(1.0, 0.9, 0.4)),
            ));
            icon.spawn((
                Button,
                SpellLevelButton { index },
                Node {
                    position_type: PositionType::Absolute,
                    top: px(-6),
                    right: px(-6),
                    width: px(22),
                    height: px(22),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    border_radius: BorderRadius::all(px(11)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.95, 0.8, 0.2)),
                Visibility::Hidden,
            ))
            .with_children(|btn| {
                btn.spawn((
                    Text::new("+"),
                    TextFont::from_font_size(16.0),
                    TextColor(Color::BLACK),
                ));
            });
        });
}

fn refresh_hud_text(
    hero: Query<(&Health, &Mana, &PlayerWallet, &HeroProgress), With<PlayerHero>>,
    mut vitals: Query<&mut Text, (With<HudVitals>, Without<HudLevel>, Without<HudGold>, Without<HudSkillPoints>)>,
    mut level: Query<&mut Text, (With<HudLevel>, Without<HudVitals>, Without<HudGold>, Without<HudSkillPoints>)>,
    mut gold: Query<&mut Text, (With<HudGold>, Without<HudVitals>, Without<HudLevel>, Without<HudSkillPoints>)>,
    mut points: Query<&mut Text, (With<HudSkillPoints>, Without<HudVitals>, Without<HudLevel>, Without<HudGold>)>,
) {
    let Ok((health, mana, wallet, progress)) = hero.single() else {
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
    if let Ok(mut text) = points.single_mut() {
        let label = if progress.skill_points > 0 {
            format!("Skill Points: {}  (Ctrl+QWER or +)", progress.skill_points)
        } else {
            "Skill Points: 0".into()
        };
        *text = Text::new(label);
    }
}

fn refresh_spell_bar(
    hero: Query<(&AbilityLoadout, &HeroProgress), With<PlayerHero>>,
    mut ranks: Query<(&SpellRankText, &mut Text), Without<SpellCdText>>,
    mut cds: Query<(&SpellCdText, &mut Text), Without<SpellRankText>>,
    mut buttons: Query<(&SpellLevelButton, &mut Visibility)>,
    mut icons: Query<(&SpellIcon, &mut BorderColor)>,
) {
    let Ok((loadout, progress)) = hero.single() else {
        return;
    };

    for (meta, mut text) in &mut ranks {
        if let Some(slot) = loadout.slots.get(meta.index) {
            *text = Text::new(format!("Lv {}/{}", slot.rank, slot.id.max_rank()));
        }
    }

    for (meta, mut text) in &mut cds {
        if let Some(slot) = loadout.slots.get(meta.index) {
            if slot.rank == 0 {
                *text = Text::new("locked");
            } else if slot.cooldown_remaining > 0.05 {
                *text = Text::new(format!("{:.0}s", slot.cooldown_remaining.ceil()));
            } else {
                *text = Text::new("ready");
            }
        }
    }

    for (meta, mut vis) in &mut buttons {
        let show = loadout
            .slots
            .get(meta.index)
            .is_some_and(|slot| {
                progress.skill_points > 0 && slot.id.can_rank_up(slot.rank, progress.level)
            });
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    for (meta, mut border) in &mut icons {
        let highlight = loadout.slots.get(meta.index).is_some_and(|slot| {
            progress.skill_points > 0 && slot.id.can_rank_up(slot.rank, progress.level)
        });
        *border = BorderColor::all(if highlight {
            Color::srgb(1.0, 0.9, 0.3)
        } else {
            Color::srgb(0.15, 0.15, 0.18)
        });
    }
}

fn handle_spell_level_clicks(
    interactions: Query<(&Interaction, &SpellLevelButton), Changed<Interaction>>,
    mut hero: Query<(&mut AbilityLoadout, &mut HeroProgress), With<PlayerHero>>,
) {
    let Ok((mut loadout, mut progress)) = hero.single_mut() else {
        return;
    };
    for (interaction, button) in &interactions {
        if *interaction == Interaction::Pressed {
            loadout.try_rank_up(button.index, progress.level, &mut progress.skill_points);
        }
    }
}

fn refresh_minimap(
    config: Res<MatchConfig>,
    hero: Query<(&Transform, &Team), With<PlayerHero>>,
    creeps: Query<(&Transform, &Team), With<Creep>>,
    towers: Query<(&Transform, &Team), (With<Tower>, Without<Ancient>)>,
    ancients: Query<&Transform, With<Ancient>>,
    mut dots: Query<(&mut MinimapDot, &mut Node, &mut BackgroundColor, &mut Visibility)>,
) {
    let Ok((hero_tf, hero_team)) = hero.single() else {
        return;
    };
    let extent = config.map_half_extent.max(1.0);

    let mut entries: Vec<(MinimapDotKind, Vec3)> = Vec::new();
    entries.push((MinimapDotKind::Hero, hero_tf.translation));

    for (tf, team) in &creeps {
        let kind = if *team == *hero_team {
            MinimapDotKind::AllyCreep
        } else {
            MinimapDotKind::EnemyCreep
        };
        entries.push((kind, tf.translation));
    }
    for (tf, team) in &towers {
        let kind = if *team == *hero_team {
            MinimapDotKind::AllyTower
        } else {
            MinimapDotKind::EnemyTower
        };
        entries.push((kind, tf.translation));
    }
    for tf in &ancients {
        entries.push((MinimapDotKind::Ancient, tf.translation));
    }

    let mut dot_iter = dots.iter_mut();
    for (i, (kind, pos)) in entries.into_iter().enumerate() {
        if i >= MAX_MINIMAP_DOTS {
            break;
        }
        let Some((mut dot, mut node, mut color, mut vis)) = dot_iter.next() else {
            break;
        };
        dot.kind = kind;
        let (x, y) = world_to_minimap(pos, extent);
        node.left = px(x);
        node.top = px(y);
        let (size, col) = match kind {
            MinimapDotKind::Hero => (8.0, Color::srgb(0.3, 0.85, 1.0)),
            MinimapDotKind::AllyCreep => (5.0, Color::srgb(0.35, 0.55, 1.0)),
            MinimapDotKind::EnemyCreep => (5.0, Color::srgb(1.0, 0.35, 0.3)),
            MinimapDotKind::AllyTower => (7.0, Color::srgb(0.2, 0.4, 0.95)),
            MinimapDotKind::EnemyTower => (7.0, Color::srgb(0.9, 0.25, 0.2)),
            MinimapDotKind::Ancient => (9.0, Color::srgb(1.0, 0.85, 0.3)),
        };
        node.width = px(size);
        node.height = px(size);
        *color = BackgroundColor(col);
        *vis = Visibility::Visible;
    }

    // Hide unused dots.
    for (_dot, mut node, mut color, mut vis) in dot_iter {
        node.left = px(-20.0);
        node.top = px(-20.0);
        *color = BackgroundColor(Color::NONE);
        *vis = Visibility::Hidden;
    }
}

fn world_to_minimap(pos: Vec3, half_extent: f32) -> (f32, f32) {
    let nx = ((pos.x / half_extent) * 0.5 + 0.5).clamp(0.0, 1.0);
    let nz = ((pos.z / half_extent) * 0.5 + 0.5).clamp(0.0, 1.0);
    // UI Y grows downward; world +Z is "south" on our camera, map similarly.
    let x = nx * (MINIMAP_SIZE - 8.0) + 1.0;
    let y = nz * (MINIMAP_SIZE - 8.0) + 1.0;
    (x, y)
}
