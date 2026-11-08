# Reactive triggers: audit and implementation plan

**Date:** 2026-11-08
**Branch base:** `6d82702` (attack modifiers merged)
**Builds on:**
- `2026-11-06-shared-effects-pipeline.md`: one trigger bus (`AbilityTriggerEvent`) and one executor (`EffectRunner`).
- `2026-11-08-attack-modifiers.md`: the `attack_triggers` fan-out, the `Proc`/`Counter` gates, `TriggerCounters`, `MAX_TRIGGER_DEPTH`, `CombatRng::roll`, and the `PerformAttack` guard.
- `2026-11-08-healing-resource-mechanics.md`: `ResourceDrainEvent` and leech rules.
- `2026-11-06-status-effects-catalog.md`: the `StatusEffectEvent` single entry point.
- `2026-11-06-generator-shared-architecture.md`: the rule that warns on `NOT_EMITTED` triggers until the runtime emits them.

**Status:** Approved by the coordinator with the defaults in §8.

---

## Executive summary

**What exists today**
- Four triggers fully work: on attack, on attack hit, chance proc and counter proc.
- `OnDamageTaken`, `OnKill` and `OnDeath` are declared and accepted in item CSVs, but **nothing emits them**. `ItemGenerator.py:118-122` warns about this.
- `OnExpire` is declared and accepted for abilities, but nothing emits it either.
- Nothing in the codebase reacts to damage dealt, statuses, cooldowns, time, movement or proximity.

**Design: no new subsystem.** It extends the existing pieces:
1. **One shared owner fan-out helper**, taken out of `attack_triggers.rs`. It lists items in slot order (deduplicated by `ItemId`) and then learned abilities. A new `abilities/reactive_triggers.rs` holds small dispatcher systems. Each one reads one authoritative message and writes ordinary `AbilityTriggerEvent`s, which go into the existing `resolve_ability_triggers` FIFO.
2. **Three new authoritative messages** at the sites that already own the state change:
   - `DamageAppliedEvent` from `combat::apply_damage_events`
   - `StatusLifecycleEvent` from `status.rs`
   - `CooldownReadyEvent` from the two cooldown tick systems

   Plus `SpellHitEvent` from `resolve_ability_triggers`. Movement, proximity and interval state live in one new per-unit component, `TriggerTracking`, required by `CombatStats` like `TriggerCounters`.
3. **New `AbilityTrigger` variants.** Parameterised ones carry integer fields, so the enum keeps `Copy + Eq + Hash`: `OnHealthBelow { percent }`, `OnInterval { millis }`, `OnEnemyEnter/Leave { radius }`, `OnStatusApplied/Removed(StatusFilter)`, `While(Condition)`. In CSV they are written as `trigger:arg`.
4. **Gate changes:**
   - `Proc` gains `pseudo: bool` (PRD, state kept in `TriggerCounters`).
   - New `Accumulate { threshold, then }` gate. It sums `ctx.amount` (damage, distance, or 1). This covers "threshold reached" and "distance traveled".
5. **Conditional bonus** = `While(Condition)` entries holding `ApplyStatus`. They are reconciled exactly like `WhileToggled` (`passive.rs:101-170`), using a shared helper.
6. **One recursion rule:** `TriggerContext.reaction`. Effects resolved under damage-class or status-class triggers emit `reactive` damage and status events. Those never fire damage or status triggers again. Kill, death and the health crossing still fire because each is a bounded edge. `PerformAttack` does nothing under a reaction.

---

## 1. Audit

Legend: **Exists**, **Partial** (declared, or works only for the casting source), **Missing**.

