# Plan: Queen of Pain hero (existing features only)

Status: **draft — awaiting user decisions (section 5)**. INT confirmed by user: base 25, gain 3.4.

## 0. Summary

The kit can be built from CSV rows alone: a hero row, 4 ability rows and 7 effect rows, with no new Rust or generator code. Several requested behaviours can only be approximated with existing features; section 5 lists every gap together with the existing-only approximation. Two items block validation:

- **Q rank-7 cooldown extrapolates to -1.** The generator rejects negative cooldowns.
- **Sonic Wave has no cooldown in the spec.**

Background: `docs/architecture/composition.md` and the plans `2026-11-08-mechanic-composition-layers`, `2026-11-07-movement-mechanics`, `2026-11-06-status-effects-catalog`, `2026-11-06-activation-targeting-mechanics`, `2026-11-06-damage-mechanics` and `2026-11-06-generator-shared-architecture`.

## 1. Project facts that drive the design

| Topic | Finding | Source |
|---|---|---|
| Rank counts | Basic abilities have 7 ranks and ultimates 4. Ultimates unlock at hero level 6/12/18/24 | `src/components.rs` `max_rank`; `src/progression.rs`; `scripts/AbilityGenerator.py` |
| Hero generator | Writes `src/heroes.rs` (HeroId, `def`), `components.rs` AbilityId stubs and the README table. No `--check` and no rustfmt | `scripts/HeroGenerator.py` |
| Hero ↔ ability link | HeroGenerator names each AbilityId as the PascalCase of the ability's display name. If that name is taken it adds the hero name as a prefix (Blink becomes `QueenOfPainBlink`). **Run HeroGenerator first**, then AbilityGenerator with ids `shadow_strike`, `queen_of_pain_blink`, `scream_of_pain`, `sonic_wave` | `HeroGenerator.unique_ability_id` |
| Blink collision | `AbilityId::Blink` and the display name "Blink" already exist (Skirmisher). Display names must be unique within a CSV | `AbilityGenerator.load` |
| Cast point / backswing | Both silently clamped to 0..0.5 | `src/abilities/definition.rs` `timing` |
| Attack point / backswing | Both clamped to 0..0.5 | `src/heroes.rs` `hero_combat` |
| Magic resist | Stored as resistance points: `factor = 1 - 0.06R/(1+0.06R)`. 27.5% = 6.3218 R. Each point of INT adds 0.12 R | `src/combat.rs` |
| Attributes | +18 HP/STR, +0.12 HP regen/STR, +0.14 armor/AGI, +12 mana/INT, +0.05 mana regen/INT, +0.12 MR/INT. APS = (100+AGI)/100/BAT | `components.rs` `HeroAttributes::apply_to` |
| Attack projectile speed | Derived from range: `clamp(500+1.2*range, 700, 1500)`, which gives 1160 at range 550 | `src/basic_attack.rs` |
| Turn rate | Hard-coded 0.6, which matches the spec | `src/facing.rs` |
| Burn (DoT) | Continuous `dps*dt`; no tick interval. The status id `"burn"` is shared | `src/status.rs` |
| Skillshot | Travels exactly `cast_range`, with constant width `aoe_radius`. Hits each enemy once | `abilities/mechanics.rs`, `combat.rs` |
| Knockback | Pushes away from the trigger point, which is the projectile's position at contact | `src/displacement.rs` |
| Point targeting | Clicking beyond `cast_range` makes the hero walk into range | `actions/targeting.rs` |
| Pending Magnus row | `data/heroes.csv` contains a Magnus row that has not been generated yet | `data/heroes.csv` |

## 2. Hero stats: column coverage

| Stat | Value | Column | Status |
|---|---|---|---|
| HP / mana | 560 / 375 | `base_health` / `base_mana` | Column exists |
| HP / mana regen | 2.25 / 1.25 | `base_health_regen` / `base_mana_regen` | The generator supports both, but they are missing from the CSV header (C1) |
| Armor | 0 | `armor` | Column exists |
| Magic resist | 27.5% | `magic_resist` | Column exists; value converted to 6.32 R |
| Attack damage / range | 54 / 550 ranged | `attack_damage` / `attack_range` / `is_melee=FALSE` | Columns exist |
| BAT | 1.5 | `base_attack_time` | Generator-supported; header column missing (C1) |
| Attack point / backswing | 0.56 / 0.41 | `attack_point` / `attack_backswing` | Generator-supported; header columns missing (C1). 0.56 is clamped to 0.5 (G5) |
| Attack projectile speed | 1500 | none | **Gap G4** (derived speed would be 1160) |
| Move speed | 290 | `move_speed` | Column exists |
| Turn rate | 0.6 | constant | Matches the hard-coded value |
| STR / AGI / INT | 20+2.4 / 20+2.5 / 25+3.4 | `base_*`, `*_per_level` | Columns exist |
| Primary attribute | not given | `primary` | Proposed: `INT` (G2) |

