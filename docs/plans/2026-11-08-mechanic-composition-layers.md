# Mechanic composition layers: audit, classification and refactor plan

**Date:** 2026-11-08
**Status:** Approved by the coordinator with the recommended defaults for every decision in §7 (D1 b, D2 keep spell, D3 yes, D4 defer, D5 include Step 6, D6 defer, D7 start unit not re-fired, D8 custom `then` trigger).
**Based on:** `2026-11-06-activation-targeting-mechanics.md`, `2026-11-06-shared-effects-pipeline.md`, `2026-11-06-damage-mechanics.md`, `2026-11-06-status-effects-catalog.md`, `2026-11-06-generator-shared-architecture.md`, `2026-11-07-item-ability-architecture-cleanup.md` (its invariants and the **V** verification commands), `2026-11-07-movement-mechanics.md`, `2026-11-08-healing-resource-mechanics.md`, `2026-11-08-attack-modifiers.md`.
**Merged with #58:** reactive and conditional triggers (`2026-11-08-reactive-triggers.md`) were merged into these layers; see `2026-11-08-merge-reactive-triggers-into-composition-layers.md`. It adds the `Accumulate` and reaction-aware `Chain` gates, invariants I10 to I12, and amends I1, I4 and I6. The permanent summary is `docs/architecture/composition.md`.
**Scope:** one PR on one branch, one commit per step. Every step compiles and passes **V** on its own.

Line references were taken at commit `6d82702` and may drift as steps land.

---

## Executive summary

The six-layer model (Activation, Mechanics, Triggers, Effects, Modifiers, Gameplay rules) mostly matches the code already:

- **Activation** is `src/actions/`.
- **Triggers** are `AbilityTrigger` plus the emitter systems.
- **Effects** are `AbilityEffect` plus the single executor `EffectRunner::execute`.
- **Gameplay rules** are `combat.rs`, `status.rs`, `vitals.rs`, `displacement.rs` and `teleport.rs`.
- **Modifiers** are `PassiveStat`/`ItemPassives` plus `StatusEffect` stat fields.

Items and abilities already share one trigger bus and one effect executor.

There are five real layering violations, which this plan fixes:

1. **Chain Lightning bypasses the mechanic, trigger and effect layers.**
   - The only chain in the game is Bolt's `AbilityEffect::Custom { id: "bolt_chain" }` (`abilities/catalog.rs:238-247`), backed by `custom.rs:17-40` and `combat::emit_chain_damage` (`combat.rs:477-524`).
   - It sits in the damage-rules module, picks its own targets and writes `DamageEvent::spell` directly.
   - It hard-codes the magical damage type, the 450 jump radius and the 0.75 falloff.
   - It fires no trigger, rejects items (`custom.rs:25`) and ignores `is_untargetable`.
   - It has no tests.
2. **The Mechanic layer only works for abilities.**
   - `CastContext.ability: AbilityId` (`mechanics.rs:34`) and `ProjectilePayload.ability: AbilityId` (`combat.rs:603`).
   - `projectile_hit_triggers` hard-codes `EffectSource::Ability` (`mechanics.rs:278`).
   - `ItemDefinition` has no `mechanics` field (`items.rs:390-412`).
   - ItemGenerator validates `projectile_type` and then drops it with a warning (`generator_common.py:557-565`).
3. **A second status executor.**
   - `tick_toggle_upkeep` (`abilities/passive.rs:142-164`) builds `StatusEffect`s itself, duplicating `EffectRunner`'s `ApplyStatus` arm (`effects.rs:505-541`).
   - It uses a different build context, never sets `dot_source` and never applies heal amp.
4. **Gameplay rules scattered across effect code and appliers.**
   - (a) The damage-kind choice is written twice: `effects.rs:422-426` and `vitals.rs:224-228`.
   - (b) Sleep-wake on damage edits statuses inline in `combat::apply_damage_events` (`combat.rs:383-396`).
   - (c) The debuff-immunity purge is duplicated: `status.rs:731-752` and `820-835`.
   - (d) Hostile-effect immunity is inconsistent between displacement (`displacement.rs:237-242`) and teleport, which has no check (`teleport.rs:110-202`).
5. **Declared-but-never-emitted triggers pass the generator silently for abilities.**
   - `OnChannelTick`, `OnExpire`, `OnDamageTaken`, `OnKill` and `OnDeath` have no emitter.
   - Only ItemGenerator warns about them.

