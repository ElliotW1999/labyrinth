# Activation & targeting mechanics — architecture audit & plan

**Date:** 2026-11-06  
**Scope:** Audit only; no production code in this pass.  
**Trace roots:** `src/actions/`, `src/abilities/casting.rs`, `src/abilities/definition.rs`, `src/items.rs`, `scripts/AbilityGenerator.py`, `scripts/ItemGenerator.py`, `scripts/generator_common.py`, `data/*.csv`, `README.md` (Limitations + Abilities sections).

---

## Executive conclusion

The shared **`actions/` activation pipeline** already defines most MOBA-style **target types**, **team filtering**, **range**, **cast point**, **backswing**, **channel**, **mana cost**, **cooldown**, and **ability charges** in one place (`ActionSpec` → `validate_activation_requests` → `tick_activations`). Abilities and active items both route through it; data is authored via CSV generators with validation rules that mirror Rust enums.

**Gaps** cluster into four buckets: (1) **data vs Rust** — `TargetType::Area` exists in code but CSV only derives it from `point` + `aoe_radius` + no projectile; (2) **behavior kinds** — toggle/passive ability *behaviors* are typed but not fully simulated (Overcharge, Stone Skin); (3) **filtering & costs** — no hero/creep/building unit classes, no health/gold costs, no cooldown reduction or reset; (4) **product features** — **autocast** is undefined everywhere.

**Recommendation:** Keep extending `actions/` + `AbilityDefinition` / `ItemActivation` rather than parallel systems. Minimal follow-up is mostly **docs + small enums/hooks**, not a rewrite.

---

## Architecture map (current)

```text
Hotkey / command queue → ActivationRequest { caster, ActionSource, CastTarget }
  → ActionSources::spec (abilities/casting.rs | items.rs item_action_spec)
  → check_ready (cooldown/charges, ActionCost mana, DisableRule)
  → resolve_target + in_range (actions/targeting.rs)
  → [out of range] QueuedActivation + MoveTarget → re-request
  → [in range] Activation: turn → cast point → pay + commit → ActionFiredEvent
  → channel (optional) → ActionChannelEndEvent (abilities: OnChannelEnd)
  → backswing → remove Activation
  → AbilityCastEvent / item OnUse → mechanics → AbilityTriggerEvent → effects
```

| Layer | Responsibility | Key files |
|-------|----------------|-----------|
| Shared activation | Target validation, timing, costs, interruption | `src/actions/mod.rs`, `pipeline.rs`, `targeting.rs`, `resources.rs` |
| Ability adapter | Rank-scaled spec, toggle flip, charge init/restore | `src/abilities/casting.rs`, `catalog.rs` (`restore_ability_charges`) |
| Item adapter | Per-instance cooldown/charges, stun-only disable, zero backswing | `src/items.rs` (`item_action_spec`, `commit_item`) |
| Definitions | Static targeting + timing fields | `src/abilities/definition.rs`, generated/catalog |
| UI targeting | Pending cast, indicators, pick filter | `src/abilities/mod.rs`, `src/picking.rs` |
| Data | CSV → Rust | `scripts/AbilityGenerator.py`, `ItemGenerator.py`, `generator_common.py` |

---

## Mechanic matrix

Status key: **Implemented** = wired end-to-end for at least abilities or items; **Partial** = typed/data present but incomplete semantics or docs; **Missing** = no type, hook, or emission.

