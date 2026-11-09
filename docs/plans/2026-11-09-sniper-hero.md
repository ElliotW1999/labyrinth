# Sniper hero: implementation spec

Status: **approved 2026-11-09** (decisions in §12). Nothing beyond the approved gaps may be built. If implementation shows that something else is needed, stop and ask.

Related plans:
- `docs/plans/2026-11-08-magnus-hero.md`: the model for this spec (scratch-CSV HeroGenerator flow, `MAX_CAST_BACKSWING`, `turn_rate` column, `<owner>_stun` id pattern).
- `docs/plans/2026-11-08-queen-of-pain-hero.md`: `attack_projectile_speed`, Variant B back-solve, per-owner burn ids (precedent for G5).
- `docs/architecture/composition.md`
- `2026-11-08-attack-modifiers.md` (on-attack-hit procs), `2026-11-07-movement-mechanics.md` (knockback), `2026-11-06-status-effects-catalog.md` (slows, stuns), `2026-11-06-activation-targeting-mechanics.md` (cast point and range).

## 1. Current state (verified)

**No Sniper data or code.** Commit 202f421 ("Update abilities.csv") only added a trailing empty column to six older rows.

**No id clashes.** `AbilityId` (`src/components.rs:661-710`) has no `Shrapnel`, `Headshot`, `TakeAim` or `Assassinate`.

**Reusable as is:**
- **Proc on attack hit.** `AbilityEffect::Proc` / `pseudo_proc` under `on_attack_hit`, as in example items `basher` / `prd_basher` (`data/examples/item_effects.csv`). A derived context keeps `trigger_unit` and `attack` (`effects.rs:240`).
- **Knockback direction.** For `on_attack_hit` the trigger point is the struck unit's position (`attack_triggers.rs:58-60`); `plan_motion` knockback falls back to "away from the attacker" (`displacement.rs:374-380`). Duration floor 0.05 s equals the spec.
- **Attack range passive.** `stat_modifier,attack_range` → `PassiveStat::AttackRange` → `attack_range_bonus` via `passives_at_rank` (precedent: `stone_skin`). `base_attack_range()` excludes the bonus (`components.rs:332`), so projectile speed does not change with Take Aim.
- **Flat attack speed debuff.** Timed `stat_modifier,attack_speed_flat` with a negative value → Debuff `Modifier` id `<owner>_attack_speed_flat`.
- **100% slow.** Supported, floored at `scale::MIN_MOVE_SPEED` (100) by `effective_move_speed` (`components.rs:359-366`).
- **Hero columns.** `attack_projectile_speed`, `turn_rate`, AGI primary already parsed by HeroGenerator.

**Gaps:**
- **Cast point cap.** `AbilityDefinition::timing` clamps cast point to 0.5 and backswing to `MAX_CAST_BACKSWING` (1.5) (`definition.rs:13,1046-1049`). AbilityGenerator warns above 0.5 (`AbilityGenerator.py:218`).
- **No range check during the cast point.** `refresh_activation_aims` only follows the target (`pipeline.rs:347-365`); `tick_activations` cancels only on disable (`pipeline.rs:414`).
- **No persistent or delayed area effect.** No zone/thinker entity; `on_interval` is tied to its holder. README lists "delayed effects" as not expressible.
- **Shared stun id.** Every generated stun uses `"stun"`; a 0.1 s mini-stun would shorten a longer stun from another source.
- **Shared slow id.** Every generated slow uses `"slow"` (`definition.rs:591`), so Headshot's and Shrapnel's slows overwrite each other.
- **Slow refresh drift.** `build_status_effect` (`effects.rs:1017`) reads `CombatStats.move_speed` after status deltas, and `apply_status` (`status.rs:939`) removes the old same-id status only after the new one is built. A re-applied p% slow settles at p/(1+p); a 100% slow refreshed while active vanishes.

## 2. Scope and level extrapolation

Basics have 7 ranks and the ultimate 4. Values extrapolate arithmetically; constants stay constant.