| Trigger | Status | Evidence | Gap |
|---|---|---|---|
| On attack (attack begins) | **Exists** | `OnAttack` built from `BasicAttackReleaseEvent`, `src/abilities/attack_triggers.rs:31-39`. Fan-out to items and learned abilities, `:71-91`. CSV `scripts/generator_common.py:65,74`. Tests `src/abilities/mod.rs:1191`, `:1287` | Fires at release, not at windup start (by design, attack plan §2.1) |
| On attack hit | **Exists** | `OnAttackHit` built from `BasicAttackLandedEvent`, `attack_triggers.rs:40-46`. Event written at `src/basic_attack.rs:592` after the hit roll | — |
| On damage dealt | **Missing** | No variant in `src/abilities/definition.rs:111-148`. `src/combat.rs:342-475` (`apply_damage_events`, the only place health goes down, `:423`) emits nothing | Needs an emission site and a recursion guard |
| On damage taken | **Partial** | Variant `definition.rs:143`. Tooltip `src/abilities/tooltip.rs:204`. Item CSV trigger `generator_common.py:78`. `attacker` target maps to `TriggerUnit`, `generator_common.py:1430-1431` | Never written anywhere in `src/`. `ItemGenerator.py:118-122` warns "not emitted". Missing from `ABILITY_TRIGGERS` (`generator_common.py:53-67`) |
| On kill | **Partial** | Variant `definition.rs:144`, item CSV `generator_common.py:79` | Not emitted. `despawn_dead` (`combat.rs:825-868`) has no killer: bounty goes to every enemy hero |
| On death | **Partial** | Variant `definition.rs:145`, item CSV `generator_common.py:80` | Not emitted. Heroes never despawn (`Without<PlayerHero>`, `combat.rs:836`). Creeps despawn the same frame they die |
| On spell cast | **Partial** | `OnCast` fires only for the cast ability's own entries (`CastContext::trigger` uses `EffectSource::Ability(self.ability)`, `src/abilities/mechanics.rs:45-58,209,248`). `AbilityCastEvent` `src/abilities/casting.rs:24-32,79-98` | No owner-wide "whenever you cast" trigger for other sources |
| On spell hit | **Partial** | `OnProjectileHit` only for the projectile's own ability (`mechanics.rs:260-295`) | No owner-wide trigger. Non-projectile spells report nothing |
| On low health | **Partial** | Only as a magnitude filter: `EffectScaling::TargetHealthBelow` (`definition.rs:206-207`, `src/abilities/effects.rs:351-356`) | No edge trigger and no rearm |
| On movement | **Partial** | `OnUnitContact` / `OnMovementEnd` for forced movement only (`definition.rs:115-121`, `src/displacement.rs:649,671`) | Nothing for ordinary walking |
| On distance traveled | **Missing** | — | No position history |
| On proximity (enter/leave) | **Missing** | Auras listed as missing in `2026-11-06-status-effects-catalog.md:75` | — |
| On status applied | **Missing** | `apply_status_effect_events` `src/status.rs:801-884` emits nothing | — |
| On status removed | **Missing** | `OnExpire` declared (`definition.rs:130`, `generator_common.py:62`) but never written. Removal sites: expiry `status.rs:650-655`, dispel/remove/immunity purge `status.rs:784-799,820-835,864-881`, sleep break `combat.rs:383-396` | Six removal sites, no shared notification |
| On cooldown completed | **Missing** | `ActionCooldown::tick` `src/actions/resources.rs:46-48`. Ticked at `src/abilities/mod.rs:165-172` and `src/items.rs:1086-1093` | Nothing detects the moment a cooldown reaches zero |
| On interval | **Missing** | (`OnChannelTick` declared but not emitted, README "Limitations") | — |
| On threshold reached | **Partial** | `Counter` gate counts executions (`effects.rs:626-638`, `TriggerCounters` `:43-66`, required by `CombatStats` `src/components.rs:111`) | No value accumulator (damage taken, distance) |
| Chance proc | **Exists** | `AbilityEffect::Proc` `definition.rs:644-647`, `effects.rs:621-625`. `CombatRng::roll` `src/combat.rs:254-262`. CSV `generator_common.py:1355-1362`. Tests `mod.rs:1394,1409` | Only as good as the triggers that feed it |
| Pseudo-random proc (PRD) | **Missing** | Explicitly deferred: `2026-11-08-attack-modifiers.md:271` ("Plain random, no PRD") | — |
| Counter-based proc | **Exists** | `Counter` gate; tests `mod.rs:1442,1458`; CSV `generator_common.py:1363-1368` | — |
| Conditional bonus | **Partial** | The reconcile pattern exists for toggles: `tick_toggle_upkeep` applies and removes permanent undispellable statuses (`src/abilities/passive.rs:101-170`) | Only "toggled" can be the condition |

---

## 2. Design

### 2.1 Frame flow after the change

