# Item and ability architecture cleanup: implementation plan

**Date:** 2026-11-07
**Based on:** the architect review of 2026-11-07 (HEAD `ffa4dfc`), plus `2026-11-06-shared-effects-pipeline.md`, `2026-11-06-activation-targeting-mechanics.md`, `2026-11-06-generator-shared-architecture.md`, `2026-11-06-damage-mechanics.md` and `2026-11-06-status-effects-catalog.md`.
**Scope:** incremental cleanup of the shared activation, trigger and effect code for abilities and items. Every phase compiles, passes its tests and can be committed on its own.

## Binding decisions (from the user)

1. Add a new trigger, **`OnChannelInterrupted`**.
   - `OnChannelEnd` fires only when a channel **completes**.
   - `ActionChannelEndEvent.completed` picks which trigger fires, for both abilities and items.
   - The CSV value is `on_channel_interrupted`, and both generators support it.
2. If a unit target dies during the cast point, the action **still fires**, at the target's last known position. It is not cancelled.
3. **Item charges do not regenerate.** ItemGenerator **rejects** `charge_restore_time` on items with an error.
4. **Multiplayer is out of scope.** No network messages, no snapshots, no shop buyer entity, no new authority gating. The one exception is moving item cooldown ticking into the gated chain (Phase 9), which was explicitly requested.
5. SeismicSlam, ArcaneLance, Cataclysm, StoneSkin and Overcharge are documented as **test content**. They are not added to any loadout.

## Architect choices (not decided by the user)

- Item `charge_restore_time` is rejected with an error, not silently ignored.
- `distance` scaling is rejected on all items.
- The unused `src/effects/mod.rs` is deleted.
- Overcharge becomes data through a new `while_toggled` trigger and a `toggle_mana_per_sec` column.
- Cancelling a channel with a new order stays silent: no channel trigger.

## Invariants (must hold after every phase)

- `EffectRunner::execute` (`src/abilities/effects.rs`) stays the only place where an `AbilityEffect` becomes gameplay messages. Do not add an item-specific executor.
- Status changes go only through `StatusEffectEvent` → `apply_status_effect_events`. The exceptions that already exist (expiry ticking, sleep waking on damage in `combat.rs`) stay where they are.
- Costs are paid when the action fires. Cancelling during the cast point is free (`actions/pipeline.rs::fire`).
- An item's `OnUse` context keeps `point = owner position` and `aim = cast aim`. The test `items.rs::item_use_trigger_keeps_the_owner_point_and_the_aim` must keep passing unchanged.
- `apply_status_effect_events` keeps its place in the `AbilitiesPlugin` chain (`abilities/mod.rs:101-111`). `tick_status_effects` keeps `.before(crate::combat::apply_damage_events)`.
- Cancelling a channel with a new order (move, stop, attack) stays silent: no channel trigger.
- Generated Rust (`src/abilities/generated.rs`, `src/item_catalog.rs`) is changed only by re-running the generators.

## Do not change

- `src/net/*`, the shop request types (`PurchaseItemRequest`, `SellItemRequest`), and the `is_sim_authority` gating, except the Phase 9 move of item cooldown ticking.
- Hero loadouts in `src/heroes.rs`.
- `Cargo.toml`.
- The `Ability*` type names. Don't rename them, and don't move any modules beyond the `status.rs` move in Phase 2.
- Damage numbers and mitigation in `combat.rs`, apart from the one shield-consumption call added in Phase 3.
- The CSV header of `data/abilities.csv`, except the one optional column added in Phase 11.

## Shared verification commands

Run these at the end of every phase; they are referred to as **V** below.

```bash
cd /home/user/labyrinth
cargo test                                                    # see Phase 0 for the environment fix
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py --check
PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py --check
(cd scripts && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest test_generator_common)
# Determinism: re-running the generators must not change anything on disk
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py && PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py && git diff --exit-code -- src/abilities/generated.rs src/item_catalog.rs src/components.rs src/items.rs
```

The last command only applies when the phase didn't intend to regenerate. When it did, commit the regenerated files and run it again; the second run must produce no diff.

---

## Phase 0: test environment and baseline

**Goal:** be able to run `cargo test`.

**Finding:**
- The Bevy 0.19 default features pull in `wayland-sys`, `alsa-sys`, `libudev-sys` and `xkbcommon-dl` (see `Cargo.lock`).
- The runtime libraries are installed (`libwayland-client0`, `libasound2t64`, `libudev1`), but the pkg-config files `wayland-client.pc` and `alsa.pc` are not.
- The build fails in the `wayland-sys` build script: "Package wayland-client was not found".
- The tests themselves are headless (`App::new()` with no `DefaultPlugins`), so only building and linking need these packages.

**Workaround, in order of preference:**
1. Install Bevy's documented Linux build dependencies. This changes the system, not the repo, and needs root plus network access through the proxy:
   ```bash
   apt-get update && apt-get install -y pkg-config libwayland-dev libxkbcommon-dev libasound2-dev libudev-dev
   ```
2. If apt is unavailable, there is no compliant way to build the tests. Turning off Bevy default features would mean editing `Cargo.toml`, which is out of scope. Don't commit such an edit. Report the blocker to the coordinator and fall back to the Python checks plus `cargo check` on a machine that has the packages.

**Steps:**
1. Install the packages.
2. Run `cargo test` and record the pass count as the baseline in the PR description.

