//! Item catalog, shop, inventory, active use (ASDZXC), and status effects.

use bevy::prelude::*;

use crate::abilities::definition::{
    AbilityEffect, AbilityEffectEntry, AbilityTrigger, AreaRadius, Attribute, DisplacementKind,
    EffectScaling, EffectTarget, PassiveStat, RankValue, StatusSpec,
};
use crate::abilities::effects::{AbilityTriggerEvent, EffectSource, TriggerContext};
use crate::basic_attack::{BasicAttackImpactEvent, BasicAttackReleaseEvent};
use crate::combat::flat_distance;
use crate::components::{
    CombatStats, DamageType, Health, HeroAttributes, Lifetime, Mana, PlayerHero, PlayerWallet,
    SpellFx, Team,
};
use crate::resources::SharedAssets;
use crate::scale;

pub struct ItemsPlugin;

impl Plugin for ItemsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShopUiState>()
            .init_resource::<ShopUndoLog>()
            .init_resource::<InventoryContextMenu>()
            .add_message::<PurchaseItemRequest>()
            .add_message::<SellItemRequest>()
            .add_message::<UndoShopRequest>()
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
                    item_attack_triggers,
                    try_purchase_from_ui,
                    try_sell_from_ui,
                    try_undo_from_ui,
                    clear_shop_undo_outside_range,
                ),
            );
    }
}

/// Whether the shop panel is open (toggled from the HUD button).
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct ShopUiState {
    pub open: bool,
    /// Item shown in the components / detail popup (None = closed).
    pub detail: Option<ItemId>,
}

/// Purchases and sales that can still be reversed. Cleared on leaving shop range.
#[derive(Resource, Debug, Clone, Default)]
pub struct ShopUndoLog {
    actions: Vec<ShopAction>,
    /// Shown in the shop when the latest undo could not be applied.
    pub notice: String,
}

impl ShopUndoLog {
    pub fn pending(&self) -> usize {
        self.actions.len()
    }

    fn clear_pending(&mut self) {
        self.actions.clear();
        self.notice.clear();
    }
}

#[derive(Debug, Clone)]
enum ShopAction {
    /// Net inventory change of one buy, including any auto-combine that followed it.
    Purchase {
        gold_spent: u32,
        /// Stacks consumed (components that were already owned, after a combine).
        removed: Vec<(ItemId, u32)>,
        /// Stacks gained (the bought item, or the combined result).
        added: Vec<(ItemId, u32)>,
    },
    Sale {
        refunded: u32,
        item: ItemInstance,
    },
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

    /// Slot that would receive one more `id`: a partial stack first, then an empty slot.
    pub fn slot_for(&self, id: ItemId) -> Option<usize> {
        let max_stack = id.definition().max_stack;
        self.slots
            .iter()
            .position(|s| s.is_some_and(|item| item.id == id && item.stack < max_stack))
            .or_else(|| self.first_empty())
    }

    pub fn try_add(&mut self, id: ItemId) -> bool {
        let Some(i) = self.slot_for(id) else {
            return false;
        };
        match self.slots[i].as_mut() {
            Some(item) => item.stack += 1,
            None => self.slots[i] = Some(ItemInstance::new(id)),
        }
        true
    }

