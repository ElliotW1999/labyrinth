//! Item catalog, shop, inventory, active use (ASDZXC), and status effects.

use bevy::prelude::*;

use crate::combat::{apply_damage, flat_distance};
use crate::components::{
    CombatStats, DamageType, Health, Lifetime, Mana, PlayerHero, PlayerWallet, SpellFx, Team,
};
use crate::resources::SharedAssets;
use crate::scale;

pub struct ItemsPlugin;

impl Plugin for ItemsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShopUiState>()
            .init_resource::<InventoryContextMenu>()
            .add_message::<PurchaseItemRequest>()
            .add_message::<SellItemRequest>()
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
                    try_sell_from_ui,
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
    /// When true, skip collision with creeps and heroes (buildings/trees still block).
    pub ignore_unit_collision: bool,
    /// When true, this unit claims more separation against overlapping creeps/heroes.
    pub force_unit_push: bool,
    pub silenced: bool,
    pub stunned: bool,
    pub rooted: bool,
    pub disarmed: bool,
    pub debuff_immune: bool,
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
            ignore_unit_collision: false,
            force_unit_push: false,
            silenced: false,
            stunned: false,
            rooted: false,
            disarmed: false,
            debuff_immune: false,
        }
    }

    pub fn debuff(id: &'static str, duration: f32) -> Self {
        Self {
            kind: StatusKind::Debuff,
            ..Self::buff(id, duration)
        }
    }

    pub fn phased(duration: f32) -> Self {
        Self {
            ignore_unit_collision: true,
            ..Self::buff("phased", duration)
        }
    }

    /// Temporary soft-separation against other units.
    pub fn forceful(duration: f32) -> Self {
        Self {
            force_unit_push: true,
            ..Self::buff("forceful", duration)
        }
    }

    pub fn silenced(duration: f32) -> Self {
        Self {
            silenced: true,
            ..Self::debuff("silence", duration)
        }
    }

    pub fn stunned(duration: f32) -> Self {
        Self {
            stunned: true,
            ..Self::debuff("stun", duration)
        }
    }

    pub fn rooted(duration: f32) -> Self {
        Self {
            rooted: true,
            ..Self::debuff("root", duration)
        }
    }

    pub fn disarmed(duration: f32) -> Self {
        Self {
            disarmed: true,
            ..Self::debuff("disarm", duration)
        }
    }

    pub fn debuff_immunity(duration: f32) -> Self {
        Self {
            debuff_immune: true,
            ..Self::buff("spell_immunity", duration)
        }
    }
}

