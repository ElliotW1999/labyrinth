# Merging reactive triggers (#58) into the mechanic composition layers

**Date:** 2026-11-08
**Branch:** `claude/tender-bardeen-3jur81` (HEAD `6222eef`, base `6d82702`). Merge `origin/main` (`7dc411d`, "Reactive and conditional triggers (#58)") with `git merge origin/main`. Do not rebase.
**Plans being merged:**
- `2026-11-08-mechanic-composition-layers.md` (this branch; the "composition plan").
- `2026-11-08-reactive-triggers.md` (origin/main; the "reactive plan").
- Permanent doc: `docs/architecture/composition.md`.

**Status:** approved by the coordinator with the defaults for U1–U4 (§12).

---

## 0. Summary

`git merge-tree --write-tree HEAD origin/main` reports 21 conflicted files. Every conflict is mechanical once three rules are fixed:

1. **Composition owns construction.**
   - `DamageEvent::from_effect` chooses the damage kind.
   - `build_status_effect` builds every status.
   - `status::wake_from_sleep` and `purge_strong_dispellable_debuffs` remove statuses.
   - The `Chain` gate replaces `bolt_chain` / `emit_chain_damage`.
2. **#58 owns the reaction metadata.** This covers `.reactive(...)`, `StatusEffect.reactive`, `StatusLifecycleEvent`, `DamageAppliedEvent` and `TriggerContext.amount/reaction/status`. It layers on top of the composition constructors and never replaces them.
3. **`Chain` is a fourth gate next to `Proc` / `Counter` / `Accumulate`.** It goes through `ctx.derived()`, so it inherits `reaction`, `amount` and `status` with no extra code.

There are also six semantic conflicts that Git does not flag (§3). One of them, the `while` status builder, breaks I1. Another, the `on_cooldown_ready` rule, wrongly rejects projectile items.

There is also one hazard that already exists on this branch (F7): chain hops clear `AttackMeta`, so an extra attack can loop through a chain. A generator rule fixes it.

## 1. Merge procedure

1. Run `git merge origin/main`. Resolve §2 and apply §3 inside the **merge commit**. The merge commit must build and pass `cargo test`, the generator unittests and every generator `--check`.
2. Add follow-up commits on the same branch and PR:
   - **C1:** layering fixes not needed to compile (F5, F7) plus the §9 tests.
   - **C2:** examples (§8).
   - **C3:** docs (§11).
3. Regenerate both catalogs in the merge commit (§10). Resolve `item_catalog.rs` by regenerating, not by hand.

## 2. Conflicted files: hunk-level resolution

"HEAD" means this branch and "theirs" means origin/main.

### Rust

**`src/abilities/custom.rs`**
- Take HEAD (`pub fn register(_app: &mut App) {}`) and drop theirs' `bolt_chain`.
- Add to the module doc: "Custom effects receive `ctx.reaction` and must stamp `.reactive(ctx.reaction)` on any damage or status they emit."

**`src/abilities/definition.rs`**
- Keep both `AbilityEffect` variants: HEAD's `Chain { jumps, radius, falloff, team, then }`, then theirs' `Accumulate { threshold, then }`.
- Update the `Accumulate` doc to say it is a gate under `MAX_TRIGGER_DEPTH`, like `Chain`.

**`src/abilities/effects.rs`**
1. Imports: `StatusChange, StatusEffect, StatusEffectEvent, StatusEffects, StatusKind, is_untargetable`.
2. `TriggerContext` fields: keep both, in the order `depth`, `magnitude` (add "independent of `amount`"), `amount`, `reaction`, `status`.
3. `TriggerContext::new`: `magnitude: 1.0, amount: 1.0, reaction: trigger.is_reaction(), status: None`.
4. `Damage` arm:
   ```rust
   let amount = amount * ctx.magnitude;
   self.damage.write(DamageEvent::from_effect(ctx.caster, target, amount, damage_type, ctx.attack.is_some()).reactive(ctx.reaction));
   ```