| Ability | Field | Ranks |
|---|---|---|
| Q | radius | 400, 425, 450, 475, 500, 525, 550 |
| Q | DPS (magical) = per tick at interval 1 s | 30, 45, 60, 75, 90, 105, 120 |
| Q | slow % (per tick) | 12, 18, 24, 30, 36, 42, 48 |
| Q | constants | cast range 1800, cast point 0.3, backswing 0, delay 1.2, duration 10, interval 1, cooldown 10, mana 75 |
| W | damage (physical) | 20, 50, 80, 110, 140, 170, 200 |
| W | slow duration (100% MS, −100 AS) | 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8 |
| W | constants | proc 40%, knockback 50 over 0.05 s |
| E | total range from the spec | 785, 940, 1095, 1250, 1405, 1560, 1715 |
| E | **bonus written to CSV** (total − 550) | 235, 390, 545, 700, 855, 1010, 1165 |
| R | damage (magical) | 300, 400, 500, 600 |
| R | cooldown | 20, 15, 10, 5 |
| R | constants | mana 175, cast range 3000, buffer 600, cast point 2.0, backswing 1.6, projectile speed 1500, mini-stun 0.1 s |

**Take Aim values are totals.** All exceed base 550; the data convention is a flat bonus (`PassiveStat::AttackRange`); there is no "set range" semantics; read as bonuses, rank 4 would already reach 1800 (= `HERO_VISION_RANGE`).

## 3. Mapping onto existing features

| Spec item | Existing surface | Fidelity |
|---|---|---|
| Vitals and attributes | Variant B back-solve (§8) via `HeroAttributes::apply_to` | Exact |
| Attack 43, BAT 1.7, AS 100, swings 0.17/0.7, MS 285 | Hero CSV columns; APS = (100+27)/100/1.7 = 0.747059 | Exact |
| Range 550, ranged | `is_melee` FALSE | Exact |
| Projectile speed 3000 | `attack_projectile_speed` 3000 | Exact |
| Turn rate 0.7 | `turn_rate` column | Exact |
| Q area target, radius, cast 0.3/0 | `point` + `aoe_radius` + `none` → `TargetType::Area` | Exact |
| Q delay 1.2, 10 s DPS + slow | — | **G2** (zone) |
| Q slow on each hit | Zone tick re-applies a short slow | Needs **G5**, **G6** |
| W 40% on attack | `pseudo_proc`/`proc` under `on_attack_hit` | Exact |
| W damage, slows, knockback | `damage` physical; `apply_status,slow` 100; `stat_modifier,attack_speed_flat,-100`; `movement,knockback` | Exact (slow conflicts without G5/G6) |
| E attack range passive | `passive,caster,stat_modifier,attack_range` | Exact |
| R unit target, 3000, homing shot | `unit`, `targeted` projectile, `on_projectile_hit` | Exact |
| R cast point 2.0, backswing 1.6 | Clamped to 0.5 / 1.5 | **G1** |
| R cast range buffer 600 | — | **G3** |
| R mini-stun | Plain `stun` would replace longer foreign stuns | **G4** |

## 4. Gap proposals

### G1. Cast point and backswing caps
- `pub const MAX_CAST_POINT: f32 = 2.0` in `src/abilities/definition.rs`; raise `MAX_CAST_BACKSWING` to 2.0; `timing()` clamps to these. Mirror in `AbilityGenerator.py` (replace hard-coded 0.5). Update doc comment (`definition.rs:982`) and README.
- Existing data unaffected (cast points ≤ 0.5, backswings ≤ 1.3).
- Tests: replace `timing_clamps_backswing_at_max_cast_backswing_and_cast_point_at_half` with `timing_clamps_at_max_cast_point_and_backswing`; integration: Assassinate does not fire at 1.9 s, fires at 2.0 s.

