//! Item catalog, shop, inventory, active use (ASDZXC), and status effects.

use bevy::prelude::*;

use crate::combat::{apply_damage, flat_distance};
use crate::components::{
    CombatStats, DamageType, Health, Lifetime, Mana, PlayerHero, PlayerWallet, SpellFx, Team,
};
use crate::resources::SharedAssets;

pub struct ItemsPlugin;

impl Plugin for ItemsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShopUiState>()
            .add_message::<PurchaseItemRequest>()
            .add_systems(
                Startup,
                spawn_shops.after(crate::resources::load_shared_assets),
            )
            .add_systems(
                Update,
                (
                    tick_item_cooldowns,
                    tick_status_effects,
                    handle_item_hotkeys,
                    try_purchase_from_ui,
                ),
            );
    }
}

/// Whether the shop panel is open (toggled from the HUD button).
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct ShopUiState {
    pub open: bool,
}

/// World shop marker near a team base.
#[derive(Component, Debug, Clone, Copy)]
pub struct ItemShop {
    pub team: Team,
    pub purchase_range: f32,
}

/// Six-slot hero inventory. Hotkeys A S D Z X C map to indices 0..=5.
#[derive(Component, Debug, Clone)]
pub struct Inventory {
    pub slots: [Option<ItemInstance>; 6],
}

impl Inventory {
    pub fn empty() -> Self {
        Self {
            slots: [None, None, None, None, None, None],
        }
    }

    pub fn first_empty(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }

    pub fn try_add(&mut self, id: ItemId) -> bool {
        let Some(i) = self.first_empty() else {
            return false;
        };
        self.slots[i] = Some(ItemInstance::new(id));
        true
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemInstance {
    pub id: ItemId,
    pub cooldown_remaining: f32,
}

impl ItemInstance {
    pub fn new(id: ItemId) -> Self {
        Self {
            id,
            cooldown_remaining: 0.0,
        }
    }
}

/// Generic buff / debuff container on heroes (and potentially other units later).
#[derive(Component, Debug, Clone, Default)]
pub struct StatusEffects {
    pub effects: Vec<StatusEffect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum StatusKind {
    Buff,
    Debuff,
}

#[derive(Debug, Clone)]
pub struct StatusEffect {
    pub id: &'static str,
    pub kind: StatusKind,
    pub remaining: f32,
    /// Flat combat modifiers applied for the duration (removed on expire).
    pub attack_damage: f32,
    pub armor: f32,
    pub magic_resist: f32,
    pub move_speed: f32,
    pub heal_per_sec: f32,
}

impl StatusEffect {
    pub fn buff(id: &'static str, duration: f32) -> Self {
        Self {
            id,
            kind: StatusKind::Buff,
            remaining: duration,
            attack_damage: 0.0,
            armor: 0.0,
            magic_resist: 0.0,
            move_speed: 0.0,
            heal_per_sec: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemId {
    IronBracer,
    SwiftBoots,
    ManaCrystal,
    BladeOfAsh,
    AegisCharm,
    VialOfLight,
    StormRod,
    WardstoneCloak,
}

impl ItemId {
    pub fn all() -> &'static [ItemId] {
        &[
            ItemId::IronBracer,
            ItemId::SwiftBoots,
            ItemId::ManaCrystal,
            ItemId::BladeOfAsh,
            ItemId::AegisCharm,
            ItemId::VialOfLight,
            ItemId::StormRod,
            ItemId::WardstoneCloak,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            ItemId::IronBracer => "Iron Bracer",
            ItemId::SwiftBoots => "Swift Boots",
            ItemId::ManaCrystal => "Mana Crystal",
            ItemId::BladeOfAsh => "Blade of Ash",
            ItemId::AegisCharm => "Aegis Charm",
            ItemId::VialOfLight => "Vial of Light",
            ItemId::StormRod => "Storm Rod",
            ItemId::WardstoneCloak => "Wardstone Cloak",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            ItemId::IronBracer => "IB",
            ItemId::SwiftBoots => "SB",
            ItemId::ManaCrystal => "MC",
            ItemId::BladeOfAsh => "BA",
            ItemId::AegisCharm => "AC",
            ItemId::VialOfLight => "VL",
            ItemId::StormRod => "SR",
            ItemId::WardstoneCloak => "WC",
        }
    }

    pub fn cost(self) -> u32 {
        match self {
            ItemId::IronBracer => 300,
            ItemId::SwiftBoots => 450,
            ItemId::ManaCrystal => 350,
            ItemId::BladeOfAsh => 800,
            ItemId::AegisCharm => 700,
            ItemId::VialOfLight => 400,
            ItemId::StormRod => 900,
            ItemId::WardstoneCloak => 650,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ItemId::IronBracer => "+100 HP, +4 Armor",
            ItemId::SwiftBoots => "+2.5 Move Speed",
            ItemId::ManaCrystal => "+80 Mana, +4 Mana Regen",
            ItemId::BladeOfAsh => "+18 Attack Damage",
            ItemId::AegisCharm => "+6 Armor, +5 Magic Resist",
            ItemId::VialOfLight => "Active: Heal 180 HP",
            ItemId::StormRod => "+12 AD. Active: 140 magic AoE",
            ItemId::WardstoneCloak => "+80 HP, +4 MR. Active: +8 Armor 5s",
        }
    }

    pub fn tooltip_body(self) -> String {
        let kind = if self.has_active() {
            "Active item"
        } else {
            "Passive item"
        };
        format!(
            "{name}\nCost: {cost}g\n{kind}\n{desc}",
            name = self.name(),
            cost = self.cost(),
            desc = self.description(),
        )
    }

    pub fn placeholder_color(self) -> Color {
        match self {
            ItemId::IronBracer => Color::srgb(0.55, 0.55, 0.6),
            ItemId::SwiftBoots => Color::srgb(0.35, 0.7, 0.45),
            ItemId::ManaCrystal => Color::srgb(0.3, 0.45, 0.95),
            ItemId::BladeOfAsh => Color::srgb(0.85, 0.35, 0.25),
            ItemId::AegisCharm => Color::srgb(0.7, 0.65, 0.35),
            ItemId::VialOfLight => Color::srgb(0.95, 0.9, 0.55),
            ItemId::StormRod => Color::srgb(0.45, 0.35, 0.95),
            ItemId::WardstoneCloak => Color::srgb(0.4, 0.55, 0.5),
        }
    }

    pub fn active_cooldown(self) -> Option<f32> {
        match self {
            ItemId::VialOfLight => Some(40.0),
            ItemId::StormRod => Some(30.0),
            ItemId::WardstoneCloak => Some(35.0),
            _ => None,
        }
    }

    pub fn has_active(self) -> bool {
        self.active_cooldown().is_some()
    }

    pub fn inventory_hotkey(index: usize) -> Option<&'static str> {
        match index {
            0 => Some("A"),
            1 => Some("S"),
            2 => Some("D"),
            3 => Some("Z"),
            4 => Some("X"),
            5 => Some("C"),
            _ => None,
        }
    }
}

/// Flat passive bonuses applied once on purchase.
#[derive(Debug, Clone, Copy, Default)]
pub struct ItemPassives {
    pub max_health: f32,
    pub max_mana: f32,
    pub mana_regen: f32,
    pub attack_damage: f32,
    pub armor: f32,
    pub magic_resist: f32,
    pub move_speed: f32,
}

impl ItemId {
    pub fn passives(self) -> ItemPassives {
        match self {
            ItemId::IronBracer => ItemPassives {
                max_health: 100.0,
                armor: 4.0,
                ..default()
            },
            ItemId::SwiftBoots => ItemPassives {
                move_speed: 2.5,
                ..default()
            },
            ItemId::ManaCrystal => ItemPassives {
                max_mana: 80.0,
                mana_regen: 4.0,
                ..default()
            },
            ItemId::BladeOfAsh => ItemPassives {
                attack_damage: 18.0,
                ..default()
            },
            ItemId::AegisCharm => ItemPassives {
                armor: 6.0,
                magic_resist: 5.0,
                ..default()
            },
            ItemId::VialOfLight => ItemPassives::default(),
            ItemId::StormRod => ItemPassives {
                attack_damage: 12.0,
                ..default()
            },
            ItemId::WardstoneCloak => ItemPassives {
                max_health: 80.0,
                magic_resist: 4.0,
                ..default()
            },
        }
    }
}

pub fn apply_passives(passives: ItemPassives, health: &mut Health, mana: &mut Mana, stats: &mut CombatStats) {
    if passives.max_health != 0.0 {
        health.max += passives.max_health;
        health.current = (health.current + passives.max_health).min(health.max);
    }
    if passives.max_mana != 0.0 {
        mana.max += passives.max_mana;
        mana.current = (mana.current + passives.max_mana).min(mana.max);
    }
    mana.regen_per_sec += passives.mana_regen;
    stats.attack_damage += passives.attack_damage;
    stats.armor += passives.armor;
    stats.magic_resist += passives.magic_resist;
    stats.move_speed += passives.move_speed;
}

fn spawn_shops(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(Cuboid::new(2.4, 2.0, 2.4));
    let radiant_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.75, 0.25),
        emissive: LinearRgba::rgb(2.0, 1.2, 0.2),
        perceptual_roughness: 0.45,
        metallic: 0.35,
        ..default()
    });
    let dire_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.35, 0.55),
        emissive: LinearRgba::rgb(1.5, 0.3, 0.8),
        perceptual_roughness: 0.45,
        metallic: 0.35,
        ..default()
    });

    // Near Radiant ancient / hero spawn.
    commands.spawn((
        Name::new("Radiant Item Shop"),
        Mesh3d(mesh.clone()),
        MeshMaterial3d(radiant_mat),
        Transform::from_xyz(-52.0, 1.0, -40.0),
        ItemShop {
            team: Team::Radiant,
            purchase_range: 14.0,
        },
    ));
    // Near Dire base (for future Dire heroes / symmetry).
    commands.spawn((
        Name::new("Dire Item Shop"),
        Mesh3d(mesh),
        MeshMaterial3d(dire_mat),
        Transform::from_xyz(52.0, 1.0, 40.0),
        ItemShop {
            team: Team::Dire,
            purchase_range: 14.0,
        },
    ));
}