5. `ApplyStatus` arm:
   - Use HEAD's `build_status_effect(...)` and drop theirs' duplicate `dot_source` and heal-amp lines.
   - Then set `built.reactive = ctx.reaction` and write `StatusEffectEvent::new(source, target, StatusChange::Apply(built)).reactive(ctx.reaction)`.
6. Gate arms: keep HEAD's `Chain` arm unchanged, then add theirs' `Accumulate` arm. Update the doc comments that list gates (`depth`, `derived()`).

**`src/abilities/mod.rs`**
- In the SparkPendant test literal, union the fields: `magnitude: 1.0, amount: 1.0, reaction: false, status: None`.

**`src/abilities/passive.rs`**
- Build with `build_status_effect(status, rank, f32::INFINITY, Some(entity), point, Some(stats), attrs, healer_amp)`, then call `reconcile_held_status(...)`.
- Keep HEAD's query and header (`Option<&Transform>`, `point`, `healer_amp`) and drop theirs' `StatusBuildContext`.

**`src/abilities/tooltip.rs`**
- In the empty-rows arm, list both `Chain { .. }` and `Accumulate { .. }`.
- In the text arm, add both lines.

**`src/combat.rs`**
- Sleep break: call `let woken = crate::status::wake_from_sleep(st, &mut target_stats, attrs.agility, shield.as_deref_mut());`, then `report_removed(&mut lifecycle, event.target, event.source, event.reactive, StatusLifecycle::Removed, woken)`.
- `emit_chain_damage` stays deleted.

**`src/status.rs`**
- In `apply_status_effect_events`, set `let purged = purge_strong_dispellable_debuffs(...)`. Theirs' `report_removed(..., Dispelled, purged)` stays.
- `purge_strong_dispellable_debuffs` and `wake_from_sleep` return `Vec<RemovedStatus>`. `wake_from_sleep` returns `Vec::new()` when the unit is not asleep.
- `apply_debuff_immunity` (the test helper) ignores the returned list.

**`src/vitals.rs`**
- Burn: `DamageEvent::from_effect(source, event.target, amount, damage_type, event.from_attack).reactive(event.reactive)`.

**`src/item_catalog.rs`**
- Regenerate. The header imports are the union (`AbilityMechanic`, `Condition`, `StatusFilter`).

### Scripts

**`generator_common.py`**
- `ITEM_TRIGGERS`: keep HEAD's `on_projectile_hit` / `on_impact`, then add `**REACTIVE_PLAIN_TRIGGERS, **REACTIVE_PARAM_TRIGGERS`. Drop HEAD's explicit `on_damage_taken`, `on_kill` and `on_death` entries.
- `EFFECT_TYPES`: keep both `accumulate` and `chain`.
- `warn_unreachable_custom_triggers`: use `CUSTOM_TRIGGER_FEEDERS` (§3.4).
- `NOT_EMITTED_TRIGGERS` keeps only `on_channel_tick` and `on_expire`.
- Unreachable-trigger message: "no proc / counter / accumulate / chain row names it".

**`AbilityGenerator.py`**
- Union the imports.
- Call both `warn_unemitted_triggers` and `warn_duplicate_held_statuses`.

**`ItemGenerator.py`**
- Union the imports (`PROJECTILE_TYPES, REACTIVE_NO_TRIGGER_UNIT, REACTIVE_TRIGGER_NAMES, SKILLSHOT_TYPES`, `reaction_triggers, warn_duplicate_held_statuses, warn_unemitted_triggers`).
- Take theirs' blank-line change.
- Union the header import string.
- In `effect_context`, keep HEAD's projectile rules, then use theirs' loop that includes `*REACTIVE_TRIGGER_NAMES`.
- Call both warners.

**`test_generator_common.py`**
- Union the imports. Test-body changes are in §3.6.

### Data

- `abilities.csv`, `ability_effects.csv`, `item_effects.csv`, `invalid/ability_effects.csv`, `invalid/item_effects.csv`: keep both sides' rows.
- `items.csv`:
  - Take HEAD's 22-column header and its rows.
  - Append theirs' **ten** new rows (`thorn_mail` … `prd_basher`), each with two trailing empty fields.
  - Do not drop `thorn_mail`.

