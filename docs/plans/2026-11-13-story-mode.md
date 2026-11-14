# Story Mode: dungeon-crawler game mode (rooms, lives, creep classes, neutral slot, bottle)

**Date:** 2026-11-13
**Status:** Approved 2026-11-13 (D1a, D2a, D4a and all recommended defaults). Step 0 records the baseline.
**Builds on:**
- `docs/architecture/composition.md`: the six layers and invariants I1 to I12 (this plan amends I4).
- `docs/plans/2026-11-07-item-ability-architecture-cleanup.md`: the **V** verification commands; source-specific rules stay in their own domain (I7).
- `docs/plans/2026-11-06-activation-targeting-mechanics.md`: `ActionSource::Item(slot)`, `ActionSources`, `commit_item`.
- `docs/plans/2026-11-08-healing-resource-mechanics.md`: `StatusSpec::HealOverTime`, timed `ResourceStat::ManaRegen`, `vitals::apply_heal` as the only health increase.
- `docs/plans/2026-11-08-reactive-triggers.md`: `owner_sources` walks every inventory slot.
- `docs/plans/2026-11-06-generator-shared-architecture.md`: generators share `scripts/generator_common.py`.
- The match lifecycle from PR #64: `MapState`, `MapGenSet`, `ResetMatchRequest`, `reset_match` in `Last`.

**Scope:** one branch and one PR, one commit per step. Every step compiles and passes **V**. The classic map and mode behave exactly as before.

---

## 1. Summary

| Requirement | How | New code |
|---|---|---|
| Story Mode button; classic unchanged | `GameMode` resource on the match lifecycle (`map.rs`). `ResetMatchRequest` carries the mode. Classic generation systems get `run_if(classic_mode)` | `GameMode`, menu button, `--story` CLI flag |
| Lives (4), keep starting gold and level | `HeroLives` on story heroes. Death leads to `Downed`, then respawn, or game over at 0 lives. `spawn_hero_entity` is untouched (25 000 gold, level 6) | `story/lives.rs`, `components::Downed`, `vitals::revive` |
| Safe room with shop and 2 exits; player-triggered room choice; completable rooms; data-driven | A room state machine in `story/`. Rooms are swapped in place at the map centre. Exit pads reuse the pressure-plate overlap rule. Rooms come from `data/rooms.csv` and `data/room_spawns.csv` through `scripts/RoomGenerator.py` | `src/story/`, `RoomGenerator.py` |
| Captain and Boss creep classes | `Creep.class: CreepClass` with a `CreepClassProfile` table. Existing creeps get `Normal` (identical stats) | `units.rs` profile table, `ai::Hunter` |
| Neutral slot (V) | Inventory slot index 6, separate from shop slots 0 to 5. Items are marked `slot=neutral` in `items.csv`. Obtained as a room reward pickup | `items::equip_neutral`, `ItemSlot` |
| Bottle slot (B): 120 HP and 60 mana over 3 s, 3 charges, refilled on room clear | Inventory slot index 7 holds a data-defined `bottle` item: `on_use` gives `heal_over_time` 40/s and timed `mana_regen` 20/s, both for 3 s. It is a non-consumable with `max_charges=3` | `items::refill_charges`; charges on non-consumables |

No new effect types, triggers or statuses. Items and abilities keep a single activation pipeline.

---

## 2. Findings: existing systems (reused)

**Match lifecycle**
- `src/map.rs`:
  - `MapState` / `map_requested` / `map_generated`.
  - `MapGenSet`, ordered after `heroes::HeroSpawnSet`.
  - `ResetMatchRequest { change_hero }`.
  - `reset_match` runs in `Last`. It despawns every root entity with a `Transform` (the `MatchEntityFilter`) and resets the match resources. It runs in `Last` so it cannot race queued inserts.
- Classic generators all sit in `MapGenSet` with `run_if(map_requested)`:
  - `map::spawn_map`: ground, lanes, trees, light, ancients, towers, training dummy.
  - `obstacle_course::spawn_obstacle_course`.
  - `items::spawn_shops`.
  - `waves::spawn_opening_wave`.
  - Recurring waves come from `waves::spawn_waves`, gated by `run_if(map_generated)`.
- `heroes::maybe_spawn_local_hero` spawns the hero at the hard-coded `scale::v(-44, 0.9, -44)` and calls `map.request()`.
- `menu.rs` (the Esc menu) offers New Game, Change Hero, Settings and Quit. Both New Game and Change Hero write a `ResetMatchRequest`.

**Hero, gold and levels**
- `units::spawn_hero_entity` sets `STARTING_GOLD = 25_000` and `STARTING_HERO_LEVEL = 6`, and inserts `Inventory::empty()`.
- **Heroes never die or respawn today.** `combat::despawn_dead` is `Without<PlayerHero>`. A dead hero stays at 0 HP, and nothing gates its orders (its caster aliveness is never checked in `actions::pipeline::validate_activation_requests`).
- `vitals::apply_heal` refuses dead units. Invariant I4 makes it the only place health goes up.

**Creeps and AI**
- `units::spawn_creep(team, lane, pos, ranged)` sets fixed stats (health 280/240, `GoldBounty(35)`, `XpBounty(45)`), `UnitDimensions::MELEE_CREEP` / `RANGED_CREEP` and a `LaneFollower`.
- `components::Creep { lane, ranged }` is built only in `spawn_creep`.
- `combat::CreepConvertedEvent` and `units::spawn_converted_creeps` handle convert-on-death.
- `ai.rs` runs this chain: `follow_lane_waypoints` → `acquire_targets` (range plus 80 aggro) → … → `assign_attack_positions`.
  - Creeps only acquire within attack range. Lane following supplies the walking.

**Items**
- `items.rs`:
  - `Inventory { slots: [Option<ItemInstance>; 6] }`.
  - `item_action_spec` / `commit_item`.
  - `ItemInstance::new`: charges are only set for consumables.
  - `try_buy_item`, `try_sell_item`, `combine_ready_items`, undo.
  - `apply_passives` / `remove_passives`, `tick_item_cooldowns`, `resolve_item_uses`.
  - `ItemId::inventory_hotkey` maps 0 to 5 onto "A" to "C".
- `abilities/mod.rs`:
  - `ACTION_HOTKEYS` maps QWER to `Ability(0..4)` and ASDZXC to `Item(0..6)`.
  - `begin_or_cast_from_hotkeys` handles them.
- `reactive_triggers::owner_sources` walks `inventory.slots.iter().flatten()`, so any slot index fires triggers.
- `ui.rs`:
  - `spawn_inventory_slot(index)`, `refresh_inventory_bar`, tooltips and the context menu are all index-based on `inv.slots`.
  - The shop grid is filtered by `ShopTab::accepts`.

