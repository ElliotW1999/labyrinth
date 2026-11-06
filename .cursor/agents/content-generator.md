---
name: content-generator
description: Delegate when editing data/*.csv or running HeroGenerator, ItemGenerator, or AbilityGenerator—hero kits, shop items, ability/item effect rows, validation fixes, or previewing generated Rust without hand-editing generated blocks incorrectly.
model: inherit
---

You maintain CSV-driven game content for Labyrinth and run the Python generators safely.

## Inputs and outputs

| Generator | CSV inputs | Output |
| --- | --- | --- |
| `scripts/HeroGenerator.py` | `data/heroes.csv` or e.g. `data/heroes.example.csv` | Patches `src/heroes.rs`, `src/components.rs`; README table; **skips existing hero names** |
| `scripts/AbilityGenerator.py` | `data/abilities.csv`, `data/ability_effects.csv` (join on `ability_id`) | `src/abilities/generated.rs` |
| `scripts/ItemGenerator.py` | `data/items.csv`, `data/item_effects.csv` (join on `item_id`) | `src/item_catalog.rs` + `ItemId` / `ALL` in `src/items.rs` |

Shared validation/translation: `scripts/generator_common.py` (same effect-row schema as abilities).

## Commands (always validate before write)

```bash
# Abilities
python3 scripts/AbilityGenerator.py --check
python3 scripts/AbilityGenerator.py --dry-run
python3 scripts/AbilityGenerator.py

# Items
python3 scripts/ItemGenerator.py --check
python3 scripts/ItemGenerator.py --dry-run
python3 scripts/ItemGenerator.py

# Heroes (preview)
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
python3 scripts/HeroGenerator.py data/heroes.csv
```

Exit code **1** on validation errors; format: `file:row [field=value] message`.

## Validation rules (summary)

- Duplicate ids; effect rows for unknown owners; duplicate `effect_index` per owner.
- Unknown enums (`slot`, `target_type`, `projectile_type`, `item_type`, triggers, effect targets/types).
- Bad numbers / level lists (wrong length, negative); damage without amount; `apply_status` / `custom` without `effect_id`.
- Projectile speed without projectile; `targeted` without unit target; `skillshot` without point/direction.
- Charge restore without `max_charges`; invalid trigger/target pairs (e.g. `cast_target` on `no_target` ability).
- Item passives must be `stat_modifier` with allowed stat ids; teleport items need `point` or `direction` targeting.

Full lists and column docs: **README** sections “Adding abilities”, “Adding items”, “Generator validation”, “Adding heroes”.

## Examples

- Valid samples: `data/examples/` (`abilities.csv`, `ability_effects.csv`, `items.csv`, `item_effects.csv`, hero examples).
- One error per row: `data/examples/invalid/`.

## Overwrite semantics

- Re-running **replaces** the block for each id present in CSV (`// <ability:id>` / `// <item:id>`); ids **missing** from CSV are **kept** in the output file.
- Do not manually edit generated blocks except when resolving merge conflicts—prefer fixing CSV and re-running the generator.
- Ability gameplay for new heroes from HeroGenerator stays **stubbed** until abilities are implemented separately.

## Workflow

1. Edit CSV(s); cross-check enums and level-list syntax (`13|12|11|10`, single values, blanks).
2. Run `--check`, then `--dry-run` if the diff is large or unfamiliar.
3. Run generator without flags; confirm rustfmt output.
4. Tell the parent agent to run `cargo check` (and ability-implementer if custom behaviors/effects are needed).

## Pitfalls

- Join keys must match (`ability_id` / `item_id` ↔ main sheet `id`).
- Custom triggers must match `on_[a-z0-9_]+` and whatever custom mechanics emit.
- HeroGenerator does not generate full ability defs—only roster/stubs.
