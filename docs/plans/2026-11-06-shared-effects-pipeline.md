# Shared effects pipeline — architecture audit & implementation spec

**Date:** 2026-11-06  
**Scope:** Audit only; no production code changes in this pass.  
**Goal:** Confirm abilities and active items share effect definitions and execution, and specify minimal follow-up work.

---

## Executive conclusion

The codebase **already implements the desired pipeline** with one shared trigger bus and one shared executor. Items reuse `AbilityEffect`, `AbilityEffectEntry`, and `AbilityTrigger` from `abilities/definition.rs`; `resolve_ability_triggers` + `EffectRunner` handle both `EffectSource::Ability` and `EffectSource::Item`. Generic effects emit shared gameplay messages consumed by `combat`, `items` (status application), `displacement`, and `teleport`.

**Recommendation:** **Do not** introduce a parallel `src/effects/` tree or duplicate executors. Treat `abilities/effects.rs` + `abilities/definition.rs` as the de facto shared effects module. Optional later work: rename types for clarity (`GameplayEffect`, `TriggerEvent`) and add a thin `crate::effects` re-export module — cosmetic only.

---

## Target pipeline vs current code

| Stage | Target | Current location | Status |
|-------|--------|------------------|--------|
| Ability / Item activation | Shared activation | `src/actions/` (`ActivationRequest` → `ActionFiredEvent` / `ActionChannelEndEvent`) | **Done** |
| Source-specific commit | Ability rank / item instance | `abilities/casting.rs`, `items.rs` (`commit_*`, `resolve_item_uses`) | **Done** |
| Mechanic (abilities) | WHEN / WHO | `abilities/mechanics.rs` → `AbilityCastEvent` | **Abilities only** |
| Trigger + context | Unified context | `abilities/effects.rs` (`TriggerContext`, `AbilityTriggerEvent`) | **Done** |
| Effect target resolution | Shared resolver | `EffectRunner::targets` | **Done** |
| Effect execution | Single executor | `EffectRunner::execute`, `resolve_ability_triggers` | **Done** |
| Gameplay systems | No duplicated logic | `combat`, `items` (status), `displacement`, `teleport` | **Done** |

```text
ActivationRequest (ActionSource::Ability | Item)
  → actions/pipeline.rs (spec, ready, target, range, cast point, channel)
  → ActionFiredEvent
       ├─ emit_ability_casts → AbilityCastEvent → mechanics → AbilityTriggerEvent
       └─ resolve_item_uses → AbilityTriggerEvent(OnUse) [+ item FX only]
  → ActionChannelEndEvent → ability_channel_end_triggers → OnChannelEnd (abilities only today)
  → ProjectileHitEvent → projectile_hit_triggers → OnProjectileHit | OnImpact
  → resolve_ability_triggers (entries for ctx.source + ctx.trigger)
  → EffectRunner::targets + execute
  → DamageEvent | HealEvent | ManaRestoreEvent | StatusEffectEvent | DisplacementEvent | TeleportEvent | Custom systems
  → apply_* systems (combat, effects.rs heal/mana/status, displacement, teleport)
```

---

## Code path trace (file references)

### Activation (shared)

- **`src/actions/mod.rs`** — `ActivationRequest`, `ActionSpec`, `ActionFiredEvent`, `ActionChannelEndEvent`, plugin messages.
- **`src/actions/pipeline.rs`** — `ActionSources` (ability vs item `spec` / `commit`), `validate_activation_requests`, `tick_activations`, channel end emission.
- **`src/actions/targeting.rs`** — `TargetType`, `TargetTeam`, `resolve_target`, `in_range`, `team_allows`.

### Ability layer

- **`src/abilities/casting.rs`** — `ability_action_spec`, `commit_ability`, `emit_ability_casts`, `ability_channel_end_triggers` (`OnChannelEnd`).
- **`src/abilities/mechanics.rs`** — `CastContext`, `MechanicRunner`, `execute_ability_casts` (`OnCast`), `projectile_hit_triggers`.
- **`src/abilities/custom.rs`** — custom behaviors emit `AbilityTriggerEvent` (e.g. `Custom("on_skewer_end")`).

### Item layer

- **`src/items.rs`** — `ItemDefinition.effects: &[AbilityEffectEntry]`, `item_action_spec`, `resolve_item_uses` (`OnUse`), `item_attack_triggers` (`OnAttack`, `OnAttackHit`), passive stats via `Passive` trigger (inventory, not trigger pipeline).

### Definitions & data

