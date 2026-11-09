# Magnus hero: implementation spec

Status: **approved by user 2026-11-08 (see §12).** §12 overrides the fallbacks in §3 to §8 wherever a gap was approved.

Related plans: `docs/plans/2026-11-08-queen-of-pain-hero.md` (the model for this spec, and the source of `cast_range=global`, bounded teleport, the swing caps and the scratch-CSV HeroGenerator flow), `docs/architecture/composition.md`, `2026-11-08-attack-modifiers.md` (cleave), `2026-11-07-movement-mechanics.md` (dash, drag, pull, `pull_point`), `2026-11-06-status-effects-catalog.md`, `2026-11-06-activation-targeting-mechanics.md`.

## 1. Current state (verified)

- **Stale hero row.** `data/heroes.csv` line 5 has `Magnus,TRUE,Shockwave,Empower,Skewer,Reverse Polarity,25,9,19,3.1,2,1.6,STR,,626,315,60,200,305,3.33,27`. The values are wrong and the row is too short. It is not in `data/heroes.example.csv`, and Magnus is not in `src/heroes.rs` or `src/components.rs`.
- **Older sample rows that clash.** `data/examples/abilities.csv` and `data/examples/ability_effects.csv` already contain the samples `shockwave` (name "Shockwave") and `skewer` (name "Skewer"). README §"Adding abilities" quotes them.
- **Id collision.** `AbilityId::Shockwave` is Vanguard's hand-written W (`components.rs:571`, `catalog.rs:179-199`).
  - Generated definitions take precedence over hand-written ones with the same `AbilityId` (`AbilityGenerator.py` docstring). The existing `blink` row already overrides Skirmisher's Blink this way.
  - **So the CSV id `shockwave` would silently replace Vanguard's W.** Magnus' Q must use another id (see §8).
- `Empower`, `Skewer` and `ReversePolarity` do not exist yet. `on_skewer_end` appears only in a test that uses `AbilityId::Bulwark` (`abilities/mod.rs:3270`); it is unrelated.

## 2. Scope and level extrapolation

Values are extrapolated arithmetically to basics 7 / ultimates 4. Nothing is clamped. Constants stay constant. **No value reaches 0 or goes negative.** The lowest cooldowns are Q 7 and E 10, so the `ActionCooldown` / `RankValue` floors never apply.

| Ability | Field | Ranks |
|---|---|---|
| Q | cooldown | 13, 12, 11, 10, 9, 8, 7 |
| Q | mana | 85, 90, 95, 100, 105, 110, 115 |
| Q | damage (magical) | 75, 150, 225, 300, 375, 450, 525 |
| Q | slow duration (75% constant) | 0.55, 0.7, 0.85, 1, 1.15, 1.3, 1.45 |
| Q | constants | cast range 1200, wave radius 200, speed 1200, pull 150 over 0.2 s |
| W | mana | 45, 55, 65, 75, 85, 95, 105 |
| W | bonus attack damage % | 16, 24, 32, 40, 48, 56, 64 |
| W | cleave % | 16, 24, 32, 40, 48, 56, 64 |
| W | duration | 30, 33, 36, 39, 42, 45, 48 |
| W | constants | cooldown 8, range 800, cleave 150 / 360 / 650 |
| E | max distance | 800, 900, 1000, 1100, 1200, 1300, 1400 |
| E | dash time (distance / 900) | 0.888889, 1, 1.111111, 1.222222, 1.333333, 1.444444, 1.555556 |
| E | cooldown | 22, 20, 18, 16, 14, 12, 10 |
| E | damage (magical) | 80, 160, 240, 320, 400, 480, 560 |
| E | slow % | 20, 30, 40, 50, 60, 70, 80 |
| E | constants | mana 80, radius 145, speed 900, cast range global |
| R | mana | 150, 225, 300, 375 |
| R | damage (magical) | 100, 200, 300, 400 |
| R | stun | 2.5, 3, 3.5, 4 |
| R | constants | cooldown 115, radius 430 |

## 3. How each part maps onto existing features

