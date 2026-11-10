# Status / disable catalog — architecture audit & phased rollout

**Date:** 2026-11-06  
**Scope:** Audit and planning only (no gameplay behavior changes in this document’s PR).  
**Goal:** Map MOBA-style disables and related mechanics against the current engine, identify reuse vs new data, and propose an incremental rollout for implementer/tester.

> **Superseded (2026-11-10).** The mechanic matrix and implementation status in this document are out of date. See `docs/plans/2026-11-10-status-effects-completion.md` for the audit, design (`ActionBlocks`, `TargetIntent`, compulsions, concealment, spell block / reflect) and the implemented steps, and `docs/architecture/composition.md` section 5 for the current rules. Kept for history.

---

## Executive conclusion

The game already has a **single shared status pipeline**: `StatusEffect` + `StatusEffects` on units (`src/status.rs`), authoring via `StatusSpec` (`src/abilities/definition.rs`), execution through `StatusEffectEvent` / `StatusChange` (`src/status.rs`), and consumption in movement (`src/movement.rs`), basic attacks (`src/basic_attack.rs`), activation gating (`src/actions/resources.rs`), teleport collision (`src/teleport.rs`), and tests in `src/abilities/mod.rs`.

Gaps vs a full MOBA surface:

- **Representation:** `StatusEffect` is mostly **flat booleans + move/armor/damage flat modifiers**; there is **no** first-class *dispel tier*, *undispellable*, *status resistance*, *spell block/reflect*, *vision/invis*, or *forced-move* CC beyond displacement + root/stun semantics.
- **Dispel:** `AbilityEffect::Dispel { kind }` removes **all** effects of that `StatusKind` (`apply_status_effect_events`); there is **no** basic vs strong split or per-buff rules.
- **“Spell immunity”:** `StatusSpec::DebuffImmunity` builds `StatusEffect::debuff_immunity`, which **blocks new debuffs** and **purges existing debuffs on apply** — closer to *strong dispel + debuff immunity* than Dota’s *spell immunity* (magic damage/effect types).
- **Generator:** `scripts/generator_common.py` `STATUSES` maps only **stun, root, silence, disarm, slow, phased, forceful, debuff_immunity/spell_immunity**; anything new must extend `StatusSpec`, `StatusEffect`, and this table together.

**Recommendation:** Extend **`StatusEffect` + `StatusSpec` + generator `STATUSES`** as the canonical place for timed states; add **`bitflags` or small enums** only where many systems query the same rule set; use **`Custom` effects + dedicated systems** for mechanics that need global hooks (fear pathing, taunt retarget, leash clamp) until patterns stabilize.

---

## Current architecture (reference)

| Concern | Location | Role |
|--------|----------|------|
| Timed effect storage, tick, stat revert | `src/items.rs` (`StatusEffects`, `tick_status_effects`, `apply_status`) | Source of truth for duration and flat `CombatStats` deltas |
| Authoring templates | `src/abilities/definition.rs` (`StatusSpec`, `AbilityEffect::ApplyStatus` / `Dispel` / `RemoveStatus`) | Compile-time ability/item definitions |
| Apply/dispel events | `src/abilities/effects.rs` (`apply_status_effect_events`) | Single consumer; debuff immunity gate; dispel by kind or by `id` |
| Movement | `src/movement.rs` (`can_move`, phased) | Root/stun stops locomotion; phased skips avoidance/collision with units |
| Attacks | `src/basic_attack.rs` (`can_attack`) | Disarm/stun gates requests and cancels swings |
| Cast/items | `src/actions/resources.rs` (`DisableRule`) | Abilities: silence, stun or sleep (hex counts as silence; mute alone does not block); items: mute or stun (current code; this row once said items were stun only) |
| Teleport | `src/teleport.rs` | Phased bypasses unit blocking |
| Vision | `src/fog.rs` | Team vision radii only; **not** tied to statuses |
| Picking | `src/picking.rs` | Uses `Visibility::Hidden` for living units — **not** status-driven today |
| Content gen | `scripts/generator_common.py` | `apply_status` → `StatusSpec` + runtime `id` |

---

## Mechanic matrix

Legend: **Implemented** = behaves as named in typical MOBA sense; **Partial** = data or hook exists but semantics differ or are incomplete; **Missing** = no support.