- **`src/abilities/definition.rs`** — `AbilityTrigger`, `EffectTarget`, `AbilityEffect`, `AbilityEffectEntry`, `AbilityMechanic`, scaling, status specs.
- **`src/abilities/catalog.rs`**, **`src/abilities/generated.rs`**, **`src/item_catalog.rs`** — compiled definitions.
- **`scripts/generator_common.py`**, **`scripts/AbilityGenerator.py`**, **`scripts/ItemGenerator.py`** — CSV → Rust; item `owner` → `EffectTarget::Caster`.

### Effects (shared executor)

- **`src/abilities/effects.rs`** — `EffectSource`, `TriggerContext`, `AbilityTriggerEvent`, `EffectRunner`, `resolve_ability_triggers`, heal/mana/status appliers, `CustomAbilityEffects`.

### Gameplay consumers (authoritative)

- **`src/combat.rs`** — `DamageEvent`, projectiles, `ProjectileHitEvent`, skillshot per-unit dedup (`Skillshot.hit`).
- **`src/displacement.rs`** — `DisplacementEvent` → forced movement + root via status event.
- **`src/teleport.rs`** — `TeleportEvent` → validation, order cleanup, completion/rejection events.
- **`src/items.rs`** — `StatusEffect` / `apply_status` (also used from `apply_status_effect_events`).

### Scheduling (ordering)

- **`src/abilities/mod.rs`** — Update chain: activation → `emit_ability_casts` → `resolve_item_uses` → `execute_ability_casts` → `ability_channel_end_triggers` → `projectile_hit_triggers` → `resolve_ability_triggers` → gameplay appliers (teleport before AI per comment).

### Existing tests proving sharing

- **`src/abilities/mod.rs`** — `item_on_use_resolves_through_the_shared_trigger_pipeline`, `items_and_abilities_teleport_through_the_same_effect`, channel `OnChannelEnd` heal.
- **`src/items.rs`** — activation pipeline integration tests with `resolve_ability_triggers`.

---

## Requirements mapping

### Triggers

| Trigger | Enum | Emitted today | Notes |
|---------|------|---------------|-------|
| `on_cast` | `OnCast` | `mechanics::execute_ability_casts` | Sets `trigger_unit` = unit target when present |
| `on_use` | `OnUse` | `items::resolve_item_uses` | |
| `on_projectile_hit` | `OnProjectileHit` | `projectile_hit_triggers` | Ability projectiles only (`ProjectilePayload.ability`) |
| `on_impact` | `OnImpact` | `projectile_hit_triggers` | |
| `on_unit_contact` | `OnUnitContact` | **Not wired in stock mechanics** | Custom mechanics should emit; generator supports CSV |
| `on_channel_end` | `OnChannelEnd` | `ability_channel_end_triggers` | **Abilities only**; channeled items do not get this yet |
| `on_channel_tick` | `OnChannelTick` | **Not wired** | Enum + docs only |
| Custom | `Custom("…")` | Custom ability behaviors | |
| Item-only | `OnAttack`, `OnAttackHit`, … | `items::item_attack_triggers` | Dedup by distinct `ItemId` per inventory |

### Targets

| Requirement | Runtime | Notes |
|-------------|---------|-------|
| `caster` | `EffectTarget::Caster` | |
| `owner` (items) | **Same as Caster** | CSV maps `owner` → `Caster` in `generator_common.py` |
| `cast_target` | `CastTarget` | |
| `trigger_unit` | `TriggerUnit` | |
| `affected_units` | `AffectedUnits` | Splash list on projectile hits |
| `units_in_radius` | `UnitsInRadius` | Items use `aoe_radius: 0.0` in resolver unless `AreaRadius::Fixed` on entry |

### Generic effects

All implemented on `AbilityEffect` and executed only via event emission in `EffectRunner::execute`:

- Damage → `DamageEvent` (`combat::apply_damage_events`)
- Heal / mana → `HealEvent`, `ManaRestoreEvent`
- Status → `StatusEffectEvent` → `apply_status_effect_events` → `items::apply_status`
- Movement → `DisplacementEvent`
- Teleport → `TeleportEvent`
- Stat modifiers (passive) → `PassiveStat` + inventory `apply_passives` (not trigger-fired)
- Custom → registered systems; must emit shared events

### Source attribution

- **Present:** `TriggerContext.source: EffectSource::Ability(AbilityId) | Item(ItemId)` on every trigger.
- **Partial:** `DamageEvent.source` is `Option<Entity>` (caster entity only), not ability/item id. Kill credit / on-hit item rules may need richer attribution later.

### Duplicate execution prevention

- **Skillshots:** `combat::Skillshot.hit` ensures one `ProjectileHitEvent` per enemy per projectile (`skillshot_contacts`).
- **Splash:** primary excluded from `splashed` (`apply_projectile_hits`).
- **Item attack triggers:** one event per distinct `ItemId` in inventory (`seen` list in `item_attack_triggers`).
- **Not present:** effect-layer dedup if multiple entries in one trigger pass hit the same target (usually intentional, e.g. damage + slow on same `OnProjectileHit`).
- **Not present:** idempotency tokens for custom multi-frame triggers (implementer choice per ability).