| Spec item | Existing surface | Fidelity |
|---|---|---|
| Hero vitals | Variant B back-solved bases (§7); `HeroAttributes::apply_to` (`components.rs:367`) | Exact |
| STR / AGI / INT, primary STR | Hero CSV columns | Exact |
| Attack damage 61, BAT 1.8, point 0.5, backswing 0.84, move speed 305 | Hero CSV columns. The swings fit `MAX_ATTACK_SWING` 1.0 | Exact |
| Attack speed 100 | `hero_combat` hard-codes `base_attack_speed: 100.0`. APS = (100 + 14) / 100 / 1.8 = 0.633333 | Exact |
| **Instant melee attack at range 200** | `CombatStats::is_melee()` uses `scale::is_melee_attack_range`, which is `range <= 150.5` (`scale.rs:131`). Range 200 makes him **ranged** (projectile, no cleave). The hero CSV `is_melee` column only picks defaults | **Gap G1.** Fallback: `attack_range` 150 |
| Turn rate 0.8 | HeroGenerator always emits `crate::facing::HERO_TURN_RATE` (0.6) (`HeroGenerator.py:356`). There is no column | **Gap G2.** Fallback: 0.6 |
| Cast backswings 1.2 / 1.07 / 1.3 | `MAX_CAST_BACKSWING = 1.0` (`definition.rs:13`). The generator warns and the runtime clamps | **Gap G3.** Fallback: clamp to 1.0 |
| Q: point **or unit** target | `TargetType::Point` already takes a hovered unit as the aim (`targeting.rs:18,163-170`). `refresh_activation_aims` follows the unit during the cast point. Skillshots ignore the unit and fly `cast_range` toward the aim (`mechanics.rs:206-217`) | Exact for enemy units. Allies cannot be clicked as an aim (`target_team enemy`) |
| Q: wave, each enemy hit once | `projectile_type=skillshot`, `aoe_radius` 200 = contact radius (`combat.rs` `Skillshot`, `area_contains`). The `hit` list ensures one hit per unit | Continuous sweep in place of 25 discrete radii. Same coverage, except for the 48-unit start offset (G10) |
| Q: pull toward the **centre of the wave** | `movement,pull_point` → `DisplacementKind::PullToPoint` moves toward `event.point`. For `on_projectile_hit`, `ctx.point = hit.impact` (`mechanics.rs:451`), which is the projectile's position, i.e. the centre of the wave when it touched the unit. Distance is capped at min(150, distance to centre) (`displacement.rs:329-336`) | Exact in direction and distance. A unit closer than 150 to the centre stops at the centre |
| Q: 75% slow | `apply_status,slow` with `amount` 75 and a duration list | Exact. Note: all slows share the `"slow"` id, so the last one applied wins (existing rule) |
| W: unit target, ally or self | `target_type unit`, `target_team ally`. `team_allows` lets the caster's own team through, so self is valid (`targeting.rs:70-80`) | Exact |
| W: bonus attack damage % | Timed `stat_modifier,attack_damage` is **flat only**; `stat_modifier` rejects scaling (`generator_common.py:1715`) | **Gap G4.** Fallback: flat percentage of Magnus' base 61 |
| W: cleave % | `PassiveStat::CleaveRatio` is permanent only. `TIMED_STATS` and `StatusSpec::Modifier` have no cleave field | **Gap G5.** Fallback: no cleave |
| W: cleave cone 150 / 360 / 650 | Runtime cleave is a circle around the attacker (radius `cleave_radius`, default `ATTACK_CLEAVE_RADIUS` 300; `basic_attack.rs:608-622`) | **Gap G6** |
| W: cleave only when the target is melee | `stats.is_melee() && cleave_damage_ratio > 0` (`basic_attack.rs:608`) | Exact once G5 exists |
| E: global cast range, dash to the point | `cast_range=global` (QoP G11) plus `on_cast,caster,movement,dash`. `value` = max distance and `duration` = time for the full distance, so speed is 900. A near click dashes shorter (`displacement.rs:337-348`) | Exact |
| E: enemies hit are dragged | `on_unit_contact,trigger_unit,movement,drag`. `MotionPath::Attached` lasts as long as the caster's own motion (`displacement.rs:367-391`) | Exact |
| E: contact radius 145 | Contact is `center_distance <= own_bound + other.bound` (heroes 36 + 36 = 72; `displacement.rs:650-666`) | **Gap G7.** Fallback: body contact |
| E: stunned and disarmed while dragged | `on_unit_contact` stun for the full dash time, then `on_movement_end,affected_units,remove_status,stun`. A stun already disarms (`status.rs:522-526`), so a separate disarm row adds nothing | Approximation. See G9 and U9 |
| E: damage and slow at the destination | `on_movement_end,affected_units`, where `affected_units` = every unit contacted (`displacement.rs:684-689`) | Exact. The slow duration is missing from the spec (U1) |
| R: no target, radius 430, damage + stun | `no_target`, `aoe_radius` 430 (shockwave ring FX), `on_cast,units_in_radius` | Exact |
| **R: teleport enemies in front of Magnus** | `movement,teleport` is limited to single targets (`TELEPORT_TARGETS`, `generator_common.py:268`). It sends every target to one aim point, and `teleport.rs` rejects stacked arrivals with `TeleportError::Occupied` | **Gap G8.** Fallback: `movement,pull` |

## 4. Plan using existing features only, in steps

Each step must build and pass tests before the next one starts. Generated files change only through the generators (I8).