### README.md

- Item CSV bullets:
  - Bullet 1: take HEAD's.
  - Bullet 2: take theirs', with three edits: custom triggers are "fed by `proc` / `pseudo_proc` / `counter` / `accumulate` / `chain` rows"; `on_projectile_hit` / `on_impact` (projectile items only) are listed; `chain` is added to the new effect types.

## 3. Semantic conflicts (fix in the merge commit)

1. **`reactive_triggers.rs` `sync_conditional_statuses` breaks I1.** It builds `While` statuses with `status.build(...)`.
   - Replace that with `build_status_effect(status, rank, f32::INFINITY, Some(owner), owner_tf.translation, Some(stats), attrs, outgoing_heal_amp(Some(stats), Some(statuses)))`.
   - Delete `build_ctx` and the `StatusBuildContext` import.
   - Intended behaviour change: held statuses get `dot_source = owner` and heal amp.
2. **`AbilityEffect::acts_on_targets`.** Add `| Self::Chain { .. }`. A chain is a gate, not a spell hit.
3. **`StatusLifecycleEvent` doc.** The sleep break is now `status::wake_from_sleep`, called by `combat::apply_damage_events`.
4. **Generator.**
   - Add `CUSTOM_TRIGGER_FEEDERS = (*GATE_TYPES, "chain")`. `GATE_TYPES` is unchanged because it selects the shared gate columns.
   - Use the new tuple in `warn_unreachable_custom_triggers` and in `reaction_triggers` propagation.
5. **`ItemGenerator.py` `on_cooldown_ready`.** The "has an active" check must count projectile items: `if item.projectile == "none" and not any(e.trigger in ("on_use", "on_channel_end", "on_channel_interrupted") ...)`.
6. **Tests that break.**
   - `while_toggled_and_on_cast_build_the_same_status`: assert `a.reactive && !b.reactive`, then normalise before comparing.
   - `custom_chain_damage_under_a_reaction_is_reactive`: replace it with `chain_under_a_reaction_emits_reactive_hop_damage`.
   - `test_unemitted_item_trigger_still_warns`: replace it with `test_reactive_triggers_are_emitted_now`.

## 4. Where each #58 addition sits in the six layers

| #58 addition | Layer | Notes |
|---|---|---|
| `ActionCooldown::tick -> bool`, `CooldownReadyEvent` | Activation (fact) | Carries an `EffectSource` (I3, I7) |
| `AbilityCastEvent` as the `OnSpellCast` source | Activation → Triggers | Ability-only; item uses write no `AbilityCastEvent` |
| `Proc.pseudo`, PRD, `TriggerCounters { counts, prd, sums }` | Mechanics (gate state) | I6 |
| `AbilityEffect::Accumulate` | Mechanics (gate) | Fourth gate; emits no gameplay messages; not a spell hit |
| `MotionSource.reaction` | Mechanics | Carries the reaction flag through motion |
| New `AbilityTrigger` variants, `StatusFilter`, `Condition`, `is_reaction()`, `is_cast_phase()` | Triggers | `OnChannelTick` and `OnExpire` remain unemitted |
| `owner_sources`, `source_triggers`, `fan_out`, the thin `attack_triggers` | Triggers | The one owner fan-out |
| Dispatchers (`track_travel`, `proximity_triggers`, `interval_triggers`, `cooldown_triggers`, `cast_triggers`, `damage_triggers`, `status_triggers`) | Triggers | Read fact messages, write only `AbilityTriggerEvent` (I11) |
| `TriggerTracking` | Triggers (state) | – |
| `sync_conditional_statuses`, `reconcile_held_status` | Triggers → Effects builder → Modifiers | Same pattern as `WhileToggled`; uses `build_status_effect` |
| `TriggerContext.amount / reaction / status` | Triggers (context) | §5.4 |
| Reactive stamping, `PerformAttack` reaction guard | Effects | Only in the executor (I1) |
| `SpellHitEvent` | Effects (fact) | Ability source, depth 0, cast phase only |
| `DamageAppliedEvent`; reflection and burn reactive | Gameplay rules (fact) | I4 |
| `StatusLifecycleEvent`, `RemovedStatus`, `report_removed` | Gameplay rules (fact) | Written only by the status appliers and the sleep wake |
| `TeleportCompletedEvent` read by `track_travel` | Gameplay rules → Triggers | §5.10 |
| Generator `name:arg`, `REACTIVE_*`, `GATE_TYPES`, `pseudo_proc` / `accumulate`, `while`, `reaction_triggers`, `warn_duplicate_held_statuses` | Generators | Plus `CUSTOM_TRIGGER_FEEDERS` and the trimmed `NOT_EMITTED_TRIGGERS` |

