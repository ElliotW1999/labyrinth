# Items

### Items

- Gold shop near each base; click an item for its build tree (components + recipe scraps); buy from the detail popup. Recipes are hidden from the default shop grid.
- Six inventory slots; RMB → **Sell (50%)** near shop; actives use ASDZXC through the same activation pipeline as abilities (no target and self fire on the key; unit, point, and direction enter targeting mode)
- Heroes carry a `StatusEffects` list for buffs / debuffs
- Heroes and creeps separate by collision size without pushing each other (moving units yield to stationary ones); the **forceful** buff can shove (e.g. Vanguard Shockwave). **Phased** units and units in forced movement (Dash, knockback, pull) ignore unit collision.
- Every item is an `ItemDefinition` (`src/item_catalog.rs`): price, sell value, tier, rarity, stacking, charges, recipe components, and `AbilityEffectEntry` effects. Passive `PassiveStat` entries fold into stats; `on_use` (hotkeys), `on_attack` and `on_attack_hit` fire `AbilityTriggerEvent`s with `EffectSource::Item`, resolved by the same effect systems as abilities

### Adding items (ItemGenerator)

`data/items.csv` + `data/item_effects.csv` (joined on `item_id`) generate `src/item_catalog.rs` and patch the `ItemId` enum / `ItemId::ALL` in `src/items.rs`.

```bash
python3 scripts/ItemGenerator.py            # data/items.csv data/item_effects.csv
python3 scripts/ItemGenerator.py --check    # validate only
```

```csv
id,name,item_type,cost,sell_value,tier,rarity,stackable,max_stack,consumable,max_charges,description
frost_hammer,Frost Hammer,weapon,2800,1400,3,rare,false,,false,,A heavy weapon imbued with frost
```

- `item_type` component/weapon/armor/accessory/consumable/utility/recipe, `rarity` common/uncommon/rare/epic/legendary; optional extras `short_label`, `cooldown`, `components` (`;`-separated ids), `color_r/g/b`
- Optional activation columns on items with `on_use` or a channel trigger (blank = instant no-target use): `target_type` (`unit`, `point`, `direction`, `no_target`, `self`), `target_team` (`enemy`, `ally`, `both`, `self`), `cast_range`, `cast_time`, `channel_time`, `mana_cost`, `projectile_type`, `projectile_speed` (`cooldown` remains on the item row). A `projectile_type` gives the item the same projectile mechanic abilities use (`ItemDefinition::mechanics`); put its consequences on `on_projectile_hit` / `on_impact` rows, which are only valid for projectile items (see `lightning_staff` in `data/examples/`). Unit targets require `target_team`. Point, unit, and direction targets require `cast_range`. Teleport items must be `point` or `direction` so the destination is the cast aim.
- Effects use the ability effect schema with item triggers `passive` (must be `stat_modifier`: `max_health`, `health_regen`, `max_mana`, `mana_regen`, `attack_damage`, `attack_speed`, `armor`, `magic_resist`, `move_speed`, `strength`, `agility`, `intelligence`, `spell_amp`, `outgoing_physical_amp`, `incoming_damage_amp`, `damage_block`, `damage_reflect`, `lifesteal`, `spell_lifesteal`, `heal_amp`, `heal_received_amp`, `mana_cost_amp`, `crit_chance`, `crit_multiplier`, `evasion`, `accuracy`, `cleave`, `splash`, `attack_range`, `attack_projectile_speed`; `health` and `mana` are aliases of `max_health` and `max_mana`), `on_use`, `on_unit_contact` / `on_movement_end` (after a dash or charge from `on_use`), `on_attack`, `on_attack_hit` (abilities accept these too), `on_channel_end` / `on_channel_interrupted` (channelled actives only), reactive triggers (`on_damage_dealt`, `on_damage_taken`, `on_kill`, `on_death`, `on_health_below:N`, `on_status_applied`, `on_move`, `on_interval:ms`, ... and `while:<condition>` rows; see Reactive triggers), `on_projectile_hit` / `on_impact` (projectile items only), custom `on_<name>` triggers fed by `proc` / `pseudo_proc` / `require_status` / `counter` / `accumulate` / `chain` / `pick_random` rows (a custom trigger nothing fires warns). New effect types: `proc` (`effect_id` = custom trigger, `value` = chance % in (0,100], owner targets), `pseudo_proc`, `accumulate`, `chain`, `zone`, `attached_zone`, `counter` (`value` = integer >= 2), `perform_attack` (`value` = damage % default 100, `amount` = max targets, `effect_id` blank or `exclude_trigger_unit`; its `units_in_radius` area is centred on the attacker), `pick_random`; new statuses `evasion`, `accuracy`, `true_strike`; timed stat `attack_range`, and targets `owner`, `cast_target` (unit and self actives), `trigger_unit`, `attacker`, `affected_units`, `units_in_radius`
- Consumables spend a charge (or one of the stack) when the use fires, and disappear when empty. A rejected activation spends nothing. Item charges never regenerate, so a `charge_restore_time` on an item is an error. `scaling_type=distance` is also rejected on items (they have no rank-scaled cast range); every other scaling works