### G2. Delayed persistent area zone (reusable; abilities and items)
- **Surface.** New effect type `zone`, shaped like `chain` (feeds a custom trigger). Columns: `effect_id` = custom tick trigger `on_<name>`; `value` = tick interval (> 0); `duration` = active duration (> 0, ≥ interval); `radius` (blank → ability `aoe_radius`; items must set it); `amount` = delay before first tick (≥ 0, blank → 0). Level lists allowed. `effect_target` must be `caster`/`owner`. Centre = trigger point (the aim for `on_cast`).
- **Rust (`definition.rs`).** `AbilityEffect::Zone { radius: AreaRadius, team: TargetTeam, delay: RankValue, duration: RankValue, interval: RankValue, then: AbilityTrigger }`; `acts_on_targets()` false.
- **Rust (`effects.rs`).** Once per execution; no-op past `MAX_TRIGGER_DEPTH`. Writes `SpawnZoneEvent { template, center, radius, team, delay, interval, ticks }`. `template = ctx.derived(then)` with `affected_units`/`trigger_unit` cleared and `attack = None`. `ticks = floor(duration / interval + 1e-4)`.
- **New module `src/abilities/zones.rs`** (Triggers layer): `SpawnZoneEvent`, `AbilityZone` component. `spawn_requested_zones` after `resolve_ability_triggers`; `tick_zones` in the dispatcher chain right after `reactive_triggers::interval_triggers` (mirror in `test_app`). Tick k due at `delay + k × interval` (1e-4 slack); overdue ticks catch up in one frame. Each tick writes an `AbilityTriggerEvent` with `affected_units` = living targetable units of `team` in radius (never the caster), via shared `unit_in_area`; written even when empty. Despawn at `delay + duration` or when the caster entity no longer exists (outlives caster death). Optional Nova ring visual via `Option<Res<SharedAssets>>`.
- **Tooltip.** "Zone: radius R, after Ds, every Is for Ts".
- **Generator (`generator_common.py`).** Add `zone` to `EFFECT_TYPES` and `CUSTOM_TRIGGER_FEEDERS`; team as `chain`; `chain_fed_triggers` follows zone rows (reject `perform_attack` under zone-fed triggers); error when radius blank and no aoe.
- **Composition doc.** Update §1, §3, §7 checklist.
- Ticks are depth ≥ 1 → not spell hits; not DoT → fire `on_damage_dealt`/`on_damage_taken`.
- Invalid examples: interval 0; interval > duration; negative delay; non-caster target; `perform_attack` under zone-fed trigger.

### G3. Cast range buffer (unit targets)
- `AbilityDefinition.cast_range_buffer: RankValue` (default ZERO) + builder. `ActionSpec.cast_range_buffer: f32` (0 = none) at its construction sites (`casting.rs`, `items.rs` 0.0, `targeting.rs` test, `actions/mod.rs`).
- `Activation` gains `break_range: Option<f32>`, `target_bounds: Option<f32>`; set by `validate_activation_requests` only when buffer > 0 and target is `CastTarget::Unit`.
- In `tick_activations`, before the cast point counts down: if target is still a unit and `cast_distance(...) > break_range`, remove the `Activation` free (no mana, no cooldown; unit commands see "interrupted before firing"). Dead target → point → skipped.
- Generator: new optional ability column `cast_range_buffer` (appended; `LEVEL_FIELDS`; > 0; requires `unit`; warn with `global`). Emit only when set. Abilities only (items follow-up).
- Tooltip row "Cast range buffer".

### G4. `mini_stun`
- `StatusSpec::OwnedStun { id }` → `StatusEffect { id, ..StatusEffect::stunned(duration) }`. Generator: `apply_status,mini_stun` → id `<owner>_stun`. `runtime_status_id` accepts it; `remove_status,mini_stun` rejected with hint. Optional warning below 0.05 s.
- Tests: a 3 s foreign `stun` survives Assassinate's 0.1 s `assassinate_stun`; a channelling target is interrupted.

### G5. Per-owner ids for generated slows
- `StatusSpec::Slow { id, percent }`. Hand-written slows keep `"slow"`; generated use `<owner>_slow`. `runtime_status_id` accepts it. Allowed generated diff: the 4 existing generated Slow sites. Update tests asserting `"slow"` on generated abilities and README/composition docs.
- Behaviour change: slows from different generated sources coexist and add; same source refreshes.

### G6. Slow measured against base speed
- `StatusEffect.move_slow_fraction: f32` (default 0) set by `StatusSpec::Slow`. In `apply_status`, after removing any same-id status, if fraction > 0: `effect.move_speed = −fraction × max(0, stats.move_speed − Σ statuses.effects.move_speed)`.
- Tests: 30% slow refreshed 10× stays exactly 0.7 × base; 100% slow refreshed while active stays 100%; 100% + 48% from two ids floors at `MIN_MOVE_SPEED` and both expire back to base; first application unchanged.

## 5. Steps (one commit each, build + tests green between)

1. G6. 2. G5 (regenerate; only 4 Slow sites change). 3. G1. 4. G3 (header column in `data/abilities.csv`, `data/examples/abilities.csv`, `data/examples/invalid/abilities.csv`; pad existing rows with one trailing comma). 5. G4. 6. G2. 7. Data: rows of §6–§8; HeroGenerator on a scratch CSV first (`--dry-run`), then for real; AbilityGenerator (4 additions, ranks 7/7/7/4); `cargo fmt`. 8. Docs: README hero row and column docs; composition.md §6.13 "Sniper"; implementation notes appended here.

## 6. `data/abilities.csv` (+ examples), header gains `,cast_range_buffer`