1. **Data rows.** Put §5 to §7 into `data/abilities.csv`, `data/ability_effects.csv`, `data/heroes.csv` (replace the stale row) and `data/heroes.example.csv`. For `data/examples/abilities.csv` and `data/examples/ability_effects.csv`, **replace** the old `shockwave` and `skewer` rows with Magnus' rows and append `empower` and `reverse_polarity` (decision U6). Appending instead would fail validation: the duplicate name "Shockwave" and the duplicate id `skewer` are both errors (`AbilityGenerator.load`).
2. **HeroGenerator first, on a scratch CSV.**
   - The scratch file holds the header and the Magnus row, outside the repo.
   - Run `--dry-run` and expect `abilities=MagnusShockwave/Empower/Skewer/ReversePolarity`, then run it for real.
   - Running it on `data/heroes.csv` would also add only Magnus today; the scratch file keeps the QoP discipline.
3. **AbilityGenerator.**
   - Expect 4 additions with ranks 7/7/7/4: `magnus_shockwave → AbilityId::MagnusShockwave`, `empower`, `skewer`, `reverse_polarity`.
   - Expect 3 `cast_backswing exceeds 1.0s` warnings (G3 fallback).
   - Confirm that `generated.rs` has **no** `AbilityId::Shockwave` arm.
4. **`cargo fmt`.** Both generators rewrite `ability_ultimates` unformatted (QoP note 6).
5. **README hero row.** Replace the generated stub row with `| Magnus | Strength | Shockwave, Empower, Skewer, Reverse Polarity |`.
6. **Docs.** Add `docs/architecture/composition.md` §6.12 "Magnus" (Shockwave = skillshot + `pull_point` toward the contact point; Skewer = dash + contact drag/stun + movement-end payload; RP = `units_in_radius` pull + stun). Append implementation notes to this plan. Leave the README generator column docs unchanged, since no surface is added.

**Invalid examples:** none are needed for this plan, because it adds no validation rule. Each gap lists its own invalid example.

## 5. `data/abilities.csv` and `data/examples/abilities.csv`

The header already ends in `cast_backswing,max_rank,toggle_mana_per_sec,projectile_end_radius`.

```csv
magnus_shockwave,Shockwave,Q,active,point,enemy,1200,200,0.3,,13|12|11|10|9|8|7,85|90|95|100|105|110|115,,,skillshot,1200,"Sends out a wave of force that damages enemies in a line, pulls each one toward the wave and slows it briefly",1.2,,,
empower,Empower,W,active,unit,ally,800,,0.3,,8,45|55|65|75|85|95|105,,,none,,"Grants an allied hero or Magnus bonus attack damage",1.07,,,
skewer,Skewer,E,active,point,enemy,global,,0.3,,22|20|18|16|14|12|10,80,,,none,,"Rushes forward, goring enemies and dragging them, stunned, to the destination, where they take magical damage and are slowed",0.3,,,
reverse_polarity,Reverse Polarity,R,active,no_target,enemy,,430,0.3,,115,150|225|300|375,,,none,,"Pulls nearby enemies to Magnus, dealing magical damage and stunning them",1.3,,,
```

- The backswings are written at their spec values. The generator warns, and the runtime clamps to 1.0 until G3 is approved; after that no data edit is needed.
- Skewer uses `target_team enemy`, as `queen_of_pain_blink` does; point targeting with team `self` is rejected.
- If G5 is approved, the Empower description becomes "...bonus attack damage; melee heroes also cleave".

## 6. `data/ability_effects.csv` and `data/examples/ability_effects.csv`

```csv
magnus_shockwave,0,on_projectile_hit,trigger_unit,damage,,75|150|225|300|375|450|525,magical,,,,,
magnus_shockwave,1,on_projectile_hit,trigger_unit,movement,pull_point,150,,0.2,,,,
magnus_shockwave,2,on_projectile_hit,trigger_unit,apply_status,slow,,,0.55|0.7|0.85|1|1.15|1.3|1.45,,75,,
empower,0,on_cast,cast_target,stat_modifier,attack_damage,9.76|14.64|19.52|24.4|29.28|34.16|39.04,,30|33|36|39|42|45|48,,,,
skewer,0,on_cast,caster,movement,dash,800|900|1000|1100|1200|1300|1400,,0.888889|1|1.111111|1.222222|1.333333|1.444444|1.555556,,,,
skewer,1,on_unit_contact,trigger_unit,movement,drag,,,,,,,
skewer,2,on_unit_contact,trigger_unit,apply_status,stun,,,0.888889|1|1.111111|1.222222|1.333333|1.444444|1.555556,,,,
skewer,3,on_movement_end,affected_units,remove_status,stun,,,,,,,
skewer,4,on_movement_end,affected_units,damage,,80|160|240|320|400|480|560,magical,,,,,
skewer,5,on_movement_end,affected_units,apply_status,slow,,,3.25,,20|30|40|50|60|70|80,,
reverse_polarity,0,on_cast,units_in_radius,movement,pull,,,0.05,,,,
reverse_polarity,1,on_cast,units_in_radius,damage,,100|200|300|400,magical,,,,,
reverse_polarity,2,on_cast,units_in_radius,apply_status,stun,,,2.5|3|3.5|4,,,,
```