fn tick_item_cooldowns(time: Res<Time>, mut inventories: Query<&mut Inventory>) {
    let dt = time.delta_secs();
    for mut inv in &mut inventories {
        for slot in &mut inv.slots {
            if let Some(item) = slot.as_mut() {
                item.cooldown_remaining = (item.cooldown_remaining - dt).max(0.0);
            }
        }
    }
}

fn tick_status_effects(
    time: Res<Time>,
    mut units: Query<(&mut StatusEffects, &mut CombatStats, &mut Health)>,
) {
    let dt = time.delta_secs();
    for (mut statuses, mut stats, mut health) in &mut units {
        let mut i = 0;
        while i < statuses.effects.len() {
            let effect = &mut statuses.effects[i];
            if effect.heal_per_sec > 0.0 {
                health.current = (health.current + effect.heal_per_sec * dt).min(health.max);
            }
            effect.remaining -= dt;
            if effect.remaining <= 0.0 {
                let expired = statuses.effects.remove(i);
                stats.attack_damage -= expired.attack_damage;
                stats.armor -= expired.armor;
                stats.magic_resist -= expired.magic_resist;
                stats.move_speed -= expired.move_speed;
            } else {
                i += 1;
            }
        }
    }
}

/// Apply a timed buff and immediately add its flat modifiers to combat stats.
pub fn apply_status(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    effect: StatusEffect,
) {
    // Refresh same-id buffs by replacing.
    if let Some(existing) = statuses.effects.iter().position(|e| e.id == effect.id) {
        let old = statuses.effects.remove(existing);
        stats.attack_damage -= old.attack_damage;
        stats.armor -= old.armor;
        stats.magic_resist -= old.magic_resist;
        stats.move_speed -= old.move_speed;
    }
    stats.attack_damage += effect.attack_damage;
    stats.armor += effect.armor;
    stats.magic_resist += effect.magic_resist;
    stats.move_speed += effect.move_speed;
    statuses.effects.push(effect);
}

