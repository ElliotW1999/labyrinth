# Queen of Pain hero — implementation spec

Status: **approved by user 2026-11-08** (§12 confirmed). Replaces the draft of the same name.

Related plans: `docs/architecture/composition.md`, `2026-11-08-mechanic-composition-layers.md`, `2026-11-07-movement-mechanics.md`, `2026-11-06-status-effects-catalog.md`, `2026-11-06-activation-targeting-mechanics.md`, `2026-11-06-damage-mechanics.md`, `2026-11-06-generator-shared-architecture.md`.

## 1. Scope and decisions

| Item | Decision |
|---|---|
| Attributes | STR 20 + 2.4, AGI 20 + 2.5, INT 25 + 3.4. Primary INT |
| Hero stats | Variant B: bases are back-solved so the level-1 totals equal the spec (HP 560, HP regen 2.25, mana 375, mana regen 1.25, armor 0, MR 27.5%). See §9 |
| Cooldowns | Q `11\|9\|7\|5\|3\|3\|3`. W `12\|10\|8\|6\|4\|2\|1`. R `110\|95\|80\|65` |
| W | Display name "QoP Blink", id `queen_of_pain_blink` (naming in §8) |
| Shadow Strike projectile speed | Default (`SPELL_PROJECTILE_SPEED`, ~884) |
| Magnus | Skipped. HeroGenerator runs on a scratch CSV holding only the Queen of Pain row |
| Hero example | `data/heroes.example.csv` |
| Approved features | C1, G4, G5, G6, G8, G11, G13, G14, G16 |
| Rejected | G15: `cast_range` = travel distance = 900. G18: knockback stays "away from contact point" |

Rank counts: basics 7, ultimates 4. Values are extrapolated arithmetically, except the cooldowns the user overrode.

| Ability | Field | Ranks |
|---|---|---|
| Q | cast range | 450, 500, 550, 600, 650, 700, 750 |
| Q | cooldown | 11, 9, 7, 5, 3, 3, 3 |
| Q | mana | 100, 105, 110, 115, 120, 125, 130 |
| Q | hit damage (magical) | 50, 80, 110, 140, 170, 200, 230 |
| Q | slow % (16 s) | 25, 35, 45, 55, 65, 75, 85 |
| Q | DoT per 3 s tick (16 s → 5 ticks) | 20, 40, 60, 80, 100, 120, 140 |
| W | max distance (min 200, global cast range) | 1075, 1150, 1225, 1300, 1375, 1450, 1525 |
| W | cooldown / mana | 12, 10, 8, 6, 4, 2, 1 / 65 |
| E | damage (magical, radius 600) | 90, 175, 260, 345, 430, 515, 600 |
| E | cooldown / mana | 7.5, 7, 6.5, 6, 5.5, 5, 4.5 / 120 |
| R | mana | 250, 400, 550, 700 |
| R | pure damage per instance (×14, 0.1 s apart) | 23, 34, 45, 56 (totals 322, 476, 630, 784) |
| R | cooldown | 110, 95, 80, 65 |

## 2. New CSV surface

Anything not listed here reuses existing columns with their existing meaning.

| Kind | Name | Where | Feature |
|---|---|---|---|
| Header columns (already parsed by HeroGenerator) | `base_health_regen`, `base_mana_regen`, `base_attack_time`, `attack_point`, `attack_backswing` | `data/heroes.csv`, `data/heroes.example.csv` | C1 |
| New column | `attack_projectile_speed` (optional, ranged heroes only) | Heroes CSVs | G4 |
| New column | `projectile_end_radius` (optional, level list, skillshots only) | `abilities.csv` (and items, as an optional activation field) | G14 |
| New enum value | `projectile_type=targeted_each` | Abilities. Items reject it until they have an `aoe_radius` column | G13 |
| New keyword | `cast_range=global` | Abilities and items (shared `parse_activation_spec`) | G11 |
| Existing columns, new meaning | `movement,teleport`: `value` = max distance, `amount` = min distance. Both blank keeps today's behaviour | Effect CSVs | G11 |
| Existing columns, new meaning | `apply_status,burn`: `amount` = tick interval in seconds. When set, `value` is damage per tick; when blank, `value` is damage per second as today | Effect CSVs | G8 |