- **Empower fallback (G4, G5).** The bonus is flat: 61 × 16% … 64% = 9.76 … 39.04. It is exact for Magnus and wrong for allies with a different base damage. There is no cleave.
- **Skewer stun (U9).** Rows 2 and 3 stun each skewered unit from contact until the dash ends.
  - `remove_status,stun` removes *every* status with the id `"stun"`, including another source's stun.
  - Separately, `apply_status` refreshes by id (`status.rs:886-889`), so the contact stun replaces a longer stun already on the target.
  - Without row 3, a unit caught late stays stunned for up to the full dash time (≤ 1.56 s) after the dash.
- **Skewer slow 3.25 s** is a placeholder for U1; the spec gives no duration.
- **Reverse Polarity fallback (G8).**
  - `pull` with no distance chases Magnus until the unit touches him (stop distance = sum of the two bounds; `displacement.rs:392-430`).
  - `plan_motion` floors the duration at 0.05 s, so the pull is effectively instant.
  - Units end adjacent to Magnus on the side they came from, not grouped in front. They may overlap each other until `resolve_unit_collisions` separates them.
- **Debuff-immune enemies** are skipped by the hostile pull, drag and stun. That is the existing rule (`blocks_hostile_relocation`, `apply_status`). Piercing immunity is not in the spec, so it is not proposed.

## 7. `data/heroes.csv` and `data/heroes.example.csv` (Variant B)

The header is unchanged. In `data/heroes.csv`, this row replaces line 5:

```csv
Magnus,TRUE,Magnus Shockwave,Empower,Skewer,Reverse Polarity,23,14,20,3.1,2.2,2.1,STR,"Str initiator — Shockwave, Empower, Skewer, Reverse Polarity",211,75,61,150,305,-0.96,3.764384,0.04,0.5,1.8,0.5,0.84,
```

`data/heroes.example.csv` gets the same row with `true`.

| Field | Formula | Value |
|---|---|---|
| base_health | 625 − 23×18 | 211 |
| base_health_regen | 2.8 − 23×0.12 | 0.04 |
| base_mana | 315 − 20×12 | 75 |
| base_mana_regen | 1.5 − 20×0.05 | 0.5 |
| armor | 1 − 14×0.14 | −0.96 |
| magic_resist | (0.27/0.73)/0.06 − 20×0.12 = 6.164384 − 2.4 | 3.764384 |
| attack_range | 200 in the spec; **150** in the fallback (G1, U3) | 150 |
| attack_projectile_speed | blank (melee) | — |

- `Ability_Q_Name = Magnus Shockwave` produces `MagnusShockwave` explicitly. The name "Shockwave" would also give it, via `unique_ability_id`'s hero prefix, but that is implicit and fragile.
- If G1 is approved, `attack_range` becomes 200.
- If G2 is approved, a `turn_rate` column is appended with the value 0.8.

## 8. Naming

- **Q** uses the id `magnus_shockwave` → `AbilityId::MagnusShockwave`, with the display name "Shockwave" (U5).
  - The display name duplicates Vanguard's in the UI. That is allowed: name uniqueness is checked only within each CSV, and nothing looks abilities up by display name (`ui.rs`, `definition.rs:963` only print it).
  - **Never use the id `shockwave` in `data/abilities.csv`.** It would replace Vanguard's hand-written W.
- **W, E, R** use `empower` → `Empower`, `skewer` → `Skewer`, `reverse_polarity` → `ReversePolarity`. None of these clash.
- HeroGenerator writes stub display names first (e.g. "Magnus Shockwave"). AbilityGenerator then upserts the CSV `name`, as with QoP Blink.

## 9. Gap proposals (none approved; each needs the user's confirmation)

### G1. Melee classification at range 200 (instant attack)
- **Missing.** A melee hero cannot have a base range above 150.5.
- **Minimal design (option b, recommended).** Split the classification threshold from the range constant: `scale::MELEE_CLASSIFICATION_MAX_RANGE` (e.g. 250), used by `is_melee_attack_range`.
  - No current unit's base range lies in (150, 499], so existing behaviour is unchanged.
  - HeroGenerator errors when `is_melee` disagrees with the classification of `attack_range`.
  - Tests: a range-200 hero is melee and gets an instant slash and cleave; range 500 stays ranged.
- **Option a.** Carry the CSV `is_melee` into a new `CombatStats` field. This is more invasive: creeps and towers would also need it.
- **Fallback.** `attack_range` 150. He loses 50 range but keeps the instant attack and cleave eligibility. The other fallback, 200 as a ranged attacker, loses both, so it is not recommended.

