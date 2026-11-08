> **Superseded in part (2026-11-08):** crit is no longer rolled at damage application. It is rolled once per landed attack in `basic_attack::resolve_basic_attack_impacts`, and `DamageEvent::can_crit` was replaced by the informational `crit`. See `2026-11-08-attack-modifiers.md`.

> **Superseded in part** by `2026-11-08-mechanic-composition-layers.md` (and `docs/architecture/composition.md`): `emit_chain_damage` / the `bolt_chain` custom effect no longer exist (Chain is a mechanic gate), the damage-kind choice lives in `DamageEvent::from_effect`, sleep waking is `status::wake_from_sleep`, and hostile teleports of debuff-immune units are rejected.

# Damage mechanics — architecture audit & implementation plan

**Date:** 2026-11-06  
**Scope:** Audit only; no gameplay code changes in this pass.  
**Goal:** Map requested damage mechanics to current code, identify gaps, and define minimal phased implementation for a follow-up implementer/tester.

---

## Executive conclusion

Labyrinth already has a **clean, event-driven damage core**: almost all spell/item damage flows through `AbilityTriggerEvent` → `EffectRunner` → `DamageEvent` → `apply_damage_events` → `apply_damage` / `mitigate`. Basic attacks bypass abilities and emit physical `DamageEvent`s from `basic_attack.rs`. Projectile geometry (homers, ground bolts, skillshots, splash lists) lives in `combat.rs`; consequences stay in `abilities/effects.rs`.

**Strong today:** physical/magical/pure types and armor/MR mitigation, instant damage, AoE via `UnitsInRadius`, projectile splash (45% to bystanders), low-health conditional scaling (Execute / Arcane Lance), debuff immunity and flat stat modifiers (including Barrier’s armor buff).

**Missing or stub-level:** DoT, cleave, chain lightning, distance-based scaling, max/missing-health scaling, crit, outgoing/incoming damage amplifiers, spell amplification, HP shields/barriers, damage block, negation, reflection, damage sharing, and periodic channel ticks (`OnChannelTick` is defined but never fired).

**Recommendation:** Extend **`combat.rs`** with a single **`resolve_damage`** (or enriched `DamageEvent`) pipeline rather than scattering logic in `effects.rs` or `basic_attack.rs`. Keep `EffectRunner` responsible for *who* and *how much raw*; keep `combat` authoritative for *type, modifiers, mitigation, shields, and final HP delta*. Generators already cover damage types and four scaling modes — extend `generator_common.py` when adding new `EffectScaling` variants.

---

## End-to-end pipeline (current)

```text
Basic attack:
  request_basic_attacks → windup → BasicAttackReleaseEvent
    → melee: BasicAttackImpactEvent immediately
    → ranged: BasicAttackInFlight → BasicAttackImpactEvent
  resolve_basic_attack_impacts → DamageEvent (Physical, pre-mitigation amount)

Abilities / items:
  mechanics / OnUse / OnAttack(Hit) → AbilityTriggerEvent(TriggerContext)
  resolve_ability_triggers → EffectRunner::execute
    → DamageEvent (amount after EffectScaling, damage_type)

Obstacle course (exception):
  apply_hazard_hits → apply_damage() directly on Health (magical)

Shared application:
  apply_damage_events → apply_damage(raw, damage_type, CombatStats)
    Physical → mitigate(armor)
    Magical  → mitigate(magic_resist)
    Pure     → raw (clamped ≥ 0)
```

**System order (abilities plugin):** trigger resolution and other effect messages run **before** `combat::apply_damage_events` (`abilities/mod.rs`), so heals/status/displacement in the same frame still precede HP subtraction.

---

## Status matrix

Legend: **Implemented** | **Partial** | **Missing**

