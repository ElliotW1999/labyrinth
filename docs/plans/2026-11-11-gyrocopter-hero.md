# Gyrocopter hero: implementation spec

Status: **approved (2026-11-11)** — U1 10 s, U2 literal 14.285714 u/s², U4 8/8/9; all other defaults accepted. Build only gaps G1–G4. If implementation shows that anything else is needed, stop and ask.

Related plans and docs:
- `docs/plans/2026-11-11-shadowheart-hero.md`: the template for this plan. It covers the scratch-CSV HeroGenerator flow, D2 half-step ranks, the area polarity rule and the allowed-generated-diff discipline.
- `docs/plans/2026-11-10-winter-wyvern-and-attribute-damage.md`: Variant B with P1 (primary-attribute damage) and A1 (flat 6.321839 MR), the `require_status` gate with its `:suffix` syntax, and `OWNER_ID_STATUSES`.
- `docs/plans/2026-11-10-slark-hero.md`: `attached_zone`, and the `base_attack_speed` column (G6).
- `docs/plans/2026-11-09-sniper-hero.md`: `zone` (delay, interval, duration), `mini_stun` (`<owner>_stun`), `<owner>_slow`, and the precedent for appending ability CSV columns (`cast_range_buffer`).
- `docs/plans/2026-11-09-ursa-hero.md`: `attack_charges` and `consume_attack_charges`.
- `docs/plans/2026-11-08-attack-modifiers.md`: `perform_attack` and the multishot recipe.
- `docs/architecture/composition.md`: §3 frame order, §5 spell landing, §6.13–6.17, §7 checklist.

---

## 1. Current state (verified)

Nothing named Gyrocopter exists yet. No `RocketBarrage`, `HomingMissile`, `FlakCannon` or `CallDown` appears in `src/`, `data/`, `scripts/` or `docs/`, and there is no `pick_random` keyword.

**Reusable as is:**
- **Hero columns.** Every value fits:
  - Ranged 365 with `attack_projectile_speed` 3000.
  - BAT 1.7, swings 0.2 / 0.1, `turn_rate` 0.6.
  - `base_attack_speed` 115 (the Slark G6 column).
  - AGI primary (P1); MR 6.321839 (A1).
- **Rocket Barrage pulses around the caster.** `attached_zone` (`AbilityEffect::Zone { follow_caster: true }`, `abilities/zones.rs`). With interval 0.1 and duration 3 it gives 30 ticks. Each tick's `affected_units` are the living, targetable enemies inside the radius, and a tick fires even when nobody is inside.
- **Call Down, a delayed area hit.** A static `zone` with delay 2, interval 1 and duration 1 gives exactly one tick at t = 2.0 s, centred on the aim (`on_cast` trigger point). It uses the ability's `aoe_radius` and has a Nova ring at the tick.
- **Slow and stun.**
  - `apply_status,slow` is measured against base move speed and uses the id `<owner>_slow` (Sniper G5/G6).
  - `apply_status,mini_stun` uses `<owner>_stun`.
- **Homing spell projectile.**
  - The mechanic is `projectile_type=targeted` → `AbilityMechanic::Projectile` (`mechanics.rs:204`) → `SpawnProjectileEvent` → `spawn_spell_bolt` (`combat.rs:1075`) → `fly_projectiles` (`combat.rs:609`) → `apply_projectile_hits` (`combat.rs:816`).
  - On a hit, `OnProjectileHit` fires with `trigger_unit` set to the struck unit (`mechanics.rs:523`).
- **Attack charges.** These are `StatusEffect.attack_charges` (`status.rs:262`). `consume_attack_charges` spends one per non-extra release (`status.rs:1684`), and the status is removed at 0 in `apply_status_effect_events` (`status.rs:1861`).
  - **Ordering:**
    1. `attack_triggers` writes `OnAttack`.
    2. `consume_attack_charges` writes the event.
    3. `resolve_ability_triggers` runs the gates while the status is still held.
    4. `apply_status_effect_events` removes the status (`abilities/mod.rs:117-145`).
  - So the last charged attack still passes a `require_status` gate.
- **Gate on a held status.** `require_status,on_x:<status_id>` (`effects.rs:994`). The derived context keeps `trigger_unit` and `attack`.
- **Split attacks.** `perform_attack` (`effects.rs:938`) works like this:
  - It ranks enemy candidates by distance from `ctx.origin` (the attacker) and supports `exclude_trigger_unit`.
  - Extra attacks never chain: an `extra` context performs nothing, and extra releases spend no charges.
  - `release_extra_attacks` (`basic_attack.rs:352`) has no range check, so 1250-unit targets are valid.

**Gaps:**
- **G1. `perform_attack` limits.**
  - `max_targets` is a single `u32`, so it cannot change per rank (`definition.rs:1161`, `generator_common.py:2308`).
  - A `units_in_radius` row under `on_attack` or a gate it feeds is centred on `TriggerPoint`, which is the primary target's position (`attack_triggers.rs:58`, `generator_common.py:2375`). Flak Cannon's search radius is around Gyrocopter.