```text
abilities chain (after SimSet::Commands, before apply_damage_events):
  tick_ability_cooldowns / tick_item_cooldowns  ── CooldownReadyEvent (NEW, same frame)
  tick_toggle_upkeep, sync_ability_passives
  ... emit_ability_casts (AbilityCastEvent) ... execute_ability_casts, projectile_hit_triggers
  attack_triggers                                (now uses owner_sources helper)
  (tick_forced_movement, settle_grounded_units)
  reactive_triggers::* (NEW, chained, in this order):
     track_travel          → OnMove (amount = distance); updates TriggerTracking.moved
     proximity_triggers    → OnEnemyEnter/Leave{radius}
     interval_triggers     → OnInterval{millis}
     cooldown_triggers     ← CooldownReadyEvent         → OnCooldownReady (that source only)
     cast_triggers         ← AbilityCastEvent (this frame), SpellHitEvent (prev frame)
                                                        → OnSpellCast / OnSpellHit
     damage_triggers       ← DamageAppliedEvent (prev frame)
                                                        → OnDamageDealt/Taken, OnHealthBelow{p}, OnKill, OnDeath
     status_triggers       ← StatusLifecycleEvent       → OnStatusApplied/Removed(filter)
     sync_conditional_statuses (While(Condition) reconcile → StatusEffectEvent)
  resolve_ability_triggers   (FIFO, gates; writes SpellHitEvent NEW)
  ... apply_status_effect_events (writes StatusLifecycleEvent) ...
combat chain: ... apply_damage_events (writes DamageAppliedEvent NEW, after each event) → despawn_dead
```

**Delays**
- Damage, kill, death, low-health, status-apply and spell-hit reactions resolve **one frame later**. This matches the `PerformAttack` delay.
- Cast, attack, cooldown, interval, move and proximity reactions resolve in the **same frame**.
- Status expiry is pinned before `status_triggers` (see §7).

### 2.2 Shared fan-out (attack_triggers refactor)

```rust
// abilities/attack_triggers.rs → keep file, add helper (or move to reactive_triggers.rs and re-export)
/// Every effect source the owner holds: items in slot order (one per distinct ItemId),
/// then learned ability slots (rank > 0). Items use rank 1.
pub(crate) fn owner_sources(inventory: Option<&Inventory>, loadout: Option<&AbilityLoadout>)
    -> impl Iterator<Item = (EffectSource, u32)>;
/// Distinct triggers of `source` matching `pred` (used by the parameterised triggers).
pub(crate) fn source_triggers(source: EffectSource, rank: u32, defs: &AbilityDefinitions,
    pred: impl Fn(AbilityTrigger) -> bool) -> SmallVec<[AbilityTrigger; 4]>;
```

`attack_triggers` becomes a thin caller with the same behaviour. Every dispatcher builds contexts with `TriggerContext::new` and fills them through the chainable setters.

### 2.3 New / changed types

**`definition.rs`**
```rust
pub enum AbilityTrigger {             // existing derives: Copy, Eq, Hash
    ...,
    OnDamageTaken, OnKill, OnDeath,   // now emitted
    OnDamageDealt,
    OnHealthBelow { percent: u8 },    // edge: this hit took owner from >= p% to < p%, still alive
    OnSpellCast,                      // owner cast an ability (fan-out excludes the cast ability)
    OnSpellHit,                       // owner's ability applied an effect to an enemy
    OnStatusApplied(StatusFilter), OnStatusRemoved(StatusFilter),   // holder side
    OnCooldownReady,                  // this source's cooldown reached 0
    OnInterval { millis: u32 },
    OnMove,                           // amount = distance this frame; rows must be `accumulate`
    OnEnemyEnter { radius: u32 }, OnEnemyLeave { radius: u32 },
    While(Condition),                 // held ApplyStatus entries, reconciled like WhileToggled
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusFilter { Any, Buff, Debuff, Id(&'static str) }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Condition {
    HealthBelow { percent: u8 }, Moving, Stationary,
    EnemiesNear { radius: u32, at_least: u8 }, AlliesNear { radius: u32, at_least: u8 },
    HasStatus(&'static str),
}
impl AbilityTrigger {
    /// Damage/status-caused: contexts get `reaction = true`.
    pub fn is_reaction(self) -> bool; // DamageDealt/Taken, HealthBelow, Kill, Death, StatusApplied/Removed
    /// Cast-originated (counted for OnSpellHit at depth 0): OnCast, OnProjectileHit, OnImpact,
    /// OnUnitContact, OnMovementEnd, OnChannelTick/End/Interrupted, Custom(_).
    pub fn is_cast_phase(self) -> bool;
}
AbilityEffect::Proc { chance, then, pseudo: bool }          // +field (2 test sites, 1 generator site)
AbilityEffect::Accumulate { threshold: RankValue, then: AbilityTrigger }
```

