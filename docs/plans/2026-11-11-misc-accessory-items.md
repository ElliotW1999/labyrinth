# Plan: Voodoo Mask, Morbid Mask, Ghost Scepter, Soul Ring, Power Treads

Related plans: `2026-11-07-item-ability-architecture-cleanup.md` (single effect executor, item instances), `2026-11-08-healing-resource-mechanics.md` (lifesteal, `spend_health`, max-cap reconciliation, invariant 3), `2026-11-10-status-effects-completion.md` (Ethereal row), `2026-11-06-generator-shared-architecture.md`.

## Resolved decisions (coordinator)
- Power Treads move speed: **+55** (as requested).
- Ethereal: **exact ×1.3** magical damage via negative `magic_resist_fraction` (D2 approved).
- Soul Ring buff: **undispellable**, independent per use.
- OR components: first owned alternative in CSV order is consumed.
- `data/items2.xlsx` / `item_effects2.xlsx` untouched.

## 1. Existing systems

| Need | Status | Where |
|---|---|---|
| Lifesteal / spell lifesteal passives | **Done** — `PassiveStat::Lifesteal/SpellLifesteal`, fractions (0.18), applied in `apply_damage_events` | `scripts/generator_common.py` `PASSIVE_STATS`/`FRACTION_STATS`; `src/items.rs` `adjust_passives`; `src/combat.rs:525-541` |
| Ethereal (disarm + attack-immune + physical-immune + magic amp) | **Done**, but amp is magic-resist *rating* cut (exact only at 0 MR) | `src/status.rs:759-776` `StatusEffect::ethereal`; generator `apply_status,ethereal` (amount = percent). Self-cast kind = Buff (`effects.rs:840`) |
| Non-lethal health cost | **Done** — `resource,spend_health` floors at 1 HP; not damage | `src/vitals.rs:191-231` |
| Temporary mana with max-mana overflow | **Missing** — timed `max_mana` modifiers use PreserveRatio; no unspent tracking | — |
| Duplicate components (2x) | **Works** — `component_needs` builds a multiset (`items.rs:1494`) | `src/items.rs`, `scripts/ItemGenerator.py` |
| OR components | **Missing** — generator splits on `[;|]` (`|` == `;`); no data uses `|` | `ItemGenerator.py` `parse_item` |
| Per-instance item state | cooldown/charges/stack on `ItemInstance` (`items.rs:189`); no item toggles | — |
| Toggle actives | ability-only (`AbilityBehavior::Toggle`) | `src/abilities/passive.rs` |
| Reconcile-a-stat-bundle pattern | `sync_ability_passives` | `src/abilities/passive.rs:118-155` |

