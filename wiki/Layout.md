# Code Layout

## Layout

```
src/
  main.rs             App entry + CLI (--mode, --hero) + plugin wiring
  components.rs       Teams, vitals, attributes, abilities, unit tags
  scale.rs            MOBA-style world units (ranges, MS, legacy×26 helper)
  heroes.rs           Hero roster, select UI, spawn-on-pick
  resources.rs        Match config + shared meshes/materials
  map.rs              Battlefield + lane waypoints
  units.rs            Hero / creep / tower / ancient factories
  facing.rs           Turn-rate / facing-cone helpers
  menu.rs             Esc main menu (New Game / Settings / Quit)
  fog.rs              Fog of war + grey overlay + tree LoS
  obstacle_course.rs  Firebreather / Heartpiercer training strip
  movement.rs         Path following + avoidance + tree / building / unit collision, order helpers
  navigation/         NavGrid + A* (grid.rs), local avoidance, attack positioning, path planning
  unit_commands.rs    UnitCommand / CommandQueue (Shift queuing, completion rules)
  basic_attack.rs     Basic attack pipeline (BasicAttackEvent → windup → impact → DamageEvent)
  combat.rs           DamageEvent + mitigation, spell projectiles, death / gold / XP
  progression.rs      Hero XP, attributes, and level-up growth
  actions/            Shared activation pipeline: targeting, range, cast point, channel, costs, cooldowns, charges
  abilities/          Ability definitions, ability cast layer, mechanics → triggers → effects, generated.rs, targeting UI
  items.rs            Shop, inventory, sell, actives
  status.rs           Buffs and debuffs, dispel, shields, DoT ticking (`StatusPlugin`)
  item_catalog.rs     Generated ItemDefinitions (ItemGenerator)
  displacement.rs     Forced movement: knockback / pull / dash / charge / drag / cyclone, priority, terrain stop, landing, contact / end triggers
  teleport.rs         Teleport effect: destination validation, instant relocation, completion / rejection events
  net/                Offline / host / client UDP session + hero snapshots
  ai.rs               Lane following + aggro
  waves.rs            Periodic creep spawns
  camera.rs           Free camera (edge / arrows / F snap)
  dimensions.rs       Collision / bounds / selection dimensions + range math
  picking.rs          Hover, left-click selection, cursor ray picking
  input.rs            RMB move / attack, attack-move, stop (→ IssueCommand), spell ranking
  ui.rs               HUD (spell bar, inventory, shop gold, minimap)
docs/
  architecture/composition.md  Layer model, frame order, rule matrix, worked examples
  plans/                       Implementation specs
scripts/
  HeroGenerator.py     CSV → add HeroId + stub AbilityIds
  ItemGenerator.py    items.csv + item_effects.csv → ItemDefinitions
  AbilityGenerator.py abilities.csv + ability_effects.csv → AbilityDefinitions
  generator_common.py Shared CSV validation + effect → Rust translation
data/
  heroes.example.csv  Sample input for HeroGenerator
  abilities.csv, ability_effects.csv  AbilityGenerator input
  items.csv, item_effects.csv          ItemGenerator input
  examples/           Sample CSVs (examples/invalid/ shows each validation error)
```

## Extending

Natural next layers: last-hit gold rules, jungle neutrals, item recipes, full unit replication, ability RPCs, and richer netcode (prediction / interpolation).