**Deliberately not done:**

- No unified modifier stack.
- No `src/effects/` move and no `Ability*` renames.
- No new death, kill or damage-taken emitters.
- No moves of the resource-regen modules.

**Main design choice.** Chain becomes a reusable **mechanic gate**, `AbilityEffect::Chain`, following the `Proc`/`Counter` pattern. The gate decides **who** (the hop units) and re-fires the context as a derived trigger (for example `Custom("on_chain_hit")`) in the same FIFO queue. Damage rows tagged with that trigger do the **what**. An ability and an item then share the same chain mechanic and the same damage effect.

---

## 1. Classification

**Flag** marks a violation and points to the step that fixes it. "Accepted" marks a deliberate choice that is documented rather than refactored.

### 1.1 Activation: how an action starts

| Item | Location | Flag |
|---|---|---|
| `ActionSource`, `ActionId`, `ActivationRequest`, `ActionSpec`, `ActionError`, `Activation`, `QueuedActivation`, `ActionFiredEvent`, `ActionChannelEndEvent` | `actions/mod.rs` | – |
| `ActionSources::spec` / `base_spec` | `actions/pipeline.rs:25-115` | – |
| `fire`, `validate_activation_requests`, `resolve_queued_activations`, `refresh_activation_aims`, `tick_activations` | `actions/pipeline.rs` | – |
| `TargetType`, `TargetTeam`, `team_allows`, `target_allowed`, `resolve_target`, `in_range` | `actions/targeting.rs` | – |
| `ActionCost`, `ActionCooldown`, `DisableRule`, `check_ready` | `actions/resources.rs` | – |
| `ability_action_spec`, `commit_ability` | `abilities/casting.rs` | Source-specific (correct) |
| `item_action_spec`, `commit_item`, `tick_item_cooldowns` | `items.rs` | Source-specific (correct) |
| `AbilityBehavior` | `definition.rs:81` | – |

### 1.2 Mechanics: how an action plays out over time

| Item | Location | Flag |
|---|---|---|
| `AbilityMechanic` (Dash, Projectile, AttackUnitTarget, RingFx) | `definition.rs:689-715` | **Ability-only** → Step 6 |
| `CastContext` | `mechanics.rs:31-59` | **`ability: AbilityId`** → Step 5 |
| `MechanicRunner::run`, `dash`, `execute_ability_casts` | `mechanics.rs:95-255` | – |
| `CustomAbilityBehaviors` | `mechanics.rs:63-91` | Escape hatch |
| Projectile systems (`spawn_requested_projectiles`, `fly_projectiles`, `skillshot_contacts`, `apply_projectile_hits`) | `combat.rs` | Report `ProjectileHitEvent` only (correct) |
| `ProjectilePayload` | `combat.rs:600-607` | **`ability: AbilityId`** → Step 5 |
| `emit_chain_damage` | `combat.rs:477-524` | **A mechanic in the rules module that applies damage** → Step 2 |
| `bolt_chain` | `abilities/custom.rs:17-40` | **A mechanic disguised as an effect** → Step 2 |
| `ForcedMovement`, `tick_forced_movement` and related | `displacement.rs` | – |
| Basic-attack lifecycle | `basic_attack.rs` | – |
| `resolve_basic_attack_impacts` | `basic_attack.rs:551-636` | Accepted: it is the authoritative source of attack damage |
| `spawn_item_use_fx` | `items.rs:1183-1212` | Presentation; accepted (D6) |

### 1.3 Triggers: when effects run

| Item | Location | Flag |
|---|---|---|
| `AbilityTrigger` | `definition.rs:111-156` | **Five variants have no emitter** → Step 1 |
| `TriggerContext`, `AbilityTriggerEvent`, `AttackMeta`, `EffectSource` | `effects.rs` | – |
| `OnCast` emission | `mechanics.rs:248-250` | – |
| `projectile_hit_triggers` | `mechanics.rs:260-291` | **Hard-codes `EffectSource::Ability`** → Step 5 |
| `OnUse` (`resolve_item_uses`) | `items.rs:1099-1179` | – |
| `action_channel_end_triggers` | `casting.rs:103-129` | – |
| `attack_triggers` | `abilities/attack_triggers.rs` | Reference pattern |
| `OnUnitContact` / `OnMovementEnd` | `displacement.rs` | – |
| `WhileToggled` reconciliation | `abilities/passive.rs:101-167` | **Duplicates status building** → Step 3 |
| `Proc`, `Counter` gates, `MAX_TRIGGER_DEPTH`, derived FIFO queue | `effects.rs` | Accepted; invariant I6 |
| Generator `NOT_EMITTED` | `ItemGenerator.py:118-121` | **Item-only** → Step 1 |