- **G2. No status that only carries attack charges.** Charges ride only on `stat_modifier` rows (`generator_common.py:2068-2073`). Flak Cannon has no stat to modify.
- **G3. No random selection.** Nothing picks one random unit out of a set: there is no `random` target or gate. `CombatRng` exposes only `roll` (`combat.rs:298`).
- **G4. Homing projectiles cannot do three things the missile needs:**
  - There is no launch delay.
  - Speed is fixed (`Projectile.speed`).
  - On target death the projectile flies to the last known position and ends as `OnImpact` (`combat.rs:627-636`).
  - Also, `Lifetime` = `max(dist/speed + 0.25, 2.5)` (`combat.rs:1092`). This would kill a delayed, slow (500) missile that is chasing a target.

## 2. Scope and ranks (D2: half steps for unspecified ranks)

| Ability | Field | Ranks |
|---|---|---|
| Q | damage per rocket (magical) | 8, 14, 20, 26, 29, 32, 35 |
| Q | cooldown | 7, 6.5, 6, 5.5, 5.25, 5, 4.75 |
| Q | constants | mana 75; radius 400; cast 0/0; 10 rockets/s (interval 0.1) for 3 s = 30 rockets |
| W | damage (magical) | 90, 180, 270, 360, 405, 450, 495 |
| W | stun | 1.25, 1.5, 1.75, 2, 2.125, 2.25, 2.375 |
| W | cooldown | 26, 25, 24, 23, 22.5, 22, 21.5 |
| W | mana | 90, 100, 110, 120, 125, 130, 135 |
| W | constants | range 1050; cast 0/0; speed 500; delay 0.5 s; acceleration 1/0.07 = 14.285714 u/s² (U2); retarget radius 700 |
| E | attacks (charges) | 4, 5, 6, 7, **8, 8, 9** (U4) |
| E | additional targets | 4, 5, 6, 7, **8, 8, 9** (U4) |
| E | constants | cd 25; mana 50; search radius 1250; cast 0/0; **duration 10 s (U1)** |
| R | damage (magical) | 200, 350, 500, **575** |
| R | cooldown | 75, 65, 55, **50** |
| R | mana | 150, 200, 250, **275** |
| R | constants | range 1000; radius 400; cast 0.3 / 0.5; delay 2 s; slow 60% for 4 s |

## 3. Mapping

| Spec item | Surface | Fidelity |
|---|---|---|
| Hero stats | Variant B, P1 and A1 (§8) | Exact |
| Q 10 rockets/s around Gyro for 3 s | `attached_zone` (interval 0.1, duration 3) | Exact |
| Q each rocket hits one random enemy in the radius | `pick_random` gate on `affected_units` → `trigger_unit,damage` | **G3** |
| W unit-target missile: dmg + stun on reach | `targeted` projectile, `on_projectile_hit,trigger_unit` damage + `mini_stun` | Exact |
| W 0.5 s delay, 500 speed with acceleration, retarget within 700 | new projectile columns | **G4** |
| E limited attacks, ends early when spent | `apply_status,attack_charges` | **G2** |
| E each attack also hits N extra enemies within 1250 of Gyro | `on_attack` `require_status` → `units_in_radius,perform_attack,exclude_trigger_unit` | **G1** |
| R delayed area damage + slow | `zone` (delay 2, one tick) → `affected_units` damage + `slow` | Exact |

---

## 4. Gap proposals

### G1. `perform_attack`: per-rank target cap, area centred on the attacker (generator + one Rust type)
- **Rust.**
  - `AbilityEffect::PerformAttack.max_targets` becomes a `RankValue` (ZERO = all). The runner uses `max_targets.at(rank).round() as usize`.
  - Hand-written sites to update: `abilities/mod.rs:1639` (the helper `perform_attack(n, ..)`), `:1835`, and `tooltip.rs:589`.
  - There are no generated or item-catalog `PerformAttack` sites (verified), so there is no generated churn.
- **Generator** (`generator_common.py:2297-2316`):
  - `amount` accepts a rank list of whole numbers ≥ 0. Emit `rank_value(amount)`, or `RankValue::ZERO`.
  - **Centre rule.** In the `units_in_radius` branch (`:2375`), the centre is `"Caster" if t == "perform_attack" else ctx.center_for.get(...)`.
  - **Rationale.**
    - Extra attacks are launched from the attacker, and the runner already ranks candidates by distance from the attacker.
    - The hand-written multishot test already uses `AreaCenter::Caster` (`abilities/mod.rs:1716`).
  - **Impact.** No shipped data is affected. The example `split_bow` (`data/examples/item_effects.csv:30`) now centres on its owner. The examples are validated only, never generated into `src/`.
- **Rejected alternatives.**
  - A new `units_around_caster` target keyword: it adds a keyword to every target check, the same objection as in the Shadowheart plan.
  - Centring on the primary target, which needs zero code but is inexact (see U5).
