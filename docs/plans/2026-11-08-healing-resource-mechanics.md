# Healing, health and resource mechanics: audit and implementation plan

**Date:** 2026-11-08
**Status:** Approved by the coordinator with all recommended decisions (D1 to D9) on 2026-11-08. The architect is read-only and did not run `cargo test`; Step 0 records the baseline.
**Based on:**
- `docs/plans/2026-11-06-shared-effects-pipeline.md`: a single executor (`EffectRunner`), and effects only emit messages.
- `docs/plans/2026-11-06-damage-mechanics.md`: `resolve_incoming_damage`, `DamageEvent` metadata, and amps on `CombatStats`.
- `docs/plans/2026-11-06-status-effects-catalog.md`: `StatusEffect`/`StatusSpec`/`STATUSES` is the narrow waist.
- `docs/plans/2026-11-07-movement-mechanics.md`: format, one commit per step, hand-listed test apps.
- `docs/plans/2026-11-07-item-ability-architecture-cleanup.md`: the **V** commands and invariants.

**Scope:** one PR with one commit per step. Every step compiles and passes **V** on its own.

---

## Executive summary

Labyrinth already has the right spine for this work:
- `AbilityEffect::Heal` / `RestoreMana` → `HealEvent` / `ManaRestoreEvent` → `apply_heal_events` / `apply_mana_restore_events` (`abilities/effects.rs:115-125, 347-367, 534-551`).
- Passive max/regen bonuses go through `PassiveStat` → `ItemPassives` → `adjust_passives` (`items.rs:735-941`).
- Regen ticks in `progression.rs:35` (health) and `abilities/mod.rs:169` (mana).
- All damage meets in one place, `combat::apply_damage_events` (`combat.rs:251-356`).

The gaps:
- **Heal resolution is just a clamp.** `current = (current + amount).min(max)` appears in three places (`effects.rs:537`, `status.rs:509`, `progression.rs:42`). None of them has amplification, reduction or prevention.
- **Two of those places heal dead units.** `apply_heal_events` and the HoT tick don't check that the target is alive. Heroes are never despawned (`despawn_dead` is `Without<PlayerHero>`, `combat.rs:711`), so an ally heal currently brings a dead hero back. This is a real bug.
- **HoT is dead code.** `StatusEffect.heal_per_sec` exists (`status.rs:96, 508`), but no `StatusSpec`, generator row or producer sets it.
- **Missing entirely:** lifesteal, mana burn/drain/shield, cost modification, conversion and sacrifice.
- **Timed regen and max modifiers are missing.** Only permanent passives exist.

**Recommendation:**
1. Add one small module, `src/vitals.rs`, holding the **central heal function** and **resource-modifier lookup**. Every heal path calls it: direct, HoT, regen, lifesteal and conversion.
2. Keep `combat.rs` authoritative for anything that comes from damage: mana shield, lifesteal, spell lifesteal and health drain. They are added as two short steps inside `apply_damage_events`.
3. Express new mana flows (burn, drain, spend, convert) as one new `ResourceDrainEvent`, applied in the existing applier chain before `apply_damage_events`.
4. Timed modifiers are `StatusEffect` fields that are **summed when read** (like `effective_move_speed`). They are never pushed into stats, so no removal path can leak a stat.
5. Max health/mana from statuses is reconciled by one sync system. It follows the existing `sync_ability_passives` pattern.

This adds no new triggers and no new CSV columns, and production CSVs are untouched.

---

## Audit

Legend: **Done** = works as named. **Partial** = there is a hook, but the semantics are incomplete. **Missing** = no support.

| # | Mechanic | Status | Where today | Gap |
|---|---|---|---|---|
| 1 | Instant heal | **Partial** | `AbilityEffect::Heal` (`definition.rs:473`), `EffectRunner` (`effects.rs:347-359`), `apply_heal_events` (`effects.rs:534-540`), generator `heal` (`generator_common.py:856-868`), Vial of Light (`item_catalog.rs:295`) | Heals dead units. No modifiers. No overheal report |
| 2 | Heal over time | **Partial** | `StatusEffect.heal_per_sec` + tick (`status.rs:96, 508-510`) | No `StatusSpec`, no generator status, nothing produces it. Heals dead units. The example `healing_salve` ("over time") is authored as an instant 250 heal (`data/examples/items.csv:4`, `item_effects.csv:7`) |
| 3 | Health regeneration | **Partial** | `Health.regen_per_sec` (`components.rs:25`), `regen_health` (`progression.rs:35-45`), `PassiveStat::HealthRegen` (`items.rs:761, 925`), Strength (`components.rs:279`) | No timed modifier. Ignores reduction and prevention. `.max(0.0)` makes negative passives irreversible (`items.rs:925`). Not gated by sim authority (`progression.rs:13`) |
| 4 | Lifesteal | **Missing** | Only a fixed-amount composition, `OnAttackHit` heal (test item `abilities/mod.rs:765`, example `battle_horn`) | Nothing proportional to damage dealt |
| 5 | Spell lifesteal | **Missing** | none | |
| 6 | Healing amplification | **Missing** | none | |
| 7 | Healing reduction | **Missing** | none | |
| 8 | Healing prevention | **Missing** | none | |
| 9 | Health transfer | **Missing** | Test-only custom `drain` effect (`abilities/mod.rs:1490-1520`) | |
| 10 | Health sacrifice | **Missing** | `ActionCost` is mana-only (`actions/resources.rs:8-21`) | |
| 11 | Mana restoration | **Done** | `AbilityEffect::RestoreMana` (`definition.rs:477`), `effects.rs:360-367, 542-551`, generator `heal,mana` (`generator_common.py:863`), Spark Pendant (`item_catalog.rs:534`) | Not gated on being alive (minor) |
| 12 | Mana regeneration | **Partial** | `Mana.regen_per_sec`, `regen_mana` (`abilities/mod.rs:169-174`), `PassiveStat::ManaRegen`, Intelligence | No timed modifier. Same irreversible clamp (`items.rs:929`) |
| 13 | Mana burn | **Missing** | none | |
| 14 | Mana drain | **Missing** | Toggle upkeep is a self-cost only (`abilities/passive.rs:127-133`) | |
| 15 | Mana cost modification | **Missing** | Costs come straight from the definitions (`casting.rs:61-63`, `items.rs:471-473`) | |
| 16 | Mana shield | **Missing** | `ShieldPool` is HP only (`components.rs:112`, `combat.rs:228-237`) | |
| 17 | Resource conversion | **Missing** | none | |
| 18 | Max health/mana modifier | **Partial** | `PassiveStat::MaxHealth/MaxMana` in `adjust_passives` (`items.rs:922-928`): a gain adds to current, a loss clamps. Attributes (`components.rs:276-310`) | No timed modifier. The rule is copy-pasted in two places |