`OnExpire` stays declared and unemitted. It is out of scope; `OnStatusRemoved` covers the reactive use.

**`effects.rs` (`TriggerContext`)**
- `amount: f32`, default `1.0`. Damage triggers set it to post-mitigation damage, `OnMove` to distance, `OnAttackHit` to landed damage.
- `reaction: bool`, inherited by `derived()`.
- `status: Option<StatusMeta { id, kind }>`.
- Setters `.amount()`, `.reaction()`, `.status()`.

**`TriggerCounters`** gains two lists:
- `prd: Vec<(Key, u32)>`: failures in a row.
- `sums: Vec<(Key, f32)>`.
- `Key = (EffectSource, AbilityTrigger)`, keyed by `then`, the same as `Counter`.

**New `TriggerTracking` component**
- Required by `CombatStats`, in `effects.rs` or `reactive_triggers.rs`.
- Fields: `last_pos: Option<Vec3>`, `moved: f32`, `intervals: Vec<(Key, f32)>`, `proximity: Vec<(Key, Vec<Entity>)>` (sorted by `Entity`), `held: Vec<(EffectSource, &'static str)>`.
- Entries for sources the owner no longer holds are pruned every frame.

**New messages**
```rust
// combat.rs
pub struct DamageAppliedEvent { source: Option<Entity>, target: Entity, kind: DamageKind, is_spell: bool,
    reactive: bool, amount: f32 /* health removed + shield absorbed */, health_before: f32,
    health_after: f32, max_health: f32, target_pos: Vec3 }
// status.rs
pub enum StatusLifecycle { Applied, Expired, Dispelled, Removed }
pub struct StatusLifecycleEvent { target, source: Option<Entity>, id: &'static str, kind: StatusKind, change: StatusLifecycle }
// actions/ or abilities/
pub struct CooldownReadyEvent { owner: Entity, source: EffectSource }
pub struct SpellHitEvent { caster: Entity, team: Team, ability: AbilityId, target: Entity, point: Vec3 }
```

**Reactive flag on events**
- `DamageEvent.reactive: bool`, set in constructors, `.reactive(b)` setter. Reflected damage is always `reactive`.
- `ResourceDrainEvent.reactive` is passed on to burn damage.
- `StatusEffectEvent.reactive`. Add `StatusEffectEvent::new(source, target, change)` and update the 19 literals.
- `StatusEffect.reactive`, copied on apply, so a reactive status also expires silently.

### 2.4 Emission rules per trigger (authoritative sites)

