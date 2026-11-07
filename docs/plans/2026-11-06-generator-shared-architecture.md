# Generator shared activation / mechanics / triggers / effects — architecture plan

**Date:** 2026-11-06  
**Scope:** Audit + implementation plan only (no generator or runtime behavior changes in this PR).  
**Related:** Companion runtime audit `2026-11-06-shared-effects-pipeline.md` (Project Context — effects executor already shared in Rust).

**Exemplar:** A **Blink** ability (`on_cast` + `movement,teleport` + point targeting) and **Blink Dagger** (`on_use` + same effect row + point `cast_range`) must compile to the same `AbilityEffect::Teleport` and run through the same activation + trigger pipeline — not parallel CSV schemas or duplicate emitters.

---

## Executive summary

The generators **already share one effect-row schema and one Rust emission path** via `scripts/generator_common.py` (`EFFECT_COLUMNS`, `effect_to_rust`, triggers, targets, enums). Abilities and items diverge in **owner CSV shape**, **activation parsing** (duplicated rules), and **mechanics** (projectiles / AoE rings exist only on the ability side). Production `data/items.csv` has **no activation columns** yet; `ItemGenerator` reads activation only as optional extras. Target headers for abilities and effect CSVs **match spec today**; items need a documented header extension without breaking legacy rows.

**Recommendation:** Consolidate activation + cross-field validation into `generator_common.py`, extend item activation columns to parity with abilities where runtime supports it, keep **one** generated-content system (`AbilityEffectEntry` everywhere). Do **not** add a second item effect format or a separate item executor.

---

## Target CSV schemas (authoritative)

### `abilities.csv`

Required columns (generator `ABILITY_COLUMNS`):

`id,name,slot,ability_type,target_type,target_team,cast_range,aoe_radius,cast_time,channel_time,cooldown,mana_cost,max_charges,charge_restore_time,projectile_type,projectile_speed,description`

Optional extras (already supported, not in required tuple): `cast_backswing`, `max_rank`.

### `ability_effects.csv`

`ability_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,duration,radius,amount,scaling_type,scaling_value`

Same as `("ability_id", *EFFECT_COLUMNS)` in code.

### `items.csv`

**Preserve** existing required identity/economy columns:

`id,name,item_type,cost,sell_value,tier,rarity,stackable,max_stack,consumable,max_charges,description`

**Preserve** optional extras already emitted: `short_label`, `cooldown`, `components`, `color_r`, `color_g`, `color_b`.

**Extend** with activation columns (blank = N/A / instant no-target use for passive-only items):

`charge_restore_time,target_type,target_team,cast_range,cast_time,channel_time,cooldown,mana_cost,projectile_type,projectile_speed`

Note: `cooldown` already exists as an optional extra for actives; treat it as part of the activation group in docs and validation, not a second cooldown concept.

### `item_effects.csv`

`item_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,duration,radius,amount,scaling_type,scaling_value`

Same as `("item_id", *EFFECT_COLUMNS)`.

---

## Enums and validation rules (single source of truth)

These should live in **`generator_common.py`** and be imported by both generators (today: mostly centralized; activation rules duplicated).