**Acceptance:** `cargo test` passes on an unmodified tree.

---

## Phase 1: small fixes, dead code and stale docs

**Goal:** remove known small defects and dead code, and fix docs that are already wrong.

**Touches:**
- `src/items.rs`: `spawn_item_use_fx` (around 1730-1757), and the stale doc comment at 1641.
- `src/abilities/catalog.rs`: `handwritten`, the `AbilityId::Dash | AbilityId::Blink` arm at 167-180, and the `ABILITY_BLINK_RANGE` import at 19.
- `src/effects/mod.rs`: delete the file. Also delete `mod effects;` at `src/main.rs:19`.
- `README.md`: lines 214, 238, 257, 341 and 360.

**Steps:**
1. Heal flash. In `spawn_item_use_fx`, call `spawn_heal_fx` only when `entry.target == EffectTarget::Caster` **and** `entry.effect` is `AbilityEffect::Heal { .. }`. Blink Dagger's teleport must no longer flash.
2. Dead Blink arm. The generated Blink (`generated.rs:205-222`) wins in `builtin`, so the hand-written arm never runs for Blink.
   - Reduce the arm to `AbilityId::Dash`, with distance `RankValue::linear(ABILITY_DASH_RANGE, 30.0)`.
   - Drop the `ABILITY_BLINK_RANGE` import if nothing else uses it (`grep -rn ABILITY_BLINK_RANGE src`).
3. `src/effects/mod.rs`: **remove it.** Nothing imports `crate::effects`. It hides its own unused imports behind `#![allow(unused_imports)]`, and every later move would have to keep its re-exports in sync. The shared-effects plan only marked it optional.
4. Docs:
   - README:214: add Blink to the list of generated abilities, and state that SeismicSlam, ArcaneLance, Cataclysm, StoneSkin and Overcharge are **test content**, not in any hero loadout (decision 5).
   - README:238: "silence, stun, sleep or mute block abilities; stun or mute block items".
   - README:257: channel ends are handled by `action_channel_end_triggers` in `abilities/casting.rs` for both sources.
   - README:341: "Phased (Dash)". Blink is now a teleport and does not phase.
   - README:360: list every `PASSIVE_STATS` key from `generator_common.py:158-178`.
   - `items.rs:1641`: name `action_channel_end_triggers`.

**Tests:**
- `items.rs`: add `blink_dagger_use_spawns_no_heal_flash`. Use Blink Dagger in `item_app` and assert that no entity carries the name or marker that `spawn_heal_fx` spawns (see `items.rs:1759`). Also add `vial_use_spawns_heal_flash` as the positive case.
- `catalog.rs`: assert that `builtin(AbilityId::Blink).mechanics` is empty and that it has an `OnCast` `Teleport` entry. This pins the generated definition.

**Verify:** V.

**Acceptance:** V passes, `src/effects/` is gone, and `grep -rn "item_channel_end_triggers\|ability_channel_end_triggers" README.md src` finds nothing.

---

## Phase 2: move status effects into `src/status.rs`

**Goal:** give status effects their own top-level module with a `StatusPlugin`, so that `actions`, `combat`, `movement` and the ability definitions stop depending on `items.rs` for statuses.

**What moves into `src/status.rs`:**
- From `src/items.rs:225-575`: `StatusEffects` and its impl, `StatusKind`, `DispelStrength`, `DispelType`, `StatusEffect` and its constructors, and `status_dispel_matches`.
- From `src/items.rs:1420-1556`: `tick_status_effects`, `apply_phased`, `apply_forceful`, `subtract_status_stat_delta`, `add_status_stat_delta`, `apply_status`, `apply_stun` and `apply_debuff_immunity`.
- From `src/abilities/effects.rs`: `StatusChange` and `StatusEffectEvent` (82-100), plus `remove_statuses` and `apply_status_effect_events` (478-552).
- These tests from `items.rs`: `status_buff_applies_and_expires_modifiers`, `phased_buff_flags_ignore_unit_collision`, `forceful_buff_enables_unit_push`, `debuff_immunity_blocks_stun` and `dispel_tiers_remove_basic_but_not_strong_debuffs`.

This code depends only on `CombatStats`, `DamageType`, `ShieldPool`, `HeroAttributes` and `DamageEvent`; it has no dependency on `ItemId`, `Inventory` or the shop.

### Step 2a: move, with temporary re-exports (one commit)

1. Create `src/status.rs` holding the code above, and add `mod status;` in `src/main.rs`.
2. Add `pub struct StatusPlugin;`. Its `build` registers `tick_status_effects.before(crate::combat::apply_damage_events)` in `Update`, with **no authority gate**, exactly as `items.rs:41` does today.
   - It also calls `add_message::<StatusEffectEvent>()`. Leave the existing `add_message` in `AbilitiesPlugin` in place; adding the same message twice is harmless.
3. Remove `tick_status_effects` from `ItemsPlugin` (`items.rs:41`). Add `StatusPlugin` to `main.rs` in the plugin tuple at 164, before `ItemsPlugin`.
4. Leave `apply_status_effect_events` **scheduled where it is**, in the `AbilitiesPlugin` chain (`abilities/mod.rs:105`). Only its path changes.
5. Add temporary re-exports:
   - In `src/items.rs`: `pub use crate::status::{StatusEffects, StatusEffect, StatusKind, DispelStrength, DispelType, status_dispel_matches, apply_status, apply_stun, apply_phased, apply_forceful, apply_debuff_immunity, subtract_status_stat_delta, add_status_stat_delta};`. Keep `pub(crate)` on the two stat-delta helpers.
   - In `src/abilities/effects.rs`: `pub use crate::status::{StatusChange, StatusEffectEvent, apply_status_effect_events};`.