### G2. Per-hero turn rate
- **Missing.** No `turn_rate` hero column.
- **Minimal design.** An optional `turn_rate` column in HeroGenerator (> 0, blank = `HERO_TURN_RATE`), emitted into the existing `hero_combat(turn_rate)` parameter. Update the README column docs. Unittest or `SystemExit` check for ≤ 0.
- **Fallback.** 0.6, so he turns 25% slower than specified.

### G3. Cast backswing above 1.0
- **Minimal design.** Raise `MAX_CAST_BACKSWING` to 1.5 in `definition.rs` and its mirror in `AbilityGenerator.py`, and update the README sentence. This is the same change QoP made in its G6. Existing data is unaffected (all values ≤ 0.9).
- **Fallback.** Clamp to 1.0. Q loses 0.2 s, W 0.07 s and R 0.3 s of backswing. Backswing is cancellable, so the gameplay effect is small.

### G4. Percentage bonus attack damage (timed)
- **Minimal design.** A new timed stat `attack_damage_pct`, mapped to a new `StatusSpec::Modifier` field.
  - It is a snapshot at build: `StatusBuildContext` gains the target's base attack damage, and the status stores flat = pct × base, reverted with the exact delta. This matches the composition §4 "snapshot at build" style.
  - Sub-decision U8: "base" is the hero-def damage, excluding item bonuses.
  - Invalid example: a negative pct, or above 1000.
- **Fallback.** Flat values derived from Magnus' 61 (§6). They are exact for Magnus only.

### G5. Timed cleave
- **Minimal design.** Add `cleave` to `TIMED_STATS`, mapped to `StatusSpec::Modifier.cleave_ratio`. It mutates `CombatStats.cleave_damage_ratio` on apply, the same way as `attack_damage`, and reuses the fraction check (0..1). With it, Empower adds `empower,1,on_cast,cast_target,stat_modifier,cleave,0.16|0.24|...|0.64,,30|...|48,,,,`.
  - Invalid example: a timed cleave of 1.5.
- **Fallback.** No cleave at all.

### G6. Cone-shaped cleave (start 150, end 360, distance 650)
- **Minimal design.** Add `CombatStats.cleave_end_radius` and `cleave_distance`, set through G5's status fields (`radius` = start, plus new semantics for end and distance; the exact columns are TBD).
  - `secondary_attack_targets` takes a trapezoid: projection t ∈ [0, distance] along attacker→target, lateral ≤ lerp(start, end, t / distance).
  - Open point: how to combine stacked cleave sources with different shapes (proposed: largest shape wins).
- **Fallback, given G5.** A circle of radius 300 around the attacker. It covers less depth (300 against 650) and more area behind the attacker.

### G7. Dash contact radius
- **Minimal design.** Give the `radius` column a new meaning on `movement,dash` / `charge` rows: the contact radius. `ForcedMovement` gains `contact_radius: Option<f32>`, and contact uses `center_distance <= contact_radius + other.bound`.
  - Invalid example: a radius ≤ 0 on a dash.
- **Fallback.** Body contact (72 for heroes), so the catch is about half as wide.

### G8. Reverse Polarity: gather enemies in front of Magnus
- **Minimal design.** Give the `amount` column a new meaning on `movement,pull_point`: a forward offset.
  - `EffectRunner` sets the displacement point to caster position + facing × amount, the same facing source as `teleport_destination`.
  - Forced motion ignores unit collision, so there are no `Occupied` rejections, which a teleport-based design would hit.
  - RP row: `reverse_polarity,0,on_cast,units_in_radius,movement,pull_point,,,0.05,,<offset>,,`. The offset is not in the spec (U7).
  - Invalid example: a negative `amount` on `pull_point`.
- **Fallback.** `movement,pull` to contact (§6). Units surround Magnus rather than gathering in front of him.

### G9. Stun bound to the drag
- **Minimal design.** Reuse `ForcedMovement.bound_status` (already used for cyclone). The drag binds a `stun` status that is removed when the attached motion ends.
- **Fallback.** Rows 2 and 3 in §6, with the caveats in U9.

### G10. Shockwave starts 48 units in front
- **Minimal design.** An optional skillshot origin offset. Not recommended.
- **Fallback.** The wave starts at Magnus' centre, so it reaches 48 units further behind him (200 against 152). Dota's 25 discrete radii spaced 48 apart are otherwise matched by the continuous sweep.

## 10. Tests