- **Tests.**
  - Rust: `perform_attack_max_targets_scales_with_rank`; `multishot_excludes_primary_and_caps_targets` is unchanged.
  - Generator: `PerformAttackTests`. It covers a rank-list amount, a non-integer amount rejected, and `units_in_radius` + `perform_attack` emitting `AreaCenter::Caster` under `on_attack` and under a gate-fed custom trigger.

### G2. `attack_charges` status (Gameplay rules; abilities and items)
- **CSV.** `apply_status,attack_charges`, with `amount` = charges (a rank list of whole numbers ≥ 1) and `duration` required.
  - The id is `<owner>_attack_charges`. Add it to `OWNER_ID_STATUSES` and to `STATUSES` as a Buff.
  - `remove_status,attack_charges` is rejected with the owner-id hint.
  - It is rejected on `passive`, `while` and `while_toggled` rows.
- **Rust.** New `StatusSpec::AttackCharges { id, charges: RankValue }` builds `StatusEffect { attack_charges: charges.at(rank).round().max(1) as u32, ..StatusEffect::buff(id) }`.
  - There is no new runtime rule. Consumption, removal at 0, the `Expired` lifecycle and recast refresh all reuse the Ursa path.
- **Tooltip.** "Attacks: N" and "Duration".
- **Tests (`status.rs` `mod attack_charges`).**
  - `charges_only_status_is_removed_after_n_releases`
  - `charges_only_status_expires_on_duration_with_charges_left`
  - `extra_attacks_do_not_spend_charges_only_status`
- **Generator.** `AttackChargesStatusTests`.
- **Invalid examples.** `amount` 0; `amount` 1.5; a `passive` row.

### G3. `pick_random` gate (Triggers layer; abilities and items)
- **CSV.** `effect_type=pick_random`.
  - `effect_id` = the custom trigger `on_<name>`.
  - `effect_target` = the candidate pool. Only `affected_units` or `units_in_radius` are allowed.
  - `value` = number of distinct picks: a single integer ≥ 1; blank means 1.
  - Errors are those of `chain`: a built-in trigger, firing its own trigger, or a trigger with no consumer.
  - Add it to `EFFECT_TYPES` and `CUSTOM_TRIGGER_FEEDERS`. It is **not** in `GATE_TYPES`, because those restrict targets to `caster`/`owner`/`attacker`.
  - `fed_trigger` and `chain_fed_triggers` already follow `CUSTOM_TRIGGER_FEEDERS`.
- **Rust.**
  - `AbilityEffect::PickRandom { count: u32, then: AbilityTrigger }`, added to the `acts_on_targets` exclusions.
  - New `CombatRng::index(len) -> usize`, built as `(next_f32() * len) as usize` clamped to `len - 1`.
  - In `EffectRunner`, next to `Chain`:
    1. If `gate_open(ctx)` passes, take the resolved targets that are still alive.
    2. Draw `min(count, len)` distinct indices with a partial Fisher–Yates on `self.rng`.
    3. For each pick, push `ctx.derived(then).trigger_unit(Some(u)).at(pos).affected(Vec::new())`.
  - The RNG draw order is fixed by message order, so the result is deterministic for a seeded `CombatRng`.
  - **Depth.** The zone tick runs at depth 1, so the gate is open and the rows under it run at depth 2. That is within `MAX_TRIGGER_DEPTH` 2 (`effects.rs:48`).
- **Tooltip.** "Strikes N random target(s)".
- **Tests (`abilities/mod.rs`).**
  - `pick_random_fires_once_per_tick_on_one_pool_member`
  - `pick_random_with_empty_pool_fires_nothing`
  - `pick_random_count_caps_at_pool_and_is_distinct`
  - `pick_random_is_deterministic_for_a_seed`
  - `pick_random_respects_depth_cap`
  - `item_and_ability_pick_random_alike`
- **Generator.** `PickRandomTests`.
- **Invalid examples.** `value` 0; target `caster`; a missing consumer; firing its own trigger.

### G4. Seeking projectiles: delay, acceleration and retarget (Mechanics layer; abilities, with items as a follow-up)
- **CSV.** Three optional columns are appended to the `abilities.csv` headers in `data/`, `data/examples/` and `data/examples/invalid/`. Existing rows are padded with 3 commas.
  - `projectile_delay`: seconds ≥ 0.
  - `projectile_acceleration`: u/s², ≥ 0.
  - `projectile_retarget_radius`: > 0.
  - Each takes a single constant, and all three are valid only with `projectile_type=targeted`. Setting any of them emits the seek block.
  - Rejected alternative: one packed `a:b:c` column. It is harder to validate and to read.
- **Rust (`components.rs`).**
  - `#[derive(Clone, Copy, Debug, PartialEq, Default)] pub struct ProjectileSeek { pub delay: f32, pub acceleration: f32, pub retarget_radius: f32 }`, used both as data and as a component. Hold the remaining delay in a separate `SeekDelay(f32)` component, or mutate `delay` in place.
  - `AbilityMechanic::Projectile` gains `seek: Option<ProjectileSeek>`, and `SpawnProjectileEvent` gains `seek: Option<ProjectileSeek>`.
  - `mechanics.rs:228` passes the mechanic's value, and `projectile_to_each` passes `None`. Update the other `SpawnProjectileEvent {` construction sites (4 in total) and the hand-written `AbilityMechanic::Projectile {` constructors in `catalog.rs` and the `abilities/mod.rs` tests.