6. Any test app that relied on `ItemsPlugin` to tick statuses also needs `StatusPlugin`. Check `src/teleport.rs:533`.

### Step 2b: migrate importers, regenerate, remove the re-exports (one commit)

1. Find importers with `grep -rn "items::\(.*\)\?\(Status\|Dispel\|apply_status\|apply_stun\|apply_phased\|apply_forceful\|apply_debuff\|subtract_status\|add_status\)" src`. Known sites:
   - `components.rs:171`
   - `abilities/definition.rs:9` and `407`
   - `abilities/mechanics.rs:27`
   - `abilities/tooltip.rs:10` and `303-304`
   - `abilities/casting.rs:146`
   - `abilities/mod.rs:40` and `1499`
   - `abilities/effects.rs:28-31`
   - `actions/pipeline.rs:18`
   - `actions/resources.rs:5`
   - `combat.rs:17`
   - `movement.rs:18`
   - `basic_attack.rs:28`
   - `displacement.rs:10`
   - `teleport.rs:23` and `182`
   - `units.rs:11`, `141` and `172`
   - `navigation/tests.rs:21`
   - `ui.rs` (wherever it uses status types)
   - `items.rs` itself

   Point every one at `crate::status::…`.
2. Change the import lines the generators emit:
   - `scripts/AbilityGenerator.py:113`: `use crate::status::{DispelStrength, StatusKind};`
   - `scripts/ItemGenerator.py:152-154`: move `DispelStrength` and `StatusKind` to a separate `use crate::status::{DispelStrength, StatusKind};`, keeping the rest under `crate::items`.
3. Regenerate (`python3 scripts/AbilityGenerator.py && python3 scripts/ItemGenerator.py`). Only the header `use` lines should change in `generated.rs` and `item_catalog.rs`.
4. Delete both temporary re-export blocks.
5. Docs: README:418 becomes "items.rs: shop, inventory, sell, actives". Add a line "status.rs: buffs and debuffs, dispel, shields, DoT ticking (`StatusPlugin`)" to the layout. Update the file index in `docs/plans/2026-11-06-status-effects-catalog.md` with a one-line note.

**Tests:** the moved tests pass unchanged in `status.rs`.

**Verify:**
- V, with the regeneration diff expected only in the header lines.
- `grep -rn "items::.*Status\|items::Dispel" src` returns nothing.

**Acceptance:**
- `src/items.rs` contains no status types.
- `actions/` imports nothing status-related from `items`.
- The generators produce the new headers deterministically.

---

## Phase 3: shield bookkeeping fixes

**Goal:** keep `ShieldPool.amount` equal to the remaining shield carried by the active shield statuses.

**Bugs being fixed:**
- A dispel or remove never touches `ShieldPool` (`remove_statuses`).
- Expiry subtracts the full original amount even after part of the shield was used (old `items.rs:1453-1457`).
- The pool is added to even when `apply_status` returns false (old `effects.rs:529-540`).

**Touches:** `src/status.rs` (`remove_statuses`, `apply_status_effect_events`, `tick_status_effects`, plus a new `consume_shields`) and `src/combat.rs` (`apply_damage_events`, around 315-325).

**Steps:**
1. Add `pub fn consume_shields(statuses: &mut StatusEffects, mut absorbed: f32)`. It walks `statuses.effects` in vector order, which is application order, and subtracts from each effect's `shield_amount` until `absorbed` is used up.
2. In `combat::apply_damage_events`, after `resolve_incoming_damage`, call `consume_shields(st, resolved.shield_absorbed)` when `resolved.shield_absorbed > 0.0` and `statuses.as_deref_mut()` is `Some(st)`.
3. Add `fn release_shield(effect: &StatusEffect, pool: Option<&mut ShieldPool>)`, which does `pool.amount = (pool.amount - effect.shield_amount).max(0.0)`. Because of step 2, `shield_amount` is now what remains of that shield.
   - Call it from `remove_statuses`, which now takes `Option<&mut ShieldPool>`. That covers both dispel and remove-by-id.
   - Call it from the expiry branch in `tick_status_effects`.
4. In the apply branch, add to the pool only if `apply_status(...)` returned `true`.

**Tests (in `status.rs` or `abilities/mod.rs`, using `test_app`):**
- `dispelling_a_shield_buff_clears_its_pool`
- `removing_a_shield_by_id_clears_its_pool`
- `partially_consumed_shield_expiry_leaves_other_shields_intact`: two shields with different ids; damage half of the first; expire the first; the second's full amount remains.
- `refreshing_a_shield_replaces_only_its_remaining_amount`
- The existing `combat.rs::shield_absorbs_before_health` must still pass. It uses a bare `ShieldPool` with no statuses.

**Verify:** V.

**Acceptance:** the new tests pass, and a dispelled Barrier shield absorbs no further damage.

---

## Phase 4: order `item_attack_triggers`