## 2. Decisions
- **D1 Soul Ring:** 150 HP paid up front via `spend_health` (cannot kill; mana still granted). Bonus mana is spent first: any mana drop while live reduces unspent bonus. At 10 s, unspent bonus removed, overflow max mana removed, current clamped. Undispellable, `independent`. Cooldown 25 s, no mana cost.
- **D2 Ghost Scepter:** self `ethereal` Buff 4 s, cooldown 22 s. `StatusEffect::ethereal` uses negative `magic_resist_fraction` (multiplicative, exact ×1.3).
- **D3 OR components:** `;` separates slots, `|` separates alternatives in a slot. Runtime `components: &'static [&'static [ItemId]]`. Combiner takes first available alternative in CSV order.
- **D4 Power Treads:** `ItemInstance.selected_attribute: Option<Attribute>`. Starts on the attribute of the alternative consumed (Belt=Str, Band=Agi, Robe=Int, derived from the component's single positive attribute passive; Strength fallback incl. direct purchase). Active cycles Str→Agi→Int→Str, no cooldown; muted/stunned block it. Toggling never kills (health floored at 1 for living hero). Sale undo keeps selection; purchase undo restores the exact alternative.
- **D5** `attribute_toggle` is an optional `items.csv` column (item-domain instance rule), not an effect row.

## 3. Data

### `data/items.csv` — append header column `attribute_toggle`
```
voodoo_mask,Voodoo Mask,miscellaneous,650,325,,,FALSE,,FALSE,,15% Spell Lifesteal,VM,,,,,,,,,,,,,,,
morbid_mask,Morbid Mask,miscellaneous,900,450,,,FALSE,,FALSE,,18% Lifesteal,MM,,,,,,,,,,,,,,,
ghost_scepter,Ghost Scepter,miscellaneous,1500,750,,,FALSE,,FALSE,,"+5 All Attributes. Active: Ethereal for 4s (immune to physical damage and attacks, disarmed, takes 30% more magical damage)",GS,22,,,,,,,,,,,,,,
soul_ring,Soul Ring,accessories,700,350,,,FALSE,,FALSE,,"+6 Strength, +2 Armor. Active: pay 150 HP for 150 temporary mana for 10s (raises max mana by any excess)",SR,25,gauntlets_of_strength;gauntlets_of_strength;ring_of_protection;recipe_soul_ring,,,,,,,,,,,,,
recipe_soul_ring,Recipe: Soul Ring,recipe,245,122,,,FALSE,,FALSE,,Combines with components into Soul Ring,R-SR,,,,,,,,,,,,,,,
power_treads,Power Treads,accessories,1400,700,,,FALSE,,FALSE,,"+55 Move Speed, +25 Attack Speed, +10 selected attribute. Active: switch Strength / Agility / Intelligence",PT,0,boots_of_speed;gloves_of_haste;belt_of_strength|band_of_elvenskin|robe_of_the_magi,,,,,,,,,,,,,10
```
Cost checks: Soul Ring 2×140+175+245 = 700; Power Treads 500+450+450 = 1400.

### `data/item_effects.csv`
```
voodoo_mask,0,passive,owner,stat_modifier,spell_lifesteal,0.15,,,,,,
morbid_mask,0,passive,owner,stat_modifier,lifesteal,0.18,,,,,,
ghost_scepter,0,passive,owner,stat_modifier,strength,5,,,,,,
ghost_scepter,1,passive,owner,stat_modifier,agility,5,,,,,,
ghost_scepter,2,passive,owner,stat_modifier,intelligence,5,,,,,,
ghost_scepter,3,on_use,owner,apply_status,ethereal,,,4,,30,,
soul_ring,0,passive,owner,stat_modifier,strength,6,,,,,,
soul_ring,1,passive,owner,stat_modifier,armor,2,,,,,,
soul_ring,2,on_use,owner,resource,spend_health,150,,,,,,
soul_ring,3,on_use,owner,apply_status,temporary_mana,150,,10,,,,
power_treads,0,passive,owner,stat_modifier,move_speed,55,,,,,,
power_treads,1,passive,owner,stat_modifier,attack_speed,25,,,,,,
```

## 4. Phases (one commit each; regenerate with `python3 scripts/ItemGenerator.py` after data/generator changes)

### Phase 1 — lifesteal masks
Rows; `src/items.rs` `passive_stat_lines`: percent rows for `lifesteal` and `spell_lifesteal` (value ×100).

### Phase 2 — Ethereal accuracy + Ghost Scepter
`status.rs`: `StatusEffect::ethereal` sets `magic_resist_fraction: -amp` (amp clamped 0..0.9) instead of rating cut; `magic_damage_factor` filters `!= 0.0`, clamps each fraction to `-0.9..=1.0`. Update Ethereal row in `2026-11-10-status-effects-completion.md`. Ghost Scepter rows.

### Phase 3 — temporary mana + Soul Ring
- `status.rs`: `StatusEffect` fields `temporary_mana: f32` (unspent), `temporary_mana_pending: bool`, `temporary_max_mana: f32` (overflow added). Constructor `StatusEffect::temporary_mana(amount, duration)`: Buff, `independent`, `undispellable`, `DispelType::None`. `consume_temporary_mana(statuses, amount)` oldest-first like `consume_shields`, skipping pending.
- `abilities/definition.rs`: `StatusSpec::TemporaryMana { amount: RankValue }` + `build_status_effect` arm; tooltip arms in `abilities/tooltip.rs` ("Temporary mana").
- `vitals.rs`: component `AppliedTemporaryMana { unspent, extra_max, last_mana }`, system `sync_temporary_mana` (skip dead). Per frame:
  1. If component: `spent = max(0, last_mana - mana.current)`; `consume_temporary_mana(spent)`; `unspent -= spent` (floor 0).
  2. `live` = Σ granted `temporary_mana`; `lost = max(0, unspent - live)`; remove `min(lost, mana.current)`; `unspent = live`.
  3. `want_extra` = Σ granted `temporary_max_mana`; if `< extra_max`, lower `mana.max` by diff, clamp current.
  4. Pending grants: `overflow = max(0, current + amount - max)`; `max += overflow`; `current += amount`; record overflow; clear pending; update totals.
  5. `last_mana = current`; remove component when nothing live and totals 0; insert lazily.
- Scheduling: chain right after `sync_status_attributes` in `AbilitiesPlugin` (`src/abilities/mod.rs` ~144) and the test app (~806).
- Generator: `STATUSES["temporary_mana"]`; amount from `value` (or `amount`) > 0; reject `while`/`while_toggled`; duration required; emit `StatusSpec::TemporaryMana { amount }`.
- Docs: amend invariant 3 of `2026-11-08-healing-resource-mechanics.md`.

### Phase 4 — OR component slots
- `ItemGenerator.py`: split on `;` then `|`; `Item.components: list[list[str]]`. Errors: unknown alternative, self-reference, duplicate id within slot, > `MAX_COMPONENTS` slots. Warning: alternatives with different costs. Render `components: &[&[ItemId::A], &[ItemId::B, ItemId::C]],`.
- `src/items.rs`: `ItemDefinition.components: &'static [&'static [ItemId]]`; `recipe_components()` returns slots; `component_options(result)` iterates cartesian product in CSV order; `combine_ready_items` uses first assemblable option. Tooltip prints "A / B / C (450g)".
- `src/ui.rs` (~1999, ~2082): one detail row per slot, label joins with " / ", click opens first alternative.

### Phase 5 — attribute toggle + Power Treads
- Generator: optional `attribute_toggle` column (> 0) → `has_use`; errors with on_use/channel/projectile rows, consumable, or activation columns. Emit `attribute_toggle: Some(10.0)` / `None`.
- `src/items.rs`: `ItemDefinition.attribute_toggle: Option<f32>`; `has_active()` true for toggles; `ItemInstance.selected_attribute`; `Inventory::try_add_at(id) -> Option<usize>`; combine sets selection from chosen alternative; `resolve_item_uses` advances selection Str→Agi→Int→Str. `AppliedItemToggles(ItemPassives)` + `sync_item_attribute_toggles` modelled on `sync_ability_passives` (remove old, apply new, floor health at 1 for living hero), chained `(resolve_item_uses, sync_item_attribute_toggles, execute_item_mechanics)` in plugin and test apps. Tooltip line for toggles.
- `src/ui.rs`: inventory label / slot tooltip show selected attribute.

### Phase 6 — docs
Update `docs/architecture/composition.md` where item columns/components/statuses are listed.

## 5. Invariants
1. Static item passives still only applied/removed by shop & consumable paths; selected-attribute bonus only by `sync_item_attribute_toggles` (no double counting across buy/sell/undo/combine).
2. Toggling never kills, no cooldown; selling/undoing treads removes the bonus within one frame.
3. Temporary mana: after all grants end, `mana.max` equals the value without them; current ≤ max; removal never exceeds unspent bonus; spent mana never taken back.
4. All removal paths reconcile through `sync_temporary_mana`.
5. Soul Ring HP cost is not damage (no lifesteal, no on_damage_taken, can't kill).
6. Ethereal: magical ×(1+amp) on top of own resistance; still physical-immune, attack-immune, disarmed.
7. OR slots consume exactly one alternative; undo restores exact ids; `;`-only rows behave as before.
8. Generated code changes only via regeneration.

## 6. Tests
**Generator** (`cd scripts && python3 -m unittest test_generator_common`): pipe alternatives slot rendering; duplicate slots kept; alternative errors; attribute_toggle field/active/errors; temporary_mana status emit/errors; `python3 scripts/ItemGenerator.py --check`.

**Rust**:
- status/vitals: overflow raises max (450/500 → 600/600); no overflow (100/500 → 250/500); unspent removed on expiry (→ 450/500); spent bonus not removed again; overspent restores max only; two grants expire independently; remove-by-id takes bonus; not dispellable.
- combat: keep `ethereal_takes_no_physical_and_amplified_magic`; add `ethereal_amp_is_relative_to_holder_resistance` (100 → 72.5 × 1.3).
- items (shop): masks lifesteal + sell revert; soul ring needs two gauntlets + exact undo; power treads build from each alternative with matching start attribute; undo restores used alternative; undo sale keeps selection; update recipe/has_active tests; catalog values; lifesteal tooltip percent.
- items ECS: treads toggle cycles and moves bonus; toggle never kills; selling removes bonus.
- abilities ECS: soul ring spends HP and grants temporary mana (500→350 HP; 450/500 → 600/600 → 450/500 after 10 s; cd 25); soul ring can't kill but grants mana; ghost scepter ethereal buff (4 s, physical 0, magic ×1.3, can't attack/be attacked).
- `cargo test`, `cargo clippy`, regenerate + `git diff` shows only intended changes.

## 7. Risks
- Every net mana drop while a grant is live counts as spending the bonus (mana burn, toggle upkeep, Int→Str treads switch, PreserveRatio drop). Matches "spent first"; multi-grant attribution within one frame approximate, totals exact.
- Ordering depends on the chained schedules noted above.
- 35 `ItemDefinition` test literals need `attribute_toggle: None` (or a `#[cfg(test)]` helper).
- `|` changes meaning from alias of `;` to alternative (no current data uses it).
- Ethereal no longer shows as a magic-resist drop in the stat panel.