---

## Design

### Ownership (unchanged boundaries)

- `EffectRunner::execute` stays the only place where an `AbilityEffect` becomes a message.
- `combat::apply_damage_events` is the only place that turns damage into a change in health, mana-shield mana or leech.
- `vitals.rs` (new) owns the **rules**: heal factor, resource stat lookup, max-change rule, and effective mana cost. It also owns the appliers for its own events.
- `status.rs` owns durations, dispel and debuff immunity for every timed modifier. Heal reduction and heal block are debuffs, so debuff immunity blocks them for free.

### 1. Data (`src/vitals.rs`, `src/status.rs`, `src/components.rs`)

```rust
// vitals.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind { Health, Mana }

/// Timed (status) or permanent (CombatStats) resource modifiers. Amps/ratios are fractions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceStat {
    Lifesteal, SpellLifesteal,
    HealAmp,          // healing provided
    HealReceivedAmp,  // healing received
    HealthRegen, ManaRegen, MaxHealth, MaxMana,
    ManaCostAmp,      // −0.25 = 25% cheaper
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ManaShield { pub absorb: f32 /*0..1*/, pub damage_per_mana: f32 /*>0*/ }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealKind { Direct, OverTime, Leech, Regen, Conversion }

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HealOutcome { pub applied: f32, pub overheal: f32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxChangeRule { KeepMissing /* existing passive rule */, PreserveRatio }
```

**New `StatusEffect` fields** (`status.rs:73-121`, defaults in `StatusEffect::buff`):
- `resource: Option<(ResourceStat, f32)>`
- `heal_reduction: f32` (0..1, the strongest one wins)
- `heal_blocked: bool`
- `mana_shield: Option<ManaShield>`
- `heal_per_sec` is reused for HoT.

**New queries on `StatusEffects`:** `is_heal_blocked()`, `heal_reduction()` (max over active effects), `mana_shield()` (the active shield with the highest `absorb`), and `resource_sum(stat)`.

**New constructors:**
- `StatusEffect::heal_over_time(hps, d)` (id `heal_over_time`, Buff)
- `heal_reduction(frac, d)` (Debuff)
- `heal_block(d)` (Debuff)
- `mana_shield(ms, d)` (Buff)
- `resource_modifier(id, kind, stat, v, d)`

All of them use `DispelType::Basic`.

**New `CombatStats` fields** (`components.rs:69-109`; the only literals are `components.rs:129` and `heroes.rs:334`): `lifesteal`, `spell_lifesteal`, `heal_amp`, `heal_received_amp`, `mana_cost_amp`, all defaulting to `0.0`.

**New `PassiveStat` variants** (`definition.rs:439`): `Lifesteal`, `SpellLifesteal`, `HealAmp`, `HealReceivedAmp`, `ManaCostAmp`. They are mapped in `ItemPassives::add_stat` / `AddAssign` / `adjust_passives` (`items.rs:735-941`), next to `spell_amp`.

**New `StatusSpec` variants** (`definition.rs:212`) and the matching arms in `build`:
- `HealOverTime { hps }`
- `HealReduction { percent }`
- `HealBlock`
- `ManaShield { absorb_percent, damage_per_mana }`
- `ResourceModifier { id, kind, stat: ResourceStat, amount }`

**New `AbilityEffect` variants** (`definition.rs:467`). Only variants are added, so no existing generated literal breaks.