**Goal:** item `OnAttack` and `OnAttackHit` triggers resolve deterministically in the same frame as the attack.

**Touches:** `src/items.rs` (`ItemsPlugin::build` line 42; make `item_attack_triggers` `pub(crate)`) and `src/abilities/mod.rs` (the chain at 76-120).

**Steps:**
1. Remove `item_attack_triggers` from `ItemsPlugin`.
2. Insert `crate::items::item_attack_triggers` into the `AbilitiesPlugin` chain right after `projectile_hit_triggers` and before `resolve_ability_triggers`.
   - The chain already runs `.after(crate::combat::apply_projectile_hits)`, which comes after `resolve_basic_attack_impacts` in the combat chain, so this frame's attack events are visible.
   - Note: this puts the system inside the `is_sim_authority` gate. The events it reads exist only on the authority anyway, so behaviour does not change.

**Tests (`abilities/mod.rs`):**
- `item_on_attack_hit_resolves_in_the_same_frame`: install a test item with an `OnAttackHit` heal; write a `BasicAttackImpactEvent`; after one `step`, the heal is applied.
- `two_copies_of_one_item_fire_one_attack_trigger`: pins the `seen` dedup in `items.rs:1708-1714`.

The test app must add `BasicAttackReleaseEvent` and `BasicAttackImpactEvent` messages and list `item_attack_triggers` in its chain.

**Verify:** V.

**Acceptance:** both tests pass.

---

## Phase 5: one `source_params` helper; real item radius and range

**Goal:** one lookup supplies effect entries, area radius and cast range for any `EffectSource`.
- Items stop getting a hard-coded `0.0` (`effects.rs:441-451`, `abilities/mod.rs:256-261`).
- The generator rejects `distance` scaling on items.

**Touches:**
- `src/abilities/effects.rs`: `resolve_ability_triggers`, a new `SourceParams` and `source_params`, and `impl From<ActionId> for EffectSource`.
- `src/items.rs`: a new `ItemDefinition::aoe_radius()`.
- `src/abilities/mod.rs`: `begin_or_cast_from_hotkeys` at 256-261.
- `scripts/generator_common.py`: `effect_to_rust`, scaling block at 704-716.
- `data/examples/invalid/item_effects.csv`.

**Steps:**
1. Add the helper:
   ```rust
   pub struct SourceParams<'a> { pub entries: &'a [AbilityEffectEntry], pub aoe_radius: f32, pub cast_range: f32 }
   pub fn source_params<'a>(source: EffectSource, defs: &'a AbilityDefinitions, rank: u32) -> Option<SourceParams<'a>>
   ```
   - Ability: `(def.effects, def.aoe_radius.at(rank), def.cast_range.at(rank))`.
   - Item: `(def.effects, def.aoe_radius(), def.activation.cast_range)`.
2. `ItemDefinition::aoe_radius()` returns the largest `AreaRadius::Fixed(r).at(1)` among `UnitsInRadius` entries whose trigger is `OnUse` or `OnChannelEnd` (Phase 7 adds `OnChannelInterrupted`), or `0.0` if there are none.
3. In `resolve_ability_triggers`, use `source_params` once per trigger. This also removes the double `defs.get`.
4. In `begin_or_cast_from_hotkeys`, set `aoe_radius` from `source_params(spec.action.into(), &defs, spec.level)`.
5. Generator: in `effect_to_rust`, if `ctx.kind == "item"` and `scaling_type == "distance"`, report the error "distance scaling is ability-only (items have no rank-scaled cast range)". Add a matching row to `data/examples/invalid/item_effects.csv`.

**Tests:**
- `effects.rs` or `abilities/mod.rs`: `item_source_params_use_activation_range_and_fixed_radius`, for a point item with a 143-radius `UnitsInRadius`.
- `abilities/mod.rs`: `item_distance_falloff_uses_the_item_cast_range`, using a hand-written test item with `DistanceFromCaster { falloff: 0.5 }` and `cast_range` 600. A target at 300 takes 75% damage.
- `scripts/test_generator_common.py`: `test_item_distance_scaling_rejected`.

**Verify:** V. Running `--check` on `data/examples/invalid/item_effects.csv` must report the new error.

**Acceptance:**
- No hard-coded `0.0` radius or range for items remains in `effects.rs` or `abilities/mod.rs`.
- The generator test passes.

---

## Phase 6: one trigger-context builder

**Goal:** one constructor builds every `TriggerContext`, which removes the five hand-built copies. One behaviour fix comes with it: an item's `OnChannelEnd` now carries `cast_target`, as an ability's already does.

**Touches:**
- `src/abilities/effects.rs`: `impl TriggerContext`.
- `src/abilities/mechanics.rs`: `CastContext::trigger` (47-61) and `projectile_hit_triggers` (274-292).
- `src/abilities/casting.rs`: `action_channel_end_triggers` (103-136).
- `src/items.rs`: `item_use_trigger`, `item_channel_end_context` and `item_trigger` (1618-1682), and `item_attack_triggers` (1715).

**Steps:**
1. Add to `TriggerContext`:
   - `pub fn new(trigger, source: EffectSource, caster: Entity, team: Team, rank: u32, origin: Vec3, aim: Vec3) -> Self`. It sets `point = aim` and leaves everything else empty.
   - Chainable setters: `.at(point)`, `.cast_target(Option<Entity>)`, `.trigger_unit(Option<Entity>)`, `.affected(Vec<Entity>)`.