### Single executor

- **Satisfied:** Only `EffectRunner::execute`; no item-specific effect runner.

---

## Gap analysis

> Superseded by `2026-11-07-item-ability-architecture-cleanup.md` (2026-11-07): several gaps below are closed there. Where this table and the cleanup plan disagree, the cleanup plan and the code win.

### P0 — Document / clarify (no behavior change)

1. **Naming** — Shared concepts live under `abilities::*` (`AbilityTriggerEvent`, `AbilityEffect`, `resolve_ability_triggers`). README already describes cross-item use; code readers may assume ability-only.
2. **`owner` target** — Alias of `caster` in data; no separate `EffectTarget::Owner`.

### P1 — Functional gaps (small diffs)

1. **Channeled item `OnChannelEnd`** — `ability_channel_end_triggers` filters `ActionId::Ability` only. Channeled actives with `OnChannelEnd` rows never fire. **Fix:** generalize to `action_channel_end_triggers` (or extend current system) building `TriggerContext` with `EffectSource::Item(item)` from `ActionFiredEvent`/`ActionChannelEndEvent` item branch.
2. **`OnChannelTick`** — No emitter during `Activation.channel_remaining`. Needed only when CSV/data uses channel ticks.
3. **`OnUnitContact`** — No default mechanic emits it; skewer-style abilities use `Custom` triggers. Either document “custom only” or add a reusable contact mechanic helper.
4. **Item projectiles** — `ProjectilePayload` is ability-scoped; items cannot use `OnProjectileHit` without extending payload to `EffectSource` (or a neutral `TriggerSource` id).

### P2 — Enhancements (optional)

1. **`DamageEvent` attribution** — Add optional `EffectSource` (or ability/item ids) for combat log, lifesteal, damage reflection.
2. **`src/effects/` module** — Thin facade re-exporting `definition` effect types + `effects::EffectRunner` to reduce cognitive load; move files only if team wants physical separation.
3. **Rename pass** — `AbilityEffect` → `GameplayEffect`, `AbilityTriggerEvent` → `TriggerEvent`, `resolve_ability_triggers` → `resolve_trigger_effects` with type aliases for one release.

---

## Recommendation: refactor vs consolidate

**Consolidate in place (preferred).** The shared pipeline is already centralized in:

- Types: `abilities/definition.rs`
- Trigger bus + executor: `abilities/effects.rs`
- Activation: `actions/`
- Gameplay: `combat`, `displacement`, `teleport`, status helpers in `items`

Creating a new `src/effects/` directory and moving ~800 lines yields **high churn** for **zero new capability**. If a dedicated module is desired:

```text
Phase A (1 PR): src/effects/mod.rs re-exports only; no moves.
Phase B (optional): move effects.rs + effect-related definition types; update imports + generators.
```

Skip Phase B unless naming confusion blocks contributors.

---

## Proposed interfaces (stable contracts)

Implementers should treat these as the public boundary between data, triggers, and gameplay.

### `TriggerContext` (unchanged)

```rust
pub struct TriggerContext {
    pub trigger: AbilityTrigger,
    pub caster: Entity,           // caster or item owner
    pub team: Team,
    pub source: EffectSource,     // Ability(id) | Item(id)
    pub rank: u32,
    pub origin: Vec3,
    pub aim: Vec3,
    pub point: Vec3,
    pub cast_target: Option<Entity>,
    pub trigger_unit: Option<Entity>,
    pub affected_units: Vec<Entity>,
}
```

### `EffectRunner` (unchanged)

- `targets(&TriggerContext, EffectTarget, aoe_radius: f32) -> Vec<Entity>`
- `execute(&mut self, &TriggerContext, &AbilityEffect, targets: Vec<Entity>)`

### Entry resolution (unchanged)

```rust
fn resolve_entries(ctx: &TriggerContext, defs: &AbilityDefinitions) -> (&[AbilityEffectEntry], f32) {
    match ctx.source {
        EffectSource::Ability(id) => defs.get(id).map(|d| (&d.effects[..], d.aoe_radius.at(ctx.rank))).…,
        EffectSource::Item(id) => (id.definition().effects, 0.0),
    }
}
```

### New (P1): channel end for items

```rust
// abilities/casting.rs or effects/triggers.rs
pub fn action_channel_end_triggers(
    mut ends: MessageReader<ActionChannelEndEvent>,
    casters: Query<(&Transform, &Team)>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for end in ends.read() {
        let source = match end.action {
            ActionId::Ability(id) => EffectSource::Ability(id),
            ActionId::Item { item, .. } => EffectSource::Item(item),
            _ => continue,
        };
        // … same TriggerContext as today, trigger: OnChannelEnd
    }
}
```

