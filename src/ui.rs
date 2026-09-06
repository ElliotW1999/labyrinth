//! HUD: vitals, spell bar, inventory, shop, unspent points, and minimap.

use bevy::prelude::*;

use crate::components::{
    AbilityId, AbilityLoadout, Ancient, CombatStats, Creep, Health, HeroAttributes, HeroProgress,
    Mana, PlayerHero, PlayerWallet, Team, Tower,
};
use crate::heroes::HeroKind;
use crate::items::{
    Inventory, InventoryContextMenu, ItemId, ItemShop, PurchaseItemRequest, SellItemRequest,
    ShopUiState, StatusEffects, StatusKind,
};
use crate::net::NetStatus;
use crate::resources::MatchConfig;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(
                Update,
                (
                    refresh_hud_text,
                    refresh_hero_name,
                    refresh_spell_bar,
                    handle_spell_level_clicks,
                    refresh_inventory_bar,
                    handle_inventory_context_menu,
                    handle_inventory_sell_clicks,
                    refresh_buffs_text,
                    handle_shop_toggle_button,
                    sync_shop_panel_visibility,
                    handle_shop_buy_clicks,
                    handle_shop_backdrop_close,
                    handle_shop_close_keys,
                    refresh_shop_status,
                    update_item_tooltips,
                    refresh_net_status,
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
struct HudHeroName;

#[derive(Component)]
struct HudLevel;

#[derive(Component)]
struct HudGold;

#[derive(Component)]
struct HudSkillPoints;

#[derive(Component)]
struct HudBuffs;

#[derive(Component)]
struct HudNet;

#[derive(Component)]
struct HudAttributes;

#[derive(Component)]
struct HudCombatStats;

#[derive(Component)]
struct SpellBarRoot;

#[derive(Component)]
struct SpellIcon {
    index: usize,
}

#[derive(Component)]
struct SpellHotkeyLabel;

#[derive(Component)]
struct SpellNameLabel {
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
struct InventorySlotIcon {
    index: usize,
}

#[derive(Component)]
struct InventorySlotLabel {
    index: usize,
}

#[derive(Component)]
struct InventorySlotCd {
    index: usize,
}

#[derive(Component)]
struct ShopToggleButton;

#[derive(Component)]
struct ShopPanel;

#[derive(Component)]
struct ShopStatusText;

#[derive(Component)]
struct ShopBuyButton {
    item: ItemId,
}

#[derive(Component)]
struct ShopBackdrop;

#[derive(Component)]
struct ItemTooltip;

#[derive(Component)]
struct ItemTooltipText;

#[derive(Component)]
struct InventorySellMenu;

#[derive(Component)]
struct InventorySellButton;

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
const SHOP_BUTTON_SIZE: f32 = 44.0;

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
                    HudHeroName,
                    Text::new("Hero: —"),
                    TextFont::from_font_size(18.0),
                    TextColor(Color::srgb(0.85, 0.95, 0.7)),
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
                panel.spawn((
                    HudBuffs,
                    Text::new("Buffs: none"),
                    TextFont::from_font_size(16.0),
                    TextColor(Color::srgb(0.7, 0.95, 0.75)),
                ));
                panel.spawn((
                    HudNet,
                    Text::new("Net: offline"),
                    TextFont::from_font_size(14.0),
                    TextColor(Color::srgb(0.7, 0.85, 1.0)),
                ));
                panel.spawn((
                    HudAttributes,
                    Text::new("STR --  AGI --  INT --"),
                    TextFont::from_font_size(16.0),
                    TextColor(Color::srgb(0.95, 0.8, 0.55)),
                ));
                panel.spawn((
                    HudCombatStats,
                    Text::new("AD --  AS --  Rng --  Arm --  MR --"),
                    TextFont::from_font_size(14.0),
                    TextColor(Color::srgb(0.75, 0.85, 0.95)),
                ));
            });

            // Inventory bar (above spell bar)
            root.spawn(Node {
                position_type: PositionType::Absolute,
                bottom: px(100),
                left: percent(50),
                margin: UiRect::left(px(-168)),
                flex_direction: FlexDirection::Row,
                column_gap: px(6),
                padding: UiRect::all(px(6)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            })
            .with_children(|bar| {
                for i in 0..6 {
                    spawn_inventory_slot(bar, i);
                }
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
                for i in 0..4 {
                    spawn_spell_icon(bar, i, AbilityId::Dash);
                }
            });

            // Help
            root.spawn((
                Text::new(
                    "1-3 hero select  |  RMB: move/attack  |  G: attack-move  |  ASDZXC: items  |  $ shop  |  Space: stop",
                ),
                TextFont::from_font_size(14.0),
                TextColor(Color::srgba(0.8, 0.85, 0.9, 0.8)),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(155),
                    left: px(16),
                    ..default()
                },
            ));

            // Minimap (above shop button)
            root.spawn((
                MinimapRoot,
                Node {
                    position_type: PositionType::Absolute,
                    right: px(14),
                    bottom: px(14.0 + SHOP_BUTTON_SIZE + 8.0),
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

            // Shop toggle — bottom-right corner of the window
            root.spawn((
                Button,
                ShopToggleButton,
                Node {
                    position_type: PositionType::Absolute,
                    right: px(14),
                    bottom: px(14),
                    width: px(SHOP_BUTTON_SIZE),
                    height: px(SHOP_BUTTON_SIZE),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    border: UiRect::all(px(2)),
                    border_radius: BorderRadius::all(px(8)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.9, 0.7, 0.2)),
                BorderColor::all(Color::srgb(0.35, 0.25, 0.05)),
                ZIndex(55),
            ))
            .with_children(|btn| {
                btn.spawn((
                    Text::new("$"),
                    TextFont::from_font_size(24.0),
                    TextColor(Color::srgb(0.1, 0.08, 0.02)),
                ));
            });

            // Full-screen backdrop closes the shop when clicked
            root.spawn((
                Button,
                ShopBackdrop,
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    height: percent(100),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
                Visibility::Hidden,
                ZIndex(40),
            ));

            // Shop panel (starts hidden)
            root.spawn((
                ShopPanel,
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(50),
                    top: percent(50),
                    margin: UiRect::new(px(-200), px(0), px(-210), px(0)),
                    width: px(400),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(10),
                    padding: UiRect::all(px(14)),
                    border: UiRect::all(px(2)),
                    border_radius: BorderRadius::all(px(10)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.06, 0.08, 0.12, 0.96)),
                BorderColor::all(Color::srgb(0.85, 0.7, 0.25)),
                Visibility::Hidden,
                ZIndex(50),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Item Shop"),
                    TextFont::from_font_size(24.0),
                    TextColor(Color::srgb(1.0, 0.9, 0.45)),
                ));
                panel.spawn((
                    ShopStatusText,
                    Text::new("Stand near your base shop to buy. Hover for details."),
                    TextFont::from_font_size(13.0),
                    TextColor(Color::srgb(0.75, 0.85, 0.9)),
                ));
                panel
                    .spawn(Node {
                        width: percent(100),
                        flex_direction: FlexDirection::Row,
                        flex_wrap: FlexWrap::Wrap,
                        column_gap: px(10),
                        row_gap: px(10),
                        justify_content: JustifyContent::Center,
                        ..default()
                    })
                    .with_children(|grid| {
                        for &item in ItemId::all() {
                            spawn_shop_item(grid, item);
                        }
                    });
                panel.spawn((
                    Text::new("Esc / click outside / $ to close"),
                    TextFont::from_font_size(12.0),
                    TextColor(Color::srgba(0.7, 0.75, 0.8, 0.8)),
                ));
            });

            // Shared item tooltip (shop + inventory)
            root.spawn((
                ItemTooltip,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(24),
                    bottom: px(220),
                    width: px(260),
                    padding: UiRect::all(px(10)),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(6)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.05, 0.07, 0.1, 0.95)),
                BorderColor::all(Color::srgb(0.85, 0.75, 0.4)),
                Visibility::Hidden,
                ZIndex(60),
            ))
            .with_children(|tip| {
                tip.spawn((
                    ItemTooltipText,
                    Text::new(""),
                    TextFont::from_font_size(14.0),
                    TextColor(Color::srgb(0.95, 0.95, 1.0)),
                ));
            });

            // Inventory sell dropdown (opened via RMB on a slot)
            root.spawn((
                InventorySellMenu,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(40),
                    bottom: px(175),
                    width: px(140),
                    padding: UiRect::all(px(6)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(4),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(6)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.08, 0.09, 0.12, 0.96)),
                BorderColor::all(Color::srgb(0.7, 0.75, 0.85)),
                Visibility::Hidden,
                ZIndex(70),
            ))
            .with_children(|menu| {
                menu.spawn((
                    Button,
                    InventorySellButton,
                    Node {
                        width: percent(100),
                        height: px(28),
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        border_radius: BorderRadius::all(px(4)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.35, 0.25, 0.2)),
                ))
                .with_children(|btn| {
                    btn.spawn((
                        Text::new("Sell (50%)"),
                        TextFont::from_font_size(14.0),
                        TextColor(Color::WHITE),
                    ));
                });
            });
        });
}