- **Rust (`spawn_spell_bolt`).**
  - When `seek` is `Some` and a `target` exists, insert the `ProjectileSeek` component.
  - Set `Projectile.lifetime` and `Lifetime` to `SEEKING_PROJECTILE_MAX_LIFETIME` (15 s, a new const in `combat.rs`).
- **Rust (`fly_projectiles`).** This is the only place seeking rules live.
  - `Projectile` becomes `&mut`. Add `Option<&mut ProjectileSeek>` and `&mut ProjectilePayload`, plus a read-only units query: `(Entity, &GlobalTransform, &Team, &Health, Option<&BoundRadius>)` and `statuses_of`.
  1. **Retarget**, checked every frame including during the delay:
     - If the seeking projectile's home target is missing or `!health.is_alive()`, and `retarget_radius > 0`, pick the nearest unit that is alive, `team != projectile.team`, and not `is_untargetable`, within `retarget_radius` (centre-to-edge via `area_contains`) of the projectile's position. Ties break on `Entity`.
     - If one is found, set `home.target` and `home.last_pos` to it, and set `payload.cast_target = Some(new)` (see U6).
     - If none is found, fall through to the existing last-known-position behaviour, which ends as `OnImpact`.
  2. **Delay.** While `delay > 0`, subtract `dt`, face the target, and do not move.
  3. **Acceleration.** After the delay, `projectile.speed += acceleration * dt` before computing the step.
  - Non-seeking projectiles are unchanged.
- **Generator.**
  - `projectile_mechanic_rust` gains `seek`, emitting `seek: None` or `Some(ProjectileSeek { .. })`.
  - `ItemGenerator` always emits `None`; item columns are a follow-up.
  - Add `LEVEL_FIELDS` exclusions (the columns are constants).
- **Allowed generated diff.** `seek: None` on the 6 existing generated `AbilityMechanic::Projectile` sites.
- **Tooltip.** "Missile: waits Ds, then seeks at S (+A/s²); retargets within R".
- **Tests (`combat.rs` `mod seeking`, using `run_system_once` like the existing projectile tests).**
  - `seeking_projectile_holds_position_during_delay`
  - `seeking_projectile_accelerates_after_delay`: speed = 500 + a·t, within float tolerance.
  - `seeking_projectile_retargets_nearest_enemy_within_radius_on_target_death`
  - `seeking_projectile_ignores_allies_untargetable_and_out_of_radius_on_retarget`
  - `seeking_projectile_without_candidate_ends_as_impact`
  - `seeking_projectile_survives_beyond_default_lifetime`: a 3 s chase still hits.
  - `non_seeking_projectile_unchanged` (regression)
- **Generator.** `ProjectileSeekTests`.
- **Invalid examples.** A seek column on `none`/`skillshot`; a negative delay; retarget radius 0.

---

## 5. Steps (one commit each; `cargo fmt`, `cargo build` and `cargo test` green between them)

1. **G1.** Generated `src/` is unchanged; the example `split_bow` area centre changes.
2. **G2.**
3. **G3.**
4. **G4.** Header columns plus padding. Regenerate: the only diff is `seek: None` on 6 sites.
5. **Data.**
   - Add the rows of §6–§8 to `data/` and `data/examples/`, plus the invalid rows.
   - Run HeroGenerator on a scratch CSV (header plus the Gyrocopter row) in the session scratchpad. Run `--dry-run` first and expect `RocketBarrage/HomingMissile/FlakCannon/CallDown`, then the real run.
   - Run AbilityGenerator: 4 additions, ranks 7/7/7/4. Then `cargo fmt`.
6. **Docs.**
   - README:
     - The hero row (drop "(stubs …)").
     - The new keywords `pick_random` and `attack_charges`.
     - `perform_attack` (rank-list cap, centred on the attacker).
     - The three projectile columns.
   - `composition.md`:
     - §1 mechanics row: seeking projectiles and the `PickRandom` gate.
     - §3: seeking runs in `fly_projectiles`.
     - §5 spell landing: a retargeted missile lands on its new target.
     - A new §6.18 Gyrocopter.
   - Append implementation notes to this plan.

## 6. `data/abilities.csv` (+ examples). The header gains `,projectile_delay,projectile_acceleration,projectile_retarget_radius`