impl StatusEffects {
    pub fn is_phased(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.ignore_unit_collision && e.remaining > 0.0)
    }

    pub fn can_push_units(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.force_unit_push && e.remaining > 0.0)
    }

    pub fn is_silenced(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.silenced && e.remaining > 0.0)
    }

    pub fn is_stunned(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.stunned && e.remaining > 0.0)
    }

    pub fn is_rooted(&self) -> bool {
        self.effects
            .iter()
            .any(|e| (e.rooted || e.stunned) && e.remaining > 0.0)
    }

    pub fn is_disarmed(&self) -> bool {
        self.effects
            .iter()
            .any(|e| (e.disarmed || e.stunned) && e.remaining > 0.0)
    }

    pub fn has_debuff_immunity(&self) -> bool {
        self.effects
            .iter()
            .any(|e| e.debuff_immune && e.remaining > 0.0)
    }

    pub fn can_move(&self) -> bool {
        !self.is_rooted()
    }

    pub fn can_attack(&self) -> bool {
        !self.is_disarmed()
    }

    pub fn can_cast(&self) -> bool {
        !self.is_silenced() && !self.is_stunned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemId {
    // <item_generator:item_enum>
    IronBracer,
    SwiftBoots,
    ManaCrystal,
    BladeOfAsh,
    AegisCharm,
    VialOfLight,
    StormRod,
    WardstoneCloak,
    HeartwoodBand,
    SparkPendant,
    // </item_generator:item_enum>
}

impl ItemId {
    pub fn all() -> &'static [ItemId] {
        &[
            // <item_generator:item_all>
            ItemId::IronBracer,
            ItemId::SwiftBoots,
            ItemId::ManaCrystal,
            ItemId::BladeOfAsh,
            ItemId::AegisCharm,
            ItemId::VialOfLight,
            ItemId::StormRod,
            ItemId::WardstoneCloak,
            ItemId::HeartwoodBand,
            ItemId::SparkPendant,
            // </item_generator:item_all>
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            // <item_generator:item_name>
            ItemId::IronBracer => "Iron Bracer",
            ItemId::SwiftBoots => "Swift Boots",
            ItemId::ManaCrystal => "Mana Crystal",
            ItemId::BladeOfAsh => "Blade of Ash",
            ItemId::AegisCharm => "Aegis Charm",
            ItemId::VialOfLight => "Vial of Light",
            ItemId::StormRod => "Storm Rod",
            ItemId::WardstoneCloak => "Wardstone Cloak",
            ItemId::HeartwoodBand => "Heartwood Band",
            ItemId::SparkPendant => "Spark Pendant",
            // </item_generator:item_name>
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            // <item_generator:item_short_label>
            ItemId::IronBracer => "IB",
            ItemId::SwiftBoots => "SB",
            ItemId::ManaCrystal => "MC",
            ItemId::BladeOfAsh => "BA",
            ItemId::AegisCharm => "AC",
            ItemId::VialOfLight => "VL",
            ItemId::StormRod => "SR",
            ItemId::WardstoneCloak => "WC",
            ItemId::HeartwoodBand => "HB",
            ItemId::SparkPendant => "SP",
            // </item_generator:item_short_label>
        }
    }

    pub fn cost(self) -> u32 {
        match self {
            // <item_generator:item_cost>
            ItemId::IronBracer => 300,
            ItemId::SwiftBoots => 450,
            ItemId::ManaCrystal => 350,
            ItemId::BladeOfAsh => 800,
            ItemId::AegisCharm => 700,
            ItemId::VialOfLight => 400,
            ItemId::StormRod => 900,
            ItemId::WardstoneCloak => 650,
            ItemId::HeartwoodBand => 550,
            ItemId::SparkPendant => 750,
            // </item_generator:item_cost>
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            // <item_generator:item_description>
            ItemId::IronBracer => "+100 HP, +4 Armor",
            ItemId::SwiftBoots => "+2.5 Move Speed",
            ItemId::ManaCrystal => "+80 Mana, +4 Mana Regen",
            ItemId::BladeOfAsh => "+18 Attack Damage",
            ItemId::AegisCharm => "+6 Armor, +5 Magic Resist",
            ItemId::VialOfLight => "Active: Heal 180 HP",
            ItemId::StormRod => "+12 AD. Active: 140 magic AoE",
            ItemId::WardstoneCloak => "+80 HP, +4 MR. Active: +8 Armor 5s",
            ItemId::HeartwoodBand => "+150 HP. +3 Armor. +2 Mana Regen",
            ItemId::SparkPendant => "+10 Attack Damage. +60 Mana. Active: On use: deal 90 magical damage to enemies in a 4.5 radius and restore 40 mana to self",
            // </item_generator:item_description>
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
            // <item_generator:item_color>
            ItemId::IronBracer => Color::srgb(0.55, 0.55, 0.6),
            ItemId::SwiftBoots => Color::srgb(0.35, 0.7, 0.45),
            ItemId::ManaCrystal => Color::srgb(0.3, 0.45, 0.95),
            ItemId::BladeOfAsh => Color::srgb(0.85, 0.35, 0.25),
            ItemId::AegisCharm => Color::srgb(0.7, 0.65, 0.35),
            ItemId::VialOfLight => Color::srgb(0.95, 0.9, 0.55),
            ItemId::StormRod => Color::srgb(0.45, 0.35, 0.95),
            ItemId::WardstoneCloak => Color::srgb(0.4, 0.55, 0.5),
            ItemId::HeartwoodBand => Color::srgb(0.405294, 0.452353, 0.558235),
            ItemId::SparkPendant => Color::srgb(0.85, 0.55, 0.25),
            // </item_generator:item_color>
        }
    }

    pub fn active_cooldown(self) -> Option<f32> {
        match self {
            ItemId::VialOfLight => Some(40.0),
            ItemId::StormRod => Some(30.0),
            ItemId::WardstoneCloak => Some(35.0),
            // <item_generator:active_cooldown>
            ItemId::SparkPendant => Some(25.0),
            // </item_generator:active_cooldown>
            _ => None,
        }
    }

    /// Pseudocode for actives (ItemGenerator fills this for stub kits).
    pub fn active_pseudocode(self) -> Option<&'static str> {
        match self {
            // <item_generator:active_pseudocode>
            ItemId::VialOfLight => Some("Heal self for 180 HP"),
            ItemId::StormRod => Some("Deal 140 magical damage in 5.5 radius around caster"),
            ItemId::WardstoneCloak => Some("Gain +8 armor for 5 seconds"),
            ItemId::SparkPendant => Some("On use: deal 90 magical damage to enemies in a 4.5 radius and restore 40 mana to self"),
            // </item_generator:active_pseudocode>
            _ => None,
        }
    }

    /// Optional recipe components (empty = basic item). Recipe combining is TBD.
    pub fn recipe_components(self) -> &'static [ItemId] {
        match self {
            // <item_generator:recipe_components>
            ItemId::HeartwoodBand => &[ItemId::IronBracer],
            ItemId::SparkPendant => &[ItemId::ManaCrystal, ItemId::BladeOfAsh],
            // </item_generator:recipe_components>
            _ => &[],
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
    pub attack_speed_flat: f32,
}