Variant A enters the values literally as bases, so the attribute bonuses stack on top. Variant B back-solves the bases so the level-1 totals equal the spec (G3):

| Field | Variant A (literal) | Level-1 result under A | Variant B (back-solved) |
|---|---|---|---|
| base_health | 560 | 920 | 200 |
| base_health_regen | 2.25 | 4.65 | -0.15 |
| base_mana | 375 | 675 | 75 |
| base_mana_regen | 1.25 | 2.5 | 0 |
| armor | 0 | 2.8 | -2.8 |
| magic_resist | 6.32 | ≈35.9% | 3.32 |

## 3. Extrapolated values (arithmetic progression, no clamping)

| Ability | Field | Values by rank |
|---|---|---|
| Q | cast range | 450, 500, 550, 600, 650, 700, 750 |
| Q | cooldown | 11, 9, 7, 5, 3, 1, **-1** (G10) |
| Q | mana | 100, 105, 110, 115, 120, 125, 130 |
| Q | hit damage | 50, 80, 110, 140, 170, 200, 230 |
| Q | slow % | 25, 35, 45, 55, 65, 75, 85 |
| Q | DoT per tick | 20, 40, 60, 80, 100, 120, 140 |
| W | max distance | 1075, 1150, 1225, 1300, 1375, 1450, 1525 |
| W | cooldown | 12, 10, 8, 6, 4, 2, **0** (G10) |
| W | mana | 65 at every rank |
| E | damage | 90, 175, 260, 345, 430, 515, 600 |
| E | cooldown | 7.5, 7, 6.5, 6, 5.5, 5, 4.5 |
| E | mana | 120 at every rank |
| R | mana | 250, 400, 550, 700 |
| R | damage per instance | 23, 34, 45, 56 (×14 = 322, 476, 630, 784) |
| R | cooldown | **not given** (G17) |

## 4. Proposed CSV rows (existing features only)

### 4.1 `data/heroes.csv`
Append `base_health_regen,base_mana_regen,base_attack_time,attack_point,attack_backswing` to the header (C1). The row below uses Variant A:
```csv
Queen of Pain,FALSE,Shadow Strike,Blink,Scream of Pain,Sonic Wave,20,20,25,2.4,2.5,3.4,INT,"Int assassin — Shadow Strike, Blink, Scream of Pain, Sonic Wave",560,375,54,550,290,0,6.32,2.25,1.25,1.5,0.56,0.41
```

### 4.2 `data/abilities.csv`
```csv
shadow_strike,Shadow Strike,Q,active,unit,enemy,450|500|550|600|650|700|750,,0.3,,11|9|7|5|3|1|-1,100|105|110|115|120|125|130,,,targeted,,"Hurls a dagger that deals magical damage, then slows the target and deals magical damage over time for 16s",0.77,,
queen_of_pain_blink,<NAME G12>,W,active,point,enemy,1075|1150|1225|1300|1375|1450|1525,,0.33,,12|10|8|6|4|2|0,65,,,none,,Teleport to the target point,0.07,,
scream_of_pain,Scream of Pain,E,active,no_target,enemy,,600,0,,7.5|7|6.5|6|5.5|5|4.5,120,,,none,,Deals magical damage to all nearby enemies,0.63,,
sonic_wave,Sonic Wave,R,active,point,enemy,900,275,0.452,,<COOLDOWN G17>,250|400|550|700,,,skillshot,<SPEED G17>,"Sends out a wave that deals pure damage and knocks enemies back",0.9,,
```
- Blink uses `target_team enemy` because point + self fails validation, matching the existing `blink` row.
- Shadow Strike's projectile speed is left blank, so the default spell speed (~884) applies (G9).