## 3. Feature designs, in implementation order

Each step must build and pass tests before the next one starts. Regenerate after every generator change. Generated files (`generated.rs`, `item_catalog.rs`) may only show the diffs a step names (I8).

### Step 1 — G5/G6: raise timing caps
- `src/basic_attack.rs`: add `pub const MAX_ATTACK_SWING: f32 = 1.0`. Use it in `heroes.rs::hero_combat` to clamp `attack_point` and `attack_backswing`, and in the basic-attack windup clamp.
- `src/abilities/definition.rs`: add `pub const MAX_CAST_BACKSWING: f32 = 1.0`. `AbilityDefinition::timing` clamps backswing to it; the cast point cap stays at 0.5.
- Update the doc comments on `CombatStats` and `AbilityDefinition`.
- AbilityGenerator warns when `cast_backswing > 1.0`. HeroGenerator rejects `attack_point` or `attack_backswing` outside 0..1.0.
- Existing data is unaffected, since every current value is ≤ 0.5.
- Tests:
  - A hero combat test keeps an attack point of 0.56.
  - A timing test clamps backswing at `MAX_CAST_BACKSWING`.
  - The basic-attack windup honours 0.56.
  - Find and update any existing test that asserts the old 0.5 clamp.

### Step 2 — C1 + G4: hero attack projectile speed
- `hero_combat` gains an `attack_projectile_speed: Option<f32>` parameter.
  - `Some(s)` sets `attack_projectile_speed_bonus = s - basic_attack::projectile_speed_for(attack_range)`. For range 550 that is 1160 + 340 = 1500.
  - The five hand-written hero arms pass `None`.
  - Document that heroes may carry a base offset in this field.
- HeroGenerator:
  - Adds `HeroRow.attack_projectile_speed` (optional, > 0). It warns and ignores the value on melee heroes.
  - Emits `Some(x)` or `None`.
  - Update the generator docstring and the README section on adding heroes.
- CSV: add the 5 C1 columns plus `attack_projectile_speed` to the headers of `data/heroes.csv` and `data/heroes.example.csv`. Short rows parse as blank.
- Test: a hero with `Some(1500.0)` and range 550 has an attack projectile speed of 1500.

### Step 3 — G8: ticking burns with per-owner ids
- `StatusSpec::Burn` becomes `{ id, amount: RankValue, damage_type, tick_interval: RankValue }`.
  - `amount` is per second when the interval is 0, and per tick otherwise.
  - A new constructor, `StatusEffect::burn_ticking(id, per_tick, interval, damage_type, duration)`, builds the ticking form.
- `StatusEffect` gains `dot_interval`, `dot_per_tick`, `dot_timer` and `dot_ticks_left`, all defaulting to 0.
  - `ticks = floor(duration / interval + 1e-4)`, so 16 s / 3 s = 5 and 1.4 s / 0.1 s = 14.
- `tick_status_effects`, for ticking effects:
  - Accumulate the timer. While the timer has reached the interval and ticks remain, emit `DamageEvent::dot` and decrement.
  - On natural expiry, flush any owed ticks, so the count is exact whatever the frame rate. Dispel or refresh does not flush.
  - The first tick lands one interval after application.
  - Continuous DoT is unchanged.
- Ids: generated burns use `<owner>_burn` (the same pattern as timed stat modifiers), so DoTs from different sources coexist. Hand-written burns keep `"burn"`, and `runtime_status_id` accepts `<owner>_burn`. No current CSV row uses burn.
- Generator burn branch:
  - `value` is required.
  - Optional `amount` must be > 0 and ≤ duration for every rank. It is emitted as `tick_interval`, and as `RankValue::ZERO` when blank.
  - Drop the old "amount as dps" fallback.
- Tooltip: "X per Ys" for ticking burns, "X per second" otherwise.
- Ticks are `DamageKind::Dot`. As with existing DoT rules, they fire no damage-dealt or damage-taken triggers; kill, death and health-below still fire.
- Tests:
  - Exact tick counts (5 × 20 over 16 s; 14 ticks with uneven `dt`).
  - First tick lands after one interval.
  - Dispel stops the ticks.
  - Two burns from different owners coexist.
  - Unittests for the generator rules.
- Add an invalid example: a burn with `amount=0`.

