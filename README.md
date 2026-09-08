# Labyrinth

Desktop MOBA / action-RTS foundations (DotA / League-style), built as a native app on **[Bevy](https://bevy.org)** 0.19.

Bevy was chosen over a browser stack and over rolling a custom engine from scratch: it is open source, ships a real desktop window, and its ECS scales cleanly to hundreds of units (creeps, towers, projectiles). The project is structured as small gameplay plugins so lanes, jungle, items, and netcode can be added without rewriting the core loop.

## Run

Offline (default — **no server required**):

```bash
cargo run
# same as:
cargo run -- --mode offline

# Skip select screen:
cargo run -- --hero vanguard
cargo run -- --hero skirmisher
cargo run -- --hero arcanist
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
| 1 / 2 / 3 or click card | Pick Vanguard / Skirmisher / Arcanist at start |
| Right click ground | Move (hold to keep issuing commands) |
| Right click enemy | Attack — path in if out of range (hold reissues) |
| G | Attack-move: path toward cursor; attack enemies in attack range |
| Left click | Confirm targeted spell (not used for move) |
| Space | Stop (clear move + attack + cancel spell) |
| Q / W / E / R | Cast ability (must be ranked first; targeted spells walk into range) |
| Ctrl+Q / W / E / R | Spend a skill point to rank that ability |
| Spell bar `+` | Rank up when highlighted |
| A S D Z X C | Use inventory item in that slot (actives only) |
| Esc | Main menu (New Game / Settings / Quit) |
| RMB inventory slot | Open sell menu at cursor (LMB Sell = 50% refund near shop) |
| `$` shop button (bottom-right, shows gold) | Toggle item shop panel |
| LMB shop item | Open components / buy popup (recipes hidden from grid) |
| Click outside shop / Esc | Close detail first, then shop panel |
| LMB minimap | Move camera to that map position |
| RMB minimap | Issue move order to that map position |
| Arrow keys / screen edge | Pan camera on the XZ plane |
| F | Snap camera to hero (no continuous lock) |

### Heroes

| Hero | Primary | Kit |
| --- | --- | --- |
| Vanguard | Strength | Dash, Shockwave (brief **forceful** push), Bolt (unit), Nova |
| Skirmisher | Agility | Blink, Flurry (silence+disarm), Caltrops, Execute (unit, bonus vs low HP) |
| Arcanist | Intelligence | Missile, Frost (root), Barrier (debuff immunity), Meteor (stun) |
| Warden | Strength | Bulwark, ShieldBash, Taunt, Aegis (stubs — implement later) |
| Hexer | Intelligence | HexBolt, Curse, Ward, Ritual (stubs — implement later) |

Each hero has its own base Str/Agi/Int and per-level growth.

### Adding heroes (HeroGenerator)

Append kits via CSV — the script patches `src/heroes.rs` / `src/components.rs` (and the README table). Ability **gameplay** stays stubbed until you implement casts.

```bash
python3 scripts/HeroGenerator.py data/heroes.example.csv
# preview only:
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
```

Required columns: `Hero_name`, `is_melee`, `Ability_Q/W/E/R_Name`, `base_STR/AGI/INT`, `STR/AGI/INT_per_level`. Optional: `primary`, `blurb`, `base_health`, `base_mana`, `attack_damage`, `attack_range`, `move_speed`, `armor`, `magic_resist`, and other combat fields (see `scripts/HeroGenerator.py`). Rows whose `Hero_name` already exists are skipped.

### World units

Distances use MOBA-style units (not meters):

| Measure | Target |
| --- | --- |
| Tower attack range | **700** |
| Melee hero attack range | **128** |
| Ranged hero attack range | **500** |
| Hero move speed | **~300** units/second |

Map layout, meshes, collision radii, and vision were scaled from the old prototype world by **26×** (`src/scale.rs`). Ability cast/AoE ranges use dedicated MOBA-tuned constants (not raw ×26 legacy).

- Units only **start** moving, attacking, or casting once the aim/target is within **11.5°** of facing
- **Turn rate** is radians per **0.03s** (heroes default **0.6**, creeps **0.5**)
- Attacks use **foreswing** (0–0.5s) then fire, then **backswing** (0–0.5s; cancelled by move/stop/new orders)
- Attack speed rating: `(base AS + agility + flat bonuses) × (1 + mult)`, clamped **20–700**; APS = `(IAS/100) / BAT`
- Abilities use **cast point** then fire, then cancellable **cast backswing**

### Fog of war

- Unrevealed ground shows a **transparent grey** overlay; enemy heroes/creeps are hidden outside team vision (buildings and trees stay visible)
- Heroes and towers have generous vision; creeps have half that range
- Trees block line of sight for heroes, creeps, and towers
- Northern **obstacle course** is fog-free

### Obstacle course (north / screen-top strip)

- **Firebreather** — fires orbs along its facing (does not target units)
- **Heartpiercer** — fires an orb when its pressure plate is stepped on

### Status effects

| Status | Effect |
| --- | --- |
| Silence | Blocks active abilities |
| Stun | Blocks move, attack, and abilities |
| Root | Blocks movement |
| Disarm | Blocks attacking |
| Debuff immunity | Blocks new debuffs (Barrier) |
| Phased / Forceful | Collision ignore / stronger soft-separation |

### Ability ranks

- Level-ups grant skill points (start with 1 at level 1)
- Basics: up to 7 ranks; rank N needs hero level ≥ 2N−1 (max at 13)
- Ultimates: up to 4 ranks; unlocked at overall levels 6 / 12 / 18 / 24
- Each rank scales damage, cooldown, range, and mana cost
- **Unit-targeted** spells (Bolt, Execute) require clicking a creep or hero
- **Seismic Slam** (Untargeted) — stub; see `data/ability_pseudos/SeismicSlam.pseudo.txt`
- **Arcane Lance** (UnitTarget) — stub; see `data/ability_pseudos/ArcaneLance.pseudo.txt`
- **Cataclysm** (TargetArea, ultimate) — stub; see `data/ability_pseudos/Cataclysm.pseudo.txt`
- **Stone Skin** (Passive) — stub; see `data/ability_pseudos/StoneSkin.pseudo.txt`
- **Overcharge** (Toggle) — stub; see `data/ability_pseudos/Overcharge.pseudo.txt`

### Adding abilities (AbilityGenerator)

Append ability kits via CSV — patches `AbilityId` + `GeneratedAbilityDef` in `src/components.rs` and writes pseudocode under `data/ability_pseudos/`. Cast effects stay stubbed until you implement the pseudocode in `abilities.rs`.

```bash
python3 scripts/AbilityGenerator.py data/abilities.example.csv
python3 scripts/AbilityGenerator.py data/abilities.example.csv --dry-run
```

Required: `Ability_name`, `ability_type` (`passive` / `untargeted` / `unit_target` / `target_area` / `target_point` / `toggle`), `pseudocode`. Optional scaling: `cast_point`, `cast_backswing`, `mana_cost_base`, `mana_cost_per_level` (alias `cost_per_level`), `damage_base`, `damage_per_level`, `cast_range_*`, `aoe_radius_*`, `cooldown_base`, `cooldown_per_level`, `cooldown_min`, `is_ultimate`, `max_rank`, `damage_type`. Rows whose ability name already exists are skipped.

### Attributes

- **Strength** — max HP and HP regen
- **Agility** — armor and attack-speed rating
- **Intelligence** — max mana, mana regen, and magic resist
- Each level raises Str / Agi / Int using that hero's growth rates (HUD shows IAS/APS and derived combat stats)

### Items

- Gold shop near each base; click an item for its build tree (components + recipe scraps); buy from the detail popup. Recipes are hidden from the default shop grid.
- Six inventory slots; RMB → **Sell (50%)** near shop; actives use ASDZXC
- Heroes carry a `StatusEffects` list for buffs / debuffs
- Heroes and creeps soft-separate by collision radius; the **forceful** buff pushes harder (e.g. Vanguard Shockwave). **Phased** (Dash/Blink) ignores unit push.
- **Heartwood Band** (550g) — 3 passive(s), passive-only; components: Iron Bracer
- **Spark Pendant** (750g) — 2 passive(s), active stub; components: Mana Crystal, Blade of Ash

### Adding items (ItemGenerator)

Append shop items via CSV — patches `src/items.rs` markers (enum, passives, optional active stub + recipe components) and writes active pseudocode under `data/item_actives/`.

```bash
python3 scripts/ItemGenerator.py data/items.example.csv
python3 scripts/ItemGenerator.py data/items.example.csv --dry-run
```

Required: `Item_name`, `cost`. Optional: `short_label`, `description`, `passive_1`…`passive_5` (`stat=value`, e.g. `max_health=100`), `has_active`, `active_cooldown`, `active_pseudocode`, `components` (`;`-separated item names), `color_r/g/b`. Rows whose `Item_name` already exists are skipped. Actives are stubbed until you implement the pseudocode.
## What's included

- Three-lane map with river, jungle pockets, tree obstacles, towers, ancients, and a northern obstacle course
- Fog of war with transparent grey overlay and tree line-of-sight (buildings always visible; course is fog-free)
- Esc main menu (New Game / Settings / Quit); borderless fullscreen
- Selectable heroes (Vanguard / Skirmisher / Arcanist) with unique spells and Str/Agi/Int growth
- Player hero with Str/Agi/Int, HP / mana / gold / XP / levels and rankable QWER abilities
- Item shop (gold on the shop button), 6-slot inventory between spells and minimap, timed buffs / debuffs
- Creep waves that path down each lane
- Explicit right-click attack orders and G attack-move (path to cursor, attack in range)
- Building/tree collision; heroes/creeps soft-separate by radius (**forceful** shoves harder)
- Targeted spells: confirm aim; if out of cast range the hero walks in then casts
- Auto-attack combat with armor / magic resist mitigation; projectiles stop at the target
- Tower and creep aggro AI
- Free camera (extended far clip for the scaled map), bottom HUD with hero icon placeholder / stats table / HP+MP bars / spell bar, inventory sell-at-cursor, shop, skill points, and clickable minimap
- Optional online 1v1 (UDP host/client) while default play stays fully offline

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
  movement.rs         Move orders + tree / building / unit collision
  combat.rs           Attack windup/backswing, projectiles, death / gold / XP
  progression.rs      Hero XP, attributes, and level-up growth
  abilities.rs        QWER casting with cast point/backswing
  items.rs            Shop, inventory, sell, actives, status effects
  net/                Offline / host / client UDP session + hero snapshots
  ai.rs               Lane following + aggro
  waves.rs            Periodic creep spawns
  camera.rs           Free camera (edge / arrows / F snap)
  input.rs            RMB move / attack-move / stop / spell ranking
  ui.rs               HUD (spell bar, inventory, shop gold, minimap)
scripts/
  HeroGenerator.py     CSV → add HeroId + stub AbilityIds
  ItemGenerator.py    CSV → add ItemId + passives / active stubs / components
  AbilityGenerator.py CSV → add AbilityId + GeneratedAbilityDef + pseudocode
data/
  heroes.example.csv  Sample input for HeroGenerator
  items.example.csv   Sample input for ItemGenerator
  abilities.example.csv Sample input for AbilityGenerator
  item_actives/       Active pseudocode dumps from ItemGenerator
  ability_pseudos/    Ability pseudocode dumps from AbilityGenerator
```

## Extending

Natural next layers: last-hit gold rules, jungle neutrals, item recipes, full unit replication, ability RPCs, and richer netcode (prediction / interpolation).
