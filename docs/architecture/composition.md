# Mechanic composition: the six layers

Every ability and item is built from the same six layers. An ability and an item that use the same mechanic and the same effect rows behave identically; only the activation source differs.

Plan and decisions: `docs/plans/2026-11-08-mechanic-composition-layers.md`. Earlier plans (`2026-11-06-shared-effects-pipeline.md`, `2026-11-06-damage-mechanics.md`) are superseded in part by this document.

## 1. The layers

| Layer | Purpose | Owning code | May emit | May read |
|---|---|---|---|---|
| **Activation** | How an action starts: targeting, range, cost, cooldown, cast point, channel | `src/actions/` (`pipeline.rs`, `targeting.rs`, `resources.rs`); source-specific halves in `abilities/casting.rs` (`ability_action_spec`, `commit_ability`) and `items.rs` (`item_action_spec`, `commit_item`) | `ActionFiredEvent`, `ActionChannelEndEvent` | Loadout / inventory, mana, cooldowns, statuses (`DisableRule`) |
| **Mechanics** | How an action plays out over time and **who** is involved: dash, projectile, attack-unit, gates (`Proc`, `Counter`, `Chain`) | `abilities/mechanics.rs` (`MechanicRunner`, `CastContext`, `chain_hops`, `execute_ability_casts`, `execute_item_mechanics`), `combat.rs` projectile systems, `displacement.rs` motion | `AbilityTriggerEvent`, `SpawnProjectileEvent`, `DisplacementEvent` (dash). Never Damage, Heal or Status events | Positions, teams, health (alive), untargetable |
| **Triggers** | **When** effects run: `AbilityTrigger` plus the systems that emit it | `definition.rs` (`AbilityTrigger`), `mechanics.rs` (`OnCast`, `projectile_hit_triggers`), `items.rs` (`OnUse`), `casting.rs` (channel end), `attack_triggers.rs`, `displacement.rs` (contact, movement end), `passive.rs` (`WhileToggled`) | `AbilityTriggerEvent(TriggerContext)` | The fired action |
| **Effects** | **What** happens: `AbilityEffect` rows executed by the single executor | `abilities/effects.rs` (`EffectRunner::execute`, `resolve_ability_triggers`, `build_status_effect`) | `DamageEvent`, `HealEvent`, `ManaRestoreEvent`, `ResourceDrainEvent`, `StatusEffectEvent`, `DisplacementEvent`, `TeleportEvent`, `ExtraAttackEvent`, derived contexts from gates | Context, target stats and statuses (for scaling) |
| **Modifiers** | Persistent numeric changes | `PassiveStat` / `ItemPassives` (`items.rs`, `abilities/passive.rs`), `StatusSpec` modifier variants and `StatusEffect` stat fields (`status.rs`), `vitals::resource_stat` | Stat changes through `CombatStats` | Owned items, ranks, active statuses |
| **Gameplay rules** | How effects interact with the game: mitigation, immunity, untargetability, dispel, healing factors | `combat.rs` (`apply_damage_events`), `status.rs` (`apply_status`, `wake_from_sleep`, `hostile`, `blocks_hostile_relocation`), `vitals.rs`, `displacement.rs`, `teleport.rs` | Health / status / position changes (the only writers) | Statuses, stats, teams |

Rules of thumb (invariants I1 to I9 in the plan):

- `EffectRunner::execute` is the only place an `AbilityEffect` becomes gameplay messages (I1). Toggle upkeep builds its statuses with the same `build_status_effect`.
- Mechanics and gates decide who and when; they never write Damage, Heal or Status events (I2).
- Every trigger emitter carries an `EffectSource` (`Ability` or `Item`); nothing hard-codes `Ability` (I3).
- Health changes only in `combat::apply_damage_events`, `vitals::apply_heal` and `vitals::apply_resource_drain_events`; statuses change only through `StatusEffectEvent`, `tick_status_effects` and `status::wake_from_sleep` (I4).
- Gates (`Proc`, `Counter`, `Chain`) only push derived contexts, capped by `MAX_TRIGGER_DEPTH` (2) and resolved FIFO in the same call (I6).
- Generated Rust (`generated.rs`, `item_catalog.rs`) changes only by re-running the generators (I8).

## 2. Interfaces between layers

