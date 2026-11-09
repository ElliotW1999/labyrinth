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
| Queen of Pain | Intelligence | Shadow Strike, QoP Blink, Scream of Pain, Sonic Wave |
| Magnus | Strength | Shockwave, Empower, Skewer, Reverse Polarity |

Each hero has its own base Str/Agi/Int and per-level growth.

### Adding heroes (HeroGenerator)

Append kits via CSV — the script patches `src/heroes.rs` / `src/components.rs` (and the README table). Ability **gameplay** stays stubbed until you implement casts.

```bash
python3 scripts/HeroGenerator.py data/heroes.example.csv
# preview only:
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
```

Required columns: `Hero_name`, `is_melee`, `Ability_Q/W/E/R_Name`, `base_STR/AGI/INT`, `STR/AGI/INT_per_level`. Optional: `primary`, `blurb`, `base_health`, `base_health_regen`, `base_mana`, `base_mana_regen`, `attack_damage`, `attack_range` (`is_melee` must agree with it: base ranges up to 250, `scale::MELEE_CLASSIFICATION_MAX_RANGE`, are melee and get the instant slash and cleave, so a melee hero may have e.g. 200; the generator rejects a mismatch), `base_attack_time`, `attack_point`, `attack_backswing` (both 0 to 1.0 s), `move_speed`, `armor`, `magic_resist`, `attack_projectile_speed` (ranged heroes only, above 0: the absolute speed of the basic-attack projectile, e.g. 1500; blank derives it from the attack range; ignored with a warning on melee heroes), and `turn_rate` (above 0; blank keeps `facing::HERO_TURN_RATE`, 0.6). Blank cells use the generator defaults (see `scripts/HeroGenerator.py`). Rows whose `Hero_name` already exists are skipped.

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
  → order components (MoveTarget / AttackTarget / AttackMoveOrder / ActivationRequest)
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
- The **obstacle course** beside the Radiant ancient is fog-free

### Obstacle course (south of the Radiant ancient)

- **Firebreather** — fires orbs along its facing (does not target units)
- **Heartpiercer** — fires an orb when its pressure plate is stepped on

### Status effects

| Status | Effect |
| --- | --- |
| Silence | Blocks active abilities (items still work) |
| Mute | Blocks active items (abilities still work) |
| Sleep | Blocks abilities, movement and attacks until damaged or expired (an item channel is not interrupted) |
| Stun | Blocks move, attack, and abilities |
| Root | Blocks movement |
| Disarm | Blocks attacking |
| Debuff immunity | Blocks new debuffs (Barrier) |
| Shield | Absorbs damage before HP (`ShieldPool`; Barrier also grants bonus armor) |
| Burn | DoT via `damage_per_sec` → `DamageEvent` each frame |
| Phased / Forceful | Collision ignore / stronger soft-separation |
| Flying | Walks over trees and other props (towers and ancients still block); forced movement ignores terrain; sets the unit down on walkable ground when it ends |
| Leash / Tether | Clamps the unit inside a circle around a fixed point (Leash) or around the caster (Tether); basic-dispellable debuffs that never break; a teleport outside the circle is rejected |
| Cyclone | Lifts and holds the unit: stun + invulnerable + **untargetable**, undispellable, immune to other displacement and teleports |
| Turn rate bonus / slow | `base × (1 + Σ bonus %) × (1 − strongest slow %)` |
| Heal over time | `StatusSpec::HealOverTime` (`heal_per_sec`, ticks through `vitals::apply_heal`); the caster's heal amp is folded in once when applied; heals nothing on a dead unit |
| Heal reduction | Debuff: `heal_reduction` fraction of healing received is negated; the strongest one applies, they do not stack |
| Heal block | Debuff: prevents all healing (direct, over time, leech, regen, conversion) |
| Mana shield | Buff: diverts `absorb` of post-mitigation damage to mana at `damage_per_mana`; the `ShieldPool` absorbs first; works with `while_toggled`; absorbs nothing at 0 mana |
| Blind | Debuff on the **attacker**: `blind_miss_chance` is rolled on the shared `CombatRng` in the attack-hit stage (blinding a unit makes *its* attacks miss; it does not protect it) |
| Evasion | Buff: `StatusSpec::Evasion` / `CombatStats::evasion`; chance that attacks against the holder miss. Sources stack as `1 - prod(1 - e)`; basic-dispellable |
| Accuracy / True strike | Buff: `StatusSpec::Accuracy` / `TrueStrike` / `CombatStats::accuracy`; chance that the holder's attacks land despite blind or evasion. True strike is accuracy 1.0 and ignores both |
| Attack range | `Modifier.attack_range` / `PassiveStat::AttackRange`: flat bonus to `attack_range`, also tracked in `attack_range_bonus`, so melee units stay melee (`CombatStats::is_melee`) |
| Resource modifier | Timed regen / max / lifesteal / heal amp / cost modifier (`StatusSpec::ResourceModifier`); summed when read, never stored into stats (timed max health / mana is reconciled by `vitals::sync_status_resource_caps`) |

**Lifecycle events:** every change to a unit's statuses is reported once as a `StatusLifecycleEvent { target, source, id, kind, change }` (`Applied` includes a refresh; `Expired`, `Dispelled` and `Removed` cover expiry, dispel / immunity purge, remove-by-id and the sleep break). The only writers are `apply_status_effect_events`, `tick_status_effects` and the sleep break (`status::wake_from_sleep`, reported by `apply_damage_events`), so nothing can change a status unseen. `OnStatusApplied` / `OnStatusRemoved` read them on the holder. A status that is `reactive` (applied by a reaction, or **held** by a toggle or a `while` row) and every change made by a reactive event is silent: no lifecycle event, so held statuses and reactions never fire status triggers. Held statuses are permanent and undispellable, and are removed exactly when the condition ends, the owner dies or the source is lost.

Walking speed never drops below `scale::MIN_MOVE_SPEED` (100) however many slows stack; there is no upper cap, and units with a base speed of zero stay stationary.

### Damage mechanics (player terms → code)

| Term | Code / behavior |
| --- | --- |
| Physical / magical / pure | `DamageType`; pure skips armor and MR in `apply_damage` |
| Mitigation | Dota-style armor and MR in `combat::mitigate` |
| Outgoing amp | `CombatStats::outgoing_physical_amp` / `outgoing_spell_amp` on `DamageEvent` resolution |
| Incoming amp | `CombatStats::incoming_damage_amp` after mitigation |
| Spell amp | Item passive `PassiveStat::SpellAmp` → `outgoing_spell_amp` |
| Crit | `crit_chance` / `crit_multiplier`, rolled **once per landed attack** in `basic_attack::resolve_basic_attack_impacts` (one `CombatRng` draw, after the hit roll); the main hit is multiplied there and `DamageEvent::crit` is informational. Crit never applies to bonus damage, DoTs, cleave or splash |
| Block | Flat `CombatStats::damage_block` per hit |
| Shield / barrier | `ShieldPool` consumed in `resolve_incoming_damage` (Barrier ability: debuff immunity + armor + shield) |
| Cleave | Melee only: each cleave instance deals its ratio x the post-crit main hit to enemies in its own area (circle radius default `scale::ATTACK_CLEAVE_RADIUS` = 300, or a cone); `DamageKind::Cleave`. Instances are independent: item passives share one (ratios add), each ability with timed cleave statuses has its own, and a unit inside several areas takes each |
| Splash (attack) | `splash_damage_ratio` x the post-crit main hit to enemies around the struck unit (`splash_radius`, default `scale::ATTACK_SPLASH_RADIUS` = 250); any attack; `DamageKind::Splash`. Secondary hits never crit, roll to hit, leech or fire triggers |
| Attack bonus damage | `Damage` / mana-burn effects under `on_attack` / `on_attack_hit` become `DamageKind::AttackBonus`: no spell amp, no block, leeched like a basic attack, reflectable |
| Chain | `AbilityEffect::Chain` mechanic gate (`mechanics::chain_hops`) re-fires an `on_<name>` trigger per hop; ordinary damage rows tagged with it deal the (falloff-scaled) damage. Bolt and the `chain` CSV effect type use it |
| DoT | Status `damage_per_sec` ticks in `tick_status_effects` |
| Reflection | `damage_reflect` fraction of dealt damage returned to attacker |
| Health scaling | `EffectScaling::{TargetMissingHealth, TargetMaxHealth, TargetCurrentHealth}` |
| Distance falloff | `EffectScaling::DistanceFromCaster { falloff }` vs ability cast range |