2. Rewrite the callers:
   - `CastContext::trigger` becomes `new(...).at(point).cast_target(self.unit_target)`.
   - `projectile_hit_triggers` becomes `new(...).at(hit.impact).cast_target(..).trigger_unit(hit.primary).affected(hit.splashed.clone())`.
   - `action_channel_end_triggers` becomes **one** path: `TriggerContext::new(OnChannelEnd, end.action.into(), end.caster, *team, end.level, transform.translation, end.aim).cast_target(end.target.unit())`.
   - Delete `item_channel_end_context`.
   - Item `OnUse` becomes `new(OnUse, Item(id), owner, team, 1, position, aim).at(position).cast_target(target.unit())`. **This keeps `point = owner position`.**
   - Item attack triggers become `new(trigger, Item(id), attacker, team, 1, owner_pos, point).trigger_unit(Some(target))`. That keeps `aim = point = target position`, as `item_trigger` does today.
3. Delete `item_trigger` once it has no callers.

**Tests:**
- The existing tests must pass unchanged, especially `item_use_trigger_keeps_the_owner_point_and_the_aim` (`items.rs:2311`) and `channeled_items_fire_on_channel_end_through_the_shared_pipeline` (`items.rs:3532`).
- New: `item_channel_end_carries_the_cast_target`, for a unit-targeted channelled test item.

**Verify:** V.

**Acceptance:** `grep -n "TriggerContext {" src` matches only the struct definition and tests.

---

## Phase 7: `OnChannelInterrupted` (decision 1)

**Goal:**
- A completed channel fires `OnChannelEnd`.
- A channel ended by a disable (`completed == false`) fires `OnChannelInterrupted`.
- This works for abilities and items, in the runtime and in both generators.

**Touches:**
- `src/abilities/definition.rs`: add a `AbilityTrigger::OnChannelInterrupted` variant.
- `src/abilities/casting.rs`: `action_channel_end_triggers`, which picks the trigger from `end.completed`.
- `src/items.rs`: `ItemId::has_active` (1046-1054) accepts either channel trigger; `describe_item_effect` gets the tag "Channel interrupted".
- `src/abilities/tooltip.rs`: `trigger_tag`, which is an exhaustive match, gets "channel interrupted".
- `src/items.rs` `ItemDefinition::aoe_radius()` from Phase 5 includes the new trigger.
- `scripts/generator_common.py`:
  - `ABILITY_TRIGGERS` and `ITEM_TRIGGERS` get `"on_channel_interrupted": "OnChannelInterrupted"`.
  - The trigger lists used in `effect_to_rust` stay as they are.
- `scripts/AbilityGenerator.py` `effect_context`: add the new trigger next to `on_channel_end`
  - in the passive-invalid list (232),
  - in the not-channelled invalid list (239-241),
  - and in the `trigger_unit`/`affected_units` invalid combos (248-250).
- `scripts/ItemGenerator.py` `effect_context`:
  - the not-channelled guard (322-323),
  - the trigger loop (324),
  - and, if present, the `has_use`/active check (245), which must count channel triggers.
- `data/examples/`: add one valid ability row and one valid item row using the trigger. Add an invalid row for the trigger on a non-channelled ability.

**Steps:** as listed above. Cancelling with a new order still emits no `ActionChannelEndEvent` and must stay that way.

**Tests (`abilities/mod.rs` and `items.rs`):**
- `completed_ability_channel_fires_only_on_channel_end`
- `stunned_ability_channel_fires_only_on_channel_interrupted`: extend `stuns_interrupt_a_channel` with an `OnChannelInterrupted` heal of 50 and the `OnChannelEnd` heal of 200. Assert health rises by 50, not 200 and not 250.
- `silenced_item_channel_does_not_interrupt_but_mute_does`: items use `MuteOrStun`. Mute interrupts and fires `OnChannelInterrupted`; silence does not.
- `order_cancelled_channel_fires_neither_trigger`
- `scripts/test_generator_common.py`: both trigger tables accept `on_channel_interrupted`.

**Verify:** V, plus `--check` on `data/examples/*.csv`, which passes, and on `data/examples/invalid/*.csv`, which reports the new error.

**Acceptance:**
- All the tests pass.
- README updated: the mechanics table (257), the interruption table (274), and the trigger lists in the "Adding abilities" and "Adding items" sections.

---

## Phase 8: unit target dies during the cast point (decision 2)

**Goal:**
- During the cast point the aim follows a living unit target.
- If the target dies or despawns before the action fires, the action still fires at the last known position, with costs paid.
- It is never cancelled for this reason.

**Touches:**
- `src/actions/pipeline.rs`: a new system `refresh_activation_aims`. Export it from `actions/mod.rs`.
- `src/abilities/mod.rs`: the chain, where the new system goes immediately before `tick_activations`.
- Test apps that list systems by hand: `items.rs::item_app` (2955), `abilities/mod.rs::test_app` (611), and the app builder in `navigation/tests.rs` (around 13-40).

**Steps:**
1. Add the system:
   ```rust
   pub fn refresh_activation_aims(mut activations: Query<&mut Activation>, units: Query<(&Transform, &Health)>)
   ```
   - There is no query conflict: `Activation` and `Transform` are different components.
   - For each activation with `!fired` and `target == CastTarget::Unit(e)`:
     - if `e` is alive, set `aim = transform.translation`;
     - otherwise set `target = CastTarget::Point(aim)` and keep `aim`.