fn handle_item_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut hero: Query<
        (
            Entity,
            &Transform,
            &Team,
            &mut Inventory,
            &mut Health,
            &mut CombatStats,
            &mut StatusEffects,
        ),
        With<PlayerHero>,
    >,
    enemies: Query<(Entity, &Transform, &Team, &Health, &CombatStats), Without<PlayerHero>>,
) {
    let picks = [
        (KeyCode::KeyA, 0usize),
        (KeyCode::KeyS, 1),
        (KeyCode::KeyD, 2),
        (KeyCode::KeyZ, 3),
        (KeyCode::KeyX, 4),
        (KeyCode::KeyC, 5),
    ];
    let pressed: Vec<usize> = picks
        .into_iter()
        .filter(|(key, _)| keys.just_pressed(*key))
        .map(|(_, i)| i)
        .collect();
    if pressed.is_empty() {
        return;
    }

    let Ok((hero_entity, transform, team, mut inv, mut health, mut stats, mut statuses)) =
        hero.single_mut()
    else {
        return;
    };

    for index in pressed {
        try_use_item(
            &mut commands,
            &assets,
            hero_entity,
            transform,
            *team,
            &mut inv,
            &mut health,
            &mut stats,
            &mut statuses,
            index,
            &enemies,
        );
    }
}

fn try_use_item(
    commands: &mut Commands,
    assets: &SharedAssets,
    hero_entity: Entity,
    transform: &Transform,
    team: Team,
    inv: &mut Inventory,
    health: &mut Health,
    stats: &mut CombatStats,
    statuses: &mut StatusEffects,
    index: usize,
    enemies: &Query<(Entity, &Transform, &Team, &Health, &CombatStats), Without<PlayerHero>>,
) {
    let Some(item) = inv.slots.get_mut(index).and_then(|s| s.as_mut()) else {
        return;
    };
    let Some(cd) = item.id.active_cooldown() else {
        return;
    };
    if item.cooldown_remaining > 0.0 {
        return;
    }

    match item.id {
        ItemId::VialOfLight => {
            health.current = (health.current + 180.0).min(health.max);
            spawn_heal_fx(commands, assets, transform.translation);
        }
        ItemId::StormRod => {
            let origin = transform.translation;
            let radius = 5.5;
            for (_e, enemy_tf, enemy_team, enemy_hp, enemy_stats) in enemies.iter() {
                if *enemy_team != team.enemy() || !enemy_hp.is_alive() {
                    continue;
                }
                if flat_distance(origin, enemy_tf.translation) <= radius {
                    let dmg = apply_damage(140.0, DamageType::Magical, enemy_stats);
                    commands.entity(_e).insert(crate::abilities::PendingDamage { amount: dmg });
                }
            }
            commands.spawn((
                Name::new("Storm Rod Pulse"),
                Mesh3d(assets.indicator_ring_mesh.clone()),
                MeshMaterial3d(assets.shockwave_mat.clone()),
                Transform::from_translation(origin + Vec3::Y * 0.15)
                    .with_scale(Vec3::new(radius, 1.0, radius)),
                SpellFx {
                    age: 0.0,
                    lifetime: 0.45,
                    start_scale: radius * 0.4,
                    end_scale: radius,
                },
                Lifetime(0.45),
            ));
        }
        ItemId::WardstoneCloak => {
            apply_status(
                statuses,
                stats,
                StatusEffect {
                    armor: 8.0,
                    ..StatusEffect::buff("wardstone_guard", 5.0)
                },
            );
            spawn_heal_fx(commands, assets, transform.translation);
        }
        _ => return,
    }

    item.cooldown_remaining = cd;
    let _ = hero_entity;
}

fn spawn_heal_fx(commands: &mut Commands, assets: &SharedAssets, at: Vec3) {
    commands.spawn((
        Name::new("Item Heal FX"),
        Mesh3d(assets.indicator_ring_mesh.clone()),
        MeshMaterial3d(assets.nova_mat.clone()),
        Transform::from_translation(at + Vec3::Y * 0.2).with_scale(Vec3::splat(1.5)),
        SpellFx {
            age: 0.0,
            lifetime: 0.5,
            start_scale: 1.0,
            end_scale: 3.0,
        },
        Lifetime(0.5),
    ));
}

