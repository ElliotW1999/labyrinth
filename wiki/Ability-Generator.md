# Ability Generator

### Adding abilities (AbilityGenerator)

`data/abilities.csv` + `data/ability_effects.csv` (joined on `ability_id`) generate `src/abilities/generated.rs`: one ordinary `AbilityDefinition` per ability, built with the same builder as `catalog.rs`. `catalog::builtin` prefers a generated definition over the hand-written one.

```bash
python3 scripts/AbilityGenerator.py                       # data/abilities.csv data/ability_effects.csv
python3 scripts/AbilityGenerator.py a.csv a_fx.csv --check  # validate only
python3 scripts/AbilityGenerator.py --dry-run             # print what would change
```

```csv
id,name,slot,ability_type,target_type,target_team,cast_range,aoe_radius,cast_time,channel_time,cooldown,mana_cost,max_charges,charge_restore_time,projectile_type,projectile_speed,description
magnus_shockwave,Shockwave,Q,active,point,enemy,1200,200,0.3,,13|12|11|10|9|8|7,85|90|95|100|105|110|115,,,skillshot,1200,Sends out a wave of force that damages enemies in a line pulls each one toward the wave and slows it briefly
```

- Enums: `slot` Q/W/E/R/innate (R ⇒ ultimate), `ability_type` active/passive/toggle, `target_type` unit/point/direction/no_target/self, `target_team` enemy/ally/both/self, `projectile_type` none/targeted/targeted_each/skillshot/skillshot_single (`skillshot_single` stops at the first enemy it strikes, for hooks; `targeted_each` is described below)
- **Area targeting in CSV:** there is no separate `area` value. Author `target_type=point` with non-zero `aoe_radius` and `projectile_type none`; the generator emits Rust `TargetType::Area` (see `scripts/AbilityGenerator.py`). Ground-placed AoE uses the same cast / range rules as point spells.
- Level values: a single number or `a|b|c|d` (one per rank; arithmetic lists become `RankValue::from_level_1`, others a level table). Blank = not applicable
- Optional extra columns: `cast_backswing` (default 0.25), `max_rank` (else the level-list length, else 4 for R / 7 otherwise), `toggle_mana_per_sec` (toggle abilities only; level list allowed), `projectile_end_radius` (skillshots only; level list allowed), `projectile_origin_offset` (skillshots only; level list allowed), `cast_range_buffer` (`unit` targets only; level list allowed), `projectile_delay` / `projectile_acceleration` / `projectile_retarget_radius` (`targeted` projectiles of abilities only; single numbers)
- **`cast_range=global`:** the keyword (any case, never inside a level list) makes the range the whole map (`scale::GLOBAL_CAST_RANGE`, 25,000). The hero never walks into range, and the cast indicator draws no range ring. Items accept it too in their `cast_range` column. `cast_time` above 2.0 s (`MAX_CAST_POINT`) and `cast_backswing` above 2.0 s (`MAX_CAST_BACKSWING`) are capped (the generator warns)
- **`projectile_end_radius` (expanding skillshots):** for `skillshot` / `skillshot_single` only, above 0, and it needs `aoe_radius`. The wave's width grows linearly from `aoe_radius` to this value over its flight (the cast range). Blank keeps the width constant. Items reject it (they have no `aoe_radius`)
- **`cast_range_buffer` (lock-on leash):** `unit`-targeted abilities only, above 0. During the cast point the cast is cancelled for free (no mana, no cooldown; a queued unit command sees it as interrupted before firing) if the target is further than `cast_range + cast_range_buffer` from the caster, measured edge to edge like the cast range. A target that dies is a point and the cast still fires. Blank means no limit (the default). Warns with `cast_range=global`; abilities only for now
- **Seeking projectiles (`projectile_delay`, `projectile_acceleration`, `projectile_retarget_radius`):** `projectile_type=targeted` only, abilities only for now (items reject them). Each is a single number and setting any of them makes the missile a seeker (a blank one counts as 0): `projectile_delay` is the seconds it waits at the launch point (0 or more), `projectile_acceleration` the speed it gains per second once flying (units/s^2, 0 or more), `projectile_retarget_radius` (above 0) how far from the missile it looks for a new enemy when its target dies. See the Seeking projectile row in the mechanics table
- **`projectile_origin_offset` (skillshot start):** for `skillshot` / `skillshot_single` only, above 0. The wave starts this many units in front of the caster, towards the aim, instead of at the caster's centre, and still flies the full `cast_range` from there (and the width and `projectile_end_radius` apply from that start). Blank starts at the caster. Items reject it
- **`projectile_type=targeted_each`:** sends one homing projectile (no splash, `projectile_speed` applies) at every living, targetable unit of `target_team` inside `aoe_radius`, never the caster. It needs `target_type` `no_target` (around the caster, with the shockwave ring) or `point` (around the aim) and a non-zero `aoe_radius`; `target_team` cannot be `self`. Each projectile fires `on_projectile_hit` with the struck unit as `trigger_unit` (`affected_units` is not available there). Items reject it until they have an `aoe_radius` column
- Mechanics are derived: `targeted` / `skillshot` → projectile (skillshots hit every enemy along `cast_range`, width = `aoe_radius`); `no_target` + AoE → shockwave ring; `point` + AoE without a projectile → area ring