### Step 4 — G11: bounded teleport + global cast range
- `AbilityEffect::Teleport` becomes `Teleport { min_distance: RankValue, max_distance: Option<RankValue> }`.
  - Update every match: `effects.rs`, `tooltip.rs`, the item path and tests.
  - The existing `blink` and `blink_dagger` regenerate with bounds `ZERO` / `None`, so their behaviour is unchanged. This is the step's only allowed generated diff.
- `EffectRunner` computes the destination:
  - `v = flat(aim - origin)` and `d = |v|.clamp(min, max)`.
  - If `|v|` ≈ 0 and min > 0, the direction falls back to the caster's facing.
  - With no bounds the destination is exactly `aim`.
  - `teleport.rs` is unchanged: it still validates the map, terrain, occupancy, leash and immunity.
- `scale.rs` gets `GLOBAL_CAST_RANGE = 25_000.0`, which is larger than the 21,496 map diagonal.
  - `parse_activation_spec` accepts `global`, case-insensitive and without a level list, for point, unit and direction abilities.
  - The cast indicator hides the range ring when the range is ≥ `GLOBAL_CAST_RANGE`.
- Generator teleport branch:
  - `value` (max) is optional and > 0.
  - `amount` (min) is optional, ≥ 0 and ≤ max.
  - Bounds are allowed only on the caster/owner target.
- Items get the same behaviour; Blink Dagger keeps blank bounds.
- Tests:
  - A far click (3000) clamps to 1075.
  - A near click (50) is pushed out to 200.
  - A click on herself uses her facing.
  - Unbounded teleport is unchanged.
  - Global range never makes the hero walk into range.
  - Unittests for `global` and for min > max.
- Add an invalid example: a teleport with `amount > value`.

### Step 5 — G14: expanding skillshot width
- `AbilityMechanic::Projectile` gains `end_radius: Option<RankValue>`.
  - Update the `catalog.rs` `PROJECTILE` const, the test constructors, the tooltip and both generator templates.
  - Move the template into a shared `generator_common.projectile_mechanic_rust(...)`.
- `combat.rs`:
  - `SpawnProjectileEvent` gains `skillshot_end_width`.
  - `Skillshot` gains `start`, `range` and `end_width`.
  - Contact width is `lerp(width, end_width, clamp(dist(start, pos) / range, 0, 1))`.
  - `mechanics.rs` fills these from `aoe_radius`, `end_radius` and `cast_range`.
- Generator: the optional `projectile_end_radius` column takes levels. It applies only to `skillshot` and `skillshot_single`, must be > 0, and requires `aoe_radius`.
- Existing rows get `end_radius: None` (an allowed generated diff), so their width stays constant.
- Tests:
  - An expanding wave hits an off-path unit near the end of its travel but not near the start.
  - A constant-width skillshot is unchanged.
- Add an invalid example: an end radius on a targeted ability.

### Step 6 — G13: one homing projectile per unit in an area
- Add `AbilityMechanic::ProjectileToEach { center: AreaCenter, team: TargetTeam, speed: Option<f32> }`.
- `MechanicRunner`:
  - Selects living, targetable units of the allowed team inside the area, excluding the caster.
  - Spawns one homing `SpawnProjectileEvent` per unit, with splash 0, the same payload and `cast_target` set to that unit.
  - Each hit fires `OnProjectileHit` with `trigger_unit` set to the struck unit.
- Extract the `UnitsInRadius` predicate into a shared `pub(crate) fn unit_in_area(...)` used by both the effect and the mechanic runner.
- Source attribution comes from `ctx.source`. Items get this through `ItemDefinition::mechanics`.
- Generator, `projectile_type=targeted_each`:
  - Requires a `no_target` ability (centred on the caster) or a `point` ability (centred on the aim), with a non-zero `aoe_radius`.
  - The team comes from `target_team`.
  - `on_projectile_hit` with `affected_units` is invalid.
  - Emits `shockwave_ring()` FX for `no_target`.
  - ItemGenerator rejects it.
- Tooltip: "Projectile at each enemy in radius".
- Tests:
  - One projectile per valid enemy: 3 inside, 1 outside, 1 ally and 1 cycloned unit gives 3.
  - `OnProjectileHit` fires once per unit.
  - Unittest: missing `aoe_radius`.
