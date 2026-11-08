# Mechanic composition: the six layers

Every ability and item is built from the same six layers. An ability and an item that use the same mechanic and the same effect rows behave identically; only the activation source differs.

Plan and decisions: `docs/plans/2026-11-08-mechanic-composition-layers.md`. Reactive and conditional triggers (PR #58) were merged into these layers by `docs/plans/2026-11-08-merge-reactive-triggers-into-composition-layers.md`; their original design is `docs/plans/2026-11-08-reactive-triggers.md`. Earlier plans (`2026-11-06-shared-effects-pipeline.md`, `2026-11-06-damage-mechanics.md`) are superseded in part by this document.

## 1. The layers

| Layer | Purpose | Owning code | May emit | May read |
|---|---|---|---|---|
| **Activation** | How an action starts: targeting, range, cost, cooldown, cast point, channel | `src/actions/` (`pipeline.rs`, `targeting.rs`, `resources.rs`); source-specific halves in `abilities/casting.rs` (`ability_action_spec`, `commit_ability`) and `items.rs` (`item_action_spec`, `commit_item`) | `ActionFiredEvent`, `ActionChannelEndEvent`; facts `AbilityCastEvent` (abilities only) and `CooldownReadyEvent` (carries an `EffectSource`, written when `ActionCooldown::tick` reports the cooldown became ready) | Loadout / inventory, mana, cooldowns, statuses (`DisableRule`) |
| **Mechanics** | How an action plays out over time and **who** is involved: dash, projectile (homing, skillshot, skillshot that widens with distance via `end_radius`), `ProjectileToEach` (one homing projectile per unit in an area), attack-unit, and the four gates (`Proc` incl. pseudo-random, `Counter`, `Accumulate`, `Chain`; gate state in `TriggerCounters { counts, prd, sums }`) | `abilities/mechanics.rs` (`MechanicRunner`, `CastContext`, `chain_hops`, `execute_ability_casts`, `execute_item_mechanics`), `combat.rs` projectile systems, `displacement.rs` motion | `AbilityTriggerEvent`, `SpawnProjectileEvent`, `DisplacementEvent` (dash). Never Damage, Heal or Status events | Positions, teams, health (alive), untargetable |
| **Triggers** | **When** effects run: `AbilityTrigger` plus the systems that emit it | `definition.rs` (`AbilityTrigger`, `StatusFilter`, `Condition`), `mechanics.rs` (`OnCast`, `projectile_hit_triggers`), `items.rs` (`OnUse`), `casting.rs` (channel end), `attack_triggers.rs`, `displacement.rs` (contact, movement end), `passive.rs` (`WhileToggled`), `reactive_triggers.rs` (dispatchers `track_travel`, `proximity_triggers`, `interval_triggers`, `cooldown_triggers`, `cast_triggers`, `damage_triggers`, `status_triggers`; state `TriggerTracking`; shared fan-out `owner_sources` / `source_triggers` / `fan_out`) | `AbilityTriggerEvent(TriggerContext)`. The dispatchers read only fact messages (I11) | The fired action; fact messages from the Effects and Gameplay-rules layers |
| **Effects** | **What** happens: `AbilityEffect` rows executed by the single executor. This is also the only place the reaction flag is stamped (I1, I10) | `abilities/effects.rs` (`EffectRunner::execute`, `resolve_ability_triggers`, `build_status_effect`) | `DamageEvent`, `HealEvent`, `ManaRestoreEvent`, `ResourceDrainEvent`, `StatusEffectEvent` (all stamped `.reactive(ctx.reaction)`), `DisplacementEvent`, `TeleportEvent`, `ExtraAttackEvent`, derived contexts from gates; the fact `SpellHitEvent` (ability source, depth 0, cast phase only) | Context, target stats and statuses (for scaling) |
| **Modifiers** | Persistent numeric changes | `PassiveStat` / `ItemPassives` (`items.rs`, `abilities/passive.rs`), `StatusSpec` modifier variants and `StatusEffect` stat fields (`status.rs`), `vitals::resource_stat` | Stat changes through `CombatStats` | Owned items, ranks, active statuses |
| **Gameplay rules** | How effects interact with the game: mitigation, immunity, untargetability, dispel, healing factors | `combat.rs` (`apply_damage_events`), `status.rs` (`apply_status`, `wake_from_sleep`, `purge_strong_dispellable_debuffs`, `hostile`, `blocks_hostile_relocation`), `vitals.rs`, `displacement.rs`, `teleport.rs` | Health / status / position changes (the only writers); the facts `DamageAppliedEvent` (only `apply_damage_events` writes it), `StatusLifecycleEvent` (only the status appliers, `tick_status_effects` and the sleep wake write it) and `TeleportCompletedEvent` / `TeleportRejectedEvent` | Statuses, stats, teams |

Invariants (I1 to I9 from the composition plan, with I1, I4 and I6 amended and I10 to I12 added by the merge plan):

- **I1.** `EffectRunner::execute` is the only place an `AbilityEffect` becomes gameplay messages. Every status (cast, toggle, `While`) is built by `build_status_effect`. Reactive stamping happens only there and in `reconcile_held_status`.
- **I2.** Mechanics and gates (`Proc`, `Counter`, `Accumulate`, `Chain`) decide who and when; they never write Damage, Heal or Status events (accepted exceptions: the basic-attack hit stage, DoT ticks, reflection and burn follow-ups).
- **I3.** Every trigger emitter carries an `EffectSource` (`Ability` or `Item`); nothing hard-codes `Ability`.
- **I4.** Health goes down only in `combat::apply_damage_events` (the only writer of `DamageAppliedEvent`) and `vitals::apply_resource_drain_events` (floors health at 1; fires no damage, kill or death triggers). Health goes up only in `vitals::apply_heal`. Statuses change only through `StatusEffectEvent`, `tick_status_effects` and `status::wake_from_sleep`, which are the only sources of `StatusLifecycleEvent`.
- **I5.** Immunity, untargetability, dispel and damage calculation are queried through the `status.rs` / `combat.rs` helpers. `DamageEvent::from_effect` alone chooses the damage kind; the Effects layer then stamps `.reactive(ctx.reaction)`.
- **I6.** Gates only push derived contexts, capped by `MAX_TRIGGER_DEPTH` (2) and resolved FIFO in the same call.
- **I7.** Source-specific rules stay in their own domain. Cooldown completion is reported once, with its source (`CooldownReadyEvent`).
- **I8.** Generated Rust (`generated.rs`, `item_catalog.rs`) changes only by re-running the generators.
- **I9.** The invariants of the cleanup plan still hold.
- **I10 (reaction rule).** `reaction` is set at the root (`trigger.is_reaction()` or a `MotionSource`) and inherited by every derived context, `Chain` hops included. Under a reaction, Damage, ResourceDrain and Status events are stamped `reactive`. Reactive changes never fire damage dealt, damage taken or status triggers. Kill, death and health-below still fire. `PerformAttack` is inert under a reaction.
- **I11.** Reactive dispatchers read only fact messages and write only `AbilityTriggerEvent`. The one exception is `sync_conditional_statuses`, which writes held statuses the way toggle upkeep does.
- **I12.** `TriggerContext.magnitude` scales the output of Damage, DrainHealth and Heal (only `Chain` sets it below 1); `TriggerContext.amount` sizes the occurrence that fired the trigger (only `Accumulate` reads it). Neither derives from the other.

## 2. Interfaces between layers

```text
 input / AI / shop
      |  ActivationRequest { caster, source: Ability(slot) | Item(slot), target }
      v
 +-----------------+   ActionFiredEvent        +----------------------------+
 |   Activation    | ------------------------> |  Mechanics                 |
 | actions/        |   ActionChannelEndEvent   |  dash, projectile, attack, |
 +-----------------+                           |  gates: Proc / Counter /   |
      | CooldownReadyEvent, AbilityCastEvent   |  Accumulate / Chain        |
      |  (facts)                               +----------------------------+
      |                                               | AbilityTriggerEvent(TriggerContext)
 attack_triggers, OnUse, OnCast, projectile_hit,      |   { trigger, source, caster, rank, origin,
 contact, channel end, WhileToggled,                  |     aim, point, cast_target, trigger_unit,
 reactive dispatchers ------------------------------->|     affected_units, attack, depth,
                       (Triggers)                     |     magnitude, amount, reaction, status }
                                                      v
                                          +--------------------+
                                          |  Effects           |  entries whose trigger matches,
                                          |  EffectRunner      |  EffectTarget resolved against ctx;
                                          +--------------------+  stamps .reactive(ctx.reaction)
                                            | Damage / Heal / ManaRestore / ResourceDrain
                                            | StatusEffect / Displacement / Teleport / ExtraAttack
                                            | (+ SpellHitEvent fact)
                                            v
                                          +--------------------+        +-----------------+
                                          |  Gameplay rules    | <----> |  Modifiers      |
                                          | combat, status,    |  reads | CombatStats,    |
                                          | vitals, teleport   |        | statuses, items |
                                          +--------------------+        +-----------------+
                                            | facts: DamageAppliedEvent, StatusLifecycleEvent,
                                            |        TeleportCompletedEvent / RejectedEvent,
                                            |        SpellHitEvent
                                            v
                              reactive dispatchers (Triggers layer, reactive_triggers.rs)
                              track_travel, proximity_triggers, interval_triggers,
                              cooldown_triggers, cast_triggers, damage_triggers, status_triggers
                                            |
                                            +--> AbilityTriggerEvent (next frame for damage,
                                                 kill, death, low health, status and spell-hit
                                                 facts; same frame for the rest) --> Effects
```

Gates feed back: `Proc` / `Counter` / `Accumulate` / `Chain` push derived `TriggerContext`s into the same queue inside `resolve_ability_triggers`. `ctx.derived()` carries `reaction`, `amount` and `status` through unchanged, so every gate inherits the reaction rule with no extra code. The fact-message loop (Effects -> Gameplay rules -> fact -> dispatcher -> Effects) is what the reaction rule keeps finite: reactive output never produces the facts that dispatch damage or status triggers.

## 3. Frame order

All systems below run in `Update`. The two plugin chains interleave: the abilities chain (2) runs after `apply_projectile_hits` and before `apply_damage_events`, both of which belong to the combat chain (1).

1. **Combat chain** (`CombatPlugin`): attack cooldowns, basic attack requests and windups, releases, impacts, projectile flight (`fly_projectiles`, `skillshot_contacts`), `apply_projectile_hits`, *[abilities chain runs here]*, **`apply_damage_events`**, lifetimes, `despawn_dead`.
2. **Abilities chain** (`AbilitiesPlugin`), ordered `.after(SimSet::Commands)`, `.after(apply_projectile_hits)` and `.before(apply_damage_events)`:
   1. cooldown, mana regen, `tick_toggle_upkeep`, `sync_ability_passives`
   2. activation: `resolve_queued_activations`, `validate_activation_requests`, `refresh_activation_aims`, `tick_activations` (emits `ActionFiredEvent`)
   3. `emit_ability_casts`, then `resolve_item_uses` (OnUse) followed by `execute_item_mechanics`, then `execute_ability_casts` (mechanics and OnCast)
   4. `action_channel_end_triggers`, `projectile_hit_triggers`, `attack_triggers`
   5. `tick_forced_movement`, `settle_grounded_units` (contact and movement-end triggers)
   6. reactive dispatchers, chained in this order: `track_travel`, `proximity_triggers`, `interval_triggers`, `cooldown_triggers` (reads `CooldownReadyEvent`), `cast_triggers` (reads `AbilityCastEvent`, `SpellHitEvent`), `damage_triggers` (reads `DamageAppliedEvent`), `status_triggers` (reads `StatusLifecycleEvent`, `.after(tick_status_effects)`), `sync_conditional_statuses` (`While` rows)
   7. **`resolve_ability_triggers`** (effects, including gates and their derived contexts; writes `SpellHitEvent`)
   8. `spawn_requested_projectiles`
   9. appliers in order: heals, mana restore, resource drains, displacement, status events (write `StatusLifecycleEvent`), resource-cap sync, teleport (`before(SimSet::Ai)`; writes `TeleportCompletedEvent` / `TeleportRejectedEvent`)
3. `tick_status_effects` runs `.before(apply_damage_events)`, so DoT damage lands in the same damage pass.
4. `SimSet::Commands` then `Ai` then `Movement` order the unit orders and locomotion.

A chain hop therefore resolves in the same frame as the trigger that started it (derived contexts resolve in the same call), and its damage lands in `apply_damage_events` that frame. Hops are picked before this frame's damage applies (risk R1 in the plan).

**Delays.** `apply_damage_events` writes `DamageAppliedEvent` after the dispatchers ran, so facts produced in frame N are read in frame N+1.

| Trigger | Fires | Why |
|---|---|---|
| `on_damage_dealt`, `on_damage_taken`, `on_kill`, `on_death`, `on_health_below` | next frame | Read from `DamageAppliedEvent`, written by `apply_damage_events` at the end of the frame |
| `on_status_applied`, `on_status_removed` | next frame for applications and removals made by the appliers; same frame for expiry (`tick_status_effects` runs before `status_triggers`) | Read from `StatusLifecycleEvent` |
| `on_spell_hit` | next frame | Read from `SpellHitEvent`, written by `resolve_ability_triggers` |
| `on_spell_cast` | same frame | `AbilityCastEvent` is written earlier in the frame |
| `on_attack`, `on_attack_hit`, `on_use`, `on_cast`, `on_projectile_hit`, `on_impact` | same frame | Emitted by the activation, mechanics and `attack_triggers` systems |
| `on_cooldown_ready`, `on_interval`, `on_move`, `on_enemy_enter`, `on_enemy_leave` | same frame | Emitted by the dispatchers from this frame's state |
| gate output (`on_overload`, `on_chain_hit`, ...) | same frame as the trigger that fed the gate | Derived contexts resolve in the same call |
| `on_channel_tick`, `on_expire` | never | Not emitted yet; both generators warn |

A reaction to damage therefore lands one frame after the hit that caused it, and a chain started by that reaction resolves in that later frame.

## 4. Modifier storage rules

Three storage styles coexist on purpose. Pick by the kind of change and keep a stat in one style.

| Style | Where | Use for | Lifetime |
|---|---|---|---|
| **Mutate on apply** | `CombatStats` fields changed by `add_status_stat_delta` / `subtract_status_stat_delta` (statuses) and `items::apply_passives` / `remove_passives` (items, passive abilities) | Flat combat stats: armor, move speed, attack damage, attack speed, range | Reverted with the exact delta when the status ends or the item leaves |
| **Sum on read** | `vitals::resource_stat`: permanent `CombatStats` value plus `StatusEffects::resource_sum` | Fractions and resource modifiers: lifesteal, heal amp, mana cost amp, regen, max health / mana | Always correct without revert bookkeeping |
| **Snapshot at build** | `StatusSpec::build` reads `StatusBuildContext.move_speed` / `agility` | Percentage slows relative to the target's speed when applied | Fixed at application |

`build_status_effect` is the single constructor for statuses (timed, toggled and held `While` statuses). Held statuses get `dot_source = owner` and the owner's outgoing heal amp, exactly like toggle statuses.

## 5. Gameplay-rule matrix

How each protection treats each kind of effect. "Skipped" means the unit is never selected or the event is dropped.

| Effect | Invulnerable | Debuff immunity | Untargetable (cyclone) | Damage immunity | Fires reactive triggers? |
|---|---|---|---|---|---|
| **Damage** (`DamageEvent`) | Dropped before anything else (`combat.rs`, `is_invulnerable`) | No effect | Not selected by `EffectRunner::valid_target` or `UnitsInRadius`; cyclone is also invulnerable | Resolves to 0 (`resolve_incoming_damage`); sleep still wakes first (`status::wake_from_sleep`) | Unless reactive (`DamageEvent.reactive`): dealt and taken, then kill, death, health-below. Reactive: only kill, death, health-below |
| **Resource drain** (`ResourceDrainEvent`) | Hostile drains blocked (`vitals.rs`); self-spend allowed; burn damage then follows the Damage column | No effect | Not selected | Drain itself unaffected; burn damage is nullified | Never dealt or taken; the burn damage follows the Damage row |
| **Statuses** (`StatusEffectEvent`) | No effect (statuses still apply) | Debuff-kind statuses rejected in `status::apply_status`; gaining immunity purges strong-dispellable debuffs (`purge_strong_dispellable_debuffs`) | Not selected | No effect; DoT ticks go through the Damage column | Unless reactive: `on_status_applied` / `on_status_removed` via `StatusLifecycleEvent` |
| **Displacement** (`DisplacementEvent`) | No effect | Hostile displacement skipped (`displacement.rs` via `status::hostile` + `blocks_hostile_relocation`); own and ally motion allowed | Cyclone motion has `Suspend` priority, so other motion is refused while it runs | No effect | No; a cyclone launched by a reaction applies a reactive status, so it is silent |
| **Teleport** (`TeleportEvent`) | No effect | Hostile teleport rejected with `TeleportError::Immune` (`teleport.rs`); self and ally teleports ignore it | `TeleportError::Suspended` while the cyclone motion runs; not selected as an effect target | No effect | No; completed teleports only reset travel tracking (not counted for `on_move`) |

### Reaction rule per event

Which events fire reactive triggers (`on_damage_dealt`, `on_damage_taken`, `on_status_applied`, `on_status_removed`) depends on whether the effect was caused by a reaction. "Reactive" means the message carries `reactive = true`, stamped by the Effects layer from `ctx.reaction`.

| Effect | Fires reactive triggers? |
|---|---|
| Damage from a non-reaction trigger | Yes: dealt for the source's owner, taken for the target |
| Damage from a reaction (thorns, Storm Mail arcs), including `Chain` hops | No dealt, no taken. Kill, death and health-below still fire |
| Burn / DoT ticks | Never dealt or taken (`DamageKind::Dot`). A burn started by a reaction is reactive and its ticks stay reactive. Kill, death and health-below still fire. Ticking burns (`tick_interval > 0`) follow the same rule: `tick_status_effects` writes one Dot event per interval, the first one interval after application, `floor(duration / interval)` in total; ticks still owed are flushed when the burn expires on its own, never on a dispel or a refresh. Generated burns carry the id `<owner>_burn`, so burns from different sources coexist (see 6.11) |
| Reflected damage | Reactive; fires nothing but kill / death / health-below |
| Resource drain | Never fires damage triggers (floors health at 1). Reactive when started by a reaction |
| Status applied or removed by a non-reaction effect | Yes: `on_status_applied` / `on_status_removed` for the holder |
| Status applied by a reaction, held `While` / toggle statuses, cyclone launched by a reaction | No: the status is stamped `reactive` and its expiry is silent too |
| Sleep break | Reported `Removed` (attributed to the damage source) unless the damage is reactive |
| Debuff-immunity purge | Reported `Dispelled`, unless the applying event is reactive |
| `PerformAttack` | Inert under a reaction; the generator also rejects it under reaction triggers and chain-fed triggers |

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

Hops are not spell hits: `OnSpellHit` counts only depth-0 cast-phase contexts, and a `Chain` is a gate, not a hit (`acts_on_targets` is false). Hops still deal damage, so non-reactive hop damage fires `on_damage_dealt` / `on_damage_taken` on the next frame with its post-mitigation `amount`. Hops inherit `amount`, `reaction` and `status` unchanged. Tests: `chain_hops_are_not_spell_hits`, `chain_hop_damage_fires_damage_dealt_next_frame`, `chain_hops_inherit_amount_reaction_and_status`.

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

Bounded teleport (Queen of Pain's blink, 6.11): the same effect with `min_distance` / `max_distance`. `EffectRunner` clamps the flat distance from the caster to the aim point into that range along the same line (an aim on the caster uses her facing) and sends the result as `TeleportEvent.destination`; `teleport.rs` is unchanged and still validates the map, terrain, occupancy, leash and immunity. Blank bounds keep the destination exactly at the aim point, so Blink and Blink Dagger are unchanged. Combined with `cast_range=global` (`scale::GLOBAL_CAST_RANGE`) the cast never walks the hero into range.

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

### 6.10 Storm Mail: accumulate, chain, reactive damage (item and passive ability)

```text
Activation: none (passive item; armor via a passive row)
    |
    v
Trigger: on_damage_taken (fact: DamageAppliedEvent, read one frame later; trigger_unit = attacker, amount = damage taken)
    |        ctx.reaction = true (on_damage_taken is a reaction trigger)
    v
Mechanic: Accumulate { threshold 300 } sums ctx.amount per owner; at >= 300 it fires and keeps the remainder
    |        derived context (depth 1) inherits reaction, amount, status
    v
Trigger: on_overload (custom)
    |
    +--> Effect: magical damage 80 to the attacker (trigger_unit), stamped reactive
    |
    v
Mechanic: Chain { jumps 2, radius 450, falloff 0.75, enemies } from the attacker
    |        each hop is a derived context (depth 2, MAX_TRIGGER_DEPTH), magnitude 0.75^n, reaction kept
    v
Trigger: on_storm_arc (custom)
    |
    v
Effect: magical damage 80 x magnitude to each hop target, stamped reactive
    |
    v
Rules: reactive damage fires no on_damage_dealt / on_damage_taken, so arcs cannot re-feed the Accumulate;
       kill, death and health-below still fire; hops are not spell hits; PerformAttack is inert
```

Item (`data/examples/items.csv` row `storm_mail`, `data/examples/item_effects.csv`):

```csv
storm_mail,0,passive,owner,stat_modifier,armor,6,,,,,,
storm_mail,1,on_damage_taken,owner,accumulate,on_overload,300,,,,,,
storm_mail,2,on_overload,trigger_unit,chain,on_storm_arc,2,,,450,75,,
storm_mail,3,on_overload,trigger_unit,damage,,80,magical,,,,,
storm_mail,4,on_storm_arc,trigger_unit,damage,,80,magical,,,,,
```

As a passive ability the same rows are written with the ability id as the owner and `caster` in place of `owner`; the effects are identical. Test: `storm_mail_item_and_passive_ability_emit_identical_damage`; the accumulate-then-chain depth bound is checked by `accumulate_then_chain_under_damage_taken_resolves_within_depth_cap` and the no-ping-pong property by `reactive_chain_between_holders_does_not_ping_pong`. Under a derived custom trigger, `trigger_unit` is still the attacker, so the damage rows target `trigger_unit`.

Two sibling reactions use the same path without a gate:

```csv
thorn_mail,1,on_damage_taken,attacker,damage,,40,magical,,,,,
thornskin,0,on_damage_taken,attacker,damage,,20|35|50,magical,,,,,
bloodstone,0,on_kill,owner,heal,,150,,,,,,
```

`thorn_mail` (item) and `thornskin` (learned passive) are the same row with a different owner column and fire the same `OnDamageTaken` through `owner_sources`; the reflected hit is reactive. `bloodstone` fires `on_kill` for the owner of the damage that crossed zero health; kill and death still fire for reactive causes.

### 6.11 Queen of Pain

All four abilities are data rows (`data/abilities.csv`, `data/ability_effects.csv`); the only hand-written pieces are the shared mechanics, effects and rules introduced for them.

**Shadow Strike** (unit target, homing projectile): damage, a 16 s slow and a ticking burn, all on `on_projectile_hit,trigger_unit`.

```text
Mechanic: Projectile (homing) -> OnProjectileHit
Effects:  magical damage 50..230 | slow 25..85% for 16 s | burn 20..140 per tick, tick_interval 3 s, 16 s
Rules:    status id shadow_strike_burn; first tick 3 s after impact, 5 ticks (DamageKind::Dot)
```

```csv
shadow_strike,2,on_projectile_hit,trigger_unit,apply_status,burn,20|40|60|80|100|120|140,magical,16,,3,,
```

**QoP Blink** (`queen_of_pain_blink`, display name "QoP Blink"): `cast_range=global` plus a bounded teleport, `value` = max distance (1075 to 1525) and `amount` = min distance (200).

```csv
queen_of_pain_blink,0,on_cast,caster,movement,teleport,1075|1150|1225|1300|1375|1450|1525,,,,200,,
```

**Scream of Pain** (no target, radius 600): `projectile_type=targeted_each` -> `AbilityMechanic::ProjectileToEach { center: Caster, team: Enemy, speed: 900 }` plus the shockwave ring. Each projectile fires its own `OnProjectileHit` with `trigger_unit` set to the struck enemy; the damage row is an ordinary `damage` effect.

**Sonic Wave** (interval design): one expanding skillshot (`projectile_end_radius` 450 from `aoe_radius` 100, 900 units at 642.857 u/s = 1.4 s) that hits each enemy once. On hit it applies a pure ticking burn (23/34/45/56 per tick, `tick_interval` 0.1 s, 1.4 s = exactly 14 instances, id `sonic_wave_burn`) and one knockback of 350 over 1.4 s, away from the contact point. Because the burn ids are per owner it coexists with Shadow Strike's. Tests: `sonic_wave_deals_fourteen_instances_and_one_knockback`, `shadow_strike_and_sonic_wave_dots_coexist`.

## 7. Checklist: adding a mechanic, effect or trigger

**New mechanic or gate**

1. Add the variant (`AbilityMechanic` or a gate `AbilityEffect`) in `definition.rs`.
2. Run it from `MechanicRunner::run` (or the gate arm of `EffectRunner::execute`). It decides who and when; it emits triggers or derived contexts, never Damage / Heal / Status events.
3. Take the source from `CastContext.source` / `TriggerContext.source`; never hard-code `EffectSource::Ability`. Make sure it works for items (`ItemDefinition::mechanics`).
4. Respect `MAX_TRIGGER_DEPTH` for anything that re-fires contexts; skip untargetable units.
5. Add the tooltip line (`tooltip.rs`; the `AbilityEffect` match is exhaustive, and `acts_on_targets` must say whether it is a spell hit: gates are not).
   - A gate builds its output with `ctx.derived(then)`, never a fresh `TriggerContext`, so `reaction`, `amount` and `status` are inherited (I10).
6. Generators: add the `effect_type` / column validation in `generator_common.py`, add it to `warn_unreachable_custom_triggers` if it feeds a custom trigger, add examples in `data/examples/` and one row in `data/examples/invalid/`, add unittest cases.

**New effect**

1. Add the `AbilityEffect` variant and handle it in `EffectRunner::execute` only (no second executor). Reuse the shared events; choose damage kind with `DamageEvent::from_effect`, build statuses with `build_status_effect`.
2. Put the interaction rule in its owner (`combat.rs`, `status.rs`, `vitals.rs`, `displacement.rs`, `teleport.rs`) and add a row to the matrix in section 5.
3. Tooltip, generator validation, examples, tests (ability and item parity).
4. Under the reaction rule: stamp `.reactive(ctx.reaction)` on every Damage, ResourceDrain and Status message the effect emits, in `EffectRunner::execute` only; a custom effect (`custom.rs`) must do the same. Add the effect to the reaction table in section 5.

**New trigger**

1. Add the `AbilityTrigger` variant and its emitter. Every emitter builds the context with `TriggerContext::new` and carries an `EffectSource`.
2. Add it to `ABILITY_TRIGGERS` / `ITEM_TRIGGERS` in `generator_common.py` (reactive ones through `REACTIVE_PLAIN_TRIGGERS` / `REACTIVE_PARAM_TRIGGERS`); if nothing emits it yet, list it in `NOT_EMITTED_TRIGGERS` (now only `on_channel_tick` and `on_expire`) so both generators warn.
   - A dispatcher reads fact messages and writes only `AbilityTriggerEvent` (I11). Decide whether the trigger is a reaction (`is_reaction()`): reaction triggers must not be fed by reactive output, and the generator rejects `perform_attack` under them and under anything a `chain` row feeds (`chain_fed_triggers`).
   - State its delay (same frame or next frame) in the table in section 3.
3. Add `trigger_tag` text in `tooltip.rs`, trigger/target combination rules in the generators, and a test that the emitter fires for both abilities and items.

**Always**: run `cargo fmt --check`, `cargo build`, `cargo test`, both generators with `--check`, `python3 -m unittest test_generator_common` from `scripts/`, regenerate and confirm no diff in `generated.rs` / `item_catalog.rs`, and `--check` on `data/examples/`.
