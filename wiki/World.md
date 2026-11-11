# World

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
- Units only **start** moving, attacking, or casting once the aim/target is within **11.5°** of facing. Facing is Bevy's local **−Z** (`Transform::forward`, where the nose sits); `facing::yaw_toward` is the one yaw convention every facing write uses
- **Turn rate** is radians per **0.03s** (heroes default **0.6**, creeps **0.5**)
- Attacks use **foreswing** (attack point) then fire, then **backswing** (cancelled by move/stop/new orders). Base values are 0–1.0s at 100 attack speed; both scale as `base × 100 / AS` (`CombatStats::effective_attack_point` / `effective_attack_backswing`)
- **Melee** auto-attacks (range ≤ 150) deal damage instantly with a rectangular slash VFX; **ranged** attacks still use projectiles
- A **white ring** on the ground shows the local hero's attack range
- Hovering an ability icon shows a **blue ring**: its cast range (for skillshots, how far they travel), or its effect radius for no-target abilities
- Attack speed rating: `(base AS + agility + flat bonuses) × (1 + mult)`, clamped **20–700**; attack interval = `BAT × 100 / AS` (APS = `(AS/100) / BAT`), measured from the **start** of one attack to the start of the next (the cooldown armed at release is the interval minus the scaled attack point; a windup cancelled before release spends nothing). E.g. BAT 1.7, attack point 0.3, 500 AS ⇒ an attack every 0.34s, connecting 0.06s in
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