### 1.4 Effects: what happens

| Item | Location | Flag |
|---|---|---|
| `AbilityEffect` | `definition.rs:575-674` | – |
| `EffectRunner::targets` / `valid_target` / `scaled_amount` | `effects.rs:266-384` | – |
| `EffectRunner::execute` (the only executor) | `effects.rs:403-661` | **The Damage arm chooses the damage kind** → Step 4a; **ApplyStatus applies heal amp** → Step 3 |
| `CustomAbilityEffects` | `effects.rs:188-227` | Escape hatch |
| Event contracts (`DisplacementEvent`, `TeleportEvent`, `HealEvent`, `ManaRestoreEvent`, `ResourceDrainEvent`, `StatusEffectEvent`, `ExtraAttackEvent`) | various | – |

### 1.5 Modifiers: persistent numerical changes

| Item | Location | Flag |
|---|---|---|
| `PassiveStat`, `AbilityEffect::PassiveStat` | `definition.rs` | Accepted |
| `ItemPassives` and related | `items.rs:735-1024` | Shared by items and passive abilities |
| `passives_at_rank`, `sync_ability_passives` | `abilities/passive.rs:14-90` | – |
| `StatusSpec` modifier variants | `definition.rs:259-321` | Timed modifiers as status templates |
| `StatusEffect` stat fields + deltas | `status.rs` | Accepted |
| `CombatStats`, `vitals::resource_stat`, `outgoing_heal_amp` | `components.rs`, `vitals.rs` | – |

Modifiers are stored three ways:

- **Mutate on apply:** `CombatStats`.
- **Sum on read:** `resource_stat`.
- **Snapshot at build:** `Slow`.

These are documented, not unified.

### 1.6 Gameplay rules: how effects interact with the game

| Item | Location | Flag |
|---|---|---|
| `DamageKind`, `DamageEvent` constructors, mitigation, `apply_damage_events` | `combat.rs` | **Inline sleep-wake** → Step 4b |
| Miss/evasion | `basic_attack.rs:513-546` | – |
| `StatusEffects` queries, dispel, `apply_status_effect_events`, `tick_status_effects` | `status.rs` | **Duplicated purge** → Step 4b |
| `heal_factor`, `apply_heal`, `apply_resource_drain_events` | `vitals.rs` | **Duplicated damage-kind choice** → Step 4a; hostility test is `source != target` (D4) |
| `apply_heal_events`, `apply_mana_restore_events` | `effects.rs:733-761` | Cosmetic misplacement; deferred |
| `apply_displacement_events` | `displacement.rs:220-286` | → Step 4c |
| `apply_teleport_events` | `teleport.rs:110-202` | **No immunity check** → Step 4c |
| `DisableRule::blocks` | `actions/resources.rs:80-89` | – |

---

## 2. Refactor plan

**V** (shared verification):

```bash
cargo test
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py --check
PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py --check
(cd scripts && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest test_generator_common)
python3 scripts/AbilityGenerator.py && python3 scripts/ItemGenerator.py && git diff --exit-code -- src/abilities/generated.rs src/item_catalog.rs
```

Also run `--check` on `data/examples/*.csv`, which must pass, and on `data/examples/invalid/*.csv`, which must report the expected errors.

### Step 0: baseline and pinning tests (no production change)

These tests must pass before any change:

- `bolt_chain_hits_the_two_nearest_enemies_with_compounding_falloff`
  - Rank 1 base is 57.
  - Hops deal 57 × 0.75 and 57 × 0.75².
  - The primary target takes projectile damage only.
- `bolt_chain_stops_when_no_enemy_is_within_the_jump_radius`
- `bolt_chain_never_revisits_a_unit`
- `while_toggled_status_has_expected_fields`, which pins the Overcharge status.