```text
 input / AI / shop
      |  ActivationRequest { caster, source: Ability(slot) | Item(slot), target }
      v
 +-----------------+   ActionFiredEvent        +----------------------------+
 |   Activation    | ------------------------> |  Mechanics                 |
 | actions/        |   ActionChannelEndEvent   |  dash, projectile, attack, |
 +-----------------+                           |  Chain / Proc / Counter    |
                                               +----------------------------+
                                                      | AbilityTriggerEvent(TriggerContext)
 attack_triggers, OnUse, OnCast, projectile_hit,      |   { trigger, source, caster, rank, origin,
 contact, channel end, WhileToggled ----------------->|     aim, point, cast_target, trigger_unit,
                       (Triggers)                     |     affected_units, attack, depth, magnitude }
                                                      v
                                          +--------------------+
                                          |  Effects           |  entries whose trigger matches,
                                          |  EffectRunner      |  EffectTarget resolved against ctx
                                          +--------------------+
                                            | Damage / Heal / ManaRestore / ResourceDrain
                                            | StatusEffect / Displacement / Teleport / ExtraAttack
                                            v
                                          +--------------------+        +-----------------+
                                          |  Gameplay rules    | <----> |  Modifiers      |
                                          | combat, status,    |  reads | CombatStats,    |
                                          | vitals, teleport   |        | statuses, items |
                                          +--------------------+        +-----------------+
```

Gates feed back: `Proc` / `Counter` / `Chain` push derived `TriggerContext`s into the same queue inside `resolve_ability_triggers`.

## 3. Frame order

All systems below run in `Update`.

1. **Combat chain** (`CombatPlugin`): attack cooldowns, basic attack requests and windups, releases, impacts, projectile flight (`fly_projectiles`, `skillshot_contacts`), `apply_projectile_hits`, **`apply_damage_events`**, lifetimes, `despawn_dead`.
2. **Abilities chain** (`AbilitiesPlugin`), ordered `.after(SimSet::Commands)`, `.after(apply_projectile_hits)` and `.before(apply_damage_events)`:
   1. cooldown, mana regen, `tick_toggle_upkeep`, `sync_ability_passives`
   2. activation: `resolve_queued_activations`, `validate_activation_requests`, `refresh_activation_aims`, `tick_activations` (emits `ActionFiredEvent`)
   3. `emit_ability_casts`, then `resolve_item_uses` (OnUse) followed by `execute_item_mechanics`, then `execute_ability_casts` (mechanics and OnCast)
   4. `action_channel_end_triggers`, `projectile_hit_triggers`, `attack_triggers`
   5. `tick_forced_movement`, `settle_grounded_units` (contact and movement-end triggers)
   6. **`resolve_ability_triggers`** (effects, including gates and their derived contexts)
   7. `spawn_requested_projectiles`
   8. appliers in order: heals, mana restore, resource drains, displacement, status events, resource-cap sync, teleport (`before(SimSet::Ai)`)
3. `tick_status_effects` runs `.before(apply_damage_events)`, so DoT damage lands in the same damage pass.
4. `SimSet::Commands` then `Ai` then `Movement` order the unit orders and locomotion.

A chain hop therefore resolves in the same frame as the trigger that started it (derived contexts resolve in the same call), and its damage lands in `apply_damage_events` that frame. Hops are picked before this frame's damage applies (risk R1 in the plan).

## 4. Modifier storage rules

Three storage styles coexist on purpose. Pick by the kind of change and keep a stat in one style.

| Style | Where | Use for | Lifetime |
|---|---|---|---|
| **Mutate on apply** | `CombatStats` fields changed by `add_status_stat_delta` / `subtract_status_stat_delta` (statuses) and `items::apply_passives` / `remove_passives` (items, passive abilities) | Flat combat stats: armor, move speed, attack damage, attack speed, range | Reverted with the exact delta when the status ends or the item leaves |
| **Sum on read** | `vitals::resource_stat`: permanent `CombatStats` value plus `StatusEffects::resource_sum` | Fractions and resource modifiers: lifesteal, heal amp, mana cost amp, regen, max health / mana | Always correct without revert bookkeeping |
| **Snapshot at build** | `StatusSpec::build` reads `StatusBuildContext.move_speed` / `agility` | Percentage slows relative to the target's speed when applied | Fixed at application |

`build_status_effect` is the single constructor for statuses (timed and toggled).

## 5. Gameplay-rule matrix

How each protection treats each kind of effect. "Skipped" means the unit is never selected or the event is dropped.

| Effect | Invulnerable | Debuff immunity | Untargetable (cyclone) | Damage immunity |
|---|---|---|---|---|
| **Damage** (`DamageEvent`) | Dropped before anything else (`combat.rs`, `is_invulnerable`) | No effect | Not selected by `EffectRunner::valid_target` or `UnitsInRadius`; cyclone is also invulnerable | Resolves to 0 (`resolve_incoming_damage`); sleep still wakes first (`status::wake_from_sleep`) |
| **Resource drain** (`ResourceDrainEvent`) | Hostile drains blocked (`vitals.rs`); self-spend allowed; burn damage then follows the Damage column | No effect | Not selected | Drain itself unaffected; burn damage is nullified |
| **Statuses** (`StatusEffectEvent`) | No effect (statuses still apply) | Debuff-kind statuses rejected in `status::apply_status`; gaining immunity purges strong-dispellable debuffs (`purge_strong_dispellable_debuffs`) | Not selected | No effect; DoT ticks go through the Damage column |
| **Displacement** (`DisplacementEvent`) | No effect | Hostile displacement skipped (`displacement.rs` via `status::hostile` + `blocks_hostile_relocation`); own and ally motion allowed | Cyclone motion has `Suspend` priority, so other motion is refused while it runs | No effect |
| **Teleport** (`TeleportEvent`) | No effect | Hostile teleport rejected with `TeleportError::Immune` (`teleport.rs`); self and ally teleports ignore it | `TeleportError::Suspended` while the cyclone motion runs; not selected as an effect target | No effect |