| Trigger | Site | Fan-out | Context |
|---|---|---|---|
| DamageDealt | `DamageAppliedEvent` | source owner | trigger_unit = victim, point = victim pos, amount. Skip if `reactive`, `kind == Dot`, `amount <= 0`, or source is `None` / equals target |
| DamageTaken | same | target | trigger_unit = source (`attacker`). Same skips, except source may be `None` |
| HealthBelow{p} | same | target, each distinct `p` held | fires iff `before/max >= p% > after/max` and `after > 0`. Ignores `reactive` and kind. Rearm = health back above p% |
| Kill / Death | same, `before > 0 && after <= 0` | source owner (Kill, source != target) / target (Death) | trigger_unit = victim / killer, point = `target_pos`. Ignores `reactive` and kind |
| SpellCast | `AbilityCastEvent` | caster, all sources except `Ability(cast.ability)` | cast_target / trigger_unit = unit target, point = aim |
| SpellHit | `resolve_ability_triggers`: one per (ctx, distinct enemy target) for `source: Ability`, `depth == 0`, `trigger.is_cast_phase()`, once at least one entry resolved that enemy | caster | trigger_unit = hit unit |
| StatusApplied/Removed(filter) | `StatusLifecycleEvent`, not written when the event or the status is `reactive` | holder, each matching filter | trigger_unit = event source if != holder; `ctx.status` set |
| CooldownReady | `ActionCooldown::tick` returns `became_ready` (`remaining > 0` before, `<= 0` after); tick systems write the event | that source only | owner pos |
| Interval{ms} | `TriggerTracking.intervals` += dt while owner alive; fire once per frame when `>= period`, subtract period | that source | — |
| Move | `track_travel`: flat distance since `last_pos`; reset on `TeleportCompletedEvent` for that unit; fire if > ε | sources with `OnMove` entries | amount = distance |
| EnemyEnter/Leave{r} | per (source, r) set of living, targetable enemies within r (`area_contains`); diff sorted by `Entity` | that source | trigger_unit = unit. Dead or despawned members are dropped silently |
| While(cond) | `sync_conditional_statuses` | sources with `While` entries | wanted = owner alive && `eval(cond)`; apply/remove via the shared `reconcile_held_status` helper (also used by `tick_toggle_upkeep`); statuses permanent, undispellable, `reactive` |

### 2.5 Gates, PRD, determinism

- **PRD.**
  - `prd_constant(p) -> f32`: a pure bisection for C such that the expected proc rate equals p (p = 0.25 gives C ≈ 0.0847).
  - Each roll: `chance = (C × (failures + 1)).min(1)`, then `rng.roll(chance)`. A success resets `failures` to 0; a failure adds 1.
  - `failures` is stored per (owner, source, then).
  - The draw count matches `Proc`, and `roll` draws nothing at chance ≥ 1.
- **Accumulate.** `sum += ctx.amount.max(0)`. When `sum >= threshold` it fires once and keeps `sum %= threshold`. It respects `gate_open` / `MAX_TRIGGER_DEPTH`.
- **Determinism.**
  - All randomness uses `CombatRng` in FIFO queue order.
  - Dispatchers run in a fixed chain order. Within a system they go by message order, then owner query order, then the `owner_sources` order.
  - Proximity diffs are sorted by `Entity`.
  - Interval timers accumulate `dt` (no wall clock).

### 2.6 Recursion and source attribution (binding)

1. **Reaction contexts.** `ctx.reaction = trigger.is_reaction()` at the root. Contexts derived through `Proc`/`Counter`/`Accumulate` inherit it. Under a reaction context, `EffectRunner` stamps `reactive` on `DamageEvent`, `ResourceDrainEvent` and `StatusEffectEvent`. Reactive damage and statuses never produce DamageDealt, DamageTaken, StatusApplied or StatusRemoved.
2. **Bounded edges still fire for reactive causes:** Kill and Death (once per death) and HealthBelow (needs a heal to rearm).
3. **`PerformAttack`** does nothing when `ctx.reaction` (runtime guard). The generator also rejects `perform_attack` rows under reaction triggers.
4. **DoT ticks** (per-frame `DamageKind::Dot`) never fire DealtDamage or DamageTaken. They still count for Kill, Death and HealthBelow.
5. **Held (`While`/toggle) statuses** are `reactive`, so they never fire status triggers. The generator rejects `While(HasStatus(x))` holding status `x`.
6. **SpellHit** only counts contexts where `is_cast_phase()` holds at depth 0. Reactive and passive contexts never produce spell hits.
7. **Attribution.**
   - `DamageEvent.source` is the dealing owner. For DoT that is `dot_source`; for reflected damage it is the reflector.
   - The killer is the source of the damage event that crossed zero.
   - Status triggers belong to the holder; `trigger_unit` is the applier.
   - Custom effects get `ctx.reaction` and must pass it on (document this in `register_custom_effect`).

### 2.7 CSV schema and generators

**Trigger parameter syntax** in the existing `trigger` column: `name[:arg[:arg]]`.
- `parse_effect_rows` keeps `e.trigger` as the base name and adds `e.trigger_args`.
- `invalid_combo`, `center_for`, `owner_triggers` and the unreachable-custom warning stay keyed by the base name.
- `CUSTOM_TRIGGER_PATTERN` cannot contain `:`, so names never collide.