```csv
ability_id,effect_index,trigger,effect_target,effect_type,effect_id,value,damage_type,duration,radius,amount,scaling_type,scaling_value
magnus_shockwave,0,on_projectile_hit,trigger_unit,damage,,75|150|225|300|375|450|525,magical,,,,,
magnus_shockwave,1,on_projectile_hit,trigger_unit,movement,pull_point,150,,0.2,,,,
skewer,0,on_cast,caster,movement,dash,800|900|1000|1100|1200|1300|1400,,0.888889|1|1.111111|1.222222|1.333333|1.444444|1.555556,145,,,
skewer,1,on_unit_contact,trigger_unit,movement,drag_stun,,,,,,,
```

- `trigger`: reactive triggers (below) and `on_cast`, `on_projectile_hit`, `on_unit_contact` (a dash or charge touched an enemy; trigger unit = that enemy), `on_movement_end` (a dash or charge ended; trigger unit = the chased unit if reached, affected units = everyone touched), `on_impact`, `on_channel_tick`, `on_channel_end` (channel completed), `on_channel_interrupted` (channel cut short by silence/stun/mute; a new order fires neither), `on_expire`, `while_toggled` (toggle abilities only: `stat_modifier` or `apply_status,mana_shield` on `caster`, no duration; applied while the toggle is on; the upkeep is the ability row's `toggle_mana_per_sec`), or any other `on_…` name raised by a custom mechanic
- `effect_target`: `caster`, `cast_target`, `trigger_unit`, `affected_units`, `units_in_radius` (around the trigger point; `radius` or the ability AoE). Team: enemy and both abilities use their own `target_team`; ally and self abilities follow effect polarity, so a harmful row (damage, debuff, knockback) picks enemies and a beneficial row (heal, buff) picks allies. Items always follow polarity
- `effect_type` (+ `effect_id`): `damage` (`value` or `amount`, `damage_type` physical/magical/pure), `heal` (`effect_id=mana` restores mana), `apply_status` (`stun`, `mini_stun` (a stun with the per-owner id `<owner>_stun`: it refreshes only itself, so a short stun never shortens a longer one from another source; `remove_status,mini_stun` is rejected, name `<owner>_stun`), `root`, `silence`, `disarm`, `slow` (% from `amount`/`value`, taken from the target's base move speed; the status id is `<owner>_slow`, so slows from different sources coexist and add, the same source refreshes, and `remove_status,slow` is rejected: name the `<owner>_slow` id), `turn_rate_slow`, `turn_rate_bonus` (% from `amount`, 0..400), `move_speed_bonus` (% of base move speed from `amount`/`value`, above 0 up to 400; the id is `<owner>_move_speed_bonus`, so `remove_status` needs that id), `untargetable` (targeting only: attacks and hostile unit-target spells cannot pick the holder; areas, dash contact and in-flight projectiles still reach it; `cyclone` / `banish` stay fully untargetable), `phased`, `frozen` (stunned + physical immunity buff), `attack_point` (`value` = foreswing seconds 0..1.0; id `<owner>_attack_point`, so `remove_status` needs that id), `taunt_target` (the holder attacks the cast's unit target, even an ally; optional `value` = attack speed bonus; unit-target sources with `on_cast` / `on_projectile_hit` only), `forceful`, `flying`, `leash` / `tether` (`value` = radius), `debuff_immunity`, `damage_reduction` / `magic_resistance` / `status_resistance` (percent 1..100 from `amount`; ids `<owner>_damage_reduction` / `<owner>_magic_resistance` / `<owner>_status_resistance`, so `remove_status` needs that id), `damage_share` (percent 1..100 from `amount` of the holder's incoming damage that is also dealt to the applier; id `damage_share`; the target cannot be `caster` / `owner`), `convert_on_death` (`value` = radius; enemy creeps dying within it return as allies; id `convert_on_death`; timed only), `attack_charges` (a buff that only counts basic attacks: `amount` = attacks, a whole number from 1, level list allowed; `duration` required; id `<owner>_attack_charges`, so `remove_status` needs that id; rejected on `passive`, `while`, `while_toggled`; gate on it with `require_status`), `stacks` (a per-target stack of the id `<owner>_stacks`, duration required, undispellable; one more stack and a fresh duration on every application, all lost on expiry; read by `scaling_type=target_stacks`), `burn` (`value` = damage, `damage_type` required; per second by default, or per tick when `amount` sets the tick interval in seconds, above 0 and at most the duration — the first tick lands one interval after application, `floor(duration / interval)` ticks in total, flushed on natural expiry but not on dispel; the status id is `<owner>_burn`, so burns from different sources coexist and `remove_status,burn` no longer matches them), or any id as a timed `stat_modifier` (the effect id may carry the suffix `:independent`, e.g. `agility:independent`: every application then keeps its own status instance and timer instead of refreshing the same-id one; rejected on `passive`, `while` and `while_toggled` rows, and any other suffix is an error): besides the plain stats, the timed attributes `strength`, `agility` and `intelligence` (folded into the hero's attributes and reverted exactly; a negative value is a debuff), `attack_damage_pct` (a percent, 0..1000, of the target's base attack damage, snapshotted when applied), `cleave` (a fraction, 0..1, added to the target's cleave ratio; only melee attackers cleave) and the cone-shape stats `cleave_radius`, `cleave_end_radius`, `cleave_distance` (see Attack modifiers)), `remove_status` / `dispel` (`buff`/`debuff` dispel; on `dispel`, `amount` 1 makes it a strong dispel, blank or below 0.5 a basic one; any other id removes that status by id), `movement` (`knockback`, `pull`, `pull_point`, `dash`, `charge`, `drag`, `drag_stun`, `cyclone`; `value` = distance, `duration`, `amount` = charge speed (or the forward offset of `pull_point`), `radius` = dash / charge contact radius — see Forced movement below; `teleport` sends `caster` / `owner` / `cast_target` / `trigger_unit` to the cast's aim point, optionally bounded (`value` = max distance, `amount` = min distance) — see Teleport below), `stat_modifier` (`effect_id` = stat, `duration`; on a timed stat `amount` is the number of **attack charges**: the status ends after that many released basic attacks, hit or miss, or at the duration, whichever is first; whole numbers from 1, level list allowed, blank = unlimited; rejected on `passive`, `while`, `while_toggled` rows and resource stats), `custom` (registered id; receives `value`, `amount`, `duration`)
- `orbit` effect type (gate): `effect_id` = `on_<name>:<hit_radius>` (the custom trigger fired on each new contact, and the radius of every hitbox, above 0), `value` = number of orbiters (integer, at least 1), `duration` = seconds (above 0, level lists allowed), `radius` = orbit radius (blank = the ability's `aoe_radius`; items must set it), `amount` = seconds per revolution (above 0). The target must be `caster` / `owner`; the team follows the ability's `target_team` like `zone`. The first orbiter starts at the caster's facing and the rest are spaced evenly; the orbit follows the caster and ends with its duration or the caster's death. Put the damage in rows tagged with the custom trigger and the target `trigger_unit`; see `spirit_guardians` in `data/examples/`
- `attached_zone` effect type (gate): identical to `zone` (same columns and validation, and it feeds its `on_<name>` trigger the same way) except that the zone follows its caster: it is re-centred on the caster before every tick and ends when the caster dies. See `dark_pact` in `data/examples/`
- `zone` effect type (gate): `effect_id` = the custom `on_<name>` trigger fired on every tick, `value` = tick interval in seconds (above 0), `duration` = seconds the zone stays active (above 0, at least the interval), `amount` = delay before the first tick in seconds (0 or more, blank = 0), `radius` = zone radius (blank = the ability's `aoe_radius`; items must set it); level lists are allowed. The target must be `caster` / `owner`; the team follows the ability's `target_team` like `chain`. Put the damage and statuses in rows tagged with the custom trigger and the target `affected_units`; see `shrapnel` in `data/examples/`
- `pick_random` effect type (gate): `effect_id` = the custom `on_<name>` trigger fired once per pick, `value` = number of distinct picks (a single integer, at least 1; blank = 1), the target is the pool: `affected_units` or `units_in_radius`. The picked unit is the trigger unit of the fired trigger. Like `chain` it rejects built-in triggers, firing its own trigger and a trigger no row uses, and feeds the unreachable-trigger warning; it is not a `caster` / `owner` gate, and a `perform_attack` under a trigger it feeds from a zone or chain is rejected like any chain-fed trigger. See `rocket_barrage` in `data/examples/`
- `chain` effect type (gate): `effect_id` = the custom `on_<name>` trigger fired per hop, `value` = jumps (integer, at least 1), `radius` = jump radius in world units (above 0), `amount` = percent of the amount kept per jump (above 0, at most 100); the target is the start unit (`cast_target`, `trigger_unit`, `caster`/`owner`, `attacker`). Put the damage in rows tagged with the custom trigger; see `chain_lightning` / `storm_charm` in `data/examples/`
- `scaling_type`: `strength` / `agility` / `intelligence` / `attack_damage` × `scaling_value` (damage, heal and `health_drain`); `target_stacks` multiplies the amount by the target's `<owner>_stacks` count plus `scaling_value` (0 or 1; 1 counts the hit's own stack, as Fury Swipes does) and needs an `apply_status,stacks` row on the same owner; a target with no stacks takes nothing when the total is 0
- Healing and resource rows (no extra columns):

| `effect_type,effect_id` | `value` | `amount` | `damage_type` | Targets |
| --- | --- | --- | --- | --- |
| `resource,health_drain` | damage (scaling allowed) | heal ratio (default 1) | required | other units |
| `resource,mana_burn` | mana removed | damage per mana (default 1) | required | other units |
| `resource,mana_drain` | mana removed | ratio given to the caster (default 1) | blank | other units |
| `resource,spend_health` / `spend_mana` | amount spent | blank | blank | `caster` / `owner` only |
| `resource,health_to_mana` / `mana_to_health` | amount spent | gained per point spent (required, > 0) | blank | `caster` / `owner` only |
| `apply_status,heal_over_time` | HP per second (optional) | percent of max health per second, 0..100 (optional; at least one of `value` / `amount` above 0) | blank | any |
| `apply_status,attack_point` | seconds, 0..1.0 (required) | blank | blank | `caster` / any; id `<owner>_attack_point` |
| `apply_status,frozen` | blank | blank | blank | any (duration required) |
| `apply_status,taunt_target` | attack speed bonus while taunted (optional, >= 0) | blank | blank | `units_in_radius` around a unit cast; `target_type=unit`, `on_cast` / `on_projectile_hit` only |
| `require_status` | blank | blank | blank | `caster` / `owner` / `attacker`; `effect_id` = `on_<trigger>:<status_id>` (a keyword, runtime id or `<owner>_<stat>` id) |
| `apply_status,heal_reduction` | blank | percent 0..100 (required) | blank | any |
| `apply_status,heal_block` | blank | blank | blank | any |
| `apply_status,damage_reduction` / `magic_resistance` / `status_resistance` / `slow_resistance` | blank | percent, above 0 up to 100 (required) | blank | any |
| `apply_status,damage_share` | blank | percent, above 0 up to 100 (required) | blank | not `caster` / `owner`; the applier becomes the partner |
| `apply_status,convert_on_death` | radius, above 0 (required) | blank | blank | any; timed (not `passive` / `while`) |
| `orbit` | orbiters (integer, at least 1) | seconds per revolution, above 0 (required) | blank | `caster` / `owner` only; `effect_id` = `on_<name>:<hit_radius>`, `duration` required, `radius` = orbit radius |
| `apply_status,stacks` | blank | blank | blank | any (duration required) |
| `apply_status,attack_charges` | blank | attacks, whole numbers from 1 (required) | blank | any; `duration` required; not `passive` / `while` / `while_toggled` |
| `pick_random` | picks, integer from 1 (blank = 1) | blank | blank | `affected_units` / `units_in_radius`; `effect_id` = `on_<name>` |
| `apply_status,mana_shield` | absorb percent, above 0 up to 100 (required) | damage per mana, above 0 (required) | blank | any; allowed under `while_toggled` |
| `stat_modifier,{lifesteal,spell_lifesteal,heal_amp,heal_received_amp,mana_cost_amp}` | fraction (-1..1; `mana_cost_amp` only has the floor -1; -0.25 = 25% cheaper) | blank | blank | passive → `PassiveStat`; timed or toggled → `StatusSpec::ResourceModifier` |
| `stat_modifier,{health_regen,mana_regen,max_health,max_mana}` timed or toggled | flat | blank | blank | passive path unchanged |
| `stat_modifier,{strength,agility,intelligence}` timed or toggled | flat points (negative = debuff) | blank | blank | any; passive → `PassiveStat::Attribute`, timed → `StatusSpec::ResourceModifier` reconciled into `HeroAttributes` |
| `stat_modifier,<stat>:independent` timed | as the stat | blank (or attack charges as the stat) | blank | any; each application keeps its own timer (not on passive, while, while_toggled) |
| `apply_status,move_speed_bonus` | blank | percent of base move speed, above 0 up to 400 (required) | blank | any |
| `apply_status,untargetable` | blank | blank | blank | any (duration required); targeting only |

  Ratios are single numbers (no per-level lists). `stat_modifier` amps are fractions; `apply_status` percents are 0..100.

#### Reactive triggers in CSV

The `trigger` cell is `name[:arg[:arg]]`; the base name decides everything else (which owner may use it, which targets resolve). Both generators accept all of these for abilities (learned passives included) and items:

| `trigger` cell | Rust | Rules |
| --- | --- | --- |
| `on_damage_dealt`, `on_damage_taken`, `on_kill`, `on_death` | plain variants | `attacker` target = the unit that caused it for `on_damage_taken` / `on_death` |
| `on_health_below:30` | `OnHealthBelow { percent: 30 }` | whole number 1..=99 |
| `on_spell_cast`, `on_spell_hit` | plain | |
| `on_status_applied`, `on_status_removed` with `:buff`, `:debuff` or `:<status id>` | `(StatusFilter::Any / Buff / Debuff / Id)` | id is a status id (`stun`, ...) or a timed-stat `<owner>_<stat>` id |
| `on_cooldown_ready` | plain | invalid on passive abilities and on items with no `on_use` |
| `on_interval:5` | `OnInterval { millis: 5000 }` | seconds, at least 0.1 |
| `aura` | plain (`Aura`) | no argument; rows are `stat_modifier` (timed or resource stats) or `apply_status` on `units_in_radius` (items must set `radius`); `duration` blank (the generator writes `AURA_LINGER` 0.5); `:independent`, `amount` on a `stat_modifier` and the statuses `stacks`, `shield`, `attack_charges`, `mana_shield`, `damage_share`, `convert_on_death`, `taunt_target`, `mini_stun`, `temporary_mana` are rejected |
| `on_move` | plain | every row must be `accumulate` |
| `on_enemy_enter:600`, `on_enemy_leave:600` | `{ radius: 600 }` | radius above 0 |
| `while:health_below:50`, `while:moving`, `while:stationary`, `while:enemies_near:600:2`, `while:allies_near:600:1`, `while:has_status:<id>` | `While(Condition::...)` | rows are `apply_status` (or a timed `stat_modifier`), blank `duration`, target `owner` / `caster`; `has_status` may not name the status the row holds |

New effect types: `pseudo_proc` (same columns as `proc`) and `accumulate` (`effect_id` = custom `on_<name>` trigger, `value` = threshold above 0, levels allowed). Both feed the unreachable-custom-trigger warning like `proc`. The generator rejects `perform_attack` under reaction triggers (also under a custom trigger fed from one), and warns when two `while` rows of one owner hold the same status id: held statuses share an id and would fight, so give each its own.

Recipes:

| Mechanic | Rows |
| --- | --- |
| Thorns | `on_damage_taken,attacker,damage,,40,magical` |
| Heal on kill | `on_kill,owner,heal,,150` (the victim may be gone next frame: target the owner or `units_in_radius`) |
| Last stand | `on_health_below:25,owner,apply_status,shield,,,4,,300` |
| Mana on cast | `on_spell_cast,owner,heal,mana,30` |
| Pulse | `on_interval:3,units_in_radius,damage,,60,magical,,500` |
| Every 1000 units walked | `on_move,owner,accumulate,on_far_walk,1000` + `on_far_walk,owner,apply_status,phased,,,2` |
| Real aura (+2 armor to allies within 1200) | `aura,units_in_radius,stat_modifier,armor,2,,,1200` |
| Aura-ish slow | `on_enemy_enter:600,trigger_unit,apply_status,slow,,,2,,30` |
| Conditional bonus | `while:health_below:50,owner,stat_modifier,attack_damage,40` |
| Pseudo-random stun | `on_attack_hit,owner,pseudo_proc,on_bash,25` + `on_bash,trigger_unit,apply_status,stun,,,1.2` |
| Storm Mail (every 300 damage taken arcs lightning from the attacker) | `on_damage_taken,owner,accumulate,on_overload,300` + `on_overload,trigger_unit,chain,on_storm_arc,2,,,450,75` + `on_overload,trigger_unit,damage,,80,magical` + `on_storm_arc,trigger_unit,damage,,80,magical`; under a derived custom trigger the attacker is `trigger_unit`; see `storm_mail` in `data/examples/` |

Samples: `thorn_mail`, `bloodstone`, `aeon_charm`, `arcane_echo`, `pulse_core`, `phase_treads`, `sentinel_ward`, `berserker_blood`, `prd_basher` and the passive ability `thornskin` in `data/examples/`; the matching bad rows (`reactor`, `aura`) are in `data/examples/invalid/`.

### Generator validation

Both generators also warn when a row uses a trigger no game system emits yet (only `on_channel_tick` and `on_expire`); the row is generated but never fires. They reject `perform_attack` under a reaction trigger and under any custom trigger a `chain` or `zone` row feeds (a chain hop or zone tick carries no attack, so the extra attack could re-fire the trigger that fed it without end).

Both generators validate before writing anything and print `file:row [field=value] message` for every problem (exit 1): duplicate ids, effect rows for unknown owners, duplicate `effect_index` per owner, unknown enums, bad numbers / level lists (wrong length, negative), damage without an amount, `apply_status` / `custom` without `effect_id`, projectile speed without a projectile, `targeted` without unit targeting / `skillshot` without point or direction, charge restore time without charges, resource rows that break their rules (`spend_*` / conversion on another unit, `mana_burn` without `damage_type`, `mana_shield` without `amount`, `heal_reduction` above 100, a conversion without a ratio, scaling on `mana_drain`, a fraction stat outside -1..1), and trigger/target combinations that cannot resolve (e.g. `cast_target` on a `no_target` ability, `trigger_unit` on `on_cast` for items). `data/examples/` holds valid samples (shockwave, skewer, frost_hammer, blink_item, …) and `data/examples/invalid/` one row per error.

Re-running overwrites: each owner lives in a `// <ability:id>` / `// <item:id>` block, CSV rows replace their own blocks and other blocks are kept, and the output is rustfmt-ed.
