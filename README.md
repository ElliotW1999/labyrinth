# Labyrinth

Desktop MOBA / action-RTS foundations (DotA / League-style), built as a native app on **[Bevy](https://bevy.org)** 0.19.

Bevy was chosen over a browser stack and over rolling a custom engine from scratch: it is open source, ships a real desktop window, and its ECS scales cleanly to hundreds of units (creeps, towers, projectiles). The project is structured as small gameplay plugins so lanes, jungle, items, and netcode can be added without rewriting the core loop.

## Run

```bash
cargo run
```

Release build:

```bash
cargo run --release
```

## Controls

| Input | Action |
| --- | --- |
| Right click ground | Move (cancels pending targeted spell) |
| Right click enemy | Attack — path in if out of range (cancels pending targeted spell) |
| Left click | Confirm targeted spell (not used for move) |
| Space | Stop (clear move + attack + cancel spell) |
| Q / W / E / R | Cast ability (must be ranked first) |
| Ctrl+Q / W / E / R | Spend a skill point to rank that ability |
| Spell bar `+` | Rank up when highlighted |
| A S D Z X C | Use inventory item in that slot (actives only) |
| `$` shop button (bottom-right) | Toggle item shop panel |
| Esc | Close shop panel (also cancels targeted spells) |
| Arrow keys / screen edge | Pan camera on the XZ plane |
| F | Snap camera to hero (no continuous lock) |

### Ability ranks

- Level-ups grant skill points (start with 1 at level 1)
- Q / W / E: up to 7 ranks; rank N needs hero level ≥ 2N−1 (max at 13)
- R: up to 4 ranks; unlocked at overall levels 6 / 12 / 18 / 24
- Each rank scales damage, cooldown, range, and mana cost

### Items

- Gold shop buildings sit near each base; buy while in range via the HUD shop
- Six inventory slots on the HUD; passives apply on purchase; actives use ASDZXC
- Heroes carry a generic `StatusEffects` list for buffs / debuffs (shown on the HUD)

## What's included

- Three-lane map with river, jungle pockets, tree obstacles, towers, and ancients
- Player hero with HP / mana / gold / XP / levels and rankable QWER abilities
- Item shop, 6-slot inventory, and timed buffs / debuffs
- Creep waves that path down each lane
- Explicit right-click attack orders (no free auto-acquire for the player)
- Auto-attack combat with armor mitigation
- Tower and creep aggro AI
- Free camera, spell bar, inventory, shop button, skill points, and minimap

## Layout

```
src/
  main.rs          App entry + plugin wiring
  components.rs    Teams, vitals, abilities, unit tags
  resources.rs     Match config + shared meshes/materials
  map.rs           Battlefield + lane waypoints
  units.rs         Hero / creep / tower / ancient factories
  movement.rs      Move orders + tree collision
  combat.rs        Attack orders, projectiles, death / gold / XP
  progression.rs   Hero XP and level-up stats
  abilities.rs     QWER casting
  items.rs         Shop, inventory, actives, status effects
  ai.rs            Lane following + aggro
  waves.rs         Periodic creep spawns
  camera.rs        Free camera (edge / arrows / F snap)
  input.rs         RMB move / attack / stop / spell ranking
  ui.rs            HUD (spell bar, inventory, shop, minimap)
```

## Extending

Natural next layers: last-hit gold rules, fog of war, jungle neutrals, item recipes / sell-back, and authoritative multiplayer (e.g. Lightyear / custom replication on top of these components).