impl ItemId {
    pub fn passives(self) -> ItemPassives {
        match self {
            // <item_generator:item_passives>
            ItemId::IronBracer => ItemPassives {
                max_health: 100.0,
                armor: 4.0,
                ..default()
            },
            ItemId::SwiftBoots => ItemPassives {
                move_speed: scale::u(2.5),
                ..default()
            },
            ItemId::ManaCrystal => ItemPassives {
                max_mana: 80.0,
                mana_regen: 4.0,
                ..default()
            },
            ItemId::BladeOfAsh => ItemPassives {
                attack_damage: 18.0,
                attack_speed_flat: 15.0,
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
                attack_speed_flat: 10.0,
                ..default()
            },
            ItemId::WardstoneCloak => ItemPassives {
                max_health: 80.0,
                magic_resist: 4.0,
                ..default()
            },
            ItemId::HeartwoodBand => ItemPassives {
                max_health: 150.0,
                armor: 3.0,
                mana_regen: 2.0,
                ..default()
            },
            ItemId::SparkPendant => ItemPassives {
                attack_damage: 10.0,
                max_mana: 60.0,
                ..default()
            },
            // </item_generator:item_passives>
        }
    }
}

pub fn apply_passives(
    passives: ItemPassives,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    agility: f32,
) {
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
    stats.attack_speed_flat += passives.attack_speed_flat;
    stats.recompute_attack_speed(agility);
}

pub fn remove_passives(
    passives: ItemPassives,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    agility: f32,
) {
    if passives.max_health != 0.0 {
        health.max = (health.max - passives.max_health).max(1.0);
        health.current = health.current.min(health.max);
    }
    if passives.max_mana != 0.0 {
        mana.max = (mana.max - passives.max_mana).max(0.0);
        mana.current = mana.current.min(mana.max);
    }
    mana.regen_per_sec = (mana.regen_per_sec - passives.mana_regen).max(0.0);
    stats.attack_damage -= passives.attack_damage;
    stats.armor -= passives.armor;
    stats.magic_resist -= passives.magic_resist;
    stats.move_speed -= passives.move_speed;
    stats.attack_speed_flat -= passives.attack_speed_flat;
    stats.recompute_attack_speed(agility);
}

