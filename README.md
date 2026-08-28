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
| Right click enemy | Attack (cancels pending targeted spell) |
| Left click | Move, or confirm targeted spell |
| Space | Stop (clear move + attack + cancel spell) |
| Q | Dash (instant) |
| W | Shockwave (instant self AoE, magic) |
| E | Arcane Bolt (targeted — show range/AoE, LMB cast) |
| R | Nova (targeted ground AoE — show range/AoE, LMB cast) |
| Arrow keys / screen edge | Pan camera on the XZ plane |
| F | Snap camera to hero (no continuous lock) |

## What's included

- Three-lane map with river, jungle pockets, tree obstacles, towers, and ancients
- Player hero with HP / mana / gold / XP / levels and QWER abilities
- Creep waves that path down each lane
- Explicit right-click attack orders (no free auto-acquire for the player)
- Auto-attack combat with armor mitigation
- Tower and creep aggro AI
- Top-down chase camera and HUD

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
  ai.rs            Lane following + aggro
  waves.rs         Periodic creep spawns
  camera.rs        Chase camera
  input.rs         Click-to-move / attack / stop
  ui.rs            HUD
```

## Extending

Natural next layers: item shop, last-hit gold rules, fog of war, jungle neutrals, ability targeting indicators, and authoritative multiplayer (e.g. Lightyear / custom replication on top of these components).