    fn count_of(&self, id: ItemId) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|item| item.id == id)
            .map(|item| item.stack)
            .sum()
    }

    /// Remove a single unit of `id`. Stacks above 1 shrink in place.
    fn remove_one(&mut self, id: ItemId) -> bool {
        for slot in &mut self.slots {
            let Some(item) = slot else {
                continue;
            };
            if item.id != id || item.stack == 0 {
                continue;
            }
            item.stack -= 1;
            if item.stack == 0 {
                *slot = None;
            }
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemInstance {
    pub id: ItemId,
    pub cooldown_remaining: f32,
    /// Units in this slot (stackable items).
    pub stack: u32,
    /// Charges left on the current unit (items with `max_charges`).
    pub charges: Option<u32>,
}

impl ItemInstance {
    pub fn new(id: ItemId) -> Self {
        Self {
            id,
            cooldown_remaining: 0.0,
            stack: 1,
            charges: id.definition().max_charges,
        }
    }

    /// Spend one use of a consumable with `max_charges` per unit. Returns how many
    /// units were used up (0 or 1).
    pub fn consume(&mut self, max_charges: Option<u32>) -> u32 {
        if let Some(charges) = self.charges.as_mut() {
            *charges = charges.saturating_sub(1);
            if *charges > 0 {
                return 0;
            }
            self.charges = max_charges;
        }
        self.stack = self.stack.saturating_sub(1);
        1
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
    RecipeHeartwoodBand,
    RecipeSparkPendant,
    // </item_generator:item_enum>
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ItemType {
    Component,
    Weapon,
    Armor,
    Accessory,
    Consumable,
    Utility,
    /// Recipe scrap combined with components; hidden from the default shop list.
    Recipe,
    Attributes,
    Equipment,
    Miscellaneous,
    Accessories,
}

impl ItemType {
    pub fn label(self) -> &'static str {
        match self {
            ItemType::Component => "Component",
            ItemType::Weapon => "Weapon",
            ItemType::Armor => "Armor",
            ItemType::Accessory => "Accessory",
            ItemType::Consumable => "Consumable",
            ItemType::Utility => "Utility",
            ItemType::Recipe => "Recipe",
            ItemType::Attributes => "Attributes",
            ItemType::Equipment => "Equipment",
            ItemType::Miscellaneous => "Miscellaneous",
            ItemType::Accessories => "Accessories",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)]
pub enum ItemRarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

impl ItemRarity {
    pub fn label(self) -> &'static str {
        match self {
            ItemRarity::Common => "Common",
            ItemRarity::Uncommon => "Uncommon",
            ItemRarity::Rare => "Rare",
            ItemRarity::Epic => "Epic",
            ItemRarity::Legendary => "Legendary",
        }
    }
}

/// Static item data: identity / economy / inventory rules plus trigger-tagged effects
/// (the same [`AbilityEffectEntry`] rows abilities use). Generated from
/// `data/items.csv` + `data/item_effects.csv` into `item_catalog.rs`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemDefinition {
    pub id: ItemId,
    pub name: &'static str,
    pub short_label: &'static str,
    pub item_type: ItemType,
    pub cost: u32,
    pub sell_value: u32,
    pub tier: u32,
    pub rarity: ItemRarity,
    /// Units per inventory slot (1 = not stackable).
    pub max_stack: u32,
    /// Used up on activation (one charge, or one unit when the item has no charges).
    pub consumable: bool,
    pub max_charges: Option<u32>,
    /// Active cooldown in seconds (items with `OnUse` effects).
    pub cooldown: f32,
    pub components: &'static [ItemId],
    pub color: [f32; 3],
    pub description: &'static str,
    pub effects: &'static [AbilityEffectEntry],
}

impl ItemDefinition {
    pub fn effects_for(&self, trigger: AbilityTrigger) -> impl Iterator<Item = &AbilityEffectEntry> {
        self.effects.iter().filter(move |entry| entry.trigger == trigger)
    }
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
            ItemId::RecipeHeartwoodBand,
            ItemId::RecipeSparkPendant,
            // </item_generator:item_all>
        ]
    }

    pub fn definition(self) -> &'static ItemDefinition {
        crate::item_catalog::definition(self)
    }

    /// Items shown in the default shop grid (recipes are inspect-only via builds).
    pub fn shop_listed() -> impl Iterator<Item = ItemId> {
        Self::all().iter().copied().filter(|id| !id.is_recipe())
    }

    /// Recipe scraps used in builds — hidden from the default shop list.
    pub fn is_recipe(self) -> bool {
        self.definition().item_type == ItemType::Recipe
    }

    pub fn name(self) -> &'static str {
        self.definition().name
    }

    pub fn short_label(self) -> &'static str {
        self.definition().short_label
    }

    pub fn cost(self) -> u32 {
        self.definition().cost
    }

    pub fn description(self) -> &'static str {
        self.definition().description
    }

    pub fn tooltip_body(self) -> String {
        let def = self.definition();
        let kind = if self.is_recipe() {
            "Recipe"
        } else if self.has_active() {
            "Active"
        } else {
            "Passive"
        };
        let mut body = format!(
            "{name}\nBuy: {cost}g    Sell: {sell}g\n{kind} · {item_type} · {rarity} · Tier {tier}\n{desc}",
            name = def.name,
            cost = def.cost,
            sell = def.sell_value,
            item_type = def.item_type.label(),
            rarity = def.rarity.label(),
            tier = def.tier,
            desc = def.description,
        );
        if def.consumable {
            body.push_str("\nConsumable");
        }
        if let Some(charges) = def.max_charges {
            body.push_str(&format!("\nCharges: {charges}"));
        }
        if def.max_stack > 1 {
            body.push_str(&format!("\nStacks to {}", def.max_stack));
        }
        if def.cooldown > 0.0 && self.has_active() {
            body.push_str(&format!("\nCooldown: {:.0}s", def.cooldown));
        }
        let stats = passive_stat_lines(&self.passives());
        if !stats.is_empty() {
            body.push_str("\n\nStats:");
            for line in stats {
                body.push_str(&format!("\n{line}"));
            }
        }
        let mut effects = Vec::new();
        for entry in def.effects {
            if entry.trigger == AbilityTrigger::Passive {
                continue;
            }
            if let Some(line) = describe_item_effect(entry) {
                effects.push(line);
            }
        }
        if !effects.is_empty() {
            body.push_str("\n\nEffects:");
            for line in effects {
                body.push_str(&format!("\n{line}"));
            }
        }
        let comps = self.recipe_components();
        if !comps.is_empty() {
            body.push_str("\n\nComponents:");
            for c in comps {
                body.push_str(&format!("\n• {} ({}g)", c.name(), c.cost()));
            }
        }
        body
    }

    pub fn placeholder_color(self) -> Color {
        let [r, g, b] = self.definition().color;
        Color::srgb(r, g, b)
    }

    pub fn active_cooldown(self) -> Option<f32> {
        self.has_active().then(|| self.definition().cooldown)
    }

    /// Build tree components (other items and/or a recipe scrap). Empty = basic item.
    pub fn recipe_components(self) -> &'static [ItemId] {
        self.definition().components
    }

    pub fn has_active(self) -> bool {
        self.definition().effects_for(AbilityTrigger::OnUse).next().is_some()
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

/// Flat passive bonuses applied once per owned unit on purchase.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ItemPassives {
    pub max_health: f32,
    pub health_regen: f32,
    pub max_mana: f32,
    pub mana_regen: f32,
    pub attack_damage: f32,
    pub armor: f32,
    pub magic_resist: f32,
    pub move_speed: f32,
    pub attack_speed_flat: f32,
    pub strength: f32,
    pub agility: f32,
    pub intelligence: f32,
}

impl ItemId {
    /// Sum of the item's `Passive` stat entries.
    pub fn passives(self) -> ItemPassives {
        let mut p = ItemPassives::default();
        for entry in self.definition().effects_for(AbilityTrigger::Passive) {
            let AbilityEffect::PassiveStat { stat, amount } = entry.effect else {
                continue;
            };
            let value = amount.at(1);
            let field = match stat {
                PassiveStat::MaxHealth => &mut p.max_health,
                PassiveStat::HealthRegen => &mut p.health_regen,
                PassiveStat::MaxMana => &mut p.max_mana,
                PassiveStat::ManaRegen => &mut p.mana_regen,
                PassiveStat::AttackDamage => &mut p.attack_damage,
                PassiveStat::AttackSpeed => &mut p.attack_speed_flat,
                PassiveStat::Armor => &mut p.armor,
                PassiveStat::MagicResist => &mut p.magic_resist,
                PassiveStat::MoveSpeed => &mut p.move_speed,
                PassiveStat::Attribute(Attribute::Strength) => &mut p.strength,
                PassiveStat::Attribute(Attribute::Agility) => &mut p.agility,
                PassiveStat::Attribute(Attribute::Intelligence) => &mut p.intelligence,
            };
            *field += value;
        }
        p
    }
}