Hostility is `status::hostile(caster, target, ...)`: another unit that is not on the caster's team (units without a team count as hostile).

## 6. Worked compositions

Each box lists a layer on its own line. CSV rows are copied from `data/*.csv` and `data/examples/*.csv`; Bolt and the Rust-defined shape are from `src/abilities/catalog.rs`.

### 6.1 Chain Lightning: ability and item share rows

```text
Activation: unit target
    |
    v
Mechanic: chain between nearby targets (AbilityEffect::Chain, mechanics::chain_hops)
    |
    v
Trigger: on_chain_hit
    |
    v
Effect: magical damage
    |
    v
Repeat until maximum jumps reached
```

Ability (`data/examples/ability_effects.csv`, `abilities.csv` row `chain_lightning`, `target_type unit`):

```csv
chain_lightning,0,on_cast,cast_target,damage,,100,magical,,,,,
chain_lightning,1,on_cast,cast_target,chain,on_chain_hit,3,,,450,75,,
chain_lightning,2,on_chain_hit,trigger_unit,damage,,100,magical,,,,,
```

Item (`data/examples/item_effects.csv`, `items.csv` row `storm_charm`, `target_type unit`):

```csv
storm_charm,0,on_use,cast_target,damage,,100,magical,,,,,
storm_charm,1,on_use,cast_target,chain,on_chain_hit,3,,,450,75,,
storm_charm,2,on_chain_hit,trigger_unit,damage,,100,magical,,,,,
```

The rows are identical apart from the owner column and `on_cast` / `on_use`. Chain columns: `effect_id` = the custom trigger, `value` = jumps, `radius` = jump radius (world units), `amount` = percent kept per jump. The start unit is not re-fired; hop `n` carries `magnitude = 0.75^n`. A hop clears the attack metadata, so chained damage is spell damage. Test: `item_and_ability_chain_emit_identical_damage`.

### 6.2 Bolt

```text
Activation: unit target (cast range, mana, cooldown)
    |
    v
Mechanic: targeted projectile + AttackUnitTarget
    |
    v
Trigger: on_projectile_hit (struck) / on_impact (landed)
    |
    +--> Effect: magical damage to the trigger unit; reduced splash to affected units
    |
    v
Mechanic: Chain { jumps 2, radius 450, falloff 0.75, enemies } -> Trigger: on_chain_hit
    |
    v
Effect: magical damage (linear 45 + 12 per rank), scaled by magnitude
```

Defined in `catalog.rs` (`AbilityId::Bolt`); the primary target takes the projectile damage only.

### 6.3 Arcane Lance

```text
Activation: unit target
    |
    v
Mechanic: targeted projectile
    |
    v
Trigger: on_projectile_hit
    |
    v
Effect: magical damage (95 rank 1)
Effect: extra magical damage, scaling target_health_below 0.35
```

```csv
arcane_lance,0,on_projectile_hit,trigger_unit,damage,,95|127|159|191|223|255|287,magical,,,,,
arcane_lance,1,on_projectile_hit,trigger_unit,damage,,23.75|31.75|39.75|47.75|55.75|63.75|71.75,magical,,,,target_health_below,0.35
```

### 6.4 Seismic Slam and Cataclysm

```text
Activation: no target (Slam) / point (Cataclysm)
    |
    v
Mechanic: area ring FX (no gameplay)
    |
    v
Trigger: on_cast
    |
    v
Effect: units_in_radius (center Caster / Aim) -> damage, then slow (Slam) or stun (Cataclysm)
```

```csv
seismic_slam,0,on_cast,units_in_radius,damage,,80|110|140|170|200|230|260,physical,,,,,
seismic_slam,1,on_cast,units_in_radius,apply_status,slow,,,1.5,,30,,
cataclysm,0,on_cast,units_in_radius,damage,,200|270|340|410,magical,,,,,
cataclysm,1,on_cast,units_in_radius,apply_status,stun,,,1.2,,,,
```

### 6.5 Blink vs Blink Dagger

```text
Activation: point target (ability: cast_range 800; item: target_type point, cast_range 800)
    |
    v
Trigger: on_cast (ability) / on_use (item)
    |
    v
Effect: teleport to the aim point
    |
    v
Rules: validate destination; hostile teleport of a debuff-immune unit is refused
```