| Mechanic | Status | Notes | Primary file refs |
| --- | --- | --- | --- |
| Physical damage | **Implemented** | Basic attacks always physical; many spells/items physical | `basic_attack.rs` (`resolve_basic_attack_impacts`), `components.rs` (`DamageType`), `combat.rs` |
| Magical damage | **Implemented** | Default for most spells; hazard orbs | `abilities/effects.rs`, `obstacle_course.rs` |
| Pure damage | **Partial** | `apply_damage` ignores resist; **no shipped ability/item** uses `DamageType::Pure` in catalogs | `combat.rs:382-387`, `generator_common.py` (`pure`) |
| Mitigation (armor / MR) | **Implemented** | Dota-style: `factor = 1 - 0.06×R / (1 + 0.06×\|R\|)` | `combat.rs:390-393`, tests `armor_reduces_physical`, `magic_resist_reduces_magical` |
| Instant damage | **Implemented** | All `DamageEvent`s apply same frame (after trigger chain) | `combat.rs:70-83` |
| DoT (damage over time) | **Missing** | `StatusEffect` has `heal_per_sec` only; no `damage_per_sec`; `OnChannelTick` unused | `items.rs:248-271`, `items.rs:1181-1204`, `abilities/definition.rs:115` |
| Area (AoE) | **Implemented** | `EffectTarget::UnitsInRadius` + ability `aoe_radius`; shockwave/nova/meteor/caltrops/cataclysm | `abilities/effects.rs:194-217`, `abilities/mechanics.rs:177-195`, `dimensions.rs` (`area_contains`) |
| Splash | **Implemented** | Projectile impact radius; splash victims in `TriggerContext.affected_units`; 45% damage via catalog helper | `combat.rs:283-308`, `abilities/mechanics.rs:266-291`, `abilities/catalog.rs:117-142` |
| Cleave | **Missing** | Attacks single target only | `basic_attack.rs` |
| Chain | **Missing** | No bounce target selection or falloff | — |
| Distance-based scaling | **Missing** | Not in `EffectScaling` | `abilities/definition.rs:174-183` |
| Current-health scaling | **Partial** | Only gate: `TargetHealthBelow` (skip or apply fixed bonus), not proportional to HP | `abilities/effects.rs:243-248` |
| Max-health scaling | **Missing** | Item passives affect max HP, not damage formulas | `items.rs` passives |
| Missing-health scaling | **Missing** | — | — |
| Execute (low-HP bonus) | **Implemented** | Handwritten Execute + generated Arcane Lance: base hit + extra damage with `TargetHealthBelow(0.35)`; constants `EXECUTE_*` in catalog | `abilities/catalog.rs:148-252`, `abilities/generated.rs:112-119` |
| Critical strike | **Missing** | No crit chance/damage on `CombatStats` or attacks | `components.rs` (`CombatStats`) |
| Damage amplification (outgoing) | **Missing** | No spell/physical amp stats | — |
| Damage reduction (incoming) | **Partial** | Only armor/MR (and temporary flat modifiers via status) | `items.rs` (`apply_status`), `combat.rs` |
| Damage block | **Missing** | — | — |
| Barrier (absorb HP) | **Missing** | **Barrier** ability = debuff immunity + **armor buff** (`StatusSpec::armor_buff("barrier", …)`), not a shield pool | `abilities/catalog.rs:264-279` |
| Damage negation | **Partial** | Debuff immunity blocks new debuffs, not damage; no “immune to damage” flag | `items.rs:1227-1230` |
| Damage reflection | **Missing** | — | — |
| Damage sharing | **Missing** | — | — |
| Spell amplification | **Missing** | Int affects MR regen, not spell damage multiplier | `components.rs` (`HeroAttributes`) |

---

## File reference map

| Concern | Location |
| --- | --- |
| Damage message + mitigation + spell projectiles | `src/combat.rs` |
| Basic attack damage emission | `src/basic_attack.rs` |
| Effect scaling + damage emission | `src/abilities/effects.rs` |
| `EffectScaling`, `AbilityEffect::Damage`, triggers | `src/abilities/definition.rs` |
| Splash recipe, Execute constants, AoE on cast | `src/abilities/catalog.rs` |
| CSV-generated definitions | `src/abilities/generated.rs`, `data/ability_effects.csv` |
| Status modifiers (armor/MR/AS/move), HoT tick | `src/items.rs` (`StatusEffect`, `tick_status_effects`, `apply_status_effect_events`) |
| Item on-hit triggers | `src/items.rs` (`item_attack_triggers` → `OnAttack` / `OnAttackHit`) |
| Architecture docs | `README.md` (Ability architecture, generators, status table) |
| Generator: damage rows + scaling | `scripts/generator_common.py`, `scripts/AbilityGenerator.py`, `scripts/ItemGenerator.py` |

---

## Design notes for gaps

### 1. Central damage resolution (recommended spine)

Today `DamageEvent` carries `{ source, target, amount, damage_type }` and `apply_damage_events` only mitigates. For block, shields, amp, and crit, introduce either:

- **`DamageEvent` extensions** (flags + metadata: `DamageKind::Ability \| Attack \| Dot`, `ability: Option<AbilityId>`, `can_crit`, `lifesteal`, …), or  
- **`DamageInstance`** resolved in one function `resolve_incoming_damage(instance, target_query) -> f32`.

Keep **`apply_damage(raw, type, stats)`** as the final mitigation step on the post-modifier raw amount so tests stay stable.

### 2. DoT

**Option A (minimal):** Add `damage_per_sec: f32` to `StatusEffect`; tick in `tick_status_effects` via **`DamageEvent`** (magical/physical/pure stored on status) so DoTs respect immunity/shields later.

**Option B:** Wire **`OnChannelTick`** in `actions/pipeline.rs` during `channel_remaining` countdown; channel abilities emit damage/heal each tick without new status fields.