```csv
rocket_barrage,Rocket Barrage,Q,active,no_target,enemy,,400,0,,7|6.5|6|5.5|5.25|5|4.75,75,,,none,,"Gyrocopter fires a barrage of rockets for 3 seconds; each rocket strikes a random nearby enemy for magical damage",0,,,,,,,,
homing_missile,Homing Missile,W,active,unit,enemy,1050,,0,,26|25|24|23|22.5|22|21.5,90|100|110|120|125|130|135,,,targeted,500,"Launches a missile that waits briefly, then accelerates toward the target, dealing magical damage and stunning it; if the target dies first, the missile seeks the nearest enemy",0,,,,,,0.5,14.285714,700
flak_cannon,Flak Cannon,E,active,no_target,self,,,0,,25,50,,,none,,"For a limited number of attacks, each of Gyrocopter's attacks also strikes additional nearby enemies",0,,,,,,,,
call_down,Call Down,R,active,point,enemy,1000,400,0.3,,75|65|55|50,150|200|250|275,,,none,,"After a 2 second delay, missiles strike the target area, damaging and slowing enemies",0.5,,,,,,,,
```

## 7. `data/ability_effects.csv` (+ examples)

```csv
rocket_barrage,0,on_cast,caster,attached_zone,on_rocket_barrage_tick,0.1,,3,,,,
rocket_barrage,1,on_rocket_barrage_tick,affected_units,pick_random,on_rocket_barrage_hit,1,,,,,,
rocket_barrage,2,on_rocket_barrage_hit,trigger_unit,damage,,8|14|20|26|29|32|35,magical,,,,,
homing_missile,0,on_projectile_hit,trigger_unit,damage,,90|180|270|360|405|450|495,magical,,,,,
homing_missile,1,on_projectile_hit,trigger_unit,apply_status,mini_stun,,,1.25|1.5|1.75|2|2.125|2.25|2.375,,,,
flak_cannon,0,on_cast,caster,apply_status,attack_charges,,,10,,4|5|6|7|8|8|9,,
flak_cannon,1,on_attack,caster,require_status,on_flak_cannon_attack:flak_cannon_attack_charges,,,,,,,
flak_cannon,2,on_flak_cannon_attack,units_in_radius,perform_attack,exclude_trigger_unit,100,,,1250,4|5|6|7|8|8|9,,
call_down,0,on_cast,caster,zone,on_call_down_impact,1,,1,,2,,
call_down,1,on_call_down_impact,affected_units,damage,,200|350|500|575,magical,,,,,
call_down,2,on_call_down_impact,affected_units,apply_status,slow,,,4,,60,,
```

- **Status ids.** `homing_missile_stun`, `flak_cannon_attack_charges` (also the gate key), `call_down_slow`.
- **Triggers.** `on_rocket_barrage_tick`, `on_rocket_barrage_hit`, `on_flak_cannon_attack`, `on_call_down_impact`.
- **Rocket Barrage.** Ticks fire at 0, 0.1, …, 2.9 s and follow Gyro. Each one hits one random enemy within 400 of his current position.
- **Flak Cannon.**
  - Each non-extra release passes the gate and requests up to N extra 100% attacks on the nearest enemies within 1250 of Gyro, excluding the primary. It also spends one charge.
  - Extra attacks pass the gate too, but `PerformAttack` refuses `extra` contexts, so there is no chaining.
- **Call Down.** It is a single tick at 2.0 s centred on the aim.

## 8. `data/heroes.csv` and `data/heroes.example.csv` (Variant B + P1 + A1, plus the G6 column)

```csv
Gyrocopter,FALSE,Rocket Barrage,Homing Missile,Flak Cannon,Call Down,22,25,21,2.5,3.4,2.4,AGI,"Agi gunship — Rocket Barrage, Homing Missile, Flak Cannon, Call Down",208,75,21,365,315,-2.5,6.321839,-0.19,0,1.7,0.2,0.1,3000,0.6,115
```
In `heroes.example.csv`, use `false`.

| Field | Formula | Value |
|---|---|---|
| base_health | 604 − 22×18 | 208 |
| base_health_regen | 2.45 − 22×0.12 | −0.19 |
| base_mana | 327 − 21×12 | 75 |
| base_mana_regen | 1.05 − 21×0.05 | 0 |
| armor | 1 − 25×0.14 | −2.5 |
| magic_resist | flat (A1) | 6.321839 (27.5%) |
| attack_damage | 46 − 25 (AGI primary) | 21 |
| attack_projectile_speed | absolute | 3000 |
| base_attack_speed | G6 column | 115 → APS (115 + 25) / 100 / 1.7 = 0.823529 |

## 9. Tests (beyond the per-gap tests)

- **`heroes.rs::gyrocopter_matches_spec`.**
  - Identity: `from_cli("gyrocopter")`, "AGI", abilities `[RocketBarrage, HomingMissile, FlakCannon, CallDown]`.
  - Level 1: HP 604, regen 2.45, mana 327, regen 1.05, armor ≈ 1, magical 100 → 72.5, damage 46.
  - Attributes: (22, 25, 21) / (2.5, 3.4, 2.4).
  - Combat: range 365, ranged, projectile 3000, BAT 1.7, swings 0.2 / 0.1, MS 315, turn 0.6, `base_attack_speed` 115, APS 0.823529.
  - Add `(HeroId::Gyrocopter, 46.0)` to `every_hero_keeps_level_one_attack_damage_parity`.