### 4.3 `data/ability_effects.csv`
```csv
shadow_strike,0,on_projectile_hit,trigger_unit,damage,,50|80|110|140|170|200|230,magical,,,,,
shadow_strike,1,on_projectile_hit,trigger_unit,apply_status,slow,,,16,,25|35|45|55|65|75|85,,
shadow_strike,2,on_projectile_hit,trigger_unit,apply_status,burn,<DPS G8>,magical,16,,,,
queen_of_pain_blink,0,on_cast,caster,movement,teleport,,,,,,,
scream_of_pain,0,on_cast,units_in_radius,damage,,90|175|260|345|430|515|600,magical,,,,,
sonic_wave,0,on_projectile_hit,trigger_unit,damage,,322|476|630|784,pure,,,,,
sonic_wave,1,on_projectile_hit,trigger_unit,movement,knockback,350,,1.4,,,,
```

### 4.4 Example CSVs
- Add the same ability and effect rows to `data/examples/abilities.csv` and `data/examples/ability_effects.csv`. `test_valid_examples_pass` validates these files.
- There is no hero example in `data/examples/`. Recommendation: add the hero row to `data/heroes.example.csv`, with the same header extension.

## 5. Gaps and ambiguities (user decisions)

Data:
- **G1 (resolved).** INT base 25, gain 3.4.
- **G2.** Primary attribute: proposed `INT`.
- **G3.** Variant A or B (section 2).
- **G9.** Shadow Strike projectile speed: proposed default.
- **G10.** Q rank 7 cooldown is -1 and W rank 7 cooldown is 0. Need rank-7 values or acceptance.
- **G12.** Blink display name must differ from "Blink".
- **G17.** Sonic Wave cooldown, projectile speed and damage interval.
- **G20.** Exclude the pending Magnus row from generation (scratch CSV) or generate it too.

Features (a = minimal new feature; b = existing-only approximation):

| # | Behaviour | (a) | (b) |
|---|---|---|---|
| C1 | Regen / BAT / attack timing columns | Add 5 generator-supported header columns (no code) | Defaults apply |
| G4 | Attack projectile speed 1500 | New hero column `attack_projectile_speed` | Derived 1160 |
| G5 | Attack point 0.56 | Raise the 0.5 clamp | 0.5 |
| G6 | Cast backswing 0.77 / 0.63 / 0.9 | Raise the 0.5 clamp | 0.5 |
| G8 | DoT ticks every 3 s | Tick interval on Burn | Continuous burn at per-tick/3 dps, or 5 ticks spread over 16 s (6.25/12.5/…/43.75) |
| G11 | Global range, max distance, 200 minimum | Max/min distance on Teleport | `cast_range` = max distance and the hero walks in; no minimum |
| G13 | One projectile per enemy | Per-unit homing projectile | Instant AoE damage |
| G14 | Wave width 100→450 | Skillshot `end_width` | Constant width 275 |
| G15 | Cast range 700 + travel 900 | Separate travel distance | `cast_range` 900 |
| G16 | 14 damage instances | Interval damage (shared with G8) | One hit for the full total |
| G18 | Knockback along the wave direction | Directional knockback option | Push away from the contact point |
| G21 | README kit text | none | Hand-edit after generation |

## 6. Implementation sequence
1. Check the baseline: `python3 scripts/AbilityGenerator.py` should leave no diff.
2. Edit the CSVs, including the examples.
3. Run `AbilityGenerator.py --check` on both the data CSVs and the example CSVs.
4. Run HeroGenerator **first** (with `--dry-run`, then for real). Use a scratch CSV if Magnus is excluded.
5. Run AbilityGenerator. Expect 4 additions with ranks 7/7/7/4.
6. Run `cargo fmt`, then hand-fix the README row.

## 7. Invariants
- Generated files change only through the generators, plus `cargo fmt`.
- No new effect, trigger, status, mechanic or CSV column unless the user approves it.
- `SonicWave.is_ultimate()` holds.
- Skirmisher's Blink is untouched.

## 8. Tests
- `src/heroes.rs`: `queen_of_pain_def_matches_csv` covers abilities, range, BAT, attack timing and level-1 vitals.
- `src/abilities/catalog.rs` covers the target type, effects, rank values and the `max_rank` of each of the four abilities.
- Commands: `cargo fmt --check`, `cargo build`, `cargo test`, the generator `--check` runs (data and examples), `python3 -m unittest test_generator_common`, and a regenerate step that should produce no diff.

## 9. Risks
- Generator order: running AbilityGenerator first creates duplicate stubs.
- Shared `slow` and `burn` status ids: other sources replace Shadow Strike's debuffs.
- A failed Blink teleport still spends mana and cooldown, as the existing Blink does.
- The hero-select row may overflow on narrow windows.
- Magnus's `magic_resist 27` is probably a percent-vs-points mistake (out of scope).