### Step 1: one "not emitted" trigger warning for both generators

1. Move `NOT_EMITTED` into `generator_common.py` as `NOT_EMITTED_TRIGGERS`. It covers `on_channel_tick`, `on_expire`, `on_damage_taken`, `on_kill` and `on_death`.
2. Add `warn_unemitted_triggers(effects, report)` and call it from both generators.

Tests:

- `test_unemitted_ability_trigger_warns`
- `test_unemitted_item_trigger_still_warns`
- `test_emitted_triggers_do_not_warn`

### Step 2: the Chain mechanic gate

```rust
/// Mechanic gate: from each resolved start unit, jump up to `jumps` times to the nearest
/// valid unit (team-allowed, alive, targetable, not yet hit) within `radius` of the
/// previous unit, re-firing this context under `then` per hop
/// (trigger unit = hop, point = its position, magnitude *= falloff^hop).
/// The start unit is not re-fired. Emits no gameplay messages.
Chain { jumps: u32, radius: RankValue, falloff: f32, team: TargetTeam, then: AbilityTrigger },
```

- Add `TriggerContext.magnitude: f32`. It defaults to 1.0 and is kept by `derived()`. It multiplies the Damage, DrainHealth and Heal amounts.
- Add a pure `chain_hops(...)` in `mechanics.rs`. It picks the nearest unit first and breaks ties by `Entity`.
- Re-author Bolt as a `Chain` row plus an `on_chain_hit` magic-damage row.
- Delete `emit_chain_damage` and `bolt_chain`, and fix the `custom.rs` module doc.
- Add a tooltip line for `Chain`.
- Add a generator effect type `chain`:

  | Column | Meaning |
  |---|---|
  | `effect_id` | the custom `on_<name>` trigger |
  | `value` | number of jumps (≥ 1) |
  | `radius` | jump radius (> 0) |
  | `amount` | percent kept per jump, in (0, 100] |

  The start target must be a unit selector. Add `chain` to the `fed` set in `warn_unreachable_custom_triggers`.
- Add examples: an ability `chain_lightning`, an item `storm_charm` that uses the same rows under `on_use`, and one invalid row.
- Intended behaviour changes:
  - Chains skip untargetable units.
  - Ties between equally near units are broken deterministically.
  - D1: a chain hop clears `AttackMeta`, so chained damage is spell damage.

Tests:

- `bolt_chain_skips_cycloned_units`
- `chain_hops_break_ties_by_entity`
- `item_and_ability_chain_emit_identical_damage`
- `chain_respects_max_trigger_depth`
- `chain_entry_emits_no_gameplay_messages`
- `magnitude_defaults_to_one_for_non_chain_triggers`
- Generator tests for the columns, the unit start target, and the fed trigger.

### Step 3: one status builder

1. Add `pub(crate) fn build_status_effect(spec, rank, duration, caster, point, target_stats, target_attrs, healer_amp) -> StatusEffect`, extracted from the `ApplyStatus` arm.
2. `tick_toggle_upkeep` calls it with the entity's own position and an infinite duration, then keeps its undispellable flags.

Test: `while_toggled_and_on_cast_build_the_same_status`.

### Step 4: gather scattered rules into their owners

- **4a.** Add `DamageEvent::from_effect(source, target, amount, damage_type, from_attack)` and use it in `effects.rs` and `vitals.rs`. DrainHealth stays spell damage (D2).
- **4b.** Add `status::wake_from_sleep` and call it from `combat::apply_damage_events`. Add a shared `purge_strong_dispellable_debuffs` used by both purge paths. Add the test `sleep_wake_releases_stat_deltas`.
- **4c.** Add `status::hostile(...)` (extracted from displacement) and `StatusEffects::blocks_hostile_relocation()`. Teleport rejects a hostile teleport of a debuff-immune unit with `TeleportError::Immune` (D3). Tests:
  - `hostile_teleport_of_debuff_immune_unit_is_rejected`
  - `self_and_ally_teleport_ignore_debuff_immunity`

### Step 5: a source-neutral projectile mechanic

1. Change `ProjectilePayload.ability` to `source: EffectSource`.
2. Change `CastContext.ability` to `source: EffectSource`, with a `fn ability() -> Option<AbilityId>` accessor.
3. `projectile_hit_triggers` uses the payload source.