## 5. Interaction rulings

1. **Chain and reaction.** Hops are `ctx.derived(then)`, so they inherit `reaction`. Under a reaction, hop damage and statuses are reactive. They never fire dealt, taken or status triggers, so there is no ping-pong. Kill, death and health-crossing triggers still fire. `PerformAttack` is inert in a hop.
2. **Chain, amount and status.** Hops inherit `amount` and `status` unchanged (U2).
3. **Chain, spell hit and damage dealt.**
   - Hops (depth ≥ 1) are not spell hits (U1).
   - Non-reactive hop damage fires `OnDamageDealt` / `OnDamageTaken` next frame, with post-mitigation `amount`.
   - Chained damage stays spell damage (D1).
4. **`magnitude` vs `amount`.** `magnitude` scales the output of Damage, DrainHealth and Heal; only `Chain` sets it below 1. `amount` sizes the triggering occurrence; only `Accumulate` reads it. Neither derives from the other.
5. **`from_effect` vs reactive stamping.** `from_effect` only chooses the damage kind. The Effects layer stamps `.reactive(ctx.reaction)`; burn stamps `event.reactive`; DrainHealth stays `spell(..).drained(..).reactive(..)` (D2).
6. **Sleep and the status lifecycle.**
   - `wake_from_sleep` returns the removed statuses. `apply_damage_events` reports them as `Removed`, attributed to `event.source`, and skips the report if the damage is reactive.
   - The purge is reported as `Dispelled`.
   - Waking requires `HeroAttributes` (R5; deferred, U4).
7. **Dispatcher placement.** `apply_damage_events` writes `DamageAppliedEvent`. `damage_triggers` reads it next frame, before `resolve_ability_triggers`. Health only changes in the rules appliers (I4).
8. **Items and spell triggers.** `resolve_item_uses` and `execute_item_mechanics` write no `AbilityCastEvent`, so items never fire `OnSpellCast`. Item projectiles carry `EffectSource::Item`, so they are never spell hits. Item damage still fires `OnDamageDealt` / `OnKill`.
9. **Owner fan-out.** `attack_triggers` stays a thin caller of `owner_sources`.
10. **Teleport immunity.**
    - A teleport rejected with `TeleportError::Immune` writes `TeleportRejectedEvent`, so the unit has not moved and no travel is counted.
    - Self and ally teleports complete, re-anchor the unit, and do not count for `OnMove`.
11. **`build_status_effect` and `reactive`.** The signature is unchanged; callers set `reactive` themselves: `EffectRunner` uses `ctx.reaction`, and `reconcile_held_status` sets `true`. The cyclone status stays rule-owned and is stamped from `MotionSource.reaction`.

## 6. Invariants after the merge

- **I1:** `EffectRunner::execute` is the only `AbilityEffect` → message executor. Every status (cast, toggle, `While`) is built by `build_status_effect`. Reactive stamping happens only there and in `reconcile_held_status`.
- **I2:** Mechanics and gates (`Proc`, `Counter`, `Accumulate`, `Chain`) never write Damage, Heal or Status events.
- **I3:** Every trigger emitter carries an `EffectSource`.
- **I4:** Health goes down only in:
  - `combat::apply_damage_events`, the only writer of `DamageAppliedEvent`;
  - `vitals::apply_resource_drain_events`, which floors health at 1 and fires no damage, kill or death triggers.

  Health goes up only in `vitals::apply_heal`. Statuses change only through `StatusEffectEvent`, `tick_status_effects` and `status::wake_from_sleep`, which are the only sources of `StatusLifecycleEvent`.