| Domain | Allowed values | Notes |
|--------|----------------|-------|
| Ability slot | `q,w,e,r,innate` | Case-insensitive in CSV; `r` → ultimate flag in `components.rs` |
| Ability type | `active,passive,toggle` | Passive: targeting/projectile/cost warnings |
| Item type | `ITEM_TYPES` map in `ItemGenerator.py` | Includes legacy plural aliases (`weapons`, `accessories`, …) |
| Rarity | `common,uncommon,rare,epic,legendary` | |
| Target type | `unit,point,direction,no_target,self` | Shared activation |
| Target team (activation) | `enemy,ally,both,self` | Maps to `TargetTeam` via `ACTION_TEAMS` |
| Target team (radius defaults) | `TARGET_TEAMS` | Effect polarity for `units_in_radius` on abilities |
| Projectile type | `none,targeted,skillshot` | **Abilities:** full support. **Items:** spec column; runtime/generator gap (see below) |
| Ability triggers | `on_cast`, `on_projectile_hit`, `on_unit_contact`, `on_impact`, `on_channel_tick`, `on_channel_end`, `on_expire`, or `on_<custom>` | Custom must match `^on_[a-z0-9_]+$` |
| Item triggers | `passive`, `on_use`, `on_attack`, `on_attack_hit`, `on_damage_taken`, `on_kill`, `on_death` | Cross-kind triggers rejected in `effect_to_rust` |
| Effect targets | `caster`, `owner`, `cast_target`, `trigger_unit`, `attacker`, `affected_units`, `units_in_radius` | `owner` → `EffectTarget::Caster` at emit time |
| Effect types | `damage`, `heal`, `apply_status`, `remove_status`, `dispel`, `movement`, `stat_modifier`, `custom` | |
| Damage type | `physical`, `magical`, `pure` | |
| Scaling | `strength`, `agility`, `intelligence`, `attack_damage`, `target_health_below` + `scaling_value` | Damage/heal only |

### Shared activation validation (target behavior)

Extract from `AbilityGenerator.parse_ability` and `ItemGenerator.parse_activation` into one function, parameterized by `kind: "ability" | "item"`:

- `target_team self` ⇔ `target_type` ∈ `{self, no_target}`
- `target_type self` ⇒ `target_team self`
- `projectile_speed` ⇒ `projectile_type` ∈ `{targeted, skillshot}`
- `targeted` ⇒ `target_type unit`; `skillshot` ⇒ `target_type` ∈ `{point, direction}`
- `charge_restore_time` ⇔ `max_charges` (abilities; extend to charge-based consumables when product needs it)
- `cast_range` required for `unit|point|direction` when not passive (abilities) / when `on_use` present (items)
- Teleport (`movement` + `effect_id=teleport`): destination from cast aim — abilities typically `on_cast`; items `on_use` with `target_type` `point` or `direction` (+ `cast_range`) — **already enforced** in `ItemGenerator.load`
- Passive-only items: no `on_use` / non-passive triggers except warnings for orphan activation columns
- Item passives: `trigger=passive` ⇒ `effect_type=stat_modifier` only

### Trigger / target static combos

Keep **`EffectContext`** per owner (`effect_context()` in each generator) but build invalid trigger/combo maps from shared tables in `generator_common.py` where possible:

- Ability: passive/toggle/projectile/channel guards (existing)
- Item: `cast_target` / `trigger_unit` / `affected_units` on `on_use` for non-unit actives (existing)
- Shared: teleport allowed targets (`TELEPORT_TARGETS`), `units_in_radius` requires radius or ability AoE / fixed radius

### Custom effect IDs

Today: `custom` requires snake_case `effect_id`; **no** check against `register_custom_effect` in Rust. Plan: optional `--repo-root` scan of `src/abilities/custom.rs` + test registrations, or a maintained `data/custom_effects.txt` list validated at `--check` time (warn if unregistered, error in CI when strict flag set).

---

## Current state vs target (gap analysis)

> Superseded by `2026-11-07-item-ability-architecture-cleanup.md` (2026-11-07): several gaps below are closed there. Where this table and the cleanup plan disagree, the cleanup plan and the code win.

### Already aligned