Test: `projectile_hit_trigger_carries_the_payload_source`.

### Step 6: items carry mechanics (D5: included)

1. Add `ItemDefinition.mechanics: &'static [AbilityMechanic]`.
2. ItemGenerator emits a `Projectile` mechanic from `projectile_type`. Add `on_projectile_hit` / `on_impact` to `ITEM_TRIGGERS`, valid only with a projectile. `aoe_radius()` also scans those triggers.
3. Add an `execute_item_mechanics` system that runs after `resolve_item_uses`, and add it to every hand-built test app.
4. Regenerate `item_catalog.rs`. The only change on each item is an added `mechanics: &[]` line.

Tests:

- `item_projectile_fires_on_projectile_hit_for_the_item`
- `item_projectile_splash_uses_the_item_radius`
- Generator tests for emitting the mechanic and for the trigger validation.

### Step 7: documentation

See §4.

---

## 3. Invariants

1. **I1:** `EffectRunner::execute` is the only place an `AbilityEffect` becomes gameplay messages. Step 3 includes `WhileToggled` in this.
2. **I2:** Mechanics and mechanic gates decide **who and when**. They never write Damage, Heal or Status events.
   - Accepted exceptions: the basic-attack hit stage, DoT ticks, reflection and burn follow-ups.
3. **I3:** Every trigger emitter carries an `EffectSource`. No mechanic hard-codes `EffectSource::Ability`.
4. **I4:** Health changes only in `combat::apply_damage_events`, `vitals::apply_heal` and `vitals::apply_resource_drain_events`. Statuses change only through `StatusEffectEvent`, `tick_status_effects` and `status::wake_from_sleep`.
5. **I5:** Immunity, untargetability, dispel and damage calculation are queried through the `status.rs` / `combat.rs` helpers.
6. **I6:** Gates (`Proc`, `Counter`, `Chain`) only push derived contexts. They are capped by `MAX_TRIGGER_DEPTH` and resolve FIFO in the same call.
7. **I7:** Source-specific rules stay in their own domain.
8. **I8:** Generated Rust changes only by re-running the generators.
9. **I9:** The invariants of the cleanup plan still hold.

## 4. Documentation plan

Add a new permanent doc, `docs/architecture/composition.md`, with these sections:

- the six layers, each with its owning modules and what it may emit or read;
- the interfaces between layers, as a diagram;
- frame order;
- the three modifier storage rules;
- a gameplay-rule matrix;
- worked compositions:
  - Chain Lightning as an ability and as an item
  - Bolt
  - Arcane Lance
  - Seismic Slam / Cataclysm
  - Blink vs Blink Dagger
  - Stone Skin vs Iron Bracer
  - Overcharge
  - Vial of Light / Storm Rod / Spark Pendant
  - frost_hammer
- a checklist for adding a mechanic.

Other documentation updates:

- `README.md`: link the doc and document the `chain` effect type, the unemitted-trigger warning and item `projectile_type`.
- Add "superseded in part" notes to `2026-11-06-shared-effects-pipeline.md` and `2026-11-06-damage-mechanics.md`.

## 5. Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | Hops are picked before this frame's damage applies | Same as today; documented |
| R2 | Duplicate triggers from gates | Derived contexts run only the entries tagged `then`, under the depth cap |
| R3 | `combat` ↔ `abilities::effects` module cycle | Allowed within one crate; the fallback is to move `EffectSource` to `components.rs` |
| R4 | A new `TriggerContext` field breaks struct literals | `TriggerContext::new` is the only constructor |
| R5 | `AbilityEffect` matches must be exhaustive | Update the tooltip in the same commit |
| R7 | The Step 6 regeneration touches every item | Mechanical diff |
| R8 | `AttackUnitTarget` inserts `AttackTarget` directly | Out of scope; recorded |
| R9 | Hand-built test apps | Add the new system to each one |

## 6. Decisions (resolved)

| # | Decision |
|---|---|
| D1 | Chain hops clear `attack`, so chained damage is spell damage |
| D2 | DrainHealth stays spell damage |
| D3 | Debuff immunity rejects hostile teleports |
| D4 | Drain hostility rule deferred |
| D5 | Step 6 included |
| D6 | Item FX via `RingFx` deferred |
| D7 | The start unit is not re-fired |
| D8 | Custom `then` trigger named in `effect_id` |