2. Don't touch `tick_activations`'s cancellation rules, and don't re-check range when firing.
3. Leave the queued (walking) path in `resolve_queued_activations` as it is. It still drops unit-only casts whose target died before the caster reached range (`pipeline.rs:287-292`). Document that this is different from the cast-point behaviour.

**What happens downstream (already supported, now pinned by tests):**
- `ActionFiredEvent.target` is a point, so `execute_ability_casts` has no `unit_target`.
- `Projectile` flies to `aim` and fires `OnImpact`.
- `CastTarget` and `TriggerUnit` effects resolve to nothing.
- An item's `cast_target` is `None`.

**Tests (`abilities/mod.rs`):**
- `unit_target_death_during_cast_point_still_fires_at_last_position`: Bolt-like test ability with cast time 0.3. Kill the target mid-cast. Assert mana is paid, the cooldown has started, a projectile spawned toward the target's last position, and the `ActionFiredEvent` target is `Point`.
- `cast_point_aim_tracks_a_moving_unit_target`
- `items.rs`: `unit_item_fires_after_target_dies_during_cast_point`

**Verify:** V.

**Acceptance:** the tests pass, and the README interruption table's "Target dies" column describes both the cast-point and queued behaviours.

**Risk:** a caster turning during the cast point now follows a moving target. This is intended.

---

## Phase 9: unify item cooldown and charge state; tick it in the gated chain (decision 3)

**Goal:**
- An item instance stores one `ActionCooldown`, as an ability slot does.
- The charge or unit is spent at **commit**, which closes the bug where two same-frame uses of the last zero-cooldown consumable give one free use (`resolve_item_uses` writes `OnUse` before it checks the slot, `items.rs:1584-1597`).
- Item cooldowns tick in the same gated chain as ability cooldowns.
- The generator rejects item `charge_restore_time`.

**Touches:**
- `src/items.rs`:
  - `ItemInstance` (189-223): replace `cooldown_remaining: f32` and `charges: Option<u32>` with `cooldown: ActionCooldown`; add `spent_units: u32`.
  - `ItemInstance::new`: `charges = def.max_charges` only when `def.consumable`.
  - `ItemInstance::consume`
  - `item_action_spec` (837-841): `cooldown_state = item.cooldown`.
  - `commit_item` (849-853)
  - `resolve_item_uses` (1593-1613)
  - `tick_item_cooldowns` (1409-1418): make it `pub(crate)` and remove it from `ItemsPlugin`.
  - `try_sell_item` (1937-1956)
  - the tests that read `cooldown_remaining` or `charges` (`slot_set_charges` 3410 and others).
- `src/abilities/mod.rs`: add `crate::items::tick_item_cooldowns` to the first sub-chain, next to `tick_ability_cooldowns` (80).
- `src/ui.rs:1832` and `2195`: read `item.cooldown.remaining`.
- `scripts/generator_common.py`:
  - `parse_activation_spec` and `validate_activation` (444-445, 493-498): for `kind == "item"`, a non-empty `charge_restore_time` is an **error**, "item charges do not regenerate".
  - Remove the "not modeled yet" warning.
  - Keep the column in `ITEM_ACTIVATION_OPTIONAL` so it is still recognised and rejected rather than silently ignored.
- `data/examples/invalid/items.csv`: one row exercising the new error.

**Steps:**
1. `commit_item(inventory, slot, spec)`:
   - `item.cooldown.start(spec.cooldown)`. This also decrements charges, which now exist only on consumables.
   - If the item is consumable and `charges` is `None` or reached `Some(0)`:
     - `item.stack -= 1`,
     - `item.spent_units += 1`,
     - `item.cooldown.charges = def.max_charges`.
2. `resolve_item_uses`: after writing `OnUse`, drain `spent_units`. Call `remove_passives(item.passives(), spent, ...)` and clear the slot if `stack == 0`. Keep writing `OnUse` exactly once per `ActionFiredEvent`.
   - Because `item_action_spec` already filters `stack > 0` (`items.rs:814`), a second request in the same frame on an emptied slot is rejected.
3. `try_sell_item`: remove passives for `item.stack + item.spent_units`. This covers a sale between commit and resolve, which can happen because the shop systems are unordered.
4. `ItemDefinition.cooldown` and `ItemActivation` stay as they are. No CSV change apart from the rejection.

**Tests (`items.rs`):**
- `last_consumable_unit_cannot_fire_twice_in_one_frame`: test consumable with cooldown 0 and stack 1; two requests in one frame; exactly one `OnUse` trigger.
- `consumable_charges_spend_at_commit_and_refill_from_the_next_unit`: adapt the existing `consumables_spend_charges_then_units` (2609) and `charges_are_per_copy_and_rejected_uses_keep_them` (3359).
- `non_consumable_items_never_lose_charges`
- `item_cooldowns_tick_in_the_abilities_chain`: `test_app` with `tick_item_cooldowns` listed.
- `scripts/test_generator_common.py`: `test_item_charge_restore_time_rejected`.

**Verify:** V. `--check` on `data/examples/invalid/items.csv` reports the new error, and the production CSV still passes (no item sets the column today).