```csv
shrapnel,Shrapnel,Q,active,point,enemy,1800,400|425|450|475|500|525|550,0.3,,10,75,,,none,,"Launches a ball of shrapnel that showers the target area in explosive pellets after 1.2s, dealing magical damage every second for 10s and slowing enemies it hits",0,,,,,
headshot,Headshot,W,passive,no_target,enemy,,,,,,,,,none,,"Grants Sniper a chance to deal extra damage and briefly knock back the attacked enemy unit",,,,,,
take_aim,Take Aim,E,passive,no_target,self,,,,,,,,,none,,"Grants Sniper additional attack range",,,,,,
assassinate,Assassinate,R,active,unit,enemy,3000,,2,,20|15|10|5,175,,,targeted,1500,"Locks onto an enemy unit; after a 2s aim, fires a devastating long-range shot that deals magical damage and mini-stuns the target. Cancelled if the target moves 600 beyond cast range",1.6,,,,,600
```

## 7. `data/ability_effects.csv` (+ examples)

```csv
shrapnel,0,on_cast,caster,zone,on_shrapnel_tick,1,,10,,1.2,,
shrapnel,1,on_shrapnel_tick,affected_units,damage,,30|45|60|75|90|105|120,magical,,,,,
shrapnel,2,on_shrapnel_tick,affected_units,apply_status,slow,,,1.1,,12|18|24|30|36|42|48,,
headshot,0,on_attack_hit,caster,pseudo_proc,on_headshot,40,,,,,,
headshot,1,on_headshot,trigger_unit,damage,,20|50|80|110|140|170|200,physical,,,,,
headshot,2,on_headshot,trigger_unit,apply_status,slow,,,0.2|0.3|0.4|0.5|0.6|0.7|0.8,,100,,
headshot,3,on_headshot,trigger_unit,stat_modifier,attack_speed_flat,-100,,0.2|0.3|0.4|0.5|0.6|0.7|0.8,,,,
headshot,4,on_headshot,trigger_unit,movement,knockback,50,,0.05,,,,
take_aim,0,passive,caster,stat_modifier,attack_range,235|390|545|700|855|1010|1165,,,,,,
assassinate,0,on_projectile_hit,trigger_unit,damage,,300|400|500|600,magical,,,,,
assassinate,1,on_projectile_hit,trigger_unit,apply_status,mini_stun,,,0.1,,,,
```

Shrapnel: 10 ticks at 1.2, 2.2, …, 10.2 s → total 10 × DPS. Slow lasts 1.1 s (interval + 0.1).
Headshot damage carries attack metadata (attack bonus damage); knockback pushes away from Sniper.

## 8. `data/heroes.csv` and `data/heroes.example.csv` (Variant B)

```csv
Sniper,FALSE,Shrapnel,Headshot,Take Aim,Assassinate,19,27,15,2,3.2,2.6,AGI,"Agi marksman — Shrapnel, Headshot, Take Aim, Assassinate",196,75,43,550,285,-3.78,4.20907,-0.13,0,1.7,0.17,0.7,3000,0.7
```

| Field | Formula | Value |
|---|---|---|
| base_health | 538 − 19×18 | 196 |
| base_health_regen | 2.15 − 19×0.12 | −0.13 |
| base_mana | 255 − 15×12 | 75 |
| base_mana_regen | 0.75 − 15×0.05 | 0 |
| armor | 0 − 27×0.14 | −3.78 |
| magic_resist | (0.265/0.735)/0.06 − 15×0.12 | 4.20907 |
| attack_projectile_speed | absolute | 3000 |

## 9. Tests

- `heroes.rs::sniper_matches_spec`: abilities `[Shrapnel, Headshot, TakeAim, Assassinate]`, AGI; level-1 health 538, regen 2.15, mana 255, regen 0.75, armor ≈ 0, magical 100 → 73.5, attrs (19,27,15), range 550 ranged, BAT 1.7, swings 0.17/0.7, turn rate 0.7, MS 285, projectile 3000, APS 0.747059.
- `catalog.rs::sniper_abilities_generate_expected_definitions`: Q Area, range 1800, aoe at(7)=550, Zone (1.2/1/10/`on_shrapnel_tick`); W passive pseudo proc 0.4; E bonus 235 / 1165; R ultimate max rank 4, cast 2.0, backswing 1.6, buffer 600, projectile 1500, stun id `assassinate_stun`.
- Integration (`abilities/mod.rs`): `shrapnel_zone_waits_then_ticks_ten_times`, `shrapnel_zone_slows_units_inside_and_releases_after_leaving`, `headshot_proc_damages_slows_and_knocks_back`, `headshot_and_shrapnel_slows_coexist`, `take_aim_adds_attack_range_per_rank_and_keeps_projectile_speed`, `assassinate_fires_after_two_second_aim_and_mini_stuns`, `assassinate_cancels_free_when_target_leaves_buffer` (3700 cancels, 3300 fires), `mini_stun_spares_a_longer_foreign_stun`.
- Per-gap unit tests (§4) incl. zones: reactive inheritance, depth cap, `attack` cleared, `item_and_ability_zone_emit_identical_ticks`.
- `scripts/test_generator_common.py`: `zone`, `cast_range_buffer`, `mini_stun`, `<owner>_slow`, invalid examples.