- **`catalog.rs::gyrocopter_abilities_generate_expected_definitions`.**
  - Q: NoTarget/Enemy, aoe 400, `Zone { follow_caster: true }` with interval 0.1 and duration 3, `PickRandom { count: 1 }`, damage at(7) = 35, cd at(5) = 5.25.
  - W: Unit, range 1050, `Projectile { speed: Some(500), seek: Some(ProjectileSeek { delay: 0.5, acceleration ≈ 14.285714, retarget_radius: 700 }) }`, `OwnedStun` duration at(7) = 2.375, mana at(7) = 135.
  - E: `AttackCharges` at(4) = 7 and at(7) = 9, a `RequireStatus` on `flak_cannon_attack_charges`, `PerformAttack { max_targets at(7) = 9, exclude_trigger_unit }` on `UnitsInRadius { center: Caster, radius: Fixed(1250), team: Enemy }`.
  - R: Area, max rank 4, cast 0.3 / backswing 0.5, `Zone` with delay 2, damage at(4) = 575, `Slow` 60 with id `call_down_slow`, cd at(4) = 50.
- **Integration (`abilities/mod.rs`; use `qop_app()` where expiry matters).**
  - `rocket_barrage_fires_thirty_rockets_over_three_seconds_on_random_enemies`: with two enemies in range, total hits = 30 and each enemy is hit at least once (fixed seed). An enemy at 500 is untouched.
  - `rocket_barrage_follows_gyrocopter`
  - `rocket_barrage_with_no_enemies_does_nothing`
  - `homing_missile_waits_then_hits_damages_and_stuns`: no movement before 0.5 s; the stun id is `homing_missile_stun`.
  - `homing_missile_retargets_when_target_dies`: the new target takes damage and the stun, and the spell-landing rules apply to it.
  - `flak_cannon_splits_each_attack_to_n_nearest_enemies_around_gyro`: an enemy 1200 from Gyro but 1500 from the primary is included.
  - `flak_cannon_ends_after_n_attacks`: the Nth attack splits, the N+1th does not.
  - `flak_cannon_ends_after_duration_with_charges_left`
  - `flak_cannon_extra_attacks_do_not_split_again_or_spend_charges`
  - `call_down_hits_and_slows_after_two_seconds_only`: nothing at 1.9 s; at 2.0 s an enemy inside takes 200 pre-mitigation magical damage and is slowed 60% for 4 s; an enemy outside is untouched.
- **`scripts/test_generator_common.py`.**
  - `PerformAttackTests`, `AttackChargesStatusTests`, `PickRandomTests`, `ProjectileSeekTests`, and a `HeroGeneratorTests` row for Gyrocopter.
  - `ExampleCatalogTests` must still pass.

Verification:
```bash
cargo fmt --check && cargo build && cargo test
python3 scripts/AbilityGenerator.py --check
python3 scripts/ItemGenerator.py --check
python3 scripts/AbilityGenerator.py data/examples/abilities.csv data/examples/ability_effects.csv --check
python3 scripts/ItemGenerator.py data/examples/items.csv data/examples/item_effects.csv --check
(cd scripts && python3 -m unittest test_generator_common)
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
git diff --stat src/abilities/generated.rs src/item_catalog.rs   # only the allowed diffs
```
Environment: install the Bevy system libraries with apt if they are missing (as for earlier heroes). This changes no repository files.

## 10. Invariants and risks

- **Where each rule lives.**
  - Seeking (delay, acceleration, retarget): only in `fly_projectiles`. Hit detection stays in `apply_projectile_hits`.
  - Random selection: only in the `PickRandom` arm of `EffectRunner`, which emits only derived trigger contexts.
  - Charge consumption: still only in `status.rs`.
  - The split-attack centre: a generator rule only. The runtime is unchanged.
- **No duplicate triggers.**
  - One `pick_random` gives one derived context per pick per tick.
  - Extra attacks never perform attacks, so Flak cannot chain.
  - A retargeted missile still produces exactly one `ProjectileHitEvent`.
- **Ordering.**
  - The last charged attack splits, because the gate runs before `apply_status_effect_events` removes the status.
  - A target that dies in frame N (`apply_damage_events`) is retargeted in `fly_projectiles` in frame N+1.
  - Zones spawned in frame N start ticking in N+1.
- **Allowed generated diffs.**
  - `seek: None` on the 6 generated `Projectile` sites.
  - The 4 Gyrocopter ability blocks, and the Gyrocopter hero arms and regions.
  - The examples change only `split_bow`'s centre (validation output).
- **Behaviour changes for existing content.**
  - In shipped data: none.
  - `PerformAttack.max_targets` changes type.
  - Seeking-projectile lifetime applies only to projectiles that have a `seek` block.
- **Performance.**
  - Rocket Barrage gives 10 gate evaluations per second per caster.
  - The retarget scan runs only while a seeking projectile's target is dead or missing.
- **Not replicated.** Projectiles and `StatusEffects` exist on the sim authority only. This limitation already exists.

## 11. Naming