| Mechanic | Status | File / data refs | Recommended work (minimal diff) |
|----------|--------|------------------|--------------------------------|
| **Unit target** | Implemented | `TargetType::Unit` (`actions/targeting.rs`); CSV `target_type=unit`; `resolve_target` unit branch; UI `unit_only` (`abilities/mod.rs`); tests `casting.rs`, `targeting.rs` | None. Document that hovered units on point/area casts still set aim (`CastTarget::Unit` → aim at unit position). |
| **Point target** | Implemented | `TargetType::Point`; generator maps plain `point` or skillshot path; trajectory indicator when `point_target` | None. |
| **Direction target** | Implemented | `TargetType::Direction` projects aim to `cast_range`; `ignores_range()`; skillshot CSV requires `point` or `direction` | None. Optionally add `direction` to item generator docs/examples if missing from `items.csv`. |
| **No target** | Implemented | `TargetType::NoTarget`; instant hotkey path (`is_instant()`); shockwave mechanic when `no_target` + AoE (generator) | None. |
| **Self target** | Implemented | `TargetType::SelfTarget` → `CastTarget::Unit(caster)`; CSV `self` + `target_team self` → `TargetTeam::SelfOnly` | None. |
| **Area target** | Partial | **Rust:** `TargetType::Area` (`targeting.rs`, `catalog.rs` Nova/Cataclysm). **CSV:** no `area` enum — generator sets `Area` when `point` + `aoe_radius` + `projectile_type none` (`AbilityGenerator.py` L214–215). **UI:** AoE disc when not `point_target` (includes Area) | **Decision (see below):** either document “area = point + AoE” in README/generator header only, *or* add optional CSV `target_type=area` alias (same Rust output). No pipeline change required for alias. |
| **Toggle** | Partial | `AbilityBehavior::Toggle`; `commit_ability` flips `AbilityState.toggled` (`casting.rs`, `components.rs`); CSV `ability_type=toggle`; Overcharge in `data/abilities.csv` has no effects | Add **toggle lifecycle**: e.g. `AbilityTrigger::OnToggleOn` / `Off` or tick `WhileToggled` + mana drain hook in `abilities/mod.rs` (regen pass) reading `state.toggled`. Wire Overcharge/Stone Skin follow-ups via CSV effects or small custom behavior. Document toggle + cooldown interaction (today: CD starts each flip). |
| **Autocast** | Missing | No code, CSV column, or README mention | **Decision:** defer MOBA autocast, *or* define as `ActivationRequest` from AI with optional `CastTarget` heuristic (separate from player targeting UI). Do not overload `AbilityDefinition` until behavior is chosen. |
| **Passive (ability)** | Partial | `AbilityBehavior::Passive`; `is_castable()` false → `CastError::Passive`; CSV passive validation; **no** runtime application of passive *ability* effects (Stone Skin: rank-scaled armor not applied) | Apply passive ability effects on **rank-up** and loadout init: reuse `AbilityTrigger::Passive` + `EffectRunner` (mirror item `apply_passives` pattern in `items.rs`) or stat modifiers on `AbilityState.rank` change. README already lists limitation — remove from Limitations when done. |
| **Channeling** | Partial | `ActionSpec.channel_time`, `Activation.channel_remaining`, `ActionChannelEndEvent`; abilities → `OnChannelEnd` (`casting.rs`); items channel but **no** item `OnChannelEnd` / tick triggers (`README.md` Limitations) | Emit item channel end trigger if parity needed. **`OnChannelTick`:** enum exists (`definition.rs`) but pipeline never fires it — add tick in `tick_activations` while `channel_remaining > 0` **or** restrict to custom mechanics only and document. |
| **Cast point** | Implemented | `cast_time` / `point_remaining`; capped 0.5s in builder + generator warn; mana/CD on fire not on request (`README.md`) | None. |
| **Cast backswing** | Implemented | `backswing` / `backswing_remaining`; CSV `cast_backswing`; abilities `occupies_caster: true`; items **fixed 0** backswing (`items.rs` L648) | If items need backswing, add optional column to `items.csv` → `ItemActivation.backswing` → `ActionSpec`. |
| **Cast range** | Implemented | `RankValue` on abilities; flat on items; `in_range` + edge distance (`dimensions::cast_distance`) | None. |
| **Target filtering (ally / enemy / self)** | Implemented | `TargetTeam` + `team_allows` / `target_allowed` (`targeting.rs`); CSV `target_team`; UI pick filter (`abilities/mod.rs` L321–322); effect AoE uses same teams (`effects.rs` + `team_allows`) | Clarify in README: CSV `target_team=self` → `SelfOnly` for activation, `Ally` for effect polarity defaults (`generator_common.py` ACTION_TEAMS vs TARGET_TEAMS). |
| **Target filtering (hero / creep / building)** | Missing | `PickableUnits` = any entity with `SelectionBounds` + `Health` + `Team` (heroes, creeps, towers, ancients, dummy) | Add optional `TargetUnitMask` on `ActionSpec` (Hero, Creep, Building/Ward) + components or query filters on `resolve_target` and `pick_under_ray`. Generator: optional `target_units` CSV column. Default **all living units** to preserve current behavior. |
| **Cooldown** | Implemented | `ActionCooldown.remaining`; tick in `abilities/mod.rs`, `items.rs`; start on commit | None. |
| **Cooldown reduction (CDR)** | Missing | Cooldown taken literally from definition at commit time | Add `cooldown_multiplier: f32` (or flat reduction) on caster stats/status; apply in `ability_action_spec` / `item_action_spec` when building `spec.cooldown`. Single hook, no per-ability logic. |
| **Cooldown reset** | Missing | No API on `ActionCooldown` | Add `ActionCooldown::reset()` / `reduce_remaining(by)`; invoke from future `AbilityEffect::ResetCooldown { slot \| action }` or item effect — one helper + one effect variant. |
| **Charges** | Partial | **Abilities:** `AbilityDefinition.charges`, `init_charges`, `restore_charges` over time (`catalog.rs`). **Items:** `max_charges` per stack unit, no time regen (`README.md` Limitations). Shared `ActionCooldown.charges` | Document dual model in README matrix. If unified behavior needed, add `ChargePolicy` enum (TimedRegen vs StackOnly) on definition — do not merge item stacks with ability regen without explicit design. |
| **Resource cost** | Partial | `ActionCost { mana }` only (`resources.rs`); health/gold not in pipeline | Extend `ActionCost` with optional `health` / `gold`; `check_ready` + `pay` + generator columns. Toggle drain (Overcharge) fits here or as channel-less tick cost while `toggled`. |
| **Cast interruption** | Implemented (document) | Cast point: silence/stun removes `Activation` before fire (`pipeline.rs` L347–349); move/stop/attack removes `Activation` (`movement.rs`); channel: stun/silence ends early + `ActionChannelEndEvent`; new order during channel drops component silently (`actions/mod.rs` comment); slot reskill/sell cancels in-flight (`ActionSources::current`) | Add short **Interruption** subsection to README (mirror table below). Optional: emit `ActivationCancelledEvent` for UI — not required for gameplay. |

