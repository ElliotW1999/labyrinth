//! Item catalog, shop, inventory, active use (ASDZXC), and status effects.

use bevy::prelude::*;

use crate::abilities::definition::{
    AbilityEffect, AbilityEffectEntry, AbilityMechanic, AbilityTrigger, AreaRadius, Attribute,
    EffectTarget, PassiveStat,
};
use crate::abilities::effects::{AbilityTriggerEvent, EffectSource, TriggerContext};
use crate::abilities::tooltip;
use crate::actions::{
    ActionCooldown, ActionCost, ActionFiredEvent, ActionId, ActionSpec, DisableRule, TargetTeam,
    TargetType,
};
use crate::combat::flat_distance;
use crate::components::{
    CastTarget, CombatStats, Health, HeroAttributes, Lifetime, Mana, PlayerHero, PlayerWallet,
    SpellFx, Team,
};
use crate::resources::SharedAssets;
use crate::scale;
use crate::vitals::MaxChangeRule;

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
                Update,
                spawn_shops
                    .in_set(crate::map::MapGenSet)
                    .run_if(crate::map::map_requested),
            )
            .add_systems(
                Update,
                (
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
    /// Which category page the shop grid is showing.
    pub tab: ShopTab,
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
        self.try_add_at(id).is_some()
    }

    /// Like [`Self::try_add`], returning the slot that received the unit.
    pub fn try_add_at(&mut self, id: ItemId) -> Option<usize> {
        let i = self.slot_for(id)?;
        match self.slots[i].as_mut() {
            Some(item) => item.stack += 1,
            None => self.slots[i] = Some(ItemInstance::new(id)),
        }
        Some(i)
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

    /// Slot currently holding `instance`.
    pub fn slot_of(&self, instance: ItemInstanceId) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.is_some_and(|item| item.instance == instance))
    }
}

/// Identity of one inventory stack, unique for the whole run. Each copy of an item
/// keeps its own cooldown and charges, and in-flight activations follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemInstanceId(u64);

impl ItemInstanceId {
    pub fn next() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ItemInstance {
    pub id: ItemId,
    pub instance: ItemInstanceId,
    /// Cooldown and, for consumables with `max_charges`, the charges left on the
    /// current unit. Item charges never regenerate; the next unit starts full.
    pub cooldown: ActionCooldown,
    /// Units in this slot (stackable items).
    pub stack: u32,
    /// Units used up at commit whose passives `resolve_item_uses` has not removed yet.
    pub spent_units: u32,
    /// The attribute an `attribute_toggle` item currently grants (`None` for other items).
    pub selected_attribute: Option<Attribute>,
}

impl ItemInstance {
    pub fn new(id: ItemId) -> Self {
        let def = id.definition();
        Self {
            id,
            instance: ItemInstanceId::next(),
            cooldown: ActionCooldown {
                charges: def.max_charges.filter(|_| def.consumable),
                ..ActionCooldown::default()
            },
            stack: 1,
            spent_units: 0,
            selected_attribute: def.attribute_toggle.map(|_| Attribute::Strength),
        }
    }
}

impl ItemInstance {
    /// "Strength" / "Agility" / "Intelligence" for a toggle item's current selection.
    pub fn selected_attribute_name(&self) -> Option<&'static str> {
        self.selected_attribute.map(tooltip::attribute_name)
    }
}

/// The next attribute a toggle item switches to: Strength -> Agility -> Intelligence -> Strength.
pub fn next_attribute(attribute: Attribute) -> Attribute {
    match attribute {
        Attribute::Strength => Attribute::Agility,
        Attribute::Agility => Attribute::Intelligence,
        Attribute::Intelligence => Attribute::Strength,
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
    GauntletsOfStrength,
    SlippersOfAgility,
    MantleOfIntelligence,
    Circlet,
    BeltOfStrength,
    BandOfElvenskin,
    RobeOfTheMagi,
    Crown,
    OgreAxe,
    BladeOfAlacrity,
    StaffOfWizardry,
    Diadem,
    RingOfProtection,
    BladesOfAttack,
    GlovesOfHaste,
    Chainmail,
    HelmOfIronWill,
    Broadsword,
    BlitzKnuckles,
    Claymore,
    MithrilHammer,
    RingOfRegen,
    SagesMask,
    FluffyHat,
    WindLace,
    Cloak,
    BootsOfSpeed,
    RingOfHealth,
    VoidStone,
    VitalityBooster,
    PointBooster,
    Platemail,
    Hyperstone,
    UltimateOrb,
    DemonEdge,
    MysticStaff,
    Reaver,
    Eaglesong,
    SacredRelic,
    NullTalisman,
    RecipeNull,
    WraithBand,
    RecipeWraith,
    Bracer,
    RecipeBracer,
    FalconBlade,
    RecipeFalcon,
    BlinkDagger,
    VoodooMask,
    MorbidMask,
    GhostScepter,
    SoulRing,
    RecipeSoulRing,
    PowerTreads,
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
    Support,
    Magical,
    Weapons,
    Artifacts,
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
            ItemType::Support => "Support",
            ItemType::Magical => "Magical",
            ItemType::Weapons => "Weapons",
            ItemType::Artifacts => "Artifacts",
        }
    }
}

/// Shop grid pages. Recipes stay off both tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShopTab {
    #[default]
    Basics,
    Upgrades,
}

impl ShopTab {
    pub fn label(self) -> &'static str {
        match self {
            ShopTab::Basics => "Basics",
            ShopTab::Upgrades => "Upgrades",
        }
    }

    pub fn all() -> [ShopTab; 2] {
        [ShopTab::Basics, ShopTab::Upgrades]
    }

    pub fn accepts(self, item_type: ItemType) -> bool {
        match self {
            ShopTab::Basics => matches!(
                item_type,
                ItemType::Attributes | ItemType::Equipment | ItemType::Miscellaneous
            ),
            ShopTab::Upgrades => matches!(
                item_type,
                ItemType::Accessories
                    | ItemType::Support
                    | ItemType::Magical
                    | ItemType::Armor
                    | ItemType::Weapons
                    | ItemType::Artifacts
                    | ItemType::Utility
            ),
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
    /// Targeting, timing, and cost of the active (ignored without `OnUse` effects).
    pub activation: ItemActivation,
    /// Points of the instance's selected attribute the item grants (Power Treads). The
    /// active switches the selection Strength -> Agility -> Intelligence, instantly and
    /// without a cooldown; the bonus is reconciled by [`sync_item_attribute_toggles`].
    pub attribute_toggle: Option<f32>,
    /// Recipe slots: each slot lists the alternatives that can fill it, in preference order
    /// (the first one the hero owns is consumed). Empty = basic item.
    pub components: &'static [&'static [ItemId]],
    pub color: [f32; 3],
    pub description: &'static str,
    pub effects: &'static [AbilityEffectEntry],
    /// How the active plays out in the world (a projectile, …): the same mechanics abilities
    /// use, run when the item fires. Their triggers resolve against `effects`.
    pub mechanics: &'static [AbilityMechanic],
}

/// How an active item is aimed and cast; the shared `actions` pipeline enforces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemActivation {
    pub target_type: TargetType,
    pub target_team: TargetTeam,
    pub cast_range: f32,
    pub cast_time: f32,
    pub channel_time: f32,
    pub mana_cost: f32,
}

impl ItemActivation {
    /// No target, no cast point, free: fires on the key press.
    pub const INSTANT: Self = Self {
        target_type: TargetType::NoTarget,
        target_team: TargetTeam::SelfOnly,
        cast_range: 0.0,
        cast_time: 0.0,
        channel_time: 0.0,
        mana_cost: 0.0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemUseError {
    EmptySlot,
    NoActive,
}

/// [`ActionSpec`] for the active item in `slot`: the item layer's half of the shared
/// activation pipeline (ownership and availability here; readiness, targeting, range,
/// and timing in `actions`).
pub fn item_action_spec(inventory: &Inventory, slot: usize) -> Result<ActionSpec, ItemUseError> {
    let item = inventory
        .slots
        .get(slot)
        .copied()
        .flatten()
        .filter(|item| item.stack > 0)
        .ok_or(ItemUseError::EmptySlot)?;
    if !item.id.has_active() {
        return Err(ItemUseError::NoActive);
    }
    let def = item.id.definition();
    let activation = def.activation;
    Ok(ActionSpec {
        action: ActionId::Item {
            item: item.id,
            instance: item.instance,
        },
        level: 1,
        target_type: activation.target_type,
        target_team: activation.target_team,
        cast_range: activation.cast_range,
        cast_range_buffer: 0.0,
        cast_time: activation.cast_time,
        channel_time: activation.channel_time,
        backswing: 0.0,
        cost: ActionCost {
            mana: activation.mana_cost,
        },
        cooldown: def.cooldown,
        cooldown_state: item.cooldown,
        disable: DisableRule::MuteOrStun,
        occupies_caster: activation.cast_time > 0.0 || activation.channel_time > 0.0,
    })
}

/// Start the item's cooldown and spend its charge, and for a consumable whose last charge
/// went, one unit of the stack. Spending at commit means a second request in the same
/// frame already sees the emptied slot. `resolve_item_uses` removes the passives of the
/// spent units (`spent_units`) and clears an emptied slot.
pub fn commit_item(inventory: &mut Inventory, slot: usize, spec: &ActionSpec) {
    let Some(item) = inventory.slots.get_mut(slot).and_then(|s| s.as_mut()) else {
        return;
    };
    item.cooldown.start(spec.cooldown);
    let def = item.id.definition();
    if def.consumable && item.cooldown.charges.is_none_or(|charges| charges == 0) {
        item.stack = item.stack.saturating_sub(1);
        item.spent_units += 1;
        item.cooldown.charges = def.max_charges;
    }
}

impl ItemDefinition {
    /// Radius of the item's fixed-size areas: the largest `AreaRadius::Fixed` among its
    /// `UnitsInRadius` entries that fire from the active (`OnUse` / `OnChannelEnd` /
    /// `OnChannelInterrupted`, and the projectile's `OnProjectileHit` / `OnImpact`).
    /// `0.0` when it has none. Used for the targeting indicator and as the
    /// `AreaRadius::Ability` fallback.
    pub fn aoe_radius(&self) -> f32 {
        self.effects
            .iter()
            .filter(|entry| {
                matches!(
                    entry.trigger,
                    AbilityTrigger::OnUse
                        | AbilityTrigger::OnChannelEnd
                        | AbilityTrigger::OnChannelInterrupted
                        | AbilityTrigger::OnProjectileHit
                        | AbilityTrigger::OnImpact
                )
            })
            .filter_map(|entry| match entry.target {
                EffectTarget::UnitsInRadius {
                    radius: AreaRadius::Fixed(radius),
                    ..
                } => Some(radius.at(1)),
                _ => None,
            })
            .fold(0.0, f32::max)
    }

    pub fn effects_for(
        &self,
        trigger: AbilityTrigger,
    ) -> impl Iterator<Item = &AbilityEffectEntry> {
        self.effects
            .iter()
            .filter(move |entry| entry.trigger == trigger)
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
            ItemId::GauntletsOfStrength,
            ItemId::SlippersOfAgility,
            ItemId::MantleOfIntelligence,
            ItemId::Circlet,
            ItemId::BeltOfStrength,
            ItemId::BandOfElvenskin,
            ItemId::RobeOfTheMagi,
            ItemId::Crown,
            ItemId::OgreAxe,
            ItemId::BladeOfAlacrity,
            ItemId::StaffOfWizardry,
            ItemId::Diadem,
            ItemId::RingOfProtection,
            ItemId::BladesOfAttack,
            ItemId::GlovesOfHaste,
            ItemId::Chainmail,
            ItemId::HelmOfIronWill,
            ItemId::Broadsword,
            ItemId::BlitzKnuckles,
            ItemId::Claymore,
            ItemId::MithrilHammer,
            ItemId::RingOfRegen,
            ItemId::SagesMask,
            ItemId::FluffyHat,
            ItemId::WindLace,
            ItemId::Cloak,
            ItemId::BootsOfSpeed,
            ItemId::RingOfHealth,
            ItemId::VoidStone,
            ItemId::VitalityBooster,
            ItemId::PointBooster,
            ItemId::Platemail,
            ItemId::Hyperstone,
            ItemId::UltimateOrb,
            ItemId::DemonEdge,
            ItemId::MysticStaff,
            ItemId::Reaver,
            ItemId::Eaglesong,
            ItemId::SacredRelic,
            ItemId::NullTalisman,
            ItemId::RecipeNull,
            ItemId::WraithBand,
            ItemId::RecipeWraith,
            ItemId::Bracer,
            ItemId::RecipeBracer,
            ItemId::FalconBlade,
            ItemId::RecipeFalcon,
            ItemId::BlinkDagger,
            ItemId::VoodooMask,
            ItemId::MorbidMask,
            ItemId::GhostScepter,
            ItemId::SoulRing,
            ItemId::RecipeSoulRing,
            ItemId::PowerTreads,
            // </item_generator:item_all>
        ]
    }

    pub fn definition(self) -> &'static ItemDefinition {
        #[cfg(test)]
        if let Some(def) = definition_override(self) {
            return def;
        }
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
        let mut effects: Vec<String> = def.effects.iter().flat_map(item_effect_lines).collect();
        if let Some(points) = def.attribute_toggle {
            effects.push(format!(
                "Use: switch the +{} attribute between Strength, Agility and Intelligence",
                tooltip::fmt_num(points)
            ));
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
            for slot in comps {
                body.push_str(&format!("\n• {}", component_slot_label(slot)));
            }
        }
        body
    }

    pub fn placeholder_color(self) -> Color {
        let [r, g, b] = self.definition().color;
        Color::srgb(r, g, b)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn active_cooldown(self) -> Option<f32> {
        self.has_active().then(|| self.definition().cooldown)
    }

    /// Build tree slots (other items and/or a recipe scrap), each with its alternatives.
    /// Empty = basic item.
    pub fn recipe_components(self) -> &'static [&'static [ItemId]] {
        self.definition().components
    }