### Healing and resources (player terms → code)

Every health increase goes through `vitals::apply_heal`; `combat::apply_damage_events` is the only place damage changes health. Rules live in `src/vitals.rs`.

| Term | Code / behavior |
| --- | --- |
| Instant heal | `AbilityEffect::Heal` → `HealEvent` → `apply_heal_events` → `heal_factor(Direct)` + `apply_heal` (clamped at max, overheal discarded, dead units are never healed) |
| Heal over time | `StatusSpec::HealOverTime` (generator `apply_status,heal_over_time`) |
| Health regeneration | `Health::regen_per_sec` + timed `ResourceStat::HealthRegen`; `regen_health` uses `HealKind::Regen` (ignores amps, respects reduction and block) |
| Healing amplification | `CombatStats::heal_amp` (provided) / `heal_received_amp` (received) plus timed modifiers; one additive bucket `1 + healer + target` |
| Healing reduction / prevention | Heal reduction (strongest, multiplicative after amp) and heal block (factor 0, wins over amp) statuses |
| Lifesteal | `CombatStats::lifesteal` on `DamageKind::BasicAttack` and `DamageKind::AttackBonus` hits: fraction of health actually removed (no overkill, nothing a shield or mana shield absorbed) |
| Spell lifesteal | `CombatStats::spell_lifesteal` on `DamageEvent::is_spell` hits |
| Health transfer | `AbilityEffect::DrainHealth` → `DamageEvent::spell(..).drained(ratio)`: damage that heals the caster for `ratio ×` health removed |
| Leech rules | Applied after the damage batch in the same frame via `HealKind::Leech`; DoT, reflected damage, `DamageKind::Cleave` and `DamageKind::Splash` never leech; a source killed in the same batch gets nothing |
| Mana restoration | `AbilityEffect::RestoreMana` (generator `heal,mana`) |
| Mana regeneration | `Mana::regen_per_sec` + timed `ResourceStat::ManaRegen`; skips dead units |
| Mana burn / drain | `AbilityEffect::DrainMana` → `ResourceDrainEvent` → `vitals::apply_resource_drain_events` (burn deals spell damage, or attack bonus damage when `ResourceDrainEvent::from_attack` is set by an attack trigger; drain gives mana to the caster, clamped to max). Blocked only by invulnerability, not debuff immunity |
| Health sacrifice | `AbilityEffect::SpendResource { Health }`: non-lethal (floor 1 HP), bypasses shields, reflection and leech |
| Resource conversion | `SpendResource { convert_to }`: health → mana, or mana → health (through the central heal path) |
| Mana shield | `StatusSpec::ManaShield` → `combat::absorb_with_mana_shield` |
| Mana cost modification | `CombatStats::mana_cost_amp` + timed `ResourceStat::ManaCostAmp`, applied once in `ActionSources::spec` (abilities, items, toggle upkeep); evaluated again when the cast fires |
| Max health / mana modifier | Permanent: `Health::change_max` with `MaxChangeRule::KeepMissing` (items). Timed: `PreserveRatio` via `sync_status_resource_caps` |

### Ability ranks