Default for MOBA parity: **Option A** for burn/poison; Option B for channeled “tick while casting”.

### 3. Cleave / chain

- **Cleave:** On `BasicAttackImpactEvent`, query additional enemies in an arc/radius behind primary; emit secondary `DamageEvent`s at reduced ratio (config on `CombatStats` or hero definition).
- **Chain:** New mechanic or custom effect: after `DamageEvent` on first target, search next enemy within jump radius, decrement remaining jumps, apply falloff multiplier.

### 4. Health-based scaling

Extend `EffectScaling`:

| Variant | Suggested semantics |
| --- | --- |
| `TargetHealthBelow(t)` | **Keep** (gate / execute bonus) |
| `TargetMissingHealth(ratio)` | `base + ratio × (max - current)` |
| `TargetMaxHealth(ratio)` | `base + ratio × max` |
| `TargetCurrentHealth(ratio)` | `base + ratio × current` (rare; document as % current) |

Add CSV keys in `generator_common.py` `SCALING_TYPES`.

### 5. Crit, amp, spell amp

Suggested **`CombatStats`** (or separate `DamageModifiers` component):

| Field | Default | Apply point |
| --- | --- | --- |
| `crit_chance` | `0.0` | Basic attacks (+ optional spell flag) |
| `crit_multiplier` | `2.0` | After raw, before mitigation |
| `outgoing_physical_amp` | `0.0` | Additive ×(1+amp) on physical |
| `outgoing_spell_amp` | `0.0` | Additive on magical ability damage |
| `incoming_damage_amp` | `0.0` | Rare; after mitigation |

**Spell amplification:** Either map from Intelligence (`int × 0.0` until designed) or item stat `spell_amp` — README already documents Int per level; avoid double-counting with `EffectScaling::CasterAttribute(Int, …)`.

### 6. Shields, block, reflection, sharing

| Mechanic | Suggested model |
| --- | --- |
| **HP barrier** | Component `ShieldPool { physical, magical, pure \| unified }` consumed in `resolve_incoming_damage` before `Health` |
| **Block** | Flat or proc: subtract min(block, damage) once per hit |
| **Negation** | Status flag `damage_immune` (distinct from debuff immunity) |
| **Reflection** | After applying damage, emit `DamageEvent` to `source` with reflected amount |
| **Sharing** | Split `%` to linked entities (bond component); emit proportional events |

Rename or document **Barrier** ability to avoid confusion with shield pools (e.g. tooltip “Debuff immunity + bonus armor”).

### 7. Pure damage usage

Add at least one test ability or item in CSV with `damage_type=pure` once pipeline supports shields (pure often bypasses armor but not shields in DotA).

---

## Minimal implementation phases

| Phase | Scope | Touch mainly | Exit criteria |
| --- | --- | --- | --- |
| **0** | Document + lock tests for current behavior | — | Existing `combat` / `basic_attack` / integration tests green |
| **1** | `resolve_incoming_damage` refactor; optional `DamageEvent` metadata | `combat.rs` | Same numeric results for armor/MR; new unit tests for ordering (shield before HP) |
| **2** | `EffectScaling` health + distance; generator + CSV | `definition.rs`, `effects.rs`, `generator_common.py` | One generated row per new scaling; Execute unchanged |
| **3** | Outgoing/incoming amp + spell amp on `CombatStats` | `components.rs`, `combat.rs`, items passives | Item or debug buff changes spell damage measurably |
| **4** | `ShieldPool` + optional block proc | `combat.rs`, `StatusSpec` or component | Barrier rework **or** new shield buff absorbs damage |
| **5** | Cleave (auto) + chain (one ability) | `basic_attack.rs`, new mechanic | Regression test: primary + 1 cleave target |
| **6** | DoT via status `damage_per_sec` | `items.rs`, `generator_common.py` | Burn status ticks through `DamageEvent` |
| **7** | Crit + reflection (optional) | `combat.rs`, `basic_attack.rs` | Deterministic crit test with seeded RNG |

**Recommended defaults for phased work**

- Mitigation formula: **keep current** (`combat.rs::mitigate`).
- Splash ratio: **keep 0.45** (`SPLASH_DAMAGE_RATIO`).
- Execute threshold: **keep 0.35**; bonus as **separate damage entry** with `TargetHealthBelow` (not a hidden multiplier).
- Crit multiplier when added: **2.0**; attacks only until spell-crit flag exists.
- Spell amp: **additive** `1 + spell_amp` on magical damage from abilities/items; do not auto-derive from Int until hero design says so.
- Shields: **unified pool** first (one number); split physical/magic later if needed.
- DoT: **tick interval 0.25s** aggregated in frame dt (same as HoT style in `tick_status_effects`).
- Pure damage: bypass armor/MR only; **respect shields** unless explicitly `pierces_shields`.

---

## Test plan

