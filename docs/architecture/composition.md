# Mechanic composition: the six layers

Every ability and item is built from the same six layers. An ability and an item that use the same mechanic and the same effect rows behave identically; only the activation source differs.

Plan and decisions: `docs/plans/2026-11-08-mechanic-composition-layers.md`. Reactive and conditional triggers (PR #58) were merged into these layers by `docs/plans/2026-11-08-merge-reactive-triggers-into-composition-layers.md`; their original design is `docs/plans/2026-11-08-reactive-triggers.md`. Earlier plans (`2026-11-06-shared-effects-pipeline.md`, `2026-11-06-damage-mechanics.md`) are superseded in part by this document.

## 1. The layers

| Layer | Purpose | Owning code | May emit | May read |
|---|---|---|---|---|
| **Activation** | How an action starts: targeting, range, cost, cooldown, cast point, channel | `src/actions/` (`pipeline.rs`, `targeting.rs`, `resources.rs`); source-specific halves in `abilities/casting.rs` (`ability_action_spec`, `commit_ability`) and `items.rs` (`item_action_spec`, `commit_item`) | `ActionFiredEvent`, `ActionChannelEndEvent`; facts `AbilityCastEvent` (abilities only) and `CooldownReadyEvent` (carries an `EffectSource`, written when `ActionCooldown::tick` reports the cooldown became ready) | Loadout / inventory, mana, cooldowns, statuses (`DisableRule`) |
| **Mechanics** | How an action plays out over time and **who** is involved: dash, projectile (homing, skillshot, skillshot that widens with distance via `end_radius`), `ProjectileToEach` (one homing projectile per unit in an area), attack-unit, the four gates (`Proc` incl. pseudo-random, `Counter`, `Accumulate`, `Chain`; gate state in `TriggerCounters { counts, prd, sums }`), and the `Zone` gate (a delayed, persistent area that fires a custom trigger on every tick; `abilities/zones.rs`) | `abilities/mechanics.rs` (`MechanicRunner`, `CastContext`, `chain_hops`, `execute_ability_casts`, `execute_item_mechanics`), `combat.rs` projectile systems, `displacement.rs` motion | `AbilityTriggerEvent`, `SpawnProjectileEvent`, `DisplacementEvent` (dash). Never Damage, Heal or Status events | Positions, teams, health (alive), untargetable |
| **Triggers** | **When** effects run: `AbilityTrigger` plus the systems that emit it | `definition.rs` (`AbilityTrigger`, `StatusFilter`, `Condition`), `mechanics.rs` (`OnCast`, `projectile_hit_triggers`), `items.rs` (`OnUse`), `casting.rs` (channel end), `attack_triggers.rs`, `displacement.rs` (contact, movement end), `passive.rs` (`WhileToggled`), `reactive_triggers.rs` (dispatchers `track_travel`, `proximity_triggers`, `interval_triggers`, `cooldown_triggers`, `cast_triggers`, `damage_triggers`, `status_triggers`; state `TriggerTracking`; shared fan-out `owner_sources` / `source_triggers` / `fan_out`) | `AbilityTriggerEvent(TriggerContext)`. The dispatchers read only fact messages (I11) | The fired action; fact messages from the Effects and Gameplay-rules layers |
| **Effects** | **What** happens: `AbilityEffect` rows executed by the single executor. This is also the only place the reaction flag is stamped (I1, I10) | `abilities/effects.rs` (`EffectRunner::execute`, `resolve_ability_triggers`, `build_status_effect`) | `DamageEvent`, `HealEvent`, `ManaRestoreEvent`, `ResourceDrainEvent`, `StatusEffectEvent` (all stamped `.reactive(ctx.reaction)`), `DisplacementEvent`, `TeleportEvent`, `ExtraAttackEvent`, derived contexts from gates; the fact `SpellHitEvent` (ability source, depth 0, cast phase only) | Context, target stats and statuses (for scaling) |
| **Modifiers** | Persistent numeric changes | `PassiveStat` / `ItemPassives` (`items.rs`, `abilities/passive.rs`), `StatusSpec` modifier variants and `StatusEffect` stat fields (`status.rs`), `vitals::resource_stat` | Stat changes through `CombatStats` | Owned items, ranks, active statuses |
| **Gameplay rules** | How effects interact with the game: mitigation, immunity, untargetability, concealment, spell landing (block / reflect), the disable matrix (`ActionBlocks`), dispel, healing factors | `combat.rs` (`apply_damage_events`), `status.rs` (`apply_status`, `wake_from_sleep`, `purge_strong_dispellable_debuffs`, `hostile`, `blocks_hostile_relocation`), `vitals.rs`, `displacement.rs`, `teleport.rs` | Health / status / position changes (the only writers); the facts `DamageAppliedEvent` (only `apply_damage_events` writes it), `StatusLifecycleEvent` (only the status appliers, `tick_status_effects` and the sleep wake write it) and `TeleportCompletedEvent` / `TeleportRejectedEvent` | Statuses, stats, teams |

Invariants (I1 to I9 from the composition plan, with I1, I4 and I6 amended and I10 to I12 added by the merge plan):

- **I1.** `EffectRunner::execute` is the only place an `AbilityEffect` becomes gameplay messages. Every status (cast, toggle, `While`) is built by `build_status_effect`. Reactive stamping happens only there and in `reconcile_held_status`.
- **I2.** Mechanics and gates (`Proc`, `Counter`, `Accumulate`, `Chain`, `Zone`) decide who and when; they never write Damage, Heal or Status events (accepted exceptions: the basic-attack hit stage, DoT ticks, reflection and burn follow-ups).
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
   4. `action_channel_end_triggers`, `projectile_hit_triggers`, `attack_triggers`, `consume_attack_charges`, `break_invisibility` (reads non-extra `BasicAttackReleaseEvent` and `ActionFiredEvent`; writes `StatusChange::Remove` for invisibility that breaks on action)
   5. `tick_forced_movement`, `settle_grounded_units` (contact and movement-end triggers)
   6. reactive dispatchers, chained in this order: `track_travel`, `proximity_triggers`, `interval_triggers`, `tick_zones` (fires the zone ticks that came due; a `follow_caster` zone is first re-centred on its caster, or despawned when the caster is dead), `cooldown_triggers` (reads `CooldownReadyEvent`), `cast_triggers` (reads `AbilityCastEvent`, `SpellHitEvent`), `damage_triggers` (reads `DamageAppliedEvent`), `status_triggers` (reads `StatusLifecycleEvent`, `.after(tick_status_effects)`), `sync_conditional_statuses` (`While` rows)
   7. **`resolve_ability_triggers`** (effects, including gates and their derived contexts; writes `SpellHitEvent`)
   8. `spawn_requested_zones` (reads `SpawnZoneEvent`; the zone starts counting next frame), `spawn_requested_projectiles`
   9. appliers in order: heals, mana restore, resource drains, displacement, status events (write `StatusLifecycleEvent`), resource-cap sync, attribute sync (`sync_status_attributes`, so it runs after `apply_status_effect_events` and `tick_status_effects`), teleport (`before(SimSet::Ai)`; writes `TeleportCompletedEvent` / `TeleportRejectedEvent`)
3. `tick_status_effects` runs `.before(apply_damage_events)` so DoT damage lands in the same damage pass, and `.after(tick_activations).after(tick_attack_swings)` so interrupt checks see a stun before it expires (a stun shorter than one frame still cancels a channel, cast point or swing).
4. `SimSet::Commands` then `Ai` then `Movement` order the unit orders and locomotion. Inside `Ai` the chain is `follow_lane_waypoints`, `acquire_targets`, `player_attack_move`, **`enforce_compulsions`**, `assign_attack_positions`: `enforce_compulsions` is the single order writer for feared, taunted and hypnotized units (it clears their queue and attack-move, writes the forced `MoveTarget` / `AttackTarget`, and ends taunt / hypnosis when the source is gone), so it must run after everything that could write an order and before attack positions are assigned. `process_issued_commands` drops commands for units whose `ActionBlocks` contain `ORDERS`.

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
| zone tick (`on_shrapnel_tick`, ...) | the frame the tick comes due (`delay + k x interval`), resolved in that frame's `resolve_ability_triggers` | A zone spawned in frame N starts counting in N+1; overdue ticks catch up in one frame, never more than `ticks` in total |
| `on_channel_tick`, `on_expire` | never | Not emitted yet; both generators warn |

A reaction to damage therefore lands one frame after the hit that caused it, and a chain started by that reaction resolves in that later frame.

## 4. Modifier storage rules

Four storage styles coexist on purpose. Pick by the kind of change and keep a stat in one style.

| Style | Where | Use for | Lifetime |
|---|---|---|---|
| **Mutate on apply** | `CombatStats` fields changed by `add_status_stat_delta` / `subtract_status_stat_delta` (statuses) and `items::apply_passives` / `remove_passives` (items, passive abilities) | Flat combat stats: armor, move speed, attack damage, attack speed, range | Reverted with the exact delta when the status ends or the item leaves |
| **Sum on read** | `vitals::resource_stat`: permanent `CombatStats` value plus `StatusEffects::resource_sum` | Fractions and resource modifiers: lifesteal, heal amp, mana cost amp, regen, max health / mana | Always correct without revert bookkeeping |
| **Snapshot at build** | `StatusSpec::build` reads `StatusBuildContext.move_speed` / `agility` | Percentage slows relative to the target's speed when applied. `apply_status` then re-measures a slow (`move_slow_fraction`) against the speed without status deltas, so a refreshed slow never compounds. Generated slows carry the id `<owner>_slow`, so slows from different sources coexist and add (floored at `scale::MIN_MOVE_SPEED`) | Fixed at application |
| **Reconcile from statuses** | `vitals::sync_status_resource_caps` (`AppliedStatusCaps`: timed max health / mana) and `vitals::sync_status_attributes` (`AppliedStatusAttributes`: timed strength / agility / intelligence, folded into `HeroAttributes` through `HeroAttributes::apply_delta`, the level-up path) | Values other systems derive from or clamp against, where a per-frame diff is simpler than revert bookkeeping | Every removal path (expiry, dispel, purge, remove) is correct without extra code; attack speed lags a changed agility by up to one frame |
| **Counter on the status** | `StatusEffect.stacks` (stacking debuffs, merged in `status::apply_status`) and `StatusEffect.attack_charges` (decremented by `StatusChange::ConsumeAttackCharge` in `apply_status_effect_events`) | Per-holder counts that belong to one status: Fury Swipes stacks, Overpower charges | Lost with the status (expiry, dispel, removal); a refresh adds stacks, or resets the charges |

**Counter / refresh and independent instances.** `status::apply_status` is the only place that decides what a same-id application does: it replaces the old instance (a refresh), merges stacks, or, when `StatusEffect.independent` is set (the `:independent` suffix on a timed `stat_modifier`), skips the replacement so every application keeps its own instance and timer. Removal by id, dispel and debuff-immunity purge remove every matching instance (`remove_statuses` retains by predicate), and each instance reports its own `Applied` / `Expired` lifecycle event, so the status vector grows by one entry per application (Essence Shift: roughly 15 to 20 per target in a long fight; the HUD groups them).

**Caps.** `StatusEffect.move_speed_cap` (0 = none) is applied by `CombatStats::effective_move_speed` as `.min(lowest live cap)` after the `scale::MIN_MOVE_SPEED` floor and the slow sum. Hex uses `scale::HEX_MOVE_SPEED`; the cap is not a stat delta, so nothing is reverted when it ends.

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
| **Damage reduction** (`StatusSpec::DamageReduction`) | Not reached (the event is dropped first) | No effect (a buff) | Not selected | Not reached (damage immunity resolves to 0 first) | Not an event: `StatusEffects::damage_taken_factor()` multiplies the post-mitigation damage in `combat::resolve_incoming_damage` for every damage type; shields soak the pre-armor amount first |
| **Status resistance** (`StatusSpec::StatusResistance`) | No effect | No effect (a buff); a debuff-immune unit still rejects the debuff | Not selected | No effect | Not an event: `apply_status_effect_events` scales the duration of a finite hostile debuff (another unit as source, not reactive) by `status_resistance_factor()` before applying it |

### Disable matrix, targeting and spell landing

**`ActionBlocks`** (`status.rs`) is the one disable matrix: a `u8` bit set derived from status flags by `StatusEffect::blocks()` and unioned by `StatusEffects::blocks()`. `can_move`, `can_attack`, `can_cast`, `is_rooted`, `is_disarmed` and `DisableRule::blocks` (item use) are all expressed on top of it.

| Flag / field | MOVE | ATTACK | CAST | ITEMS | ORDERS | PASSIVES |
|---|---|---|---|---|---|---|
| `stunned` | x | x | x | x | | |
| `asleep` | x | x | x | x | | |
| `rooted` | x | | | | | |
| `silenced` | | | x | | | |
| `muted` | | | | x | | |
| `disarmed` | | x | | | | |
| `breaks_passives` (break, hex) | | | | | | x |
| `compulsion` Flee (fear) / Approach (hypnosis) | | x | x | x | x | |
| `compulsion` Attack (taunt) | | | x | x | x | |

`ORDERS` drops player and AI orders. `PASSIVES` suppresses ability passives only (stats, triggers, conditional statuses; `passive_suppressed`); item passives keep working.

**Targeting intent.** Every "may this unit be picked" test other than an area test goes through `status::is_targetable(statuses, entity, TargetIntent)` / `StatusEffects::rejects(intent)`:

| Intent | Rejects | Used by |
|---|---|---|
| `Any` | `untargetable` (cyclone, banish) only | area membership (`unit_in_area`, `UnitsInRadius`), `valid_target` |
| `Attack` | untargetable, `attack_immune`, `targeting_blocked`, concealed | basic attack start / swing / acquisition, AI target selection, `UnitCommand::Attack`, taunt targets |
| `HostileSpell` | untargetable, `spell_immune`, `targeting_blocked`, concealed | `UnitInfo.rejects_hostile_spell` in `actions/targeting.rs::resolve_target` for enemy unit targets (allies and self are never rejected) |

**Targeting-only untargetable (`apply_status,untargetable`, Shadow Dance).** `StatusEffect.targeting_blocked` is read only by `StatusEffects::rejects`, in the `Attack` and `HostileSpell` arms. It is deliberately not the full `untargetable` flag: area effects (`Any`, so enemy and ally areas), dash contact (`displacement.rs`) and homing projectiles already in flight (`combat.rs`) still reach the holder, because those read `is_untargetable()`. Attackers drop the holder (the swing and acquisition checks read `rejects(Attack)`), enemies cannot start a unit-target cast on it (allies and self are never rejected), and its own caster-targeted effects keep working. Cyclone and banish keep the full flag.

**Damage-type immunity.** `StatusEffect.immune_damage` is a `DamageTypeMask`; `combat.rs` asks `is_immune_to(damage_type)` for the event's type. Spell immunity immunises to magical damage only, ethereal to physical only, invulnerable to everything (dropped earlier).

**Concealment.** A unit is concealed when any live status has `invisible` and none has `revealed` (`StatusEffects::is_concealed`). Concealed units are rejected by `Attack` and `HostileSpell` picks, so creeps and towers do not acquire them and unit-target spells cannot be cast at them; area effects, `Any` picks and ally picks still reach them. `fog.rs` keeps a concealed enemy hidden even inside vision, and a `hidden` unit (banish) is drawn for nobody. True sight is content: a proximity / interval trigger applying `reveal`. Invisibility with `invis_breaks_on_action` ends via `break_invisibility` (a non-extra attack release or a fired activation).

**Spell landing.** Only the landing of a hostile unit-target spell is blockable or reflectable. `effects::landing_target(ctx, defs)` returns the unit when the context is at depth 0, not a reaction, the source is cast on a unit, and either `OnProjectileHit` struck the cast target or `OnCast` / `OnUse` fired with no homing projectile mechanic (launching a projectile is not a landing). Skillshots, areas, `ProjectileToEach` fan-outs, gate-derived contexts and ally casts never qualify. In `resolve_ability_triggers`, for an enemy landing target:

1. `spell_immune`: the target is removed from every entry's targets of this context (shielded).
2. else an unspent live `spell_block`: shielded, and the status is removed with `StatusChange::Remove`. Because the removal is applied after the call, a per-call list of spent blocks stops a second spell resolved in the same call from reusing it.
3. else a live `spell_reflect` and `!ctx.reflected`: a `SpellReflectEvent` is written and the original still lands (Lotus style).

A shielded target produces no `SpellHitEvent`. `SpellReflectEvent` is consumed next frame by `execute_ability_casts` / `execute_item_mechanics`: the reflector recasts the spell on the original caster (`CastContext { caster: reflector, unit_target: original, reflected: true }`), running the mechanics and the `OnCast` / `OnUse` trigger only. It is not a cast: no `AbilityCastEvent`, no cooldown, no custom behavior, no cast reactions. `reflected` is carried by `TriggerContext` (inherited by derived contexts), `CastContext` and `ProjectilePayload`, so a reflected spell, including its projectile hit, is never reflected again and two reflectors cannot ping-pong.

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

### 6.12 Magnus

All four abilities are data rows. Q is the generated `AbilityId::MagnusShockwave` (display name "Shockwave"); Vanguard's hand-written `AbilityId::Shockwave` is untouched (the generated Q has its own id, so `generated::definition(AbilityId::Shockwave)` stays `None`). Plan and decisions: `docs/plans/2026-11-08-magnus-hero.md`.

New surface (each item is one approved gap, G1 to G10; nothing else was added):

| Gap | Surface | Where it lives |
|---|---|---|
| G1 | Melee classification up to range 250 (`scale::MELEE_CLASSIFICATION_MAX_RANGE`); HeroGenerator rejects an `is_melee` that disagrees with `attack_range` | `scale.rs`, `HeroGenerator.py` |
| G2 | Optional `turn_rate` hero column | `HeroGenerator.py` -> `hero_combat(turn_rate)` |
| G3 | `MAX_CAST_BACKSWING` 1.5 | `definition.rs`, `AbilityGenerator.py` |
| G4 | Timed stat `attack_damage_pct`: a snapshot of the target's base attack damage (`StatusBuildContext.base_attack_damage`) | Modifiers: `StatusSpec::Modifier`, `build_status_effect` |
| G5 | Timed stat `cleave` (a ratio fed into the status source's cleave instance in `CombatStats::cleaves`) | Modifiers: `StatusEffect.cleave` in `add_status_stat_delta` |
| G6 | Cone cleave: timed `cleave_radius`, `cleave_end_radius`, `cleave_distance` feed the same instance; a non-zero distance makes it a trapezoid | Modifiers + `basic_attack.rs` (`CleaveCone`) |
| G7 | `radius` on `movement,dash` / `charge` = contact radius (`ForcedMovement.contact_radius`) | `displacement.rs`, `AbilityEffect::Displace.contact_radius` |
| G8 | `amount` on `movement,pull_point` = forward offset; `EffectRunner` replaces the event's point with caster position + facing x offset | `effects.rs` (`forward_point`), `AbilityEffect::Displace.forward_offset` |
| G9 | `movement,drag_stun`: a drag that also applies a stun under `<owner>_stun`, bound to the drag through `ForcedMovement.bound_status` and removed when the carrying movement ends | `displacement.rs`, `AbilityEffect::Displace.bound_stun` |
| G10 | `projectile_origin_offset`: a skillshot starts that far in front of the caster | `mechanics.rs`, `AbilityMechanic::Projectile.origin_offset` |

**Shockwave** (point, skillshot, width 200, speed 1200, cast range 1200): the wave starts 48 units in front of Magnus and hits each enemy once. On each hit, `movement,pull_point` pulls the unit toward the trigger point, which for `on_projectile_hit` is the projectile's position (the wave's centre), by at most 150 over 0.2 s; a unit nearer the centre stops there. A 75% slow and the damage land in the same trigger.

```csv
magnus_shockwave,1,on_projectile_hit,trigger_unit,movement,pull_point,150,,0.2,,,,
```

**Empower** (unit target, ally or self): five timed `stat_modifier` rows with one duration. `attack_damage_pct` 16 to 64 of the target's base (hero-definition) damage, `cleave` 0.16 to 0.64, and the cone shape 150 / 360 / 650. The status ids are `empower_<stat>`, so a refresh replaces the old deltas exactly. Cleave only works on melee targets (`CombatStats::is_melee`).

**Cleave instances.** Cleave does not sum into one aggregate shape. `CombatStats::cleaves` (`CleaveSet`, a fixed-size `Copy` list of `CleaveInstance`) holds one instance per `CleaveSource`: all item passives share the `Items` instance (ratios add, default 300 circle), and every timed modifier status carrying a cleave field feeds the instance keyed by its owner (`StatusEffect.cleave_source`, set from the `EffectSource` in `build_status_effect`; the five Empower statuses form one instance). Fields sum per key; the instance is dropped when its last contributing status ends, and a re-cast refreshes it. On a melee hit `basic_attack.rs` runs each instance separately (own targets, own `DamageEvent::cleave(ratio x damage)`), so a unit inside two shapes takes both. No CSV or generator change.

**Skewer** (point, global cast range): the dash row carries the contact radius 145; `on_unit_contact` drags and stuns; `on_movement_end,affected_units` deals the damage and a 3.25 s slow. The stun id is `skewer_stun`: it behaves as a stun (`stunned`), but refresh is by id, so another source's stun is neither overwritten nor removed when the drag ends.

```csv
skewer,0,on_cast,caster,movement,dash,800|900|1000|1100|1200|1300|1400,,0.888889|1|1.111111|1.222222|1.333333|1.444444|1.555556,145,,,
skewer,1,on_unit_contact,trigger_unit,movement,drag_stun,,,,,,,
```

**Reverse Polarity** (no target, radius 430): one `pull_point` row on `units_in_radius` with `amount` 250 gathers every enemy at the point 250 in front of Magnus over the 0.05 s minimum; damage and stun are ordinary rows. Forced motion ignores unit collision, so the crowd stacks without teleport rejections and `resolve_unit_collisions` separates them afterwards.

Tests: `magnus_matches_spec`, `magnus_abilities_generate_expected_definitions`, `magnus_shockwave_pulls_toward_wave_center_and_slows`, `skewer_drags_stunned_enemy_and_hits_at_destination`, `reverse_polarity_pulls_damages_and_stuns_in_radius`, `empower_grants_percent_damage_and_cone_cleave_to_self_and_ally`, plus one set per gap.

### 6.13 Sniper

All four abilities are data rows (`shrapnel`, `headshot`, `take_aim`, `assassinate`; status ids `shrapnel_slow`, `headshot_slow`, `headshot_attack_speed_flat`, `assassinate_stun`). Plan and decisions: `docs/plans/2026-11-09-sniper-hero.md`.

New surface (G1 to G6; nothing else was added):

| Gap | Surface | Where it lives |
|---|---|---|
| G1 | `MAX_CAST_POINT` 2.0 and `MAX_CAST_BACKSWING` 2.0 | `definition.rs`, `AbilityGenerator.py` |
| G2 | `zone` effect type / `AbilityEffect::Zone`: a delayed, persistent area that fires a custom trigger per tick with the units inside | `abilities/zones.rs`, `effects.rs`, `generator_common.py` |
| G3 | `cast_range_buffer` column: a unit-targeted cast is cancelled free during the cast point if the target moves beyond `cast_range + buffer` | `actions/pipeline.rs` (`Activation.break_range`), `AbilityDefinition.cast_range_buffer` |
| G4 | `apply_status,mini_stun`: a stun under `<owner>_stun` (`StatusSpec::OwnedStun`) | `definition.rs`, `generator_common.py` |
| G5 | Generated slows carry `<owner>_slow` (`StatusSpec::Slow { id, percent }`) | `definition.rs`, `generator_common.py` |
| G6 | A slow is measured against base move speed on every apply (`StatusEffect.move_slow_fraction`) | `status.rs` (`apply_status`) |

**Shrapnel** (area target, cast range 1800, radius 400 to 550): one `zone` row on `on_cast` (centre = the aim, delay 1.2 s, duration 10 s, interval 1 s, radius from `aoe_radius`) feeds `on_shrapnel_tick`; the damage and the 1.1 s slow are ordinary `affected_units` rows on that trigger. Ticks land at 1.2, 2.2, ..., 10.2 s after the cast fires, so the total is 10 x the per-tick damage. The slow is re-applied on every tick (G5 and G6 keep it from compounding or colliding with Headshot's).

```csv
shrapnel,0,on_cast,caster,zone,on_shrapnel_tick,1,,10,,1.2,,
shrapnel,1,on_shrapnel_tick,affected_units,damage,,30|45|60|75|90|105|120,magical,,,,,
```

**Headshot** (passive): `on_attack_hit,caster,pseudo_proc,on_headshot,40` re-fires the attack context under `on_headshot`. Its damage keeps the attack metadata (attack bonus damage, so armor applies), the 100% slow and the -100 attack speed share one duration, and the 50-unit knockback over the 0.05 s minimum pushes away from Sniper.

**Take Aim** (passive): `stat_modifier,attack_range` with the total range minus the 550 base (235 to 1165). `base_attack_range()` excludes the bonus, so the projectile speed stays 3000.

**Assassinate** (unit target, range 3000, cast point 2.0, backswing 1.6, `cast_range_buffer` 600, homing projectile at 1500): damage and `mini_stun` (0.1 s, id `assassinate_stun`) on `on_projectile_hit`. The stun refreshes only itself, so a longer stun from another source is neither replaced nor shortened.

Tests: `sniper_matches_spec`, `sniper_abilities_generate_expected_definitions`, `shrapnel_zone_waits_then_ticks_ten_times`, `shrapnel_zone_slows_units_inside_and_releases_after_leaving`, `headshot_proc_damages_slows_and_knocks_back`, `headshot_and_shrapnel_slows_coexist`, `take_aim_adds_attack_range_per_rank_and_keeps_projectile_speed`, `assassinate_fires_after_two_second_aim_and_mini_stuns`, `assassinate_cancels_free_when_target_leaves_buffer`, `mini_stun_spares_a_longer_foreign_stun`, `item_and_ability_zone_emit_identical_ticks`, plus one set per gap.

### 6.14 Ursa

All four abilities are data rows (`earthshock`, `overpower`, `fury_swipes`, `enrage`; status ids `earthshock_slow`, `overpower_attack_speed_flat`, `fury_swipes_stacks`, `enrage_damage_reduction`, `enrage_status_resistance`). Plan and decisions: `docs/plans/2026-11-09-ursa-hero.md`.

New surface (G1 to G7; nothing else was added):

| Gap | Surface | Where it lives |
|---|---|---|
| G1 | A dash whose aim is on the mover goes the full distance along the mover's facing (every dash, `AbilityMechanic::Dash` included) | `displacement.rs` (`plan_motion`) |
| G2 | Attack charges: `amount` on a timed `stat_modifier` row; `StatusSpec::Modifier.attack_charges`, `StatusChange::ConsumeAttackCharge`, `status::consume_attack_charges` | `definition.rs`, `status.rs`, `generator_common.py` |
| G3 | `apply_status,stacks` (`StatusSpec::Stacks`, `StatusEffect.stacks`) and `scaling_type=target_stacks` (`EffectScaling::TargetStacks`) | `definition.rs`, `status.rs`, `effects.rs`, `generator_common.py` |
| G4 | `apply_status,damage_reduction` (`StatusSpec::DamageReduction`) | `status.rs`, `combat.rs` (`resolve_incoming_damage`) |
| G5 | `apply_status,status_resistance` (`StatusSpec::StatusResistance`) | `status.rs` (`apply_status_effect_events`) |
| G6 | `dispel,debuff` with `amount` 1 (strong dispel) no longer warns about `amount` | `generator_common.py` |
| G7 | A refused own dash or charge still fires `on_movement_end` at the mover's position | `displacement.rs` (`report_refused_run`) |

**Earthshock** (no target, radius 385, cast point 0): `movement,dash` 250 over 0.25 s on `on_cast` (G1: the aim is the caster, so it leaps along its facing), then damage and the 4 s `earthshock_slow` on `on_movement_end,units_in_radius`, centred on the landing point (`AreaCenter::TriggerPoint`). It stops short at terrain and shocks there; rooted or stunned, it shocks in place (G7).

```csv
earthshock,0,on_cast,caster,movement,dash,250,,0.25,,,,
earthshock,1,on_movement_end,units_in_radius,damage,,75|125|175|225|275|325|375,magical,,,,,
```

**Overpower**: `stat_modifier,attack_speed_flat` 400 for 20 s with `amount` 3 to 9 attack charges (G2). The charge is spent at attack release, hit or miss, never by extra attacks; a recast resets it.

**Fury Swipes** (passive): two `on_attack_hit,trigger_unit` rows. `apply_status,stacks` adds a stack (8 to 32 s, refreshed by every hit, undispellable) and `damage` physical with `target_stacks` offset 1 deals `value x (stacks + 1)`, so the first hit deals one stack's worth and the hit counts itself.

```csv
fury_swipes,0,on_attack_hit,trigger_unit,apply_status,stacks,,,8|12|16|20|24|28|32,,,,
fury_swipes,1,on_attack_hit,trigger_unit,damage,,12|20|28|36|44|52|60,physical,,,,target_stacks,1
```

**Enrage**: strong self dispel first (`dispel,debuff` with `amount` 1; rows run in `effect_index` order and status events apply in that order), then 80% damage reduction (G4) and 50% status resistance (G5) for 4 to 5.5 s.

Tests: `ursa_matches_spec`, `ursa_abilities_generate_expected_definitions`, `earthshock_leaps_forward_then_shocks_at_landing`, `earthshock_stops_at_terrain_and_shocks_there`, `rooted_earthshock_shocks_in_place`, `overpower_lasts_exactly_rank_attacks`, `fury_swipes_bonus_grows_per_hit_and_resets_after_expiry`, `fury_swipes_stacks_are_per_target`, `enrage_strong_dispels_then_reduces_damage_and_debuff_durations`, plus one set per gap.

### 6.15 Slark

All four abilities are data rows (`dark_pact`, `pounce`, `essence_shift`, `shadow_dance`; status ids `essence_shift_attack_damage`, `essence_shift_agility`, `untargetable`, `shadow_dance_move_speed_bonus`, `shadow_dance_health_regen`, leash id `leash`; trigger `on_dark_pact_pulse`). Plan and decisions: `docs/plans/2026-11-10-slark-hero.md`.

New surface (G1 to G7; nothing else was added):

| Gap | Surface | Where it lives |
|---|---|---|
| G1 | `attached_zone` effect type: `AbilityEffect::Zone.follow_caster`, re-centred on the caster before every tick, despawned when the caster dies | `zones.rs` (`tick_zones`), `definition.rs`, `effects.rs`, `generator_common.py` |
| G2 | `:independent` suffix on a timed `stat_modifier` id: `StatusEffect.independent`, `StatusSpec::Modifier` / `ResourceModifier.independent`; no same-id replacement | `status.rs` (`apply_status`), `definition.rs`, `generator_common.py` |
| G3 | Timed `strength` / `agility` / `intelligence` (`ResourceStat::Strength / Agility / Intelligence`), reconciled into `HeroAttributes` | `vitals.rs` (`sync_status_attributes`, `AppliedStatusAttributes`) |
| G4 | `apply_status,move_speed_bonus` (`StatusSpec::MoveSpeedBonus`, `StatusEffect.move_bonus_fraction`), measured against base speed like slow | `status.rs` (`apply_status`), `definition.rs` |
| G5 | `apply_status,untargetable` (`StatusSpec::Untargetable`): targeting only, via `StatusEffect.targeting_blocked` (amended: not cyclone's full flag) | `status.rs` (`rejects`), `definition.rs` |
| G6 | Optional `base_attack_speed` hero column (`CombatStats { base_attack_speed, ..hero_combat(..) }`) | `HeroGenerator.py` |
| G7 | HUD groups same-id statuses (`+id xN <longest>s`) | `ui.rs` (`buffs_summary`) |

**Dark Pact** (no target, radius 325): `attached_zone` with a 1.5 s delay, a 0.1 s interval and a 1 s duration gives ten pulses at 1.5 to 2.4 s, centred on Slark's current position each time. Per pulse, rows run in `effect_index` order: `dispel,debuff` on the caster (Basic, so stuns go too), then `resource,health_drain` on `affected_units` with a 0.15 transfer ratio (healing for a share of what was actually removed). The caster is never an affected unit, so there is no self-damage. The zone fires even with nobody inside, so the dispel always happens.

```csv
dark_pact,0,on_cast,caster,attached_zone,on_dark_pact_pulse,0.1,,1,,1.5,,
dark_pact,1,on_dark_pact_pulse,caster,dispel,debuff,,,,,,,
dark_pact,2,on_dark_pact_pulse,affected_units,resource,health_drain,7.5|15|22.5|30|37.5|45|52.5,magical,,,0.15,,
```

**Pounce**: `movement,dash` 700 over 0.75 s along the facing (Ursa G1) with a 120 contact radius (damage once per enemy on `on_unit_contact`), then `apply_status,leash` (radius 325, anchored at the landing point) on enemies within 120 of the landing point at `on_movement_end`. A refused dash reports its end in place (Ursa G7).

**Essence Shift** (passive): two `on_attack_hit` rows with independent instances (G2): a Debuff `attack_damage:independent` of -1 on the target and a Buff `agility:independent` of +3 (G3) on Slark, 10 s each, so every stack expires on its own. Agility feeds armor and attack speed only, because the engine has no primary-attribute damage.

**Shadow Dance**: three `on_cast` caster rows: `untargetable` (G5), `move_speed_bonus` 20 to 50% (G4) and `health_regen` 70 to 160, for 4 to 4.75 s. He cannot be attacked or picked by enemy unit-target spells, but area effects, dash contact and projectiles already in flight still hit him, and his own Dark Pact keeps pulsing.

Tests: `slark_matches_spec`, `slark_abilities_generate_expected_definitions`, the `dark_pact_*`, `pounce_*`, `essence_shift_*` and `shadow_dance_*` integration tests in `abilities/mod.rs`, plus one set per gap.

## 7. Checklist: adding a mechanic, effect or trigger

**New mechanic or gate**

1. Add the variant (`AbilityMechanic` or a gate `AbilityEffect`) in `definition.rs`.
2. Run it from `MechanicRunner::run` (or the gate arm of `EffectRunner::execute`). It decides who and when; it emits triggers or derived contexts, never Damage / Heal / Status events.
3. Take the source from `CastContext.source` / `TriggerContext.source`; never hard-code `EffectSource::Ability`. Make sure it works for items (`ItemDefinition::mechanics`).
4. Respect `MAX_TRIGGER_DEPTH` for anything that re-fires contexts; skip untargetable units.
5. Add the tooltip line (`tooltip.rs`; the `AbilityEffect` match is exhaustive, and `acts_on_targets` must say whether it is a spell hit: gates are not).
   - A gate builds its output with `ctx.derived(then)`, never a fresh `TriggerContext`, so `reaction`, `amount` and `status` are inherited (I10).
   - A gate that must outlive its trigger (`Zone`) writes a spawn message from `EffectRunner::execute` and a Triggers-layer module owns the entity; it still fires only `AbilityTriggerEvent`s. Register the message in `AbilitiesPlugin` and in every test app that runs `resolve_ability_triggers` (`abilities::tests::test_app`, the items test app).
6. Generators: add the `effect_type` / column validation in `generator_common.py`, add it to `warn_unreachable_custom_triggers` if it feeds a custom trigger, add examples in `data/examples/` and one row in `data/examples/invalid/`, add unittest cases.

**New effect**

1. Add the `AbilityEffect` variant and handle it in `EffectRunner::execute` only (no second executor). Reuse the shared events; choose damage kind with `DamageEvent::from_effect`, build statuses with `build_status_effect`.
2. Put the interaction rule in its owner (`combat.rs`, `status.rs`, `vitals.rs`, `displacement.rs`, `teleport.rs`) and add a row to the matrix in section 5.
3. Tooltip, generator validation, examples, tests (ability and item parity).
4. Under the reaction rule: stamp `.reactive(ctx.reaction)` on every Damage, ResourceDrain and Status message the effect emits, in `EffectRunner::execute` only; a custom effect (`custom.rs`) must do the same. Add the effect to the reaction table in section 5.

**New trigger**

1. Add the `AbilityTrigger` variant and its emitter. Every emitter builds the context with `TriggerContext::new` and carries an `EffectSource`.
2. Add it to `ABILITY_TRIGGERS` / `ITEM_TRIGGERS` in `generator_common.py` (reactive ones through `REACTIVE_PLAIN_TRIGGERS` / `REACTIVE_PARAM_TRIGGERS`); if nothing emits it yet, list it in `NOT_EMITTED_TRIGGERS` (now only `on_channel_tick` and `on_expire`) so both generators warn.
   - A dispatcher reads fact messages and writes only `AbilityTriggerEvent` (I11). Decide whether the trigger is a reaction (`is_reaction()`): reaction triggers must not be fed by reactive output, and the generator rejects `perform_attack` under them and under anything a `chain` or `zone` row feeds (`chain_fed_triggers`).
   - State its delay (same frame or next frame) in the table in section 3.
3. Add `trigger_tag` text in `tooltip.rs`, trigger/target combination rules in the generators, and a test that the emitter fires for both abilities and items.

**Always**: run `cargo fmt --check`, `cargo build`, `cargo test`, both generators with `--check`, `python3 -m unittest test_generator_common` from `scripts/`, regenerate and confirm no diff in `generated.rs` / `item_catalog.rs`, and `--check` on `data/examples/`.