**Acceptance:**
- No `cooldown_remaining` field remains.
- The README Limitations line about item charges says they never regenerate and the column is rejected.

**Risks:**
- Test apps must list `tick_item_cooldowns` wherever they relied on `ItemsPlugin` ticking it.
- `ShopUndoLog` stores `ItemInstance` values. Check that undo of a sale restores `cooldown` and `spent_units == 0`.

---

## Phase 10: deduplicate passive stats

**Goal:** one place maps `PassiveStat` to a bonus field.

**Touches:** `src/items.rs` (`ItemPassives` at 1071-1089, `ItemId::passives` at 1093-1122) and `src/abilities/passive.rs` (`accumulate_passives` at 38-56, `passive_stat_entry` at 58-82, `passives_at_rank`, `loadout_passives`).

**Steps:**
1. In `items.rs`, add:
   - `impl ItemPassives { pub fn add_stat(&mut self, stat: PassiveStat, amount: f32) }`, holding the single exhaustive `match`;
   - `impl std::ops::AddAssign for ItemPassives`.
2. `ItemId::passives` calls `add_stat`. In `passive.rs`, delete `accumulate_passives` and `passive_stat_entry` and use `add_stat` and `+=`.
3. Don't rename `ItemPassives`. Optionally add `pub type StatBonuses = ItemPassives;` with a doc comment, but don't migrate callers.

**Tests (`items.rs`):**
- `every_passive_stat_maps_to_a_distinct_field`: list every `PassiveStat` variant explicitly, call `add_stat(v, 1.0)` on a fresh value, and assert each changes exactly one field and no two variants share a field.
- The existing `stone_skin_passive_scales_with_rank` and `attribute_passives_apply_and_revert` must pass.

**Verify:** V.

**Acceptance:** `grep -n "PassiveStat::MaxHealth =>" src` matches once outside tests and tooltips.

---

## Phase 11: make Overcharge data-driven

**Goal:** remove the hard-coded Overcharge id and constants (`passive.rs:34-36`, `115-124`, `186`) and the `ToggleStatBonuses` component.
- Toggle upkeep (mana per second) becomes definition data.
- Stat bonuses while toggled are ordinary statuses applied through `StatusEffectEvent`.

**Touches:**
- `src/abilities/definition.rs`:
  - `AbilityTrigger::WhileToggled`
  - `AbilityDefinition.toggle_mana_per_sec: RankValue` (default `ZERO`) and a builder `.toggle_upkeep(RankValue)`
- `src/abilities/passive.rs`:
  - replace `tick_toggle_upkeep`
  - delete `ToggleStatBonuses`, `desired_toggle_bonuses`, `apply_toggle_delta` and the `OVERCHARGE_*` constants
- `src/units.rs:78`: remove `ToggleStatBonuses::default()`.
- `src/abilities/tooltip.rs` `trigger_tag`: "while toggled". `src/items.rs` `describe_item_effect` or the Phase 12 describer: items never use this trigger.
- `scripts/generator_common.py`:
  - `ABILITY_TRIGGERS` gets `"while_toggled": "WhileToggled"`.
  - In the `stat_modifier` branch (871-898), when `e.trigger == "while_toggled"`, a duration is an **error** ("lasts while toggled") and the emitted duration is `RankValue::ZERO`.
  - Only `effect_target` `caster` is allowed.
- `scripts/AbilityGenerator.py`:
  - a new optional column `toggle_mana_per_sec` (level list allowed), emitted as `.toggle_upkeep(...)`;
  - error if it is set on a non-toggle ability;
  - `effect_context` makes `while_toggled` invalid unless `ability_type == toggle`, and allows only `stat_modifier` effects.
- `data/abilities.csv`: add the `toggle_mana_per_sec` column, with `8` for overcharge and blank elsewhere.
- `data/ability_effects.csv`: two rows:
  - `overcharge,0,while_toggled,caster,stat_modifier,attack_speed_mult,0.2,...`
  - `overcharge,1,while_toggled,caster,stat_modifier,move_speed,10,...`
- Regenerate `generated.rs`.

**Steps (the new `tick_toggle_upkeep`, at the same chain position `abilities/mod.rs:82`):**
1. For each slot whose definition has `behavior == Toggle`:
   - If `toggled` and `rank > 0`: drain `def.toggle_mana_per_sec.at(rank) * dt`. At 0 mana, clamp to 0 and set `toggled = false`, with no cooldown, as today.
2. Reconcile against the `WhileToggled` `ApplyStatus` entries:
   - If toggled and the entry's status id is absent from the caster's `StatusEffects`, write `StatusEffectEvent::Apply(status.build(rank, f32::INFINITY, ctx))`. Before writing, set `undispellable = true` and `dispel_type = DispelType::None`.
   - If not toggled and the id is present, write `StatusChange::Remove(id)`.
3. `apply_status_effect_events` runs later in the same chain, so changes land in the same frame.

**Tests (`abilities/passive.rs` with a small app, or `abilities/mod.rs`):**
- `overcharge_on_adds_attack_speed_and_move_speed_and_drains_mana`. The numbers must match the old constants: +0.2 attack-speed multiplier, +10 move speed, 8 mana per second.
- `overcharge_turns_off_at_zero_mana_and_reverts_stats`
- `overcharge_bonus_survives_strong_dispel`
- `toggle_off_cast_removes_the_bonus`
- `scripts/test_generator_common.py`: reject `while_toggled` on a non-toggle ability, and reject a duration on a `while_toggled` row.