- Add an invalid example: `targeted_each` without `aoe_radius`.

### Step 7 — G16: Sonic Wave's 14 instances
**Recommended design (pending user confirmation, §12.1): the interval mechanism.**
- Sonic Wave is one expanding skillshot (G14) moving at 900 / 1.4 s ≈ 642.857 u/s, and it hits each enemy once.
- On hit it applies:
  - a per-owner pure burn: 23/34/45/56 per tick, interval 0.1 s, 1.4 s duration, which is exactly 14 instances;
  - one knockback of 350 over 1.4 s, away from the contact point.
- No code beyond Steps 3 and 5.

Alternative, the pulse model: 14 timed area pulses at increasing distance and radius. It would need:
- a projectile re-hit interval with per-pulse hit lists;
- a once-per-unit knockback rule;
- and a unit's damage would depend on how many pulses it stood in (1–14).

### Step 8 — Queen of Pain data
1. Add the rows in §4–§6.
2. Run HeroGenerator **first**, on a scratch CSV holding the header and the Queen of Pain row. Use `--dry-run` to confirm `ShadowStrike/QueenOfPainBlink/ScreamOfPain/SonicWave`, then run it for real.
3. Run AbilityGenerator: expect 4 additions with ranks 7/7/7/4.
4. Run `cargo fmt`.
5. Edit the README hero row: `| Queen of Pain | Intelligence | Shadow Strike, QoP Blink, Scream of Pain, Sonic Wave |`.

### Step 9 — Docs
- `docs/architecture/composition.md`:
  - Mechanics: add `ProjectileToEach`.
  - Effects: teleport bounds.
  - Rules: ticking burns.
  - Add §6.11 "Queen of Pain" compositions.
- `README.md`: generator column docs for each §2 entry, and the hero columns.
- This plan: append implementation notes and any deviations.

## 4. `data/heroes.csv` and `data/heroes.example.csv`

Header:
```csv
Hero_name,is_melee,Ability_Q_Name,Ability_W_Name,Ability_E_Name,Ability_R_Name,base_STR,base_AGI,base_INT,STR_per_level,AGI_per_level,INT_per_level,primary,blurb,base_health,base_mana,attack_damage,attack_range,move_speed,armor,magic_resist,base_health_regen,base_mana_regen,base_attack_time,attack_point,attack_backswing,attack_projectile_speed
```

`data/heroes.csv`:
```csv
Queen of Pain,FALSE,Shadow Strike,Queen of Pain Blink,Scream of Pain,Sonic Wave,20,20,25,2.4,2.5,3.4,INT,"Int assassin — Shadow Strike, QoP Blink, Scream of Pain, Sonic Wave",200,75,54,550,290,-2.8,3.321839,-0.15,0,1.5,0.56,0.41,1500
```

`data/heroes.example.csv` is the same row with `false` in place of `FALSE`.

## 5. `data/abilities.csv` and `data/examples/abilities.csv`

Append `,projectile_end_radius` to the header.
```csv
shadow_strike,Shadow Strike,Q,active,unit,enemy,450|500|550|600|650|700|750,,0.3,,11|9|7|5|3|3|3,100|105|110|115|120|125|130,,,targeted,,"Hurls a dagger that deals magical damage, then slows the target and deals magical damage every 3s for 16s",0.77,,,
queen_of_pain_blink,QoP Blink,W,active,point,enemy,global,,0.33,,12|10|8|6|4|2|1,65,,,none,,"Teleports toward the target point, at least 200 and at most the maximum blink distance away",0.07,,,
scream_of_pain,Scream of Pain,E,active,no_target,enemy,,600,0,,7.5|7|6.5|6|5.5|5|4.5,120,,,targeted_each,900,"Sends a projectile at every nearby enemy, each dealing magical damage",0.63,,,
sonic_wave,Sonic Wave,R,active,point,enemy,900,100,0.452,,110|95|80|65,250|400|550|700,,,skillshot,642.857143,"Sends out a widening wave; each enemy struck takes pure damage 14 times over 1.4s and is knocked back",0.9,,,450
```
- W uses `target_team enemy`, the same as the existing `blink` row; point targeting with team `self` is rejected.
- Scream of Pain's projectile speed is 900 (user decision).