    pub fn has_active(self) -> bool {
        let def = self.definition();
        def.attribute_toggle.is_some()
            || def.effects_for(AbilityTrigger::OnUse).next().is_some()
            || !def.mechanics.is_empty()
            || (def.activation.channel_time > 0.0
                && (def
                    .effects_for(AbilityTrigger::OnChannelEnd)
                    .next()
                    .is_some()
                    || def
                        .effects_for(AbilityTrigger::OnChannelInterrupted)
                        .next()
                        .is_some()))
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
    pub spell_amp: f32,
    pub outgoing_physical_amp: f32,
    pub incoming_damage_amp: f32,
    pub damage_block: f32,
    pub damage_reflect: f32,
    pub heal_amp: f32,
    pub heal_received_amp: f32,
    pub lifesteal: f32,
    pub spell_lifesteal: f32,
    pub mana_cost_amp: f32,
    pub crit_chance: f32,
    pub crit_multiplier: f32,
    pub evasion: f32,
    pub accuracy: f32,
    pub cleave_ratio: f32,
    pub splash_ratio: f32,
    pub attack_range: f32,
    pub attack_projectile_speed: f32,
}

impl ItemPassives {
    /// Add `amount` to the field `stat` maps to. This is the one place that maps a
    /// [`PassiveStat`] to a bonus field; items and passive abilities both use it.
    pub fn add_stat(&mut self, stat: PassiveStat, amount: f32) {
        let field = match stat {
            PassiveStat::MaxHealth => &mut self.max_health,
            PassiveStat::HealthRegen => &mut self.health_regen,
            PassiveStat::MaxMana => &mut self.max_mana,
            PassiveStat::ManaRegen => &mut self.mana_regen,
            PassiveStat::AttackDamage => &mut self.attack_damage,
            PassiveStat::AttackSpeed => &mut self.attack_speed_flat,
            PassiveStat::Armor => &mut self.armor,
            PassiveStat::MagicResist => &mut self.magic_resist,
            PassiveStat::MoveSpeed => &mut self.move_speed,
            PassiveStat::Attribute(Attribute::Strength) => &mut self.strength,
            PassiveStat::Attribute(Attribute::Agility) => &mut self.agility,
            PassiveStat::Attribute(Attribute::Intelligence) => &mut self.intelligence,
            PassiveStat::SpellAmp => &mut self.spell_amp,
            PassiveStat::OutgoingPhysicalAmp => &mut self.outgoing_physical_amp,
            PassiveStat::IncomingDamageAmp => &mut self.incoming_damage_amp,
            PassiveStat::DamageBlock => &mut self.damage_block,
            PassiveStat::DamageReflect => &mut self.damage_reflect,
            PassiveStat::HealAmp => &mut self.heal_amp,
            PassiveStat::HealReceivedAmp => &mut self.heal_received_amp,
            PassiveStat::Lifesteal => &mut self.lifesteal,
            PassiveStat::SpellLifesteal => &mut self.spell_lifesteal,
            PassiveStat::ManaCostAmp => &mut self.mana_cost_amp,
            PassiveStat::CritChance => &mut self.crit_chance,
            PassiveStat::CritMultiplier => &mut self.crit_multiplier,
            PassiveStat::Evasion => &mut self.evasion,
            PassiveStat::Accuracy => &mut self.accuracy,
            PassiveStat::CleaveRatio => &mut self.cleave_ratio,
            PassiveStat::SplashRatio => &mut self.splash_ratio,
            PassiveStat::AttackRange => &mut self.attack_range,
            PassiveStat::AttackProjectileSpeed => &mut self.attack_projectile_speed,
        };
        *field += amount;
    }
}

impl std::ops::AddAssign for ItemPassives {
    fn add_assign(&mut self, rhs: Self) {
        // Destructured so a new field is a compile error here until it is summed.
        let Self {
            max_health,
            health_regen,
            max_mana,
            mana_regen,
            attack_damage,
            armor,
            magic_resist,
            move_speed,
            attack_speed_flat,
            strength,
            agility,
            intelligence,
            spell_amp,
            outgoing_physical_amp,
            incoming_damage_amp,
            damage_block,
            damage_reflect,
            heal_amp,
            heal_received_amp,
            lifesteal,
            spell_lifesteal,
            mana_cost_amp,
            crit_chance,
            crit_multiplier,
            evasion,
            accuracy,
            cleave_ratio,
            splash_ratio,
            attack_range,
            attack_projectile_speed,
        } = rhs;
        self.max_health += max_health;
        self.health_regen += health_regen;
        self.max_mana += max_mana;
        self.mana_regen += mana_regen;
        self.attack_damage += attack_damage;
        self.armor += armor;
        self.magic_resist += magic_resist;
        self.move_speed += move_speed;
        self.attack_speed_flat += attack_speed_flat;
        self.strength += strength;
        self.agility += agility;
        self.intelligence += intelligence;
        self.spell_amp += spell_amp;
        self.outgoing_physical_amp += outgoing_physical_amp;
        self.incoming_damage_amp += incoming_damage_amp;
        self.damage_block += damage_block;
        self.damage_reflect += damage_reflect;
        self.heal_amp += heal_amp;
        self.heal_received_amp += heal_received_amp;
        self.lifesteal += lifesteal;
        self.spell_lifesteal += spell_lifesteal;
        self.mana_cost_amp += mana_cost_amp;
        self.crit_chance += crit_chance;
        self.crit_multiplier += crit_multiplier;
        self.evasion += evasion;
        self.accuracy += accuracy;
        self.cleave_ratio += cleave_ratio;
        self.splash_ratio += splash_ratio;
        self.attack_range += attack_range;
        self.attack_projectile_speed += attack_projectile_speed;
    }
}

impl ItemId {
    /// Sum of the item's `Passive` stat entries.
    pub fn passives(self) -> ItemPassives {
        let mut p = ItemPassives::default();
        for entry in self.definition().effects_for(AbilityTrigger::Passive) {
            if let AbilityEffect::PassiveStat { stat, amount } = entry.effect {
                p.add_stat(stat, amount.at(1));
            }
        }
        p
    }
}

fn passive_stat_lines(passives: &ItemPassives) -> Vec<String> {
    let rows = [
        (passives.max_health, PassiveStat::MaxHealth),
        (passives.health_regen, PassiveStat::HealthRegen),
        (passives.max_mana, PassiveStat::MaxMana),
        (passives.mana_regen, PassiveStat::ManaRegen),
        (passives.attack_damage, PassiveStat::AttackDamage),
        (passives.attack_speed_flat, PassiveStat::AttackSpeed),
        (passives.armor, PassiveStat::Armor),
        (passives.magic_resist, PassiveStat::MagicResist),
        (passives.move_speed, PassiveStat::MoveSpeed),
        (
            passives.strength,
            PassiveStat::Attribute(Attribute::Strength),
        ),
        (passives.agility, PassiveStat::Attribute(Attribute::Agility)),
        (
            passives.intelligence,
            PassiveStat::Attribute(Attribute::Intelligence),
        ),
    ];
    let mut lines: Vec<String> = rows
        .into_iter()
        .filter(|(value, _)| value.abs() >= 0.05)
        .map(|(value, stat)| format!("{:+.0} {}", value, tooltip::passive_name(stat)))
        .collect();
    // Lifesteal stats are fractions; show them as percentages like the descriptions do.
    let percent_rows = [
        (passives.lifesteal, PassiveStat::Lifesteal),
        (passives.spell_lifesteal, PassiveStat::SpellLifesteal),
    ];
    lines.extend(
        percent_rows
            .into_iter()
            .filter(|(value, _)| value.abs() >= 0.0005)
            .map(|(value, stat)| format!("{:+.0}% {}", value * 100.0, tooltip::passive_name(stat))),
    );
    lines
}

/// One recipe slot for tooltips and the shop: "A / B / C (450g)", priced at the first
/// alternative (alternatives are meant to cost the same).
pub fn component_slot_label(slot: &[ItemId]) -> String {
    let names: Vec<&str> = slot.iter().map(|id| id.name()).collect();
    let cost = slot.first().map_or(0, |id| id.cost());
    format!("{} ({}g)", names.join(" / "), cost)
}

/// Tooltip lines for one non-passive effect entry, from the same describer abilities
/// use: `"{Tag}: {label} {value}"` per row, then the notes.
fn item_effect_lines(entry: &AbilityEffectEntry) -> Vec<String> {
    if entry.trigger == AbilityTrigger::Passive {
        return Vec::new();
    }
    let tag = capitalized(tooltip::trigger_tag(entry.trigger));
    let (rows, notes) = tooltip::describe_effect(entry);
    let area = tooltip::area_note(&entry.target);
    let mut lines: Vec<String> = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let mut line = match row.value {
                Some(value) => {
                    let unit = if row.seconds { "s" } else { "" };
                    format!(
                        "{tag}: {} {}{unit}",
                        row.label,
                        tooltip::fmt_num(value.at(1))
                    )
                }
                None => format!("{tag}: {}", row.label),
            };
            // The area belongs to the effect, so it rides on the effect's first line.
            if let (0, Some(area)) = (index, &area) {
                line.push_str(&format!(" ({area})"));
            }
            line
        })
        .collect();
    lines.extend(notes.into_iter().map(|note| format!("{tag}: {note}")));
    lines
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
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

    health.change_max(passives.max_health * count, MaxChangeRule::KeepMissing);
    health.regen_per_sec = (health.regen_per_sec + passives.health_regen * count).max(0.0);
    mana.change_max(passives.max_mana * count, MaxChangeRule::KeepMissing);
    mana.regen_per_sec = (mana.regen_per_sec + passives.mana_regen * count).max(0.0);
    stats.attack_damage += passives.attack_damage * count;
    stats.armor += passives.armor * count;
    stats.magic_resist += passives.magic_resist * count;
    stats.move_speed += passives.move_speed * count;
    stats.attack_speed_flat += passives.attack_speed_flat * count;
    stats.outgoing_spell_amp += passives.spell_amp * count;
    stats.outgoing_physical_amp += passives.outgoing_physical_amp * count;
    stats.incoming_damage_amp += passives.incoming_damage_amp * count;
    stats.damage_block += passives.damage_block * count;
    stats.damage_reflect += passives.damage_reflect * count;
    stats.heal_amp += passives.heal_amp * count;
    stats.heal_received_amp += passives.heal_received_amp * count;
    stats.lifesteal += passives.lifesteal * count;
    stats.spell_lifesteal += passives.spell_lifesteal * count;
    stats.mana_cost_amp += passives.mana_cost_amp * count;
    stats.crit_chance += passives.crit_chance * count;
    stats.crit_multiplier += passives.crit_multiplier * count;
    stats.evasion += passives.evasion * count;
    stats.accuracy += passives.accuracy * count;
    if passives.cleave_ratio != 0.0 {
        // Item cleaves stack additively in one instance with the default circle.
        stats.cleaves.adjust(
            crate::components::CleaveSource::Items,
            passives.cleave_ratio * count,
            0.0,
            0.0,
            0.0,
            count.round() as i32,
        );
    }
    stats.splash_damage_ratio += passives.splash_ratio * count;
    stats.attack_range += passives.attack_range * count;
    stats.attack_range_bonus += passives.attack_range * count;
    stats.attack_projectile_speed_bonus += passives.attack_projectile_speed * count;
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

/// Ticks in the `AbilitiesPlugin` chain next to the ability cooldowns, so both sources
/// share the authority gate. Charges do not regenerate.
pub(crate) fn tick_item_cooldowns(
    time: Res<Time>,
    mut inventories: Query<(Entity, &mut Inventory)>,
    mut ready: MessageWriter<crate::abilities::reactive_triggers::CooldownReadyEvent>,
) {
    let dt = time.delta_secs();
    for (owner, mut inv) in &mut inventories {
        for item in inv.slots.iter_mut().flatten() {
            if item.cooldown.tick(dt) {
                ready.write(crate::abilities::reactive_triggers::CooldownReadyEvent {
                    owner,
                    source: crate::abilities::effects::EffectSource::Item(item.id),
                });
            }
        }
    }
}

/// Item layer of the shared `actions` pipeline: once an item activation fires (cooldown
/// already started, mana paid), spawn its feedback, fire `OnUse`, and spend a charge or
/// unit of consumables. Finds the copy by instance, so it follows a moved stack.
#[allow(clippy::type_complexity)]
pub fn resolve_item_uses(
    mut fired: MessageReader<ActionFiredEvent>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut owners: Query<(
        &Transform,
        &Team,
        &mut Inventory,
        &mut Health,
        &mut Mana,
        &mut CombatStats,
        &mut HeroAttributes,
    )>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for event in fired.read() {
        let ActionId::Item { item, instance } = event.action else {
            continue;
        };
        let Ok((transform, team, mut inv, mut health, mut mana, mut stats, mut attrs)) =
            owners.get_mut(event.caster)
        else {
            continue;
        };
        // An attribute toggle only switches the instance's selection (reconciled by
        // `sync_item_attribute_toggles`): no feedback and no `OnUse` trigger.
        if item.definition().attribute_toggle.is_some() {
            if let Some(slot) = inv
                .slot_of(instance)
                .and_then(|index| inv.slots[index].as_mut())
            {
                let current = slot.selected_attribute.unwrap_or(Attribute::Strength);
                slot.selected_attribute = Some(next_attribute(current));
            }
            continue;
        }
        let position = transform.translation;
        spawn_item_use_fx(&mut commands, &assets, item, position);
        triggers.write(AbilityTriggerEvent(item_use_trigger(
            item,
            event.caster,
            *team,
            position,
            event.aim,
            event.target,
        )));

        let def = item.definition();
        let Some(index) = inv.slot_of(instance).filter(|_| def.consumable) else {
            continue;
        };
        let Some(slot) = inv.slots[index].as_mut() else {
            continue;
        };
        let spent = std::mem::take(&mut slot.spent_units);
        if spent > 0 {
            remove_passives(
                item.passives(),
                spent,
                &mut health,
                &mut mana,
                &mut stats,
                &mut attrs,
            );
        }
        if slot.stack == 0 {
            inv.slots[index] = None;
        }
    }
}

/// `OnUse` context of a fired item: effects centred on the caster read `origin` /
/// `point`; point and direction targets (e.g. teleport destinations) read `aim`.
fn item_use_trigger(
    item: ItemId,
    owner: Entity,
    team: Team,
    position: Vec3,
    aim: Vec3,
    target: CastTarget,
) -> TriggerContext {
    TriggerContext::new(
        AbilityTrigger::OnUse,
        EffectSource::Item(item),
        owner,
        team,
        1,
        position,
        aim,
    )
    .at(position)
    .cast_target(target.unit())
}

/// Presentation only: a heal flash when the active affects its owner and a pulse ring
/// for each fixed-radius area it hits.
/// The selected-attribute bonuses of every toggle item the hero owns, already folded into
/// its stats. Inserted lazily by [`sync_item_attribute_toggles`].
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct AppliedItemToggles(pub ItemPassives);

/// What the hero's toggle items grant right now: `attribute_toggle` points of each owned
/// instance's selected attribute.
fn desired_toggle_passives(inv: &Inventory) -> ItemPassives {
    let mut total = ItemPassives::default();
    for item in inv.slots.iter().flatten() {
        let Some(points) = item.id.definition().attribute_toggle else {
            continue;
        };
        let points = points * item.stack as f32;
        match item.selected_attribute.unwrap_or(Attribute::Strength) {
            Attribute::Strength => total.strength += points,
            Attribute::Agility => total.agility += points,
            Attribute::Intelligence => total.intelligence += points,
        }
    }
    total
}