fn passive_stat_lines(passives: &ItemPassives) -> Vec<String> {
    let rows = [
        (passives.max_health, "Health"),
        (passives.health_regen, "Health regen"),
        (passives.max_mana, "Mana"),
        (passives.mana_regen, "Mana regen"),
        (passives.attack_damage, "Attack damage"),
        (passives.attack_speed_flat, "Attack speed"),
        (passives.armor, "Armor"),
        (passives.magic_resist, "Magic resist"),
        (passives.move_speed, "Move speed"),
        (passives.strength, "Strength"),
        (passives.agility, "Agility"),
        (passives.intelligence, "Intelligence"),
    ];
    rows.into_iter()
        .filter(|(value, _)| value.abs() >= 0.05)
        .map(|(value, label)| format!("{:+.0} {label}", value))
        .collect()
}

fn describe_item_effect(entry: &AbilityEffectEntry) -> Option<String> {
    let tag = match entry.trigger {
        AbilityTrigger::OnUse => "Use",
        AbilityTrigger::OnAttack => "On attack",
        AbilityTrigger::OnAttackHit => "On attack hit",
        AbilityTrigger::OnDamageTaken => "When damaged",
        AbilityTrigger::OnKill => "On kill",
        AbilityTrigger::OnDeath => "On death",
        AbilityTrigger::Passive => return None,
        other => return Some(format!("{other:?}")),
    };
    let detail = match entry.effect {
        AbilityEffect::Damage { amount, damage_type, scaling } => {
            let mut line = format!(
                "Deal {} {} damage",
                fmt_rank(amount),
                damage_label(damage_type)
            );
            if let Some(note) = scaling_label(scaling) {
                line.push_str(&format!(" ({note})"));
            }
            line.push_str(&target_suffix(&entry.target));
            line
        }
        AbilityEffect::Heal { amount, scaling } => {
            let mut line = format!("Heal {}", fmt_rank(amount));
            if let Some(note) = scaling_label(scaling) {
                line.push_str(&format!(" ({note})"));
            }
            line
        }
        AbilityEffect::RestoreMana { amount } => format!("Restore {} mana", fmt_rank(amount)),
        AbilityEffect::ApplyStatus { status, duration } => match status {
            StatusSpec::Slow { percent } => {
                format!("Slow {}% for {}s", fmt_rank(percent), fmt_rank(duration))
            }
            StatusSpec::Stun => format!("Stun for {}s", fmt_rank(duration)),
            StatusSpec::Root => format!("Root for {}s", fmt_rank(duration)),
            StatusSpec::Silence => format!("Silence for {}s", fmt_rank(duration)),
            StatusSpec::Disarm => format!("Disarm for {}s", fmt_rank(duration)),
            StatusSpec::Modifier { armor, magic_resist, move_speed, attack_damage, .. } => {
                let mut parts = Vec::new();
                if armor != RankValue::ZERO {
                    parts.push(format!("{:+.0} armor", armor.at(1)));
                }
                if magic_resist != RankValue::ZERO {
                    parts.push(format!("{:+.0} magic resist", magic_resist.at(1)));
                }
                if move_speed != RankValue::ZERO {
                    parts.push(format!("{:+.0} move speed", move_speed.at(1)));
                }
                if attack_damage != RankValue::ZERO {
                    parts.push(format!("{:+.0} attack damage", attack_damage.at(1)));
                }
                format!("{} for {}s", parts.join(", "), fmt_rank(duration))
            }
            other => format!("{other:?} for {}s", fmt_rank(duration)),
        },
        AbilityEffect::Displace { kind, distance, duration } => {
            let name = match kind {
                DisplacementKind::Knockback => "Knockback",
                DisplacementKind::Pull => "Pull",
            };
            match distance {
                Some(distance) => format!("{name} {} over {}s", fmt_rank(distance), fmt_rank(duration)),
                None => format!("{name} to the caster over {}s", fmt_rank(duration)),
            }
        }
        AbilityEffect::Dispel { kind } => format!("Dispel {kind:?}s"),
        AbilityEffect::RemoveStatus { id } => format!("Remove {id}"),
        AbilityEffect::Custom { id, .. } => format!("Custom: {id}"),
        AbilityEffect::PassiveStat { .. } => return None,
    };
    Some(format!("{tag}: {detail}"))
}

fn target_suffix(target: &EffectTarget) -> String {
    match target {
        EffectTarget::UnitsInRadius { radius, team, .. } => {
            let reach = match radius {
                AreaRadius::Ability => "the area".into(),
                AreaRadius::Fixed(value) => format!("{}", fmt_rank(*value)),
            };
            format!(" to {team:?} in {reach}")
        }
        _ => String::new(),
    }
}

fn scaling_label(scaling: EffectScaling) -> Option<String> {
    match scaling {
        EffectScaling::None => None,
        EffectScaling::CasterAttackDamage(ratio) => Some(format!("+ {ratio:.2}× attack damage")),
        EffectScaling::CasterAttribute(attribute, ratio) => {
            Some(format!("+ {ratio:.2}× {attribute:?}"))
        }
        EffectScaling::TargetHealthBelow(fraction) => {
            Some(format!("below {:.0}% health", fraction * 100.0))
        }
    }
}

fn damage_label(damage_type: DamageType) -> &'static str {
    match damage_type {
        DamageType::Physical => "physical",
        DamageType::Magical => "magical",
        DamageType::Pure => "pure",
    }
}