```csv
blink,0,on_cast,caster,movement,teleport,,,,,,,
blink_dagger,0,on_use,owner,movement,teleport,,,,,,,
```

### 6.6 Stone Skin vs Iron Bracer

```text
Activation: none (passive)
    |
    v
Trigger: passive (applied on loadout init / rank-up for the ability; on acquisition for the item)
    |
    v
Effect: PassiveStat armor
    |
    v
Modifier: mutate on apply (CombatStats), removed with the exact delta
```

```csv
stone_skin,0,passive,caster,stat_modifier,armor,4|8|12|16,,,,,,
iron_bracer,0,passive,owner,stat_modifier,max_health,100,,,,,,
iron_bracer,1,passive,owner,stat_modifier,armor,4,,,,,,
```

### 6.7 Overcharge

```text
Activation: toggle (flips on each cast)
    |
    v
Trigger: while_toggled (tick_toggle_upkeep)
    |
    v
Effect: apply status via build_status_effect (permanent, undispellable)
    |
    v
Rules: upkeep 8 mana/sec; turns off at 0 mana and the statuses are removed
```

```csv
overcharge,0,while_toggled,caster,stat_modifier,attack_speed_mult,0.2,,,,,,
overcharge,1,while_toggled,caster,stat_modifier,move_speed,10,,,,,,
```

### 6.8 Vial of Light, Storm Rod, Spark Pendant

```text
Activation: item active (no target), cooldown 40 / 30 / 25
    |
    v
Trigger: on_use
    |
    v
Effect: heal owner (Vial) | units_in_radius magical damage (Storm Rod 143, Spark Pendant 117) | + restore mana (Pendant)
```

```csv
vial_of_light,0,on_use,owner,heal,,180,,,,,,
storm_rod,2,on_use,units_in_radius,damage,,140,magical,,143,,,
spark_pendant,2,on_use,units_in_radius,damage,,90,magical,,117,,,
spark_pendant,3,on_use,owner,heal,mana,40,,,,,,
```

### 6.9 frost_hammer

```text
Activation: none (passive item; stats via passive rows)
    |
    v
Trigger: on_attack_hit (landed attacks only; fired by attack_triggers)
    |
    v
Effect: apply slow 25% for 2s to the trigger unit
    |
    v
Rules: attack bonus damage rules do not apply (status only); debuff immunity blocks the slow
```

```csv
frost_hammer,3,on_attack_hit,trigger_unit,apply_status,slow,,,2,,25,,
```

## 7. Checklist: adding a mechanic, effect or trigger

**New mechanic or gate**

1. Add the variant (`AbilityMechanic` or a gate `AbilityEffect`) in `definition.rs`.
2. Run it from `MechanicRunner::run` (or the gate arm of `EffectRunner::execute`). It decides who and when; it emits triggers or derived contexts, never Damage / Heal / Status events.
3. Take the source from `CastContext.source` / `TriggerContext.source`; never hard-code `EffectSource::Ability`. Make sure it works for items (`ItemDefinition::mechanics`).
4. Respect `MAX_TRIGGER_DEPTH` for anything that re-fires contexts; skip untargetable units.
5. Add the tooltip line (`tooltip.rs`; the `AbilityEffect` match is exhaustive).
6. Generators: add the `effect_type` / column validation in `generator_common.py`, add it to `warn_unreachable_custom_triggers` if it feeds a custom trigger, add examples in `data/examples/` and one row in `data/examples/invalid/`, add unittest cases.

**New effect**

1. Add the `AbilityEffect` variant and handle it in `EffectRunner::execute` only (no second executor). Reuse the shared events; choose damage kind with `DamageEvent::from_effect`, build statuses with `build_status_effect`.
2. Put the interaction rule in its owner (`combat.rs`, `status.rs`, `vitals.rs`, `displacement.rs`, `teleport.rs`) and add a row to the matrix in section 5.
3. Tooltip, generator validation, examples, tests (ability and item parity).

**New trigger**

1. Add the `AbilityTrigger` variant and its emitter. Every emitter builds the context with `TriggerContext::new` and carries an `EffectSource`.
2. Add it to `ABILITY_TRIGGERS` / `ITEM_TRIGGERS` in `generator_common.py`; if nothing emits it yet, list it in `NOT_EMITTED_TRIGGERS` so both generators warn.
3. Add `trigger_tag` text in `tooltip.rs`, trigger/target combination rules in the generators, and a test that the emitter fires for both abilities and items.

**Always**: run `cargo fmt --check`, `cargo build`, `cargo test`, both generators with `--check`, `python3 -m unittest test_generator_common` from `scripts/`, regenerate and confirm no diff in `generated.rs` / `item_catalog.rs`, and `--check` on `data/examples/`.