| CSV trigger | Rust | Rules |
|---|---|---|
| `on_damage_dealt`, `on_damage_taken`, `on_kill`, `on_death` | plain | Added to `ABILITY_TRIGGERS`. `NOT_EMITTED` deleted. `attacker` also maps to `TriggerUnit` for `on_death` |
| `on_health_below:30` | `OnHealthBelow { percent: 30 }` | integer 1..=99 |
| `on_spell_cast`, `on_spell_hit` | plain | — |
| `on_status_applied[:buff\|debuff\|<status id>]`, `on_status_removed[:…]` | `(StatusFilter::…)` | ids checked against `STATUSES` runtime ids or `stat_modifier` ids |
| `on_cooldown_ready` | plain | invalid for passive abilities and for items without `on_use` |
| `on_interval:5` | `OnInterval { millis: 5000 }` | seconds ≥ 0.1 |
| `on_move` | plain | every row must be `accumulate` |
| `on_enemy_enter:600`, `on_enemy_leave:600` | `{ radius: 600 }` | radius > 0 |
| `while:health_below:50`, `while:moving`, `while:stationary`, `while:enemies_near:600:2`, `while:allies_near:600:1`, `while:has_status:<id>` | `While(Condition::…)` | rows must be `apply_status` with blank duration, targeting `owner`/`caster` |

**New effect types**
- `pseudo_proc`: same columns as `proc`; emits `pseudo: true`.
- `accumulate`: `effect_id` = custom `on_<name>`; `value` = threshold > 0, with levels allowed.

Both feed the unreachable-custom-trigger check (`generator_common.py:661-674`).

**Other generator work**
- `AbilityGenerator.py:244-248`: reactive triggers stay valid for passives.
- Emitted Rust imports `StatusFilter` and `Condition`. Mind the generated-header import issue fixed in `44bc68c`.
- No new CSV columns. Production CSVs are unchanged. Examples go in `data/examples/` only.

---

## 3. Implementation steps (one commit each; every step compiles and passes tests)

Add the new `AbilityTrigger`/`AbilityEffect` arms to `tooltip.rs` (`trigger_tag` `:188-209`, effect rows `:362,453`) and to `items::item_effect_lines` in the same step that adds the variant.

1. **Fan-out helper.** Add `owner_sources` / `source_triggers`; `attack_triggers` uses them.
   - Tests: existing `attack_triggers` tests unchanged; `owner_sources_dedups_items_then_learned_abilities`.
2. **Context and reactive plumbing.** Add `TriggerContext.amount/reaction/status` and `.reactive` on `DamageEvent`, `ResourceDrainEvent`, `StatusEffectEvent`, `StatusEffect`. Runner stamping; `PerformAttack` reaction guard; `OnAttackHit` amount = landed damage.
   - Tests: `reaction_context_stamps_damage_status_and_drain_events`, `derived_context_inherits_reaction`, `perform_attack_is_inert_under_reaction`.
3. **Damage triggers.** `DamageAppliedEvent` in `apply_damage_events` (add a read-only `Query<&Transform>`); `damage_triggers` system; new variants `OnDamageDealt` and `OnHealthBelow`.
   - Tests (`abilities/mod.rs`, with a test app that registers `DamageAppliedEvent`): `damage_taken_fires_with_attacker_as_trigger_unit`, `damage_dealt_fires_for_source_owner_next_frame`, `reactive_damage_fires_no_damage_triggers` (two thorn items, no ping-pong over 10 frames), `dot_ticks_do_not_fire_damage_taken`, `fully_absorbed_by_invulnerability_fires_nothing`, `kill_and_death_fire_once_with_killer_attribution`, `dot_kill_credits_dot_source`, `health_below_fires_on_crossing_only_and_rearms_after_heal`, `lethal_hit_does_not_fire_health_below`.
   - Tests (`combat.rs`): `damage_applied_event_reports_shield_and_health`.
4. **Gates.** `Proc.pseudo`, `Accumulate`, `TriggerCounters.prd/sums`.
   - Tests: `prd_constant_matches_known_values` (0.25 → 0.0847 ± 1e-3), `prd_long_run_rate_matches_nominal` (seeded, 20k rolls, ±1%), `prd_failures_reset_on_success_and_are_per_source`, `accumulate_fires_at_threshold_and_keeps_remainder`, `accumulate_on_count_trigger_equals_counter`, `gates_respect_max_depth`.