fn fmt_rank(value: RankValue) -> String {
    let number = value.at(1);
    if (number - number.round()).abs() < 0.05 {
        format!("{}", number.round() as i32)
    } else {
        format!("{number:.1}")
    }
}

/// Apply `count` copies of an item's passive bonuses (negative `count` removes them).
fn adjust_passives(
    passives: ItemPassives,
    count: f32,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
) {
    let before = attrs.clone();
    attrs.strength += passives.strength * count;
    attrs.agility += passives.agility * count;
    attrs.intelligence += passives.intelligence * count;
    HeroAttributes::apply_delta(&before, attrs, health, mana, stats);

    let max_health = passives.max_health * count;
    health.max = (health.max + max_health).max(1.0);
    health.current = (health.current + max_health.max(0.0)).min(health.max);
    health.regen_per_sec = (health.regen_per_sec + passives.health_regen * count).max(0.0);
    let max_mana = passives.max_mana * count;
    mana.max = (mana.max + max_mana).max(0.0);
    mana.current = (mana.current + max_mana.max(0.0)).min(mana.max);
    mana.regen_per_sec = (mana.regen_per_sec + passives.mana_regen * count).max(0.0);
    stats.attack_damage += passives.attack_damage * count;
    stats.armor += passives.armor * count;
    stats.magic_resist += passives.magic_resist * count;
    stats.move_speed += passives.move_speed * count;
    stats.attack_speed_flat += passives.attack_speed_flat * count;
    stats.recompute_attack_speed(attrs.agility);
}

pub fn apply_passives(
    passives: ItemPassives,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
) {
    adjust_passives(passives, 1.0, health, mana, stats, attrs);
}

pub fn remove_passives(
    passives: ItemPassives,
    count: u32,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
) {
    adjust_passives(passives, -(count as f32), health, mana, stats, attrs);
}

/// Gold shop beside a team's ancient. Layout uses map scale, not the old `scale::u` grid.
pub fn shop_world_position(team: Team) -> Vec3 {
    let ground = match team {
        Team::Radiant => scale::ground(-55.0, -48.0),
        Team::Dire => scale::ground(55.0, 48.0),
    };
    let height = 140.0;
    Vec3::new(ground.x, height * 0.5, ground.z)
}

pub fn shop_purchase_range() -> f32 {
    scale::map(6.0)
}

fn spawn_shops(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let height = 140.0;
    let mesh = meshes.add(Cuboid::new(height, height, height));
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

    commands.spawn((
        Name::new("Radiant Item Shop"),
        Mesh3d(mesh.clone()),
        MeshMaterial3d(radiant_mat),
        Transform::from_translation(shop_world_position(Team::Radiant)),
        ItemShop {
            team: Team::Radiant,
            purchase_range: shop_purchase_range(),
        },
    ));
    commands.spawn((
        Name::new("Dire Item Shop"),
        Mesh3d(mesh),
        MeshMaterial3d(dire_mat),
        Transform::from_translation(shop_world_position(Team::Dire)),
        ItemShop {
            team: Team::Dire,
            purchase_range: shop_purchase_range(),
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
#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_phased(statuses: &mut StatusEffects, duration: f32) {
    statuses.effects.retain(|e| e.id != "phased");
    statuses.effects.push(StatusEffect::phased(duration));
}

/// Apply a forceful buff so this unit claims more separation against overlaps.
#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_stun(statuses: &mut StatusEffects, stats: &mut CombatStats, duration: f32) -> bool {
    apply_status(statuses, stats, StatusEffect::stunned(duration))
}

#[cfg_attr(not(test), allow(dead_code))]
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
            &mut Mana,
            &mut CombatStats,
            &mut HeroAttributes,
        ),
        With<PlayerHero>,
    >,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
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

    let Ok((hero_entity, transform, team, mut inv, mut health, mut mana, mut stats, mut attrs)) =
        hero.single_mut()
    else {
        return;
    };

    for index in pressed {
        let Some(used) = try_use_item(&mut inv, index, hero_entity, transform.translation, *team, &mut triggers) else {
            continue;
        };
        spawn_item_use_fx(&mut commands, &assets, used, transform.translation);
        if let Some(item) = inv.slots[index].as_mut().filter(|_| used.definition().consumable) {
            if item.consume(used.definition().max_charges) > 0 {
                remove_passives(used.passives(), 1, &mut health, &mut mana, &mut stats, &mut attrs);
            }
            if item.stack == 0 {
                inv.slots[index] = None;
            }
        }
    }
}

/// Starts the active's cooldown and fires its `OnUse` trigger; effects are resolved by
/// the shared trigger pipeline. Returns the used item.
pub fn try_use_item(
    inv: &mut Inventory,
    index: usize,
    owner: Entity,
    position: Vec3,
    team: Team,
    triggers: &mut MessageWriter<AbilityTriggerEvent>,
) -> Option<ItemId> {
    let item = inv.slots.get_mut(index).and_then(|s| s.as_mut())?;
    let cooldown = item.id.active_cooldown()?;
    if item.cooldown_remaining > 0.0 {
        return None;
    }
    item.cooldown_remaining = cooldown;
    triggers.write(AbilityTriggerEvent(item_trigger(
        AbilityTrigger::OnUse,
        item.id,
        owner,
        team,
        position,
        None,
        position,
    )));
    Some(item.id)
}

fn item_trigger(
    trigger: AbilityTrigger,
    item: ItemId,
    owner: Entity,
    team: Team,
    origin: Vec3,
    trigger_unit: Option<Entity>,
    point: Vec3,
) -> TriggerContext {
    TriggerContext {
        trigger,
        caster: owner,
        team,
        source: EffectSource::Item(item),
        rank: 1,
        origin,
        aim: point,
        point,
        cast_target: None,
        trigger_unit,
        affected_units: Vec::new(),
    }
}