fn spawn_shops(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(Cuboid::new(scale::u(2.4), scale::u(2.0), scale::u(2.4)));
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
        Transform::from_xyz(scale::u(-52.0), scale::u(1.0), scale::u(-40.0)),
        ItemShop {
            team: Team::Radiant,
            purchase_range: scale::u(14.0),
        },
    ));
    // Near Dire base (for future Dire heroes / symmetry).
    commands.spawn((
        Name::new("Dire Item Shop"),
        Mesh3d(mesh),
        MeshMaterial3d(dire_mat),
        Transform::from_xyz(scale::u(52.0), scale::u(1.0), scale::u(40.0)),
        ItemShop {
            team: Team::Dire,
            purchase_range: scale::u(14.0),
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

/// Apply a phased buff (no combat-stat mutation).
pub fn apply_phased(statuses: &mut StatusEffects, duration: f32) {
    statuses.effects.retain(|e| e.id != "phased");
    statuses.effects.push(StatusEffect::phased(duration));
}

/// Apply a forceful buff so this unit claims more separation against overlaps.
pub fn apply_forceful(statuses: &mut StatusEffects, duration: f32) {
    statuses.effects.retain(|e| e.id != "forceful");
    statuses.effects.push(StatusEffect::forceful(duration));
}

/// Apply a timed buff/debuff and immediately add its flat modifiers to combat stats.
/// Debuffs are ignored while the unit has debuff immunity.
pub fn apply_status(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    effect: StatusEffect,
) -> bool {
    if effect.kind == StatusKind::Debuff && statuses.has_debuff_immunity() {
        return false;
    }
    // Refresh same-id effects by replacing.
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
    true
}

pub fn apply_silence(statuses: &mut StatusEffects, stats: &mut CombatStats, duration: f32) -> bool {
    apply_status(statuses, stats, StatusEffect::silenced(duration))
}

pub fn apply_stun(statuses: &mut StatusEffects, stats: &mut CombatStats, duration: f32) -> bool {
    apply_status(statuses, stats, StatusEffect::stunned(duration))
}

pub fn apply_root(statuses: &mut StatusEffects, stats: &mut CombatStats, duration: f32) -> bool {
    apply_status(statuses, stats, StatusEffect::rooted(duration))
}

pub fn apply_disarm(statuses: &mut StatusEffects, stats: &mut CombatStats, duration: f32) -> bool {
    apply_status(statuses, stats, StatusEffect::disarmed(duration))
}

pub fn apply_debuff_immunity(
    statuses: &mut StatusEffects,
    stats: &mut CombatStats,
    duration: f32,
) -> bool {
    // Purge existing debuffs when gaining immunity.
    statuses.effects.retain(|e| e.kind != StatusKind::Debuff);
    apply_status(statuses, stats, StatusEffect::debuff_immunity(duration))
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
            let radius = scale::u(5.5);
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
                Transform::from_translation(origin + Vec3::Y * scale::u(0.15))
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
        // <item_generator:active_arms>
        // Generated actives: stub until pseudocode is implemented.
        ItemId::SparkPendant => {
            // ACTIVE_PSEUDOCODE: On use: deal 90 magical damage to enemies in a 4.5 radius and restore 40 mana to self
            // TODO: implement active from ItemGenerator active_pseudocode
        }
        // </item_generator:active_arms>
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
        Transform::from_translation(at + Vec3::Y * scale::u(0.2)).with_scale(Vec3::splat(scale::u(1.5))),
        SpellFx {
            age: 0.0,
            lifetime: 0.5,
            start_scale: scale::u(1.0),
            end_scale: scale::u(3.0),
        },
        Lifetime(0.5),
    ));
}

#[derive(Message, Debug, Clone, Copy)]
pub struct PurchaseItemRequest {
    pub item: ItemId,
}

#[derive(Message, Debug, Clone, Copy)]
pub struct SellItemRequest {
    pub slot: usize,
}

/// Right-click inventory context menu state.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct InventoryContextMenu {
    pub slot: Option<usize>,
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
            &crate::components::HeroAttributes,
        ),
        With<PlayerHero>,
    >,
    shops: Query<(&Transform, &ItemShop)>,
) {
    let Ok((hero_tf, hero_team, mut wallet, mut inv, mut health, mut mana, mut stats, attrs)) =
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
        apply_passives(
            item.passives(),
            &mut health,
            &mut mana,
            &mut stats,
            attrs.agility,
        );
        let _ = inv.try_add(*item);
    }
}

fn try_sell_from_ui(
    mut events: MessageReader<SellItemRequest>,
    mut menu: ResMut<InventoryContextMenu>,
    mut hero: Query<
        (
            &Transform,
            &Team,
            &mut PlayerWallet,
            &mut Inventory,
            &mut Health,
            &mut Mana,
            &mut CombatStats,
            &crate::components::HeroAttributes,
        ),
        With<PlayerHero>,
    >,
    shops: Query<(&Transform, &ItemShop)>,
) {
    let Ok((hero_tf, hero_team, mut wallet, mut inv, mut health, mut mana, mut stats, attrs)) =
        hero.single_mut()
    else {
        return;
    };
    let near_shop = shops.iter().any(|(shop_tf, shop)| {
        shop.team == *hero_team
            && flat_distance(hero_tf.translation, shop_tf.translation) <= shop.purchase_range
    });
    for SellItemRequest { slot } in events.read() {
        if try_sell_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            *slot,
            near_shop,
            attrs.agility,
        ) {
            menu.slot = None;
        }
    }
}