## 6. `data/ability_effects.csv` and `data/examples/ability_effects.csv`
```csv
shadow_strike,0,on_projectile_hit,trigger_unit,damage,,50|80|110|140|170|200|230,magical,,,,,
shadow_strike,1,on_projectile_hit,trigger_unit,apply_status,slow,,,16,,25|35|45|55|65|75|85,,
shadow_strike,2,on_projectile_hit,trigger_unit,apply_status,burn,20|40|60|80|100|120|140,magical,16,,3,,
queen_of_pain_blink,0,on_cast,caster,movement,teleport,1075|1150|1225|1300|1375|1450|1525,,,,200,,
scream_of_pain,0,on_projectile_hit,trigger_unit,damage,,90|175|260|345|430|515|600,magical,,,,,
sonic_wave,0,on_projectile_hit,trigger_unit,apply_status,burn,23|34|45|56,pure,1.4,,0.1,,
sonic_wave,1,on_projectile_hit,trigger_unit,movement,knockback,350,,1.4,,,,
```

## 7. Invalid examples (`data/examples/invalid/`)

Add one row per new rule:
- a burn with `amount=0`;
- a teleport with `amount > value`;
- `projectile_end_radius` on a targeted ability;
- `targeted_each` without `aoe_radius`.

The invalid examples must still be rejected.

## 8. Naming
- HeroGenerator names variants with `to_pascal_case(display name)`. AbilityGenerator names them with the PascalCase of the `id`.
- In the hero CSV, `Ability_W_Name = Queen of Pain Blink` gives `QueenOfPainBlink`, which matches the id `queen_of_pain_blink`.
- The ability CSV's `name = QoP Blink` then becomes the display name. "QoP Blink" in the hero CSV would give `QoPBlink`, which no sane id maps to.
- Skirmisher's `AbilityId::Blink` is untouched.

## 9. Variant B values

| Field | Formula | Value |
|---|---|---|
| base_health | 560 − 20×18 | 200 |
| base_health_regen | 2.25 − 20×0.12 | −0.15 |
| base_mana | 375 − 25×12 | 75 |
| base_mana_regen | 1.25 − 25×0.05 | 0 |
| armor | 0 − 20×0.14 | −2.8 |
| magic_resist | (0.275/0.725)/0.06 − 25×0.12 | 3.321839 |

Negative bases are accepted by the generator (`parse_float` has no minimum) and by the runtime (`mitigate` handles negative resistance with `abs()`).

## 10. Tests

- `heroes.rs` `queen_of_pain_matches_spec` checks:
  - the CLI lookup, the primary attribute and the abilities;
  - level-1 vitals: 560, 2.25, 375, 1.25, armor ≈ 0, MR ≈ 27.5%;
  - range 550, BAT 1.5, attack point 0.56, backswing 0.41, turn rate 0.6, move speed 290, projectile speed 1500, APS 0.8.
- `catalog.rs` `queen_of_pain_abilities_generate_expected_definitions` checks the cooldowns, burn intervals, global range, teleport bounds, `ProjectileToEach`, end radius, ultimate rank count and the "QoP Blink" display name.
- Integration tests:
  - `sonic_wave_deals_fourteen_instances_and_one_knockback`;
  - `shadow_strike_and_sonic_wave_dots_coexist`.

Commands:
```bash
cargo fmt --check
cargo build
cargo test
python3 scripts/AbilityGenerator.py --check
python3 scripts/ItemGenerator.py --check
python3 scripts/AbilityGenerator.py data/examples/abilities.csv data/examples/ability_effects.csv --check
python3 scripts/ItemGenerator.py data/examples/items.csv data/examples/item_effects.csv --check
(cd scripts && python3 -m unittest test_generator_common)
python3 scripts/AbilityGenerator.py && python3 scripts/ItemGenerator.py && git diff --stat src/abilities/generated.rs src/item_catalog.rs
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
```