```bash
cargo fmt --check && cargo build && cargo test
python3 scripts/AbilityGenerator.py --check
python3 scripts/ItemGenerator.py --check
python3 scripts/AbilityGenerator.py data/examples/abilities.csv data/examples/ability_effects.csv --check
python3 scripts/ItemGenerator.py data/examples/items.csv data/examples/item_effects.csv --check
(cd scripts && python3 -m unittest test_generator_common)
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
```

## 10. Invariants and risks

- Zone only emits `AbilityTriggerEvent`; damage/slows go through `EffectRunner`. Cast-range cancel lives in the activation layer; slow snapshot in `status::apply_status`.
- Only allowed generated diffs: the 4 Slow sites (G5), the 4 new ability blocks, Sniper hero arms/regions.
- Zone templates inherit `reaction`; depth = spawn depth + 1, capped.
- A zone spawned in frame N starts counting in N+1. One zone per row execution; catch-up never exceeds `ticks`.
- At rank-7 Take Aim (1715) Sniper attacks near the edge of vision (1800); Assassinate's 3000 exceeds vision.

## 11. Naming

| CSV id | `AbilityId` | Display |
|---|---|---|
| `shrapnel` | `Shrapnel` | Shrapnel |
| `headshot` | `Headshot` | Headshot |
| `take_aim` | `TakeAim` | Take Aim |
| `assassinate` | `Assassinate` | Assassinate |

Status ids: `shrapnel_slow`, `headshot_slow`, `headshot_attack_speed_flat`, `assassinate_stun`. Triggers: `on_headshot`, `on_shrapnel_tick`.

## 12. Open decisions

- G1–G6 approval (G2 required for Shrapnel).
- U1 Shrapnel cooldown 10, no charges. U2 tick interval 1 s, slow 1.1 s. U3 `pseudo_proc` vs `proc`. U4 Assassinate projectile speed 2500 (unspecified). U5 mini-stun 0.1 s. U6 R rank-4 cooldown 5. U7 Take Aim 7 ranks. U8 keyword `mini_stun`. U9 zone visual: optional ring only.

### Decisions (user, 2026-11-09)
- G1–G6: all approved.
- U3: `pseudo_proc`.
- U4: Assassinate projectile speed **1500** (user choice; overrides the 2500 assumption).
- U1, U2, U5–U9: accepted as proposed (cooldown 10 no charges; 1 s ticks, 1.1 s slow; 0.1 s mini-stun; R rank-4 cooldown 5; 7-rank Take Aim; keyword `mini_stun`; optional ring visual only).

## 13. Implementation notes (2026-11-09)

Steps ran in the §5 order, one commit each (G6, G5, G1, G3, G4, G2, data, docs), with `cargo fmt`, `cargo build` and `cargo test` green between them. Nothing outside G1 to G6 was built.

