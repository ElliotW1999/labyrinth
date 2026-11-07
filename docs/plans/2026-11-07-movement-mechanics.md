# Movement and repositioning mechanics: audit and implementation plan

**Date:** 2026-11-07
**Status:** Approved 2026-11-07 with the decisions recorded under "Approved decisions" (end of file). The architect did not run `cargo test`, so Step 0 records the baseline.
**Based on:** `2026-11-06-shared-effects-pipeline.md`, `2026-11-06-status-effects-catalog.md` (rows Leash, Cyclone, Turn rate slow, Phased; Phase E), `2026-11-07-item-ability-architecture-cleanup.md` (invariants, the V commands, test-app rules).
**Scope:** one PR with one commit per step. Every step compiles and passes **V** (the shared verification commands in the cleanup plan) on its own.

---

## Executive summary

The engine already has the right place for these mechanics. `AbilityEffect::Displace` → `DisplacementEvent` → `displacement.rs` (`ForcedMovement`) handles knockback and pull. `AbilityEffect::Teleport` → `TeleportEvent` → `teleport.rs` handles blink. Statuses (`status.rs`) handle phased, slow, turn-rate slow and fear. `NavGrid::nearest_walkable` / `point_clear` (`navigation/grid.rs`) already answer the question "is this a valid spot?".

The gaps are concentrated in `ForcedMovement`. It is a blind lerp:
- It goes through trees and the map edge. README:379 says "Displacement ignores obstacles."
- It has no priority rules.
- It never re-settles the unit on valid ground.
- It depends on a "displaced" **root** status, which deletes the victim's `MoveTarget`.
- It cannot track a moving entity, report contacts, or carry other units.

Separately:
- Dash is an ability-only mechanic that just walks at normal move speed (`MoveTarget` + phased). Root blocks it and a new order cancels it.
- Flying, leash/tether/region restriction, cyclone, turn-rate *increase*, and the move-speed floor are missing.

**Recommendation:** extend `ForcedMovement` into the single displacement primitive, with a `MotionPath` enum, a priority, terrain stop + landing, contact/end triggers, and unit phasing during motion. Add the new kinds to the existing `DisplacementKind` / `AbilityEffect::Displace` / generator `movement` effect type. Express the time-limited constraints (flying, leash, tether, turn-rate bonus, cyclone) as `StatusEffect` fields, so that duration, dispel and debuff immunity come for free. Do not add 16 systems; this plan adds two (`settle_grounded_units`, `enforce_leashes`).

---

## Audit

Legend: **Done** = works as named. **Partial** = there is a hook, but the semantics are incomplete. **Missing** = no support.