## 11. Invariants and risks
- Teleport clamping lives in `EffectRunner`.
- `ProjectileToEach` only emits projectiles, and its source comes from `ctx.source`.
- Ticks are written only by `tick_status_effects`.
- Generated files change only through the generators, with the diffs named in Steps 4–6.
- Ticks are written before `apply_damage_events` in the same frame, and the final-tick flush happens on expiry.
- A bounded teleport still drops stale move orders through `teleport.rs`.
- A rejected teleport destination still spends mana and cooldown, as the existing Blink does.
- A global cast range must hide the range ring.
- Generated burns now use per-owner ids, so `remove_status,burn` no longer matches them. No current data is affected.
- The hero-select UI may overflow with 6 heroes (cosmetic).

## 12. User confirmations (2026-11-08)
1. Sonic Wave: interval design (Step 7).
2. Shadow Strike: first tick 3 s after impact, 5 ticks.
3. Blink clicked on self: moves 200 in facing direction.
4. Scream of Pain projectile speed: 900.
5. Generated burns use per-owner status ids.

## 13. Implementation notes (2026-11-08)

Steps 1 to 9 landed as separate commits on the PR branch (Step 7 is an empty commit: the interval design needs no code beyond Steps 3 and 5). No mechanic, trigger, effect, status, CSV column or keyword beyond §2/§3 was added. Deviations and clarifications:

1. **Skillshot range.** `Skillshot.start` and `range` are derived in `combat::spawn_spell_bolt` from the event's `origin` and `target_pos` (for a skillshot, `target_pos = origin + dir * cast_range`, set in `mechanics.rs`), so `SpawnProjectileEvent` only gains `skillshot_end_width`. This is the cast range in every case, without a second field that could disagree with it.
2. **`unit_in_area` signature.** It takes `untargetable: bool` instead of a status query. `EffectTarget::UnitsInRadius` waives the check for the caster, and `ProjectileToEach` excludes the caster altogether, so each caller computes the flag. Both runners use the same predicate otherwise.
3. **Items and `projectile_end_radius`.** The shared validator requires `aoe_radius` for it, and items have no `aoe_radius` column, so items reject the column with a clear message (the same position as `targeted_each`). The column is accepted in the optional item activation list so that the error, not an "unknown column" warning, is shown.
4. **Validation beyond the listed rules** (none adds surface; each only rejects a combination that would otherwise do nothing or lie):
   - `cast_range=global` mixed into a level list is an error.
   - A teleport with bounds on a target other than `caster` / `owner` is an error (the bounds are measured from the caster).
   - `targeted_each` with `target_team=self` is an error (the caster is never a target, so nothing would be sent).
   - Burn `amount` must also be above 0 (the spec's "> 0 and <= duration" rule), checked per rank; the invalid example uses `amount=0`.
5. **Tooltips.** Ticking burns show "Burn damage per Ns"; continuous ones show "Burn damage per second" (previously only "Burn" and the duration). Skillshots with an end radius add an "End radius" row. Bounded teleports add a "Max distance" row and a "at least N away" line. A global cast range still prints its numeric value (25000) in the "Cast range" row.
6. **Generator output formatting.** `AbilityGenerator.py` and `HeroGenerator.py` rewrite the `ability_ultimates` region in `components.rs` unformatted; run `cargo fmt` after every generator run (as Step 8 already prescribed). The only hand-formatted diffs left in generated files are the ones named in Steps 4 to 6 and 8.
7. **Ticking burn tolerance.** A tick is due when `dot_timer + 1e-4 >= dot_interval`, matching the `1e-4` slack used for `floor(duration / interval + 1e-4)`. With 0.05 s frames the 16 s / 3 s burn ticks exactly at 3, 6, 9, 12 and 15 s and the 1.4 s / 0.1 s burn deals 14 ticks at any frame rate (the test uses uneven frames and a single long frame).
8. **Test harness.** The Sonic Wave and Shadow Strike integration tests use `qop_app()`: the ability `test_app()` plus `tick_status_effects` ordered before `status_triggers`. They land the projectile with an explicit `ProjectileHitEvent` (the harness has no projectile entities); the skillshot's width growth is covered separately in `combat.rs` (`expanding_skillshot_widens_with_distance_travelled`).
9. **Heroes CSV.** `data/heroes.csv` also got the Queen of Pain row, next to the still-pending Magnus row. HeroGenerator ran only on a scratch CSV (header plus the Queen of Pain row, kept outside the repo), so Magnus was not generated.

Remaining limitations: the hero-select UI may overflow with six heroes (cosmetic, §11); the global-range tooltip shows 25000.