**What was built**
- **G6.** `StatusEffect.move_slow_fraction`, set by `StatusSpec::Slow`; `apply_status` recomputes the slow against `stats.move_speed - Σ statuses.move_speed` after removing the same-id status. First application is unchanged. Tests in `status.rs`: `first_slow_application_is_unchanged`, `refreshed_slow_stays_at_its_percent_of_base_speed`, `refreshed_full_slow_stays_a_full_slow`, `slows_from_two_ids_add_floor_and_expire_back_to_base`.
- **G5.** `StatusSpec::Slow { id, percent }`; generated slows use `<owner>_slow`. The only generated diff was the four existing Slow sites. The tests that asserted `"slow"` on generated abilities now use `shadow_strike_slow`, `magnus_shockwave_slow` and `skewer_slow`.
- **G1.** `MAX_CAST_POINT` 2.0 and `MAX_CAST_BACKSWING` 2.0, mirrored in `AbilityGenerator.py`. `timing_clamps_at_max_cast_point_and_backswing` replaces the old clamp test.
- **G3.** `AbilityDefinition.cast_range_buffer` plus builder, `ActionSpec.cast_range_buffer`, `Activation.break_range` / `target_bounds`; the break check sits in `tick_activations` after the disable check and before the turn and cast point countdown. Column `cast_range_buffer` appended to the three ability CSV headers (existing rows padded with one comma). Tooltip row "Cast range buffer".
- **G4.** `StatusSpec::OwnedStun { id }`; `apply_status,mini_stun` binds `<owner>_stun`.
- **G2.** `AbilityEffect::Zone`, `abilities/zones.rs` (`SpawnZoneEvent`, `AbilityZone`, `spawn_requested_zones`, `tick_zones`), the `zone` effect type in the generators, five invalid example rows and `ZoneTests`.
- **Data.** HeroGenerator ran only on a scratch CSV (header plus the Sniper row, kept in the session scratchpad outside the repo): `--dry-run` showed `Shrapnel/Headshot/TakeAim/Assassinate`, then the real run patched `heroes.rs`, `components.rs` and the README table. AbilityGenerator added the four ability blocks (ranks 7/7/7/4) and the generated diff is additions only. Sniper rows were added to `data/heroes.csv`, `data/heroes.example.csv` and the ability CSVs, examples included. Assassinate's projectile speed is 1500 (decision U4).

**Deviations from the spec (all small and additive)**
1. `SpawnZoneEvent` and `AbilityZone` carry a `duration` field in addition to the §4 list. Despawn is specified at `delay + duration`, which cannot be recovered from `ticks` alone when the duration is not a multiple of the interval.
2. `EffectRunner::execute` gained an `aoe_radius: f32` parameter (its single caller passes `params.aoe_radius`), so a `Zone` with `AreaRadius::Ability` resolves the way `UnitsInRadius` does.
3. `remove_status,slow` and `remove_status,mini_stun` are rejected with a hint to name `<owner>_slow` / `<owner>_stun`. Without it, a generated `remove_status,slow` would have become a silent no-op once G5 renamed the ids.
4. `runtime_status_id("mini_stun")` returns `None`; only `<owner>_stun` is accepted as a trigger argument, because the keyword's id depends on the owner. `<owner>_slow` and `<owner>_stun` are accepted by the same suffix rule as `<owner>_burn`.
5. The zone's activation ring (a Nova ring, only when `SharedAssets` exists) is spawned at the first tick rather than when the zone spawns. The generator also adds the usual cast-time `nova_ring()` mechanic to Shrapnel because it is an active area ability with a radius; both are cosmetic.
6. The "Assassinate does not fire at 1.9 s, fires at 2.0 s" check is two tests (`assassinate_does_not_fire_before_the_aim_completes`, and the timing assertion in `assassinate_fires_after_two_second_aim_and_mini_stuns`) with one frame (0.05 s) of tolerance for the f32 countdown. In that test the foreign stun lasts 10 s instead of 3 s, because a 3 s stun would already have run out during the 2 s aim.
7. `mini_stun_spares_a_longer_foreign_stun` uses a minimal test ability with `StatusSpec::OwnedStun`; `assassinate_fires_after_two_second_aim_and_mini_stuns` covers the Assassinate case. `mini_stun_interrupts_a_channelling_target` covers the interruption.
8. Extra tests beyond §9: `shrapnel_ticks_scale_with_rank`, `shrapnel_slow_is_refreshed_not_compounded_while_inside`, and the zone and cast-range-buffer unit tests.
9. The status ticker is not part of `abilities::tests::test_app`, so tests that need expiry use `qop_app()`; the generic zone and G3/G4 tests do not rely on expiry.

**Environment.** The build needed system libraries this container lacked (`libwayland-dev`, `libxkbcommon-dev`, `libasound2-dev`, `libudev-dev`, `pkg-config`); they were installed with apt. No repository file changed for this.

**Remaining limitations**
- A zone has no persistent visual: one Nova ring at its first tick, nothing for the rest of its life.
- Zone ticks and the cast-range-buffer cancel are abilities-only for the buffer (items follow-up, as approved) while zones work for items; no shipped item uses a zone.
- Take Aim at rank 7 (1715) puts Sniper's attacks at the edge of vision (1800), and Assassinate's 3000 exceeds vision; no vision rule was changed.
- `data/ability_pseudos/*.pseudo.txt` were not touched.