/// Shared helper for purchases (used by tests).
#[cfg_attr(not(test), allow(dead_code))]
pub fn try_buy_item(
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    item: ItemId,
    in_range: bool,
    agility: f32,
) -> bool {
    if !in_range || wallet.gold < item.cost() || inv.first_empty().is_none() {
        return false;
    }
    wallet.gold -= item.cost();
    apply_passives(item.passives(), health, mana, stats, agility);
    inv.try_add(item)
}

/// Sell inventory slot for half the shop cost. Must be in shop range.
pub fn try_sell_item(
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    slot: usize,
    in_range: bool,
    agility: f32,
) -> bool {
    if !in_range {
        return false;
    }
    let Some(item) = inv.slots.get_mut(slot).and_then(|s| s.take()) else {
        return false;
    };
    remove_passives(item.id.passives(), health, mana, stats, agility);
    wallet.gold += item.id.cost() / 2;
    true
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
        let mut stats = CombatStats::simple(55.0, 8.0, 1.1, 4.0, 3.0, 12.0);
        assert!(try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            ItemId::IronBracer,
            true,
            18.0,
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
        let mut stats = CombatStats::simple(55.0, 8.0, 1.1, 4.0, 3.0, 12.0);
        assert!(!try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            ItemId::SwiftBoots,
            true,
            18.0,
        ));
        wallet.gold = 500;
        assert!(!try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            ItemId::SwiftBoots,
            false,
            18.0,
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
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 10.0);
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

    #[test]
    fn phased_buff_flags_ignore_unit_collision() {
        let mut statuses = StatusEffects::default();
        assert!(!statuses.is_phased());
        apply_phased(&mut statuses, 1.0);
        assert!(statuses.is_phased());
        assert!(statuses.effects.iter().any(|e| e.ignore_unit_collision));
        statuses.effects[0].remaining = 0.0;
        assert!(!statuses.is_phased());
    }

    #[test]
    fn forceful_buff_enables_unit_push() {
        let mut statuses = StatusEffects::default();
        assert!(!statuses.can_push_units());
        apply_forceful(&mut statuses, 2.0);
        assert!(statuses.can_push_units());
        assert!(!statuses.is_phased());
        statuses.effects[0].remaining = 0.0;
        assert!(!statuses.can_push_units());
    }

    #[test]
    fn sell_refunds_half_and_removes_passives() {
        let mut wallet = PlayerWallet { gold: 600 };
        let mut inv = Inventory::empty();
        let mut health = Health::new(720.0);
        let mut mana = Mana::new(320.0, 12.0);
        let mut stats = CombatStats::simple(55.0, 8.0, 1.1, 4.0, 3.0, 12.0);
        assert!(try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            ItemId::IronBracer,
            true,
            18.0,
        ));
        assert!(try_sell_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            0,
            true,
            18.0,
        ));
        assert_eq!(wallet.gold, 450); // 600 - 300 + 150
        assert!(inv.slots[0].is_none());
        assert_eq!(health.max, 720.0);
        assert_eq!(stats.armor, 4.0);
    }

    #[test]
    fn debuff_immunity_blocks_stun() {
        let mut statuses = StatusEffects::default();
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 2.0, 1.0, 10.0);
        assert!(apply_debuff_immunity(&mut statuses, &mut stats, 5.0));
        assert!(!apply_stun(&mut statuses, &mut stats, 2.0));
        assert!(!statuses.is_stunned());
    }

    #[test]
    fn attack_speed_formula_clamps() {
        let mut stats = CombatStats::simple(10.0, 8.0, 1.0, 0.0, 0.0, 10.0);
        stats.base_attack_speed = 100.0;
        stats.attack_speed_flat = 50.0;
        stats.attack_speed_mult = 0.5;
        stats.base_attack_time = 1.0;
        let ias = stats.attack_speed_rating(20.0); // (100+20+50)*1.5 = 255
        assert!((ias - 255.0).abs() < 0.01);
        stats.attack_speed_flat = 1000.0;
        assert_eq!(stats.attack_speed_rating(0.0), 700.0);
        stats.base_attack_speed = 1.0;
        stats.attack_speed_flat = 0.0;
        stats.attack_speed_mult = 0.0;
        assert_eq!(stats.attack_speed_rating(0.0), 20.0);
    }
}
