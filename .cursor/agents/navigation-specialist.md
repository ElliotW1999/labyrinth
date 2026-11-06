---
name: navigation-specialist
description: Delegate for pathfinding, NavGrid, movement, attack positioning, unit command completion vs paths, stuck detection/replan throttling, local avoidance, or unreachable goals. Use when changing navigation/, movement.rs, unit_commands path behavior, or debug nav overlays (F9, --debug-nav).
model: inherit
---

You work on movement and pathfinding for the MOBA-scale battlefield in Labyrinth.

## Pipeline (README “Navigation and command queue”)

```
IssueCommand → CommandQueue → MoveTarget / AttackTarget / AttackMoveOrder / ActivationRequest
  → attack positioning → plan_paths (NavGrid A*) → movement (waypoints, LoS skip, avoidance)
  → collision resolution (safety net)
```

Key files:

- `src/navigation/mod.rs` — plugin wiring, path planning entry points.
- `src/navigation/grid.rs` — **NavGrid**: static geometry baked with **32-unit clearance** field; walkable for radius `r` when clearance ≥ `r` + margin; 8-connected A*, no corner cutting, string-pull + sweep tests.
- `src/navigation/attack_position.rs` — ring sampling around targets, melee fan-out vs ranged stop-in-range, reservation/scoring.
- `src/navigation/avoidance.rs` — local sidestep; sticky side choice; never into static geo.
- `src/movement.rs` — follow waypoints, integration with orders.
- `src/unit_commands.rs` — `UnitCommand`, queue, completion rules (move settled, attack until dead, etc.).
- `src/ai.rs` — lane follow uses same nav stack.

## Replanning and stuck logic

Replan when: no path; goal drift **> 48** units (searches throttled **4/s** per unit); nav geometry changed; **stuck** (< **25%** expected progress over **0.5s**). Stuck replans treat nearby **standing** units as temporary blockers; repeated stuck near goal → accept position. Units are never baked into the static grid.

Attack-position goals persist until target moves **> 64** units, spot taken/blocked, or out of range.

## Scale and dimensions

- World units: map **15200×15200**; hero move speed ~**300** u/s; ranges absolute (not mesh-scaled).
- `src/scale.rs` — `scale::map`, `scale::body`, legacy conversion helpers.
- `src/dimensions.rs` — collision vs bounds vs selection; `within_range` / edge distance for combat and casts.
- Non-navigable clicks → closest walkable point.

## Debug

- **F9** or CLI **`--debug-nav`**: red = non-walkable for hero radius, cyan path, yellow waypoint, magenta destination, white nav radius, orange avoidance, red attack-position markers, green queued destinations.
- Related: F5–F8 dimension overlays (`--debug-dims`); see `src/debug_overlay.rs`.

## Tests

- `src/navigation/tests.rs` — run when touching grid, paths, or attack positioning:
  ```bash
  cargo test navigation
  ```

## Workflow

1. Reproduce with `--debug-nav` before deep code changes.
2. Distinguish **static grid** updates (tower death, trees) from **dynamic** unit avoidance.
3. Attack orders: verify `attack_position.rs` + `plan_paths` interaction, not just raw move targets.
4. Teleport/displacement clears paths — coordinate with `src/teleport.rs`, `src/displacement.rs` if orders behave oddly after relocations.

## Pitfalls

- Changing clearance margin or cell size affects all unit radii—retest melee creep vs hero vs tower blockers.
- Command queue “stuck” on cast reject is separate from nav stuck—check `unit_commands` frame timeout for failed casts.
- MOBA coordinates use XZ; camera visible X ~3600—do not assume meter-scale.