**Healing**
- `apply_status,heal_over_time` (value = HP per second) becomes `StatusSpec::HealOverTime`. Precedent: `cold_embrace`.
- A timed `stat_modifier,mana_regen` becomes `StatusSpec::ResourceModifier`, read by `regen_mana`.
- `heal,mana` becomes `RestoreMana` (precedent: `spark_pendant`).

**Geometry and triggers**
- `obstacle_course::PressurePlate` and `tick_heartpiercer_plates` define the "a walker's collision circle overlaps a rectangle" rule.
- `navigation::sync_nav_grid` rebuilds the fixed-extent (`MAP_HALF`) `NavGrid` whenever the set of `Obstacle` shapes changes.
- `fog::FogFreeZones` holds fog-free rectangles.
- `camera::CameraFocus` holds the camera focus point.
- `teleport::apply_teleport_events` drops stale orders after a relocation using a fixed removal bundle.

**Network**
- Host and client are a 1v1 UDP prototype that only syncs hero snapshots (`net/proto.rs`).
- Every simulation plugin is gated by `run_if(is_sim_authority)`.

**Labels**
- `healthbars.rs` attaches the world name label (`WorldHeroNameLabel`) only to `HeroKind` owners.

---

## 3. Key design decisions (recommended options; see section 14)

### 3.1 Room loading: swap rooms in place (recommended) rather than extending the map

The current room is built centred on the map origin, well inside `MAP_HALF`. On transition, every room entity is despawned in `Last` and the next room is spawned the following frame. Heroes persist and are moved to the new room's entry point.

Why:
- `NavGrid` has a fixed extent and rebuilds automatically when the obstacle set changes (`sync_nav_grid`). Swapping walls and pillars needs no navigation changes.
- Fog tiles, the minimap and the camera all assume a single bounded map (`MatchConfig.map_half_extent`). A growing map would need extent changes, door gating, fog handling for far rooms and unbounded entity counts.
- Exactly one room is ever alive. Completion is therefore just "no living `RoomHostile` remains", and stale entities cannot leak between rooms.
- The teardown reuses the proven `reset_match` pattern (despawn in `Last`), minus the heroes.

What it costs: the transition is a cut rather than a walk-through. A fade can be added later without changing the model.

### 3.2 Neutral and bottle slots: reserved indices of the one `Inventory`

```rust
pub const BACKPACK_SLOTS: usize = 6;   // A S D Z X C, shop-managed
pub const NEUTRAL_SLOT: usize = 6;     // V
pub const BOTTLE_SLOT: usize = 7;      // B
pub const INVENTORY_SLOTS: usize = 8;
pub struct Inventory { pub slots: [Option<ItemInstance>; INVENTORY_SLOTS] }
```

What this reuses unchanged:
- `ActionSource::Item(slot)` / `item_action_spec` / `commit_item` / `ActionSources::current` (an instance follows its slot).
- `tick_item_cooldowns`, `resolve_item_uses`, `owner_sources` (reactive triggers and auras), `desired_toggle_passives`.
- The UI's index-based slot widgets.

The alternative was separate components (`NeutralSlot`, `BottleSlot`). It would need a new `ActionSource` variant and duplicate cooldown ticking, passive application and trigger fan-out for each. That breaks "items and abilities share one pipeline" and I7.

What changes: every shop-domain helper is limited to `slots[..BACKPACK_SLOTS]`. That covers `first_empty`, `slot_for`, `count_of`, `remove_one`, `inventory_counts`, sell, undo and combine. So the slots are "separate from the normal shop inventory slots" by rule, not by storage. `Inventory::kind_of(index) -> SlotKind { Backpack, Neutral, Bottle }` makes the rule explicit.

### 3.3 Bottle availability

The slot exists structurally in every mode, so there is one code path. It stays empty in classic. Only Story Mode puts the Bottle in it (at run start), refills it (on room completion), and shows the V/B panel.

Classic is unchanged because nothing populates slot 7 there. The refill rule depends on rooms, so it lives in `story/`. `items.rs` only provides the domain operation `refill_charges`.

### 3.4 Story Mode is offline only (v1)

The menu button is disabled unless `NetMode::Offline`, with the label "Story Mode (offline only)". All story simulation systems are `run_if(is_sim_authority)`.

Rooms, lives and pickups are authority-owned state. Replicating them would need new `ServerToClient` messages, which is out of scope.

---

## 4. New and changed types

### 4.1 Game mode (`src/map.rs`, the owner of the match lifecycle)

```rust
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameMode { #[default] Classic, Story }

pub struct ResetMatchRequest { pub change_hero: bool, pub mode: GameMode }   // + ResetMatchRequest::classic(change_hero)

pub fn classic_mode(mode: Option<Res<GameMode>>) -> bool { mode.is_none_or(|m| *m == GameMode::Classic) }
pub fn story_mode(mode: Option<Res<GameMode>>) -> bool { mode.is_some_and(|m| *m == GameMode::Story) }
```

- `reset_match` sets `*mode = request.mode` and makes `MatchEntityFilter` `pub(crate)`.
- A missing resource counts as Classic, so test apps without `MapPlugin` keep today's behaviour (the same pattern as `map_requested`).
- Dependency direction: `story` depends on `map`, `items`, `units` and `ai`. None of those depend on `story`.

### 4.2 Creep classes (`src/components.rs`, `src/units.rs`)

```rust
// components.rs
pub enum CreepClass { #[default] Normal, Captain, Boss }
pub struct Creep { pub lane: Option<Lane>, pub ranged: bool, pub class: CreepClass }
#[derive(Component)] pub struct WorldTag(pub String);        // generic world label (Captain / Boss / exit names)
#[derive(Component)] pub struct Downed { pub respawn_in: f32 } // dead hero waiting to respawn; takes no orders

// units.rs
pub struct CreepClassProfile {
    pub health_mult: f32, pub damage_mult: f32, pub armor_bonus: f32,
    pub model_scale: f32,        // scales Transform, CollisionRadius, BoundRadius, SelectionBounds
    pub bounty_mult: f32, pub xp_mult: f32,
    pub aggro_radius: f32,       // ai::Hunter radius for lane-less spawns
    pub tag: Option<&'static str>,
    pub convertible: bool,       // may be raised by convert_on_death
}
impl CreepClass { pub const fn profile(self) -> CreepClassProfile }
pub struct CreepSpawn { pub team: Team, pub position: Vec3, pub ranged: bool, pub class: CreepClass, pub lane: Option<Lane> }
pub fn spawn_creep_with(commands, assets, spawn: CreepSpawn) -> Entity   // lane Some → LaneFollower, None → ai::Hunter
pub fn spawn_creep(...) -> Entity   // unchanged signature: CreepSpawn { class: Normal, lane: Some(lane), .. }
```