- **`heroes.rs` `magnus_matches_spec`:**
  - `from_cli("magnus")`, primary label "STR", abilities `[MagnusShockwave, Empower, Skewer, ReversePolarity]`.
  - Level-1 vitals: health 625, regen 2.8, mana 315, regen 1.5 (tolerance 1e-4), armor ≈ 1.0.
  - Magical 100 → 73.0 (tolerance 0.01). Attributes (23, 14, 20).
  - Attack damage 61, range 150 and `is_melee()` (range 200 if G1), BAT 1.8, attack point 0.5, backswing 0.84, turn rate 0.6 (0.8 if G2), move speed 305, APS 0.633333.
- **`catalog.rs` `magnus_abilities_generate_expected_definitions`:**
  - `generated::definition(AbilityId::Shockwave).is_none()`, so Vanguard is untouched.
  - Q: Point, range 1200, AoE 200, skillshot speed `Some(1200)`, cooldown at(7) = 7, backswing 1.0 (clamped).
  - W: Unit/Ally, status id `empower_attack_damage`, duration at(7) = 48.
  - E: range `GLOBAL_CAST_RANGE`, Dash distance at(7) = 1400.
  - R: `is_ultimate`, `max_rank` 4, NoTarget, AoE 430, stun at(4) = 4.
  - Display names.
- **Integration tests in `abilities/mod.rs`**, using the existing `test_app()` and an explicit `ProjectileHitEvent` as in QoP note 8:
  - `magnus_shockwave_pulls_toward_wave_center_and_slows`: an enemy is offset from the impact point. Its distance to the impact shrinks by min(150, d) in about 0.2 s; it has a 75% slow for 0.55 s; it takes 75 magical before mitigation.
  - `skewer_drags_stunned_enemy_and_hits_at_destination`: an enemy on the path is contacted, carried and stunned while the dash runs. At the end it has no stun, has taken 80 magical and has a 20% slow.
  - `reverse_polarity_pulls_damages_and_stuns_in_radius`: enemies at 200 and 400 end within the sum of bounds (+ε) of Magnus, are stunned for 2.5 s and take 100. An enemy at 500 is untouched.
  - `empower_grants_flat_attack_damage_to_ally_and_self`.