```rust
/// Damage that heals the caster for `transfer_ratio × HP actually removed` (health transfer).
DrainHealth { amount: RankValue, damage_type: DamageType, scaling: EffectScaling, transfer_ratio: f32 },
/// Remove mana from each target. `burn_ratio × removed` is dealt as damage (mana burn),
/// `transfer_ratio × removed` is given to the caster (mana drain).
DrainMana { amount: RankValue, burn_ratio: f32, damage_type: DamageType, transfer_ratio: f32 },
/// The target spends its own resource (health never below 1); optionally gains `ratio × spent`
/// of another (sacrifice / conversion).
SpendResource { resource: ResourceKind, amount: RankValue, convert_to: Option<(ResourceKind, f32)> },
```

### 2. Central heal resolution (`vitals.rs`)

```rust
/// Permanent (CombatStats) + Σ active statuses carrying `stat`. The one lookup for every resource modifier.
pub fn resource_stat(stats: Option<&CombatStats>, statuses: Option<&StatusEffects>, stat: ResourceStat) -> f32;
pub fn outgoing_heal_amp(stats: Option<&CombatStats>, statuses: Option<&StatusEffects>) -> f32; // HealAmp
pub fn heal_factor(kind: HealKind, outgoing_amp: f32,
                   target_stats: Option<&CombatStats>, target_statuses: Option<&StatusEffects>) -> f32;
pub fn apply_heal(health: &mut Health, raw: f32, factor: f32) -> HealOutcome;
```

**Normative order inside `heal_factor` + `apply_heal`:**
1. If the target is dead or `raw <= 0`, return `HealOutcome::default()`. This fixes the resurrection bug.
2. **Prevention:** if `target_statuses.is_heal_blocked()`, the factor is `0`.
3. **Amplification** (additive, one bucket): `amp = outgoing_amp (healer) + HealReceivedAmp (target)`. `HealKind::Regen` uses `amp = 0` (D3).
4. **Reduction** (multiplicative, strongest only): `factor = (1 + amp).max(0) × (1 − heal_reduction.clamp(0,1))`.
5. **Overheal clamp:** `amount = raw × factor`, `applied = amount.min(max − current).max(0)`, `overheal = amount − applied`. Overheal is discarded; there is no overheal-to-shield.

**Callers. Each one is the only change at its site:**

| Path | Kind | Healer amp source |
|---|---|---|
| `apply_heal_events` (`effects.rs:534`). `HealEvent` stays unchanged | Direct | `heal.source`'s stats/statuses, read before the target is borrowed |
| HoT in `tick_status_effects` (`status.rs:508`) | OverTime | Taken from the caster at application time: in the `ApplyStatus` arm (`effects.rs:368-397`), `built.heal_per_sec *= 1 + outgoing_heal_amp(caster)`. The tick passes `0` so the amp is not applied twice |
| `regen_health` (`progression.rs:35`) | Regen | none. Effective regen = `regen_per_sec + resource_sum(HealthRegen)`, then `heal_factor` |
| Leech in `apply_damage_events` | Leech | the damage source (the healer is the one being healed) |
| `SpendResource` converting into Health | Conversion | the target itself |

`regen_mana` (`abilities/mod.rs:169`): effective = `regen_per_sec + resource_sum(ManaRegen)`, clamped `>= 0`, and it skips dead units. Mana has no amp, reduction or prevention.

### 3. Damage-pipeline integration (`combat.rs`)

**`DamageEvent` changes** (`combat.rs:74-149`):
- Add the field `drain_ratio: f32`. It defaults to `0` in every constructor and in the reflect literal at `combat.rs:338`.
- Add a builder, `.drained(ratio)`.
- Add `DamageKind::Cleave` and a `DamageEvent::cleave(..)` constructor, the same as `basic_attack` but with that kind. `basic_attack.rs:504` switches to it. Nothing matches on `DamageKind::BasicAttack` outside tests.

**Order in `apply_damage_events`.** The existing steps are kept and the new steps are in bold:

```text
invulnerable skip → sleep wake → outgoing (amp, crit) → ShieldPool (raw) → block → armor/MR
→ incoming amp → **mana shield** → health −= delta → **queue leech** → reflect (from final delta)
after the loop: **apply queued leech heals via vitals::apply_heal (same frame)** → write reflected events
```

**Mana shield step:**
- Inputs: the target's `StatusEffects::mana_shield()` and an `Option<&mut Mana>`, which is added to the `p1` query.
- Compute `absorbed = (delta × absorb).min(mana.current × damage_per_mana)`.
- Then `mana.current -= absorbed / damage_per_mana` and `delta -= absorbed`.
- It runs after mitigation, so mana pays for post-armor damage (D8).
- `resolve_incoming_damage`'s signature is unchanged, so existing tests stay stable. The step is a separate helper, `absorb_with_mana_shield(&mut f32, &mut Mana, ManaShield) -> f32`.

**Leech step:**
- `dealt = delta.min(pre_hit_current.max(0))`, so overkill does not leech.
- `ratio = event.drain_ratio + (kind == BasicAttack ? Lifesteal : 0) + (is_spell ? SpellLifesteal : 0)`. Both stats come from `resource_stat(source)`, read through `p0`, which is widened to `Query<(&CombatStats, Option<&StatusEffects>)>`.
- Queue `(source, dealt × ratio)` when `source != target` and the value is above 0.
- Leech skips DoT (`kind Dot`, `is_spell false`), reflection (`kind Generic`, `is_spell false`) and cleave (`kind Cleave`, D4).
- Damage absorbed by `ShieldPool` or a mana shield does not leech (D4).
- Leech heals land after every damage event in the batch. A source killed in the same batch gets no heal, because `apply_heal` checks that it is alive.

