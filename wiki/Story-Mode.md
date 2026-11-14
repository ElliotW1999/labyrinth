# Story Mode

A room-by-room dungeon crawl on the same engine as the classic game. Design and decisions: `docs/plans/2026-11-13-story-mode.md`.

## Starting a run

- Esc menu, **Story Mode**: starts (or restarts) a run with the current hero. Pick a hero first if none is chosen yet.
- `cargo run -- --story` (optionally with `--hero vanguard`) starts in Story Mode directly, for quick QA.
- **New Game** in the Esc menu is always the classic map; **Change Hero** keeps the current mode.
- Story Mode is **offline only**: the menu button is greyed out ("Story Mode (offline only)") in host and client sessions, and `--story` is ignored there.

The classic map, waves, towers and the obstacle course are not built in Story Mode, and nothing of Story Mode exists in a classic match.

## Heroes, lives and respawn

- Heroes keep what classic gives them: 25 000 starting gold and level 6.
- Each hero has **4 lives** (arcade style). A death uses one life: the hero is **downed** for 5 seconds (it takes no orders and uses no items; it stays visible as a corpse), then revives at the current room's entry with full health and mana. A strong dispel of debuffs runs on death and on revival, so damage over time cannot kill a revived hero.
- The death that uses the **last life** ends the run: the hero stays dead and the **Game Over** overlay appears with two buttons, **Retry Story** and **Classic Game**.
- Hostile creeps keep their state while the hero is down.

## Rooms

One room exists at a time, built at the centre of the map. The run starts in the **Safe Camp**: no enemies, an item shop and two exit pads. Each pad names the room it leads to and what that room pays ("Skirmish - 400 gold", "Ambush - neutral item"). **Stand on a pad for half a second** to take it; the old room is removed, the next one is built, and the heroes are moved to its entry (their gold, level, items and lives carry over). Brief touches do nothing, and creeps never trigger pads.

- Combat, elite and boss rooms start with **hostile creeps** (`Dire` team, no lane): they hunt the nearest hero within their aggro radius.
- A room is **completed** when no living hostile is left. Its exit pads arm then (until that, the pads are dim and ignore heroes), and the completion pays out once: the **bottle is refilled**, and the **reward** is given (gold to every hero, or a neutral item pickup at the room centre).
- After the safe camp, each exit draw picks two different rooms by weight among those eligible at the next depth. With a single candidate (the depth before the boss) only one pad is used.
- The run is **4 rooms deep** (`FINAL_DEPTH`). Clearing the boss room at depth 4 is **Victory**; the overlay offers the same two buttons.
- Rooms are fog-free (`FogFreeZones` covers the room). The minimap keeps the full-map extent, so a room shows as a small area on it.
- Effects the hero had in flight (zones, orbits, projectiles) are removed with the room.

The HUD (top centre) shows lives, "Depth d / 4 - Room name" and the objective ("Defeat all enemies (k left)", "Choose a path").

## Neutral slot (V) and bottle slot (B)

Both slots live in the one `Inventory` (indices 6 and 7) and use the normal item pipeline (cooldowns, mute / stun blocking, passives, reactive triggers). They are **separate from the six shop slots A S D Z X C**: the shop never buys into, sells from or combines them, and neutral and bottle items are not shown in the shop. They are only shown (above the inventory bar) in Story Mode.

- **Neutral item (V)**: obtained as a room reward. A `neutral_item` reward drops a pickup at the room centre; walk over it to equip it. The previously equipped neutral item is swapped out as a new pickup beside it (it is armed once you step off, so items never ping-pong). The offered item is never the one you wear. Items: Arcane Ring (+3 Intelligence, active: +75 mana, 45 s), Broom Handle (+4 armor, +12 attack damage), Faded Broach (+150 mana, +20 move speed).
- **Bottle (B)**: granted at the start of a run. Active: restores **120 HP and 60 mana over 3 seconds** (a 40 HP/s heal over time plus a 20 mana/s timed regeneration), 0.5 s cooldown, **3 charges**, **refilled to 3 on every room completion**. The bottle is never consumed; at 0 charges the slot is not ready. A sip while the effect runs refreshes it instead of stacking.

## Creep classes

`CreepClass` (`Normal`, `Captain`, `Boss`) scales a creep through `CreepClassProfile` in `src/units.rs`. The Captain and Boss numbers are stubs to be tuned. A Normal creep is exactly the classic lane creep.

| Class | HP | Damage | Armor | Scale | Gold / XP | Aggro radius | World label | Converts on death |
|---|---|---|---|---|---|---|---|---|
| Normal | x1 | x1 | +0 | 1.0 | x1 | 1600 | none | yes |
| Captain | x3 | x1.6 | +4 | 1.35 | x3 | 1800 | "Captain" | no |
| Boss | x12 | x2.5 | +8 | 1.9 | x10 | 2400 | "Boss" | no |

The scale applies to the model, collision, bounds and selection sizes. Captains and Bosses are never raised by `convert_on_death`.

## Adding rooms (RoomGenerator)

Rooms are data: `data/rooms.csv` and `data/room_spawns.csv` generate `src/story/room_catalog.rs` (the `RoomId` enum, `ROOMS` and `FINAL_DEPTH`).

```bash
python3 scripts/RoomGenerator.py            # regenerate src/story/room_catalog.rs
python3 scripts/RoomGenerator.py --check    # validate only
python3 scripts/RoomGenerator.py --dry-run  # print the generated Rust
```

```csv
id,name,kind,layout,min_depth,max_depth,weight,reward,reward_amount,shop,description
skirmish,Skirmish,combat,arena,1,3,2,gold,400,FALSE,
```

```csv
room_id,spawn_index,creep,class,count,x,z
skirmish,0,melee,normal,4,0,-500
```

- `rooms.csv`: `kind` safe / combat / elite / boss; `layout` is a `RoomLayoutId` variant in snake case (`camp`, `arena`, `pillars`, `large`, read from `src/story/layouts.rs`); `min_depth` / `max_depth` give the depths the room can be offered at; `weight` is the relative odds (default 1); `reward` none / gold / neutral_item (`reward_amount` for gold); `shop` spawns a shop; `description` is shown for the room in the HUD.
- `room_spawns.csv`: `creep` melee / ranged, `class` normal / captain / boss, `count` creeps spread on a ring (radius 60 x count) around the offset `(x, z)` from the room centre.
- Validation: unique ids; one safe room at depth 0 with a shop and no spawns; other rooms start at depth 1 or later and have at least one spawn; boss rooms sit exactly at `FINAL_DEPTH` (the largest boss `max_depth`) and no other kind reaches it; every depth below `FINAL_DEPTH` has an eligible combat or elite room (a warning if only one); a gold reward needs an amount; unknown layouts, creeps and classes are errors. Re-running is byte-identical.
- The generator does not know layout geometry. A Rust test checks that every entry, exit pad, shop and spawn point is walkable (with a Boss-sized collision circle) in its layout, so a new spawn offset or layout is covered by `cargo test`.
- A new layout is a `RoomLayoutId` variant plus its `RoomLayout` in `src/story/layouts.rs`.

## Code

`src/story/`: `mod.rs` (plugin, state machine, systems), `rooms.rs` (definitions, exit draw, spawn rings), `room_catalog.rs` (generated), `layouts.rs` (geometry), `lives.rs`, `pickups.rs`, `hud.rs`. Story depends on `map`, `items`, `units` and `ai`; none of those depend on it. `GameMode` lives in `map.rs` with the match lifecycle (`ResetMatchRequest` carries the mode).