---

## Migration steps (implementation order)

1. **Tests first** — Add cross-source parity tests (see below); generalize channel-end trigger test for a channeled item stub.
2. **P1 channel end** — Replace ability-only channel end handler with action-wide handler; keep schedule slot in `AbilitiesPlugin`.
3. **P1 channel tick (if needed)** — In `tick_activations`, on channel interval or each frame, emit `OnChannelTick` with throttling policy (document tick rate).
4. **Docs / README** — One paragraph: “effects module is shared; `Ability*` names are historical.”
5. **Optional re-export** — `src/effects/mod.rs` + `lib.rs`/`main.rs` path if crate structure allows.
6. **Projectile payload (if item projectiles required)** — Introduce `TriggerSource { caster, source: EffectSource, rank, … }` on projectiles; update `projectile_hit_triggers`.

**Do not** duplicate damage/heal/status logic outside existing appliers.

---

## Invariants

1. Mechanics and item use handlers **must not** mutate `Health`, `CombatStats`, or apply statuses directly except presentation-only FX (`spawn_item_use_fx`).
2. All trigger-fired consequences go through **`AbilityTriggerEvent` → `resolve_ability_triggers`** (single reader ordering in schedule).
3. **`EffectRunner::execute`** is the only place generic `AbilityEffect` variants become gameplay messages.
4. Passive item stats stay on **`Passive` + inventory`**, not the trigger pipeline.
5. Activation costs (mana, cooldown) are paid in **`actions/pipeline`** at fire time; effects assume payment already happened.
6. Teleport and displacement remain authoritative in **`teleport`** / **`displacement`** systems.
7. Custom effect systems registered via **`register_custom_effect`** must emit shared events, not bypass mitigation/immunity.

---

## Test plan (for implementer)

| Test | Purpose |
|------|---------|
| **Parity: damage** | Same `AbilityEffect::Damage` row on ability `OnCast` and item `OnUse`; assert identical `DamageEvent` amounts after mitigation stub. |
| **Parity: status** | Apply slow/stun via ability projectile trigger vs item `OnUse`; assert same `StatusEffectEvent` → stats. |
| **Parity: teleport** | Extend `items_and_abilities_teleport_through_the_same_effect` to run full item activation through `resolve_item_uses` + trigger pipeline (not only direct `EffectRunner`). |
| **Channel end item** | Channeled item definition with `OnChannelEnd` heal; assert one heal after channel completes, none on cancel. |
| **No double skillshot hit** | Existing combat test; keep as regression for projectile dedup. |
| **Item attack dedup** | Two stacks same `ItemId` → one `OnAttackHit` trigger per id (existing behavior; assert explicitly). |
| **Silence** | Ability blocked, item fires (existing `silence_blocks_an_ability_while_an_item_still_fires`). |

Place integration tests in **`abilities/mod.rs`** (full app) or **`items.rs`** (`item_app`) consistent with existing patterns.

---

## Risks

| Risk | Mitigation |
|------|------------|
| Event ordering (triggers before damage applied same frame) | Keep current chain; tests use multi-step `step()` |
| Renaming types breaks generators | Defer rename; aliases only |
| Item channel end double-fires | Fire only on `ActionChannelEndEvent`, not on `ActionFiredEvent` |
| `OnChannelTick` spam | Throttle (e.g. 0.1s) or once-per-frame documented |
| Moving files breaks PR stack | Re-export-only phase |
| Projectile payload ability-only | Feature-flag item projectiles until payload generalized |

---

## Open decisions

1. **Physical `src/effects/` move** — Team preference: re-export only vs full move?
2. **`OnChannelTick` semantics** — Every frame vs fixed interval vs manual in custom channel abilities?
3. **`DamageEvent` enrichment** — Is caster entity enough for combat rules long-term?
4. **Item skillshots / projectiles** — Required for parity with abilities, or out of scope?
5. **Rename `Ability*` shared types** — Worth a breaking rename for clarity?

---

## Blockers for implementer

- None for parity tests and item channel-end fix (paths are clear).
- **Item projectile + `OnProjectileHit`** blocked on **`ProjectilePayload` redesign** (P1/P2 depending on product need).
- **`OnUnitContact` stock mechanic** needs design if generator-authored contact abilities should not require Rust custom behaviors.

---

## Affected files (expected touch list for P1)

- `src/abilities/casting.rs` — channel end generalization
- `src/abilities/mod.rs` — system rename/wiring, tests
- `src/items.rs` — optional channeled item test fixture
- `README.md` — clarify shared effects ownership (optional)
- **Not required:** new `src/effects/` unless Phase A re-export is chosen