| CSV id | `AbilityId` | Display |
|---|---|---|
| `rocket_barrage` | `RocketBarrage` | Rocket Barrage |
| `homing_missile` | `HomingMissile` | Homing Missile |
| `flak_cannon` | `FlakCannon` | Flak Cannon |
| `call_down` | `CallDown` | Call Down |

## 12. Open decisions (each has a recommended default)

- **G1–G4.** Approval. All four are required for the kit as specified.
- **U1. Flak Cannon duration** (not in the spec).
  - Recommended: **10 s at every rank**, from memory of Dota's value, unverified. Cooldown 25 > 10, so a buff never overlaps itself; a recast refreshes the charges.
  - Alternative: 20 s, matching Overpower's charged buff in this repo.
- **U2. Acceleration** "+1 per 0.07 s" is read literally as a continuous 14.285714 u/s².
  - That is only about +30 speed over a typical 2 s flight, so the missile is effectively about 500 u/s.
  - If a much stronger ramp was meant (for example +1 *per frame-tick* scaled differently, or Dota-like values), only the CSV cell changes.
- **U3. Retarget.**
  - Search centre: the **missile's position**. Alternative: the dead target's last position.
  - Pool: any living, targetable enemy unit (heroes, creeps and towers alike).
  - Retarget happens on **death or despawn only**. A cycloned or banished (untargetable) target still makes the missile miss, as today.
  - Retargeting can happen any number of times.