#[derive(Message, Debug, Clone, Copy)]
pub struct PurchaseItemRequest {
    pub item: ItemId,
}

fn try_purchase_from_ui(
    mut events: MessageReader<PurchaseItemRequest>,
    mut hero: Query<
        (
            &Transform,
            &Team,
            &mut PlayerWallet,
            &mut Inventory,
            &mut Health,
            &mut Mana,
            &mut CombatStats,
        ),
        With<PlayerHero>,
    >,
    shops: Query<(&Transform, &ItemShop)>,
) {
    let Ok((hero_tf, hero_team, mut wallet, mut inv, mut health, mut mana, mut stats)) =
        hero.single_mut()
    else {
        return;
    };

    for PurchaseItemRequest { item } in events.read() {
        let near_shop = shops.iter().any(|(shop_tf, shop)| {
            shop.team == *hero_team
                && flat_distance(hero_tf.translation, shop_tf.translation) <= shop.purchase_range
        });
        if !near_shop {
            continue;
        }
        if wallet.gold < item.cost() {
            continue;
        }
        if inv.first_empty().is_none() {
            continue;
        }
        wallet.gold -= item.cost();
        apply_passives(item.passives(), &mut health, &mut mana, &mut stats);
        let _ = inv.try_add(*item);
    }
}

/// Shared helper for purchases (used by tests and future sell/buy flows).
#[cfg_attr(not(test), allow(dead_code))]
pub fn try_buy_item(
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    item: ItemId,
    in_range: bool,
) -> bool {
    if !in_range || wallet.gold < item.cost() || inv.first_empty().is_none() {
        return false;
    }
    wallet.gold -= item.cost();
    apply_passives(item.passives(), health, mana, stats);
    inv.try_add(item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buy_applies_passives_and_fills_slot() {
        let mut wallet = PlayerWallet { gold: 600 };
        let mut inv = Inventory::empty();
        let mut health = Health::new(720.0);
        let mut mana = Mana::new(320.0, 12.0);
        let mut stats = CombatStats {
            attack_damage: 55.0,
            attack_range: 8.0,
            attack_speed: 1.1,
            armor: 4.0,
            magic_resist: 3.0,
            move_speed: 12.0,
        };
        assert!(try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            ItemId::IronBracer,
            true
        ));
        assert_eq!(wallet.gold, 300);
        assert!(inv.slots[0].is_some());
        assert_eq!(health.max, 820.0);
        assert_eq!(stats.armor, 8.0);
    }

    #[test]
    fn buy_requires_range_and_gold() {
        let mut wallet = PlayerWallet { gold: 100 };
        let mut inv = Inventory::empty();
        let mut health = Health::new(720.0);
        let mut mana = Mana::new(320.0, 12.0);
        let mut stats = CombatStats {
            attack_damage: 55.0,
            attack_range: 8.0,
            attack_speed: 1.1,
            armor: 4.0,
            magic_resist: 3.0,
            move_speed: 12.0,
        };
        assert!(!try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            ItemId::SwiftBoots,
            true
        ));
        wallet.gold = 500;
        assert!(!try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            ItemId::SwiftBoots,
            false
        ));
    }

    #[test]
    fn inventory_holds_six_items() {
        let mut inv = Inventory::empty();
        for id in ItemId::all().iter().take(6) {
            assert!(inv.try_add(*id));
        }
        assert!(!inv.try_add(ItemId::StormRod));
    }

    #[test]
    fn status_buff_applies_and_expires_modifiers() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats {
            attack_damage: 10.0,
            attack_range: 8.0,
            attack_speed: 1.0,
            armor: 2.0,
            magic_resist: 1.0,
            move_speed: 10.0,
        };
        apply_status(
            &mut statuses,
            &mut stats,
            StatusEffect {
                armor: 8.0,
                ..StatusEffect::buff("test", 1.0)
            },
        );
        assert_eq!(stats.armor, 10.0);
        assert_eq!(statuses.effects.len(), 1);
        statuses.effects[0].remaining = 0.0;
        // Simulate expire path
        let expired = statuses.effects.remove(0);
        stats.armor -= expired.armor;
        assert_eq!(stats.armor, 2.0);
    }
}
