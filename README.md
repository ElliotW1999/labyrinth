# Labyrinth

Desktop MOBA / action-RTS foundations (DotA / League-style), built as a native app on **[Bevy](https://bevy.org)** 0.19.

Bevy was chosen over a browser stack and over rolling a custom engine from scratch: it is open source, ships a real desktop window, and its ECS scales cleanly to hundreds of units (creeps, towers, projectiles). The project is structured as small gameplay plugins so lanes, jungle, items, and netcode can be added without rewriting the core loop.

## Run

Offline (default — **no server required**):

```bash
cargo run
# same as:
cargo run -- --mode offline
```

Release build:

```bash
cargo run --release
```

### Multiplayer (optional UDP 1v1)

Host (authoritative listen server + local Radiant hero):

```bash
cargo run -- --mode host --addr 0.0.0.0:7777
```

Client (connects as Dire; sends orders, receives hero snapshots):

```bash
cargo run -- --mode client --addr 127.0.0.1:7777
```

The HUD shows connection status. Host sim runs combat / creeps / AI; clients apply hero snapshots (~20 Hz). Spells/items are host-authoritative for now (clients move/attack via the network).

## Controls

| Input | Action |
| --- | --- |
| Right click ground | Move (hold to keep issuing commands) |
| Right click enemy | Attack — path in if out of range (hold reissues) |
| G | Attack-move: path toward cursor; attack enemies in attack range |
| Left click | Confirm targeted spell (not used for move) |
| Space | Stop (clear move + attack + cancel spell) |
| Q / W / E / R | Cast ability (must be ranked first; targeted spells walk into range) |
| Ctrl+Q / W / E / R | Spend a skill point to rank that ability |
| Spell bar `+` | Rank up when highlighted |
| A S D Z X C | Use inventory item in that slot (actives only) |
| `$` shop button (bottom-right) | Toggle item shop panel |
| Click outside shop / Esc | Close shop panel |
| Arrow keys / screen edge | Pan camera on the XZ plane |
| F | Snap camera to hero (no continuous lock) |

### Ability ranks

- Level-ups grant skill points (start with 1 at level 1)
- Q / W / E: up to 7 ranks; rank N needs hero level ≥ 2N−1 (max at 13)
- R: up to 4 ranks; unlocked at overall levels 6 / 12 / 18 / 24
- Each rank scales damage, cooldown, range, and mana cost

### Attributes

- **Strength** — max HP and HP regen
- **Agility** — armor and attack speed
- **Intelligence** — max mana, mana regen, and magic resist
- Each level raises Str / Agi / Int (shown on the HUD with derived combat stats)

### Items

- Gold shop near each base; buy in range via the HUD shop (icons + names; hover for details)
- Six inventory slots (icon + name; hover for details); actives use ASDZXC
- Heroes carry a `StatusEffects` list for buffs / debuffs

## What's included

- Three-lane map with river, jungle pockets, tree obstacles, towers, and ancients
- Player hero with Str/Agi/Int, HP / mana / gold / XP / levels and rankable QWER abilities
- Item shop, 6-slot inventory, and timed buffs / debuffs
- Creep waves that path down each lane
- Explicit right-click attack orders and G attack-move (path to cursor, attack in range)
- Unit/building collision; Dash grants a short **phased** buff (ignores creep/hero collision)
- Targeted spells (Q/E/R): confirm aim; if out of cast range the hero walks in then casts
- Auto-attack combat with armor / magic resist mitigation; projectiles stop at the target
- Tower and creep aggro AI
- Free camera, spell bar, inventory tooltips, shop, skill points, and minimap
- Optional online 1v1 (UDP host/client) while default play stays fully offline

## Layout

```
src/
  main.rs          App entry + CLI (--mode offline|host|client) + plugin wiring
  components.rs    Teams, vitals, attributes, abilities, unit tags
  resources.rs     Match config + shared meshes/materials
  map.rs           Battlefield + lane waypoints
  units.rs         Hero / creep / tower / ancient factories
  movement.rs      Move orders + tree / building / unit collision
  combat.rs        Attack orders, projectiles, death / gold / XP
  progression.rs   Hero XP, attributes, and level-up growth
  abilities.rs     QWER casting
  items.rs         Shop, inventory, actives, status effects
  net/             Offline / host / client UDP session + hero snapshots
  ai.rs            Lane following + aggro
  waves.rs         Periodic creep spawns
  camera.rs        Free camera (edge / arrows / F snap)
  input.rs         RMB move / attack-move / stop / spell ranking
  ui.rs            HUD (spell bar, inventory, shop, minimap)
```

## Extending

Natural next layers: last-hit gold rules, fog of war, jungle neutrals, item recipes / sell-back, full unit replication, ability RPCs, and richer netcode (prediction / interpolation).