fn spawn_inventory_slot(parent: &mut ChildSpawnerCommands, index: usize) {
    let hotkey = ItemId::inventory_hotkey(index).unwrap_or("?");
    parent
        .spawn((
            Button,
            InventorySlotIcon { index },
            Node {
                width: px(72),
                height: px(70),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                padding: UiRect::all(px(2)),
                border: UiRect::all(px(2)),
                border_radius: BorderRadius::all(px(6)),
                row_gap: px(2),
                ..default()
            },
            BackgroundColor(Color::srgba(0.12, 0.14, 0.18, 0.85)),
            BorderColor::all(Color::srgb(0.3, 0.35, 0.4)),
        ))
        .with_children(|slot| {
            slot.spawn((
                InventorySlotLabel { index },
                Text::new(format!("{hotkey}")),
                TextFont::from_font_size(11.0),
                TextColor(Color::srgb(0.9, 0.92, 0.95)),
            ));
            slot.spawn((
                InventorySlotCd { index },
                Text::new(""),
                TextFont::from_font_size(10.0),
                TextColor(Color::srgb(1.0, 0.85, 0.4)),
            ));
        });
}

fn spawn_shop_item(parent: &mut ChildSpawnerCommands, item: ItemId) {
    parent
        .spawn((
            Button,
            ShopBuyButton { item },
            Node {
                width: px(86),
                height: px(92),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: px(4),
                padding: UiRect::all(px(4)),
                border: UiRect::all(px(2)),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(item.placeholder_color()),
            BorderColor::all(Color::srgb(0.2, 0.2, 0.25)),
        ))
        .with_children(|btn| {
            btn.spawn((
                Text::new(item.short_label()),
                TextFont::from_font_size(20.0),
                TextColor(Color::WHITE),
            ));
            btn.spawn((
                Text::new(item.name()),
                TextFont::from_font_size(11.0),
                TextColor(Color::srgb(0.95, 0.95, 1.0)),
            ));
        });
}

fn spawn_spell_icon(parent: &mut ChildSpawnerCommands, index: usize, id: AbilityId) {
    let hotkey = ["Q", "W", "E", "R"].get(index).copied().unwrap_or("?");
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
                SpellHotkeyLabel,
                Text::new(hotkey),
                TextFont::from_font_size(18.0),
                TextColor(Color::srgb(0.85, 0.9, 1.0)),
            ));
            icon.spawn((
                SpellNameLabel { index },
                Text::new(id.display_name()),
                TextFont::from_font_size(12.0),
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

fn refresh_hero_name(
    hero: Query<&HeroKind, With<PlayerHero>>,
    mut text_q: Query<&mut Text, With<HudHeroName>>,
) {
    let Ok(kind) = hero.single() else {
        return;
    };
    let Ok(mut text) = text_q.single_mut() else {
        return;
    };
    *text = Text::new(format!(
        "Hero: {} ({})",
        kind.0.name(),
        kind.0.primary_label()
    ));
}

fn refresh_hud_text(
    hero: Query<
        (
            &Health,
            &Mana,
            &PlayerWallet,
            &HeroProgress,
            &HeroAttributes,
            &CombatStats,
        ),
        With<PlayerHero>,
    >,
    mut texts: ParamSet<(
        Query<&mut Text, With<HudVitals>>,
        Query<&mut Text, With<HudLevel>>,
        Query<&mut Text, With<HudGold>>,
        Query<&mut Text, With<HudSkillPoints>>,
        Query<&mut Text, With<HudAttributes>>,
        Query<&mut Text, With<HudCombatStats>>,
    )>,
) {
    let Ok((health, mana, wallet, progress, attrs, stats)) = hero.single() else {
        return;
    };

    if let Ok(mut text) = texts.p0().single_mut() {
        *text = Text::new(format!(
            "HP {hp:.0} / {hp_max:.0} (+{hpr:.1}/s)   MP {mp:.0} / {mp_max:.0}",
            hp = health.current.max(0.0),
            hp_max = health.max,
            hpr = health.regen_per_sec,
            mp = mana.current,
            mp_max = mana.max,
        ));
    }
    if let Ok(mut text) = texts.p1().single_mut() {
        if progress.level >= 25 {
            *text = Text::new(format!("Level {}   MAX", progress.level));
        } else {
            *text = Text::new(format!(
                "Level {}   XP {} / {}",
                progress.level, progress.xp, progress.xp_to_next
            ));
        }
    }
    if let Ok(mut text) = texts.p2().single_mut() {
        *text = Text::new(format!("Gold: {}", wallet.gold));
    }
    if let Ok(mut text) = texts.p3().single_mut() {
        let label = if progress.skill_points > 0 {
            format!("Skill Points: {}  (Ctrl+QWER or +)", progress.skill_points)
        } else {
            "Skill Points: 0".into()
        };
        *text = Text::new(label);
    }
    if let Ok(mut text) = texts.p4().single_mut() {
        *text = Text::new(format!(
            "STR {s:.0}  AGI {a:.0}  INT {i:.0}",
            s = attrs.strength,
            a = attrs.agility,
            i = attrs.intelligence,
        ));
    }
    if let Ok(mut text) = texts.p5().single_mut() {
        *text = Text::new(format!(
            "AD {ad:.0}  IAS {ias:.0}  APS {aspeed:.2}  Rng {rng:.0}  Arm {arm:.1}  MR {mr:.1}",
            ad = stats.attack_damage,
            ias = stats.attack_speed_rating(attrs.agility),
            aspeed = stats.attack_speed,
            rng = stats.attack_range,
            arm = stats.armor,
            mr = stats.magic_resist,
        ));
    }
}

fn refresh_buffs_text(
    hero: Query<&StatusEffects, With<PlayerHero>>,
    mut buffs: Query<&mut Text, With<HudBuffs>>,
) {
    let Ok(statuses) = hero.single() else {
        return;
    };
    let Ok(mut text) = buffs.single_mut() else {
        return;
    };
    if statuses.effects.is_empty() {
        *text = Text::new("Buffs: none");
        return;
    }
    let parts: Vec<String> = statuses
        .effects
        .iter()
        .map(|e| {
            let tag = match e.kind {
                StatusKind::Buff => "+",
                StatusKind::Debuff => "-",
            };
            format!("{tag}{} {:.0}s", e.id, e.remaining.ceil())
        })
        .collect();
    *text = Text::new(format!("Buffs: {}", parts.join(", ")));
}

fn refresh_spell_bar(
    hero: Query<(&AbilityLoadout, &HeroProgress), With<PlayerHero>>,
    mut ranks: Query<
        (&SpellRankText, &mut Text),
        (Without<SpellCdText>, Without<SpellNameLabel>),
    >,
    mut cds: Query<
        (&SpellCdText, &mut Text),
        (Without<SpellRankText>, Without<SpellNameLabel>),
    >,
    mut names: Query<
        (&SpellNameLabel, &mut Text),
        (Without<SpellRankText>, Without<SpellCdText>),
    >,
    mut buttons: Query<(&SpellLevelButton, &mut Visibility)>,
    mut icons: Query<(&SpellIcon, &mut BorderColor, &mut BackgroundColor)>,
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

    for (meta, mut text) in &mut names {
        if let Some(slot) = loadout.slots.get(meta.index) {
            *text = Text::new(slot.id.display_name());
        }
    }

    for (meta, mut vis) in &mut buttons {
        let show = loadout.slots.get(meta.index).is_some_and(|slot| {
            progress.skill_points > 0 && slot.id.can_rank_up(slot.rank, progress.level)
        });
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    for (meta, mut border, mut bg) in &mut icons {
        let slot = loadout.slots.get(meta.index);
        if let Some(slot) = slot {
            *bg = BackgroundColor(slot.id.placeholder_color());
        }
        let highlight = slot.is_some_and(|slot| {
            progress.skill_points > 0 && slot.id.can_rank_up(slot.rank, progress.level)
        });
        *border = BorderColor::all(if highlight {
            Color::srgb(1.0, 0.9, 0.3)
        } else {
            Color::srgb(0.15, 0.15, 0.18)
        });
    }
}

fn handle_inventory_context_menu(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    slots: Query<(&Interaction, &InventorySlotIcon)>,
    inv: Query<&Inventory, With<PlayerHero>>,
    mut menu: ResMut<InventoryContextMenu>,
    mut menu_vis: Query<&mut Visibility, With<InventorySellMenu>>,
) {
    if mouse.just_pressed(MouseButton::Right) {
        let Ok(inventory) = inv.single() else {
            return;
        };
        for (interaction, slot) in &slots {
            if *interaction == Interaction::None {
                continue;
            }
            if inventory.slots.get(slot.index).and_then(|s| s.as_ref()).is_some() {
                menu.slot = Some(slot.index);
                if let Ok(mut vis) = menu_vis.single_mut() {
                    *vis = Visibility::Visible;
                }
                return;
            }
        }
        menu.slot = None;
        if let Ok(mut vis) = menu_vis.single_mut() {
            *vis = Visibility::Hidden;
        }
    }
    if keys.just_pressed(KeyCode::Escape) {
        menu.slot = None;
        if let Ok(mut vis) = menu_vis.single_mut() {
            *vis = Visibility::Hidden;
        }
    }
}

fn handle_inventory_sell_clicks(
    interactions: Query<&Interaction, (Changed<Interaction>, With<InventorySellButton>)>,
    menu: Res<InventoryContextMenu>,
    mut sell: MessageWriter<SellItemRequest>,
    mut menu_state: ResMut<InventoryContextMenu>,
    mut menu_vis: Query<&mut Visibility, With<InventorySellMenu>>,
) {
    for interaction in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let Some(slot) = menu.slot {
            sell.write(SellItemRequest { slot });
        }
        menu_state.slot = None;
        if let Ok(mut vis) = menu_vis.single_mut() {
            *vis = Visibility::Hidden;
        }
    }
}

fn refresh_inventory_bar(
    hero: Query<&Inventory, With<PlayerHero>>,
    mut icons: Query<(&InventorySlotIcon, &mut BackgroundColor, &mut BorderColor)>,
    mut labels: Query<(&InventorySlotLabel, &mut Text), Without<InventorySlotCd>>,
    mut cds: Query<(&InventorySlotCd, &mut Text), Without<InventorySlotLabel>>,
) {
    let Ok(inv) = hero.single() else {
        return;
    };

    for (meta, mut bg, mut border) in &mut icons {
        if let Some(item) = inv.slots.get(meta.index).and_then(|s| s.as_ref()) {
            *bg = BackgroundColor(item.id.placeholder_color());
            *border = BorderColor::all(if item.id.has_active() {
                Color::srgb(0.95, 0.85, 0.3)
            } else {
                Color::srgb(0.25, 0.3, 0.35)
            });
        } else {
            *bg = BackgroundColor(Color::srgba(0.12, 0.14, 0.18, 0.85));
            *border = BorderColor::all(Color::srgb(0.3, 0.35, 0.4));
        }
    }

    for (meta, mut text) in &mut labels {
        let hotkey = ItemId::inventory_hotkey(meta.index).unwrap_or("?");
        if let Some(item) = inv.slots.get(meta.index).and_then(|s| s.as_ref()) {
            *text = Text::new(format!("{}\n{}", item.id.short_label(), item.id.name()));
        } else {
            *text = Text::new(format!("{hotkey}"));
        }
    }

    for (meta, mut text) in &mut cds {
        if let Some(item) = inv.slots.get(meta.index).and_then(|s| s.as_ref()) {
            if item.cooldown_remaining > 0.05 {
                *text = Text::new(format!("{:.0}s", item.cooldown_remaining.ceil()));
            } else if item.id.has_active() {
                *text = Text::new("rdy");
            } else {
                *text = Text::new("");
            }
        } else {
            *text = Text::new("");
        }
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

fn handle_shop_toggle_button(
    interactions: Query<&Interaction, (Changed<Interaction>, With<ShopToggleButton>)>,
    mut shop_ui: ResMut<ShopUiState>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            shop_ui.open = !shop_ui.open;
        }
    }
}

fn sync_shop_panel_visibility(
    shop_ui: Res<ShopUiState>,
    mut panel: Query<&mut Visibility, (With<ShopPanel>, Without<ShopBackdrop>)>,
    mut backdrop: Query<&mut Visibility, (With<ShopBackdrop>, Without<ShopPanel>)>,
) {
    let vis = if shop_ui.open {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if let Ok(mut v) = panel.single_mut() {
        *v = vis;
    }
    if let Ok(mut v) = backdrop.single_mut() {
        *v = vis;
    }
}

fn handle_shop_backdrop_close(
    interactions: Query<&Interaction, (Changed<Interaction>, With<ShopBackdrop>)>,
    mut shop_ui: ResMut<ShopUiState>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            shop_ui.open = false;
        }
    }
}

fn handle_shop_close_keys(keys: Res<ButtonInput<KeyCode>>, mut shop_ui: ResMut<ShopUiState>) {
    if shop_ui.open && keys.just_pressed(KeyCode::Escape) {
        shop_ui.open = false;
    }
}

fn handle_shop_buy_clicks(
    interactions: Query<(&Interaction, &ShopBuyButton), Changed<Interaction>>,
    shop_ui: Res<ShopUiState>,
    mut writer: MessageWriter<PurchaseItemRequest>,
) {
    if !shop_ui.open {
        return;
    }
    for (interaction, button) in &interactions {
        if *interaction == Interaction::Pressed {
            writer.write(PurchaseItemRequest { item: button.item });
        }
    }
}

fn update_item_tooltips(
    shop_ui: Res<ShopUiState>,
    shop_items: Query<(&Interaction, &ShopBuyButton)>,
    inv_slots: Query<(&Interaction, &InventorySlotIcon)>,
    inv: Query<&Inventory, With<PlayerHero>>,
    mut tip: Query<&mut Visibility, With<ItemTooltip>>,
    mut tip_text: Query<&mut Text, With<ItemTooltipText>>,
) {
    let Ok(mut tip_vis) = tip.single_mut() else {
        return;
    };
    let Ok(mut text) = tip_text.single_mut() else {
        return;
    };

    let mut body: Option<String> = None;

    if shop_ui.open {
        for (interaction, button) in &shop_items {
            if matches!(*interaction, Interaction::Hovered | Interaction::Pressed) {
                body = Some(button.item.tooltip_body());
                break;
            }
        }
    }

    if body.is_none() {
        let Ok(inventory) = inv.single() else {
            *tip_vis = Visibility::Hidden;
            return;
        };
        for (interaction, slot) in &inv_slots {
            if matches!(*interaction, Interaction::Hovered | Interaction::Pressed) {
                if let Some(item) = inventory.slots.get(slot.index).and_then(|s| s.as_ref()) {
                    let hotkey = ItemId::inventory_hotkey(slot.index).unwrap_or("?");
                    body = Some(format!(
                        "{}\nHotkey: {}\n{}",
                        item.id.tooltip_body(),
                        hotkey,
                        if item.cooldown_remaining > 0.05 {
                            format!("Cooldown: {:.0}s", item.cooldown_remaining.ceil())
                        } else if item.id.has_active() {
                            "Ready".into()
                        } else {
                            "Passive".into()
                        }
                    ));
                } else {
                    body = Some(format!(
                        "Empty slot\nHotkey: {}",
                        ItemId::inventory_hotkey(slot.index).unwrap_or("?")
                    ));
                }
                break;
            }
        }
    }

    if let Some(body) = body {
        *text = Text::new(body);
        *tip_vis = Visibility::Visible;
    } else {
        *tip_vis = Visibility::Hidden;
    }
}

fn refresh_shop_status(
    shop_ui: Res<ShopUiState>,
    hero: Query<(&Transform, &Team, &PlayerWallet, &Inventory), With<PlayerHero>>,
    shops: Query<(&Transform, &ItemShop)>,
    mut status: Query<&mut Text, With<ShopStatusText>>,
) {
    if !shop_ui.open {
        return;
    }
    let Ok((hero_tf, hero_team, wallet, inv)) = hero.single() else {
        return;
    };
    let Ok(mut text) = status.single_mut() else {
        return;
    };
    let near = shops.iter().any(|(shop_tf, shop)| {
        shop.team == *hero_team
            && crate::combat::flat_distance(hero_tf.translation, shop_tf.translation)
                <= shop.purchase_range
    });
    let empty = inv.first_empty().map(|i| i + 1);
    let range_msg = if near {
        "In shop range"
    } else {
        "Out of range — walk to the gold shop near base"
    };
    let slots_msg = match empty {
        Some(n) => format!("free slot {}", n),
        None => "inventory full".into(),
    };
    *text = Text::new(format!(
        "{range_msg}  |  Gold: {}  |  {}  |  Hover item for details",
        wallet.gold, slots_msg
    ));
}


fn refresh_net_status(
    status: Res<NetStatus>,
    mut text: Query<&mut Text, With<HudNet>>,
) {
    let Ok(mut label) = text.single_mut() else {
        return;
    };
    let peers = if status.connected_peers > 0 {
        format!(" | peers {}", status.connected_peers)
    } else {
        String::new()
    };
    *label = Text::new(format!("Net: {}{}", status.detail, peers));
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
    let x = nx * (MINIMAP_SIZE - 8.0) + 1.0;
    let y = nz * (MINIMAP_SIZE - 8.0) + 1.0;
    (x, y)
}
