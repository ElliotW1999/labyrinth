# Features

## What's included

- Three-lane map with river, jungle pockets, tree obstacles, towers, ancients, and a northern obstacle course
- Fog of war with transparent grey overlay and tree line-of-sight (buildings always visible; course is fog-free)
- Esc main menu (New Game / Settings / Quit); borderless fullscreen
- Selectable heroes (Vanguard / Skirmisher / Arcanist) with unique spells and Str/Agi/Int growth
- Player hero with Str/Agi/Int, HP / mana / gold / XP / levels and rankable QWER abilities
- Item shop (gold on the shop button), 6-slot inventory between spells and minimap, timed buffs / debuffs
- Creep waves that path down each lane
- Explicit right-click attack orders and G attack-move (path to cursor, attack in range), all Shift-queueable
- Radius-aware A* pathfinding around trees / towers / ancients, local avoidance between units, and attack positioning that spreads melee attackers around their target
- Building/tree collision as a safety net; heroes/creeps separate by collision size without pushing (**forceful** shoves)
- Targeted spells: confirm aim; if out of cast range the hero walks in then casts
- Auto-attack combat with armor / magic resist mitigation; projectiles stop at the target
- Tower and creep aggro AI
- Free camera (extended far clip for the scaled map; collision ignores the camera), bottom HUD with hero icon + name / stats table / HP+MP bars (with regen) / spell bar centered on screen, inventory sell-at-cursor, shop, skill points, and clickable minimap
- Heroes/creeps show facing noses; towers/ancients/trees use multi-part meshes; hero names appear above world health bars
- Abilities spend mana and start cooldown only after the cast point completes (cancel during cast point is free)
- Optional online 1v1 (UDP host/client) while default play stays fully offline