### Existing (keep green)

| Test | File |
| --- | --- |
| Armor / MR mitigation | `combat.rs` tests |
| Damage events skip dead | `combat.rs` `damage_events_apply_mitigation_and_skip_dead_targets` |
| Projectile splash victim list (no direct HP) | `combat.rs` `projectile_reports_its_victims_without_dealing_damage` |
| Ranged/melee attack → mitigated HP | `basic_attack.rs` integration tests |
| Execute / Arcane Lance definition scaling | `abilities/catalog.rs`, `abilities/generated.rs` + `catalog` tests |
| Integration: cast → damage | `abilities/mod.rs` |

### Add per phase

| Phase | Tests |
| --- | --- |
| 1 | Shield absorbs before HP; pure vs physical vs magical ordering |
| 2 | `TargetMissingHealth` numeric; distance falloff at 0 / mid / max range |
| 3 | Spell amp 0 vs 0.25 on same ability rank |
| 4 | Barrier shield expires; overkill does not leak to HP |
| 5 | Cleave hits N enemies; chain stops at jump limit |
| 6 | DoT ticks N times; dispel removes tick |
| 7 | Crit doubles damage with `crit_chance = 1` test hook |

Prefer **headless** `World` + `run_system_once` patterns already used in `combat.rs` and `abilities/mod.rs`. One **playtest** script: Skirmisher Execute on dummy below 35% HP vs above.

---

## CSV / generator hooks

### Supported today (`scripts/generator_common.py`)

- **`damage_type`:** `physical`, `magical`, `pure` → `DamageType::*`
- **`scaling_type` + `scaling_value`:**  
  `strength`, `agility`, `intelligence`, `attack_damage`, `target_health_below` → `EffectScaling::*`
- **Triggers with damage:** `on_cast`, `on_projectile_hit`, `on_impact`, … → `AbilityTrigger`
- **Targets:** `trigger_unit`, `affected_units`, `units_in_radius`, … → `EffectTarget`
- **Splash:** Not a CSV column — hand-written `projectile_damage()` in `catalog.rs` duplicates pattern; generated skillshots use single-target rows unless extended

### Generator gaps to close with new mechanics

| Feature | CSV / codegen change |
| --- | --- |
| Missing/max/current HP scale | New `scaling_type` values → new `EffectScaling` variants |
| DoT | `apply_status` template `burn` with DPS + `damage_type`, or new `effect_type=dot` |
| Distance scale | Optional `scaling_type=distance` with max range from ability row |
| Cleave / chain | Likely **custom** mechanics + `effect_type=custom`, not raw CSV |
| Passive spell amp | `ItemGenerator` / `stat_modifier` or new passive stat in `PassiveStat` |
| Shield | `apply_status` with `effect_id=shield` and `amount` = pool, or component spawn in custom effect |

**Commands**

```bash
python3 scripts/AbilityGenerator.py --check
python3 scripts/ItemGenerator.py --check
```

Examples: `data/ability_effects.csv` (Seismic Slam AoE physical, Arcane Lance execute row), `data/examples/` per README.

---

## Related plans

- `/cursor/stores/bc-454f7707-265b-4c74-89ae-eea431d8bebe/docs/plans/2026-11-06-shared-effects-pipeline.md` — trigger/effect bus (damage should stay downstream of this).
- `/cursor/stores/bc-454f7707-265b-4c74-89ae-eea431d8bebe/docs/plans/2026-11-06-activation-targeting-mechanics.md` — casting/range (feeds distance scaling).

---

## Out of scope (this pass)

- Tower/creep damage formulas balance  
- Netcode replication of damage numbers  
- VFX for crit / block / shield break  
- Renaming `Barrier` ability (cosmetic; note in implementer PR if shields added)

---

## Implemented (2026-11-06, PR #51)

Phases **1–7** landed on branch `cursor/damage-mechanics-architect-415b`:

| Phase | Done |
| --- | --- |
| 1 | `resolve_incoming_damage` / `resolve_outgoing_amount`; enriched `DamageEvent`; shield-before-HP tests |
| 2 | New `EffectScaling` variants + `generator_common.py` CSV keys |
| 3 | Amp / spell amp on `CombatStats` + item `PassiveStat` hooks |
| 4 | `ShieldPool`, block, Barrier shield buff (armor + debuff immunity unchanged) |
| 5 | Cleave on basic attacks; Bolt chain via `bolt_chain` custom effect |
| 6 | DoT via `damage_per_sec` → `DamageEvent` |
| 7 | Seeded `CombatRng`, `DamageTestHooks::force_crit`, reflection |

README damage table added. **Limitations for tester:** no shipped CSV row for pure/burn/scaling demos yet; damage sharing / `OnChannelTick` / split physical-magic shields still out of scope; hazard orbs still call `apply_damage` directly.