Default profiles (stubs to be tuned later):

| Class | HP | Damage | Armor | Scale | Gold / XP | Aggro | Tag | Convertible |
|---|---|---|---|---|---|---|---|---|
| Normal | ×1 | ×1 | +0 | 1.0 | ×1 | 1600 | none | yes |
| Captain | ×3 | ×1.6 | +4 | 1.35 | ×3 | 1800 | "Captain" | no |
| Boss | ×12 | ×2.5 | +8 | 1.9 | ×10 | 2400 | "Boss" | no |

Classic stays exact: `Normal` multiplies by 1, adds 0 and uses scale 1, so lane creeps are bit-for-bit as before.

Conversion:
- `despawn_dead` skips conversion when `!creep.class.profile().convertible`.
- `CreepConvertedEvent.lane` becomes `Option<Lane>`. A lane-less converted creep gets a `Hunter` instead of a `LaneFollower`.

Labels:
- `healthbars.rs` attaches and syncs the world label for `Or<(With<HeroKind>, With<WorldTag>)>`. The text is the hero name or the `WorldTag`.
- The orphan cull is extended to match.

### 4.3 Hunter AI (`src/ai.rs`)

```rust
#[derive(Component)] pub struct Hunter { pub aggro_radius: f32 }
fn hunt_enemies(...)  // chain: follow_lane_waypoints, hunt_enemies, acquire_targets, ...
```

`hunt_enemies` handles creeps that have a `Hunter`, have no `AttackTarget` and are not blocked by `ActionBlocks::ORDERS`:
- It picks the nearest living enemy within `aggro_radius` that is targetable with `TargetIntent::Attack` (via `is_targetable`).
- It writes a `MoveTarget` at that enemy's position.
- `acquire_targets` then takes over once the enemy is in range. It removes the `MoveTarget` later in the same chain, just like lane following.
- With no enemy in range, the creep keeps its last move and idles.

### 4.4 Downed and revive (shared; inserted only by Story)

- `components::Downed`: while it is present the unit takes no orders and fires no actions.
  - `unit_commands::CommandContext::orders_blocked` returns true for `Has<Downed>`.
  - `actions::pipeline::validate_activation_requests` rejects a caster with `Downed` (`ActionError::CasterDisabled`).
  - The hero query in `abilities::begin_or_cast_from_hotkeys` adds `Without<Downed>`.
- `vitals::revive(health: &mut Health, mana: &mut Mana)`:
  - only for a dead unit;
  - sets current health and mana to max;
  - **amends I4:** "health goes up only in `vitals::apply_heal`, and for dead units only in `vitals::revive`".
- `teleport::drop_stale_orders(commands, entity)` is extracted from `apply_teleport_events`. It removes `MoveTarget`, `NavPath`, `NavSteering`, `AttackMoveOrder`, `AttackPositionGoal` and `ForcedMovement`, so behaviour is unchanged. Story relocation reuses it and also removes `AttackTarget`, `Activation` and `QueuedActivation`, and clears `CommandQueue`.

### 4.5 Items (`src/items.rs`, `scripts/ItemGenerator.py`)

```rust
pub enum ItemSlot { Backpack, Neutral, Bottle }       // ItemDefinition.slot (generated)
impl Inventory {
    pub fn kind_of(index: usize) -> Option<SlotKind>;
    // first_empty / slot_for / count_of / remove_one: backpack only
}
pub fn equip_neutral(inv, health, mana, stats, attrs, instance: ItemInstance) -> Result<Option<ItemInstance>, EquipError>
    // requires def.slot == Neutral; removes the old passives and returns the previous instance; applies the new passives
pub fn place_in_slot(inv, health, mana, stats, attrs, slot: usize, id: ItemId) -> bool   // empty-slot insert with passives
pub fn refill_charges(inv: &mut Inventory, slot: usize)   // charges = def.max_charges
```

- `ItemInstance::new` sets `charges: def.max_charges` for every item, not only consumables.
  - A non-consumable at 0 charges is not ready (`ActionCooldown::ready`) and is never removed (`commit_item` only spends units for consumables).
  - No current item has `max_charges` (checked against `data/items.csv`), so nothing changes for existing content.
- `try_sell_item` and the inventory context menu refuse indices `>= BACKPACK_SLOTS`. Neutral and bottle items cannot be sold.
- The shop grid shows only `ItemSlot::Backpack` items: `ShopTab::accepts` plus a slot check in `ui::sync_shop_panel_visibility`.
- `ItemId::inventory_hotkey`: 6 is "V" and 7 is "B".
- `items::spawn_item_shop(commands, mesh, material, team, position)` is extracted from `spawn_shops` and reused by the safe room.

### 4.6 Story module (`src/story/`)

```
src/story/mod.rs          StoryPlugin, StoryRun, StoryPhase, StorySet, events, scheduling
src/story/rooms.rs        RoomDefinition / RoomKind / RoomGoal / RoomReward / RoomSpawn, choose_exits (pure)
src/story/room_catalog.rs GENERATED by scripts/RoomGenerator.py: RoomId enum, ROOMS, FINAL_DEPTH
src/story/layouts.rs      RoomLayoutId (marker region read by the generator) + geometry + spawn_layout
src/story/lives.rs        HeroLives, death -> Downed -> respawn / game over
src/story/pickups.rs      ItemPickup (neutral rewards, swap)
src/story/hud.rs          lives / depth / objective panel, game over / victory overlay
```

```rust
pub struct RoomDefinition {
    pub id: RoomId, pub name: &'static str, pub kind: RoomKind,          // Safe | Combat | Elite | Boss
    pub layout: RoomLayoutId, pub min_depth: u32, pub max_depth: u32, pub weight: u32,
    pub goal: RoomGoal,                                                   // None (safe) | KillAll
    pub reward: RoomReward,                                               // None | Gold(u32) | NeutralItem
    pub shop: bool, pub spawns: &'static [RoomSpawn],
}
pub struct RoomSpawn { pub ranged: bool, pub class: CreepClass, pub count: u32, pub offset: Vec2 }
pub struct RoomLayout { pub half: Vec2, pub entry: Vec2, pub exits: [Vec2; 2], pub shop: Vec2, pub pillars: &'static [(Vec2, Vec2)] }

#[derive(Resource)] pub struct StoryRun {
    pub phase: StoryPhase, pub depth: u32, pub room: Option<RoomId>,
    pub entry: Vec3, pub exits: [Option<RoomId>; 2],
    pub hostiles_spawned: u32, pub needs_placement: bool, pub rng: StoryRng,
}
pub enum StoryPhase { Idle, Loading(RoomId), Active, Cleared, Transition(RoomId), GameOver, Victory }
#[derive(Resource, Default)] pub struct StorySeed(pub Option<u64>);   // None = time-based; tests set Some

// markers
#[derive(Component)] pub struct RoomHostile;
#[derive(Component)] pub struct ExitPad { pub slot: usize, pub half: Vec2, pub dwell: f32 }
#[derive(Component)] pub struct HeroLives { pub remaining: u32 }
#[derive(Component)] pub struct ItemPickup { pub instance: ItemInstance, pub radius: f32, pub armed: bool }

// facts
#[derive(Message)] pub struct RoomEnteredEvent { pub room: RoomId, pub depth: u32 }
#[derive(Message)] pub struct RoomCompletedEvent { pub room: RoomId, pub depth: u32 }
#[derive(Message)] pub struct StoryRunEndedEvent { pub victory: bool }

pub const STORY_STARTING_LIVES: u32 = 4;
pub const STORY_RESPAWN_SECS: f32 = 5.0;
pub const EXIT_DWELL_SECS: f32 = 0.5;
```