/// `OnAttack` (release) and `OnAttackHit` (impact) for every distinct item the
/// attacker owns that has entries for the trigger.
fn item_attack_triggers(
    mut releases: MessageReader<BasicAttackReleaseEvent>,
    mut impacts: MessageReader<BasicAttackImpactEvent>,
    owners: Query<(&Transform, &Team, &Inventory)>,
    targets: Query<&Transform>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    let fired = releases
        .read()
        .map(|e| (AbilityTrigger::OnAttack, e.attacker, e.target))
        .chain(impacts.read().map(|e| (AbilityTrigger::OnAttackHit, e.attacker, e.target)));
    for (trigger, attacker, target) in fired {
        let Ok((owner_tf, team, inventory)) = owners.get(attacker) else {
            continue;
        };
        let point = targets.get(target).map_or(owner_tf.translation, |tf| tf.translation);
        let mut seen: Vec<ItemId> = Vec::new();
        for item in inventory.slots.iter().flatten() {
            if seen.contains(&item.id)
                || item.id.definition().effects_for(trigger).next().is_none()
            {
                continue;
            }
            seen.push(item.id);
            triggers.write(AbilityTriggerEvent(item_trigger(
                trigger,
                item.id,
                attacker,
                *team,
                owner_tf.translation,
                Some(target),
                point,
            )));
        }
    }
}