`DrainHealth` in `EffectRunner` emits `DamageEvent::spell(..).drained(transfer_ratio)` after `scaled_amount`. As a result it also gets spell amp and spell lifesteal (it is a spell).

### 4. Mana flows (`vitals.rs`)

```rust
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ResourceDrainEvent {
    pub source: Option<Entity>, pub target: Entity,
    pub resource: ResourceKind, pub amount: f32,
    pub burn: Option<(f32, DamageType)>,     // mana only
    pub to_source: Option<f32>,              // same resource → source (mana drain)
    pub convert_to: Option<(ResourceKind, f32)>, // → target (conversion)
}
pub fn apply_resource_drain_events(..., mut damage: MessageWriter<DamageEvent>)
```

`EffectRunner` emits these events:
- `DrainMana` → `{Mana, burn: burn_ratio>0, to_source: transfer_ratio>0}`.
- `SpendResource` → `{resource, convert_to, source: Some(caster)}`.

**Applier rules:**
1. Skip a dead target. Skip an invulnerable target when `source != target` (D7).
2. `removed = amount.min(available)`. For Health, `available = (current − 1).max(0)`, so spending is non-lethal (D5). For Mana, `available = current`.
3. Health is removed directly, not as damage. It ignores shields, reflection and lifesteal, and the generator only allows it on `caster`/`owner`.
4. `burn` → `DamageEvent::spell(source, target, removed × ratio, type)`. It is written before `apply_damage_events` in the same frame, and spell amp applies to it.
5. `to_source` → add the mana to the source, clamped to its max.
6. `convert_to` Health → `apply_heal(Conversion)`. `convert_to` Mana → add, clamped.

**Health transfer from yourself to an ally** is a composition: `SpendResource{Health}` on the caster plus `Heal` on the cast target.

### 5. Mana cost modification (`actions/pipeline.rs`, `abilities/passive.rs`)

- `vitals::effective_mana_cost(base, stats, statuses) = base × (1 + resource_stat(ManaCostAmp)).max(0)`.
- It is applied once, in `ActionSources::spec` (`pipeline.rs:31-54`). Add a read-only `Query<(Option<&CombatStats>, Option<&StatusEffects>)>` to the `SystemParam`.
- That covers every consumer: validate, hotkeys (`abilities/mod.rs:221-235`), queued activations, and the re-check at fire time in `tick_activations` → `current()` → `fire()`. So the cost is evaluated when the cost is paid.
- Every system that uses `ActionSources` today reads `CombatStats`/`StatusEffects` without mutating them, so there is no B0001 conflict.
- It applies to items too, and to toggle upkeep (`passive.rs:127-133`) (D6).
- Tooltips keep showing the base cost (out of scope).

### 6. Max health/mana semantics (`components.rs`, `vitals.rs`)

- Add `Health::change_max(delta, rule)` and `Mana::change_max(delta, rule)`.
  - `KeepMissing` (the current rule): `max = (max+delta).max(1|0)`; on a gain `current += delta`; then `current = current.min(max)`.
  - `PreserveRatio`: `current = current / old_max × new_max`. When `old_max == 0`, `current` stays where it is, clamped.
  - Neither rule kills a living unit: `current` is kept `>= min(1, max)` while alive.
- Behaviour-preserving refactor: `adjust_passives` (`items.rs:922-928`) and `HeroAttributes::apply_to/apply_delta` (`components.rs:276-310`) call `KeepMissing`.
- Timed modifiers: add the component `AppliedStatusCaps { health: f32, mana: f32 }` and the system `vitals::sync_status_resource_caps`.
  - Each frame it computes `resource_sum(MaxHealth/MaxMana)` and applies the delta from the stored value with `PreserveRatio` (D1). It inserts the component lazily.
  - It runs after `apply_status_effect_events` and `tick_status_effects`, and before `apply_damage_events`.
  - Because it reconciles, it is immune to every removal path: expiry, dispel, remove, refresh, debuff-immunity purge, sleep wake.

### Scheduling

- `AbilitiesPlugin` applier chain (`abilities/mod.rs:106-115`) becomes `apply_heal_events, apply_mana_restore_events, apply_resource_drain_events, apply_displacement_events, apply_status_effect_events, vitals::sync_status_resource_caps, apply_teleport_events`. The whole chain stays `.before(apply_damage_events)`.
- `StatusPlugin`: `tick_status_effects.before(apply_damage_events).before(vitals::sync_status_resource_caps)`.
- Register `ResourceDrainEvent` in `AbilitiesPlugin`.
- Hand-listed test apps that must be updated in the same commit:
  - `abilities/mod.rs::test_app` (~line 610)
  - `items.rs` item app (~2750)
  - the `status.rs` test apps (~868, 1036, 1323) wherever the new systems or messages are needed.

---

## Content schema (generator; no new columns)

**`EFFECT_TYPES` gains `resource`.** Columns used: `value` = amount, `amount` = ratio, plus `damage_type`.