- **I5, I7–I9:** unchanged.
- **I6:** "Gates (`Proc`, `Counter`, `Accumulate`, `Chain`)…". `MAX_TRIGGER_DEPTH = 2`.
- **I10 (reaction rule):**
  - `reaction` is set at the root (`trigger.is_reaction()` or `MotionSource`) and inherited by every derived context.
  - Under a reaction, Damage, ResourceDrain and Status events are reactive.
  - Reactive changes never fire dealt, taken or status triggers. Kill, death and health-below do fire.
  - `PerformAttack` is inert under a reaction.
- **I11:** Reactive dispatchers read only fact messages and write only `AbilityTriggerEvent`. The one exception is `sync_conditional_statuses`, which writes held statuses the way toggle upkeep does.
- **I12:** `magnitude` scales effect output and `amount` sizes the occurrence; neither derives from the other.

## 7. Fixes to #58 code

| # | Violation | Fix |
|---|---|---|
| F1 | Third status builder in `sync_conditional_statuses` | Use `build_status_effect` |
| F2 | Inline sleep break in `apply_damage_events` | `wake_from_sleep` + `report_removed` |
| F3 | Inline debuff-immunity purge | `purge_strong_dispellable_debuffs` returns the removed list |
| F4 | Damage kind chosen inline | `from_effect(..).reactive(..)` |
| F5 | `acts_on_targets` doesn't know about `Chain` | Add `Chain` |
| F6 | `on_cooldown_ready` rejects projectile items | Add the projectile guard |
| F7 | `on_attack_hit` → `chain` → hop `perform_attack` can loop, because hops drop `AttackMeta` | Generator: reject `perform_attack` on any trigger fed by a `chain` row (`chain_fed_triggers`); the runtime is unchanged (U3, R4) |

## 8. Examples (C2)

`data/examples/items.csv`:

```csv
storm_mail,Storm Mail,armor,2600,1300,3,rare,false,,false,,Every 300 damage taken arcs lightning from the attacker to nearby enemies,,,,,,,,,,
```

`data/examples/item_effects.csv`:

```csv
storm_mail,0,passive,owner,stat_modifier,armor,6,,,,,,
storm_mail,1,on_damage_taken,owner,accumulate,on_overload,300,,,,,,
storm_mail,2,on_overload,trigger_unit,chain,on_storm_arc,2,,,450,75,,
storm_mail,3,on_overload,trigger_unit,damage,,80,magical,,,,,
storm_mail,4,on_storm_arc,trigger_unit,damage,,80,magical,,,,,
```

`data/examples/invalid/item_effects.csv`:

```csv
reactor,18,on_damage_taken,attacker,chain,on_arc,2,,,450,75,,
reactor,19,on_arc,trigger_unit,perform_attack,,70,,,,,,
reactor,20,on_attack_hit,trigger_unit,chain,on_bounce,2,,,450,75,,
reactor,21,on_bounce,trigger_unit,perform_attack,,70,,,,,,
```

Under a derived custom trigger, the attacker is `trigger_unit`.

## 9. Tests to add

**Rust, `abilities/mod.rs`:**
- `chain_under_a_reaction_emits_reactive_hop_damage`, extended with two holders over 10 frames to show there is no ping-pong.
- `chain_hop_damage_fires_damage_dealt_next_frame`
- `chain_hops_are_not_spell_hits`
- `chain_hops_inherit_amount_reaction_and_status`
- `accumulate_then_chain_under_damage_taken_resolves_within_depth_cap`
- `storm_mail_item_and_passive_ability_emit_identical_damage`
- `item_projectile_use_is_neither_spell_cast_nor_spell_hit`
- `while_status_is_built_like_a_toggle_status`
- `acts_on_targets_excludes_every_gate`
- `rejected_hostile_teleport_does_not_count_as_travel`
- `blink_does_not_count_as_travel`