### Cast interruption (reference)

| Phase | Silence / stun | New move / stop / attack | Target dies | Source slot changes |
|-------|----------------|---------------------------|-------------|------------------------|
| Cast point (before fire) | Cancel, no cost | Cancel, no cost | Queued cast may fail on re-request | Cancel |
| After fire, channeling | End channel, `OnChannelEnd` (abilities) | Cancel channel silently | N/A | Cancel |
| Backswing | Does not remove backswing by itself | Removes `Activation` | N/A | Cancel |

---

## Generator & CSV alignment

| Topic | Ability CSV | Item CSV | Rust |
|-------|-------------|----------|------|
| `target_type` | `unit`, `point`, `direction`, `no_target`, `self` only (`generator_common.TARGET_TYPES`) | Same set | + derived **`Area`** |
| `target_team` | `enemy`, `ally`, `both`, `self` | Same | `TargetTeam` (+ `SelfOnly` for self casts) |
| Timing | `cast_time`, `cast_backswing`, `channel_time` | `cast_time`, `channel_time` (no backswing column) | `ActionSpec` |
| Charges | `max_charges` + `charge_restore_time` (paired) | `max_charges` on consumables | Shared `ActionCooldown` shape, different regen rules |
| Behavior | `ability_type` active/passive/toggle | N/A (passive via effect trigger) | `AbilityBehavior` |

Validation already encodes many targeting rules (e.g. skillshot → point/direction, toggle → no_target/self, passive → no cast costs). See `AbilityGenerator.py` `parse_ability` and README generator section.

---

## Decisions needed (for implementer / product)

1. **Area targeting in data**  
   - **A (recommended, minimal):** Keep CSV as today; document that **`area` in gameplay = `point` + non-zero `aoe_radius` + no projectile** → Rust `TargetType::Area`.  
   - **B:** Add explicit CSV `target_type=area` (alias to same Rust), for author clarity only.

2. **Autocast**  
   - **Defer** until AI/copilot casting is in scope, *or*  
   - Define as **server-side / AI `ActivationRequest`** with no UI targeting resource — separate from `AbilityTargeting`.

3. **Cooldown reduction**  
   - Apply at **spec build time** from hero stats/status (`cooldown_multiplier`) vs. at **commit time** — prefer build time so UI `check_ready` matches fire.

4. **Ability passives vs toggles**  
   - **Passives:** stat bonuses on rank (Stone Skin) via shared effect pipeline.  
   - **Toggles:** stateful `toggled` + periodic mana cost and stat modifiers while on (Overcharge) — needs tick system, not only `OnCast`.

5. **Unit class filtering**  
   - Default **no filter** (current). Add opt-in mask when designing tower-only or hero-only spells.

6. **Item channel triggers**  
   - Parity with abilities for `OnChannelEnd` / tick, or explicitly document items as channel-without-triggers permanently.

---

## Suggested implementation phases (same PR branch later)

| Phase | Scope | Touch |
|-------|--------|-------|
| **P0 — Docs** | README activation matrix + interruption table; generator comment for Area derivation | `README.md`, `scripts/generator_common.py` header |
| **P1 — Passive abilities** | Apply `Passive` trigger effects on rank/loadout | `abilities/catalog.rs` or new `abilities/passive.rs`, small hook from rank-up |
| **P2 — Toggle + upkeep** | While-toggled mana drain + stat mods; optional on/off triggers | `abilities/mod.rs` tick, `definition.rs` triggers optional |
| **P3 — Spec extensions** | CDR multiplier, cooldown reset effect, `TargetUnitMask`, extended `ActionCost` | `actions/resources.rs`, `casting.rs`, `generator` columns |
| **P4 — Autocast** | Only after P3 stable; AI issues `ActivationRequest` | `ai.rs` / hero bot layer |

---

## README Limitations cross-check

These README items map directly to this audit:

| README limitation | Matrix row |
|-------------------|------------|
| Ability passives / toggle drains | Passive, Toggle |
| Channel ticks; item channel triggers | Channeling |
| Cooldown floors / CDR | Cooldown reduction (missing) |
| Item charges vs ability charges | Charges |
| `%` attack speed / timed attribute buffs on abilities | Passive / Toggle (effect types exist; ability passives not applied) |

---

## Out of scope (this plan)

- Projectile / mechanic rewrites (`abilities/mechanics.rs`)  
- Netcode replication of targeting state  
- Dota-style spell immunity / invuln targeting rules (unless added as `TargetUnitMask` + status checks)

---

## Verification checklist (for tester on this branch)

- [ ] Matrix row count matches requested mechanic list (21 rows).  
- [ ] Every **Implemented** row has at least one Rust test or documented manual path in README.  
- [ ] Generator validation rules cited match `AbilityGenerator.py` / `ItemGenerator.py` on `main`.  
- [ ] Decisions section reviewed with coordinator before P1 coding.
