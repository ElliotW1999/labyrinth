# Labyrinth

Desktop MOBA / action-RTS foundations (DotA / League-style), built as a native app on **[Bevy](https://bevy.org)** 0.19.

Bevy was chosen over a browser stack and over rolling a custom engine from scratch: it is open source, ships a real desktop window, and its ECS scales cleanly to hundreds of units (creeps, towers, projectiles). The project is structured as small gameplay plugins so lanes, jungle, items, and netcode can be added without rewriting the core loop.

<img width="1680" height="1049" alt="image" src="https://github.com/user-attachments/assets/22108a87-8b32-4c4c-8590-ec8fdda9abcd" />



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
| Right click ground | Move (hold to keep steering toward the cursor) |
| Right click enemy | Attack — walk to a free spot in range if out of range |
| G | Attack-move: path toward cursor; attack enemies in attack range |
| Shift + any order | Queue it after the current command (RMB move / attack, G, QWER, spell confirm, RMB minimap) |
| Left click | Confirm targeted spell (not used for move) |
| Space | Stop (clear move + attack + cancel spell + clear the command queue) |
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
| Map size | **15200×15200** |
| Visible X (camera) | **3600** |
| Tower attack range | **700** |
| Melee hero attack range | **150** |
| Melee creep attack range | **100** |
| Ranged creep / hero attack range | **500** |
| Hero collision / bounds | **40 / 36** |
| Melee creep collision / bounds | **36 / 24** |
| Ranged creep collision / bounds | **28 / 16** |
| Tower collision / bounds | **100 / 96** |
| Ancient collision / bounds | **80 / 72** |
| Tree collision box | **128×128** (model smaller) |
| Hero move speed | **~300** units/second |

Map layout XZ uses `scale::map` (legacy ±70 → ±7600). Body meshes use `scale::body`. Attack/cast/AoE ranges stay absolute.

Each unit carries three explicit gameplay dimensions from `dimensions::UnitDimensions` (heroes via `HeroDef::dimensions`); none are derived from mesh size:

- **Collision size** (`CollisionRadius`) — unit separation and pathing gaps. Moving units yield to stationary ones, so units never push each other (only **forceful** units shove; **phased** units ignore collision). Trees use a 128×128 AABB
- **Bounds radius** (`BoundRadius`) — range, targeting, attack reach and spells, measured edge-to-edge (`center distance − source bounds − target bounds`) by `dimensions::within_range` / `edge_distance` / `area_contains` / `cast_distance`
- **Selection bounds** (`SelectionBounds { offset, half_extents }`) — a forgiving 3D box ray-tested for hover, left-click selection and right-click / unit-target clicks. Overlaps resolve by most-centered hit, then nearest, then entity index
- Debug overlay: **F5** collision rings, **F6** bounds rings, **F7** selection boxes, **F8** all (or start with `--debug-dims`); **F9** navigation (or `--debug-nav`, see below)
- Units only **start** moving, attacking, or casting once the aim/target is within **11.5°** of facing
- **Turn rate** is radians per **0.03s** (heroes default **0.6**, creeps **0.5**)
- Attacks use **foreswing** (0–0.5s) then fire, then **backswing** (0–0.5s; cancelled by move/stop/new orders)
- **Melee** auto-attacks (range ≤ 150) deal damage instantly with a rectangular slash VFX; **ranged** attacks still use projectiles
- A **white ring** on the ground shows the local hero's attack range
- Attack speed rating: `(base AS + agility + flat bonuses) × (1 + mult)`, clamped **20–700**; APS = `(IAS/100) / BAT`
- Abilities use **cast point** then fire, then cancellable **cast backswing**

### Navigation and command queue

```
IssueCommand (input / minimap / network host)
  → CommandQueue (normal: replace queue · Shift: append)
  → order components (MoveTarget / AttackTarget / AttackMoveOrder / AbilityCastRequest)
  → AI + attack positioning (free spot within range, not the target's center)
  → plan_paths: radius-aware A* on the static NavGrid (replans only when needed)
  → movement: follow waypoints, skip ahead on line of sight, local avoidance
  → collision resolution (safety net only)
```

- **Static geometry** (map edge, trees, towers, ancients) is baked into `navigation::NavGrid`: a 32-unit clearance field rebuilt automatically when a static blocker appears or disappears (e.g. a tower dies). A cell is walkable for radius `r` when its clearance ≥ `r` + a small margin, so one grid serves every unit size. Paths are 8-connected A* (no corner cutting) string-pulled with exact sweep tests against the real shapes, so they keep a full collision radius off obstacle edges
- **Non-navigable destinations** (a click inside a tree / tower) resolve to the closest walkable point
- **Replanning** happens only when the unit has no path, the requested goal drifted > 48 units (searched replans are throttled to 4/s per unit), the nav geometry changed, or the unit is **stuck** (< 25% of expected progress over 0.5s). Stuck replans treat nearby *standing* units as temporary blockers; after repeated stuck windows next to its goal a unit accepts where it is. Units are never baked into the grid
- **Local avoidance** sidesteps units directly ahead (standing units weigh more, the chosen side is sticky to avoid jitter) and never steers into static geometry. The unit non-overlap rules still hold — moving units yield to standing ones, **phased** units ignore units
- **Attack positioning**: every mobile attacker (creeps, heroes) walks to a reachable spot inside attack range sampled on rings around the target, scored by travel distance plus penalties for spots occupied by units or reserved by other attackers this frame. Melee attackers fan out around the target instead of queueing single-file; ranged attackers stop as soon as they are in range. The goal is kept until the target moves > 64 units, the spot is taken / blocked, or it falls out of range
- **Command queue** (`unit_commands`): `UnitCommand::{Move, AttackMove, Attack, CastAbility, Stop}`. A queued command starts when the current one completes:

| Command | Completes when |
| --- | --- |
| Move | arrived (or settled next to a crowded / blocked goal) |
| Attack-move | destination reached |
| Attack | target died or was removed, or the attack was replaced by another order |
| Cast (unit / point / no target) | the ability fired (the next command cancels its backswing), or the cast was rejected / interrupted |
| Stop | immediately (and clears the queue, being a normal command) |

  Commands with a dead or missing target are skipped when they come up, and a command that never takes effect (e.g. a spell on cooldown) fails after a few frames, so nothing stalls the queue
- **F9 / `--debug-nav`**: red cells = non-navigable for the hero's radius (near the hero), cyan = path, yellow = current waypoint, magenta cross = requested destination, white ring = nav radius, orange arrow = avoidance, red ring + line = attack-position goal, red cross = unreachable goal (route ends at the closest point), green chain = queued command destinations

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
- **Seismic Slam** (Untargeted) — primary damage only (derived from its generated data); see `data/ability_pseudos/SeismicSlam.pseudo.txt`
- **Arcane Lance** (UnitTarget) — primary damage only (derived from its generated data); see `data/ability_pseudos/ArcaneLance.pseudo.txt`
- **Cataclysm** (TargetArea, ultimate) — primary damage only (derived from its generated data); see `data/ability_pseudos/Cataclysm.pseudo.txt`
- **Stone Skin** (Passive) — stub; see `data/ability_pseudos/StoneSkin.pseudo.txt`
- **Overcharge** (Toggle) — stub; see `data/ability_pseudos/Overcharge.pseudo.txt`

### Ability architecture

```text
AbilityDefinition (abilities/catalog.rs, AbilityDefinitions resource)
AbilityState      (AbilityLoadout on the hero: level, cooldown, charges, toggled)
input / AI → AbilityCastRequest → validate (usable, cooldown, mana, caster state, target, range)
           → cast point → AbilityCastEvent → definition effects + optional custom behavior
           → DamageEvent / HealEvent / StatusEffectEvent / SpawnProjectileEvent
spell projectile impact → ProjectileHitEvent → DamageEvent (+ the projectile's on_hit effects)
```

- **Definitions** (`abilities/definition.rs`) are data: behavior (active / passive / toggle), target type (no-target / unit / point / area), target team, rank-scaled cast range, AoE, cooldown, mana, cast time, and a list of reusable `AbilityEffect`s (Damage, Heal, ApplyStatus, Dispel, Dash, SpawnProjectile with `on_hit`, AttackUnitTarget, RingFx)
- **Casting** (`abilities/casting.rs`) is shared by every caster; out-of-range casts walk into range and re-request. Mana and cooldown are paid when the cast point completes
- **Custom behavior** (`abilities/custom.rs`): `app.register_ability_behavior(id, system)` runs a one-shot system with the `CastContext` after the common effects (Execute's low-HP bonus is the example)

Adding a simple ability: add an `AbilityId` variant (or use the generators), then add an arm to `catalog::builtin`:

```rust
AbilityId::Fireball => def
    .targeting(TargetType::Unit, RankValue::linear(600.0, 25.0), RankValue::ZERO)
    .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
    .timing(0.3, 0.4)
    .effect(AbilityEffect::SpawnProjectile {
        damage: RankValue::linear(150.0, 50.0),
        damage_type: DamageType::Magical,
        splash_radius: AreaRadius::Ability,
        on_hit: vec![status(StatusSpec::Stun, RankValue::fixed(1.0), EffectTargets::UnitTarget)],
    }),
```

### Adding abilities (AbilityGenerator)

Append ability kits via CSV — patches `AbilityId` + `GeneratedAbilityDef` in `src/components.rs` and writes pseudocode under `data/ability_pseudos/`. Generated data becomes an `AbilityDefinition` automatically (targeting, costs, and a primary damage effect); pseudocode extras still need effects or a custom behavior.

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
- Heroes and creeps separate by collision size without pushing each other (moving units yield to stationary ones); the **forceful** buff can shove (e.g. Vanguard Shockwave). **Phased** (Dash/Blink) ignores unit collision.
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
  abilities/          Ability definitions, cast pipeline, effects, custom behaviors, targeting UI
  items.rs            Shop, inventory, sell, actives, status effects
  net/                Offline / host / client UDP session + hero snapshots
  ai.rs               Lane following + aggro
  waves.rs            Periodic creep spawns
  camera.rs           Free camera (edge / arrows / F snap)
  dimensions.rs       Collision / bounds / selection dimensions + range math
  picking.rs          Hover, left-click selection, cursor ray picking
  input.rs            RMB move / attack, attack-move, stop (→ IssueCommand), spell ranking
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