/// Presentation only: a heal flash when the active affects its owner and a pulse ring
/// for each fixed-radius area it hits.
fn spawn_item_use_fx(commands: &mut Commands, assets: &SharedAssets, item: ItemId, at: Vec3) {
    for entry in item.definition().effects_for(AbilityTrigger::OnUse) {
        match entry.target {
            EffectTarget::Caster => spawn_heal_fx(commands, assets, at),
            EffectTarget::UnitsInRadius {
                radius: AreaRadius::Fixed(radius),
                ..
            } => {
                let radius = radius.at(1);
                commands.spawn((
                    Name::new(format!("{} Pulse", item.name())),
                    Mesh3d(assets.indicator_ring_mesh.clone()),
                    MeshMaterial3d(assets.shockwave_mat.clone()),
                    Transform::from_translation(at + Vec3::Y * scale::u(0.15))
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
            _ => {}
        }
    }
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

type ShopHeroQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Team,
        &'static mut PlayerWallet,
        &'static mut Inventory,
        &'static mut Health,
        &'static mut Mana,
        &'static mut CombatStats,
        &'static mut HeroAttributes,
    ),
    With<PlayerHero>,
>;

fn near_own_shop(hero_pos: Vec3, team: Team, shops: &Query<(&Transform, &ItemShop)>) -> bool {
    shops.iter().any(|(shop_tf, shop)| {
        shop.team == team && flat_distance(hero_pos, shop_tf.translation) <= shop.purchase_range
    })
}

fn try_purchase_from_ui(
    mut events: MessageReader<PurchaseItemRequest>,
    mut log: ResMut<ShopUndoLog>,
    mut hero: ShopHeroQuery,
    shops: Query<(&Transform, &ItemShop)>,
) {
    let Ok((hero_tf, team, mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs)) =
        hero.single_mut()
    else {
        return;
    };
    let in_range = near_own_shop(hero_tf.translation, *team, &shops);
    for PurchaseItemRequest { item } in events.read() {
        record_purchase(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            *item,
            in_range,
        );
    }
}

fn try_sell_from_ui(
    mut events: MessageReader<SellItemRequest>,
    mut log: ResMut<ShopUndoLog>,
    mut menu: ResMut<InventoryContextMenu>,
    mut hero: ShopHeroQuery,
    shops: Query<(&Transform, &ItemShop)>,
) {
    let Ok((hero_tf, team, mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs)) =
        hero.single_mut()
    else {
        return;
    };
    let in_range = near_own_shop(hero_tf.translation, *team, &shops);
    for SellItemRequest { slot } in events.read() {
        if record_sale(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            *slot,
            in_range,
        ) {
            menu.slot = None;
        }
    }
}

fn try_undo_from_ui(
    mut events: MessageReader<UndoShopRequest>,
    mut log: ResMut<ShopUndoLog>,
    mut hero: ShopHeroQuery,
    shops: Query<(&Transform, &ItemShop)>,
) {
    let Ok((hero_tf, team, mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs)) =
        hero.single_mut()
    else {
        for _ in events.read() {}
        return;
    };
    let in_range = near_own_shop(hero_tf.translation, *team, &shops);
    for _ in events.read() {
        if !in_range {
            continue;
        }
        try_undo_last(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
        );
    }
}

fn clear_shop_undo_outside_range(
    mut log: ResMut<ShopUndoLog>,
    hero: Query<(&Transform, &Team), With<PlayerHero>>,
    shops: Query<(&Transform, &ItemShop)>,
) {
    let in_range = hero.single().is_ok_and(|(hero_tf, team)| {
        near_own_shop(hero_tf.translation, *team, &shops)
    });
    expire_shop_undo(&mut log, in_range);
}

/// Buy one unit of `item` (stacking onto a partial stack when possible).
pub fn try_buy_item(
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
    item: ItemId,
    in_range: bool,
) -> bool {
    if !in_range || wallet.gold < item.cost() || inv.slot_for(item).is_none() {
        return false;
    }
    wallet.gold -= item.cost();
    apply_passives(item.passives(), health, mana, stats, attrs);
    if !inv.try_add(item) {
        wallet.gold = wallet.gold.saturating_add(item.cost());
        remove_passives(item.passives(), 1, health, mana, stats, attrs);
        return false;
    }
    combine_ready_items(inv, health, mana, stats, attrs);
    true
}

/// Sell a whole inventory slot for the item's sell value per unit. Must be in shop range.
pub fn try_sell_item(
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
    slot: usize,
    in_range: bool,
) -> bool {
    if !in_range {
        return false;
    }
    let Some(item) = inv.slots.get_mut(slot).and_then(|s| s.take()) else {
        return false;
    };
    remove_passives(item.id.passives(), item.stack, health, mana, stats, attrs);
    wallet.gold += item.id.definition().sell_value * item.stack;
    true
}

fn inventory_counts(inv: &Inventory) -> Vec<(ItemId, u32)> {
    let mut counts = Vec::new();
    for slot in inv.slots.iter().flatten() {
        if let Some((_, count)) = counts.iter_mut().find(|(id, _)| *id == slot.id) {
            *count += slot.stack;
        } else {
            counts.push((slot.id, slot.stack));
        }
    }
    counts
}

fn count_in(counts: &[(ItemId, u32)], id: ItemId) -> u32 {
    counts
        .iter()
        .find(|(item, _)| *item == id)
        .map(|(_, count)| *count)
        .unwrap_or(0)
}

fn count_diff(
    before: &[(ItemId, u32)],
    after: &[(ItemId, u32)],
) -> (Vec<(ItemId, u32)>, Vec<(ItemId, u32)>) {
    let mut ids = Vec::new();
    for (id, _) in before.iter().chain(after.iter()) {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for id in ids {
        let from = count_in(before, id);
        let to = count_in(after, id);
        if to > from {
            added.push((id, to - from));
        } else if from > to {
            removed.push((id, from - to));
        }
    }
    (removed, added)
}

/// Component multiset for `result`, or `None` when the item is not a build.
fn component_needs(result: ItemId) -> Option<Vec<(ItemId, u32)>> {
    let comps = result.recipe_components();
    if comps.is_empty() || comps.contains(&result) {
        return None;
    }
    let mut needs = Vec::new();
    for &comp in comps {
        if let Some((_, count)) = needs.iter_mut().find(|(id, _)| *id == comp) {
            *count += 1;
        } else {
            needs.push((comp, 1));
        }
    }
    Some(needs)
}

fn can_assemble(inv: &Inventory, result: ItemId, needs: &[(ItemId, u32)]) -> bool {
    if needs.iter().any(|&(id, n)| inv.count_of(id) < n) {
        return false;
    }
    let mut scratch = inv.clone();
    for &(id, n) in needs {
        for _ in 0..n {
            if !scratch.remove_one(id) {
                return false;
            }
        }
    }
    scratch.slot_for(result).is_some()
}

/// Combine every complete component set. Does not run during undo.
fn combine_ready_items(
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
) {
    for _ in 0..8 {
        let Some(result) = ItemId::all().iter().copied().find(|&result| {
            component_needs(result).is_some_and(|needs| can_assemble(inv, result, &needs))
        }) else {
            break;
        };
        let Some(needs) = component_needs(result) else {
            break;
        };
        for &(id, n) in &needs {
            remove_passives(id.passives(), n, health, mana, stats, attrs);
            for _ in 0..n {
                inv.remove_one(id);
            }
        }
        if inv.try_add(result) {
            apply_passives(result.passives(), health, mana, stats, attrs);
        } else {
            for &(id, n) in &needs {
                for _ in 0..n {
                    apply_passives(id.passives(), health, mana, stats, attrs);
                    inv.try_add(id);
                }
            }
            break;
        }
    }
}

fn record_purchase(
    log: &mut ShopUndoLog,
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
    item: ItemId,
    in_range: bool,
) -> bool {
    let before_gold = wallet.gold;
    let before = inventory_counts(inv);
    if !try_buy_item(wallet, inv, health, mana, stats, attrs, item, in_range) {
        return false;
    }
    let (removed, added) = count_diff(&before, &inventory_counts(inv));
    log.actions.push(ShopAction::Purchase {
        gold_spent: before_gold.saturating_sub(wallet.gold),
        removed,
        added,
    });
    log.notice.clear();
    true
}

fn record_sale(
    log: &mut ShopUndoLog,
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
    slot: usize,
    in_range: bool,
) -> bool {
    let Some(item) = inv.slots.get(slot).copied().flatten() else {
        return false;
    };
    let before_gold = wallet.gold;
    if !try_sell_item(wallet, inv, health, mana, stats, attrs, slot, in_range) {
        return false;
    }
    log.actions.push(ShopAction::Sale {
        refunded: wallet.gold.saturating_sub(before_gold),
        item,
    });
    log.notice.clear();
    true
}

fn undo_purchase(
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
    gold_spent: u32,
    removed: &[(ItemId, u32)],
    added: &[(ItemId, u32)],
) -> bool {
    if added.iter().any(|&(id, n)| inv.count_of(id) < n) {
        return false;
    }
    let mut scratch = inv.clone();
    for &(id, n) in added {
        for _ in 0..n {
            if !scratch.remove_one(id) {
                return false;
            }
        }
    }
    for &(id, n) in removed {
        for _ in 0..n {
            if !scratch.try_add(id) {
                return false;
            }
        }
    }
    for &(id, n) in added {
        remove_passives(id.passives(), n, health, mana, stats, attrs);
        for _ in 0..n {
            inv.remove_one(id);
        }
    }
    for &(id, n) in removed {
        for _ in 0..n {
            apply_passives(id.passives(), health, mana, stats, attrs);
            inv.try_add(id);
        }
    }
    wallet.gold = wallet.gold.saturating_add(gold_spent);
    true
}

fn undo_sale(
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
    refunded: u32,
    item: &ItemInstance,
) -> bool {
    if wallet.gold < refunded || inv.first_empty().is_none() {
        return false;
    }
    let Some(slot) = inv.first_empty() else {
        return false;
    };
    wallet.gold -= refunded;
    for _ in 0..item.stack {
        apply_passives(item.id.passives(), health, mana, stats, attrs);
    }
    inv.slots[slot] = Some(*item);
    true
}

/// Pop the latest shop transaction. Restored components are not combined again.
fn try_undo_last(
    log: &mut ShopUndoLog,
    wallet: &mut PlayerWallet,
    inv: &mut Inventory,
    health: &mut Health,
    mana: &mut Mana,
    stats: &mut CombatStats,
    attrs: &mut HeroAttributes,
) -> bool {
    let Some(action) = log.actions.last().cloned() else {
        return false;
    };
    let undone = match &action {
        ShopAction::Purchase {
            gold_spent,
            removed,
            added,
        } => undo_purchase(
            wallet, inv, health, mana, stats, attrs, *gold_spent, removed, added,
        ),
        ShopAction::Sale { refunded, item } => {
            undo_sale(wallet, inv, health, mana, stats, attrs, *refunded, item)
        }
    };
    if undone {
        log.actions.pop();
        log.notice.clear();
    } else {
        log.notice =
            "Undo failed — the items or gold from that trade are no longer available.".to_string();
    }
    undone
}

fn expire_shop_undo(log: &mut ShopUndoLog, in_range: bool) {
    if !in_range {
        log.clear_pending();
    }
}

#[derive(Message, Debug, Clone, Copy)]
pub struct UndoShopRequest;

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
        let mut attrs = HeroAttributes::starter();
        assert!(try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::IronBracer,
            true,
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
        let mut attrs = HeroAttributes::starter();
        assert!(!try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::SwiftBoots,
            true,
        ));
        wallet.gold = 500;
        assert!(!try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::SwiftBoots,
            false,
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
        let mut attrs = HeroAttributes::starter();
        assert!(try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::IronBracer,
            true,
        ));
        assert!(try_sell_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            0,
            true,
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

    #[test]
    fn recipes_hidden_from_shop_and_listed_in_builds() {
        assert!(ItemId::RecipeHeartwoodBand.is_recipe());
        assert!(ItemId::RecipeSparkPendant.is_recipe());
        assert!(!ItemId::shop_listed().any(|id| id.is_recipe()));
        assert!(ItemId::shop_listed().any(|id| id == ItemId::HeartwoodBand));
        let hb = ItemId::HeartwoodBand.recipe_components();
        assert!(hb.contains(&ItemId::IronBracer));
        assert!(hb.contains(&ItemId::RecipeHeartwoodBand));
        let sp = ItemId::SparkPendant.recipe_components();
        assert!(sp.contains(&ItemId::ManaCrystal));
        assert!(sp.contains(&ItemId::BladeOfAsh));
        assert!(sp.contains(&ItemId::RecipeSparkPendant));
    }

    #[test]
    fn item_data_comes_from_definitions() {
        let bracer = ItemId::IronBracer.passives();
        assert_eq!(bracer.max_health, 100.0);
        assert_eq!(bracer.armor, 4.0);
        assert_eq!(ItemId::BladeOfAsh.passives().attack_speed_flat, 15.0);
        assert!(ItemId::VialOfLight.has_active());
        assert_eq!(ItemId::VialOfLight.active_cooldown(), Some(40.0));
        assert!(!ItemId::IronBracer.has_active());
        assert_eq!(ItemId::IronBracer.active_cooldown(), None);
        assert_eq!(ItemId::SparkPendant.definition().sell_value, 375);
        for &id in ItemId::all() {
            assert_eq!(id.definition().id, id);
        }
    }

    #[test]
    fn attribute_passives_apply_and_revert() {
        let mut health = Health::new(720.0);
        let mut mana = Mana::new(320.0, 12.0);
        let mut stats = CombatStats::simple(55.0, 8.0, 1.1, 4.0, 3.0, 12.0);
        let mut attrs = HeroAttributes::starter();
        let strength = attrs.strength;
        let passives = ItemPassives {
            strength: 10.0,
            attack_speed_flat: 20.0,
            ..default()
        };
        apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
        assert_eq!(attrs.strength, strength + 10.0);
        assert_eq!(health.max, 720.0 + 10.0 * HeroAttributes::HP_PER_STR);
        assert_eq!(stats.attack_speed_flat, 20.0);
        remove_passives(passives, 1, &mut health, &mut mana, &mut stats, &mut attrs);
        assert_eq!(attrs.strength, strength);
        assert_eq!(health.max, 720.0);
        assert_eq!(stats.attack_speed_flat, 0.0);
    }

    #[test]
    fn consumables_spend_charges_then_units() {
        let mut item = ItemInstance {
            stack: 2,
            charges: Some(2),
            ..ItemInstance::new(ItemId::VialOfLight)
        };
        assert_eq!(item.consume(Some(2)), 0);
        assert_eq!(item.consume(Some(2)), 1);
        assert_eq!((item.stack, item.charges), (1, Some(2)));
        let mut plain = ItemInstance::new(ItemId::VialOfLight);
        assert_eq!(plain.consume(None), 1);
        assert_eq!(plain.stack, 0);
    }

    #[test]
    fn tooltips_list_price_stats_and_effects() {
        let bracer = ItemId::IronBracer.tooltip_body();
        assert!(bracer.contains("Buy: 300g    Sell: 150g"), "{bracer}");
        assert!(bracer.contains("Armor"));
        assert!(bracer.contains("+100 Health"));
        assert!(bracer.contains("+4 Armor"));
        assert!(bracer.contains("+100 HP, +4 Armor"));

        let vial = ItemId::VialOfLight.tooltip_body();
        assert!(vial.contains("Use: Heal 180"), "{vial}");
        assert!(vial.contains("Cooldown: 40s"));
    }

    #[test]
    fn shops_sit_beside_their_ancients() {
        let radiant = scale::ground(-48.0, -48.0);
        let shop = shop_world_position(Team::Radiant);
        let dist = Vec3::new(shop.x - radiant.x, 0.0, shop.z - radiant.z).length();
        assert!(dist > 80.0 && dist < scale::map(10.0), "{dist}");

        let dire = scale::ground(48.0, 48.0);
        let shop = shop_world_position(Team::Dire);
        let dist = Vec3::new(shop.x - dire.x, 0.0, shop.z - dire.z).length();
        assert!(dist > 80.0 && dist < scale::map(10.0), "{dist}");
        assert!(shop_purchase_range() > 200.0);
    }

    fn fresh_hero() -> (
        PlayerWallet,
        Inventory,
        Health,
        Mana,
        CombatStats,
        HeroAttributes,
    ) {
        (
            PlayerWallet { gold: 2000 },
            Inventory::empty(),
            Health::new(720.0),
            Mana::new(320.0, 12.0),
            CombatStats::simple(55.0, 8.0, 1.1, 4.0, 3.0, 12.0),
            HeroAttributes::starter(),
        )
    }

    fn owned(inv: &Inventory) -> Vec<ItemId> {
        inv.slots.iter().flatten().map(|item| item.id).collect()
    }

    #[test]
    fn item_types_include_attributes_equipment_misc_and_accessories() {
        assert_eq!(ItemType::Attributes.label(), "Attributes");
        assert_eq!(ItemType::Equipment.label(), "Equipment");
        assert_eq!(ItemType::Miscellaneous.label(), "Miscellaneous");
        assert_eq!(ItemType::Accessories.label(), "Accessories");
        assert_eq!(ItemId::IronBracer.definition().item_type, ItemType::Equipment);
        assert_eq!(ItemId::SwiftBoots.definition().item_type, ItemType::Equipment);
        assert_eq!(ItemId::VialOfLight.definition().item_type, ItemType::Miscellaneous);
        assert_eq!(ItemId::ManaCrystal.definition().item_type, ItemType::Accessories);
        assert_eq!(ItemId::HeartwoodBand.definition().item_type, ItemType::Accessories);
        assert_eq!(ItemId::SparkPendant.definition().item_type, ItemType::Accessories);
        assert_eq!(ItemId::RecipeHeartwoodBand.definition().item_type, ItemType::Recipe);
    }

    #[test]
    fn buying_the_last_component_combines_and_undo_refunds_only_that_purchase() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let mut log = ShopUndoLog::default();
        assert!(record_purchase(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
            ItemId::IronBracer, true,
        ));
        assert_eq!(owned(&inv), vec![ItemId::IronBracer]);
        assert_eq!(health.max, 820.0);
        assert_eq!(stats.armor, 8.0);

        assert!(record_purchase(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
            ItemId::RecipeHeartwoodBand, true,
        ));
        assert_eq!(owned(&inv), vec![ItemId::HeartwoodBand]);
        assert!(!owned(&inv).contains(&ItemId::RecipeHeartwoodBand));
        assert_eq!(wallet.gold, 2000 - ItemId::IronBracer.cost() - ItemId::RecipeHeartwoodBand.cost());
        assert_eq!(health.max, 870.0);
        assert_eq!(stats.armor, 7.0);
        assert_eq!(mana.regen_per_sec, 14.0);
        assert_eq!(log.pending(), 2);

        assert!(try_undo_last(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
        ));
        assert_eq!(owned(&inv), vec![ItemId::IronBracer]);
        assert!(!owned(&inv).contains(&ItemId::HeartwoodBand));
        assert!(!owned(&inv).contains(&ItemId::RecipeHeartwoodBand));
        assert_eq!(wallet.gold, 2000 - ItemId::IronBracer.cost());
        assert_eq!(health.max, 820.0);
        assert_eq!(stats.armor, 8.0);
        assert_eq!(mana.regen_per_sec, 12.0);
        assert_eq!(log.pending(), 1);

        assert!(try_undo_last(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
        ));
        assert!(owned(&inv).is_empty());
        assert_eq!(wallet.gold, 2000);
        assert_eq!(health.max, 720.0);
        assert_eq!(stats.armor, 4.0);
        assert_eq!(log.pending(), 0);
    }

    #[test]
    fn undo_reverses_a_plain_buy_and_a_sale_without_recombining() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let mut log = ShopUndoLog::default();
        assert!(record_purchase(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
            ItemId::SwiftBoots, true,
        ));
        assert!(try_undo_last(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
        ));
        assert!(owned(&inv).is_empty());
        assert_eq!(wallet.gold, 2000);
        assert_eq!(stats.move_speed, 12.0);

        assert!(record_purchase(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
            ItemId::IronBracer, true,
        ));
        inv.slots[0].as_mut().unwrap().cooldown_remaining = 3.5;
        assert!(record_sale(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
            0, true,
        ));
        assert!(owned(&inv).is_empty());
        assert_eq!(wallet.gold, 2000 - ItemId::IronBracer.cost() + ItemId::IronBracer.definition().sell_value);
        assert!(try_undo_last(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
        ));
        assert_eq!(owned(&inv), vec![ItemId::IronBracer]);
        assert_eq!(inv.slots.iter().flatten().next().unwrap().cooldown_remaining, 3.5);
        assert_eq!(wallet.gold, 2000 - ItemId::IronBracer.cost());
        assert_eq!(health.max, 820.0);
    }

    #[test]
    fn undo_refuses_when_the_combined_item_is_gone_and_leaving_range_locks_trades() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let mut log = ShopUndoLog::default();
        assert!(record_purchase(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
            ItemId::IronBracer, true,
        ));
        assert!(record_purchase(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
            ItemId::RecipeHeartwoodBand, true,
        ));
        let gold = wallet.gold;
        inv.slots = [None, None, None, None, None, None];
        assert!(!try_undo_last(
            &mut log, &mut wallet, &mut inv, &mut health, &mut mana, &mut stats, &mut attrs,
        ));
        assert_eq!(wallet.gold, gold);
        assert_eq!(log.pending(), 2);
        assert!(!log.notice.is_empty());

        expire_shop_undo(&mut log, false);
        assert_eq!(log.pending(), 0);
        assert!(log.notice.is_empty());
        expire_shop_undo(&mut log, true);
    }
}
