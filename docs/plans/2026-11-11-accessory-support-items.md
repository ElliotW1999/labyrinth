# Accessory and support items (Phase Boots, Oblivion Staff, Perseverance, Mask of Madness, Boots of Travel, Moon Shard, Buckler, Ring of Basilius, Headdress)

**Date:** 2026-11-11
**Status:** Approved by the coordinator (decisions D1–D7 below take the recommended option)
**Builds on:**
- `docs/plans/2026-11-08-reactive-triggers.md`: `interval_triggers`, `TriggerTracking.intervals`, `owner_sources`, the reaction flag, and silent (`reactive`) statuses.
- `docs/plans/2026-11-08-merge-reactive-triggers-into-composition-layers.md` and `docs/architecture/composition.md`: layer ownership and the dispatcher order.
- `docs/plans/2026-11-06-status-effects-catalog.md` and `docs/plans/2026-11-10-status-effects-completion.md`: `StatusSpec`, refresh-by-id, and the `status_resistance` precedent.
- `docs/plans/2026-11-07-movement-mechanics.md`: `phased` and `move_speed_bonus`.
- `docs/plans/2026-11-08-healing-resource-mechanics.md`: lifesteal as a fraction, and regen as `ResourceModifier` (summed when read).

**Coordinator correction:** Morbid Mask already exists on main (PR #72: `morbid_mask`, 900 g, passive `lifesteal` 0.18). Do not add it again. Mask of Madness uses it as a component. Short labels follow the recent letter convention (for example `PB`, `R-BoT`), not the old numeric block.

---

## 1. Summary

| Item | How | Engine work |
|---|---|---|
| Oblivion Staff, Perseverance, Boots of Travel (+ recipe), Moon Shard | Pure CSV | none |
| Phase Boots | Pure CSV (`move_speed_bonus` 10 % and `phased`, 3 s, on `on_use`) | none |
| Mask of Madness | CSV plus one new status, `slow_resistance` | small: `status.rs`, `definition.rs`, generator |
| Buckler, Ring of Basilius, Headdress (+ recipes) | CSV plus one new trigger, `aura` | small: `AbilityTrigger::Aura`, handled in `interval_triggers`, generator |

The plan adds two engine features and no new subsystem:
- **`aura` trigger:** a pulsed refresh of short-lived, silent statuses on the units in a radius. It reuses `interval_triggers`, `EffectTarget::UnitsInRadius` and the existing same-id refresh.
- **`slow_resistance` status:** a percentage applied at the single slow-measuring site in `status::apply_status`.

## 2. Findings: what is already expressible

- **Percent move speed.** `apply_status,move_speed_bonus` with `amount` = percent. The id is `<owner>_move_speed_bonus`. It is measured against base speed, so refreshes do not compound. Precedent: `shadow_dance`.
- **Phased.** `apply_status,phased` gives `StatusSpec::Phased`, runtime id `"phased"` (precedent: `arctic_burn`).
- **Timed attack speed.** `stat_modifier,attack_speed_flat` with a duration (precedent: `overpower`). Plain `attack_speed` is passive-only.
- **Timed negative armor.** `stat_modifier,armor,-7` with a duration gives a Debuff `StatusSpec::Modifier`, id `mask_of_madness_armor`.
- **Silence on self.** `apply_status,silence` on `owner` is allowed. Silence does not block items (`src/actions/resources.rs`). Self-applied debuffs skip status resistance.
- **Lifesteal.** A fraction from 0 to 1 (18 % → `0.18`). Items with lifesteal get an explicit `description`, because `describe()` would print "+0.18 Lifesteal".
- **Mana cost on an item.** The `mana_cost` column with a blank `target_type` gives an instant no-target activation.
- **Slow resistance.** Missing. Only `status_resistance` exists.
- **Ally auras.** These are not expressible as passives. `on_interval` with `units_in_radius` comes close, but it has three flaws:
  - every refresh emits `Applied` lifecycle events, which spams status triggers;
  - the first application waits a whole period;
  - authors can make it stack without bound.
- **Duplicate components.** Already supported: `component_needs` builds a multiset (Soul Ring already uses `gauntlets_of_strength;gauntlets_of_strength`). Only a regression test is needed.
- **Cost versus component sum.** Neither the shop nor the generator checks it. Buying a finished item directly charges its `cost`; building it from parts costs the sum of the parts. Buckler, Ring of Basilius and Headdress sum to 385, not 425.
- **Combine order.** `combine_ready_items` picks the first match in `ItemId::all()` order. Keep Phase Boots before Boots of Travel in the CSV.

## 3. Design: the `aura` trigger

Rejected alternatives:
- `attached_zone` is timed and needs `on_cast` or `on_use` to start it.
- A per-target reconcile in the style of `While` breaks when two carriers share a status id.

**Semantics**
- **CSV.** `trigger = aura`, with no argument. It is valid for items and abilities.
- **Allowed rows.**
  - `effect_type` is `stat_modifier` (timed or resource stats) or `apply_status`.
  - `effect_target` is `units_in_radius`. Items must set `radius`; abilities may fall back to `aoe_radius`.
  - `duration` must be blank: the generator writes `AURA_LINGER = 0.5`.
  - The generator rejects the `:independent` suffix, a `stat_modifier` `amount`, and the statuses `stacks`, `shield`, `attack_charges`, `mana_shield`, `damage_share`, `convert_on_death`, `taunt_target` and `mini_stun` and `temporary_mana`.
- **Team.** The existing polarity rule applies: beneficial rows reach allies, including the owner; harmful rows reach enemies.
- **Runtime.**
  - New `AbilityTrigger::Aura`.
  - `interval_triggers` treats it as a fixed-period interval, `AURA_PULSE_SECS = 0.25`. Its timer is created already due, so the first pulse fires on the first frame the owner holds the source.
  - Ability sources skip the pulse while `passive_suppressed` (Break). Items ignore Break.
- **Silent statuses.** Add `Aura` to `AbilityTrigger::is_reaction()`. The statuses it applies are then `reactive` and fire no `StatusLifecycleEvent`s.
- **Stacking.** Status ids are `<owner_id>_<stat>`. Identical auras, from several carriers or from two copies in one inventory, refresh one status; different auras stack. The carrier's own passive is separate (Buckler carrier: +1 + 2 = +3 armor).
- **Removal.** The status lingers at most 0.5 s after the unit leaves the radius, the owner sells the item, the owner dies, or Break hits an ability source.
- **Invariant.** `AURA_LINGER > AURA_PULSE_SECS` with margin. A test pins it.

```rust
pub(crate) const AURA_PULSE_SECS: f32 = 0.25;
fn pulse_period(t: AbilityTrigger) -> Option<f32> {
    match t {
        AbilityTrigger::OnInterval { millis } if millis > 0 => Some(millis as f32 / 1000.0),
        AbilityTrigger::Aura => Some(AURA_PULSE_SECS),
        _ => None,
    }
}
// source_triggers predicate: |t| pulse_period(t).is_some()
// on first insert: elapsed = if trigger == Aura { period } else { 0.0 }
// skip Aura from ability sources when passive_suppressed(source, &defs, statuses)
```

## 4. Design: `slow_resistance` status (Mask of Madness)

- **CSV.** `apply_status,slow_resistance` with `amount` (or `value`) = percent, more than 0 and at most 100. The id is `<owner>_slow_resistance`. It is a Buff, and it shares the generator branch with `status_resistance`. Add it to `STATUSES` and `OWNER_ID_STATUSES`.
- **Rust.**
  - `StatusSpec::SlowResistance { id, percent: RankValue }` and `StatusEffect::slow_resistance(id, fraction, duration)`.
  - A new `StatusEffect.slow_resistance` field.
  - `StatusEffects::slow_resistance_factor()`: the product of `(1 - r)` over live statuses.
  - In `apply_status`:
    ```rust
    effect.move_speed = (effect.move_bonus_fraction
        - effect.move_slow_fraction * statuses.slow_resistance_factor()) * base;
    ```
    Keep `move_slow_fraction` raw.
- **Scope.** Percent move slows only. Flat `move_speed` modifiers, attack-speed slows and hex caps are not affected.
- **Snapshot.** The resistance applies when a slow lands or refreshes (D3).
- Add the variant to the tooltip `describe_effect`.

## 5. Exact CSV rows

Append to `data/items.csv` in this order (28 columns after #72: the last is `attribute_toggle`, written as a trailing empty field; sell value = `cost // 2`; colors blank so they derive from the name):
```csv
phase_boots,Phase Boots,accessories,1450,725,,,FALSE,,FALSE,,"+4 Armor, +50 Move Speed, +18 Attack Damage. Active: +10% Move Speed and phased for 3s",PB,8,boots_of_speed;chainmail;blades_of_attack,,,,,,,,,,,,
oblivion_staff,Oblivion Staff,accessories,1625,812,,,FALSE,,FALSE,,"+10 Intelligence, +1.25 Mana Regen, +35 Attack Speed",OS,,blitz_knuckles;robe_of_the_magi;sages_mask,,,,,,,,,,,,
perseverance,Perseverance,accessories,1400,700,,,FALSE,,FALSE,,"+5.5 HP Regen, +2.25 Mana Regen",PV,,ring_of_health;void_stone,,,,,,,,,,,,
mask_of_madness,Mask of Madness,accessories,1900,950,,,FALSE,,FALSE,,"+20 Attack Damage, +18% Lifesteal. Active (6s): +12% Move Speed, +110 Attack Speed, 30% slow resistance, -7 Armor, silenced",MoM,16,morbid_mask;broadsword,,,,,,,,,,25,,
boots_of_travel,Boots of Travel,accessories,2500,1250,,,FALSE,,FALSE,,+110 Move Speed,BoT,,boots_of_speed;recipe_boots_of_travel,,,,,,,,,,,,
recipe_boots_of_travel,Recipe: Boots of Travel,recipe,2000,1000,,,FALSE,,FALSE,,Combines with components into Boots of Travel,R-BT,,,,,,,,,,,,,,
moon_shard,Moon Shard,accessories,4000,2000,,,FALSE,,FALSE,,+140 Attack Speed,MS,,hyperstone;hyperstone,,,,,,,,,,,,
buckler,Buckler,support,425,212,,,FALSE,,FALSE,,+1 Armor. Aura: +2 Armor to allies within 1200,BK,,ring_of_protection;recipe_buckler,,,,,,,,,,,,
recipe_buckler,Recipe: Buckler,recipe,210,105,,,FALSE,,FALSE,,Combines with components into Buckler,R-BK,,,,,,,,,,,,,,
ring_of_basilius,Ring of Basilius,support,425,212,,,FALSE,,FALSE,,+0.5 Mana Regen. Aura: +1 Mana Regen to allies within 1200,RB,,sages_mask;recipe_ring_of_basilius,,,,,,,,,,,,
recipe_ring_of_basilius,Recipe: Ring of Basilius,recipe,210,105,,,FALSE,,FALSE,,Combines with components into Ring of Basilius,R-RB,,,,,,,,,,,,,,
headdress,Headdress,support,425,212,,,FALSE,,FALSE,,+0.5 HP Regen. Aura: +2 HP Regen to allies within 1200,HD,,ring_of_regen;recipe_headdress,,,,,,,,,,,,
recipe_headdress,Recipe: Headdress,recipe,210,105,,,FALSE,,FALSE,,Combines with components into Headdress,R-HD,,,,,,,,,,,,,,
```
Short labels must be unique and at most 4 characters (the generator rejects longer ones, so the Boots of Travel recipe is `R-BT`, Perseverance is `PV`, Buckler is `BK`). Implemented as listed here.

Append to `data/item_effects.csv`:
```csv
phase_boots,0,passive,owner,stat_modifier,armor,4,,,,,,
phase_boots,1,passive,owner,stat_modifier,move_speed,50,,,,,,
phase_boots,2,passive,owner,stat_modifier,attack_damage,18,,,,,,
phase_boots,3,on_use,owner,apply_status,move_speed_bonus,,,3,,10,,
phase_boots,4,on_use,owner,apply_status,phased,,,3,,,,
oblivion_staff,0,passive,owner,stat_modifier,intelligence,10,,,,,,
oblivion_staff,1,passive,owner,stat_modifier,mana_regen,1.25,,,,,,
oblivion_staff,2,passive,owner,stat_modifier,attack_speed,35,,,,,,
perseverance,0,passive,owner,stat_modifier,health_regen,5.5,,,,,,
perseverance,1,passive,owner,stat_modifier,mana_regen,2.25,,,,,,
mask_of_madness,0,passive,owner,stat_modifier,attack_damage,20,,,,,,
mask_of_madness,1,passive,owner,stat_modifier,lifesteal,0.18,,,,,,
mask_of_madness,2,on_use,owner,apply_status,move_speed_bonus,,,6,,12,,
mask_of_madness,3,on_use,owner,stat_modifier,attack_speed_flat,110,,6,,,,
mask_of_madness,4,on_use,owner,apply_status,slow_resistance,,,6,,30,,
mask_of_madness,5,on_use,owner,stat_modifier,armor,-7,,6,,,,
mask_of_madness,6,on_use,owner,apply_status,silence,,,6,,,,
boots_of_travel,0,passive,owner,stat_modifier,move_speed,110,,,,,,
moon_shard,0,passive,owner,stat_modifier,attack_speed,140,,,,,,
buckler,0,passive,owner,stat_modifier,armor,1,,,,,,
buckler,1,aura,units_in_radius,stat_modifier,armor,2,,,1200,,,
ring_of_basilius,0,passive,owner,stat_modifier,mana_regen,0.5,,,,,,
ring_of_basilius,1,aura,units_in_radius,stat_modifier,mana_regen,1,,,1200,,,
headdress,0,passive,owner,stat_modifier,health_regen,0.5,,,,,,
headdress,1,aura,units_in_radius,stat_modifier,health_regen,2,,,1200,,,
```

## 6. Implementation sequence

Each step builds and passes its tests on its own.

1. **Pure-CSV items:** Oblivion Staff, Perseverance, Boots of Travel and its recipe, Moon Shard, Phase Boots. Run `python3 scripts/ItemGenerator.py` and add the item tests.
2. **`slow_resistance`:**
   - `status.rs`: field, constructor, factor and `apply_status`.
   - `definition.rs`: `StatusSpec::SlowResistance`.
   - `tooltip.rs`: describe the new status.
   - `generator_common.py`: `STATUSES`, `OWNER_ID_STATUSES` and the `status_resistance` branch.
   - Tests.
3. **Mask of Madness rows:** regenerate and add tests.
4. **`aura` trigger:**
   - `definition.rs`: `AbilityTrigger::Aura`, added to `is_reaction()`.
   - `reactive_triggers.rs`: `AURA_PULSE_SECS`, `pulse_period`, the immediate first pulse and the Break check.
   - `tooltip.rs`: `trigger_tag(Aura) => "aura"`.
   - `generator_common.py`:
     - add `aura` to `ABILITY_TRIGGERS`, `ITEM_TRIGGERS`, `REACTION_TRIGGERS` and `REACTIVE_NO_TRIGGER_UNIT`;
     - add `AURA_LINGER = 0.5` and the duration handling next to `while`;
     - add the target, `:independent`, `amount` and status-blacklist checks;
     - add `invalid_combo` entries for `aura` with `owner`, `caster`, `trigger_unit`, `affected_units` and `attacker`.
   - `ItemGenerator.py` and `AbilityGenerator.py`: `center_for["aura"] = "Caster"`.
5. **Aura item rows:** Buckler, Ring of Basilius, Headdress and their recipes. Regenerate.
6. **Examples:**
   - One valid aura item in `data/examples/`.
   - In `data/examples/invalid/`: an `aura` row on `owner`, `aura` with a `duration`, `aura` with `armor:independent`, and `aura` with `apply_status,shield`.
7. **Docs** (§8).

## 7. Tests

### 7.1 Generator (`scripts/test_generator_common.py`)

`AuraTriggerTests`:
- **Valid rows:**
  - A stat-modifier aura targets allies around the caster, with `UnitsInRadius { center: Caster, radius: Fixed(1200), team: Ally }`, duration 0.5 and id `<owner>_armor`.
  - Resource stats generate a `ResourceModifier`.
  - A harmful row targets enemies.
- **Rejected rows:**
  - Targets other than `units_in_radius`.
  - An item aura with no radius.
  - A duration, `:independent`, attack charges, accumulating statuses, or `damage` / `heal`.
- `aura` is in `REACTION_TRIGGERS`.

`test_slow_resistance_status`: valid at 30; errors at 0 and 101.

`test_duplicate_components_are_accepted`: `hyperstone;hyperstone` validates and emits both ids.

### 7.2 Rust: aura (reactive-trigger tests in `src/abilities/mod.rs`)

- Applies on the first frame to the owner and to allies inside the radius. Not to allies outside the radius, and not to enemies.
- Lingers, then expires after the unit leaves the radius.
- Ends when the item is removed and when the owner dies.
- Identical auras from two carriers do not stack (+2 only).
- Two copies share one pulse.
- Fires no status triggers.
- Never lapses between pulses, sampled every frame for 3 s. Also assert every aura row's duration is greater than `1.5 * AURA_PULSE_SECS`.
- An ability aura is suppressed by Break.

### 7.3 Rust: items (`src/items.rs`)

- Moon Shard combines from two Hyperstones (+140 attack speed).
- Phase Boots and Boots of Travel combine.
- Phase Boots active: move speed bonus and `phased` for 3 s, 8 s cooldown, no mana.
- Mask of Madness active:
  - spends 25 mana;
  - applies its four statuses for 6 s, with armor −7;
  - silences the owner: an ability cast is rejected, but the item still fires;
  - starts a 16 s cooldown.
- The Buckler family costs the listed prices (425 for the items, 210 for the recipes).

### 7.4 Rust: status (`src/status.rs`)

- 30 % resistance turns a 50 % slow into 35 %.
- Two resistances multiply.
- Slows already applied are unchanged (snapshot).
- Move speed bonuses are untouched.

## 8. Docs to update

- `wiki/Items.md`:
  - the `aura` trigger and its rules;
  - the `slow_resistance` status;
  - components may repeat an id;
  - a direct buy charges `cost`, building charges the sum of the parts.
- `wiki/Limitations.md`: replace "aura statuses applied to other units" with what is still missing:
  - auras are pulsed with a 0.5 s linger and reach every allied unit (there is no hero mask);
  - slow resistance applies only to slows that land after it;
  - boots move speed stacks additively.
- `wiki/Status-Effects.md`: `slow_resistance`.
- `wiki/Abilities.md` and `wiki/Ability-Generator.md`: an `aura` row in the trigger tables.
- `docs/architecture/composition.md`: `interval_triggers` also pulses `Aura`, and `Aura` is a reaction trigger, so its statuses are silent.

## 9. Invariants

- I1. Aura statuses are finite (`AURA_LINGER`), refresh by a stable owner-based id, and never stack.
- I2. `AURA_LINGER > AURA_PULSE_SECS`.
- I3. Aura contexts are reactions: no lifecycle events, damage triggers or spell-hit.
- I4. A dead owner, an owner who no longer holds the source, or a Break-suppressed ability source emits no pulses.
- I5. Slow resistance only scales `move_slow_fraction` in `apply_status`.
- I6. Items and abilities share one trigger and one dispatcher.

## 10. Risks

- **Shared `"phased"` id.** Phase Boots' 3 s phase replaces Winter Wyvern's Arctic Burn phase.
- **Shared `"silence"` id.** An enemy silence and the Mask of Madness self-silence replace each other.
- **Mask of Madness kinds.** An enemy buff dispel strips the bonuses but leaves the silence and armor. The owner's debuff dispel removes the silence and armor.
- **Combine order.** CSV order picks Phase Boots over Boots of Travel when the parts for both are present.
- **Aura reach.** Auras reach creeps and buildings, and Ring of Basilius reaches units without mana. Both are harmless.

## 11. Decisions (coordinator-approved defaults)

- D1. Keep the listed prices: 425 for the items, 210 for the recipes. No cost-sum warning.
- D2. Keep the shared `phased` and `silence` ids for now. Per-owner variants are deferred.
- D3. Slow resistance is snapshotted when a slow lands.
- D4. Accept the per-row Buff/Debuff split for Mask of Madness.
- D5. Auras reach all allied units.
- D6. (Resolved) Morbid Mask already exists on main.
- D7. Boots do not have a "do not stack" rule.


## 12. Implementation notes

- Rows were generated against the 28-column header (one trailing empty field). Mask of Madness and the aura items were added in their own steps.
- `aura` joined `REACTIVE_PLAIN_TRIGGERS` (so it is in both trigger tables and `REACTIVE_TRIGGER_NAMES`), plus `REACTION_TRIGGERS` and `REACTIVE_NO_TRIGGER_UNIT`.
- The duplicate-component generator test was skipped: `test_semicolon_only_rows_and_duplicate_slots_are_kept` already covers it. `boots_of_speed_combine_priority_is_deterministic` was added in `src/items.rs`.
