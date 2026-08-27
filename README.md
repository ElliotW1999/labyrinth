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
| Right click | Move, or attack if clicking an enemy |
| Left click | Move |
| Q | Dash |
| W | Shockwave (AoE damage) |
| E | Bolt (targeted projectile) |
| R | Nova (heal + large AoE) |
| Arrow keys | Pan camera |

## What's included

- Three-lane map with river, jungle pockets, towers, and ancients
- Player hero with HP / mana / gold / QWER abilities
- Creep waves that path down each lane
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
  movement.rs      Move orders
  combat.rs        Auto-attack, projectiles, death / bounty
  abilities.rs     QWER casting
  ai.rs            Lane following + aggro
  waves.rs         Periodic creep spawns
  camera.rs        Chase camera
  input.rs         Click-to-move / attack
  ui.rs            HUD
```

## Extending

Natural next layers: item shop, last-hit gold rules, fog of war, jungle neutrals, ability targeting indicators, and authoritative multiplayer (e.g. Lightyear / custom replication on top of these components).