/// Reconcile the selected-attribute bonus of toggle items (Power Treads) every frame, like
/// `sync_ability_passives`: remove what was applied, apply what is wanted now. This is the
/// only place that applies or removes it, so buying, selling, undoing, combining and
/// toggling can never double count it; a sold item's bonus goes within one frame. Changing
/// attributes never kills a living hero: its health is floored at 1.
///
/// Runs right after `resolve_item_uses`. Dead heroes are skipped (reconciled on respawn).
#[allow(clippy::type_complexity)]
pub fn sync_item_attribute_toggles(
    mut commands: Commands,
    mut heroes: Query<(
        Entity,
        &Inventory,
        &mut Health,
        &mut Mana,
        &mut CombatStats,
        &mut HeroAttributes,
        Option<&mut AppliedItemToggles>,
    )>,
) {
    for (entity, inv, mut health, mut mana, mut stats, mut attrs, applied) in &mut heroes {
        if !health.is_alive() {
            continue;
        }
        let desired = desired_toggle_passives(inv);
        let current = applied.as_ref().map_or(ItemPassives::default(), |a| a.0);
        if desired == current {
            continue;
        }
        if current != ItemPassives::default() {
            remove_passives(current, 1, &mut health, &mut mana, &mut stats, &mut attrs);
        }
        if desired != ItemPassives::default() {
            apply_passives(desired, &mut health, &mut mana, &mut stats, &mut attrs);
        }
        if health.current < 1.0 {
            health.current = 1.0;
        }
        match applied {
            Some(mut applied) => applied.0 = desired,
            None => {
                commands.entity(entity).insert(AppliedItemToggles(desired));
            }
        }
    }
}