**Verify:** V. The regeneration diff should show only `overcharge` changing in `generated.rs`. The generator warning "overcharge has no effects" disappears.

**Acceptance:**
- No `AbilityId::Overcharge` reference remains outside generated code, `components.rs` and tests.
- README:214 and 241 and the Limitations line ("Overcharge uses hardcoded toggle upkeep") are updated.

**Risk:** ranking up while toggled keeps the old values until the toggle is cycled. Document it; it is acceptable for test content.

---

## Phase 12: one shared tooltip describer

**Goal:** abilities and items describe effects with the same vocabulary. This replaces the parallel `items.rs:1146-1292` and `tooltip.rs:181-408`.

**Touches:** `src/abilities/tooltip.rs` and `src/items.rs` (`tooltip_body` at 968-1029, plus `describe_item_effect`, `target_suffix`, `scaling_label`, `damage_label` and `fmt_rank`, which are all deleted).

**Steps:**
1. In `tooltip.rs`, expose `pub(crate) struct EffectRow { label: String, value: Option<RankValue> }` and `pub(crate) fn describe_effect(entry: &AbilityEffectEntry) -> (Vec<EffectRow>, Vec<String> /* notes */)`.
   - Labels carry **no** trigger tag; for example "Physical damage", "Heal", "Slow %".
   - Build it from the existing `effect_rows`, `status_rows` and `effect_notes` logic. `scaling_note` and an area note replace `target_suffix`.
   - `trigger_tag` becomes `pub(crate)`.
2. The ability renderer (`ability_tooltip_bits`) produces `"{label} ({tag})"` with the per-rank series, so ability output is **unchanged**.
3. The item renderer in `tooltip_body` produces `"{Tag}: {label} {value.at(1)}"`, with the tag capitalised, plus the notes. For example "Use: Heal 180".
4. `passive_stat_lines` uses `passive_name` from `tooltip.rs` where its labels match. Keep "+100 Health"; check that `PassiveStat::MaxHealth` maps to "Health".

**Tests:**
- The existing `tooltip.rs` tests and `items.rs::tooltips_list_price_stats_and_effects` must pass. Update only assertions on wording that intentionally changed, such as Storm Rod's damage line.
- New: `item_and_ability_describe_the_same_effect_with_the_same_label`. Take the same `Damage` entry under `OnCast` and under `OnUse`, and check the label text is identical.

**Verify:** V.

**Acceptance:** one effect describer remains, and `grep -n "fn describe_item_effect\|fn scaling_label" src` finds nothing.

---

## Phase 13: final docs pass

**Goal:** the README and the earlier plans match the code.

**Touches:** `README.md`:
- "Ability architecture" (216-241): mention `status.rs`, `WhileToggled`, `OnChannelInterrupted`, and the trigger-context builder.
- The mechanics and interruption tables (242-275).
- The two "Adding …" sections (trigger lists, `toggle_mana_per_sec`, the item `charge_restore_time` rejection, the item `distance` rejection).
- Limitations (371-373).
- Layout (395-430).

Also add a dated "Superseded by 2026-11-07-item-ability-architecture-cleanup.md" note to the gap tables of:
- `docs/plans/2026-11-06-shared-effects-pipeline.md`
- `docs/plans/2026-11-06-generator-shared-architecture.md`
- `docs/plans/2026-11-06-activation-targeting-mechanics.md`

**Verify:** V. Check that every trigger in `ABILITY_TRIGGERS` and `ITEM_TRIGGERS` appears in the README.

**Acceptance:** the reviewer finds no doc statement contradicted by the code.

---

## Risks across phases

| Risk | Mitigation |
|---|---|
| Test apps list systems by hand (`items.rs::item_app`, `abilities/mod.rs::test_app`, `navigation/tests.rs`, `teleport.rs:533` with `ItemsPlugin`), so new or moved systems are missed | Each phase that adds or moves a system updates every test app in the same commit. Grep for the system name. |
| Generated headers drift (Phase 2) | Change the generator templates, regenerate, and run the determinism step of V. `rustfmt` must be on `PATH`. |
| Registering `StatusEffectEvent` in two plugins | Harmless; Bevy ignores duplicate message registration. Leave the `AbilitiesPlugin` registration in place. |
| Shield consumption order differs from what designers expect | Oldest first (vector order). Documented in `status.rs`. |
| Item tooltip wording changes (Phase 12) | Ability wording is unchanged. Update only the item test assertions. |
| A sale between commit and resolve (Phase 9) | Handled in `try_sell_item` (`stack + spent_units`). |
| Overcharge rank-up while on (Phase 11) | Documented limitation. |
| Moving `tick_item_cooldowns` behind the authority gate (Phase 9) | Clients can't use items today, so behaviour is unchanged. No other gating changes. |

## Out of scope

Network messages and snapshots, shop buyer entities and authority gating, adding generated abilities to loadouts, inverting the `actions` dependencies (an `ActionSources` provider trait), item projectiles, `OnChannelTick` emission, renaming `Ability*` types, and changes to damage or mitigation formulas.

## Commit sequence

One commit per phase, with Phase 2 split into 2a and 2b. All on the same workstream branch and PR, as `CLAUDE.md` requires.