| # | Mechanic | Status | Where today | Gap |
|---|---|---|---|---|
| 1 | Teleport / Blink | **Done** | `teleport.rs` (`apply_teleport_events`, `validate_destination`); `AbilityEffect::Teleport` (`definition.rs:430`); generator `movement,teleport` (`generator_common.py:875`); Blink (`generated.rs:242`), Blink Dagger (`item_catalog.rs:2126`) | Ignores the new constraints: a leashed or cycloned unit can teleport (fixed in Steps 5 and 7) |
| 2 | Dash | **Partial** | `AbilityMechanic::Dash` → `MechanicRunner::dash` (`mechanics.rs:201-222`) inserts `MoveTarget` + `StatusEffect::phased` | Walks at `move_speed` through normal pathing. Root blocks it and any new order cancels it. Abilities only (items can't dash). No contact reporting |
| 3 | Charge | **Missing** | none | Needs a motion that tracks a unit and reports arrival |
| 4 | Knockback | **Partial** | `DisplacementKind::Knockback`, `apply_displacement_events` (`displacement.rs:51-58`) | Goes through trees and the map edge. No landing on valid ground. The root drops the victim's orders. Newest event always overwrites the old one |
| 5 | Pull | **Partial** | `DisplacementKind::Pull` (toward the caster, to contact) | Destination is fixed when the event is applied (no tracking). No "toward a point" variant. Same terrain gaps as knockback |
| 6 | Drag | **Missing** | Doc only: `custom.rs:1-8`, `data/examples/ability_effects.csv` skewer rows (`custom,skewer_movement` / `skewer_drag`) | No carry primitive. `OnUnitContact` has no stock emitter (shared-effects plan, P1 #3) |
| 7 | Hook | **Partial** | Composition: skillshot `Projectile` + `OnProjectileHit` + `movement,pull` (`data/examples/ability_effects.csv` shockwave rows) | Pull doesn't track the source. Skillshots pierce every unit instead of stopping at the first hit (`combat.rs:522`) |
| 8 | Cyclone | **Missing** | Catalog row "Cyclone: Partial", but no code | Needs suspend (stun + invulnerable) + lift + immunity to other displacement |
| 9 | Forced movement (generic) | **Partial** | `ForcedMovement` + `tick_forced_movement` (`displacement.rs:25-109`); fear forced flee (`movement.rs:164-178`) | Kept from conflicting with walking only by the root status. No priority, no terrain checks, `NavPath` goes stale afterwards |
| 10 | Movement restriction (region) | **Missing** | Catalog "Leash: Missing, P3" | |
| 11 | Phasing | **Done** | `StatusSpec::Phased`, `StatusEffects::is_phased`; `movement.rs:137,230,398`; `teleport.rs:133-143` | None (forced motion will phase by default, Step 1) |
| 12 | Flying | **Missing** | none | `NavGrid` is a single ground layer; no status flag |
| 13 | Movement speed modifier | **Done** (with gaps) | `StatusSpec::Slow` (% of current), `Modifier.move_speed` (flat, generator `stat_modifier,move_speed`), `PassiveStat::MoveSpeed` | No floor: stacked negative flats make `move_speed <= 0`, which `apply_move_targets:152` treats as stationary (a silent root). No cap |
| 14 | Turn rate modifier | **Partial** | `StatusSpec::TurnRateSlow`, `CombatStats::effective_turn_rate` (`components.rs:171`), used by `movement.rs:262`, `basic_attack.rs:199`, `actions/pipeline.rs:402` | Can only slow; nothing increases it |
| 15 | Movement tether | **Missing** | none | Same primitive as #10, but anchored to an entity |
| 16 | Reposition to valid terrain | **Partial** | `NavGrid::nearest_walkable` (`grid.rs:314`), used only by path planning | Never applied to unit positions after displacement or flying, or when a cyclone ends |

---

## Design

### Ownership (unchanged boundaries)

- `EffectRunner::execute` (`abilities/effects.rs`) stays the only place where an `AbilityEffect` becomes a message. Mechanics write `DisplacementEvent` directly, as they already write `StatusEffectEvent` and `SpawnProjectileEvent`.
- `displacement.rs` is the **only** writer of `Transform` for units that have `ForcedMovement`.
- `teleport.rs` stays the only instant relocator.
- `movement.rs` owns walking, collision and the leash clamp.
- `status.rs` owns durations, dispels and immunity for every timed constraint.

### 1. Shared primitive: `ForcedMovement` (in `src/displacement.rs`)

```rust
pub enum MotionPath {
    /// Straight segment (knockback, pull_point, dash, fixed pull).
    Line { from: Vec3, to: Vec3 },
    /// Chase `anchor` at `speed` until within `stop_distance` or `max_travel` is spent
    /// (tracking pull / hook, charge). Falls back to the last known anchor position.
    Toward { anchor: Entity, speed: f32, stop_distance: f32, max_travel: f32, travelled: f32, last_anchor: Vec3 },
    /// Keep `offset` from `anchor` while the anchor itself has a ForcedMovement (drag).
    Attached { anchor: Entity, offset: Vec2 },
    /// Stay at `at` (`at.y` = ground height), lifted by `height` (cyclone).
    Hold { at: Vec3, height: f32 },
}

#[derive(PartialOrd, Ord, PartialEq, Eq, Clone, Copy, Debug)]
pub enum MotionPriority { Own, External, Suspend }

/// Copy-able trigger identity carried by self-motion so it can report contacts / end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSource { pub source: EffectSource, pub caster: Entity, pub team: Team, pub rank: u32, pub origin: Vec3, pub aim: Vec3, pub cast_target: Option<Entity> }

#[derive(Component, Debug, Clone, PartialEq)]
pub struct ForcedMovement {
    pub path: MotionPath,
    pub elapsed: f32,
    pub duration: f32,              // hard cap for every path
    pub priority: MotionPriority,
    pub ignore_terrain: bool,       // flying target or Hold
    pub bound_status: Option<&'static str>, // end early when this status id is gone ("cyclone")
    pub source: Option<MotionSource>,       // Some → emits OnUnitContact / OnMovementEnd
    pub hit: Vec<Entity>,           // contacted units, deduplicated
}
impl ForcedMovement { pub fn line(from: Vec3, to: Vec3, duration: f32) -> Self { .. } }
```

`ForcedMovement` stops being `Copy`. That affects `teleport.rs:455`, which should use `ForcedMovement::line`.

**`DisplacementEvent`** gains `aim: Vec3`, `cast_target: Option<Entity>`, `speed: Option<f32>` and `source: Option<MotionSource>`. Add a `DisplacementEvent::from_ctx(ctx, target, kind, distance, duration, speed)` helper. `EffectRunner` and the Dash mechanic use it.

**`DisplacementKind`** (`definition.rs:357`) gains variants. It stays `Copy + Eq`.

| Kind | Mover | Path | Priority | Notes |
|---|---|---|---|---|
| `Knockback` (exists) | target | `Line` away from `point` (fallback: `origin`) | External | |
| `Pull` (exists, changed) | target | `Toward { anchor: caster, stop = bounds sum, max_travel = distance or ∞ }` | External | Now tracks the caster, which is what a hook needs. Existing test result is unchanged (static caster ends at contact) |
| `PullToPoint` (new) | target | `Line` toward `point`, capped by `distance` | External | Vortex-style "draw toward a point" |
| `Dash` (new) | target, normally the caster | `Line` toward `aim`, capped at `distance`. Duration = `duration × actual/distance` (constant speed) | Own | Also faces the travel direction at start |
| `Charge` (new) | target, normally the caster | `Toward { anchor: cast_target, speed, stop = bounds sum, max_travel = distance or ∞ }` | Own | Rejected without a living `cast_target` |
| `Drag` (new) | target | `Attached { anchor: caster, offset = (target−caster) clamped to bounds sum }` | External | Ends when the anchor's motion ends, or after `duration` if > 0 |
| `Cyclone` (new) | target | `Hold { at: pos, height: CYCLONE_HEIGHT }` | Suspend | Also writes the `cyclone` status (Step 5). `bound_status = "cyclone"` |

**`AbilityEffect::Displace`** gains `speed: Option<RankValue>`, which only Charge uses. The generator emits `speed: None` everywhere else. This is the only change that breaks generated code, so it lands in Step 1 together with the regeneration.

**`AbilityTrigger::OnMovementEnd`** (new; CSV `on_movement_end`), plus stock emission of the existing `OnUnitContact`:
- `OnUnitContact`: one per newly contacted living unit that is an enemy of `source.team`. Contact means the bounds circles overlap. `TriggerContext::new(OnUnitContact, …).at(mover_pos).cast_target(src.cast_target).trigger_unit(Some(unit))`. This closes shared-effects plan P1 #3.
- `OnMovementEnd`: once, when an Own motion with a source ends by arrival, block, timeout or interruption. It has `.at(end_pos).trigger_unit(anchor if reached).affected(hit)`.
- Replacement by a higher-priority motion, a teleport, death or despawn end the motion **silently**. This follows the "order-cancelled channel is silent" rule.

### 2. `apply_displacement_events` rules

1. The target must exist and be alive.
2. `hostile = target team != caster team`. Debuff immunity blocks only **hostile** requests. Today it also blocks a debuff-immune caster's own dash.
3. **Priority:** priority is `Own` if `target == caster`, `Suspend` for Cyclone, otherwise `External`. Insert only if `new >= current.priority`; the newest wins at equal priority. A unit in `Suspend` rejects everything except another Cyclone, which refreshes it.
4. **Own motion needs `can_move()`.** A rooted, stunned or slept caster does not dash or charge. Costs are already paid, which matches the current rule that "a rejected teleport still spends mana".
5. **No more "displaced" root** (decision D1). `apply_move_targets` skips movers that have `ForcedMovement`, so orders survive and resume afterwards.
6. `ignore_terrain = target.is_flying() || kind == Cyclone`.

### 3. `tick_forced_movement` rules (moved earlier in the chain, see Scheduling)

- Pass 1 (Line / Toward / Hold), then pass 2 (Attached), which reads anchor positions from pass 1. Use a `ParamSet` of a mutable mover query and a read-only snapshot of `(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)`.
- **Terrain stop:** unless `ignore_terrain`, if the next step is not `grid.point_clear(next, r)` while the current position *is* clear, end the motion where it is. A unit that starts inside geometry may still move, so it can escape.
- **Interrupt:** Own motion ends (with `OnMovementEnd`) when `!can_move()`. A `bound_status` that is no longer present ends a `Hold`.
- **Contacts:** run for motions with `source.is_some()`. They are deduplicated through `hit`.
- **Landing**, on every non-silent end, and also on a silent end caused by death: restore the `Hold` Y. Then, if the unit is not flying and not `grid.point_clear(pos, r)`, move it to `grid.nearest_walkable(pos, r)`. If that returns `None`, leave the unit where it is. Remove `NavPath` and `NavSteering`, so `plan_paths` replans the surviving `MoveTarget` from the new spot (no stale-waypoint snap-back).
- Shared helper: `pub fn reposition_to_valid_terrain(grid: &NavGrid, pos: Vec3, r: f32) -> Vec3` in `displacement.rs`. Step 6 reuses it.

### 4. Statuses (`src/status.rs`, `StatusSpec` in `definition.rs`)

| New field on `StatusEffect` | Query | Spec / constructor | Kind / dispel |
|---|---|---|---|
| `flying: bool` | `is_flying()` | `StatusSpec::Flying`, `StatusEffect::flying(d)` | Buff, Basic |
| `leash: Option<Leash>` with `Leash { anchor: LeashAnchor::{Point(Vec2), Unit(Entity)}, radius: f32 }` | `leashes() -> impl Iterator<Item=&Leash>` | `StatusSpec::Leash { radius: RankValue, anchor: LeashAnchorKind::{TriggerPoint, Caster} }` → ids `leash` / `tether` | Debuff, Basic |
| `turn_rate_bonus_percent: f32` | used inside `effective_turn_rate` | `StatusSpec::TurnRateBonus { percent }` | Buff, Basic |
| (reuse `stunned` + `invulnerable`) | `is_cycloned()` = id `"cyclone"` | `StatusEffect::cyclone(d, hostile)` | Debuff if hostile, else Buff; `undispellable`, `DispelType::None` |

`StatusBuildContext` gains `source: Option<Entity>` and `point: Vec3`. Update both construction sites: `effects.rs:373` and `passive.rs:115`. `fear_source` stays as it is.

`CombatStats::effective_turn_rate` becomes `base × (1 + Σbonus/100) × (1 − max slow/100)`.

`CombatStats::effective_move_speed(statuses)` (new):
- `raw = move_speed`; `base = raw − Σ status.move_speed`.
- If `base <= 0`, the result is `0`, so wards and other stationary units stay put.
- Otherwise the result is `raw.clamp(MIN_MOVE_SPEED, MAX_MOVE_SPEED)`, with the constants in `scale.rs` (100 / 550; decision D4).
- Used by `apply_move_targets` (step length and the `<= 0` check).

### 5. Consumers

- **`movement.rs`**
  - `MoverQuery` adds `Without<ForcedMovement>`.
  - The unit-collision snapshot, `SnapQuery` and the teleport blocker filter treat `Has<ForcedMovement>` as phased.
  - Flying units: skip `Obstacle` circles and AABBs in `apply_move_targets` (buildings are kept), skip the avoidance fallback `grid.point_clear`, and skip `resolve_obstacle_collisions` (add `Option<&StatusEffects>` to its query).
  - New `enforce_leashes` runs in the `SimSet::Movement` chain after `apply_move_targets` and before `resolve_obstacle_collisions`. For each leash whose anchor still exists (if the anchor entity is gone, the leash does nothing), it clamps the position to the circle around the anchor. That covers walking and forced motion alike.
- **`navigation/mod.rs::plan_paths`:** flying movers get a direct `NavRoute` to the goal clamped inside the map. No A*; buildings fall back to the collision safety net (decision D5).
- **`teleport.rs`:** new `TeleportError::Suspended` (target has `Suspend` motion) and `TeleportError::Leashed` (destination outside any leash circle). Both are checked before `validate_destination`.
- **`status.rs::apply_status_effect_events`:** when a `flying` effect is applied, insert the `SettleOnLanding` marker (the function already has `Commands`).
- **New `settle_grounded_units`** (`displacement.rs`). Query: `With<SettleOnLanding>`. Skip units that are flying or have `ForcedMovement`. Otherwise call `reposition_to_valid_terrain` and remove the marker. It runs in the abilities chain right after `tick_forced_movement`.

### Scheduling (`AbilitiesPlugin`, `abilities/mod.rs:93-112`)

Move `tick_forced_movement` and `settle_grounded_units` to just **before** `resolve_ability_triggers`, after `item_attack_triggers`. That way contact and end triggers resolve in the same frame, and a `Drag` from `OnUnitContact` is inserted in that same frame by `apply_displacement_events`. The rest of the inner chain is unchanged: `apply_status_effect_events` stays where it is, and `apply_teleport_events.before(SimSet::Ai)` too.

Ordering that follows: forced motion → teleport → AI → movement (`enforce_leashes`, then collisions).

Update the hand-listed test apps in the same commit: `abilities/mod.rs::test_app` (around line 640) and `teleport.rs::sim_app`. `items.rs::item_app` does not list displacement systems, so it is only affected if a test needs them.

### How content exposes each mechanic

The generator `movement` effect type (`generator_common.py:145`, `MOVEMENTS`) is extended. Columns used: `value` = distance, `duration` = seconds, `amount` = speed.

| Mechanic | CSV | Rust |
|---|---|---|
| Teleport | `movement,teleport` (unchanged) | `AbilityEffect::Teleport` |
| Dash | `movement,dash`. `value` = max distance (required), `duration` = time at max distance (default 0.3). Target `caster`/`owner` only | `Displace{Dash}`. The handwritten `AbilityMechanic::Dash` writes the same event plus ghost FX |
| Charge | `movement,charge`. `amount` = speed (required), `duration` = max time (required), `value` = max distance (optional). Target `caster`/`owner`; the ability needs `target_type unit`; on items the generator rejects anything except `unit` activation | `Displace{Charge, speed}` |
| Knockback | `movement,knockback` (unchanged) | `Displace{Knockback}` |
| Pull | `movement,pull` (unchanged; now tracks the caster) and `movement,pull_point` (`value` optional) | `Displace{Pull}` / `Displace{PullToPoint}` |
| Drag | `movement,drag`. `duration` optional; blank means until the source stops. Target must not be `caster`/`owner` | `Displace{Drag}`; typically `on_unit_contact,trigger_unit` |
| Hook | Composition: skillshot ability + `on_projectile_hit,trigger_unit,movement,pull`. With Step 9: `projectile_type skillshot_single` | |
| Cyclone | `movement,cyclone`. `duration` required | `Displace{Cyclone}` (motion + status in one row) |
| Forced movement | Implicit in every kind above | `ForcedMovement` |
| Region restriction | `apply_status,leash`. `value` = radius (required), anchor = trigger point | `StatusSpec::Leash{TriggerPoint}` |
| Tether | `apply_status,tether`. `value` = radius, anchor = caster | `StatusSpec::Leash{Caster}` |
| Phasing | `apply_status,phased` (unchanged) | |
| Flying | `apply_status,flying` | `StatusSpec::Flying` |
| Move speed | `stat_modifier,move_speed`, `apply_status,slow` (unchanged), plus the floor/cap | |
| Turn rate | `apply_status,turn_rate_slow` (unchanged), `apply_status,turn_rate_bonus` (`amount` %, 0..=400) | `StatusSpec::TurnRateBonus` |
| Reposition | Automatic: forced-motion landing and flying expiry | `reposition_to_valid_terrain` |

Trigger tables:
- `ABILITY_TRIGGERS` and `ITEM_TRIGGERS` gain `on_movement_end`.
- `ITEM_TRIGGERS` also gains `on_unit_contact`, so items can dash and charge with contact rows.
- AbilityGenerator `effect_context`: add `on_movement_end` to the passive-invalid list (`AbilityGenerator.py:241`) and the cast_target combo list (251).
- ItemGenerator: the matching `invalid_combo` entries (333-339).

`STATUSES` gains `flying`, `leash`, `tether`, `turn_rate_bonus`. Data schema: **no new CSV columns** and no header change.

---

## Interactions (normative)

| Situation | Rule |
|---|---|
| Stun, sleep or root during **Own** motion (dash/charge) | Motion ends at the current position, lands, fires `OnMovementEnd` |
| Stun or root during **External** motion (knockback/pull/drag) | Motion continues; statuses tick independently |
| Hostile displacement on a debuff-immune unit | Rejected (unchanged). Own and ally motion are allowed |
| New displacement on a unit in a cyclone | Rejected (Suspend). Teleport is rejected with `Suspended` |
| Dispel / `remove_status cyclone` | `cyclone` is undispellable. Explicit remove ends the Hold through `bound_status` |
| Cyclone and incoming spells | Damage is blocked (invulnerable); statuses still apply. Untargetable is out of scope (catalog: Banish, P3) |
| Fear during forced motion | Forced motion wins; movement skips the unit. Fear steering resumes afterwards |
| Leash and forced motion | The clamp applies after forced motion every frame, so a knockback stops at the leash edge |
| Leash and teleport | Destination outside the circle → `Leashed` |
| Flying and forced motion | `ignore_terrain`; lands on expiry via `SettleOnLanding` |
| Motion into a tree or the map edge | Stops at the last clear position, then lands |
| Unit overlap after landing | Resolved by the normal `resolve_unit_collisions` (forced movers phase only while moving) |
| Teleport during External / Own motion | Allowed; removes `ForcedMovement` (existing). No `OnMovementEnd` |
| Death or despawn mid-motion | Silent end. Drag victims end when the anchor's `ForcedMovement` is gone |

## Invariants

1. Only `displacement.rs` writes the `Transform` of an entity with `ForcedMovement`. `movement.rs` never steers it, apart from the leash clamp and the collision safety net.
2. At most one `ForcedMovement` per entity. Higher or equal priority replaces it; lower is rejected.
3. Every non-silent end lands the unit on valid terrain (or leaves it in place when `nearest_walkable` is `None`) and clears `NavPath` / `NavSteering`.
4. Contact and end triggers go through `AbilityTriggerEvent` → `resolve_ability_triggers`. `displacement.rs` never applies damage or statuses itself, except the `cyclone` status write, which mirrors today's "displaced" write.
5. Every new timed constraint is a `StatusEffect` field, so dispel, debuff immunity and expiry are unchanged code paths.
6. All cleanup-plan invariants hold (single executor, chain positions, generated code changed only by regeneration).

---

## Implementation steps (one commit each)

**Step 0: baseline.** Run V and record the `cargo test` pass count (cleanup plan, Phase 0).

**Step 1: primitive refactor, behaviour-preserving apart from D1.**
- `displacement.rs`: `MotionPath`, `MotionPriority`, `MotionSource`, new `ForcedMovement`, extended `DisplacementEvent`, terrain stop, landing, `reposition_to_valid_terrain`, priority, the hostile-only immunity check, and removal of the root write.
- `definition.rs`: add `Displace.speed`.
- `effects.rs:405-421`: `DisplacementEvent::from_ctx`.
- `movement.rs`: `Without<ForcedMovement>` on movers; forced movers phase in the collision snapshot.
- `teleport.rs`: blocker filter; test fixture uses `ForcedMovement::line`.
- `tooltip.rs:286-300`.
- Generator: emit `speed: None`. Regenerate `generated.rs` and `item_catalog.rs`.
- Move `tick_forced_movement` in the chain and in the test apps.

**Step 2: tracking pull, pull-to-point, contact and end triggers.**
- `Toward` path; `DisplacementKind::PullToPoint`.
- `AbilityTrigger::OnMovementEnd` (`definition.rs`, `tooltip.rs::trigger_tag`, `items.rs` describer).
- Contact/end emission in the tick.

**Step 3: Dash and Charge.**
- `DisplacementKind::{Dash, Charge}`.
- `MechanicRunner` gets a `MessageWriter<DisplacementEvent>`. `dash()` keeps the ghost FX, writes `DisplacementEvent{Dash}` with `duration = distance / DASH_SPEED` (`DASH_SPEED` in `scale.rs`, default 1200), and stops inserting `MoveTarget` and phased.
- README:347 changes from "Phased (Dash)" to "forced movement phases".

**Step 4: Drag.** `Attached` path, the second tick pass, `DisplacementKind::Drag`.

**Step 5: Cyclone.**
- `DisplacementKind::Cyclone`, `Hold` with Y lift, `StatusEffect::cyclone`, `bound_status`.
- `TeleportError::Suspended`.

**Step 6: Flying.**
- `StatusSpec::Flying` and the field.
- Movement and obstacle-collision skips; direct route in `plan_paths`.
- `SettleOnLanding` + `settle_grounded_units`.

**Step 7: Leash / tether / region.**
- `Leash` field, `StatusSpec::Leash`, `StatusBuildContext.{source, point}`.
- `enforce_leashes` in `MovementPlugin`.
- `TeleportError::Leashed`.

**Step 8: speed modifiers.**
- `turn_rate_bonus_percent` + `StatusSpec::TurnRateBonus`.
- `effective_turn_rate` formula; `effective_move_speed` + constants.

**Step 9: generator, data and examples.**
- `MOVEMENTS` + per-kind validation (table above); `STATUSES` additions; trigger tables.
- `data/examples/ability_effects.csv`:
  - a dash row, a charge row with `on_movement_end` damage + stun, and a skewer-style row using `movement,dash` + `on_unit_contact` `movement,drag` (replacing the two `custom,skewer_*` rows);
  - a hook-composition row, a cyclone row, a leash row, a flying row, a turn_rate_bonus row.
- `data/examples/invalid/*`: dash on `cast_target`; charge without a unit target; drag on caster; leash without `value`; `on_movement_end` on a passive.
- Production `data/*.csv` is untouched; the regeneration diff must be empty.

**Step 10 (optional, decision D3): first-hit skillshot for hooks.**
- `AbilityMechanic::Projectile.first_hit: bool` → `Skillshot.max_hits = Some(1)` (`combat.rs:517-560`).
- `projectile_type skillshot_single` in AbilityGenerator.

**Step 11: docs.**
- README:
  - effect list (326), Limitations (379, remove "Displacement ignores obstacles"), status table (181-188), layout (427);
  - a new "Forced movement" section next to Teleport (369-375).
- Add a dated note on the catalog rows Leash / Cyclone / Turn rate.

---

## Affected files

`src/displacement.rs` (main), `src/abilities/definition.rs`, `src/abilities/effects.rs`, `src/abilities/mechanics.rs`, `src/abilities/mod.rs`, `src/abilities/tooltip.rs`, `src/abilities/passive.rs` (StatusBuildContext), `src/status.rs`, `src/components.rs` (`effective_*`), `src/movement.rs`, `src/navigation/mod.rs`, `src/teleport.rs`, `src/items.rs` (describer only), `src/scale.rs`, `src/combat.rs` (Step 10 only), `scripts/generator_common.py`, `scripts/AbilityGenerator.py`, `scripts/ItemGenerator.py`, `scripts/test_generator_common.py`, `data/examples/**`, regenerated `src/abilities/generated.rs` and `src/item_catalog.rs`, `README.md`.

Not touched: `src/net/*`, `src/heroes.rs`, `Cargo.toml`, the production CSV headers.

---

## Regression tests (`cargo test`)

New `#[cfg(test)] mod tests` in `src/displacement.rs`. Its `sim_app` is modelled on `teleport.rs::sim_app`:
- `MovementPlugin`, `NavGrid`;
- messages: `DisplacementEvent`, `StatusEffectEvent`, `AbilityTriggerEvent`, `DamageEvent`, `TeleportEvent`;
- systems `(tick_forced_movement, settle_grounded_units, apply_displacement_events, apply_status_effect_events, tick_status_effects).chain().before(SimSet::Ai)`.

| Test | Location | Step |
|---|---|---|
| `knockback_stops_at_a_tree_instead_of_passing_through` | displacement.rs | 1 |
| `knockback_stops_at_the_map_edge` | displacement.rs | 1 |
| `unit_starting_inside_an_obstacle_can_still_be_displaced_out` | displacement.rs | 1 |
| `landing_inside_geometry_relocates_to_nearest_walkable` | displacement.rs | 1 |
| `knockback_preserves_move_orders_and_replans_after` (MoveTarget kept, NavPath reset) | displacement.rs | 1 |
| `walking_does_not_fight_forced_movement` | displacement.rs | 1 |
| `debuff_immune_target_ignores_hostile_but_not_own_displacement` | displacement.rs | 1 |
| `lower_priority_motion_does_not_replace_higher` | displacement.rs | 1 |
| `forced_movers_ignore_unit_collision_while_moving` | displacement.rs | 1 |
| `displace_effect_pulls_the_target_toward_the_caster` (existing; replace the `was_rooted` assertion with ForcedMovement present + the 48.0 contact result) | abilities/mod.rs | 1 |
| `teleport::stale_orders_do_not_pull_the_unit_back` (existing, using `ForcedMovement::line`) | teleport.rs | 1 |
| `teleport_is_ordered_consistently_in_the_real_schedule` (existing; no cycle after the move) | teleport.rs | 1 |
| `pull_tracks_a_moving_caster_until_contact` | displacement.rs | 2 |
| `pull_with_distance_stops_after_max_travel` | displacement.rs | 2 |
| `pull_to_point_ends_at_the_trigger_point` | displacement.rs | 2 |
| `own_motion_reports_each_contacted_enemy_once` | displacement.rs | 2 |
| `movement_end_fires_once_with_contacts_as_affected_units` | displacement.rs | 2 |
| `replaced_or_teleported_motion_fires_no_end_trigger` | displacement.rs | 2 |
| `hook_composition_pulls_the_skillshot_victim_to_the_caster` | abilities/mod.rs | 2 |
| `dash_ability_moves_at_dash_speed_and_caps_distance` | abilities/mod.rs | 3 |
| `dash_is_not_cancelled_by_a_new_move_order` | abilities/mod.rs | 3 |
| `rooted_caster_pays_but_does_not_dash` | abilities/mod.rs | 3 |
| `stun_mid_dash_ends_it_and_fires_movement_end` | abilities/mod.rs | 3 |
| `item_dash_uses_the_same_effect` (`install_test_item_definitions`) | abilities/mod.rs | 3 |
| `charge_tracks_target_and_fires_end_with_trigger_unit` | abilities/mod.rs | 3 |
| `charge_without_living_target_does_nothing` | displacement.rs | 3 |
| `catalog::rank_values_match_legacy_formulas` (existing, unchanged) | abilities/catalog.rs | 3 |
| `drag_carries_contacted_units_with_the_dash` (skewer-style definition) | abilities/mod.rs | 4 |
| `drag_ends_when_the_source_stops` | displacement.rs | 4 |
| `cyclone_stuns_lifts_and_restores_height` | displacement.rs | 5 |
| `cyclone_blocks_damage_displacement_and_teleport` | displacement.rs + teleport.rs | 5 |
| `cyclone_survives_strong_dispel_and_ends_on_remove` | displacement.rs | 5 |
| `flying_unit_walks_through_trees_but_not_towers` | navigation/tests.rs | 6 |
| `flying_expiry_over_a_tree_lands_on_walkable_ground` | displacement.rs | 6 |
| `leash_clamps_walking_and_knockback_to_the_region` | movement.rs tests (small app) | 7 |
| `tether_follows_a_moving_anchor` | movement.rs tests | 7 |
| `leash_is_dispellable_and_blocked_by_debuff_immunity` | status.rs | 7 |
| `leashed_unit_cannot_teleport_outside` | teleport.rs | 7 |
| `turn_rate_bonus_and_slow_combine` | components.rs or status.rs | 8 |
| `stacked_slows_respect_the_move_speed_floor` | components.rs | 8 |
| `zero_base_speed_units_stay_stationary_with_haste` | components.rs | 8 |
| `test_movement_dash_requires_caster_target`, `test_charge_requires_unit_target_and_speed`, `test_drag_rejects_caster`, `test_leash_requires_radius`, `test_on_movement_end_in_trigger_tables`, `test_cyclone_requires_duration` | scripts/test_generator_common.py | 9 |
| `first_hit_skillshot_stops_on_first_enemy` | combat.rs | 10 |

Run with `cargo test`. Focused filters: `cargo test displacement::`, `cargo test teleport::`, `cargo test abilities::tests::dash`. Also run the full **V** set, including generator `--check` on `data/examples/*.csv`, which must pass, and on `data/examples/invalid/*.csv`, which must report the new errors.

---

## Risks

| Risk | Mitigation |
|---|---|
| Event ordering: contact triggers one frame late | `tick_forced_movement` moves before `resolve_ability_triggers`. Tests assert the same frame |
| Duplicate contact triggers | Deduplicated per motion through `hit`. Replacement starts a new motion with a new `hit`, which is intended |
| Stale movement orders after displacement | `NavPath` / `NavSteering` cleared on landing; `MoveTarget` kept (D1) |
| Unit command `Move` completing early | It used to complete when the root deleted `MoveTarget`; it now waits. Covered by `knockback_preserves_move_orders…` |
| Dash behaviour change (straight line that stops at trees instead of pathing around them) | Intended "rapid movement" semantics; call it out in the PR |
| Generated code compatibility | Only `Displace.speed` breaks it; regenerate in Step 1. Every other addition is a new enum variant or field outside generated code |
| Module cycle (`displacement` ↔ `abilities::effects`) | Rust handles intra-crate cycles; `displacement` already imports `abilities::definition`. Keep `MotionSource` in `displacement.rs` |
| Leash clamp jitter against path planning | Clamp before collisions; the stuck tracker replans. Acceptable for v1 |
| `nearest_walkable` search limit (about 1500 units) returns `None` | Leave the unit in place; the collision safety net still applies |
| Hand-listed test apps miss moved or new systems | Grep for `tick_forced_movement` in every step (cleanup-plan rule) |

## Decisions needing approval

- **D1 (recommended):** drop the "displaced" root, so displaced units keep their orders. The alternative is to keep the root (orders lost) and keep the existing `was_rooted` assertion.
- **D2:** should hostile displacement interrupt channels on its own? Recommended: no. Designers add a stun or mini-stun row.
- **D3:** include Step 10 (first-hit skillshot) in this PR, or leave hooks piercing?
- **D4:** move-speed floor/cap values (100 / 550, Dota-like) and whether to have a cap at all.
- **D5:** flying pathing. A straight line plus building collision (v1, recommended), or a second `NavGrid` clearance layer without trees (more cost, correct routing around buildings).
- **D6:** should cyclone also make the unit untargetable or immune to new debuffs? Recommended: defer to the Banish / untargetable work (catalog P3).
- **D7:** tether break-on-distance, as opposed to a clamp. Out of scope unless requested.
- **D8:** `DASH_SPEED` default (1200) and `CYCLONE_HEIGHT`.

## Approved decisions (user, 2026-11-07)

These override the recommendations above where they differ.

- **D1: approved.** Remove the "displaced" root. Displaced units keep their orders.
- **D2: no.** Hostile displacement does not interrupt channels by itself.
- **D3: approved.** Include Step 10, the first-hit skillshot for hooks.
- **D4: floor only.** `MIN_MOVE_SPEED = 100` stays. Do **not** add `MAX_MOVE_SPEED` and do not cap speed. Units whose base speed is 0 still stay stationary. Rename the test `stacked_slows_respect_the_move_speed_floor` as needed and add `haste_is_not_capped`.
- **D5: straight line.** Flying units take a direct route and rely on building collision.
- **D6: changed. A cycloned unit is also untargetable.** The codebase has no untargetable concept yet. Add the smallest version that works:
  - a `StatusEffects::is_untargetable()` query, true while `cyclone` is active;
  - the unit cannot be chosen as a unit target by other units: unit-targeted casts are rejected at validation, basic attacks and auto-acquire skip it, and AI target selection skips it;
  - homing projectiles already in flight toward it are dropped or miss;
  - skillshots and area effects do not affect it. Damage is already blocked by invulnerability; statuses and displacement from other units are also skipped.

  Its own caster's targeting is unaffected. Reuse existing target-filter helpers rather than adding parallel checks.

  Tests to add:
  - `cycloned_unit_cannot_be_targeted_or_attacked`;
  - `in_flight_projectile_at_cycloned_unit_does_not_hit`.

  Leave immunity to new debuffs out of scope, except where skipping effects on untargetable units already covers it.
- **D7: no.** Tethers clamp and never break.
- **D8:** use the proposed `DASH_SPEED = 1200`; choose a sensible `CYCLONE_HEIGHT` constant in `scale.rs`.