| Area | Evidence |
|------|----------|
| Effect row schema | `EFFECT_COLUMNS` shared; both generators call `parse_effect_rows` + `effect_to_rust` |
| Duplicate IDs / names | Per-file duplicate `id` and casefolded `name` checks in both loaders |
| Duplicate `effect_index` | `(owner_id, effect_index)` uniqueness in `parse_effect_rows` |
| Effect refs | Rows with unknown `ability_id` / `item_id` error |
| Enum validation | `parse_enum` for typed columns |
| Deterministic Rust blocks | `// <ability:id>` / `// <item:id>` merge via `read_blocks`; unknown blocks preserved |
| Auto-register IDs | `AbilityGenerator.patch_components`; `ItemGenerator.patch_items_rs` upsert enum + ordering regions |
| Auto-register completeness (items) | Every `ItemId` enum variant must have a CSV row or existing block |
| Ability ID variant collision | PascalCase collision detection for `AbilityId` |
| Examples + invalid fixtures | `data/examples/`, `data/examples/invalid/` documented in README |
| Teleport parity (data) | `data/examples/items.csv` + `item_effects.csv` blink_item; README Teleport section |

### Gaps (generator / data)

| Gap | Severity | Detail |
|-----|----------|--------|
| Duplicated activation validation | P1 | ~40 lines overlap between `parse_ability` and `parse_activation`; drift risk |
| Item activation columns not in `ITEM_COLUMNS` | P1 | Extra headers work via `DictReader`, but `--check` docs and strict header tests don’t list the target activation set |
| `charge_restore_time` on items | P2 | Spec column; not parsed; consumables use stack/charges differently from ability charge restore |
| Item `projectile_type` / `projectile_speed` | P2 | Not in `ACTIVATION_COLUMNS`; `ItemActivation` Rust struct has no projectile fields; runtime item projectiles not wired (see runtime audit) |
| Item `cooldown` vs ability rank cooldown | P1 | Item: scalar on item row; Ability: `RankValue` — intentional, document in shared activation struct |
| Mechanics inference | P1 | Only `AbilityGenerator.ability_fn` derives `Projectile`, `shockwave_ring`, `nova_ring`; items have no mechanic layer (effects-only on `OnUse`) |
| `ItemId` PascalCase collision | P2 | Abilities check variant collisions; items do not |
| Custom effect registry | P2 | Validation is syntactic only |
| Production CSV drift | P0 doc | `data/abilities.csv` includes optional `cast_backswing,max_rank`; slots use `Q/W/E/R` (OK). `data/items.csv` ** lacks activation headers** — all actives in repo may be hand-written in `item_catalog.rs`. `blink_item` exists only under `data/examples/`. |
| Hand-written vs generated Blink | P1 product | Skirmisher **Blink** in `catalog.rs` uses `AbilityMechanic::Dash`, not CSV `Teleport`; tests use synthetic `AbilityEffect::Teleport`. Migration to shared teleport effect is a **content + runtime** follow-up, not a second generator. |
| Level lists on items | P2 | Abilities support `\|` ranks; item activation scalars are single-value — document as intentional unless items gain ranks |

### Runtime blockers (out of generator scope but affects column spec)

From shared-effects pipeline audit — implementers should not emit CSV that requires unsupported runtime:

| Feature | Generator today | Runtime |
|---------|-----------------|--------|
| Item `on_channel_end` | Can author rows | Not emitted for items |
| Item projectiles / `on_projectile_hit` | Ability-only triggers rejected for items | `ProjectilePayload` ability-scoped |
| `on_damage_taken` / `on_kill` / `on_death` | Warn only (`NOT_EMITTED`) | Not emitted |

Generators should **keep** validating these as warnings until runtime lands, then upgrade to errors or full support in lockstep.

---

## Architecture: one shared module, two thin generators

```text
                    ┌─────────────────────────────────────┐
                    │       generator_common.py           │
                    │  CSV I/O, Report, enums, EFFECT_*   │
                    │  parse_effect_rows, effect_to_rust  │
                    │  parse_activation_spec (NEW)        │
                    │  validate_activation (NEW)          │
                    │  shared EffectContext helpers (NEW) │
                    │  region patch, rustfmt, blocks      │
                    └──────────────┬──────────────────────┘
           ┌───────────────────────┴───────────────────────┐
           ▼                                               ▼
  AbilityGenerator.py                              ItemGenerator.py
  ABILITY_COLUMNS + ranks + mechanics              ITEM_COLUMNS + economy
  → generated.rs + AbilityId patches               → item_catalog.rs + ItemId patches
           │                                               │
           └───────────────────────┬───────────────────────┘
                                   ▼
              AbilityDefinition / ItemDefinition
              both use AbilityEffectEntry + same triggers executor (Rust)
```