| CSV `effect_type,effect_id` | `value` | `amount` | `damage_type` | Targets | Rust |
|---|---|---|---|---|---|
| `heal` / `heal,health` (unchanged) | HP | n/a | n/a | any | `Heal` |
| `heal,mana` (unchanged) | mana | n/a | n/a | any | `RestoreMana` |
| `resource,health_drain` | damage (scaling allowed) | heal ratio (default 1) | required | non-self | `DrainHealth` |
| `resource,mana_burn` | mana removed | damage per mana (default 1) | required | non-self | `DrainMana{burn}` |
| `resource,mana_drain` | mana removed | ratio given to the caster (default 1) | blank | non-self | `DrainMana{transfer}` |
| `resource,spend_health` / `spend_mana` | spent | n/a | blank | `caster`/`owner` only | `SpendResource{.., None}` |
| `resource,health_to_mana` / `mana_to_health` | spent | gained per point spent (required, >0) | blank | `caster`/`owner` only | `SpendResource{.., Some}` |
| `apply_status,heal_over_time` | HP/s (required) | n/a | n/a | any | `StatusSpec::HealOverTime` |
| `apply_status,heal_reduction` | n/a | percent 0..100 (required) | n/a | any | `StatusSpec::HealReduction` |
| `apply_status,heal_block` | n/a | n/a | n/a | any | `StatusSpec::HealBlock` |
| `apply_status,mana_shield` | absorb % 0..100 (required) | damage per mana (>0, required) | n/a | any; works with `while_toggled` | `StatusSpec::ManaShield` |
| `stat_modifier,{lifesteal,spell_lifesteal,heal_amp,heal_received_amp,mana_cost_amp}` | fraction (−1..=1; `mana_cost_amp >= −1`) | n/a | n/a | passive → `PassiveStat`; timed/toggled → `StatusSpec::ResourceModifier` | |
| `stat_modifier,{health_regen,mana_regen,max_health,max_mana}` timed/toggled | flat | n/a | n/a | passive path unchanged | `ResourceModifier` |

Generator changes in `generator_common.py`:
- `STATUSES` gains `heal_over_time` (buff), `heal_reduction`, `heal_block` (debuffs) and `mana_shield` (buff).
- Add a `RESOURCE_STATS` map next to `TIMED_STATS` (`:170`). The timed `stat_modifier` branch (`:1018-1037`) emits `StatusSpec::ResourceModifier { id: "{owner}_{stat}", .. }` for those ids.
- Add the new `PASSIVE_STATS` keys (`:182`).
- Add the new header imports (`ResourceKind`, `ResourceStat`) in both generators' header templates. The regeneration diff should be header-only.

Units follow the existing conventions: passive amps are fractions (like `spell_amp`), and `apply_status` percents are 0..100 (like `slow`) (D9).

---

## Interactions (normative)

| Situation | Rule |
|---|---|
| Heal on a dead unit (any path) | No effect. The returned `applied` is 0 |
| Heal block + amp | Block wins (factor 0) |
| Two heal-reduction debuffs | The strongest applies; they don't stack |
| Debuff-immune target | Heal reduction/block are rejected (Debuff kind). Mana burn/drain still apply unless the target is invulnerable |
| Invulnerable / damage-immune target | Leech gets 0 because `delta` is 0. Heals still land |
| Lifesteal vs heal reduction/block on the attacker | Applies (Leech goes through `heal_factor`) |
| Overkill | Leech is capped at the HP the target had before the hit |
| Reflected damage, DoT, cleave | Never leech (DoT may only spell-leech if it is ever marked `is_spell`; it is not today) |
| Mana shield + `ShieldPool` | `ShieldPool` absorbs raw damage first. The mana shield absorbs post-mitigation damage after that |
| Mana shield at 0 mana | It absorbs nothing. The status stays (a toggle may switch off through its upkeep) |
| Temporary max HP buff ends | `PreserveRatio`, so no free heal and no extra loss |
| Item purchase / sale of max HP | `KeepMissing` (unchanged) |
| Spending health at low HP | Floors at 1 HP. The cast still fires (costs are not gated, D5) |
| Cost buff expires during the cast point | The cost re-evaluated in `fire()` is the one paid. Not enough mana → the cast is cancelled (existing rule) |
| Mana burn damage | `DamageEvent::spell`: spell amp, MR, shields and spell lifesteal all apply |

## Invariants

1. Every health increase goes through `vitals::apply_heal`. `grep -n "current + " src` should find no other heal site, apart from the max-change helper.
2. Every health decrease from damage goes through `apply_damage_events`. The one non-damage decrease is a self-spend in `apply_resource_drain_events`.
3. Timed resource modifiers are summed when read, never stored into `CombatStats`. The exceptions are max HP/mana, which only `sync_status_resource_caps` reconciles, and the `temporary_mana` status (Soul Ring), which only `sync_temporary_mana` reconciles: it adds the granted mana (and any max-mana overflow) to `Mana` and removes exactly the unspent bonus and the overflow when the grant ends, never more than was unspent (spent mana is never taken back).
4. Every leech heal is applied in the same frame as its damage, after the damage batch. No new `AbilityTrigger` is emitted.
5. Cleanup-plan invariants hold: a single executor, the chain positions above, and generated code changed only by regeneration.