fn spawn_item_use_fx(commands: &mut Commands, assets: &SharedAssets, item: ItemId, at: Vec3) {
    for entry in item.definition().effects_for(AbilityTrigger::OnUse) {
        match entry.target {
            EffectTarget::Caster if matches!(entry.effect, AbilityEffect::Heal { .. }) => {
                spawn_heal_fx(commands, assets, at)
            }
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
        Transform::from_translation(at + Vec3::Y * scale::u(0.2))
            .with_scale(Vec3::splat(scale::u(1.5))),
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
    let in_range = hero
        .single()
        .is_ok_and(|(hero_tf, team)| near_own_shop(hero_tf.translation, *team, &shops));
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
    remove_passives(
        item.id.passives(),
        item.stack + item.spent_units,
        health,
        mana,
        stats,
        attrs,
    );
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

/// Every way to fill the recipe slots of `result`, one id per slot: the cartesian product of
/// the alternatives in CSV order (the first slot varies slowest), so the first option that
/// can be assembled prefers earlier alternatives. Empty when the item is not a build or lists
/// itself as a component.
fn component_options(result: ItemId) -> Vec<Vec<ItemId>> {
    let slots = result.recipe_components();
    if slots.is_empty()
        || slots
            .iter()
            .any(|slot| slot.is_empty() || slot.contains(&result))
    {
        return Vec::new();
    }
    let mut options: Vec<Vec<ItemId>> = vec![Vec::new()];
    for slot in slots {
        options = options
            .into_iter()
            .flat_map(|prefix| {
                slot.iter().map(move |&alt| {
                    let mut option = prefix.clone();
                    option.push(alt);
                    option
                })
            })
            .collect();
    }
    options
}

/// The multiset of one option: each item and how many of it the option consumes.
fn option_needs(option: &[ItemId]) -> Vec<(ItemId, u32)> {
    let mut needs: Vec<(ItemId, u32)> = Vec::new();
    for &comp in option {
        if let Some((_, count)) = needs.iter_mut().find(|(id, _)| *id == comp) {
            *count += 1;
        } else {
            needs.push((comp, 1));
        }
    }
    needs
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

/// The first option (in CSV order) of `result` the inventory can assemble right now.
fn assemblable_option(inv: &Inventory, result: ItemId) -> Option<Vec<(ItemId, u32)>> {
    component_options(result)
        .iter()
        .map(|option| option_needs(option))
        .find(|needs| can_assemble(inv, result, needs))
}

/// The one attribute `id` grants through its passives (Belt of Strength -> Strength), if it
/// grants exactly one.
fn single_attribute(id: ItemId) -> Option<Attribute> {
    let passives = id.passives();
    let granted: Vec<Attribute> = [
        (passives.strength, Attribute::Strength),
        (passives.agility, Attribute::Agility),
        (passives.intelligence, Attribute::Intelligence),
    ]
    .into_iter()
    .filter(|(value, _)| *value > 0.0)
    .map(|(_, attribute)| attribute)
    .collect();
    match granted.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
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
        let Some((result, needs)) = ItemId::all()
            .iter()
            .copied()
            .find_map(|result| assemblable_option(inv, result).map(|needs| (result, needs)))
        else {
            break;
        };
        for &(id, n) in &needs {
            remove_passives(id.passives(), n, health, mana, stats, attrs);
            for _ in 0..n {
                inv.remove_one(id);
            }
        }
        if let Some(index) = inv.try_add_at(result) {
            apply_passives(result.passives(), health, mana, stats, attrs);
            // A toggle item starts on the attribute of the component that was consumed.
            if result.definition().attribute_toggle.is_some()
                && let Some(item) = inv.slots[index].as_mut()
            {
                item.selected_attribute = Some(
                    needs
                        .iter()
                        .find_map(|&(id, _)| single_attribute(id))
                        .unwrap_or(Attribute::Strength),
                );
            }
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
    // Units already spent at commit are gone for good: `try_sell_item` removed their
    // passives and paid nothing for them. Only the units actually sold can be undone, so
    // a sale of an emptied slot leaves nothing to undo.
    if item.stack > 0 {
        log.actions.push(ShopAction::Sale {
            refunded: wallet.gold.saturating_sub(before_gold),
            item: ItemInstance {
                spent_units: 0,
                ..item
            },
        });
    }
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
    inv.slots[slot] = Some(ItemInstance {
        spent_units: 0,
        ..*item
    });
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
            wallet,
            inv,
            health,
            mana,
            stats,
            attrs,
            *gold_spent,
            removed,
            added,
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
thread_local! {
    static DEFINITION_OVERRIDE: std::cell::RefCell<Vec<&'static ItemDefinition>> =
        std::cell::RefCell::new(Vec::new());
}

#[cfg(test)]
fn definition_override(id: ItemId) -> Option<&'static ItemDefinition> {
    DEFINITION_OVERRIDE.with(|slot| slot.borrow().iter().rev().find(|def| def.id == id).copied())
}

/// Test definitions stand in for catalog rows so activation tests can cover target
/// types the shipped items do not use. Overrides are per-thread and dropped with the guard.
#[cfg(test)]
pub(crate) struct DefinitionOverride {
    previous: Vec<&'static ItemDefinition>,
}

#[cfg(test)]
impl DefinitionOverride {
    fn install(defs: impl IntoIterator<Item = ItemDefinition>) -> Self {
        let leaked = defs
            .into_iter()
            .map(|def| &*Box::leak(Box::new(def)))
            .collect();
        let previous =
            DEFINITION_OVERRIDE.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), leaked));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for DefinitionOverride {
    fn drop(&mut self) {
        DEFINITION_OVERRIDE.with(|slot| {
            *slot.borrow_mut() = std::mem::take(&mut self.previous);
        });
    }
}

/// Install temporary item definitions for cross-module integration tests.
#[cfg(test)]
pub(crate) fn install_test_item_definitions(
    defs: impl IntoIterator<Item = ItemDefinition>,
) -> DefinitionOverride {
    DefinitionOverride::install(defs)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::abilities::catalog::{AbilityDefinitions, register_loadout_definitions};
    use crate::abilities::definition::{EffectScaling, RankValue};
    use crate::abilities::effects::{HealEvent, apply_heal_events, resolve_ability_triggers};
    use crate::actions::{
        Activation, ActivationRequest, QueuedActivation, refresh_activation_aims,
        resolve_queued_activations, tick_activations, validate_activation_requests,
    };
    use crate::components::CleaveSource;
    use crate::components::{AbilityId, AbilityLoadout, MoveTarget};
    use crate::displacement::DisplacementEvent;
    use crate::resources::SharedAssets;
    use crate::status::{StatusEffect, StatusEffects};
    use crate::teleport::TeleportEvent;

    #[test]
    fn item_use_trigger_keeps_the_owner_point_and_the_aim() {
        let owner = Entity::from_raw_u32(1).unwrap();
        let aim = Vec3::new(900.0, 0.0, -300.0);
        let context = item_use_trigger(
            ItemId::SparkPendant,
            owner,
            Team::Radiant,
            Vec3::ZERO,
            aim,
            CastTarget::Point(aim),
        );
        assert_eq!(context.aim, aim);
        assert_eq!(context.origin, Vec3::ZERO);
        assert_eq!(
            context.point,
            Vec3::ZERO,
            "area effects stay centred on the owner"
        );
        assert_eq!(context.cast_target, None);
        assert_eq!(context.trigger, AbilityTrigger::OnUse);
    }

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
        let flat = |id: ItemId| -> Vec<ItemId> {
            id.recipe_components()
                .iter()
                .flat_map(|s| s.iter().copied())
                .collect()
        };
        let hb = flat(ItemId::HeartwoodBand);
        assert!(hb.contains(&ItemId::IronBracer));
        assert!(hb.contains(&ItemId::RecipeHeartwoodBand));
        let sp = flat(ItemId::SparkPendant);
        assert!(sp.contains(&ItemId::ManaCrystal));
        assert!(sp.contains(&ItemId::BladeOfAsh));
        assert!(sp.contains(&ItemId::RecipeSparkPendant));
    }

    #[test]
    fn has_active_matches_use_effects_for_every_catalog_item() {
        for &id in ItemId::all() {
            let def = id.definition();
            if !def.mechanics.is_empty() {
                assert!(id.has_active(), "{id:?} has mechanics so it is an active");
                continue;
            }
            if def.attribute_toggle.is_some() {
                assert!(
                    id.has_active(),
                    "{id:?} toggles an attribute so it is an active"
                );
                assert_eq!(def.cooldown, 0.0, "{id:?} toggles have no cooldown");
                continue;
            }
            let uses = def.effects_for(AbilityTrigger::OnUse).next().is_some();
            let channel = def.activation.channel_time > 0.0
                && (def
                    .effects_for(AbilityTrigger::OnChannelEnd)
                    .next()
                    .is_some()
                    || def
                        .effects_for(AbilityTrigger::OnChannelInterrupted)
                        .next()
                        .is_some());
            if !uses && !channel {
                assert!(
                    !id.has_active(),
                    "passive-only {id:?} must not be an active"
                );
            }
        }
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
    fn strength_item_adds_damage_only_to_a_strength_primary_hero() {
        use crate::heroes::HeroId;

        let passives = ItemPassives {
            strength: 10.0,
            ..default()
        };
        // Vanguard is Strength primary: +10 damage, reverted on removal.
        let (mut health, mut mana, mut stats, mut attrs) = HeroId::Vanguard.def().vitals();
        let damage = stats.attack_damage;
        apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
        assert!((stats.attack_damage - (damage + 10.0)).abs() < 1e-4);
        remove_passives(passives, 1, &mut health, &mut mana, &mut stats, &mut attrs);
        assert!((stats.attack_damage - damage).abs() < 1e-4);

        // Skirmisher is Agility primary: the same item adds no damage.
        let (mut health, mut mana, mut stats, mut attrs) = HeroId::Skirmisher.def().vitals();
        let damage = stats.attack_damage;
        apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
        assert_eq!(stats.attack_damage, damage);
    }

    #[test]
    fn attack_modifier_passives_apply_and_remove_symmetrically() {
        let mut health = Health::new(720.0);
        let mut mana = Mana::new(320.0, 12.0);
        let mut stats = CombatStats::simple(55.0, scale::MELEE_ATTACK_RANGE, 1.1, 4.0, 3.0, 12.0);
        let mut attrs = HeroAttributes::starter();
        let before = stats;
        let mut passives = ItemPassives::default();
        for (stat, amount) in [
            (PassiveStat::CritChance, 0.25),
            (PassiveStat::CritMultiplier, 0.5),
            (PassiveStat::Evasion, 0.2),
            (PassiveStat::Accuracy, 0.3),
            (PassiveStat::CleaveRatio, 0.4),
            (PassiveStat::SplashRatio, 0.35),
            (PassiveStat::AttackRange, 100.0),
            (PassiveStat::AttackProjectileSpeed, 150.0),
        ] {
            passives.add_stat(stat, amount);
        }
        // Two copies: bonuses are additive.
        apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
        apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
        assert!((stats.crit_chance - 0.5).abs() < 1e-6);
        assert!(
            (stats.crit_multiplier - 3.0).abs() < 1e-6,
            "2.0 base + 2 x 0.5"
        );
        assert!((stats.evasion - 0.4).abs() < 1e-6);
        assert!((stats.accuracy - 0.6).abs() < 1e-6);
        let items = stats
            .cleaves
            .get(CleaveSource::Items)
            .expect("items cleave");
        assert!(
            (items.ratio - 0.8).abs() < 1e-6,
            "two items stack additively"
        );
        assert_eq!(stats.cleaves.iter().count(), 1, "one aggregated instance");
        assert!((stats.splash_damage_ratio - 0.7).abs() < 1e-6);
        assert_eq!(stats.attack_range, scale::MELEE_ATTACK_RANGE + 200.0);
        assert_eq!(stats.base_attack_range(), scale::MELEE_ATTACK_RANGE);
        assert!(stats.is_melee(), "a range bonus keeps a melee unit melee");
        assert_eq!(stats.attack_projectile_speed_bonus, 300.0);

        remove_passives(passives, 2, &mut health, &mut mana, &mut stats, &mut attrs);
        assert!((stats.crit_chance - before.crit_chance).abs() < 1e-6);
        assert!((stats.crit_multiplier - before.crit_multiplier).abs() < 1e-6);
        assert!(stats.evasion.abs() < 1e-6 && stats.accuracy.abs() < 1e-6);
        assert!(stats.cleaves.is_empty());
        assert!(stats.splash_damage_ratio.abs() < 1e-6);
        assert_eq!(stats.attack_range, before.attack_range);
        assert_eq!(stats.attack_range_bonus, 0.0);
        assert_eq!(stats.attack_projectile_speed_bonus, 0.0);
    }

    fn commit(inv: &mut Inventory, slot: usize) {
        let spec = item_action_spec(inv, slot).expect("usable");
        commit_item(inv, slot, &spec);
    }

    #[test]
    fn consumable_charges_spend_at_commit_and_refill_from_the_next_unit() {
        let mut consumable = test_item(
            ItemId::BladeOfAsh,
            aimed(
                TargetType::NoTarget,
                TargetTeam::SelfOnly,
                0.0,
                0.0,
                0.0,
                0.0,
            ),
        );
        consumable.consumable = true;
        consumable.max_charges = Some(2);
        let _defs = DefinitionOverride::install([consumable]);
        let mut item = ItemInstance::new(ItemId::BladeOfAsh);
        assert_eq!(item.cooldown.charges, Some(2));
        item.stack = 2;
        let mut inv = Inventory::empty();
        inv.slots[0] = Some(item);

        commit(&mut inv, 0);
        let first = inv.slots[0].unwrap();
        assert_eq!(first.cooldown.charges, Some(1));
        assert_eq!((first.stack, first.spent_units), (2, 0));

        commit(&mut inv, 0);
        let second = inv.slots[0].unwrap();
        assert_eq!((second.stack, second.spent_units), (1, 1));
        assert_eq!(
            second.cooldown.charges,
            Some(2),
            "the next unit starts full"
        );

        let mut plain = Inventory::empty();
        let mut single = ItemInstance::new(ItemId::BladeOfAsh);
        single.cooldown.charges = None;
        plain.slots[0] = Some(single);
        commit(&mut plain, 0);
        let used = plain.slots[0].unwrap();
        assert_eq!((used.stack, used.spent_units), (0, 1));
        assert!(
            item_action_spec(&plain, 0).is_err(),
            "an emptied slot cannot be requested again"
        );
    }

    #[test]
    fn non_consumable_items_never_lose_charges() {
        let _defs = DefinitionOverride::install([ItemDefinition {
            max_charges: Some(3),
            consumable: false,
            cooldown: 5.0,
            ..test_item(
                ItemId::AegisCharm,
                aimed(
                    TargetType::NoTarget,
                    TargetTeam::SelfOnly,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                ),
            )
        }]);
        let mut item = ItemInstance::new(ItemId::AegisCharm);
        assert_eq!(item.cooldown.charges, None);
        let mut inv = Inventory::empty();
        item.stack = 1;
        inv.slots[0] = Some(item);
        for _ in 0..3 {
            commit(&mut inv, 0);
        }
        let item = inv.slots[0].unwrap();
        assert_eq!(item.cooldown.charges, None);
        assert_eq!((item.stack, item.spent_units), (1, 0));
    }

    #[test]
    fn last_consumable_unit_cannot_fire_twice_in_one_frame() {
        let mut def = test_item(
            ItemId::BladeOfAsh,
            aimed(
                TargetType::NoTarget,
                TargetTeam::SelfOnly,
                0.0,
                0.0,
                0.0,
                0.0,
            ),
        );
        def.consumable = true;
        def.max_charges = None;
        def.cooldown = 0.0;
        let _defs = DefinitionOverride::install([def]);
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 50.0);
        give(&mut app, caster, ItemId::BladeOfAsh);

        request(&mut app, caster, 0, CastTarget::None);
        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);

        assert_eq!(triggers(&app).len(), 1, "exactly one OnUse");
        assert!(slot(&app, caster, 0).is_none());
    }

    #[test]
    fn undoing_a_sale_restores_the_cooldown_and_leaves_no_spent_units() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let mut log = ShopUndoLog::default();
        assert!(record_purchase(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::IronBracer,
            true,
        ));
        inv.slots[0].as_mut().unwrap().cooldown.remaining = 4.0;
        assert!(record_sale(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            0,
            true,
        ));
        assert!(try_undo_last(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
        ));
        let restored = inv.slots.iter().flatten().next().unwrap();
        assert_eq!(restored.cooldown.remaining, 4.0);
        assert_eq!(restored.spent_units, 0);
        assert_eq!(restored.stack, 1);
    }

    #[test]
    fn undoing_a_sale_made_between_commit_and_resolve_leaves_no_dangling_spent_units() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let mut log = ShopUndoLog::default();
        assert!(record_purchase(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::IronBracer,
            true,
        ));
        let with_item = health.max;
        // As if committed (unit spent) but not yet resolved.
        let item = inv.slots[0].as_mut().unwrap();
        item.stack = 0;
        item.spent_units = 1;
        assert!(record_sale(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            0,
            true,
        ));
        assert_eq!(
            health.max, 720.0,
            "the spent unit's passives left with the sale"
        );
        let _ = try_undo_last(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
        );
        // Whatever undo decides, the slot must not carry a unit that is already spent.
        for item in inv.slots.iter().flatten() {
            assert_eq!(item.spent_units, 0, "dangling spent_units after undo");
            assert!(item.stack > 0, "a ghost slot with no units");
        }
        assert!(
            inv.slots.iter().all(Option::is_none),
            "nothing was left to restore"
        );
        assert_eq!(
            health.max, 720.0,
            "passives are not applied for a spent unit"
        );
        assert!(health.max <= with_item);
    }

    fn two_unit_consumable(cooldown: f32) -> ItemDefinition {
        let mut def = test_item(
            ItemId::BladeOfAsh,
            aimed(
                TargetType::NoTarget,
                TargetTeam::SelfOnly,
                0.0,
                0.0,
                0.0,
                0.0,
            ),
        );
        def.consumable = true;
        def.max_charges = None;
        def.max_stack = 2;
        def.cooldown = cooldown;
        def
    }

    #[test]
    fn same_frame_requests_never_use_more_units_than_the_stack_spends() {
        for cooldown in [0.0, 5.0] {
            let _defs = DefinitionOverride::install([two_unit_consumable(cooldown)]);
            let mut app = item_app();
            let caster = spawn_caster(&mut app, 50.0);
            give(&mut app, caster, ItemId::BladeOfAsh);
            give(&mut app, caster, ItemId::BladeOfAsh);
            assert_eq!(slot(&app, caster, 0).unwrap().stack, 2);

            request(&mut app, caster, 0, CastTarget::None);
            request(&mut app, caster, 0, CastTarget::None);
            let mut uses = 0;
            for _ in 0..4 {
                step(&mut app);
                uses += triggers(&app).len();
            }
            let left = slot(&app, caster, 0).map_or(0, |item| item.stack);
            assert_eq!(
                uses as u32,
                2 - left,
                "cooldown {cooldown}: uses == units spent"
            );
            assert!(uses >= 1);
            if cooldown > 0.0 {
                assert_eq!(uses, 1, "the cooldown blocks the second use");
            }
            assert!(slot(&app, caster, 0).is_none_or(|item| item.spent_units == 0));
        }
    }

    #[test]
    fn selling_between_commit_and_resolve_removes_the_spent_unit_passives() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
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
        let max_health = health.max;
        assert!(max_health > 720.0);
        // As if committed (unit spent) but not yet resolved.
        let item = inv.slots[0].as_mut().unwrap();
        item.stack = 0;
        item.spent_units = 1;
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
        assert_eq!(health.max, 720.0);
    }

    fn passive_fields(p: &ItemPassives) -> [f32; 30] {
        let ItemPassives {
            max_health,
            health_regen,
            max_mana,
            mana_regen,
            attack_damage,
            armor,
            magic_resist,
            move_speed,
            attack_speed_flat,
            strength,
            agility,
            intelligence,
            spell_amp,
            outgoing_physical_amp,
            incoming_damage_amp,
            damage_block,
            damage_reflect,
            heal_amp,
            heal_received_amp,
            lifesteal,
            spell_lifesteal,
            mana_cost_amp,
            crit_chance,
            crit_multiplier,
            evasion,
            accuracy,
            cleave_ratio,
            splash_ratio,
            attack_range,
            attack_projectile_speed,
        } = *p;
        [
            max_health,
            health_regen,
            max_mana,
            mana_regen,
            attack_damage,
            armor,
            magic_resist,
            move_speed,
            attack_speed_flat,
            strength,
            agility,
            intelligence,
            spell_amp,
            outgoing_physical_amp,
            incoming_damage_amp,
            damage_block,
            damage_reflect,
            heal_amp,
            heal_received_amp,
            lifesteal,
            spell_lifesteal,
            mana_cost_amp,
            crit_chance,
            crit_multiplier,
            evasion,
            accuracy,
            cleave_ratio,
            splash_ratio,
            attack_range,
            attack_projectile_speed,
        ]
    }

    #[test]
    fn passive_max_health_keeps_missing_rule() {
        use crate::vitals::MaxChangeRule;
        let mut health = Health {
            current: 400.0,
            max: 1000.0,
            regen_per_sec: 0.0,
        };
        health.change_max(200.0, MaxChangeRule::KeepMissing);
        assert_eq!((health.current, health.max), (600.0, 1200.0), "a gain adds");
        health.change_max(-900.0, MaxChangeRule::KeepMissing);
        assert_eq!(
            (health.current, health.max),
            (300.0, 300.0),
            "a loss only clamps"
        );
        health.change_max(-900.0, MaxChangeRule::KeepMissing);
        assert_eq!((health.current, health.max), (1.0, 1.0), "never below 1");

        let mut mana = Mana::new(100.0, 0.0);
        mana.current = 20.0;
        mana.change_max(50.0, MaxChangeRule::KeepMissing);
        assert_eq!((mana.current, mana.max), (70.0, 150.0));
        mana.change_max(-500.0, MaxChangeRule::KeepMissing);
        assert_eq!((mana.current, mana.max), (0.0, 0.0));
    }

    #[test]
    fn preserve_ratio_never_kills_a_living_unit() {
        use crate::vitals::MaxChangeRule;
        let mut health = Health {
            current: 1.0,
            max: 1000.0,
            regen_per_sec: 0.0,
        };
        health.change_max(-900.0, MaxChangeRule::PreserveRatio);
        assert_eq!(health.max, 100.0);
        assert_eq!(health.current, 1.0);
    }

    #[test]
    fn every_passive_stat_maps_to_a_distinct_field() {
        let stats = [
            PassiveStat::MaxHealth,
            PassiveStat::HealthRegen,
            PassiveStat::MaxMana,
            PassiveStat::ManaRegen,
            PassiveStat::AttackDamage,
            PassiveStat::AttackSpeed,
            PassiveStat::Armor,
            PassiveStat::MagicResist,
            PassiveStat::MoveSpeed,
            PassiveStat::Attribute(Attribute::Strength),
            PassiveStat::Attribute(Attribute::Agility),
            PassiveStat::Attribute(Attribute::Intelligence),
            PassiveStat::SpellAmp,
            PassiveStat::OutgoingPhysicalAmp,
            PassiveStat::IncomingDamageAmp,
            PassiveStat::DamageBlock,
            PassiveStat::DamageReflect,
            PassiveStat::HealAmp,
            PassiveStat::HealReceivedAmp,
            PassiveStat::Lifesteal,
            PassiveStat::SpellLifesteal,
            PassiveStat::ManaCostAmp,
            PassiveStat::CritChance,
            PassiveStat::CritMultiplier,
            PassiveStat::Evasion,
            PassiveStat::Accuracy,
            PassiveStat::CleaveRatio,
            PassiveStat::SplashRatio,
            PassiveStat::AttackRange,
            PassiveStat::AttackProjectileSpeed,
        ];
        let mut seen = Vec::new();
        for stat in stats {
            let mut p = ItemPassives::default();
            p.add_stat(stat, 1.0);
            let changed: Vec<usize> = passive_fields(&p)
                .iter()
                .enumerate()
                .filter(|(_, v)| **v != 0.0)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(changed.len(), 1, "{stat:?} must change exactly one field");
            assert!(!seen.contains(&changed[0]), "{stat:?} shares a field");
            seen.push(changed[0]);
        }
        assert_eq!(seen.len(), 30, "every field is reachable");
    }

    #[test]
    fn passives_add_assign_sums_every_field() {
        let mut a = ItemPassives::default();
        let mut b = ItemPassives::default();
        a.add_stat(PassiveStat::Armor, 2.0);
        b.add_stat(PassiveStat::Armor, 3.0);
        b.add_stat(PassiveStat::DamageReflect, 0.1);
        a += b;
        assert_eq!(a.armor, 5.0);
        assert_eq!(a.damage_reflect, 0.1);
    }

    #[test]
    fn item_and_ability_describe_the_same_effect_with_the_same_label() {
        use crate::abilities::definition::AbilityDefinition;
        use crate::components::DamageType;

        let damage = AbilityEffect::Damage {
            amount: RankValue::fixed(50.0),
            damage_type: DamageType::Magical,
            scaling: EffectScaling::None,
        };
        let on_cast = AbilityEffectEntry {
            trigger: AbilityTrigger::OnCast,
            target: EffectTarget::Caster,
            effect: damage,
        };
        let on_use = AbilityEffectEntry {
            trigger: AbilityTrigger::OnUse,
            ..on_cast
        };
        let (ability_rows, _) = tooltip::describe_effect(&on_cast);
        let (item_rows, _) = tooltip::describe_effect(&on_use);
        assert_eq!(
            ability_rows, item_rows,
            "the label does not depend on the trigger"
        );
        assert_eq!(ability_rows[0].label, "Magical damage");

        let mut def = AbilityDefinition::new(AbilityId::Bolt);
        def.effects.push(on_cast);
        let ability_text: String = tooltip::ability_tooltip_bits(&def, 1)
            .iter()
            .map(|bit| bit.text.as_str())
            .collect();
        assert!(
            ability_text.contains("Magical damage (on cast): 50"),
            "{ability_text}"
        );
        assert_eq!(item_effect_lines(&on_use), vec!["Use: Magical damage 50"]);
    }

    #[test]
    fn item_tooltip_duration_rows_carry_a_seconds_unit() {
        let cloak = ItemId::WardstoneCloak.tooltip_body();
        assert!(cloak.contains("Use: Armor 8"), "{cloak}");
        assert!(cloak.contains("Use: Duration 5s"), "{cloak}");
    }

    #[test]
    fn item_tooltip_area_note_rides_on_the_effect_line() {
        let rod = ItemId::StormRod.tooltip_body();
        assert!(rod.contains("damage 140 (enemies within 143)"), "{rod}");
        assert!(
            !rod.lines()
                .any(|line| line.trim_start().starts_with("Use: enemies")),
            "no standalone area line: {rod}"
        );
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
    fn lifesteal_tooltip_lines_are_percentages() {
        let morbid = ItemId::MorbidMask.tooltip_body();
        assert!(morbid.contains("+18% Lifesteal"), "{morbid}");
        let voodoo = ItemId::VoodooMask.tooltip_body();
        assert!(voodoo.contains("+15% Spell lifesteal"), "{voodoo}");
        assert!(!voodoo.contains("+0 "), "{voodoo}");
    }

    #[test]
    fn masks_grant_lifesteal_and_sale_reverts_it() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let base = stats.lifesteal;
        let base_spell = stats.spell_lifesteal;
        for item in [ItemId::MorbidMask, ItemId::VoodooMask] {
            assert!(try_buy_item(
                &mut wallet,
                &mut inv,
                &mut health,
                &mut mana,
                &mut stats,
                &mut attrs,
                item,
                true,
            ));
        }
        assert_eq!(wallet.gold, 2000 - 900 - 650);
        assert!((stats.lifesteal - base - 0.18).abs() < 1e-5);
        assert!((stats.spell_lifesteal - base_spell - 0.15).abs() < 1e-5);
        for slot in 0..2 {
            assert!(try_sell_item(
                &mut wallet,
                &mut inv,
                &mut health,
                &mut mana,
                &mut stats,
                &mut attrs,
                slot,
                true,
            ));
        }
        assert!((stats.lifesteal - base).abs() < 1e-5);
        assert!((stats.spell_lifesteal - base_spell).abs() < 1e-5);
        assert_eq!(wallet.gold, 2000 - 900 - 650 + 450 + 325);
    }

    #[test]
    fn ghost_scepter_catalog_values_and_attribute_bonus() {
        let def = ItemId::GhostScepter.definition();
        assert_eq!((def.cost, def.sell_value, def.cooldown), (1500, 750, 22.0));
        assert!(ItemId::GhostScepter.has_active());
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let base = attrs;
        assert!(try_buy_item(
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::GhostScepter,
            true,
        ));
        assert_eq!(attrs.strength, base.strength + 5.0);
        assert_eq!(attrs.agility, base.agility + 5.0);
        assert_eq!(attrs.intelligence, base.intelligence + 5.0);
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
        assert_eq!(attrs.strength, base.strength);
        assert_eq!(wallet.gold, 2000 - 1500 + 750);
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

    /// A hero at the shop with an undo log, for tests that only care about trades.
    struct Shopper {
        wallet: PlayerWallet,
        inv: Inventory,
        health: Health,
        mana: Mana,
        stats: CombatStats,
        attrs: HeroAttributes,
        log: ShopUndoLog,
    }

    impl Shopper {
        fn new() -> Self {
            let (wallet, inv, health, mana, stats, attrs) = fresh_hero();
            Self {
                wallet,
                inv,
                health,
                mana,
                stats,
                attrs,
                log: ShopUndoLog::default(),
            }
        }

        fn buy(&mut self, item: ItemId) -> bool {
            record_purchase(
                &mut self.log,
                &mut self.wallet,
                &mut self.inv,
                &mut self.health,
                &mut self.mana,
                &mut self.stats,
                &mut self.attrs,
                item,
                true,
            )
        }

        fn sell(&mut self, slot: usize) -> bool {
            record_sale(
                &mut self.log,
                &mut self.wallet,
                &mut self.inv,
                &mut self.health,
                &mut self.mana,
                &mut self.stats,
                &mut self.attrs,
                slot,
                true,
            )
        }

        fn undo(&mut self) -> bool {
            try_undo_last(
                &mut self.log,
                &mut self.wallet,
                &mut self.inv,
                &mut self.health,
                &mut self.mana,
                &mut self.stats,
                &mut self.attrs,
            )
        }

        fn owned(&self) -> Vec<ItemId> {
            owned(&self.inv)
        }
    }

    /// Stand-in recipe for `AegisCharm`: (`IronBracer` or `SwiftBoots`) and `ManaCrystal`.
    fn or_recipe() -> DefinitionOverride {
        let slots: &'static [&'static [ItemId]] = &[
            &[ItemId::IronBracer, ItemId::SwiftBoots],
            &[ItemId::ManaCrystal],
        ];
        install_test_item_definitions([ItemDefinition {
            components: slots,
            ..*ItemId::AegisCharm.definition()
        }])
    }

    #[test]
    fn component_options_follow_csv_order() {
        let _defs = or_recipe();
        assert_eq!(
            component_options(ItemId::AegisCharm),
            vec![
                vec![ItemId::IronBracer, ItemId::ManaCrystal],
                vec![ItemId::SwiftBoots, ItemId::ManaCrystal],
            ]
        );
        assert!(component_options(ItemId::IronBracer).is_empty());
    }

    #[test]
    fn or_slot_consumes_the_first_owned_alternative() {
        let _defs = or_recipe();
        // Only the second alternative owned: it is consumed.
        let mut shop = Shopper::new();
        assert!(shop.buy(ItemId::SwiftBoots));
        assert!(shop.buy(ItemId::ManaCrystal));
        assert_eq!(shop.owned(), vec![ItemId::AegisCharm]);

        // Both owned: the first in CSV order goes, the other stays.
        let mut shop = Shopper::new();
        assert!(shop.buy(ItemId::SwiftBoots));
        assert!(shop.buy(ItemId::IronBracer));
        assert!(shop.buy(ItemId::ManaCrystal));
        assert_eq!(shop.owned(), vec![ItemId::SwiftBoots, ItemId::AegisCharm]);
    }

    #[test]
    fn or_slot_without_any_alternative_does_not_combine() {
        let _defs = or_recipe();
        let mut shop = Shopper::new();
        assert!(shop.buy(ItemId::ManaCrystal));
        assert!(shop.buy(ItemId::VialOfLight));
        assert!(!shop.owned().contains(&ItemId::AegisCharm));
    }

    #[test]
    fn undo_restores_the_exact_alternative_that_was_consumed() {
        let _defs = or_recipe();
        let mut shop = Shopper::new();
        assert!(shop.buy(ItemId::SwiftBoots));
        assert!(shop.buy(ItemId::ManaCrystal));
        assert_eq!(shop.owned(), vec![ItemId::AegisCharm]);
        let reference = {
            let mut r = Shopper::new();
            assert!(r.buy(ItemId::SwiftBoots));
            r
        };
        assert!(shop.undo());
        assert_eq!(shop.owned(), vec![ItemId::SwiftBoots]);
        assert_eq!(shop.wallet.gold, reference.wallet.gold);
        assert_eq!(shop.stats.move_speed, reference.stats.move_speed);
        assert_eq!(shop.stats.armor, reference.stats.armor);
    }

    #[test]
    fn component_tooltip_lists_alternatives_in_one_line() {
        let _defs = or_recipe();
        let body = ItemId::AegisCharm.tooltip_body();
        assert!(
            body.contains("• Iron Bracer / Swift Boots (300g)"),
            "{body}"
        );
        assert!(body.contains("• Mana Crystal (350g)"), "{body}");
    }

    fn treads_selection(shop: &Shopper) -> Option<Attribute> {
        shop.inv
            .slots
            .iter()
            .flatten()
            .find(|item| item.id == ItemId::PowerTreads)
            .and_then(|item| item.selected_attribute)
    }

    #[test]
    fn power_treads_build_from_each_alternative_with_the_matching_start_attribute() {
        let def = ItemId::PowerTreads.definition();
        assert_eq!((def.cost, def.attribute_toggle), (1400, Some(10.0)));
        assert!(ItemId::PowerTreads.has_active());
        for (alternative, expected) in [
            (ItemId::BeltOfStrength, Attribute::Strength),
            (ItemId::BandOfElvenskin, Attribute::Agility),
            (ItemId::RobeOfTheMagi, Attribute::Intelligence),
        ] {
            let mut shop = Shopper::new();
            for item in [ItemId::BootsOfSpeed, ItemId::GlovesOfHaste, alternative] {
                assert!(shop.buy(item), "{item:?}");
            }
            assert_eq!(shop.owned(), vec![ItemId::PowerTreads], "{alternative:?}");
            assert_eq!(treads_selection(&shop), Some(expected), "{alternative:?}");
            assert_eq!(shop.wallet.gold, 2000 - 500 - 450 - 450);
        }
    }

    #[test]
    fn power_treads_bought_directly_start_on_strength_and_pay_static_stats() {
        let mut shop = Shopper::new();
        let base_speed = shop.stats.move_speed;
        shop.wallet.gold = 5000;
        assert!(shop.buy(ItemId::PowerTreads));
        assert_eq!(treads_selection(&shop), Some(Attribute::Strength));
        assert_eq!(shop.stats.move_speed, base_speed + 55.0);
    }

    #[test]
    fn undoing_a_treads_build_restores_the_alternative_that_was_used() {
        let mut shop = Shopper::new();
        for item in [ItemId::BootsOfSpeed, ItemId::GlovesOfHaste] {
            assert!(shop.buy(item));
        }
        let before_gold = shop.wallet.gold;
        assert!(shop.buy(ItemId::BandOfElvenskin));
        assert_eq!(shop.owned(), vec![ItemId::PowerTreads]);
        assert!(shop.undo());
        let mut back = shop.owned();
        back.sort_by_key(|id| id.name());
        assert_eq!(back, vec![ItemId::BootsOfSpeed, ItemId::GlovesOfHaste]);
        assert_eq!(shop.wallet.gold, before_gold);
    }

    #[test]
    fn undoing_a_treads_sale_keeps_the_selected_attribute() {
        let mut shop = Shopper::new();
        shop.wallet.gold = 5000;
        assert!(shop.buy(ItemId::PowerTreads));
        shop.inv.slots[0].as_mut().unwrap().selected_attribute = Some(Attribute::Intelligence);
        assert!(shop.sell(0));
        assert!(shop.owned().is_empty());
        assert!(shop.undo());
        assert_eq!(treads_selection(&shop), Some(Attribute::Intelligence));
    }

    #[test]
    fn treads_tooltip_lists_the_alternatives_and_the_toggle() {
        let body = ItemId::PowerTreads.tooltip_body();
        assert!(
            body.contains("• Belt of Strength / Band of Elvenskin / Robe of the Magi (450g)"),
            "{body}"
        );
        assert!(body.contains("Use: switch the +10 attribute"), "{body}");
    }

    #[test]
    fn soul_ring_needs_two_gauntlets_and_a_ring_and_undo_is_exact() {
        let mut shop = Shopper::new();
        let base_armor = shop.stats.armor;
        let base_str = shop.attrs.strength;
        for item in [
            ItemId::GauntletsOfStrength,
            ItemId::RingOfProtection,
            ItemId::RecipeSoulRing,
        ] {
            assert!(shop.buy(item));
        }
        assert!(
            !shop.owned().contains(&ItemId::SoulRing),
            "one gauntlets is not enough"
        );
        assert!(shop.buy(ItemId::GauntletsOfStrength));
        assert_eq!(shop.owned(), vec![ItemId::SoulRing]);
        assert_eq!(shop.wallet.gold, 2000 - 140 - 140 - 175 - 245);
        assert_eq!(shop.stats.armor, base_armor + 2.0);
        assert_eq!(shop.attrs.strength, base_str + 6.0);

        assert!(shop.undo());
        let mut back = shop.owned();
        back.sort_by_key(|id| id.name());
        assert_eq!(
            back,
            vec![
                ItemId::GauntletsOfStrength,
                ItemId::RecipeSoulRing,
                ItemId::RingOfProtection,
            ]
        );
        // Back to the state before the last buy: exactly what three plain buys give.
        let mut reference = Shopper::new();
        for item in [
            ItemId::GauntletsOfStrength,
            ItemId::RingOfProtection,
            ItemId::RecipeSoulRing,
        ] {
            assert!(reference.buy(item));
        }
        assert_eq!(shop.wallet.gold, reference.wallet.gold);
        assert_eq!(shop.stats.armor, reference.stats.armor);
        assert_eq!(shop.attrs.strength, reference.attrs.strength);
        assert_eq!(shop.health.max, reference.health.max);
    }

    #[test]
    fn item_types_include_attributes_equipment_misc_and_accessories() {
        assert_eq!(ItemType::Attributes.label(), "Attributes");
        assert_eq!(ItemType::Equipment.label(), "Equipment");
        assert_eq!(ItemType::Miscellaneous.label(), "Miscellaneous");
        assert_eq!(ItemType::Accessories.label(), "Accessories");
        assert_eq!(ItemType::Support.label(), "Support");
        assert_eq!(ItemType::Magical.label(), "Magical");
        assert_eq!(ItemType::Weapons.label(), "Weapons");
        assert_eq!(ItemType::Artifacts.label(), "Artifacts");
        assert_eq!(
            ItemId::IronBracer.definition().item_type,
            ItemType::Equipment
        );
        assert_eq!(
            ItemId::SwiftBoots.definition().item_type,
            ItemType::Equipment
        );
        assert_eq!(
            ItemId::VialOfLight.definition().item_type,
            ItemType::Miscellaneous
        );
        assert_eq!(
            ItemId::ManaCrystal.definition().item_type,
            ItemType::Accessories
        );
        assert_eq!(ItemId::BladeOfAsh.definition().item_type, ItemType::Weapons);
        assert_eq!(ItemId::AegisCharm.definition().item_type, ItemType::Armor);
        assert_eq!(ItemId::StormRod.definition().item_type, ItemType::Magical);
        assert_eq!(
            ItemId::WardstoneCloak.definition().item_type,
            ItemType::Support
        );
        assert_eq!(
            ItemId::HeartwoodBand.definition().item_type,
            ItemType::Artifacts
        );
        assert_eq!(
            ItemId::SparkPendant.definition().item_type,
            ItemType::Magical
        );
        assert_eq!(
            ItemId::RecipeHeartwoodBand.definition().item_type,
            ItemType::Recipe
        );
    }

    #[test]
    fn shop_tabs_split_basics_and_upgrades() {
        assert!(ShopTab::Basics.accepts(ItemType::Attributes));
        assert!(ShopTab::Basics.accepts(ItemType::Equipment));
        assert!(ShopTab::Basics.accepts(ItemType::Miscellaneous));
        assert!(ShopTab::Upgrades.accepts(ItemType::Accessories));
        assert!(ShopTab::Upgrades.accepts(ItemType::Support));
        assert!(ShopTab::Upgrades.accepts(ItemType::Magical));
        assert!(ShopTab::Upgrades.accepts(ItemType::Armor));
        assert!(ShopTab::Upgrades.accepts(ItemType::Weapons));
        assert!(ShopTab::Upgrades.accepts(ItemType::Artifacts));
        assert!(!ShopTab::Basics.accepts(ItemType::Armor));
        assert!(!ShopTab::Upgrades.accepts(ItemType::Equipment));
        assert!(!ShopTab::Upgrades.accepts(ItemType::Recipe));

        let basics: Vec<_> = ItemId::shop_listed()
            .filter(|id| ShopTab::Basics.accepts(id.definition().item_type))
            .collect();
        let upgrades: Vec<_> = ItemId::shop_listed()
            .filter(|id| ShopTab::Upgrades.accepts(id.definition().item_type))
            .collect();
        assert!(basics.contains(&ItemId::IronBracer));
        assert!(basics.contains(&ItemId::SwiftBoots));
        assert!(basics.contains(&ItemId::VialOfLight));
        assert!(upgrades.contains(&ItemId::ManaCrystal));
        assert!(upgrades.contains(&ItemId::BladeOfAsh));
        assert!(upgrades.contains(&ItemId::AegisCharm));
        assert!(upgrades.contains(&ItemId::StormRod));
        assert!(upgrades.contains(&ItemId::WardstoneCloak));
        assert!(upgrades.contains(&ItemId::HeartwoodBand));
        assert!(upgrades.contains(&ItemId::SparkPendant));
        assert_eq!(basics.len() + upgrades.len(), ItemId::shop_listed().count());
    }

    #[test]
    fn buying_the_last_component_combines_and_undo_refunds_only_that_purchase() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let mut log = ShopUndoLog::default();
        assert!(record_purchase(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::IronBracer,
            true,
        ));
        assert_eq!(owned(&inv), vec![ItemId::IronBracer]);
        assert_eq!(health.max, 820.0);
        assert_eq!(stats.armor, 8.0);

        assert!(record_purchase(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::RecipeHeartwoodBand,
            true,
        ));
        assert_eq!(owned(&inv), vec![ItemId::HeartwoodBand]);
        assert!(!owned(&inv).contains(&ItemId::RecipeHeartwoodBand));
        assert_eq!(
            wallet.gold,
            2000 - ItemId::IronBracer.cost() - ItemId::RecipeHeartwoodBand.cost()
        );
        assert_eq!(health.max, 870.0);
        assert_eq!(stats.armor, 7.0);
        assert_eq!(mana.regen_per_sec, 14.0);
        assert_eq!(log.pending(), 2);

        assert!(try_undo_last(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
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
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
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
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::SwiftBoots,
            true,
        ));
        assert!(try_undo_last(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
        ));
        assert!(owned(&inv).is_empty());
        assert_eq!(wallet.gold, 2000);
        assert_eq!(stats.move_speed, 12.0);

        assert!(record_purchase(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::IronBracer,
            true,
        ));
        inv.slots[0].as_mut().unwrap().cooldown.remaining = 3.5;
        assert!(record_sale(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            0,
            true,
        ));
        assert!(owned(&inv).is_empty());
        assert_eq!(
            wallet.gold,
            2000 - ItemId::IronBracer.cost() + ItemId::IronBracer.definition().sell_value
        );
        assert!(try_undo_last(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
        ));
        assert_eq!(owned(&inv), vec![ItemId::IronBracer]);
        assert_eq!(
            inv.slots
                .iter()
                .flatten()
                .next()
                .unwrap()
                .cooldown
                .remaining,
            3.5
        );
        assert_eq!(wallet.gold, 2000 - ItemId::IronBracer.cost());
        assert_eq!(health.max, 820.0);
    }

    #[test]
    fn undo_refuses_when_the_combined_item_is_gone_and_leaving_range_locks_trades() {
        let (mut wallet, mut inv, mut health, mut mana, mut stats, mut attrs) = fresh_hero();
        let mut log = ShopUndoLog::default();
        assert!(record_purchase(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::IronBracer,
            true,
        ));
        assert!(record_purchase(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            ItemId::RecipeHeartwoodBand,
            true,
        ));
        let gold = wallet.gold;
        inv.slots = [None, None, None, None, None, None];
        assert!(!try_undo_last(
            &mut log,
            &mut wallet,
            &mut inv,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
        ));
        assert_eq!(wallet.gold, gold);
        assert_eq!(log.pending(), 2);
        assert!(!log.notice.is_empty());

        expire_shop_undo(&mut log, false);
        assert_eq!(log.pending(), 0);
        assert!(log.notice.is_empty());
        expire_shop_undo(&mut log, true);
    }

    fn item_app() -> App {
        let mut app = App::new();
        // Definition overrides live in a thread local. The multi-threaded executor would
        // run these systems on another thread and miss them.
        app.edit_schedule(Update, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.init_resource::<Time>()
            .init_resource::<SharedAssets>()
            .init_resource::<AbilityDefinitions>()
            .init_resource::<crate::abilities::effects::CustomAbilityEffects>()
            .init_resource::<crate::combat::CombatRng>()
            .add_message::<crate::basic_attack::ExtraAttackEvent>()
            .add_message::<ActivationRequest>()
            .add_message::<crate::actions::ActionFiredEvent>()
            .add_message::<crate::actions::ActionChannelEndEvent>()
            .add_message::<AbilityTriggerEvent>()
            .add_message::<crate::abilities::effects::SpellHitEvent>()
            .add_message::<crate::abilities::effects::SpellReflectEvent>()
            .add_message::<crate::abilities::zones::SpawnZoneEvent>()
            .add_message::<crate::abilities::orbits::SpawnOrbitEvent>()
            .add_message::<crate::abilities::reactive_triggers::CooldownReadyEvent>()
            .add_message::<HealEvent>()
            .add_message::<crate::abilities::effects::ManaRestoreEvent>()
            .add_message::<crate::vitals::ResourceDrainEvent>()
            .add_message::<crate::status::StatusEffectEvent>()
            .add_message::<crate::combat::DamageEvent>()
            .add_message::<crate::combat::DamageAppliedEvent>()
            .add_message::<crate::status::StatusLifecycleEvent>()
            .add_message::<DisplacementEvent>()
            .add_message::<TeleportEvent>()
            .add_message::<crate::combat::SpawnProjectileEvent>()
            .add_systems(
                Update,
                (
                    register_loadout_definitions,
                    tick_item_cooldowns,
                    resolve_queued_activations,
                    validate_activation_requests,
                    refresh_activation_aims,
                    tick_activations,
                    (
                        resolve_item_uses,
                        sync_item_attribute_toggles,
                        crate::abilities::mechanics::execute_item_mechanics,
                    )
                        .chain(),
                    crate::abilities::casting::action_channel_end_triggers,
                    resolve_ability_triggers,
                    apply_heal_events,
                )
                    .chain(),
            );
        app
    }

    fn step(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(0.05));
        app.update();
    }

    fn spawn_caster(app: &mut App, mana: f32) -> Entity {
        let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0);
        stats.turn_rate = 1_000.0;
        app.world_mut()
            .spawn((
                Transform::from_translation(Vec3::ZERO),
                Team::Radiant,
                Health::new(1000.0),
                Mana::new(mana, 0.0),
                stats,
                HeroAttributes::starter(),
                Inventory::empty(),
                StatusEffects::default(),
            ))
            .id()
    }

    fn give(app: &mut App, caster: Entity, id: ItemId) -> ItemInstanceId {
        let mut inventory = app.world_mut().get_mut::<Inventory>(caster).unwrap();
        assert!(inventory.try_add(id), "inventory rejected {id:?}");
        inventory.slots.iter().flatten().last().unwrap().instance
    }

    fn request(app: &mut App, caster: Entity, slot: usize, target: CastTarget) {
        app.world_mut()
            .write_message(ActivationRequest::item(caster, slot, target));
    }

    fn heal_effect() -> &'static [AbilityEffectEntry] {
        Box::leak(Box::new([AbilityEffectEntry {
            trigger: AbilityTrigger::OnUse,
            target: EffectTarget::Caster,
            effect: AbilityEffect::Heal {
                amount: RankValue::fixed(10.0),
                scaling: EffectScaling::None,
            },
        }]))
    }

    fn channel_end_heal_effects() -> &'static [AbilityEffectEntry] {
        Box::leak(Box::new([AbilityEffectEntry {
            trigger: AbilityTrigger::OnChannelEnd,
            target: EffectTarget::Caster,
            effect: AbilityEffect::Heal {
                amount: RankValue::fixed(200.0),
                scaling: EffectScaling::None,
            },
        }]))
    }

    fn channeled_test_item(id: ItemId) -> ItemDefinition {
        ItemDefinition {
            id,
            name: "Channeled Test",
            short_label: "CH",
            item_type: ItemType::Miscellaneous,
            cost: 1,
            sell_value: 0,
            tier: 1,
            rarity: ItemRarity::Common,
            max_stack: 1,
            consumable: false,
            max_charges: None,
            cooldown: 0.0,
            activation: aimed(
                TargetType::NoTarget,
                TargetTeam::SelfOnly,
                0.0,
                0.1,
                0.5,
                0.0,
            ),
            attribute_toggle: None,
            components: &[],
            color: [1.0, 1.0, 1.0],
            description: "test",
            effects: channel_end_heal_effects(),
            mechanics: &[],
        }
    }

    fn test_item(id: ItemId, activation: ItemActivation) -> ItemDefinition {
        ItemDefinition {
            id,
            name: "Test Active",
            short_label: "TA",
            item_type: ItemType::Miscellaneous,
            cost: 1,
            sell_value: 0,
            tier: 1,
            rarity: ItemRarity::Common,
            max_stack: 1,
            consumable: false,
            max_charges: None,
            cooldown: 5.0,
            activation,
            attribute_toggle: None,
            components: &[],
            color: [1.0, 1.0, 1.0],
            description: "test",
            effects: heal_effect(),
            mechanics: &[],
        }
    }

    fn aimed(
        target_type: TargetType,
        target_team: TargetTeam,
        cast_range: f32,
        cast_time: f32,
        channel_time: f32,
        mana_cost: f32,
    ) -> ItemActivation {
        ItemActivation {
            target_type,
            target_team,
            cast_range,
            cast_time,
            channel_time,
            mana_cost,
        }
    }

    fn slot(app: &App, caster: Entity, index: usize) -> Option<ItemInstance> {
        app.world().get::<Inventory>(caster).unwrap().slots[index]
    }

    fn health_of(app: &App, entity: Entity) -> f32 {
        app.world().get::<Health>(entity).unwrap().current
    }

    fn triggers(app: &App) -> Vec<crate::abilities::effects::TriggerContext> {
        app.world()
            .resource::<Messages<AbilityTriggerEvent>>()
            .iter_current_update_messages()
            .map(|event| event.0.clone())
            .collect()
    }

    fn run_until_fired(app: &mut App, frames: usize) -> bool {
        for _ in 0..frames {
            step(app);
            if !triggers(app).is_empty() {
                return true;
            }
        }
        false
    }

    fn attributes_of(app: &App, owner: Entity) -> (f32, f32, f32) {
        let a = app.world().get::<HeroAttributes>(owner).unwrap();
        (a.strength, a.agility, a.intelligence)
    }

    fn selection(app: &App, owner: Entity) -> Option<Attribute> {
        slot(app, owner, 0).unwrap().selected_attribute
    }

    fn toggle(app: &mut App, owner: Entity) {
        request(app, owner, 0, CastTarget::None);
        for _ in 0..3 {
            step(app);
        }
    }

    #[test]
    fn treads_toggle_cycles_the_attribute_and_moves_the_bonus() {
        let mut app = item_app();
        let owner = spawn_caster(&mut app, 100.0);
        let base = attributes_of(&app, owner);
        give(&mut app, owner, ItemId::PowerTreads);
        step(&mut app);
        assert_eq!(selection(&app, owner), Some(Attribute::Strength));
        assert_eq!(
            attributes_of(&app, owner),
            (base.0 + 10.0, base.1, base.2),
            "a freshly given pair starts on Strength"
        );
        let base_hp = 1000.0;
        assert_eq!(
            app.world().get::<Health>(owner).unwrap().max,
            base_hp + 10.0 * HeroAttributes::HP_PER_STR
        );

        toggle(&mut app, owner);
        assert_eq!(selection(&app, owner), Some(Attribute::Agility));
        assert_eq!(attributes_of(&app, owner), (base.0, base.1 + 10.0, base.2));
        assert_eq!(app.world().get::<Health>(owner).unwrap().max, base_hp);

        toggle(&mut app, owner);
        assert_eq!(selection(&app, owner), Some(Attribute::Intelligence));
        assert_eq!(attributes_of(&app, owner), (base.0, base.1, base.2 + 10.0));

        toggle(&mut app, owner);
        assert_eq!(selection(&app, owner), Some(Attribute::Strength));
        assert_eq!(attributes_of(&app, owner), (base.0 + 10.0, base.1, base.2));
        assert_eq!(
            slot(&app, owner, 0).unwrap().cooldown.remaining,
            0.0,
            "toggling has no cooldown"
        );
    }

    #[test]
    fn treads_toggle_never_kills_a_low_health_hero() {
        let mut app = item_app();
        let owner = spawn_caster(&mut app, 100.0);
        give(&mut app, owner, ItemId::PowerTreads);
        step(&mut app);
        app.world_mut().get_mut::<Health>(owner).unwrap().current = 20.0;
        toggle(&mut app, owner);
        assert_eq!(selection(&app, owner), Some(Attribute::Agility));
        let health = app.world().get::<Health>(owner).unwrap();
        assert_eq!(health.current, 1.0, "losing strength floors health at 1");
        assert!(health.is_alive());
    }

    #[test]
    fn treads_toggle_is_blocked_by_mute_and_stun() {
        let mut app = item_app();
        let owner = spawn_caster(&mut app, 100.0);
        give(&mut app, owner, ItemId::PowerTreads);
        step(&mut app);
        for status in [StatusEffect::mute(5.0), StatusEffect::stunned(5.0)] {
            app.world_mut()
                .get_mut::<StatusEffects>(owner)
                .unwrap()
                .effects
                .push(status);
            toggle(&mut app, owner);
            assert_eq!(selection(&app, owner), Some(Attribute::Strength));
            app.world_mut()
                .get_mut::<StatusEffects>(owner)
                .unwrap()
                .effects
                .clear();
        }
        toggle(&mut app, owner);
        assert_eq!(selection(&app, owner), Some(Attribute::Agility));
    }

    #[test]
    fn removing_treads_removes_the_selected_bonus_within_a_frame() {
        let mut app = item_app();
        let owner = spawn_caster(&mut app, 100.0);
        let base = attributes_of(&app, owner);
        let base_hp = app.world().get::<Health>(owner).unwrap().max;
        give(&mut app, owner, ItemId::PowerTreads);
        toggle(&mut app, owner);
        assert_eq!(attributes_of(&app, owner), (base.0, base.1 + 10.0, base.2));
        app.world_mut().get_mut::<Inventory>(owner).unwrap().slots[0] = None;
        step(&mut app);
        assert_eq!(attributes_of(&app, owner), base);
        assert_eq!(app.world().get::<Health>(owner).unwrap().max, base_hp);
        assert_eq!(
            app.world().get::<AppliedItemToggles>(owner).unwrap().0,
            ItemPassives::default()
        );
    }

    #[test]
    fn two_treads_each_keep_their_own_selection() {
        let mut app = item_app();
        let owner = spawn_caster(&mut app, 100.0);
        let base = attributes_of(&app, owner);
        give(&mut app, owner, ItemId::PowerTreads);
        give(&mut app, owner, ItemId::PowerTreads);
        app.world_mut().get_mut::<Inventory>(owner).unwrap().slots[1]
            .as_mut()
            .unwrap()
            .selected_attribute = Some(Attribute::Intelligence);
        step(&mut app);
        assert_eq!(
            attributes_of(&app, owner),
            (base.0 + 10.0, base.1, base.2 + 10.0)
        );
    }

    #[test]
    fn no_target_item_heals_starts_cooldown_and_ignores_silence() {
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 200.0);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 40.0;
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(StatusEffect::silenced(5.0));
        give(&mut app, caster, ItemId::VialOfLight);

        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);

        assert_eq!(
            health_of(&app, caster),
            220.0,
            "180 heal from the catalog active"
        );
        assert_eq!(slot(&app, caster, 0).unwrap().cooldown.remaining, 40.0);
        assert!(
            app.world().get::<Activation>(caster).is_none(),
            "instant items do not occupy the caster"
        );
        let fired = triggers(&app);
        assert_eq!(fired.len(), 1);
        assert_eq!(
            fired[0].aim,
            Vec3::ZERO,
            "no-target items aim at the caster"
        );
        assert_eq!(
            fired[0].source,
            crate::abilities::effects::EffectSource::Item(ItemId::VialOfLight)
        );
    }

    /// An item and an ability reach the same zone path: an item's attached zone asks for a
    /// following zone exactly as an ability's does.
    #[test]
    fn item_and_ability_attached_zone_alike() {
        let zone = |follow_caster: bool| -> &'static [AbilityEffectEntry] {
            Box::leak(Box::new([AbilityEffectEntry {
                trigger: AbilityTrigger::OnUse,
                target: EffectTarget::Caster,
                effect: AbilityEffect::Zone {
                    radius: AreaRadius::Fixed(RankValue::fixed(300.0)),
                    team: TargetTeam::Enemy,
                    delay: RankValue::ZERO,
                    duration: RankValue::fixed(2.0),
                    interval: RankValue::fixed(0.5),
                    then: AbilityTrigger::Custom("on_item_zone_tick"),
                    follow_caster,
                },
            }]))
        };
        for follow in [true, false] {
            let _defs = DefinitionOverride::install([ItemDefinition {
                effects: zone(follow),
                ..test_item(
                    ItemId::AegisCharm,
                    aimed(
                        TargetType::NoTarget,
                        TargetTeam::SelfOnly,
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                    ),
                )
            }]);
            let mut app = item_app();
            let caster = spawn_caster(&mut app, 200.0);
            give(&mut app, caster, ItemId::AegisCharm);
            request(&mut app, caster, 0, CastTarget::None);
            let mut requested = Vec::new();
            for _ in 0..4 {
                step(&mut app);
                requested.extend(
                    app.world()
                        .resource::<Messages<crate::abilities::zones::SpawnZoneEvent>>()
                        .iter_current_update_messages()
                        .cloned(),
                );
            }
            assert_eq!(requested.len(), 1, "follow = {follow}");
            assert_eq!(requested[0].follow_caster, follow);
            assert_eq!(requested[0].ticks, 4);
        }
    }

    fn heal_fx_count(app: &mut App) -> usize {
        let mut query = app.world_mut().query::<&Name>();
        query
            .iter(app.world())
            .filter(|name| name.as_str() == "Item Heal FX")
            .count()
    }

    #[test]
    fn vial_use_spawns_heal_flash() {
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 200.0);
        give(&mut app, caster, ItemId::VialOfLight);

        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);

        assert_eq!(heal_fx_count(&mut app), 1);
    }

    #[test]
    fn blink_dagger_use_spawns_no_heal_flash() {
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 200.0);
        give(&mut app, caster, ItemId::BlinkDagger);

        request(
            &mut app,
            caster,
            0,
            CastTarget::Point(Vec3::new(200.0, 0.0, 0.0)),
        );
        let mut teleports = Vec::new();
        for _ in 0..10 {
            step(&mut app);
            teleports.extend(
                app.world()
                    .resource::<Messages<TeleportEvent>>()
                    .iter_current_update_messages()
                    .copied(),
            );
        }

        assert_eq!(teleports.len(), 1, "the blink must actually have been used");
        assert_eq!(teleports[0].target, caster);
        assert_eq!(teleports[0].caster, caster);
        assert!((teleports[0].destination - Vec3::new(200.0, 0.0, 0.0)).length() < 1.0);
        assert!(
            slot(&app, caster, 0).unwrap().cooldown.remaining > 0.0,
            "the use also paid its cooldown"
        );
        assert_eq!(heal_fx_count(&mut app), 0);
    }

    #[test]
    fn stunned_caster_and_passive_items_do_not_activate() {
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 200.0);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 40.0;
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(StatusEffect::stunned(5.0));
        give(&mut app, caster, ItemId::VialOfLight);
        give(&mut app, caster, ItemId::IronBracer);

        request(&mut app, caster, 0, CastTarget::None);
        request(&mut app, caster, 1, CastTarget::None);
        step(&mut app);

        assert_eq!(health_of(&app, caster), 40.0);
        assert_eq!(slot(&app, caster, 0).unwrap().cooldown.remaining, 0.0);
        assert_eq!(slot(&app, caster, 1).unwrap().cooldown.remaining, 0.0);
        assert!(triggers(&app).is_empty());
    }

    #[test]
    fn mana_cost_amp_reduces_item_costs() {
        let mut app = item_app();
        let _defs = DefinitionOverride::install([test_item(
            ItemId::StormRod,
            aimed(
                TargetType::SelfTarget,
                TargetTeam::SelfOnly,
                0.0,
                0.0,
                0.0,
                40.0,
            ),
        )]);
        let caster = spawn_caster(&mut app, 100.0);
        app.world_mut()
            .get_mut::<CombatStats>(caster)
            .unwrap()
            .mana_cost_amp = -0.5;
        give(&mut app, caster, ItemId::StormRod);
        request(&mut app, caster, 0, CastTarget::Point(Vec3::ZERO));
        step(&mut app);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 80.0);
    }

    #[test]
    fn each_target_type_resolves_and_invalid_or_unpaid_uses_spend_nothing() {
        let mut app = item_app();
        let _defs = DefinitionOverride::install([
            test_item(
                ItemId::StormRod,
                aimed(TargetType::Unit, TargetTeam::Enemy, 200.0, 0.0, 0.0, 25.0),
            ),
            test_item(
                ItemId::WardstoneCloak,
                aimed(TargetType::Point, TargetTeam::Any, 150.0, 0.0, 0.0, 0.0),
            ),
            test_item(
                ItemId::SparkPendant,
                aimed(TargetType::Direction, TargetTeam::Any, 120.0, 0.0, 0.0, 0.0),
            ),
            test_item(
                ItemId::HeartwoodBand,
                aimed(
                    TargetType::SelfTarget,
                    TargetTeam::SelfOnly,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                ),
            ),
        ]);
        let caster = spawn_caster(&mut app, 100.0);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 50.0;
        let enemy = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(80.0, 0.0, 0.0)),
                Team::Dire,
                Health::new(100.0),
            ))
            .id();
        let ally = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(40.0, 0.0, 0.0)),
                Team::Radiant,
                Health::new(100.0),
            ))
            .id();
        let far = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(800.0, 0.0, 0.0)),
                Team::Dire,
                Health::new(100.0),
            ))
            .id();
        give(&mut app, caster, ItemId::StormRod);
        give(&mut app, caster, ItemId::WardstoneCloak);
        give(&mut app, caster, ItemId::SparkPendant);
        give(&mut app, caster, ItemId::HeartwoodBand);

        request(&mut app, caster, 0, CastTarget::Unit(ally));
        step(&mut app);
        assert!(
            triggers(&app).is_empty(),
            "allies are not valid enemy-item targets"
        );
        assert_eq!(slot(&app, caster, 0).unwrap().cooldown.remaining, 0.0);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 100.0);

        request(&mut app, caster, 0, CastTarget::Unit(far));
        step(&mut app);
        assert!(app.world().get::<QueuedActivation>(caster).is_some());
        assert!(app.world().get::<MoveTarget>(caster).is_some());
        assert_eq!(
            slot(&app, caster, 0).unwrap().cooldown.remaining,
            0.0,
            "walking into range is free"
        );
        assert_eq!(health_of(&app, caster), 50.0);

        app.world_mut()
            .get_mut::<Transform>(caster)
            .unwrap()
            .translation = Vec3::new(700.0, 0.0, 0.0);
        step(&mut app);
        let fired = triggers(&app);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].cast_target, Some(far));
        assert_eq!(slot(&app, caster, 0).unwrap().cooldown.remaining, 5.0);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 75.0);
        assert_eq!(health_of(&app, caster), 60.0);

        app.world_mut().get_mut::<Inventory>(caster).unwrap().slots[0]
            .as_mut()
            .unwrap()
            .cooldown
            .remaining = 0.0;
        app.world_mut().get_mut::<Mana>(caster).unwrap().current = 10.0;
        request(&mut app, caster, 0, CastTarget::Unit(enemy));
        step(&mut app);
        assert!(
            triggers(&app).is_empty(),
            "unaffordable uses do not start the cooldown"
        );
        assert_eq!(slot(&app, caster, 0).unwrap().cooldown.remaining, 0.0);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 10.0);

        let point = Vec3::new(400.0, 0.0, 0.0);
        request(&mut app, caster, 1, CastTarget::Point(point));
        step(&mut app);
        assert!(
            app.world().get::<QueuedActivation>(caster).is_some(),
            "the point is out of range"
        );
        assert_eq!(slot(&app, caster, 1).unwrap().cooldown.remaining, 0.0);
        app.world_mut()
            .entity_mut(caster)
            .remove::<QueuedActivation>();

        app.world_mut()
            .get_mut::<Transform>(caster)
            .unwrap()
            .translation = Vec3::ZERO;
        request(
            &mut app,
            caster,
            1,
            CastTarget::Point(Vec3::new(100.0, 0.0, 0.0)),
        );
        step(&mut app);
        assert_eq!(triggers(&app)[0].aim, Vec3::new(100.0, 0.0, 0.0));
        assert_eq!(slot(&app, caster, 1).unwrap().cooldown.remaining, 5.0);

        request(
            &mut app,
            caster,
            2,
            CastTarget::Point(Vec3::new(300.0, 0.0, 0.0)),
        );
        step(&mut app);
        let aim = triggers(&app)[0].aim;
        let along = Vec3::new(aim.x, 0.0, aim.z).length();
        assert!(
            (along - 120.0).abs() < 1.0,
            "direction aim is projected to cast range, got {along}"
        );
        assert!(app.world().get::<QueuedActivation>(caster).is_none());

        request(&mut app, caster, 3, CastTarget::None);
        step(&mut app);
        assert_eq!(triggers(&app)[0].cast_target, Some(caster));
        assert_eq!(triggers(&app)[0].aim, Vec3::ZERO);
    }

    #[test]
    fn charges_are_per_copy_and_rejected_uses_keep_them() {
        let mut def = test_item(
            ItemId::BladeOfAsh,
            aimed(
                TargetType::NoTarget,
                TargetTeam::SelfOnly,
                0.0,
                0.0,
                0.0,
                0.0,
            ),
        );
        def.consumable = true;
        def.max_charges = Some(2);
        def.cooldown = 0.0;
        let _defs = DefinitionOverride::install([def]);

        let mut app = item_app();
        let caster = spawn_caster(&mut app, 50.0);
        let first = give(&mut app, caster, ItemId::BladeOfAsh);
        let second = give(&mut app, caster, ItemId::BladeOfAsh);
        assert_ne!(first, second, "each copy has its own instance");
        assert_eq!(slot(&app, caster, 0).unwrap().cooldown.charges, Some(2));
        assert_eq!(slot(&app, caster, 1).unwrap().cooldown.charges, Some(2));

        slot_set_charges(&mut app, caster, 0, Some(0));
        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        assert!(triggers(&app).is_empty());
        assert_eq!(
            slot(&app, caster, 0).unwrap().cooldown.charges,
            Some(0),
            "an empty copy is not spent"
        );
        assert_eq!(slot(&app, caster, 1).unwrap().cooldown.charges, Some(2));

        slot_set_charges(&mut app, caster, 0, Some(2));
        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        assert_eq!(slot(&app, caster, 0).unwrap().cooldown.charges, Some(1));
        assert_eq!(
            slot(&app, caster, 1).unwrap().cooldown.charges,
            Some(2),
            "the other copy is untouched"
        );

        request(&mut app, caster, 1, CastTarget::None);
        step(&mut app);
        assert_eq!(slot(&app, caster, 1).unwrap().cooldown.charges, Some(1));
    }

    fn slot_set_charges(app: &mut App, caster: Entity, index: usize, charges: Option<u32>) {
        app.world_mut().get_mut::<Inventory>(caster).unwrap().slots[index]
            .as_mut()
            .unwrap()
            .cooldown
            .charges = charges;
    }

    #[test]
    fn cast_time_pays_on_fire_and_follows_the_instance_across_slots() {
        let _defs = DefinitionOverride::install([test_item(
            ItemId::AegisCharm,
            aimed(TargetType::Point, TargetTeam::Any, 300.0, 0.3, 0.2, 30.0),
        )]);
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 100.0);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 50.0;
        let instance = give(&mut app, caster, ItemId::AegisCharm);
        let aim = Vec3::new(100.0, 0.0, 0.0);

        request(&mut app, caster, 0, CastTarget::Point(aim));
        step(&mut app);
        assert!(app.world().get::<Activation>(caster).is_some());
        assert!(triggers(&app).is_empty());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 100.0);
        assert_eq!(slot(&app, caster, 0).unwrap().cooldown.remaining, 0.0);

        {
            let mut inventory = app.world_mut().get_mut::<Inventory>(caster).unwrap();
            let item = inventory.slots[0].take();
            inventory.slots[2] = item;
        }
        assert!(run_until_fired(&mut app, 12));
        assert!(slot(&app, caster, 0).is_none());
        let moved = slot(&app, caster, 2).unwrap();
        assert_eq!(moved.instance, instance);
        assert_eq!(
            moved.cooldown.remaining, 5.0,
            "the cooldown lands on the copy, not the old slot"
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 70.0);
        assert!(
            app.world().get::<Activation>(caster).is_some(),
            "the channel keeps the caster after the item fires"
        );
        assert_eq!(health_of(&app, caster), 60.0);
    }

    #[test]
    fn removing_the_item_during_the_cast_point_cancels_for_free() {
        let _defs = DefinitionOverride::install([test_item(
            ItemId::ManaCrystal,
            aimed(
                TargetType::NoTarget,
                TargetTeam::SelfOnly,
                0.0,
                0.4,
                0.0,
                15.0,
            ),
        )]);
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 40.0);
        let before = health_of(&app, caster);
        give(&mut app, caster, ItemId::ManaCrystal);
        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        app.world_mut().get_mut::<Inventory>(caster).unwrap().slots[0] = None;
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 40.0);
        assert_eq!(health_of(&app, caster), before);
    }

    #[test]
    fn silence_blocks_an_ability_while_an_item_still_fires() {
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 500.0);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 40.0;
        let mut loadout = AbilityLoadout::from_abilities([
            AbilityId::Shockwave,
            AbilityId::Bolt,
            AbilityId::Execute,
            AbilityId::Bulwark,
        ]);
        for slot in &mut loadout.slots {
            slot.rank = 1;
        }
        app.world_mut().entity_mut(caster).insert(loadout);
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(StatusEffect::silenced(5.0));
        give(&mut app, caster, ItemId::VialOfLight);

        app.world_mut()
            .write_message(ActivationRequest::ability(caster, 0, CastTarget::None));
        step(&mut app);
        assert!(
            app.world().get::<Activation>(caster).is_none(),
            "silence blocks abilities"
        );
        assert_eq!(
            app.world().get::<AbilityLoadout>(caster).unwrap().slots[0]
                .cooldown
                .remaining,
            0.0
        );

        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        assert_eq!(
            health_of(&app, caster),
            220.0,
            "silence does not block items"
        );
        assert!(slot(&app, caster, 0).unwrap().cooldown.remaining > 0.0);
    }

    #[test]
    fn channeled_items_fire_on_channel_end_through_the_shared_pipeline() {
        let mut app = item_app();
        let _defs = DefinitionOverride::install([channeled_test_item(ItemId::HeartwoodBand)]);
        let caster = spawn_caster(&mut app, 200.0);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        give(&mut app, caster, ItemId::HeartwoodBand);

        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(app.world().get::<Activation>(caster).unwrap().fired);
        assert_eq!(
            health_of(&app, caster),
            500.0,
            "OnChannelEnd has not fired yet"
        );

        let mut channel_end = None;
        for _ in 0..14 {
            step(&mut app);
            for ctx in triggers(&app) {
                if ctx.trigger == AbilityTrigger::OnChannelEnd {
                    channel_end = Some(ctx);
                }
            }
        }
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(health_of(&app, caster), 700.0);
        let ctx = channel_end.expect("OnChannelEnd trigger fired");
        assert_eq!(
            ctx.source,
            crate::abilities::effects::EffectSource::Item(ItemId::HeartwoodBand)
        );
    }

    fn channel_both_effects() -> &'static [AbilityEffectEntry] {
        Box::leak(Box::new([
            AbilityEffectEntry {
                trigger: AbilityTrigger::OnChannelEnd,
                target: EffectTarget::Caster,
                effect: AbilityEffect::Heal {
                    amount: RankValue::fixed(200.0),
                    scaling: EffectScaling::None,
                },
            },
            AbilityEffectEntry {
                trigger: AbilityTrigger::OnChannelInterrupted,
                target: EffectTarget::Caster,
                effect: AbilityEffect::Heal {
                    amount: RankValue::fixed(50.0),
                    scaling: EffectScaling::None,
                },
            },
        ]))
    }

    /// Run a channelled item for a few frames, then apply `status`, and report the
    /// channel triggers that fired plus the caster's final health (started at 500).
    fn channel_with_status(status: Option<StatusEffect>) -> (Vec<AbilityTrigger>, f32) {
        let mut app = item_app();
        let def = ItemDefinition {
            effects: channel_both_effects(),
            ..channeled_test_item(ItemId::HeartwoodBand)
        };
        let _defs = DefinitionOverride::install([def]);
        let caster = spawn_caster(&mut app, 200.0);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        give(&mut app, caster, ItemId::HeartwoodBand);
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(app.world().get::<Activation>(caster).unwrap().fired);
        if let Some(status) = status {
            app.world_mut()
                .get_mut::<StatusEffects>(caster)
                .unwrap()
                .effects
                .push(status);
        }
        let mut fired = Vec::new();
        for _ in 0..14 {
            step(&mut app);
            fired.extend(
                triggers(&app)
                    .into_iter()
                    .map(|ctx| ctx.trigger)
                    .filter(|t| {
                        matches!(
                            t,
                            AbilityTrigger::OnChannelEnd | AbilityTrigger::OnChannelInterrupted
                        )
                    }),
            );
        }
        (fired, health_of(&app, caster))
    }

    #[test]
    fn silenced_item_channel_does_not_interrupt_but_mute_does() {
        let (fired, health) = channel_with_status(Some(StatusEffect::silenced(5.0)));
        assert_eq!(fired, vec![AbilityTrigger::OnChannelEnd]);
        assert_eq!(health, 700.0, "silence does not stop item channels");

        let (fired, health) = channel_with_status(Some(StatusEffect::mute(5.0)));
        assert_eq!(fired, vec![AbilityTrigger::OnChannelInterrupted]);
        assert_eq!(
            health, 550.0,
            "mute interrupts and heals the smaller amount"
        );

        let (fired, health) = channel_with_status(None);
        assert_eq!(fired, vec![AbilityTrigger::OnChannelEnd]);
        assert_eq!(health, 700.0);
    }

    #[test]
    fn stunned_item_channel_fires_only_on_channel_interrupted() {
        let (fired, health) = channel_with_status(Some(StatusEffect::stunned(5.0)));
        assert_eq!(fired, vec![AbilityTrigger::OnChannelInterrupted]);
        assert_eq!(health, 550.0);
    }

    #[test]
    fn asleep_unit_cannot_use_item() {
        let (fired, health) = channel_with_status(Some(StatusEffect::sleep(5.0)));
        assert_eq!(fired, vec![AbilityTrigger::OnChannelInterrupted]);
        assert_eq!(health, 550.0, "sleep interrupts an item channel");
        assert!(
            crate::actions::DisableRule::MuteOrStun.blocks(Some(&StatusEffects {
                effects: vec![StatusEffect::sleep(5.0)],
            }))
        );
    }

    fn radius_entry(trigger: AbilityTrigger, radius: AreaRadius) -> AbilityEffectEntry {
        AbilityEffectEntry {
            trigger,
            target: EffectTarget::UnitsInRadius {
                center: crate::abilities::definition::AreaCenter::Caster,
                radius,
                team: TargetTeam::Enemy,
            },
            effect: AbilityEffect::Heal {
                amount: RankValue::fixed(1.0),
                scaling: EffectScaling::None,
            },
        }
    }

    fn with_effects(entries: Vec<AbilityEffectEntry>) -> ItemDefinition {
        ItemDefinition {
            effects: Box::leak(entries.into_boxed_slice()),
            ..channeled_test_item(ItemId::HeartwoodBand)
        }
    }

    #[test]
    fn item_aoe_radius_counts_only_active_fixed_areas() {
        let fixed = |r: f32| AreaRadius::Fixed(RankValue::fixed(r));
        assert_eq!(with_effects(vec![]).aoe_radius(), 0.0);
        assert_eq!(
            with_effects(vec![radius_entry(AbilityTrigger::OnUse, fixed(200.0))]).aoe_radius(),
            200.0,
            "a no-target item's area counts"
        );
        assert_eq!(
            with_effects(vec![
                radius_entry(AbilityTrigger::OnUse, fixed(120.0)),
                radius_entry(AbilityTrigger::OnChannelEnd, fixed(300.0)),
                radius_entry(AbilityTrigger::OnChannelInterrupted, fixed(250.0)),
            ])
            .aoe_radius(),
            300.0,
            "the largest wins"
        );
        assert_eq!(
            with_effects(vec![radius_entry(
                AbilityTrigger::OnChannelInterrupted,
                fixed(250.0)
            )])
            .aoe_radius(),
            250.0
        );
        assert_eq!(
            with_effects(vec![
                radius_entry(AbilityTrigger::OnAttack, fixed(500.0)),
                radius_entry(AbilityTrigger::Passive, fixed(500.0)),
            ])
            .aoe_radius(),
            0.0,
            "attack and passive areas are not the active's indicator"
        );
        assert_eq!(
            with_effects(vec![radius_entry(
                AbilityTrigger::OnUse,
                AreaRadius::Ability
            )])
            .aoe_radius(),
            0.0,
            "an AreaRadius::Ability area has no radius of its own to borrow"
        );
    }

    #[test]
    fn unit_item_fires_after_target_dies_during_cast_point() {
        let mut app = item_app();
        let def = ItemDefinition {
            activation: aimed(TargetType::Unit, TargetTeam::Enemy, 500.0, 0.3, 0.0, 20.0),
            effects: heal_effect(),
            cooldown: 12.0,
            ..channeled_test_item(ItemId::HeartwoodBand)
        };
        let _defs = DefinitionOverride::install([def]);
        let caster = spawn_caster(&mut app, 200.0);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        let target = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(100.0, 0.0, 0.0)),
                Team::Dire,
                Health::new(1000.0),
            ))
            .id();
        give(&mut app, caster, ItemId::HeartwoodBand);

        request(&mut app, caster, 0, CastTarget::Unit(target));
        step(&mut app);
        assert!(
            !app.world().get::<Activation>(caster).unwrap().fired,
            "still in the cast point"
        );
        app.world_mut().despawn(target);

        let mut fired_targets = Vec::new();
        let mut contexts = Vec::new();
        for _ in 0..10 {
            step(&mut app);
            fired_targets.extend(
                app.world()
                    .resource::<Messages<crate::actions::ActionFiredEvent>>()
                    .iter_current_update_messages()
                    .map(|event| (event.target, event.aim)),
            );
            contexts.extend(triggers(&app));
        }

        assert_eq!(
            fired_targets,
            vec![(
                CastTarget::Point(Vec3::new(100.0, 0.0, 0.0)),
                Vec3::new(100.0, 0.0, 0.0)
            )],
            "fires once, at the target's last position"
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 180.0);
        let remaining = slot(&app, caster, 0).unwrap().cooldown.remaining;
        assert!(
            (11.0..=12.0).contains(&remaining),
            "cooldown started at fire and has ticked since, got {remaining}"
        );
        assert_eq!(health_of(&app, caster), 510.0);
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].cast_target, None);
    }

    #[test]
    fn item_channel_end_carries_the_cast_target() {
        let mut app = item_app();
        let def = ItemDefinition {
            activation: aimed(TargetType::Unit, TargetTeam::Enemy, 500.0, 0.1, 0.5, 0.0),
            ..channeled_test_item(ItemId::HeartwoodBand)
        };
        let _defs = DefinitionOverride::install([def]);
        let caster = spawn_caster(&mut app, 200.0);
        let target = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(100.0, 0.0, 0.0)),
                Team::Dire,
                Health::new(1000.0),
            ))
            .id();
        give(&mut app, caster, ItemId::HeartwoodBand);

        request(&mut app, caster, 0, CastTarget::Unit(target));
        let mut channel_end = None;
        for _ in 0..20 {
            step(&mut app);
            for ctx in triggers(&app) {
                if ctx.trigger == AbilityTrigger::OnChannelEnd {
                    channel_end = Some(ctx);
                }
            }
        }
        let ctx = channel_end.expect("OnChannelEnd trigger fired");
        assert_eq!(ctx.cast_target, Some(target));
    }

    #[test]
    fn distinct_instances_keep_distinct_cooldowns() {
        let mut app = item_app();
        let caster = spawn_caster(&mut app, 100.0);
        give(&mut app, caster, ItemId::VialOfLight);
        give(&mut app, caster, ItemId::VialOfLight);
        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        assert!(slot(&app, caster, 0).unwrap().cooldown.remaining > 0.0);
        assert_eq!(slot(&app, caster, 1).unwrap().cooldown.remaining, 0.0);
        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        let remaining = slot(&app, caster, 0).unwrap().cooldown.remaining;
        assert!(
            (39.0..=40.0).contains(&remaining),
            "the rejected second use does not refresh or clear the cooldown, got {remaining}"
        );
        assert_eq!(slot(&app, caster, 1).unwrap().cooldown.remaining, 0.0);
    }

    #[test]
    fn selling_an_item_mid_empower_only_changes_the_items_instance() {
        use crate::abilities::effects::EffectSource;
        use crate::status::{StatusEffect, StatusEffects, apply_status};
        let mut health = Health::new(720.0);
        let mut mana = Mana::new(320.0, 12.0);
        let mut stats = CombatStats::simple(55.0, scale::MELEE_ATTACK_RANGE, 1.1, 4.0, 3.0, 12.0);
        let mut attrs = HeroAttributes::starter();
        let mut passives = ItemPassives::default();
        passives.add_stat(PassiveStat::CleaveRatio, 0.4);
        let empower = CleaveSource::Effect(EffectSource::Ability(AbilityId::Empower));
        let source = Some(EffectSource::Ability(AbilityId::Empower));

        apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
        let mut statuses = StatusEffects::default();
        for (id, ratio, dist) in [("e_ratio", 0.16, 0.0), ("e_shape", 0.0, 650.0)] {
            apply_status(
                &mut statuses,
                &mut stats,
                StatusEffect {
                    cleave: ratio,
                    cleave_distance: dist,
                    cleave_source: source,
                    ..StatusEffect::buff(id, 30.0)
                },
                0.0,
            );
        }
        apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
        assert_eq!(stats.cleaves.iter().count(), 2);
        assert!((stats.cleaves.get(CleaveSource::Items).unwrap().ratio - 0.8).abs() < 1e-6);

        // Sell one copy mid-fight, then the other (and a third removal that cannot exist).
        remove_passives(passives, 1, &mut health, &mut mana, &mut stats, &mut attrs);
        assert!((stats.cleaves.get(CleaveSource::Items).unwrap().ratio - 0.4).abs() < 1e-6);
        remove_passives(passives, 1, &mut health, &mut mana, &mut stats, &mut attrs);
        assert!(stats.cleaves.get(CleaveSource::Items).is_none());
        let cone = stats.cleaves.get(empower).expect("empower is untouched");
        assert!((cone.ratio - 0.16).abs() < 1e-6 && cone.distance == 650.0);

        // Expiring the statuses afterwards leaves a clean set.
        for effect in std::mem::take(&mut statuses.effects) {
            crate::status::subtract_status_stat_delta(&effect, &mut stats, 0.0);
        }
        assert!(stats.cleaves.is_empty());
    }

    #[test]
    fn removing_several_copies_at_once_drops_the_items_instance() {
        let mut health = Health::new(720.0);
        let mut mana = Mana::new(320.0, 12.0);
        let mut stats = CombatStats::simple(55.0, scale::MELEE_ATTACK_RANGE, 1.1, 4.0, 3.0, 12.0);
        let mut attrs = HeroAttributes::starter();
        let mut passives = ItemPassives::default();
        passives.add_stat(PassiveStat::CleaveRatio, 0.4);
        for _ in 0..3 {
            apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
        }
        remove_passives(passives, 3, &mut health, &mut mana, &mut stats, &mut attrs);
        assert!(stats.cleaves.is_empty());
    }
}