| Mechanic | State | Primary file refs | Design hook (reuse vs new) | Phased rollout |
|----------|-------|-------------------|----------------------------|----------------|
| **Stun** | Implemented | `items.rs` (`stunned`, `is_stunned`, `can_cast`/`can_move`/`can_attack` via stun) | Reuse boolean on `StatusEffect`; extend if *mini-stun* needs a distinct class | **P0** stable; optional **P1** *stun tier* (full vs mini) via `id` or enum field |
| **Mini-stun** | Partial | Same as stun (no distinction) | Reuse `stun` + **metadata**: `InterruptOnly` flag or separate `id` (`"mini_stun"`) + systems that only care read it | **P1** after interrupt/channel rules are centralized |
| **Root** | Implemented | `items.rs` (`rooted`, `is_rooted`); `movement.rs` | Reuse; note **stun counts as rooted** today | **P0** |
| **Leash** | **Implemented 2026-11-07** (clamp, never breaks; see `docs/plans/2026-11-07-movement-mechanics.md`) | `status.rs` (`Leash`), `movement.rs::enforce_leashes`, `StatusSpec::Leash` | New: **component or status** with `anchor`, `max_range`, break behavior; hook in `movement.rs` / `apply_move_targets` | **P3** (needs design: snap back vs break vs damage) |
| **Silence** | Implemented | `items.rs` (`silenced`); `actions/resources.rs` (`DisableRule::SilenceOrStun` for abilities) | Reuse | **P0** |
| **Mute** | Missing | — | New flag `muted: bool` OR reuse `silenced` with **DisableRule** variant for items | **P2** (pair with `DisableRule` matrix) |
| **Break** | Missing | — | Typically *disable passive abilities* — needs **`AbilityState` / passive runner** rules; not in `StatusEffects` alone | **P3** (depends on passive architecture) |
| **Disarm** | Implemented | `items.rs` (`disarmed`, `is_disarmed`); `basic_attack.rs` | Reuse; **stun implies disarm** today | **P0**; **P1** optional: disarm without stun semantics on interrupts |
| **Hex** | Missing | `components.rs` (`HexBolt` ability id only) | **Decision:** composite **silence + disarm + pacify** + forced model vs **transform** unit type | **P2** — start as **composite flags** + anim; transform later |
| **Sleep** | Missing | — | New: `asleep: bool` + **wake on damage** in `apply_damage_events` | **P2** |
| **Fear** | Missing | — | New: forced **MoveTarget** + **AI/player override** (contested input); pathing toward safe direction | **P3** (high AI/input coupling) |
| **Taunt** | Missing | `components.rs`, `heroes.rs` (stub ability name) | New: **`AttackTarget` force** + duration; PvE AI already picks targets — extend for forced hero attack | **P3** |
| **Hypnosis** | Missing | — | Often *sleep* or *move toward source* — pick one **P2**/**P3** spec | **Defer** until sleep/fear exist |
| **Slow (move)** | Implemented | `StatusSpec::Slow`, `StatusEffect.move_speed` | Reuse `%` slow from current `CombatStats.move_speed` | **P0** |
| **Attack speed slow** | Partial | `CombatStats` has `attack_speed_*` but **no status applies it** | Extend `StatusEffect` with `attack_speed_flat` / `attack_speed_mult` **or** map via `Modifier` + recompute in tick | **P2** |
| **Turn rate slow** | Partial; **2026-11-07: `StatusSpec::TurnRateBonus` added and the slow/bonus formula lives in `CombatStats::effective_turn_rate`** | `facing::turn_toward` + `CombatStats.turn_rate` — **no status modifier** | Add optional delta on `StatusEffect` or `%` slow applied where `turn_toward` reads stats | **P2** |
| **Blind** | Missing | — | New: miss chance or **cannot basic attack** subset — hook in `basic_attack` resolution | **P2** (PvE vs RNG) |
| **Cyclone** | **Implemented 2026-11-07**: `DisplacementKind::Cyclone` (hold + lift, stun + invulnerable status, untargetable, immune to other displacement and teleports). Banish / full untargetable semantics beyond targeting remain P3 | `displacement.rs`, `status.rs` (`StatusEffect::cyclone`, `is_untargetable`) | Custom mechanic: **untargetable + hover** + purge rules | **P3** |
| **Banish / hide** | Missing | — | Overlap with **untargetable** + `Visibility` — hook `picking.rs` / targeting | **P3** |
| **Ethereal** | Missing | — | New: **immune physical**, **can't attack**, may take magic amp — extend `DamageEvent` handling in `combat.rs` | **P3** |
| **Invisibility** | Missing | `fog.rs` | New team/stat **vision modifier** + optional `Visibility`; **reveal** counters | **P3** |
| **Invulnerability** | Missing | `combat.rs` `apply_damage_events` | New: `invulnerable` flag short-circuit **or** damage type rules | **P2** |
| **Debuff immunity** | Implemented (with caveats) | `items.rs` (`debuff_immune`, `has_debuff_immunity`, `apply_status` gate); `apply_debuff_immunity` purges | Reuse; **rename/tooltip** vs *spell immunity* to reduce confusion | **P0** doc; **P1** separate *true spell immunity* if needed |
| **Spell immunity** | Partial | Generator maps `spell_immunity` → `DebuffImmunity` (`generator_common.py`) | **New** type if magic-target rules differ from debuff block | **P2** |
| **Attack immunity** | Missing | — | New flag checked in `basic_attack` / `DamageEvent` (physical subset) | **P2** |
| **Spell block** | Missing | — | One-shot: **resource on unit** or `StatusEffect` charge consumed when targeted | **P3** |
| **Spell reflection** | Missing | — | **Redirect** in projectile/spell resolution — not status-only | **P3** |
| **Phased** | Implemented | `items.rs` (`ignore_unit_collision`); `movement.rs`, `teleport.rs` | Reuse | **P0** |
| **Reveal** | Missing | `fog.rs` | Vision aura or **True Sight** against invis — pair with invis | **P3** with invis |
| **Basic dispel** | Partial | `AbilityEffect::Dispel { kind }`; `effects.rs` | Extend **dispel** with **BuffDebuff | Strong** and **filter by tag** | **P1** |
| **Strong dispel** | Partial | Debuff immunity application **purges** debuffs (`apply_debuff_immunity`) | Formalize as **dispel kind + removes hard dispel types** | **P1** |
| **Undispellable** | Missing | — | Add `dispel_tier` or `undispellable: bool` on `StatusEffect`; filter in `remove_statuses` | **P1** |
| **Status resistance** | Missing | — | New: **`duration_multiplier`** applied in `apply_status` / tick; optional cap | **P2** |

---

## Cross-cutting design notes

### 1. Keep `StatusEffect` as the narrow waist

Adding dozens of `bool` fields does not scale. Prefer:

- **`StatusCategory` bitflags** for queries (`HARD_CC`, `SOFT_CC`, `SILENCE`, `DISARM`, …), **or**
- **`id` + data-driven behavior** for generated content (`generator_common.py` stays in sync).

Timed stat changes already live on the same struct (`move_speed`, `armor`, …); **attack speed / turn rate** should follow the same pattern before adding parallel components.

### 2. Stun stacks and mini-stun

Today **same `id` refresh** replaces (`apply_status`). Multiple concurrent stun sources may need **separate ids** (generator) **or** explicit **max remaining** stacking rules — document when implementer adds multi-source stuns.

### 3. Dispel model

Current API: **`Dispel(Buff|Debuff)`** wipes all matching `kind`. MOBA “basic/strong” usually **filters** by dispel type on each buff. **Plan:** add `DispelStrength` + per-effect `DispelType { Basic, Strong, None }` in `apply_status_effect_events`’s removal path.

### 4. Disable matrix (abilities vs items vs channels)

Centralize in one table (conceptually) so **mute**, **break**, **terror**, etc. do not scatter new `if`s in `actions/pipeline.rs` and `casting.rs`.

### 5. Vision and status

`fog.rs` is **independent** of `StatusEffects`. Invis/reveal/true sight need a **deliberate bridge** (e.g. unit-scoped vision modifiers resource) to avoid ad hoc `Visibility` hacks.

### 6. Generator alignment

Any new `StatusSpec` variant must be reflected in:

- `scripts/generator_common.py` (`STATUSES`, `apply_status` branch)
- `src/abilities/tooltip.rs` (player-readable strings)
- `src/item_catalog.rs` / generated RS if items expose the effect

---

## Recommended implementation phases (for implementer)

| Phase | Focus | Outcome |
|-------|--------|---------|
| **A — Status metadata** | `StatusEffect`: `dispel_type`, `undispellable`, optional `tags`; `Dispel`/`apply_status_effect_events` filtering | Enables basic vs strong dispel and protection |
| **B — Soft modifiers** | Attack speed slow, turn rate slow on `StatusEffect` + `CombatStats` recompute | Parity with common slows without new mechanics |
| **C — Disable extensions** | Mute (`DisableRule`), sleep + wake-on-damage, invulnerability hook in damage | Broad CC/defensive coverage with localized hooks |
| **D — Vision suite** | Invis, reveal, blind (as miss or target rules) + `fog` integration | Full information warfare |
| **E — Forced behavior** | Fear, taunt, leash; AI + player conflict resolution | Highest complexity; needs specs per unit control |

### Implementation status (branch `cursor/status-effects-architect-6a4f`)

| Phase | Status |
|-------|--------|
| **A** | Done: `DispelType` / `DispelStrength` on `StatusEffect` and `AbilityEffect::Dispel`, filtering in `apply_status_effect_events`; generator `dispel` rows use basic vs strong when `amount` is set (legacy rows default to basic). |
| **B** | Done: `StatusSpec::AttackSpeedSlow`, `TurnRateSlow`; `StatusEffect` carries `attack_speed_flat` / `attack_speed_mult` / `turn_rate_slow_percent`; `CombatStats::effective_turn_rate`; movement and cast-point turning use it. |
| **C** | Done: `Mute`, `Sleep`, `Invulnerable`, `Hex` (`StatusSpec` + effects); `DisableRule::MuteOrStun` for items; sleep wakes on damage in `apply_damage_events`; invuln blocks all `DamageEvent` receipt. |
| **D** | Partial: **Blind** implemented as deterministic miss on basic-attack resolution (stable hash from attacker/target entity bits), not fog / invis / reveal. |
| **E** | Partial: **Fear** applies forced flee in `apply_move_targets` using `fear_source` + cached positions; **taunt** and **leash** not implemented. |

---

## Open decisions (for product / implementer)

1. **Hex scope (initial):** *Full disable* (silence + disarm + no spells/items) + optional cosmetic vs *model transform* with different hitbox/collision.  
2. **Fear pathing:** Pure **AI** (creeps) first vs **shared forced MoveTarget** with player lockout — affects `unit_commands` and network.  
3. **Mute vs silence:** Items blocked by mute only, or treat mute as silence for v1.  
4. **Blind:** Deterministic miss (e.g. cannot attack) vs % evasion — PvP implications.  
5. **“Spell immunity” naming:** Keep generator alias but split **`DebuffImmunity`** vs **`SpellImmunity`** in code when magic immunity behavior is specified.

---

## Suggested tests (for tester phase)

- Status: dispel respects `undispellable`; debuff immunity blocks new debuffs; stun/root/disarm/silence gates unchanged unless spec’d.
- Movement: leash clamp and fear forced move do not deadlock `NavPath`.
- Combat: ethereal / invuln / blind interact correctly with `DamageEvent` and basic attack pipeline.
- Regression: existing tests in `src/items.rs`, `src/abilities/mod.rs` (channel interrupt, displacement root) remain green.

---

## File index (audit trace)

`src/items.rs`, `src/abilities/definition.rs`, `src/abilities/effects.rs`, `src/components.rs`, `src/input.rs`, `src/movement.rs`, `src/basic_attack.rs`, `src/combat.rs`, `src/fog.rs`, `src/picking.rs`, `src/actions/resources.rs`, `src/actions/pipeline.rs`, `src/teleport.rs`, `src/displacement.rs`, `scripts/generator_common.py`.

Note (2026-11-07): the status types, `tick_status_effects`, `StatusChange` / `StatusEffectEvent` and `apply_status_effect_events` now live in `src/status.rs` (`StatusPlugin`); references to `src/items.rs` and `src/abilities/effects.rs` above for those items now mean `src/status.rs`.