---

## Implementation steps (one commit each)

**Step 0: baseline.** Run V and record the `cargo test` pass count.

**Step 1: central heal plus the dead-heal fix.**
- Create `src/vitals.rs` (types, `resource_stat`, `heal_factor`, `apply_heal`) and register it in `main.rs`.
- Add the new `StatusEffect` and `CombatStats` fields, defaulting to zero.
- Route `apply_heal_events`, the HoT tick and `regen_health` through it.
- `regen_mana` skips dead units.

**Step 2: heal modifiers and HoT.**
- `StatusSpec::{HealOverTime, HealReduction, HealBlock, ResourceModifier}`.
- `PassiveStat::{HealAmp, HealReceivedAmp}` plus the `ItemPassives` plumbing.
- Take the healer amp into HoT at application time.
- Tooltip rows (`tooltip.rs:284`, `468`), and the item describer.

**Step 3: regen and max modifiers.**
- `ResourceStat::{Health,Mana}Regen` in the regen systems.
- `change_max` + `MaxChangeRule`. Refactor `adjust_passives` / `HeroAttributes` to use it (behaviour-preserving).
- `AppliedStatusCaps` + `sync_status_resource_caps` and its scheduling.
- Optional: the HUD regen text (`ui.rs:1466-1469`) shows the effective regen.

**Step 4: lifesteal, spell lifesteal, health transfer.**
- `DamageEvent.drain_ratio`, `DamageKind::Cleave`, the leech queue in `apply_damage_events`.
- `PassiveStat::{Lifesteal, SpellLifesteal}`, `AbilityEffect::DrainHealth`.

**Step 5: mana shield.**
- `StatusSpec::ManaShield`, the `Option<&mut Mana>` in `apply_damage_events`, and `absorb_with_mana_shield`.

**Step 6: burn, drain, sacrifice, conversion.**
- `ResourceDrainEvent` and its applier.
- `AbilityEffect::{DrainMana, SpendResource}` arms in `EffectRunner`.
- Chain registration and test apps.

**Step 7: mana cost modification.**
- `PassiveStat::ManaCostAmp`, `ResourceStat::ManaCostAmp`, `effective_mana_cost` in `ActionSources::spec`, and toggle upkeep.

**Step 8: generator, data and examples.**
- The schema table above.
- `data/examples/`:
  - change `healing_salve` to `apply_status,heal_over_time`;
  - add a lifesteal item (passive), a spell-lifesteal item, a health-drain ability, mana burn and mana drain rows, a mana-shield toggle (`while_toggled`), a Soul-Ring-style `health_to_mana` item, a heal-reduction debuff, a heal block, a `mana_cost_amp` passive, and a timed `max_health` buff.
- `data/examples/invalid/*`, one row per new error:
  - `spend_health` on `cast_target`;
  - `mana_burn` without `damage_type`;
  - `mana_shield` without `amount`;
  - `heal_reduction` above 100;
  - `health_to_mana` without a ratio;
  - scaling on `mana_drain`.
- Production `data/*.csv` stays untouched. The regeneration diff is header imports only.

**Step 9: docs.**
- README:
  - a new "Healing and resources (player terms → code)" table after Damage mechanics (`README.md:196`);
  - status table rows for heal over time, heal reduction, heal block and mana shield (`:177-190`);
  - the generator section (`:302-375`).
- Add an "Implemented" note at the end of this plan.

---

## Affected files

`src/vitals.rs` (new), `src/main.rs` (mod), `src/components.rs`, `src/status.rs`, `src/combat.rs`, `src/basic_attack.rs` (cleave constructor), `src/abilities/definition.rs`, `src/abilities/effects.rs`, `src/abilities/mod.rs` (chain, `regen_mana`, test_app), `src/abilities/passive.rs` (toggle cost), `src/abilities/tooltip.rs`, `src/actions/pipeline.rs`, `src/items.rs` (`ItemPassives`, `adjust_passives`, describer, item app), `src/progression.rs`, `src/heroes.rs` (`CombatStats` literal), optionally `src/ui.rs`. Also `scripts/generator_common.py`, `scripts/AbilityGenerator.py`, `scripts/ItemGenerator.py` (headers, trigger/target validation), `scripts/test_generator_common.py`, `data/examples/**`, regenerated `src/abilities/generated.rs` and `src/item_catalog.rs`, and `README.md`.

Not touched: `src/net/*`, `Cargo.toml`, the production CSV headers or rows, `actions/resources.rs` (`ActionCost` stays mana-only).

---

## Regression tests (`cargo test`)

Unit tests go in `#[cfg(test)] mod tests` inside `src/vitals.rs` (pure functions). Integration tests use the existing `test_app` patterns, built on `World` / `run_system_once` / `App::update`.

