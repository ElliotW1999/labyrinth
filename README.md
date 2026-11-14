# Labyrinth

Desktop MOBA / action-RTS foundations (DotA / League-style), built as a native app on **[Bevy](https://bevy.org)** 0.19.

Bevy was chosen over a browser stack and over rolling a custom engine from scratch: it is open source, ships a real desktop window, and its ECS scales cleanly to hundreds of units (creeps, towers, projectiles). The project is structured as small gameplay plugins so lanes, jungle, items, and netcode can be added without rewriting the core loop.

<img width="1680" height="1049" alt="image" src="https://github.com/user-attachments/assets/22108a87-8b32-4c4c-8590-ec8fdda9abcd" />

## Wiki

- [Running](wiki/Running.md) — How to run offline, release, and UDP multiplayer
- [Controls](wiki/Controls.md) — Keyboard and mouse controls
- [Heroes](wiki/Heroes.md) — Hero roster, HeroGenerator, stat conventions, attributes
- [World](wiki/World.md) — World units, navigation and the command queue, fog of war, obstacle course
- [Status Effects](wiki/Status-Effects.md) — Buffs and debuffs
- [Damage and Resources](wiki/Damage-and-Resources.md) — Damage mechanics, healing and resources
- [Abilities](wiki/Abilities.md) — Ability ranks, architecture, reactive triggers, attack modifiers, activation and targeting
- [Ability Generator](wiki/Ability-Generator.md) — Adding abilities from CSV and generator validation
- [Story Mode](wiki/Story-Mode.md) — Room-based dungeon crawl: lives, rooms and exits, neutral and bottle slots, creep classes, RoomGenerator
- [Items](wiki/Items.md) — Shop, inventory, and adding items with ItemGenerator
- [Movement](wiki/Movement.md) — Teleport and forced movement
- [Limitations](wiki/Limitations.md) — What is not yet expressible without engine changes
- [Features](wiki/Features.md) — What's included
- [Code Layout](wiki/Layout.md) — Source tree layout and natural next layers