5. **Cast triggers.** `OnSpellCast` from `AbilityCastEvent`; `SpellHitEvent` written by `resolve_ability_triggers`; `OnSpellHit`.
   - Tests: `spell_cast_fires_for_other_sources_not_the_cast_ability`, `item_use_is_not_a_spell_cast`, `spell_hit_once_per_enemy_per_cast_context`, `stun_only_spell_counts_as_hit`, `reactive_and_proc_contexts_never_spell_hit`.
6. **Status lifecycle.** `StatusLifecycleEvent` from apply, refresh, dispel, remove, immunity purge, expiry and sleep break (`remove_statuses` returns the removed list). `status_triggers` with filters. Pin `tick_status_effects.before(status_triggers)`.
   - Tests (`status.rs`): `lifecycle_events_for_each_removal_cause`, `rejected_debuff_emits_nothing`, `reactive_status_expires_silently`.
   - Tests (`abilities/mod.rs`): `status_applied_filter_debuff_ignores_buffs`, `status_removed_on_expiry_and_dispel`, `status_reaction_does_not_loop`.
7. **Cooldown and interval.** `ActionCooldown::tick -> bool`; `CooldownReadyEvent`; `TriggerTracking` (intervals); `OnInterval`.
   - Tests: `cooldown_ready_fires_once_when_reaching_zero`, `zero_cooldown_never_fires_ready`, `interval_fires_every_period_and_pauses_while_dead`, `two_copies_of_an_item_share_one_interval`, `sold_item_interval_is_pruned`.
8. **Movement and proximity.** `track_travel` (`OnMove`), `proximity_triggers`.
   - Tests: `distance_traveled_fires_every_n_units_via_accumulate`, `teleport_distance_not_counted`, `forced_movement_counts`, `enemy_enter_and_leave_fire_once_in_entity_order`, `units_in_range_at_acquire_fire_enter`, `dead_member_dropped_silently`, `allies_never_tracked`.
9. **Conditional bonus.** `While(Condition)`; `reconcile_held_status` shared with `tick_toggle_upkeep`; `sync_conditional_statuses`.
   - Tests: existing toggle tests pass; `low_health_bonus_applies_below_and_reverts_above_exactly`, `moving_and_stationary_conditions`, `enemies_near_counts_only_living_targetable_enemies`, `held_status_removed_when_item_sold`, `held_status_is_undispellable_and_fires_no_status_triggers`.
10. **Generators.** Trigger parameter syntax, new triggers and effect types, new rules, `NOT_EMITTED` removed, imports. Then regenerate both catalogs in the same commit.
    - Tests (`scripts/test_generator_common.py`): `test_trigger_args_parsed_and_emitted` (each parameterised trigger), `test_trigger_arg_ranges`, `test_damage_kill_death_triggers_accepted_for_abilities`, `test_on_move_requires_accumulate`, `test_while_requires_permanent_owner_apply_status`, `test_while_has_status_self_reference_rejected`, `test_cooldown_ready_invalid_for_passive_and_non_use_items`, `test_perform_attack_rejected_under_reaction_triggers`, `test_pseudo_proc_and_accumulate_columns`, `test_accumulate_feeds_custom_trigger`.
11. **Examples and docs (§4).**
    - Examples in `data/examples/`: `thorn_mail` (`on_damage_taken`), `bloodstone` (`on_kill`), `aeon_charm` (`on_health_below:25`), `arcane_echo` (`on_spell_cast`), `pulse_core` (`on_interval:3`), `phase_treads` (`on_move` accumulate 1000), `sentinel_ward` (`on_enemy_enter:600`), `berserker_blood` (`while:health_below:50`), `prd_basher` (`pseudo_proc`).
    - Each example needs an invalid counterpart under `data/examples/invalid`.

**Commands (each step)**
```bash
cargo fmt --check && cargo build && cargo test
python3 scripts/test_generator_common.py            # 47 tests pass today
python3 scripts/AbilityGenerator.py --check
python3 scripts/ItemGenerator.py --check
```

---

## 4. Docs to update