- `StoryRng` is a private xorshift, separate from `CombatRng`, so combat determinism tests are not disturbed.
- The overlap rule "a collision circle touches an axis-aligned rectangle" moves into `dimensions::rect_touches_circle(center, half, pos, r)`. `obstacle_course::tick_heartpiercer_plates` is refactored to call it with identical behaviour. Exit pads and pickups use it too.

---

## 5. Data formats

### 5.1 `data/items.csv`: new trailing column `slot`

The value is blank (meaning `backpack`), `neutral` or `bottle`. Every existing row gets a trailing comma.

ItemGenerator rules:
- `neutral` / `bottle`: not stackable, not consumable, no `components`, never listed as another item's component, and not shown in the shop. `cost` may be 0.
- `bottle`: needs `max_charges` and an `on_use` effect. Exactly one bottle item may exist.
- The warning "charges are only spent by consumable items" stays for backpack items and is dropped for `bottle`.
- A non-consumable `neutral` with `max_charges` gets the same warning (nothing refills it).

New rows (exact values are the coordinator's call; the CSV mechanics are fixed):

```csv
# items.csv (columns ... attribute_toggle,slot)
bottle,Bottle,utility,0,0,1,common,FALSE,,FALSE,3,"Active: restore 120 HP and 60 mana over 3 s. 3 charges, refilled when a room is completed.",BTL,0.5,,0.35,0.6,0.95,,,,,,,,,,bottle
arcane_ring,Arcane Ring,artifacts,0,0,1,uncommon,FALSE,,FALSE,,"+3 Intelligence. Active: restore 75 mana.",ARC,45,,0.55,0.45,0.95,,,,,,,,,,neutral
broom_handle,Broom Handle,artifacts,0,0,1,uncommon,FALSE,,FALSE,,"+4 Armor, +12 Attack Damage",BRM,,,0.6,0.45,0.3,,,,,,,,,,neutral
faded_broach,Faded Broach,artifacts,0,0,1,uncommon,FALSE,,FALSE,,"+150 Mana, +20 Move Speed",FBR,,,0.5,0.75,0.9,,,,,,,,,,neutral

# item_effects.csv
bottle,0,on_use,owner,apply_status,heal_over_time,40,,3,,,,
bottle,1,on_use,owner,stat_modifier,mana_regen,20,,3,,,,
arcane_ring,0,passive,owner,stat_modifier,intelligence,3,,,,,,
arcane_ring,1,on_use,owner,heal,mana,75,,,,,,
broom_handle,0,passive,owner,stat_modifier,armor,4,,,,,,
broom_handle,1,passive,owner,stat_modifier,attack_damage,12,,,,,,
faded_broach,0,passive,owner,stat_modifier,max_mana,150,,,,,,
faded_broach,1,passive,owner,stat_modifier,move_speed,20,,,,,,
```

The bottle comes to 40 × 3 = 120 HP through `HealOverTime`, which goes through `vitals::apply_heal` so heal amp and reduction apply. Mana comes to 20 × 3 = 60 through the timed `ResourceModifier` id `bottle_mana_regen`, summed in `regen_mana`. Items block on mute or stun (`DisableRule::MuteOrStun`).

### 5.2 `data/rooms.csv` and `data/room_spawns.csv` → `scripts/RoomGenerator.py` → `src/story/room_catalog.rs`

```csv
# rooms.csv
id,name,kind,layout,min_depth,max_depth,weight,reward,reward_amount,shop,description
safe_camp,Safe Camp,safe,camp,0,0,1,none,,TRUE,Rest and resupply
skirmish,Skirmish,combat,arena,1,3,2,gold,400,FALSE,
ambush,Ambush,combat,pillars,1,3,2,neutral_item,,FALSE,
captains_watch,Captain's Watch,elite,arena,2,3,1,neutral_item,,FALSE,
warlord_lair,Warlord's Lair,boss,large,4,4,1,none,,FALSE,

# room_spawns.csv (offset x,z in world units from the room centre)
room_id,spawn_index,creep,class,count,x,z
skirmish,0,melee,normal,4,0,-500
skirmish,1,ranged,normal,2,0,-800
ambush,0,melee,normal,3,-500,-400
ambush,1,ranged,normal,3,500,-600
captains_watch,0,melee,captain,1,0,-600
captains_watch,1,melee,normal,4,0,-400
captains_watch,2,ranged,normal,2,0,-850
warlord_lair,0,melee,boss,1,0,-900
warlord_lair,1,ranged,captain,2,0,-600
```

How `count` is placed: units of one row are spread on a small ring of radius `60 × count` around the offset, done in Rust by `rooms::spawn_points`.

Generator validation (reusing `generator_common` `Report`, `read_csv`, `parse_*`, `rustfmt`):
- **Ids:** they are unique.
- **Layouts:** each `layout` names a `RoomLayoutId` variant, read from the `// <room_layouts>` marker region in `src/story/layouts.rs` (same approach as `find_registered_custom_effects`).
- **Rooms:**
  - exactly one `safe` room, at depth 0..0, with `shop=TRUE`;
  - every depth `1..FINAL_DEPTH` has at least one eligible non-boss room (error if none, warning if only one);
  - `boss` rooms sit only at `FINAL_DEPTH = max(boss.max_depth)`, and there is at least one;
  - `gold` needs `reward_amount >= 1`;
  - `safe` rooms have no spawns, and every combat, elite or boss room has at least one.
- **Spawns:** `creep` is `melee` or `ranged`, `class` is `normal`, `captain` or `boss`, `count >= 1`, and the `room_id` exists.
- **Output:** it writes the whole file, starting with a "generated, do not edit" header: the `RoomId` enum, `pub static ROOMS: &[RoomDefinition]` and `pub const FINAL_DEPTH: u32`. Re-running must be byte-identical.
- **CLI:** `--check` and `--dry-run`, like the other generators.

Spawn and pad positions are checked against layout geometry by a Rust test (section 10.7), because the generator does not know layout extents.

---

## 6. Room state machine

```
                 start_story_run (MapGenSet frame, story)
Idle ──────────────────────────────────────────────► Loading(safe_camp)
                                                         │ spawn_pending_room (Update, StorySet::Spawn)
                                                         ▼
               goal == None ──────────────► Cleared (exits armed immediately; NO RoomCompletedEvent)
               goal == KillAll ───────────► Active
Active ── no living RoomHostile (hostiles_spawned > 0) ──► Cleared  (+ RoomCompletedEvent, exactly once)
Cleared ── living hero dwells EXIT_DWELL_SECS on armed ExitPad(slot) ──► Transition(run.exits[slot])
Transition(to) ── teardown_room (Last, before reset_match): despawn room entities, clear FogFreeZones ──► Loading(to), depth += 1
Cleared at depth == FINAL_DEPTH (boss room) ──► Victory (StoryRunEndedEvent{victory:true}; no exits)
any phase ── local hero death with remaining == 1 ──► GameOver (StoryRunEndedEvent{victory:false})
```

**`spawn_pending_room`** (once per `Loading`):
1. Spawns the layout: `Ground` floor plane, four wall `Obstacle` AABBs, pillar `Obstacle`s and a directional light.
2. Pushes a `FogRect` covering the room (rooms are fog-free).
3. Spawns two `ExitPad`s, with `WorldTag` labels once armed.
4. Spawns an `ItemShop` (via `items::spawn_item_shop`) if `shop`.
5. Spawns the hostiles with `spawn_creep_with(CreepSpawn { team: Dire, lane: None, class, .. })` plus `RoomHostile`, and sets `hostiles_spawned`.
6. Sets `entry` and `needs_placement`.
7. Writes `RoomEnteredEvent`.

**`place_heroes_at_entry`** runs while `needs_placement`. For every living hero (`NetworkedHero`):
- sets the transform position to `entry` (spread by collision radius when there are several heroes);
- calls `teleport::drop_stale_orders`, removes `AttackTarget`, `Activation` and `QueuedActivation`, and clears `CommandQueue`;
- sets `CameraFocus` to the entry;
- clears the flag once it has moved at least one hero. This makes it robust to the command flush that happens in the same frame as the hero spawn.

**`check_room_completion`** runs after `combat::apply_damage_events`. When the phase is `Active`, `hostiles_spawned > 0` and no `RoomHostile` with `Health::is_alive` remains, it sets `Cleared` and writes `RoomCompletedEvent`. The phase latch guarantees a single event.

**`on_room_completed`** reads `RoomCompletedEvent` and, in order:
1. calls `items::refill_charges(BOTTLE_SLOT)` for every hero;
2. grants the reward:
   - `Gold(n)` adds n to `PlayerWallet.gold`;
   - `NeutralItem` spawns an `ItemPickup` at the room centre, picked with `StoryRng` from `ItemId::all()` filtered to `slot == Neutral`, excluding the one currently equipped;
3. if `depth == FINAL_DEPTH`, sets `Victory`; otherwise assigns exits, sets the pad materials to armed and the `WorldTag` to "Name - Reward".

For the safe room, exits are assigned in `spawn_pending_room`.

**`rooms::choose_exits(depth, rng) -> [Option<RoomId>; 2]`** is a pure function: a weighted draw of two distinct rooms eligible at `depth`. If only one is eligible, the second pad stays hidden and inactive.

**`tick_exit_pads`** accumulates dwell time while a living hero overlaps an armed pad and resets it when no hero does. At `EXIT_DWELL_SECS` it sets `Transition(to)` and closes the shop panel (`ShopUiState.open = false`). Creeps never trigger pads.

**`collect_item_pickups`**: a living hero overlapping an armed pickup calls `items::equip_neutral`. A previous neutral item is dropped as a new pickup with `armed: false`, at an offset. Pickups arm once no hero overlaps them, which prevents immediate re-swaps.

**`teardown_room`** (`Last`):
- Despawns every entity matching `map::MatchEntityFilter` plus `Without<NetworkedHero>` and `Without<HealthBar>`. That covers floor, walls, pads, pickups, shop, hostiles, projectiles, zones, orbits and FX.
- Creep health bars are removed by the existing `cull_orphan_health_bars`.
- Runs `.before(map::reset_match)`.

**Lives (`lives.rs`)**, ordered after `apply_damage_events`:
- **`story_hero_deaths`**: for a hero with `HeroLives`, without `Downed`, and dead:
  - `remaining -= 1`.
  - If it reaches 0, the phase becomes `GameOver` and the hero stays dead.
  - Otherwise: insert `Downed { respawn_in: STORY_RESPAWN_SECS }`, drop all orders (as in placement), and write `StatusEffectEvent(Dispel { kind: Debuff, strength: Strong })` so DoTs cannot kill the revived hero. This uses the existing dispel path; held and permanent statuses are untouched.
- **`tick_downed_heroes`**: when the timer runs out, calls `vitals::revive`, moves the hero to `run.entry` with orders dropped, and removes `Downed`.

Lives semantics ("arcade" style): `HeroLives { remaining: 4 }`. Each death uses one life, and the death that brings it to 0 ends the run, so that is 3 respawns. See D2.

**Scheduling (`StoryPlugin`)**: all systems are `run_if(story_mode)` and `run_if(is_sim_authority)`.
- `start_story_run`: `in_set(MapGenSet)`, `run_if(map_requested)`. It resets `StoryRun` with the seed, gives `HeroLives` to every hero and puts the Bottle into `BOTTLE_SLOT` via `items::place_in_slot`.
- `StorySet::Spawn`: `spawn_pending_room` then `place_heroes_at_entry`, `.after(MapGenSet)` and `.before(SimSet::Commands)`.
- `StorySet::Rules`: `story_hero_deaths`, `tick_downed_heroes`, `check_room_completion`, `on_room_completed`, `tick_exit_pads`, `collect_item_pickups`, chained, `.after(combat::apply_damage_events)` and `.before(combat::despawn_dead)`.
- `teardown_room`: in `Last`, `.before(map::reset_match)`.

The classic gating covers `spawn_map`, `spawn_obstacle_course`, `spawn_shops`, `spawn_opening_wave` and `spawn_waves`, which all add `.run_if(map::classic_mode)`. The hazard tick systems only act on components that exist, so they need no change.

---

## 7. Input bindings

- `abilities::ACTION_HOTKEYS` grows to 12 entries: `(KeyCode::KeyV, ActionSource::Item(NEUTRAL_SLOT))` and `(KeyCode::KeyB, ActionSource::Item(BOTTLE_SLOT))`. V and B have no other binding today (checked).
- An empty slot gives `item_action_spec` → `EmptySlot` → `Unavailable`, so the keys do nothing in classic.
- Instant items (the Bottle; Arcane Ring) skip the command queue, as today. Shift queues targeted ones.
- Menu: a new `MainMenuButton::StoryMode`. Button semantics:

| Button | Writes | Notes |
|---|---|---|
| New Game | `ResetMatchRequest { change_hero: false, mode: Classic }` | Unchanged label and behaviour |
| Story Mode | `ResetMatchRequest { change_hero: false, mode: Story }` | Also acts as "restart story". Disabled unless `NetMode::Offline` |
| Change Hero | `ResetMatchRequest { change_hero: true, mode: *current }` | Keeps the current mode |

- `heroes::maybe_spawn_local_hero` takes `Option<Res<GameMode>>` and spawns at `story::SAFE_ENTRY` in Story, so the hero never flashes at the classic base.
- CLI: `--story` starts in `GameMode::Story`, for quick QA (`main.rs`, `wiki/Running.md`).

---

## 8. UI changes

**`ui.rs`**
- A `StorySlotsBar` (spawned at Startup, `BlocksWorldRmb`) sits directly above the inventory bar. It holds `spawn_inventory_slot(NEUTRAL_SLOT)` and `spawn_inventory_slot(BOTTLE_SLOT)`.
- It is visible only when `GameMode::Story` and `map_generated`.
- `refresh_inventory_bar` and the tooltips already work by index. They add a charges line: "2/3" when `cooldown.charges` is `Some`, and "Neutral" / "Bottle" captions for empty special slots.
- The context menu opens only for `index < BACKPACK_SLOTS`.
- The help text adds "V: neutral | B: bottle" in Story.

**`story/hud.rs`**
- A top-centre panel: "Lives: N", "Depth d / FINAL_DEPTH - Room name", and an objective line: "Defeat all enemies (k left)" / "Choose a path" / "Victory".
- A game over / victory overlay (ZIndex above the HUD, below the Esc menu) with two buttons:
  - "Retry Story" writes `ResetMatchRequest { mode: Story }`.
  - "Classic Game" writes `mode: Classic`.
- The game does not pause; the Esc menu still works.

**`healthbars.rs`**
- `WorldTag` labels for Captain and Boss above their (wider, because scaled) bars, and for exit pads.

**`menu.rs`**
- The Story Mode button, greyed out and not interactive when not offline.

---

## 9. Implementation steps (one commit each; run **V** after each)

0. **Baseline.** Run **V** and record the result in Implementation notes.
1. **Game mode plumbing.**
   - `GameMode`, `classic_mode` / `story_mode` and `ResetMatchRequest.mode` in `map.rs`; `reset_match` applies the mode.
   - `run_if(classic_mode)` on the five classic generators.
   - The menu Story button with the offline gate; the hero spawn point by mode; the `--story` CLI flag.
   - An empty `StoryPlugin` registered in `main.rs`.
   - Update `ResetMatchRequest` literals in `sim_tests.rs`.
   - Tests: 10.1 (classic regression, empty story world).
2. **Inventory slots.**
   - `INVENTORY_SLOTS = 8`, the slot constants and `kind_of`; shop helpers limited to the backpack; sell and context menu guards.
   - `ItemInstance::new` charges for all items.
   - `equip_neutral`, `place_in_slot`, `refill_charges`.
   - `inventory_hotkey` 6 and 7; the V and B `ACTION_HOTKEYS`.
   - Fix any `[None; 6]` or `0..6` assumptions (grep `slots` / `0..6` in `items.rs`, `ui.rs`, tests).
   - Tests: 10.2.
3. **Item data.**
   - `ItemSlot` and `ItemDefinition.slot`; ItemGenerator `slot` column and validation.
   - The new CSV rows (bottle and three neutrals); regenerate `item_catalog.rs` (I8).
   - Shop grid filter; update the test `ItemDefinition` literals in `items.rs`.
   - Tests: 10.3 and the generator tests.
4. **Creep classes.**
   - `CreepClass`, `Creep { lane: Option, class }`, `CreepClassProfile`, `CreepSpawn` / `spawn_creep_with` (`spawn_creep` delegates).
   - The conversion rule and `CreepConvertedEvent.lane: Option`.
   - `WorldTag` and the label generalisation in `healthbars.rs`.
   - Tests: 10.4.
5. **Hunter AI.** `ai::Hunter` and `hunt_enemies` in the AI chain. Tests: 10.5.
6. **Downed and revive.**
   - `components::Downed`, `vitals::revive`, `teleport::drop_stale_orders` (extracted).
   - `Downed` guards in `unit_commands`, `actions::pipeline` and the hotkeys.
   - `dimensions::rect_touches_circle` and the obstacle-course refactor.
   - Amend I4 in `composition.md`.
   - Tests: 10.6.
7. **Story core.**
   - `story/rooms.rs`, `layouts.rs`, and a hand-written `room_catalog.rs` in exactly the generator's output format.
   - `StoryRun` and the state machine, spawn / placement / teardown, exit pads, completion, rewards, bottle refill, pickups, lives / respawn / game over / victory.
   - Tests: 10.7 and 10.8.
8. **Story UI.** The V/B panel, `story/hud.rs`, the overlay and the help text. Tests: 10.9 (system initialisation, visibility by mode).
9. **RoomGenerator.**
   - `scripts/RoomGenerator.py`, `data/rooms.csv`, `data/room_spawns.csv`, `scripts/test_room_generator.py`.
   - The regenerated `room_catalog.rs` must be byte-identical to step 7's file.
   - Add the generator to **V**: `python3 scripts/RoomGenerator.py --check`, plus the determinism re-run.
10. **Documentation.** See section 11.

---

## 10. Tests

### 10.1 Mode lifecycle (`src/sim_tests.rs`; `match_app` plus `StoryPlugin` and `ItemsPlugin`)
- `classic_new_game_is_unchanged_with_story_plugin`: the existing `map_is_generated_only_after_a_hero_is_picked_and_resets_on_change_hero` passes with `StoryPlugin` added. There is no `ExitPad`, `HeroLives` or `RoomHostile`, and the bottle slot is empty.
- `story_mode_does_not_build_the_classic_map`: after Story is picked there are 0 `Tower`, 0 `Ancient`, 0 lane `Creep`, and no waves after 31 s.
- `change_hero_keeps_the_current_mode`, and `new_game_from_story_returns_to_classic`.

### 10.2 Inventory slots (`src/items.rs`)
- `inventory_has_six_backpack_slots_plus_neutral_and_bottle`.
- `purchases_and_undo_never_touch_neutral_or_bottle_slots`: with the backpack full and slots 6 and 7 occupied, buying fails and undo of a sale fails.
- `combines_ignore_items_outside_the_backpack`: a component placed in `NEUTRAL_SLOT` is not consumed by a combine.
- `selling_neutral_or_bottle_slot_is_rejected`.
- `equip_neutral_applies_passives_and_returns_previous` (armor and damage deltas exact); `equip_neutral_rejects_backpack_items`.
- `non_consumable_charged_item_spends_charges_and_is_never_removed` (3 uses, then not ready, stack still 1); `refill_charges_restores_max`.
- `v_and_b_hotkeys_map_to_neutral_and_bottle_slots` (`ACTION_HOTKEYS`, `inventory_hotkey`).

### 10.3 Item content (`src/items.rs` app tests, as for the existing `on_use` tests)
- `bottle_restores_120_hp_and_60_mana_over_3_seconds`: drop HP and mana, `ActivationRequest::item(hero, BOTTLE_SLOT)`. Check about +40 HP after 1 s, and +120 ±1 HP and +60 ±1 mana (beyond base regen, measured against a control hero) after 3.1 s. Charges are 2.
- `bottle_blocked_while_muted_or_stunned`.
- `neutral_active_fires_from_v_slot` (Arcane Ring restores 75 mana).
- `neutral_passives_and_triggers_count_from_neutral_slot` (`owner_sources` includes slot 6).
- `shop_grid_lists_only_backpack_items`.
- Generator (`scripts/test_generator_common.py` or a new item test case): neutral consumable is an error; bottle without `max_charges` is an error; two bottles is an error; neutral listed as a component is an error; bottle `max_charges` gives no warning.

### 10.4 Creep classes (`src/units.rs`, `src/combat.rs`, `src/sim_tests.rs`)
- `normal_class_lane_creep_is_identical_to_before`: HP 280/240, damage 18/15, bounty 35, XP 45, dimensions and transform scale 1, `LaneFollower` present.
- `captain_and_boss_profiles_scale_stats_dimensions_and_bounties`; `class_profiles_are_monotonic`.
- `captains_and_bosses_never_convert_on_death`; `lane_less_converted_creep_hunts`.
- `captain_and_boss_get_a_world_tag_label`.
- The existing converted-creep tests pass (lane assertion becomes `Some(Lane::Top)`).

### 10.5 Hunter (`src/sim_tests.rs`)
- `hunter_walks_to_enemy_hero_in_aggro_radius_and_attacks`.
- `hunter_ignores_enemies_outside_radius_or_invisible`.
- `hunter_does_not_override_compulsions` (feared hunter flees).

### 10.6 Downed and revive
- `vitals`: `revive_restores_full_health_and_mana_only_for_dead_units`.
- `unit_commands` / `actions`: `downed_unit_drops_issued_commands_and_activation_requests`.
- `obstacle_course`: the Heartpiercer plate still fires (behaviour of the `rect_touches_circle` refactor).
- `teleport`: the existing tests pass after the `drop_stale_orders` extraction.

### 10.7 Rooms, pure (`src/story/rooms.rs`, `layouts.rs`)
- `catalog_has_one_safe_shop_room_at_depth_zero_and_a_boss_at_final_depth`.
- `every_depth_has_an_eligible_room`.
- `choose_exits_is_deterministic_and_distinct_for_a_seed`; `choose_exits_single_candidate_leaves_second_pad_empty`.
- `every_spawn_point_entry_exit_and_shop_is_walkable_in_its_layout`: build a `NavGrid` from the layout shapes and assert `point_clear` with the largest scaled creep radius (Boss).

### 10.8 Story simulation (`src/sim_tests.rs`, `StorySeed(Some(7))`)
- `story_start_builds_safe_room`:
  - one `Ground`, one `ItemShop`, two armed pads;
  - hero at the entry with `HeroLives(4)`, a bottle in slot 7 with 3 charges, gold 25 000, level 6.
- `dwelling_on_exit_pad_loads_next_room_and_tears_down_previous`:
  - the previous floor, shop and pads are gone;
  - `RoomHostile` count equals the room's spawn sum; depth is 1; phase is `Active`;
  - the hero is at the new entry with no `MoveTarget` / `NavPath`; there is one `Ground`.
- `brief_pad_touch_does_not_transition` (under the dwell time).
- `exits_stay_inactive_while_hostiles_live`.
- `killing_all_hostiles_completes_room_once`:
  - exactly one `RoomCompletedEvent` over 10 frames;
  - bottle refilled from 0 to 3;
  - pads armed;
  - the gold reward is credited once.
- `neutral_reward_pickup_equips_then_swaps_without_ping_pong`.
- `hero_death_consumes_a_life_and_respawns_at_entry`:
  - lives drop from 4 to 3; `Downed` is set; IssueCommand and the bottle are ignored while downed;
  - after 5 s the hero has full HP and mana, stands at the entry and has no `Downed`.
- `fourth_death_is_game_over_without_respawn`.
- `boss_room_clear_is_victory`.
- `transition_cancels_in_flight_activation_and_command_queue`.

### 10.9 UI
- System-initialisation tests in the style of `healthbars.rs`.
- `story_slots_bar_hidden_in_classic`.
- `story_button_disabled_when_not_offline` (pure helper `story_available(NetMode)`).

---

## 11. Documentation updates

- **`wiki/Story-Mode.md` (new):** how to start a run, lives and respawn, rooms and exits, completion, rewards, neutral slot, bottle, creep classes, adding rooms with RoomGenerator (CSV columns, validation), offline-only.
- **`README.md`:** link Story Mode in the Wiki list.
- **`wiki/Controls.md`:** V (neutral item), B (bottle), the Story Mode menu entry, and Esc menu semantics (New Game = classic).
- **`wiki/Items.md`:**
  - the `slot` column and the neutral / bottle rules;
  - charges on non-consumables (gate the active, never regenerate, refilled only by game rules);
  - neutral items are not sold or shown in the shop.
- **`wiki/World.md`:** creep classes and the profile table; the Hunter AI; story rooms are fog-free.
- **`wiki/Features.md`, `wiki/Layout.md`:** `src/story/`, `RoomGenerator.py`, `data/rooms.csv`, `data/room_spawns.csv`.
- **`wiki/Running.md`:** the `--story` flag.
- **`wiki/Limitations.md`:**
  - Story is offline only.
  - The Bottle's heal-over-time shares the id `heal_over_time`, so a second sip within 3 s refreshes instead of stacking, and overlaps Cold Embrace.
  - Downed heroes stay visible as corpses.
  - A classic dead hero can still act (pre-existing).
- **`docs/architecture/composition.md`:** amend I4 (`vitals::revive`); note that `Downed` blocks activation in the Activation layer; story rules sit outside the six layers as match / gameplay-mode rules that only emit through existing APIs.

---

## 12. Invariants

- **S1.** Classic mode is unchanged: every classic generator is gated by `classic_mode`, Normal creeps keep their exact stats, and slots 6 and 7 are never filled outside Story.
- **S2.** Only shop-domain code touches slots `0..BACKPACK_SLOTS`. Neutral and bottle slots change only through `equip_neutral`, `place_in_slot` and `refill_charges`, which all live in `items.rs`.
- **S3.** Item effects for the neutral and bottle slots go through the single activation and effect pipeline (`ActionSource::Item`, `EffectRunner`). Story never writes `HealEvent`, `StatusEffectEvent` or damage for the bottle.
- **S4.** Health of a dead unit rises only through `vitals::revive` (amended I4). Respawn and death cleanup use the existing `StatusEffectEvent` dispel.
- **S5.** `RoomCompletedEvent` is written at most once per room (phase latch). Bottle refill and rewards happen only in its reader.
- **S6.** Room entities are despawned only in `Last` (`teardown_room` or `reset_match`). Heroes are never despawned by a transition.
- **S7.** Every hero relocation drops stale orders (`drop_stale_orders` plus attack, activation and queue).
- **S8.** Story simulation runs only with `story_mode` and `is_sim_authority`. `map`, `items`, `units` and `ai` never import `story`.
- **S9.** `room_catalog.rs` and `item_catalog.rs` change only by re-running their generators (I8).

---

## 13. Risks

- **R1, event ordering.** Completion reads `Health` after `apply_damage_events`, the same frame as the killing blow. A hero dying in that frame still loses a life. The rewards and refill reader is chained after the check, so it runs in the same frame.
- **R2, duplicate triggers.** The pad dwell resets after a transition because the pads are despawned. Pickups arm only after a hero leaves them. The completion latch is guarded by `hostiles_spawned > 0`, so the frame before the spawn flushes cannot complete the room.
- **R3, stale movement orders.**
  - Relocation removes `MoveTarget`, `NavPath`, `NavSteering`, `AttackPositionGoal`, `ForcedMovement`, `AttackTarget`, `Activation` and `QueuedActivation`, and clears the queue.
  - `NavGrid` rebuilds from the new walls in `sync_nav_grid` (the version bump forces replans).
  - The hero is placed on authored, test-checked walkable points.
- **R4, despawn races.** Teardown runs in `Last` for the same reason as `reset_match`.
- **R5, in-flight effects.** A transition despawns the hero's zones, orbits, projectiles and FX, which are root entities. This is accepted and documented.
- **R6, generated-content compatibility.**
  - The new `slot` column needs a trailing comma on every `items.csv` row and a full `item_catalog.rs` regeneration.
  - `ItemDefinition` test literals need the new field.
  - `Creep` gains fields; it is built in one place only.
- **R7, hidden 6-slot assumptions.** Grep for `0..6`, `[None, None, None, None, None, None]` and `slots.len()` in `items.rs`, `ui.rs` and the tests. The UI loops only over the bars that spawn them.
- **R8, shared HoT id.** A bottle sip refreshes rather than stacks with another `heal_over_time` (Cold Embrace). This is documented (D9).
- **R9, fog and minimap.** `FogFreeZones.rects` is cleared by teardown and re-pushed per room. The minimap keeps the map extent, so the room shows as a small area (acceptable).
- **R10, determinism.** `StoryRng` is separate from `CombatRng`, and tests pin `StorySeed`.
- **R11, circular dependencies.** `GameMode` lives in `map.rs`, `Downed` and `WorldTag` in `components.rs`, `Hunter` in `ai.rs`. Story is a leaf.

---

## 14. Decisions needing approval (recommended option first)

- **D1, room loading.** (a) Swap rooms in place at the map centre, recommended (section 3.1). (b) Extend the map with doors; needs dynamic nav, fog and minimap extents.
- **D2, lives semantics.** (a) Arcade: 4 lives, each death uses one, and the death that reaches 0 is game over (3 respawns). (b) 4 respawns, game over on the 5th death.
- **D3, bottle scope.** (a) The slot exists in all modes, but only Story grants, refills and shows it, recommended. (b) The bottle in classic too, refilled at the fountain (needs a classic refill rule).
- **D4, how neutral items are obtained.** (a) Room reward `neutral_item` spawns a pickup; walking over it equips it and swaps out the old one, recommended. (b) Auto-equip on clear (the old one is lost). (c) Also sold in the safe-room shop.
- **D5, room data source.** (a) CSV plus `RoomGenerator.py`, recommended (repo convention, validation). (b) Hand-authored `room_catalog.rs` only (steps 1 to 8 work without step 9).
- **D6, networking.** Offline only for v1.
- **D7, menu semantics.** New Game is always classic; Story Mode starts or restarts a run; Change Hero keeps the mode.
- **D8, respawn.** 5 s, at the current room's entry; on death a strong dispel of debuffs. Hostiles keep their state.
- **D9, bottle tuning.** 40 HP/s and 20 mana/s for 3 s, 0.5 s cooldown, blocked by mute or stun, refreshes rather than stacks.
- **D10, run length.** `FINAL_DEPTH = 4`, with victory on clearing the boss room.
- **D11, Captain and Boss defaults.** The table in 4.2; neither converts; tags shown on world labels.
- **D12, dead-hero rule.** `Downed` blocking is Story-only. Extending it to all dead heroes (fixing the classic "dead hero can act" bug) is a separate change.

## 15. Affected files

- **Changed:**
  - `src/main.rs`, `src/map.rs`, `src/menu.rs`, `src/heroes.rs`, `src/components.rs`, `src/units.rs`, `src/combat.rs`, `src/ai.rs`, `src/items.rs`, `src/item_catalog.rs` (generated), `src/abilities/mod.rs`, `src/actions/pipeline.rs`, `src/unit_commands.rs`, `src/vitals.rs`, `src/teleport.rs`, `src/dimensions.rs`, `src/obstacle_course.rs`, `src/waves.rs`, `src/healthbars.rs`, `src/ui.rs`, `src/sim_tests.rs`.
  - `data/items.csv`, `data/item_effects.csv`, `scripts/ItemGenerator.py`, `scripts/test_generator_common.py`.
  - `docs/architecture/composition.md`, and the `wiki/*` pages listed in section 11.
- **New:**
  - `src/story/{mod,rooms,room_catalog,layouts,lives,pickups,hud}.rs`.
  - `data/rooms.csv`, `data/room_spawns.csv`.
  - `scripts/RoomGenerator.py`, `scripts/test_room_generator.py`.
  - `wiki/Story-Mode.md`, and this plan.