- Level-ups grant skill points (start with 1 at level 1)
- Basics: up to 7 ranks; rank N needs hero level ≥ 2N−1 (max at 13)
- Ultimates: up to 4 ranks; unlocked at overall levels 6 / 12 / 18 / 24
- Each rank scales damage, cooldown, range, and mana cost
- **Unit-targeted** spells (Bolt, Execute) require clicking a creep or hero
- **Blink**, **Seismic Slam**, **Arcane Lance**, **Cataclysm**, **Stone Skin**, **Overcharge** are generated from `data/abilities.csv` + `data/ability_effects.csv` (Stone Skin's passive armor is applied in `abilities/passive.rs`; Overcharge is pure data: `toggle_mana_per_sec` plus `while_toggled` statuses). Seismic Slam, Arcane Lance, Cataclysm, Stone Skin and Overcharge are test content: they are not in any hero loadout

### Ability architecture

The six composition layers (activation, mechanics, triggers, effects, modifiers, gameplay rules), their interfaces, frame order, the gameplay-rule matrix, worked examples and the checklist for adding a mechanic live in [`docs/architecture/composition.md`](docs/architecture/composition.md).

Mechanics decide **when** something happens and **who** is involved; effects decide **what** happens to them.

```text
AbilityDefinition (abilities/catalog.rs, AbilityDefinitions resource)
AbilityState      (AbilityLoadout on the hero: level, cooldown, charges, toggled)
input / AI → ActivationRequest { caster, source: Ability(slot) | Item(slot), target }   (actions/)
           → validate (source available, cooldown + charges, mana, caster state, target, range)
           → cast point → ActionFiredEvent → AbilityCastEvent / item OnUse → channel → backswing
           → mechanics: Dash / Projectile / AttackUnitTarget / RingFx / custom behavior
           → AbilityTriggerEvent(TriggerContext)          OnCast, OnProjectileHit, OnImpact, OnChannelEnd / OnChannelInterrupted, WhileToggled, Custom("on_…")
                                                          (every context is built by TriggerContext::new)
           → effect entries whose trigger matches → resolve EffectTarget
           → DamageEvent / HealEvent / StatusEffectEvent  (shared systems apply mitigation, immunity, stats;
                                                          statuses live in status.rs and change only through StatusEffectEvent)
spell projectile → ProjectileHitEvent { primary, splashed } → OnProjectileHit (struck) / OnImpact (landed)
```

- **Definitions** (`abilities/definition.rs`) are data: behavior, target type, target team, rank-scaled cast range, AoE, cooldown, mana, cast time, a list of `AbilityMechanic`s, and a list of `AbilityEffectEntry { trigger, target, effect }`
- **Mechanics** (`abilities/mechanics.rs`) move units and spawn projectiles. They never apply consequences; they write an `AbilityTriggerEvent` whose `TriggerContext` carries the caster, cast target, trigger unit, affected units, and trigger point
- **Effects** (`abilities/effects.rs`): `resolve_ability_triggers` runs every entry tagged with the fired trigger. Targets: `Caster`, `CastTarget`, `TriggerUnit`, `AffectedUnits` (e.g. splash victims), `UnitsInRadius`. Effects: `Damage` (optional `EffectScaling`), `Heal`, `ApplyStatus`, `Dispel`, and `Custom { id }` for systems registered with `app.register_custom_effect(id, system)`
- **Projectiles** (`combat.rs`) know their source, target / aim, speed, and collision / splash radius only
- **Activation** (`actions/`) is shared by abilities and active items: target types (no target, self, unit, point, area, direction), team rules (enemy / ally / both / self), range, cast point, channel, mana cost, cooldown, and charges. Out-of-range activations walk into range and re-request. Mana and cooldown are paid when the action fires. `ActionSources` asks the ability layer (learned / castable, rank) or the item layer (the inventory slot holds that active, per-copy cooldown and charges). Items carry an instance id, so two copies keep separate state and an in-flight use follows its copy if it changes slots
- **Casting** (`abilities/casting.rs`) is the ability layer on top: learned / passive gating, the `ActionSpec` at the slot's rank, toggle state, and turning fired actions into `AbilityCastEvent`s and channel ends into `OnChannelEnd` triggers (`action_channel_end_triggers`, for both abilities and items). Abilities (`DisableRule::SilenceOrStun`, via `can_cast()`) are blocked and interrupted by silence, stun or sleep (hex counts as silence); mute alone does not stop them. Items (`DisableRule::MuteOrStun`) are blocked and interrupted by mute or stun; silence and sleep do not stop them
- **Passive abilities** (`abilities/passive.rs`): `ability_type=passive` with `passive` effect rows (`stat_modifier` → `AbilityEffect::PassiveStat`) apply on loadout init and rank-up, mirroring item passives. **Toggle upkeep:** `ability_type=toggle` flips `AbilityState.toggled` on each cast (cooldown starts each flip); a toggle drains `toggle_mana_per_sec` while on and turns off at 0 mana (no cooldown). Its `while_toggled` `stat_modifier` rows are ordinary permanent, undispellable statuses applied and removed through `StatusEffectEvent` by `tick_toggle_upkeep`; ranking up while on keeps the old values until the toggle is cycled
- **Derived triggers:** `AbilityEffect::Proc { chance, then }` and `Counter { every, then }` re-fire the same context under the custom trigger `then`. `resolve_ability_triggers` drains a FIFO queue, so derived contexts resolve in the same call; each carries `depth` and gates stop at `MAX_TRIGGER_DEPTH` (2). `Proc` makes one `CombatRng` draw per execution (one roll covers every entry under `then`); `Counter` state lives in `TriggerCounters`, per source and `then`
- **Chain gate:** `AbilityEffect::Chain { jumps, radius, falloff, team, then }` jumps from each start unit to the nearest valid unit (alive, targetable, not yet hit, ties broken by `Entity`) and re-fires the context under `then` per hop with `TriggerContext::magnitude *= falloff^hop`; it works for abilities and items. Hops clear the attack metadata, so chained damage is spell damage. Hops inherit the reaction of the context that started the chain (reactive hop damage fires no damage triggers), are **not** spell hits (`OnSpellHit` counts only the depth-0 target), and non-reactive hop damage does fire `on_damage_dealt` / `on_damage_taken` on the next frame
- **Attack triggers:** `abilities/attack_triggers.rs` fires `OnAttack` (every release) and `OnAttackHit` (landed attacks only) for every distinct item and every learned ability (rank above 0), with `TriggerContext::attack = Some(AttackMeta { extra, crit })`
- **Reactive triggers:** `abilities/reactive_triggers.rs` holds small dispatcher systems. Each reads one authoritative message and writes ordinary `AbilityTriggerEvent`s into the same queue (no dispatcher applies an effect). All of them share `owner_sources` (items in slot order, one per distinct item, then learned abilities), so every distinct item and learned ability gets one trigger per occurrence.
- **Autocast:** not implemented (no CSV column or AI heuristic yet)

#### Reactive triggers

| Trigger (CSV) | Fires when | Source message | Resolves |
| --- | --- | --- | --- |
| `on_damage_dealt` | the owner's damage hurt a unit (trigger unit = victim) | `DamageAppliedEvent` | next frame |
| `on_damage_taken` | the owner was hurt (trigger unit = attacker; `attacker` target) | `DamageAppliedEvent` | next frame |
| `on_health_below:P` | one hit took the owner from at or above P% to below it and it survived; rearms once health is back above P% (stateless crossing) | `DamageAppliedEvent` | next frame |
| `on_kill` / `on_death` | the hit that took health from above zero to zero (killer is the source of that hit, DoT sources included); once per death | `DamageAppliedEvent` | next frame |
| `on_spell_cast` | the owner finished casting an ability; every other source reacts, never the cast ability itself; item uses do not count | `AbilityCastEvent` | same frame |
| `on_spell_hit` | an effect of the owner's cast resolved onto an enemy (one per enemy per cast context, damage or not; derived, reactive and attack contexts never count) | `SpellHitEvent` | next frame |
| `on_status_applied[:buff\|debuff\|id]` / `on_status_removed[:...]` | a status was applied to / left the owner | `StatusLifecycleEvent` | applied next frame, expiry same frame |
| `on_cooldown_ready` | the source's cooldown finished (never for a zero cooldown or a charge restore) | `CooldownReadyEvent` | same frame |
| `on_interval:S` | every S seconds (at least 0.1) of the owner's life; copies of an item share one timer; paused while dead; at most once per frame | `TriggerTracking` | same frame |
| `on_move` | the owner moved; `amount` is the distance this frame (teleports excluded, forced movement counts) | `TriggerTracking` | same frame |
| `on_enemy_enter:R` / `on_enemy_leave:R` | a living, targetable enemy came within / left R; units in range when the source is first held count as entering; a death or despawn drops silently | `TriggerTracking` | same frame |
| `while:<condition>` | held `apply_status` / `stat_modifier` rows while the condition is true (`health_below:P`, `moving`, `stationary`, `enemies_near:R:N`, `allies_near:R:N`, `has_status:id`) | `sync_conditional_statuses` | same frame |

**The reaction rule.** Triggers caused by damage or statuses (`on_damage_dealt`, `on_damage_taken`, `on_health_below`, `on_kill`, `on_death`, `on_status_applied`, `on_status_removed`) set `TriggerContext::reaction`, and derived contexts inherit it. Under a reaction `EffectRunner` stamps `reactive` on every `DamageEvent`, `ResourceDrainEvent`, `StatusEffectEvent` and applied `StatusEffect`. Reactive damage and statuses never fire dealt, taken or status triggers, so two thorn items cannot ping-pong. `PerformAttack` does nothing under a reaction. Kill, death and the health crossing still fire for reactive causes because each is a bounded edge. DoT ticks never fire dealt or taken but count for kill, death and low health. Custom effects receive `ctx.reaction` and must pass it on (a custom effect that emits damage or statuses should set `.reactive(ctx.reaction)`).

**Frame delays.** Damage-class reactions, status application and spell hits resolve one frame after the cause (the same delay `PerformAttack` has); a reaction therefore cannot cancel the lethal hit. Cast, attack, cooldown, interval, move, proximity and expiry reactions resolve in the same frame.

**Gates.** `Proc { chance, then, pseudo }` (a pseudo-random proc raises the chance by a constant after every failure and resets on success, so the long-run rate is still `chance` with short streaks; failures are kept in `TriggerCounters`, per owner, source and `then`), `Counter { every, then }`, and `Accumulate { threshold, then }`, which sums `TriggerContext::amount` (damage, distance, or 1) and fires once each time the sum reaches the threshold, keeping the remainder. `amount` is post-mitigation damage for damage triggers, the landed damage for `on_attack_hit`, the distance for `on_move`, otherwise 1. All gates stop at `MAX_TRIGGER_DEPTH`.

### Attack modifiers

```text
request_basic_attacks -> windup -> tick_attack_swings -> BasicAttackReleaseEvent { extra: false } (sets cooldown)
release_extra_attacks (ExtraAttackEvent)  -> BasicAttackReleaseEvent { extra: true }  (no cooldown / swing / AttackTarget change)
resolve_attack_releases (melee: impact now, extra skips the range check | ranged: BasicAttackInFlight)
resolve_basic_attack_impacts = the attack-hit stage:
  validate -> hit roll -> crit roll -> main DamageEvent -> BasicAttackLandedEvent -> cleave / splash
attack_triggers (OnAttack / OnAttackHit) -> resolve_ability_triggers (Proc / Counter / PerformAttack)
```

Rules (binding):

- **Order:** base damage (snapshotted at windup; extra attacks use `attack_damage x ratio`), crit on the main hit only, cleave / splash = ratio x post-crit main hit, then `apply_damage_events` (amp, shield, block (main hit only), armor, incoming amp, mana shield, HP), then leech from health actually removed
- **Hit roll:** `miss = (1 - (1 - blind)(1 - evasion)) x (1 - accuracy)`; blind from the attacker, evasion from the target, accuracy from the attacker. One `CombatRng` draw; a miss stops everything (no damage, no landed event, no secondary hits, no `OnAttackHit`). An attacker that despawned mid-flight has no blind, accuracy or crit and does not cleave
- **Stacking:** item / passive stats are additive and undo exactly; chances combine as `1 - prod(1 - c)`; `crit_multiplier` passive is a bonus on top of 2.0
- **RNG:** only `CombatRng`, in fixed order (hit, crit per impact in message order; one draw per `Proc` in queue order); `CombatRng::roll` draws nothing at chance <= 0 or >= 1. A `pseudo_proc` row uses the same single draw per execution, with a chance that grows with failures (PRD)
- **Recursion guards:** `PerformAttack` does nothing under a trigger caused by an extra attack (extra attacks still fire `OnAttack`, `OnAttackHit`, procs and counters) and nothing under a reaction (see Reactive triggers); derived triggers stop at depth 2; cleave, splash, DoT, reflection and burn never send attack events; one trigger per distinct item and per learned ability per attack. Extra attacks start the frame after the effect that requested them

Recipes (CSV rows, `trigger,target,effect_type,effect_id,value,...`):

| Mechanic | Rows |
| --- | --- |
| Crit | passive `crit_chance` (fraction) and `crit_multiplier` (bonus on 2.0) |
| Bash | `on_attack_hit,attacker,proc,on_bash,25` + `on_bash,trigger_unit,apply_status,stun,,,1.2` + `on_bash,trigger_unit,damage,,40,physical` |
| Orb / bonus damage | `on_attack_hit,trigger_unit,damage,,40,magical` (attack bonus damage) plus any status (poison = `burn`, slow, `armor` timed modifier) |
| Mana break | `on_attack_hit,trigger_unit,resource,mana_burn,40,physical,,,,0.8` |
| Cleave / splash | passive `cleave` / `splash` ratios |
| Timed cleave | `stat_modifier,cleave` (fraction 0..1, with a duration) adds to the cleave ratio of the granting ability's own cleave instance (it does not merge with item cleave) and reverts exactly; only melee attackers cleave (base attack range up to 250 counts as melee) |
| Cone cleave | timed `cleave_radius` (half-width at the attacker), `cleave_end_radius` (half-width at the far end) and `cleave_distance` (reach), each above 0 and additive like every stat. A non-zero `cleave_distance` turns that instance's circular cleave into a trapezoid from the attacker towards the struck unit; units overlap it when their centre projects within `0..distance` (widened by their body radius) and lies within the half-width there. Without `cleave_distance` the circle (`cleave_radius`, default 300) is used. Cleave stats from one ability's statuses sum into one instance; item cleave stays a separate circle, so both can hit the same unit |
| Percent bonus damage | timed `stat_modifier,attack_damage_pct` (0..1000): that percent of the target's base attack damage (the hero definition's damage for heroes, excluding item bonuses; the unit's current damage for other units), snapshotted when applied and reverted with the exact delta |
| Evasion / accuracy / true strike | passive `evasion` / `accuracy`, or timed statuses `evasion`, `accuracy`, `true_strike` |
| Instant attack | `on_cast,cast_target,perform_attack,,100` |
| Double attack | `on_attack_hit,attacker,proc,on_double,25` + `on_double,trigger_unit,perform_attack,,100` |
| Multi-target attack | `on_attack,units_in_radius,perform_attack,exclude_trigger_unit,70,,,600,2` (value = damage %, radius, amount = max targets) |
| Attack counter | `on_attack_hit,attacker,counter,on_fourth,4` + `on_fourth,...` rows |
| Pseudo-random bash | `on_attack_hit,owner,pseudo_proc,on_bash,25` + `on_bash,...` rows (same columns as `proc`) |
| Range / projectile speed | passive or timed `attack_range`; passive `attack_projectile_speed` |

Samples: `basher`, `desolator`, `crit_blade`, `split_bow`, `mana_breaker`, `thunder_axe` in `data/examples/`. Out of scope: orb uniqueness, autocast orbs, bounce / pierce attack projectiles.

### Activation & targeting mechanics (reference)

Shared pipeline: `ActivationRequest` → `ActionSpec` (`src/actions/`) → cast point → fire → optional channel → backswing. Abilities and actives use the same fields; items use `ItemActivation` mapped into `ActionSpec` (`items.rs`).

| Mechanic (player-facing) | Code / data |
| --- | --- |
| Unit target | `TargetType::Unit`; CSV `target_type=unit`; pick filter `unit_only` in targeting UI |
| Point target | `TargetType::Point`; CSV `target_type=point` (with projectile → skillshot path) |
| Direction target | `TargetType::Direction`; CSV `target_type=direction`; `ignores_range()` for validation |
| No target | `TargetType::NoTarget`; instant hotkey when `is_instant()` |
| Self target | `TargetType::SelfTarget`; CSV `target_type=self` + usually `target_team=self` |
| Area target | `TargetType::Area`; CSV **`point` + `aoe_radius` + `projectile_type none`** (no CSV `area` alias) |
| Toggle | `AbilityBehavior::Toggle`; `AbilityState.toggled`; mana upkeep (`toggle_mana_per_sec`) and `WhileToggled` statuses in `abilities/passive.rs` |
| Autocast | *Not implemented* |
| Passive ability | `AbilityBehavior::Passive`; `AbilityTrigger::Passive` + `PassiveStat` effects; `sync_ability_passives` |
| Channeling | `ActionSpec.channel_time`, `Activation.channel_remaining`; `OnChannelEnd` (completed) and `OnChannelInterrupted` (ended by a disable) for abilities and items are produced by `action_channel_end_triggers` in `abilities/casting.rs` (`on_channel_end` / `on_channel_interrupted` in the effect CSVs) |
| Cast point | `ActionSpec.cast_time` / `point_remaining`; mana and cooldown on fire |
| Cast backswing | `ActionSpec.backswing`; items default to **0** backswing |
| Cast range | `ActionSpec.cast_range` (`RankValue` on abilities, flat on items); edge-to-edge via `dimensions` |
| Target team filter | `TargetTeam` + `team_allows` / `target_allowed`; CSV `target_team` enemy/ally/both/self |
| Hero / creep / building filter | *Not implemented* (`TargetUnitMask` planned) |
| Cooldown | `ActionCooldown.remaining`; starts on commit |
| Cooldown reduction / reset | *Not implemented* |
| Charges | `ActionCooldown.charges`; abilities may regen over time (`restore_ability_charges`); item charges per stack only |
| Resource cost | `ActionCost { mana }` only (health / gold / toggle upkeep via `toggle_mana_per_sec` in `passive.rs`) |
| Cast interruption | See table below |

#### Cast interruption

| Phase | Disable (abilities: silence / stun / sleep; items: mute / stun) | New move / stop / attack | Target dies | Source slot changes |
| --- | --- | --- | --- | --- |
| Cast point (before fire) | Cancel, no cost (a silenced ability is cancelled but an item still fires; a muted item is cancelled but an ability still fires) | Cancel, no cost | The aim follows a living unit target; if it dies or despawns the action still fires at its last known position (costs paid, effects keyed on the target resolve to nothing). A queued cast still walking into range is dropped instead if its unit-only target died | Cancel |
| After fire, channeling | End channel early and fire `OnChannelInterrupted` (abilities: silence, stun or sleep; items: mute or stun; sleep does not interrupt an item channel). A channel that runs its full time fires `OnChannelEnd` | Cancel channel (no item/ability trigger) | — | Cancel |
| Backswing | Does not remove backswing by itself | Removes `Activation` | — | Cancel |

A `CastTarget::Unit` that turns invisible or untargetable during the cast point is still tracked by `refresh_activation_aims`; only death or despawn stops the tracking. That is acceptable today and would need revisiting if fog-based targeting is added.
- **Custom mechanics** (`abilities/custom.rs`): `app.register_ability_behavior(id, system)` runs a one-shot system with the `CastContext`. It reports what happened with a trigger such as `AbilityTrigger::Custom("on_my_event")`, and the ability's effect entries apply the consequences

Adding a simple ability: add an `AbilityId` variant (or use the generators), then add an arm to `catalog::builtin`:

```rust
AbilityId::Fireball => def
    .targeting(TargetType::Unit, RankValue::linear(600.0, 25.0), RankValue::ZERO)
    .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
    .timing(0.3, 0.4)
    .mechanic(PROJECTILE)
    .on(OnProjectileHit, EffectTarget::TriggerUnit, magic_damage(RankValue::linear(150.0, 50.0)))
    .on(OnProjectileHit, EffectTarget::TriggerUnit, status(StatusSpec::Stun, RankValue::fixed(1.0))),
```

### Adding abilities (AbilityGenerator)

`data/abilities.csv` + `data/ability_effects.csv` (joined on `ability_id`) generate `src/abilities/generated.rs`: one ordinary `AbilityDefinition` per ability, built with the same builder as `catalog.rs`. `catalog::builtin` prefers a generated definition over the hand-written one.

```bash
python3 scripts/AbilityGenerator.py                       # data/abilities.csv data/ability_effects.csv
python3 scripts/AbilityGenerator.py a.csv a_fx.csv --check  # validate only
python3 scripts/AbilityGenerator.py --dry-run             # print what would change
```

```csv
id,name,slot,ability_type,target_type,target_team,cast_range,aoe_radius,cast_time,channel_time,cooldown,mana_cost,max_charges,charge_restore_time,projectile_type,projectile_speed,description
magnus_shockwave,Shockwave,Q,active,point,enemy,1200,200,0.3,,13|12|11|10|9|8|7,85|90|95|100|105|110|115,,,skillshot,1200,Sends out a wave of force that damages enemies in a line pulls each one toward the wave and slows it briefly
```

- Enums: `slot` Q/W/E/R/innate (R ⇒ ultimate), `ability_type` active/passive/toggle, `target_type` unit/point/direction/no_target/self, `target_team` enemy/ally/both/self, `projectile_type` none/targeted/targeted_each/skillshot/skillshot_single (`skillshot_single` stops at the first enemy it strikes, for hooks; `targeted_each` is described below)
- **Area targeting in CSV:** there is no separate `area` value. Author `target_type=point` with non-zero `aoe_radius` and `projectile_type none`; the generator emits Rust `TargetType::Area` (see `scripts/AbilityGenerator.py`). Ground-placed AoE uses the same cast / range rules as point spells.
- Level values: a single number or `a|b|c|d` (one per rank; arithmetic lists become `RankValue::from_level_1`, others a level table). Blank = not applicable
- Optional extra columns: `cast_backswing` (default 0.25), `max_rank` (else the level-list length, else 4 for R / 7 otherwise), `toggle_mana_per_sec` (toggle abilities only; level list allowed), `projectile_end_radius` (skillshots only; level list allowed), `projectile_origin_offset` (skillshots only; level list allowed), `cast_range_buffer` (`unit` targets only; level list allowed)
- **`cast_range=global`:** the keyword (any case, never inside a level list) makes the range the whole map (`scale::GLOBAL_CAST_RANGE`, 25,000). The hero never walks into range, and the cast indicator draws no range ring. Items accept it too in their `cast_range` column. `cast_time` above 2.0 s (`MAX_CAST_POINT`) and `cast_backswing` above 2.0 s (`MAX_CAST_BACKSWING`) are capped (the generator warns)
- **`projectile_end_radius` (expanding skillshots):** for `skillshot` / `skillshot_single` only, above 0, and it needs `aoe_radius`. The wave's width grows linearly from `aoe_radius` to this value over its flight (the cast range). Blank keeps the width constant. Items reject it (they have no `aoe_radius`)
- **`cast_range_buffer` (lock-on leash):** `unit`-targeted abilities only, above 0. During the cast point the cast is cancelled for free (no mana, no cooldown; a queued unit command sees it as interrupted before firing) if the target is further than `cast_range + cast_range_buffer` from the caster, measured edge to edge like the cast range. A target that dies is a point and the cast still fires. Blank means no limit (the default). Warns with `cast_range=global`; abilities only for now
- **`projectile_origin_offset` (skillshot start):** for `skillshot` / `skillshot_single` only, above 0. The wave starts this many units in front of the caster, towards the aim, instead of at the caster's centre, and still flies the full `cast_range` from there (and the width and `projectile_end_radius` apply from that start). Blank starts at the caster. Items reject it
- **`projectile_type=targeted_each`:** sends one homing projectile (no splash, `projectile_speed` applies) at every living, targetable unit of `target_team` inside `aoe_radius`, never the caster. It needs `target_type` `no_target` (around the caster, with the shockwave ring) or `point` (around the aim) and a non-zero `aoe_radius`; `target_team` cannot be `self`. Each projectile fires `on_projectile_hit` with the struck unit as `trigger_unit` (`affected_units` is not available there). Items reject it until they have an `aoe_radius` column
- Mechanics are derived: `targeted` / `skillshot` → projectile (skillshots hit every enemy along `cast_range`, width = `aoe_radius`); `no_target` + AoE → shockwave ring; `point` + AoE without a projectile → area ring

```csv
ability_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,duration,radius,amount,scaling_type,scaling_value
magnus_shockwave,0,on_projectile_hit,trigger_unit,damage,,75|150|225|300|375|450|525,magical,,,,,
magnus_shockwave,1,on_projectile_hit,trigger_unit,movement,pull_point,150,,0.2,,,,
skewer,0,on_cast,caster,movement,dash,800|900|1000|1100|1200|1300|1400,,0.888889|1|1.111111|1.222222|1.333333|1.444444|1.555556,145,,,
skewer,1,on_unit_contact,trigger_unit,movement,drag_stun,,,,,,,
```

- `trigger`: reactive triggers (below) and `on_cast`, `on_projectile_hit`, `on_unit_contact` (a dash or charge touched an enemy; trigger unit = that enemy), `on_movement_end` (a dash or charge ended; trigger unit = the chased unit if reached, affected units = everyone touched), `on_impact`, `on_channel_tick`, `on_channel_end` (channel completed), `on_channel_interrupted` (channel cut short by silence/stun/mute; a new order fires neither), `on_expire`, `while_toggled` (toggle abilities only: `stat_modifier` or `apply_status,mana_shield` on `caster`, no duration; applied while the toggle is on; the upkeep is the ability row's `toggle_mana_per_sec`), or any other `on_…` name raised by a custom mechanic
- `effect_target`: `caster`, `cast_target`, `trigger_unit`, `affected_units`, `units_in_radius` (around the trigger point; `radius` or the ability AoE; team from the ability / effect polarity)
- `effect_type` (+ `effect_id`): `damage` (`value` or `amount`, `damage_type` physical/magical/pure), `heal` (`effect_id=mana` restores mana), `apply_status` (`stun`, `root`, `silence`, `disarm`, `slow` (% from `amount`/`value`, taken from the target's base move speed; the status id is `<owner>_slow`, so slows from different sources coexist and add, the same source refreshes, and `remove_status,slow` is rejected: name the `<owner>_slow` id), `turn_rate_slow`, `turn_rate_bonus` (% from `amount`, 0..400), `phased`, `forceful`, `flying`, `leash` / `tether` (`value` = radius), `debuff_immunity`, `burn` (`value` = damage, `damage_type` required; per second by default, or per tick when `amount` sets the tick interval in seconds, above 0 and at most the duration — the first tick lands one interval after application, `floor(duration / interval)` ticks in total, flushed on natural expiry but not on dispel; the status id is `<owner>_burn`, so burns from different sources coexist and `remove_status,burn` no longer matches them), or any id as a timed `stat_modifier`: besides the plain stats, `attack_damage_pct` (a percent, 0..1000, of the target's base attack damage, snapshotted when applied), `cleave` (a fraction, 0..1, added to the target's cleave ratio; only melee attackers cleave) and the cone-shape stats `cleave_radius`, `cleave_end_radius`, `cleave_distance` (see Attack modifiers)), `remove_status` (`buff`/`debuff` dispel, otherwise a status id), `movement` (`knockback`, `pull`, `pull_point`, `dash`, `charge`, `drag`, `drag_stun`, `cyclone`; `value` = distance, `duration`, `amount` = charge speed (or the forward offset of `pull_point`), `radius` = dash / charge contact radius — see Forced movement below; `teleport` sends `caster` / `owner` / `cast_target` / `trigger_unit` to the cast's aim point, optionally bounded (`value` = max distance, `amount` = min distance) — see Teleport below), `stat_modifier` (`effect_id` = stat, `amount`, `duration`), `custom` (registered id; receives `value`, `amount`, `duration`)
- `chain` effect type (gate): `effect_id` = the custom `on_<name>` trigger fired per hop, `value` = jumps (integer, at least 1), `radius` = jump radius in world units (above 0), `amount` = percent of the amount kept per jump (above 0, at most 100); the target is the start unit (`cast_target`, `trigger_unit`, `caster`/`owner`, `attacker`). Put the damage in rows tagged with the custom trigger; see `chain_lightning` / `storm_charm` in `data/examples/`
- `scaling_type`: `strength` / `agility` / `intelligence` / `attack_damage` × `scaling_value` (damage, heal and `health_drain`)
- Healing and resource rows (no extra columns):

| `effect_type,effect_id` | `value` | `amount` | `damage_type` | Targets |
| --- | --- | --- | --- | --- |
| `resource,health_drain` | damage (scaling allowed) | heal ratio (default 1) | required | other units |
| `resource,mana_burn` | mana removed | damage per mana (default 1) | required | other units |
| `resource,mana_drain` | mana removed | ratio given to the caster (default 1) | blank | other units |
| `resource,spend_health` / `spend_mana` | amount spent | blank | blank | `caster` / `owner` only |
| `resource,health_to_mana` / `mana_to_health` | amount spent | gained per point spent (required, > 0) | blank | `caster` / `owner` only |
| `apply_status,heal_over_time` | HP per second (required) | blank | blank | any |
| `apply_status,heal_reduction` | blank | percent 0..100 (required) | blank | any |
| `apply_status,heal_block` | blank | blank | blank | any |
| `apply_status,mana_shield` | absorb percent, above 0 up to 100 (required) | damage per mana, above 0 (required) | blank | any; allowed under `while_toggled` |
| `stat_modifier,{lifesteal,spell_lifesteal,heal_amp,heal_received_amp,mana_cost_amp}` | fraction (-1..1; `mana_cost_amp` only has the floor -1; -0.25 = 25% cheaper) | blank | blank | passive → `PassiveStat`; timed or toggled → `StatusSpec::ResourceModifier` |
| `stat_modifier,{health_regen,mana_regen,max_health,max_mana}` timed or toggled | flat | blank | blank | passive path unchanged |

  Ratios are single numbers (no per-level lists). `stat_modifier` amps are fractions; `apply_status` percents are 0..100.

#### Reactive triggers in CSV

The `trigger` cell is `name[:arg[:arg]]`; the base name decides everything else (which owner may use it, which targets resolve). Both generators accept all of these for abilities (learned passives included) and items:

| `trigger` cell | Rust | Rules |
| --- | --- | --- |
| `on_damage_dealt`, `on_damage_taken`, `on_kill`, `on_death` | plain variants | `attacker` target = the unit that caused it for `on_damage_taken` / `on_death` |
| `on_health_below:30` | `OnHealthBelow { percent: 30 }` | whole number 1..=99 |
| `on_spell_cast`, `on_spell_hit` | plain | |
| `on_status_applied`, `on_status_removed` with `:buff`, `:debuff` or `:<status id>` | `(StatusFilter::Any / Buff / Debuff / Id)` | id is a status id (`stun`, ...) or a timed-stat `<owner>_<stat>` id |
| `on_cooldown_ready` | plain | invalid on passive abilities and on items with no `on_use` |
| `on_interval:5` | `OnInterval { millis: 5000 }` | seconds, at least 0.1 |
| `on_move` | plain | every row must be `accumulate` |
| `on_enemy_enter:600`, `on_enemy_leave:600` | `{ radius: 600 }` | radius above 0 |
| `while:health_below:50`, `while:moving`, `while:stationary`, `while:enemies_near:600:2`, `while:allies_near:600:1`, `while:has_status:<id>` | `While(Condition::...)` | rows are `apply_status` (or a timed `stat_modifier`), blank `duration`, target `owner` / `caster`; `has_status` may not name the status the row holds |

New effect types: `pseudo_proc` (same columns as `proc`) and `accumulate` (`effect_id` = custom `on_<name>` trigger, `value` = threshold above 0, levels allowed). Both feed the unreachable-custom-trigger warning like `proc`. The generator rejects `perform_attack` under reaction triggers (also under a custom trigger fed from one), and warns when two `while` rows of one owner hold the same status id: held statuses share an id and would fight, so give each its own.

Recipes:

| Mechanic | Rows |
| --- | --- |
| Thorns | `on_damage_taken,attacker,damage,,40,magical` |
| Heal on kill | `on_kill,owner,heal,,150` (the victim may be gone next frame: target the owner or `units_in_radius`) |
| Last stand | `on_health_below:25,owner,apply_status,shield,,,4,,300` |
| Mana on cast | `on_spell_cast,owner,heal,mana,30` |
| Pulse | `on_interval:3,units_in_radius,damage,,60,magical,,500` |
| Every 1000 units walked | `on_move,owner,accumulate,on_far_walk,1000` + `on_far_walk,owner,apply_status,phased,,,2` |
| Aura-ish slow | `on_enemy_enter:600,trigger_unit,apply_status,slow,,,2,,30` |
| Conditional bonus | `while:health_below:50,owner,stat_modifier,attack_damage,40` |
| Pseudo-random stun | `on_attack_hit,owner,pseudo_proc,on_bash,25` + `on_bash,trigger_unit,apply_status,stun,,,1.2` |
| Storm Mail (every 300 damage taken arcs lightning from the attacker) | `on_damage_taken,owner,accumulate,on_overload,300` + `on_overload,trigger_unit,chain,on_storm_arc,2,,,450,75` + `on_overload,trigger_unit,damage,,80,magical` + `on_storm_arc,trigger_unit,damage,,80,magical`; under a derived custom trigger the attacker is `trigger_unit`; see `storm_mail` in `data/examples/` |

Samples: `thorn_mail`, `bloodstone`, `aeon_charm`, `arcane_echo`, `pulse_core`, `phase_treads`, `sentinel_ward`, `berserker_blood`, `prd_basher` and the passive ability `thornskin` in `data/examples/`; the matching bad rows (`reactor`, `aura`) are in `data/examples/invalid/`.

### Generator validation

Both generators also warn when a row uses a trigger no game system emits yet (only `on_channel_tick` and `on_expire`); the row is generated but never fires. They reject `perform_attack` under a reaction trigger and under any custom trigger a `chain` row feeds (a chain hop carries no attack, so the extra attack could re-fire the trigger that fed the chain without end).

Both generators validate before writing anything and print `file:row [field=value] message` for every problem (exit 1): duplicate ids, effect rows for unknown owners, duplicate `effect_index` per owner, unknown enums, bad numbers / level lists (wrong length, negative), damage without an amount, `apply_status` / `custom` without `effect_id`, projectile speed without a projectile, `targeted` without unit targeting / `skillshot` without point or direction, charge restore time without charges, resource rows that break their rules (`spend_*` / conversion on another unit, `mana_burn` without `damage_type`, `mana_shield` without `amount`, `heal_reduction` above 100, a conversion without a ratio, scaling on `mana_drain`, a fraction stat outside -1..1), and trigger/target combinations that cannot resolve (e.g. `cast_target` on a `no_target` ability, `trigger_unit` on `on_cast` for items). `data/examples/` holds valid samples (shockwave, skewer, frost_hammer, blink_item, …) and `data/examples/invalid/` one row per error.

Re-running overwrites: each owner lives in a `// <ability:id>` / `// <item:id>` block, CSV rows replace their own blocks and other blocks are kept, and the output is rustfmt-ed.

### Attributes

- **Strength** — max HP and HP regen
- **Agility** — armor and attack-speed rating
- **Intelligence** — max mana, mana regen, and magic resist
- Each level raises Str / Agi / Int using that hero's growth rates (HUD shows IAS/APS and derived combat stats)

### Items

- Gold shop near each base; click an item for its build tree (components + recipe scraps); buy from the detail popup. Recipes are hidden from the default shop grid.
- Six inventory slots; RMB → **Sell (50%)** near shop; actives use ASDZXC through the same activation pipeline as abilities (no target and self fire on the key; unit, point, and direction enter targeting mode)
- Heroes carry a `StatusEffects` list for buffs / debuffs
- Heroes and creeps separate by collision size without pushing each other (moving units yield to stationary ones); the **forceful** buff can shove (e.g. Vanguard Shockwave). **Phased** units and units in forced movement (Dash, knockback, pull) ignore unit collision.
- Every item is an `ItemDefinition` (`src/item_catalog.rs`): price, sell value, tier, rarity, stacking, charges, recipe components, and `AbilityEffectEntry` effects. Passive `PassiveStat` entries fold into stats; `on_use` (hotkeys), `on_attack` and `on_attack_hit` fire `AbilityTriggerEvent`s with `EffectSource::Item`, resolved by the same effect systems as abilities

### Adding items (ItemGenerator)

`data/items.csv` + `data/item_effects.csv` (joined on `item_id`) generate `src/item_catalog.rs` and patch the `ItemId` enum / `ItemId::ALL` in `src/items.rs`.

```bash
python3 scripts/ItemGenerator.py            # data/items.csv data/item_effects.csv
python3 scripts/ItemGenerator.py --check    # validate only
```

```csv
id,name,item_type,cost,sell_value,tier,rarity,stackable,max_stack,consumable,max_charges,description
frost_hammer,Frost Hammer,weapon,2800,1400,3,rare,false,,false,,A heavy weapon imbued with frost
```

- `item_type` component/weapon/armor/accessory/consumable/utility/recipe, `rarity` common/uncommon/rare/epic/legendary; optional extras `short_label`, `cooldown`, `components` (`;`-separated ids), `color_r/g/b`
- Optional activation columns on items with `on_use` or a channel trigger (blank = instant no-target use): `target_type` (`unit`, `point`, `direction`, `no_target`, `self`), `target_team` (`enemy`, `ally`, `both`, `self`), `cast_range`, `cast_time`, `channel_time`, `mana_cost`, `projectile_type`, `projectile_speed` (`cooldown` remains on the item row). A `projectile_type` gives the item the same projectile mechanic abilities use (`ItemDefinition::mechanics`); put its consequences on `on_projectile_hit` / `on_impact` rows, which are only valid for projectile items (see `lightning_staff` in `data/examples/`). Unit targets require `target_team`. Point, unit, and direction targets require `cast_range`. Teleport items must be `point` or `direction` so the destination is the cast aim.
- Effects use the ability effect schema with item triggers `passive` (must be `stat_modifier`: `max_health`, `health_regen`, `max_mana`, `mana_regen`, `attack_damage`, `attack_speed`, `armor`, `magic_resist`, `move_speed`, `strength`, `agility`, `intelligence`, `spell_amp`, `outgoing_physical_amp`, `incoming_damage_amp`, `damage_block`, `damage_reflect`, `lifesteal`, `spell_lifesteal`, `heal_amp`, `heal_received_amp`, `mana_cost_amp`, `crit_chance`, `crit_multiplier`, `evasion`, `accuracy`, `cleave`, `splash`, `attack_range`, `attack_projectile_speed`; `health` and `mana` are aliases of `max_health` and `max_mana`), `on_use`, `on_unit_contact` / `on_movement_end` (after a dash or charge from `on_use`), `on_attack`, `on_attack_hit` (abilities accept these too), `on_channel_end` / `on_channel_interrupted` (channelled actives only), reactive triggers (`on_damage_dealt`, `on_damage_taken`, `on_kill`, `on_death`, `on_health_below:N`, `on_status_applied`, `on_move`, `on_interval:ms`, ... and `while:<condition>` rows; see Reactive triggers), `on_projectile_hit` / `on_impact` (projectile items only), custom `on_<name>` triggers fed by `proc` / `pseudo_proc` / `counter` / `accumulate` / `chain` rows (a custom trigger nothing fires warns). New effect types: `proc` (`effect_id` = custom trigger, `value` = chance % in (0,100], owner targets), `pseudo_proc`, `accumulate`, `chain`, `counter` (`value` = integer >= 2), `perform_attack` (`value` = damage % default 100, `amount` = max targets, `effect_id` blank or `exclude_trigger_unit`); new statuses `evasion`, `accuracy`, `true_strike`; timed stat `attack_range`, and targets `owner`, `cast_target` (unit and self actives), `trigger_unit`, `attacker`, `affected_units`, `units_in_radius`
- Consumables spend a charge (or one of the stack) when the use fires, and disappear when empty. A rejected activation spends nothing. Item charges never regenerate, so a `charge_restore_time` on an item is an error. `scaling_type=distance` is also rejected on items (they have no rank-scaled cast range); every other scaling works

### Teleport

`movement,teleport` (`on_cast` for abilities, `on_use` for items) is one shared effect: `AbilityEffect::Teleport` → `TeleportEvent` → `teleport.rs`. The destination is the trigger's aim point (the cast's point or target position). Item teleports take that aim from a point or direction activation, so they use the same range and targeting indicator as abilities. Cast time, cooldown, mana, and range stay on the shared activation.

- The destination must fit the target's `CollisionRadius` inside the map and clear of trees, towers, and ancients (`NavGrid`), and must not overlap a non-phased unit (phased targets ignore units, like normal collision). Otherwise nothing changes — no nearby fallback and no pushing — and a `TeleportRejectedEvent` is emitted.
- On success the unit moves instantly, keeps its Y, health, mana, statuses, and cooldowns, and loses its `MoveTarget`, `NavPath`, attack-move, attack-position goal, and in-flight forced movement. A `TeleportCompletedEvent` is emitted. Attack and queued-cast orders are kept (units re-path from the new spot).
- Costs are paid at cast / use time, so a rejected teleport still spends mana and starts the cooldown.
- **Bounded teleports:** with `value` (maximum distance, above 0) and/or `amount` (minimum distance, 0 or more, at most `value`) the destination is the aim point with its distance from the caster clamped to `min..=max` along the same line. An aim on the caster itself uses her facing. Both blank keeps the destination exactly at the aim point (Blink, Blink Dagger). Bounds need `effect_target` `caster` / `owner`. Pair a max distance with `cast_range=global` to make a blink that ignores range but not distance (Queen of Pain's blink).

### Forced movement

Knockback, pull, dash, charge, drag and cyclone are one shared primitive: `AbilityEffect::Displace { kind, distance, duration, speed, contact_radius, forward_offset, bound_stun }` → `DisplacementEvent` → `ForcedMovement` on the moved unit, ticked by `displacement.rs`, which is the only writer of a moving unit's `Transform`. Abilities and items use the same effect (`movement,<kind>` rows); `AbilityMechanic::Dash` writes the same event and only adds the ghost trail.

| Kind | CSV | Moves | Notes |
| --- | --- | --- | --- |
| `knockback` | `value` = distance, `duration` | target, away from the trigger point | |
| `pull` | `duration` (and optional `value` = max distance) | target, toward the caster | Tracks the caster until contact; the pull time is the time to close the starting gap |
| `pull_point` | `duration`, optional `value`, optional `amount` = forward offset | target, toward the trigger point | With `amount` (not negative) the target is pulled to a point that far in front of the caster, along its facing, measured from where the caster stood when the trigger fired; a crowd gathers there without teleport occupancy rejections |
| `dash` | `value` = max distance (required), `duration` = time at max distance (default 0.3), optional `radius` = contact radius | caster / owner only | Straight line at constant speed; shorter dashes take proportionally less time. `radius` (above 0; also on `charge`) is the reach from the mover's centre, plus the other unit's body radius, within which it contacts enemies; blank is body contact |
| `charge` | `amount` = speed, `duration` = max time (both required), optional `value` | caster / owner only | Chases the cast target (needs `target_type unit`) until contact |
| `drag` | optional `duration` | target (not the caster) | Rides along at a fixed offset with the caster's own dash or charge; blank duration = until that movement ends. Typically `on_unit_contact,trigger_unit` |
| `drag_stun` | optional `duration` | target (not the caster) | A `drag` that also stuns each dragged unit under the status id `<owner>_stun` (a debuff that behaves as a stun). The stun is bound to the drag (`ForcedMovement.bound_status`): exactly that status is removed when the carrying movement ends, however it ends, so no other stun is overwritten (refresh is by id) or removed |
| `cyclone` | `duration` (required) | target, held in place | Also applies the `cyclone` status |

- **Priority.** A unit has at most one motion. A new request replaces the current one only if its priority is at least as high: own movement (dash, charge) < external (knockback, pull, drag) < suspend (cyclone). The newest wins at equal priority. A cycloned unit rejects every other displacement and teleport; another cyclone refreshes it.
- **Own movement needs a unit that can move.** A rooted, stunned or sleeping caster pays for the cast but does not dash. A stun or root during a dash ends it at the current spot. External motion continues through stuns and roots.
- **Hostile only.** Debuff immunity blocks hostile displacement; a unit's own and its allies' motion are allowed.
- **Terrain.** Motion stops at trees, towers, ancients and the map edge (unless the unit is flying or cycloned). A unit that starts inside geometry may move so it can get out.
- **Landing.** Every motion that ends normally sets the unit on valid ground (the nearest walkable spot if it ended inside geometry) and drops its stale `NavPath`, so its `MoveTarget` is replanned from the new position. Motion no longer roots the victim: move orders survive and resume afterwards. Units in forced motion ignore unit collision while they travel.
- **Triggers.** Own movement carrying a cast reports `OnUnitContact` for each enemy it newly touches and one `OnMovementEnd` when it ends by arrival, block, timeout, or the mover being disabled. Replacement, teleport, and death end it silently. Both resolve in the frame they happen.
- **Untargetable.** A cycloned unit cannot be picked as a unit target (casts, attack commands, basic attacks, AI acquisition), homing projectiles miss it, and skillshots, splash, cleave, and ability area effects skip it. Its own targeting is unaffected.
- **Flying.** Flying units path in a straight line and walk over trees; towers and ancients still block. When the status ends over geometry the unit is moved to the nearest walkable spot.
- **Leash / tether.** `enforce_leashes` clamps a leashed unit to its circle after walking and after forced motion every frame, so a knockback stops at the edge. A tether whose anchor is gone does nothing. Teleporting outside a circle is rejected (`TeleportError::Leashed`).

A hook is a composition: an ability with `projectile_type skillshot_single` and `on_projectile_hit,trigger_unit,movement,pull`. A skewer is `movement,dash` on cast plus `on_unit_contact,trigger_unit,movement,drag` (or `drag_stun`), with consequences on `on_movement_end`.

### Limitations

Not yet expressible without engine changes: **autocast**; **cooldown reduction / reset**; **hero / creep / building** target masks; **channel ticks** (`on_channel_tick` is defined but the shared pipeline does not emit it — custom mechanics only); delayed effects; cooldown floors other than explicit level lists; an internal-cooldown gate for reactive passives; aura statuses applied to other units; applier-side status triggers ("when I debuff someone"); cooldown-ready on charge restore; and `on_expire` (declared, never emitted — use `on_status_removed`). A reaction cannot cancel the lethal hit (reactions resolve a frame later). Item charges are spent when the item fires and refill with the next unit of the stack; they never regenerate over time the way ability charges do, and `ItemGenerator` rejects a `charge_restore_time` on items. An instant item (no cast point and no channel) fires without joining the command queue, so Shift does not queue it.

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