**`README.md`**
- "Ability architecture": add reactive triggers, the reaction rule and the frame-delay table, plus the new gates.
- "Attack modifiers": recursion guards gain the reaction rule; the PRD note replaces "plain random".
- "Adding abilities / Adding items": trigger argument syntax, new triggers and effect types, a recipe table.
- "Status effects": lifecycle events, and held statuses are silent.
- "Limitations": remove the `on_damage_taken` / `on_kill` / `on_death` sentence.

**Plans**
- `docs/plans/2026-11-06-generator-shared-architecture.md`: the runtime-blockers row for these triggers is resolved.
- `docs/plans/2026-11-08-attack-modifiers.md`: decision 5 (no PRD) is superseded by this plan.

## 5. Invariants

1. Health only goes down in `apply_damage_events`, which is the only writer of `DamageAppliedEvent`.
2. Statuses change only through `StatusEffectEvent` and `tick_status_effects`, which with the sleep break are the only writers of `StatusLifecycleEvent`.
3. Every reactive trigger goes through `AbilityTriggerEvent` and `resolve_ability_triggers`. No dispatcher applies an effect itself.
4. A reaction never re-fires damage or status triggers. Kill and death fire exactly once per death.
5. One trigger per distinct item and per learned ability, per occurrence.
6. All randomness uses `CombatRng` in a fixed order.
7. Held statuses undo exactly and never outlive their source.

## 6. Tests summary

Listed per step in §3. The key regressions to protect: the no-ping-pong thorn test, the PRD long-run rate, and the low-health crossing with rearm.

## 7. Risks

- **One-frame delay** for damage, status and spell-hit reactions. "Proc on death to prevent death" (Aeon-style) cannot cancel the lethal hit. Out of scope.
- **Schedule ambiguity:** `tick_status_effects` is not ordered against the abilities chain today. Pin it before `status_triggers` (no cycle, because it is already before `sync_status_resource_caps`). Verify there is no behaviour drift in the status tests.
- **Literal churn:**
  - The `reactive` field touches 19 `StatusEffectEvent` literals, ~31 `StatusEffect` literals (most use `..StatusEffect::buff()`), 3 test `DamageEvent` literals, and the `ResourceDrainEvent` literals.
  - Hand-built test apps must register `DamageAppliedEvent`, `StatusLifecycleEvent`, `CooldownReadyEvent` and `SpellHitEvent`.
- **`EffectRunner` param conflicts** when `SpellHitEvent` and `MessageWriter` are added. Write `SpellHitEvent` from `resolve_ability_triggers` itself, not from the runner.
- **Conflicting held statuses:** two sources holding the same status id will fight. The generator should warn on duplicate `While` status ids within one owner, and the README should tell authors to use unique ids.
- **Creeps despawn the frame they die.** `OnKill` `trigger_unit` may be invalid next frame. Effects should target the point or `units_in_radius`.
- **Cost of the per-frame scans** (proximity, conditions): owners × ≤10 sources × units. Restrict dispatchers to owners that have an `Inventory` or `AbilityLoadout`.
- **Custom effects** must pass on `ctx.reaction` or they can bypass the guard.

## 8. Decisions (approved defaults)

1. Spell cast = ability casts only, toggles included. Item uses do not count. The cast ability's own sources are excluded from the `OnSpellCast` fan-out.
2. Spell hit = an effect resolved onto an enemy from a cast-phase context, not damage-based. One per (context, enemy).
3. DoT ticks do not fire damage dealt or taken. They do count for kill, death and low health.
4. Low health fires only if the unit survives the hit. Rearm is crossing-based (stateless), not a latch.
5. Proximity tracks living, targetable enemies only. Death is a silent drop.
6. `OnMove` is only usable through `accumulate`. Teleports do not count; forced movement does.
7. Status triggers are holder-side only. Applier-side triggers ("when I debuff someone") are a follow-up.
8. Interval timers are per distinct item / ability (copies share one), pause while the owner is dead, and fire at most once per frame.
9. Out of scope:
   - An internal-cooldown gate for reactive passives. It is the natural next step: a `Cooldown` gate using the source's `ActionCooldown`.
   - Aura statuses applied to others.
   - Cooldown-ready on charge restore.
   - `OnExpire` and `OnChannelTick` emission.