- **U4. Integer half-steps for Flak.** Charges and extra targets for ranks 5–7 round the half step up: **8, 8, 9**. Alternatives: round down (7, 8, 8), or full steps (8, 9, 10).
- **U5. Flak radius centre.** Centred on Gyrocopter (G1). The zero-code alternative is centring on the primary target (TriggerPoint); it differs by up to ~400 at the edge.
- **U6. Spell landing on retarget.** The payload's `cast_target` follows the new target. A retargeted missile can therefore be spell-blocked, blocked by spell immunity, or reflected by the new target, and `cast_target` rows act on it. The alternative of keeping the original `cast_target` would let the new target bypass spell block and immunity.
- **U7. Delay and lifetime.**
  - During its 0.5 s delay the missile sits at the launch point (Gyro's position plus body height) facing the target. It does not follow Gyro.
  - Seeking missiles live up to 15 s, then fizzle without a trigger.
  - Dota's destroyable missile and its pre-flight vision are not built.
- **U8. Flak targets.**
  - Extra attacks use 100% damage and fire `on_attack_hit` (`extra: true`), so on-hit procs apply. Precedent: extra attacks are excluded only from charge spending and chaining.
  - Buildings in range are valid extra targets: there is no building filter in `PerformAttack`. Dota excludes buildings; adding that is out of scope unless requested.
  - Charges are spent on every non-extra release, including attacks on buildings.
- **U9. Rocket Barrage.**
  - Rockets are instant hits (no projectile travel).
  - The 30 rockets fire even with no enemy in range (wasted).
  - Towers inside 400 are valid targets under the existing zone rules.
  - The stun keyword for Homing Missile is `mini_stun` (owner id), not `stun`.
- **U10. Call Down.** One impact at 2 s, not Dota's two missiles. The area is fixed at the cast point, and the slow is `<owner>_slow` (60%, 4 s), refreshed by a recast.
- **U11. Ranks.** D2 half steps: Q dmg 29/32/35, cd 5.25/5/4.75; W dmg 405/450/495, stun 2.125/2.25/2.375, cd 22.5/22/21.5, mana 125/130/135; R rank 4: 575 damage, 50 cd, 275 mana.
- **U12.** The three projectile columns are abilities-only for now. Item support is a follow-up, as was done for `cast_range_buffer`.

---

## 13. Implementation notes (2026-11-11)

Built exactly G1-G4 plus the data, in the §5 order, one commit per step. Defaults applied: U1 Flak Cannon 10 s, U2 acceleration 14.285714 u/s^2, U4 Flak ranks 4/5/6/7/8/8/9; every other open decision used its recommended default.

### What was built, and where

| Step | Surface | Where |
|---|---|---|
| G1 | `PerformAttack.max_targets: RankValue`; generator `amount` rank list of whole numbers, `perform_attack` area always `AreaCenter::Caster` | `definition.rs`, `effects.rs`, `tooltip.rs`, `generator_common.py` |
| G2 | `StatusSpec::AttackCharges { id, charges }`, `apply_status,attack_charges` (id `<owner>_attack_charges`), tooltip rows | `definition.rs`, `tooltip.rs`, `generator_common.py` (no `status.rs` rule change) |
| G3 | `AbilityEffect::PickRandom { count, then }`, `CombatRng::index`, `pick_random` effect type (feeder, not a `GATE_TYPES` member) | `definition.rs`, `effects.rs`, `combat.rs`, `tooltip.rs`, `generator_common.py` |
| G4 | `components::ProjectileSeek`, `AbilityMechanic::Projectile.seek`, `SpawnProjectileEvent.seek`, retarget / delay / acceleration in `fly_projectiles`, `SEEKING_PROJECTILE_MAX_LIFETIME` (15 s), three ability columns | `components.rs`, `definition.rs`, `mechanics.rs`, `combat.rs`, `tooltip.rs`, `generator_common.py`, `AbilityGenerator.py` |
| Data | Gyrocopter hero (HeroGenerator on a scratch CSV, `--dry-run` first), 4 abilities (AbilityGenerator: 4 additions, ranks 7/7/7/4) | `data/`, `data/examples/`, `heroes.rs`, `components.rs`, `generated.rs` |
| Docs | README, `composition.md` (§1, §2, §3, §5 spell landing, new §6.18) | |

### Deviations and judgement calls

1. **`apply_projectile_hits` got a one-line guard** (not in §4 G4, which keeps seeking only in `fly_projectiles`): a seeking missile whose `delay` is still above 0 is skipped. Without it, a target already inside the contact radius at launch would be struck during the delay, so "waits 0.5 s" would not hold. Hit detection is otherwise unchanged. Test: `seeking_projectile_cannot_hit_during_delay`.
2. **A dead-but-not-despawned target is abandoned at once when nothing can replace it.** The pre-existing code only ended at the last known position when the target entity was gone, so a corpse that lingered a frame would be chased. For seeking missiles with no candidate, `fly_projectiles` now converts to the last-known-position ending immediately (`abandon`). Non-seeking projectiles are untouched (`non_seeking_projectile_unchanged`).
3. **"Untargetable" for retargeting means `is_untargetable`** (cyclone, banish), as in chain, zone and skillshot contact. The targeting-only `untargetable` status (Shadow Dance) does not block a retarget, consistent with "in-flight projectiles still reach it". The test uses `StatusEffect::cyclone`.
4. **The delay frame does not carry over leftover time**: the frame in which the delay runs out still does not move. At 20 Hz this is at most one frame (0.05 s).
5. **Seek columns on items are an error, not a silent ignore** (U12 says abilities only). `ItemGenerator` always emits `seek: None`; the existing item projectile test string was updated for it. `item_catalog.rs` has no projectile items, so there is no generated item diff.
6. **`seek` is dropped for skillshots at the mechanic** (`seek.filter(|_| !skillshot)`) and in `spawn_spell_bolt` for projectiles without a target, so the component only ever rides on a homing missile. The generator already rejects the columns for non-`targeted` types.
7. **`fly_projectiles` and `tick_lifetimes` became `pub(crate)`** so the Gyrocopter integration tests can run the real projectile pipeline (`gyro_app`).
8. **Tooltips** (not specified beyond the G-sections): "Missile: waits Ds, then seeks at S (+A/s^2); retargets within R", "Strikes N random target(s)", and "Attacks" / "Duration" rows for the charges status.
9. **Invalid examples.** The `value 0` pick_random row in `data/examples/invalid/ability_effects.csv` is also reported for its missing consumer; `PickRandomTests` isolates each case. The attack_charges, seek and pick_random invalid rows are in `data/examples/invalid/`.
10. **Generated `components.rs` is unformatted by the generators**; `cargo fmt` was run after every generator run, so the committed diff is only the new variants.
11. **README hero row**: HeroGenerator writes "(stubs - implement later)"; it was removed by hand in the docs step as planned.

### Behaviour notes for reviewers

- A retargeted missile rewrites `ProjectilePayload.cast_target` (U6), so spell block, spell immunity and reflection judge the new target. Test: `homing_missile_retarget_obeys_spell_landing_rules`.
- Rocket Barrage is `zone (depth 1) -> pick_random (depth 1) -> damage (depth 2)`, within `MAX_TRIGGER_DEPTH`. Test: `pick_random_respects_depth_cap` and the 30-rocket integration test.
- Flak Cannon counts a charge on every non-extra release, including attacks on buildings; buildings are valid extra targets (U8).
- Not replicated: projectiles and `StatusEffects` exist on the sim authority only (existing limitation).

### Verification (all run on this branch after the last code change)

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo build` | ok (existing dead-code warnings only) |
| `cargo test` | 871 passed, 0 failed |
| `python3 scripts/AbilityGenerator.py --check` | validated 38 abilities, 98 effect rows |
| `python3 scripts/ItemGenerator.py --check` | validated 60 items, 85 effect rows |
| `AbilityGenerator.py data/examples/... --check` | validated 51 abilities, 124 effect rows |
| `ItemGenerator.py data/examples/... --check` | validated 28 items, 56 effect rows |
| `python3 -m unittest test_generator_common` (in `scripts/`) | 250 tests, OK |
| `HeroGenerator.py data/heroes.example.csv --dry-run` | Gyrocopter already present: "Nothing to add" |
| `git diff --stat src/abilities/generated.rs src/item_catalog.rs` | `generated.rs` only: `seek: None` on the 6 projectile sites, the `ProjectileSeek` import and the 4 Gyrocopter blocks; `item_catalog.rs` unchanged |

Remaining limitations: Dota's destroyable missile, its pre-flight vision, a second Call Down missile and the building filter on Flak Cannon are not built (U7, U8, U10). Seek columns are abilities-only (U12).