| Test | Location | Step |
|---|---|---|
| `heal_does_not_revive_dead_units` (direct, HoT, regen) | vitals.rs + abilities/mod.rs | 1 |
| `heal_clamps_at_max_and_reports_overheal` | vitals.rs | 1 |
| `existing_item_heal_tests_unchanged` (`no_target_item_heals_…`, `vial_use_spawns_heal_flash`, channel heal) | items.rs, abilities/mod.rs | 1 |
| `heal_amp_outgoing_and_incoming_add` | vitals.rs | 2 |
| `strongest_heal_reduction_wins_and_multiplies_after_amp` | vitals.rs | 2 |
| `heal_block_zeroes_every_heal_kind_and_is_dispellable` | status.rs | 2 |
| `heal_reduction_blocked_by_debuff_immunity` | status.rs | 2 |
| `heal_over_time_ticks_total_over_duration_with_caster_amp_once` | status.rs | 2 |
| `regen_ignores_amp_but_respects_reduction` | progression.rs or vitals.rs | 3 |
| `timed_regen_modifier_adds_and_expires` (health and mana) | status.rs | 3 |
| `passive_max_health_keeps_missing_rule` (existing `items.rs:2238` area stays green) | items.rs | 3 |
| `timed_max_health_preserves_ratio_on_apply_expire_and_dispel` | status.rs | 3 |
| `lifesteal_heals_fraction_of_mitigated_damage` | combat.rs | 4 |
| `lifesteal_capped_by_overkill_and_skips_shield_absorbed` | combat.rs | 4 |
| `cleave_reflect_and_dot_do_not_leech` | combat.rs | 4 |
| `spell_lifesteal_applies_to_ability_damage_only` | combat.rs | 4 |
| `drain_health_ability_damages_and_heals_caster_same_frame` | abilities/mod.rs | 4 |
| `source_killed_in_same_batch_gets_no_leech` | combat.rs | 4 |
| `mana_shield_absorbs_post_mitigation_and_spends_mana` | combat.rs | 5 |
| `mana_shield_runs_out_and_overflow_hits_health` | combat.rs | 5 |
| `mana_shield_toggle_applies_and_removes` | abilities/passive.rs | 5 |
| `mana_burn_deals_damage_equal_to_removed_mana` (capped by target mana) | abilities/mod.rs | 6 |
| `mana_drain_transfers_to_caster_clamped_to_max` | vitals.rs app | 6 |
| `spend_health_is_non_lethal` | vitals.rs app | 6 |
| `health_to_mana_conversion_uses_spent_amount` | vitals.rs app | 6 |
| `invulnerable_target_ignores_hostile_mana_drain` | vitals.rs app | 6 |
| `mana_cost_amp_reduces_ability_and_item_costs` | abilities/mod.rs, items.rs | 7 |
| `cost_is_reevaluated_when_the_cast_fires` (buff expires during cast point) | abilities/mod.rs | 7 |
| `toggle_upkeep_uses_effective_cost` | abilities/passive.rs | 7 |
| `real_schedule_builds_without_conflicts` (re-run the existing `teleport_is_ordered_consistently_in_the_real_schedule`) | teleport.rs | 1, 6, 7 |
| `test_resource_effects_*`, `test_heal_statuses_*`, `test_resource_stat_modifiers_*`, `test_spend_requires_self_target` | scripts/test_generator_common.py | 8 |

Commands (**V**):

```bash
cd /home/user/labyrinth
cargo test                      # focused: cargo test vitals:: ; cargo test combat:: ; cargo test status::
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py --check
PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py --check
(cd scripts && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest test_generator_common)
PYTHONDONTWRITEBYTECODE=1 python3 scripts/AbilityGenerator.py && PYTHONDONTWRITEBYTECODE=1 python3 scripts/ItemGenerator.py && git diff --exit-code -- src/abilities/generated.rs src/item_catalog.rs src/components.rs src/items.rs
```

Also run `--check` on `data/examples/*.csv`, which must pass, and on `data/examples/invalid/*.csv`, which must report the new errors.

---

## Risks

| Risk | Mitigation |
|---|---|
| Event ordering: burn damage or leech one frame late | The drain applier sits in the chain before `apply_damage_events`, and leech is applied inline after the batch. Tests assert the same frame |
| Double-counting the amp on HoT | The amp is taken at application time and the tick passes `outgoing_amp = 0`. There is a test for it |
| Stat leaks from timed modifiers | They are summed when read, and max caps go through reconciliation. No `add/subtract_status_stat_delta` changes |
| Resurrection fix changes behaviour | Intended. The PR should call it out. No current test depends on healing a dead unit (verify in Step 0) |
| Bevy param conflicts (B0001) | The `ActionSources` addition is read-only. `apply_damage_events` adds only to the existing `ParamSet`s. The real-schedule test catches cycles and conflicts |
| Hand-listed test apps miss the new systems | Grep for `apply_heal_events` / `apply_status_effect_events` in every step (cleanup-plan rule) |
| Generated-code compatibility | Only enum variants are added, plus `CombatStats`/`DamageEvent` fields with two to three hand-written literals. Generated literals are unaffected; the header imports are regenerated |
| Module cycle `status` ↔ `vitals` | Intra-crate cycles are fine. Keep `vitals` free of `abilities::*` imports |
| `regen_health` runs on clients (not gated) | Out of scope. Recommend `.run_if(is_sim_authority)` in Step 3 if the netcode owner agrees |

## Decisions (approved: recommended option for each)