**Rust, `combat.rs`:**
- `sleep_break_reports_removed_unless_reactive`

**Rust, `status.rs`:**
- Extend `sleep_wake_releases_stat_deltas` to check the returned ids.
- `debuff_immunity_purge_reports_dispelled`, if it is not already covered.

**Python:**
- `test_reactive_triggers_are_emitted_now`
- `test_not_emitted_triggers_are_channel_tick_and_expire`
- `test_chain_fed_by_reaction_is_a_reaction`
- `test_perform_attack_rejected_under_chain_fed_trigger`
- `test_cooldown_ready_valid_for_projectile_items`

## 10. Regeneration and verification

To regenerate:

1. Run both generators.
2. Run `cargo fmt`. The AbilityGenerator leaves `components.rs` unformatted on both branches (R6).

The merged `git diff -- src/abilities/generated.rs src/item_catalog.rs` should contain only header-import changes.

V:

```bash
cargo fmt --check
cargo build
cargo test
(cd scripts && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest test_generator_common)
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py --check
PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py --check
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py --check data/examples/abilities.csv data/examples/ability_effects.csv
PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py --check data/examples/items.csv data/examples/item_effects.csv
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py --check data/examples/invalid/abilities.csv data/examples/invalid/ability_effects.csv   # must exit 1
PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py --check data/examples/invalid/items.csv data/examples/invalid/item_effects.csv           # must exit 1
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py && PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py && cargo fmt && git diff --exit-code
```

Expected at the merge commit: `cargo test` reports about 456 tests (the architect's trial merge) and the generator unittests report 73.

## 11. Documentation updates (C3)

**`docs/architecture/composition.md`:**
- Add the reactive plan and this plan to the references.
- Layer tables: `CooldownReadyEvent`; the four gates; `reactive_triggers.rs` dispatchers and `TriggerTracking`; `SpellHitEvent` and reactive stamping; `DamageAppliedEvent` / `StatusLifecycleEvent`.
- Replace the rules of thumb with invariants I1–I12.
- Diagram: add the fact-message feedback loop, the new context fields, and the gate list.
- Frame order: the reactive dispatcher chain, plus a delay table (which triggers fire next frame and which fire the same frame).
- Rule matrix: add a "fires reactive triggers?" column and a per-event reaction-rule table.
- New worked example 6.10, Storm Mail. `on_damage_taken` → `Accumulate` → `Chain` → reactive magical damage, shown as an item and as a passive ability. Also add thorn_mail / thornskin parity and bloodstone `on_kill`.
- In 6.1, note that hops are not spell hits but do fire damage dealt.
- Checklist: how to add a new trigger, gate or effect under the reaction rule.

**`README.md`:**
- The merged item-trigger bullet.
- The unemitted-trigger warning now lists only `on_channel_tick` and `on_expire`.
- Status-lifecycle writers.
- Chain bullet: hops inherit the reaction, are not spell hits, and fire damage dealt.
- The Storm Mail recipe and the `trigger_unit` note.
- Remove the stale item `projectile_type` limitation and add the F7 rule.

**Plans:**
- Composition plan: add a "merged with #58" note.
- Reactive plan: add a header note and deviation 15.
- `2026-11-06-generator-shared-architecture.md`: note that `NOT_EMITTED_TRIGGERS` survives with two entries.

## 12. Decisions and risks

| # | Item | Decision |
|---|---|---|
| U1 | Chain hops as spell hits | **No** |
| U2 | Should hops reset `amount`? | **No**; they inherit it |
| U3 | F7 enforcement | **Generator error only** |
| U4 | Hero-only sleep wake | **Defer** (R5) |
| R1 | Damage reactions arrive one frame later | Documented |
| R2 | Held `While` statuses now get `dot_source` and heal amp | Intended; tested |
| R3 | Test apps must register the new systems and messages | Already true |
| R4 | The F7 loop is still possible for content written in Rust | No current content does this |
| R5 | Creeps without `HeroAttributes` never wake from sleep | Pre-existing; follow-up |
| R6 | The generator leaves `components.rs` unformatted | Pre-existing; run `cargo fmt` |