**Non-goals:** Separate item effect types, duplicate validation scripts, or a third “unified” CSV that merges abilities and items (keep two owner tables joined to one effect schema).

### Proposed `generator_common` additions

1. **`ACTIVATION_SCALAR_COLUMNS`** — tuple shared by docs and item optional headers.  
2. **`@dataclass ActivationSpec`** — `target_type`, `target_team`, `cast_range`, `cast_time`, `channel_time`, `mana_cost`, `max_charges`, `charge_restore_time`, `projectile_type`, `projectile_speed`, plus ability-only `aoe_radius` and rank lists where applicable.  
3. **`parse_activation_spec(row, *, kind, has_active_effects, report) -> ActivationSpec | None`**  
4. **`validate_activation(spec, *, kind, behavior, report)`** — unified rules above.  
5. **`ability_mechanic_from_spec(spec) -> Rust fragments`** — stay in AbilityGenerator but consume `ActivationSpec`.  
6. **`item_activation_rust(spec, cooldown: float) -> ItemActivation`** — ItemGenerator; map cooldown from item row + spec.

---

## Backward compatibility

| Change | Strategy |
|--------|----------|
| New item CSV columns | **Do not** add activation columns to `ITEM_COLUMNS` required tuple until a migration PR adds headers to `data/items.csv` (blank cells OK). `read_csv` only enforces required headers. |
| Existing optional item columns | Unchanged; generator continues to accept rows with only legacy headers. |
| Ability optional columns | `cast_backswing`, `max_rank` remain optional. |
| Legacy hero/item CSV formats | `legacy_hint` on `read_csv` when old column names detected. |
| Generated Rust blocks | Re-run overwrites only IDs present in CSV; other blocks kept. |
| Hand-written catalog entries | Still override until same `AbilityId` appears in generated CSV (abilities) or item row added (items). |

---

## Migration steps (implementation PR sequence)

1. **Refactor (no CSV change)** — Introduce `ActivationSpec` + shared validation in `generator_common.py`; wire AbilityGenerator and ItemGenerator; run `--check` on `data/` and `data/examples/` — output must match pre-refactor (determinism test).  
2. **Header migration** — Add activation columns to `data/items.csv` with empty values for passive equipment; document in README table.  
3. **Port actives** — For each active item in `item_catalog.rs` without CSV row, add row + effects (e.g. blink dagger when product wants it in shop).  
4. **Blink exemplar** — Add CSV ability `blink` (or migrate Skirmisher slot) with `on_cast` + `teleport` + point targeting; add `blink_dagger` to production items mirroring `data/examples/`; align runtime hero kit to generated defs when ready.  
5. **Validation tightening** — Item PascalCase collision check; optional strict custom-effect registry; promote selected warnings to errors when runtime implements triggers.  
6. **Item projectile column (optional)** — Only after `ItemActivation` + action pipeline support projectiles: extend `ItemGenerator` to emit mechanics or reject non-`none` with clear error.  
7. **CI** — `python3 scripts/AbilityGenerator.py --check && python3 scripts/ItemGenerator.py --check` on every PR touching `data/*.csv` or generators.

---

## Test plan

### Generator / validation