- **`scripts/test_generator_common.py`:** valid examples still pass and invalid ones are still rejected (existing tests).

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
python3 scripts/HeroGenerator.py <scratch>/magnus.csv --dry-run
python3 scripts/AbilityGenerator.py && python3 scripts/ItemGenerator.py && git diff --stat src/abilities/generated.rs src/item_catalog.rs
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
```

## 11. Invariants, risks and open decisions

**Invariants and risks**
- I8 holds: the only generated diffs are the 4 new ability blocks, the Magnus hero arms and the component regions.
- The Vanguard `Shockwave` definition must stay hand-written.
- Status refresh by id: all slows share `"slow"`, and all stuns share `"stun"`. The Skewer and Shockwave slows overwrite each other, and Skewer's `remove_status,stun` strips foreign stuns.
- With a 0.05 s pull, Reverse Polarity moves units up to about 8600 u/s. The terrain sweep in `tick_forced_movement` stops them at walls.
- A hero-select UI with 7 heroes may overflow (cosmetic).
- `data/heroes.example.csv` now contains Magnus. Running HeroGenerator on it for real would add him; use only `--dry-run`, as the README already says.

**Open decisions**
- **U1.** Skewer slow duration (placeholder 3.25 s).
- **U2.** Skewer damage at the destination (recommended) or on contact.
- **U3.** G1 fallback range: 150 melee (recommended) or 200 ranged.
- **U4.** Accept Empower's flat fallback values.
- **U5.** Q display name "Shockwave" (duplicates Vanguard's in the UI) or "Magnus Shockwave".
- **U6.** Replace the old sample rows `shockwave` / `skewer` in `data/examples/`.
- **U7.** RP forward offset (needed only for G8).
- **U8.** G4's definition of "base damage".
- **U9.** Skewer stun ends with `remove_status` (strips other stuns) or simply over-stuns.

## 12. User decisions (2026-11-08)

Approved gaps: **G1, G2, G3, G4, G5, G6, G7, G8 (modified), G9 (modified), G10.**

- **G1.** Build option b: `MELEE_CLASSIFICATION_MAX_RANGE`. Magnus' `attack_range` is **200** and he is melee (instant attack, cleave eligible).
- **G2.** Add an optional `turn_rate` hero column. Magnus is 0.8.
- **G3.** Raise `MAX_CAST_BACKSWING` to 1.5. The backswings 1.2, 1.07 and 1.3 apply unclamped.
- **G4.** Timed `attack_damage_pct`, snapshot of the target's **base (hero-def) attack damage only**, excluding item bonuses (U8). Empower gives 16 to 64% (16|24|32|40|48|56|64), replacing the flat fallback.
- **G5.** Add a timed `cleave` stat. Empower gives 0.16 to 0.64, effective only on melee targets.
- **G6.** Cone (trapezoid) cleave: start 150, end 360, distance 650.
- **G7.** Dash contact radius. Skewer uses 145.
- **G8 (modified).** Pull, not teleport. Enemies in radius 430 are **pulled to a point 250 units in front of Magnus** (caster position + facing × 250, U7). This uses the proposed `pull_point` forward-offset design, not a teleport.
- **G9 (modified).** The stun is applied on contact and removed when the dash ends. **Removing it must not remove any other stun debuff on the unit**, and applying it must not overwrite another source's stun. Implement this with the `bound_status` approach and a status id unique to this source (e.g. `skewer_stun`, which still counts as a stun). Do not use `remove_status,stun`, which strips every stun. Skewer row 3 of §6 is dropped.
- **G10.** Optional skillshot origin offset. Shockwave starts 48 units in front of Magnus.
- **U1.** Skewer slow lasts 3.25 s.
- **U2.** Skewer damage lands at the destination (default).
- **U5.** Q display name "Shockwave", id `magnus_shockwave`.
- **U6.** Replace the old `shockwave` and `skewer` sample rows in `data/examples/` with Magnus' rows, and append `empower` and `reverse_polarity`.
- **U9.** Superseded by the G9 decision.

Constraint: build nothing beyond these approved items. If implementation shows that anything else is needed, stop and ask the user.

## 13. Implementation notes (2026-11-09)

Each approved gap landed as its own commit (G1 to G10, in that order), each with Rust and generator tests and, for every new validation rule, a row in `data/examples/invalid/`; the Magnus data, docs and hero tests came last. Nothing was built beyond G1 to G10. Where §12 left the exact surface open, the choices below were made (each is the smallest form that expresses the approved design):

1. **G1.** `scale::MELEE_CLASSIFICATION_MAX_RANGE` is 250 and `is_melee_attack_range` no longer has the `+ 0.5` slack (no unit's base range lies between 150 and 500). HeroGenerator exposes `is_melee_range` and rejects a row whose `is_melee` disagrees with `attack_range` (blank ranges still default to 128 / 500, so existing rows pass).
2. **G2.** `turn_rate` is the last hero column (after `attack_projectile_speed`). Both hero CSV headers gained it; short rows parse as blank.
3. **G3.** The cap is 1.5. Magnus' 1.2, 1.07 and 1.3 backswings apply unclamped, and the generator emitted no `cast_backswing` warning.
4. **G4.** `StatusSpec::Modifier.attack_damage_pct` (a percent, 0..1000) is added to the flat bonus as `pct / 100 x StatusBuildContext.base_attack_damage`. The base comes from a new `base_attack_damage(Option<&HeroKind>, Option<&CombatStats>)` helper: the hero definition's `combat.attack_damage` for heroes (U8), and **the unit's current damage for units that have no hero definition** (creeps, test units). The spec did not say what a non-hero should use; this was the least surprising fallback. `build_status_effect` gained a `target_base_attack_damage` parameter, so the three call sites (`EffectRunner`, `sync_conditional_statuses`, `tick_toggle_upkeep`) now query `HeroKind`.
5. **G5.** `StatusEffect.cleave` is a delta on `cleave_damage_ratio` (applied and reverted by `add_status_stat_delta` / `subtract_status_stat_delta`). The generator checks the value is within 0..1.
6. **G6 (deviation from the proposal).** §9 left the columns open ("`radius` = start, plus new semantics for end and distance"). A single row has only `radius` and `amount` free and the shape needs three numbers, so the shape is three more **timed stats** instead, each above 0 and additive like every stat: `cleave_radius` (start half-width, the existing `CombatStats::cleave_radius`), `cleave_end_radius` and `cleave_distance` (new `CombatStats` fields). A non-zero `cleave_distance` turns the cleave into a cone. Consequences:
   - Empower has five rows (`attack_damage_pct`, `cleave`, `cleave_radius`, `cleave_end_radius`, `cleave_distance`), all with the same duration.
   - Shapes from different sources **add** (the proposal's "largest shape wins" is not implemented; there is only one source today).
   - The widths are half-widths measured on the centre line (`lateral <= lerp(start, end, t)`, exactly as written in §9), widened by the other unit's body radius. If `cleave_end_radius` is unset, the end equals the start.
7. **G7.** Contact is `center_distance <= contact_radius + other.bound` (the mover's own bound is replaced, not added). Blank keeps body contact. The new `radius` meaning is only read for `dash` and `charge`; on other movements that are not `units_in_radius` the generator warns that it is ignored.
8. **G8.** `forward_offset` is `caster position when the trigger fired + flat facing x amount`, with `-Z` when the facing has no flat direction (the same fallback as bounded teleports). `amount` must not be negative. `PullToPoint` with no `value` pulls the whole way, so units end exactly on the gather point.
9. **G9 (deviation from the §6 rows).** §12 asked for a bound stun with a source-unique id without a `remove_status` row. The CSV needed one keyword to say "this drag also stuns", so `movement,drag_stun` was added (a `drag` plus `bound_stun: Some("<owner>_stun")`). The drag itself applies the stun, so the old row 2 (`apply_status,stun`) is also gone and Skewer has four rows: dash, `drag_stun`, damage, slow.
   - `apply_displacement_events` applies a stun status with the unique id and records it in `ForcedMovement.bound_status`; `tick_forced_movement` writes `StatusChange::Remove(id)` when an `Attached` motion ends, however it ends (arrival, block, stun, death, vanished anchor).
   - The stun's own duration is only a backstop for a drag that is replaced without ending (a cyclone, say): the carrying dash's remaining time plus 0.25 s, or 3.0 s when the dash started in the same frame.
   - Refresh is by id, so another source's stun is neither overwritten nor removed (covered by `drag_bound_stun_lasts_exactly_as_long_as_the_drag_and_spares_other_stuns` and `skewer_drags_stunned_enemy_and_hits_at_destination`).
   - The cyclone's use of `bound_status` (end the motion when the status is gone, on `Hold` paths) is unchanged.
10. **G10.** The column is `projectile_origin_offset` (skillshots only, level list, above 0; items reject it). The wave starts `offset` in front of the caster towards the aim and flies the full cast range **from there** (it ends `offset` further than before). The width and `projectile_end_radius` apply from the new start.
11. **Data.**
    - The Q id is `magnus_shockwave` with the display name "Shockwave"; `generated.rs` has no `AbilityId::Shockwave` arm.
    - `data/heroes.csv` replaced the stale Magnus row (attack range 200, `turn_rate` 0.8) and `data/heroes.example.csv` got the same row. HeroGenerator ran only on a scratch CSV holding the header and the Magnus row.
    - `data/examples/` replaced the old `shockwave` / `skewer` rows with Magnus' rows and appended `empower` and `reverse_polarity`.
    - Descriptions of Empower and Reverse Polarity were edited to match what the rows do (cone cleave; gather point in front).
    - README samples that quoted the old `shockwave` / `skewer` rows now quote the Magnus rows.
12. **Superseded test names.** §10 expected the W status id `empower_attack_damage`; with G4 and G5 the ids are `empower_attack_damage_pct`, `empower_cleave`, `empower_cleave_radius`, `empower_cleave_end_radius` and `empower_cleave_distance`. `empower_grants_flat_attack_damage_to_ally_and_self` became `empower_grants_percent_damage_and_cone_cleave_to_self_and_ally`.
13. **Environment.** The build needs system libraries that this container lacked (`libwayland-dev`, `libxkbcommon-dev`, `libasound2-dev`, `libudev-dev`, `libx11-dev`, `pkg-config`); they were installed with apt. No repository file was changed for this.

Remaining limitations:

- Skewer's slow and Shockwave's slow share the id `slow`, so the last one applied wins.
- Tooltips show no row for a dash contact radius, a pull-point forward offset or a skillshot origin offset.
- `data/ability_pseudos/*.pseudo.txt` mention behaviour that §12 did not approve (a 50% larger Empower bonus when cast on self, an attack-speed slow on Skewer). None of it is implemented.
- Empower's `attack_damage_pct` for a non-hero target is a snapshot of that unit's current damage, which includes any item bonuses it has at that moment.
- The hero-select UI may overflow with seven heroes (cosmetic).

## 14. Follow-up user decisions (2026-11-09)

1. **Independent cleaves (changes the G6/G5 stacking rule).** Each cleave source works on its own instead of being summed into one aggregate shape. An item's permanent cleave and each timed cleave status (e.g. Empower) is its own instance with its own ratio and shape: a circle of `cleave_radius` (default `ATTACK_CLEAVE_RADIUS`), or a cone when its distance is non-zero. On each melee attack hit, every instance independently selects its own secondary targets and deals its own ratio × attack damage. A unit inside two shapes takes both. The CSV surface is unchanged: the same timed stats, now grouped per status rather than summed into `CombatStats`.
2. **G4 non-hero base damage.** Confirmed: units with no hero definition snapshot their current attack damage.
3. **Cleave grouping (2026-11-09, refines item 1).** The user chose option 1: an internal cleave-group key, with no CSV or generator change.
   - Each timed cleave modifier status carries a cleave source key, the owning ability id. Statuses with the same key make up **one** cleave instance: ratio and shape fields are summed per key.
   - The instance is removed when its last contributing status ends. A re-cast refreshes the single instance.
   - **Items:** item cleave ratios stack with each other additively, as today (two 40% items give 80%), in one "items" instance with the default circle.
   - **Items and abilities stack:** the item instance and every ability instance each deal their own cleave on the same hit, so their damage adds.
   - Item 1's wording "grouped per status" is superseded by this item.