- **D1:** max change rule. `PreserveRatio` for timed statuses, `KeepMissing` kept for item and attribute changes.
- **D2:** heal stacking: outgoing + incoming amp in one additive bucket, then the strongest reduction multiplicatively.
- **D3:** regen is affected by reduction and prevention but not by amps.
- **D4:** leech only on HP actually removed: no shield-absorbed damage, no overkill, no cleave.
- **D5:** health sacrifice as a non-lethal effect (floor 1 HP, no activation gate). Percent-of-current sacrifice is deferred.
- **D6:** the mana cost modifier also applies to items and toggle upkeep.
- **D7:** hostile mana burn/drain is blocked only by invulnerability, not by debuff immunity.
- **D8:** mana shield after mitigation; with several shields, the strongest applies.
- **D9:** units: fractions for `stat_modifier` amps/ratios, 0..100 percents for `apply_status`.

## Out of scope

`OnHeal` / `OnDamageTaken` trigger emission, healing numbers or VFX, overheal-to-shield, percent-based max-resource scaling for burn (`TargetMaxMana`), tooltips showing the modified cost, and netcode replication of mana.

---

## Implemented

Steps 0 to 9 landed as one commit each on `claude/gracious-albattani-90iorv` (PR 56). Every step compiled and passed **V**. `cargo test` went from **276** (Step 0 baseline) to **330** passing Rust tests, and the generator unittest from 21 to 38. Production `data/*.csv` is untouched; regeneration only adds the `ResourceKind` / `ResourceStat` imports to the two generated headers.

| Step | Commit subject |
|---|---|
| 1 | Central heal resolution and dead-unit heal fix |
| 2 | Heal modifiers and heal over time |
| 3 | Timed regen and max modifiers |
| 4 | Lifesteal, spell lifesteal and health transfer |
| 5 | Mana shield |
| 6 | Mana burn, mana drain, sacrifice and conversion |
| 7 | Mana cost modification |
| 8 | Generator support and examples |
| 9 | Documentation (this note and the README tables) |

### Deviations and clarifications

- **`HeroAttributes::apply_to` / `apply_delta` still use their own arithmetic.** Step 3 said to route them through `KeepMissing`. They do not match it: `apply_to` raises max without adding to current, and `apply_delta` also lowers current on a negative delta (for example selling a Strength item), whereas `KeepMissing` only clamps on a loss. Routing them through it would have changed behavior, so only `adjust_passives` (items and passive abilities) uses `change_max`.
- **A dead unit is no longer revived by a max-health or max-mana-style gain.** `Health::change_max` keeps a dead unit dead under both rules (the same bug family as the heal fix). The old `adjust_passives` code added the gain to `current` even at 0 HP. Mana is unaffected (it has no alive state).
- **Cleave keeps `can_crit: true`.** `DamageEvent::cleave` is exactly `basic_attack` with `DamageKind::Cleave`, so cleave behavior is unchanged apart from never leeching.
- **Two timed-regen tests live in `progression.rs` and `abilities/mod.rs`**, not `status.rs`, because `regen_health` and `regen_mana` are private to those modules. The max-cap tests are in `status.rs` as planned.
- **Test names are split in places**: `mana_cost_amp_reduces_ability_costs` (abilities/mod.rs) and `mana_cost_amp_reduces_item_costs` (items.rs); `heal_over_time_ticks_total_over_duration` (status.rs) and `heal_over_time_takes_the_casters_heal_amp_once_at_application` (abilities/mod.rs). `existing_item_heal_tests_unchanged` is the existing heal tests staying green, and `real_schedule_builds_without_conflicts` is the existing `teleport_is_ordered_consistently_in_the_real_schedule`, run at every step. Extra tests were added for dead-unit mana regen, refresh and remove not leaking max, heal block on leech, the toggle shield, and `invalid` generator rows.
- **Test apps.** The abilities `test_app` system tuple hit Bevy's 20-system limit, so the status and sync systems and the mana restore and drain systems are nested `.chain()` tuples. The `items.rs` item app only needed the `ResourceDrainEvent` message registered, not the applier system.
- **Optional HUD regen text (`ui.rs`) was not changed.** It still shows the base `regen_per_sec`, not the effective value with timed modifiers.
- **`ResourceDrainEvent.to_source` is mana-only.** Giving health back to the source is not exposed by any generator row. `SpendResource` is limited to `caster` / `owner` by the generator only; the runtime accepts any target.
- **`while_toggled` was widened in the generator** to allow `apply_status,mana_shield` next to `stat_modifier`, which the runtime already supported (`tick_toggle_upkeep` applies any `ApplyStatus`).
- **Pre-existing formatting debt.** `cargo fmt --check` already failed at the Step 0 baseline on `abilities/custom.rs`, `basic_attack.rs`, `combat.rs`, `ui.rs`, `units.rs` and the generated region in `components.rs`. Those files were not reformatted (to keep the diff reviewable); the new code was kept rustfmt-clean, and the fmt hunk count in those files did not grow.

### Verification notes

- The example CSVs were run through both generators in a scratch copy of the repository and the generated Rust compiled with `cargo build`. The scratch tests that failed there only failed because the examples replace the production definitions with the same ids; the real files are not touched by examples.
- Not run: any in-game manual check (the environment has no display). Netcode replication of mana, `OnHeal` / `OnDamageTaken` triggers, healing VFX and tooltip display of modified costs remain out of scope, as planned.