| Test | Method |
|------|--------|
| Regression `--check` | Snapshot exit code 0 on `data/abilities.csv`, `data/items.csv`, all `data/examples/*.csv` |
| Invalid fixtures | Each file under `data/examples/invalid/` must produce ≥1 error and exit 1 |
| Determinism | Run generator twice on unchanged CSV; `git diff` empty on `generated.rs` / `item_catalog.rs` |
| Shared validation | Unit tests (pytest) for `validate_activation`: teleport+point, projectile+unit, passive violations |
| Duplicate detection | Tests for duplicate id, name, effect_index, ability AbilityId collision |
| Blink parity row | `--dry-run` on example CSVs: ability `on_cast` + item `on_use` both emit `AbilityEffect::Teleport` |
| Trigger cross-kind | Item row with `on_cast` errors; ability row with `passive` errors |

### Integration (existing Rust; extend)

| Test | Location |
|------|----------|
| Item on_use pipeline | `abilities/mod.rs` `item_on_use_resolves_through_the_shared_trigger_pipeline` |
| Teleport same effect | `items_and_abilities_teleport_through_the_same_effect` |
| Full item activation | Add test driving `blink_item` from generated catalog through activation + `resolve_item_uses` once in production CSV |

### Manual

```bash
python3 scripts/AbilityGenerator.py --check
python3 scripts/ItemGenerator.py --check
python3 scripts/AbilityGenerator.py data/examples/abilities.csv data/examples/ability_effects.csv --dry-run
python3 scripts/ItemGenerator.py data/examples/items.csv data/examples/item_effects.csv --dry-run
```

---

## Current CSV drift (not fixed in this PR)

- `data/abilities.csv`: 5 abilities; `overcharge` has no effect rows (warning). Optional columns `cast_backswing`, `max_rank` present.  
- `data/ability_effects.csv`: 6 effect rows; matches schema.  
- `data/items.csv`: 59 items; **no** activation columns; actives defined only in generated/hand-written catalog.  
- `data/item_effects.csv`: 84 rows; mostly `passive` + `owner` + `stat_modifier`.  
- `data/examples/`: canonical blink_item + shockwave/skewer; should stay in sync when shared validation changes.

---

## Risks

| Risk | Mitigation |
|------|------------|
| Refactor changes emitted Rust whitespace | Determinism test + rustfmt |
| Stricter shared validation breaks legacy rows | Phased rollout; fix CSV in same PR as validation |
| Item projectile columns imply runtime support | Gate emission behind feature + runtime PR |
| Dash vs Teleport for hero Blink | Explicit content decision in migration step 4 |

---

## Blockers

| Blocker | Owner |
|---------|--------|
| `.cursor/agents/architect.md` missing from repo | Process/docs — this plan follows user message + README |
| Item channel end / item projectiles | Runtime (see shared-effects pipeline doc) before CSV promises |
| Skirmisher Blink still `Dash` mechanic in `catalog.rs` | Game design + hero migration to CSV teleport |
| Strict custom-effect ID validation | Needs registry convention (scan vs manifest) |

---

## Files touched by future implementation (expected)

- `scripts/generator_common.py` — shared activation + validation  
- `scripts/AbilityGenerator.py` — thin wrapper, mechanics from `ActivationSpec`  
- `scripts/ItemGenerator.py` — extended activation columns, shared validation  
- `data/items.csv` — header extension + active item rows  
- `data/abilities.csv` / effect CSVs — as content migrates  
- `README.md` — single table of shared enums (dedupe ability vs item sections)  
- **Not in generator PR:** `src/items.rs` (`ItemActivation` projectiles), `src/abilities/casting.rs` (item channel triggers)

---

## Acceptance criteria for generator unification epic

1. One effect schema and one `effect_to_rust` path (already true; must stay true).  
2. One activation validation implementation used by both generators.  
3. Documented item CSV headers match spec; production CSV includes columns (may be blank).  
4. `--check` covers duplicates, refs, enums, trigger/target combos, passive-only rules, teleport rules.  
5. Blink ability + Blink Dagger example in `data/examples/` passes `--check` and emits identical teleport effect AST.  
6. No parallel generated-content system introduced.
