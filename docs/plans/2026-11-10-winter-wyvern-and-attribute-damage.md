# Winter Wyvern, primary-attribute damage and uniform magic resistance: implementation spec

Status: **approved 2026-11-10 with amendments (see §13); implemented (see §14).** Build nothing beyond the listed gaps (P1, P2, G1–G7). If implementation shows that something else is needed, stop and ask.

Related plans and docs:
- `docs/plans/2026-11-10-slark-hero.md`: the model for this spec. It covers the scratch-CSV HeroGenerator flow, the precedent for adding a field to every `StatusSpec::Modifier` site (G2 `independent`), the `:suffix` effect_id syntax, `OWNER_ID_STATUSES`, and the `base_attack_speed` column (G6).
- `docs/plans/2026-11-09-ursa-hero.md`: the rank 5–7 extrapolation (U2) and the Variant B back-solve.
- `docs/plans/2026-11-08-queen-of-pain-hero.md` §9: the Variant B formulas.
- `docs/plans/2026-11-10-status-effects-completion.md`: `Compulsion` (Taunt, step 4), D4 (stuns are Basic-dispellable), `TargetIntent`, and step 7 (the per-row suffix precedent).
- `docs/plans/2026-11-08-attack-modifiers.md`: the attack-hit stage and cleave/splash secondary targets.
- `docs/plans/2026-11-08-healing-resource-mechanics.md`: heal-over-time and heal amp folding.
- `docs/architecture/composition.md`: §3 frame order, §4 storage styles, §5 targeting and disable matrix, §6.15 Slark, §7 checklist.

---

## 1. Current state (verified)

### Hero data flow
- **Generator.** `data/heroes.csv` is read by `scripts/HeroGenerator.py`, which is append-only. It skips existing names and patches marker regions:
  - `src/heroes.rs`: `hero_enum`, `hero_all`, `hero_name`, `hero_blurb`, `hero_primary`, `hero_def`.
  - `src/components.rs`: `ability_enum`, `ability_display_name`, `ability_ultimates`.
  - The `README.md` heroes table.
- **Changing an existing hero** means editing its arm in `heroes.rs` by hand and keeping the CSV row in sync.
  - Vanguard, Skirmisher and Arcanist are hand-written arms inside the region.
  - Warden and Hexer rows leave the optional columns blank, so their arms carry the generator defaults.
- **Abilities.** `scripts/AbilityGenerator.py` turns `data/abilities.csv` and `data/ability_effects.csv` into `src/abilities/generated.rs`, plus the `max_rank` and display-name regions in `components.rs`.
  - Rust ids come from the CSV id: `winters_curse` becomes `WintersCurse`.

### Attributes and levels
- **Attribute effects.** `HeroAttributes` (`components.rs:431`) holds str/agi/int and their per-level growth.
  - `apply_to` adds HP 18/STR, HP regen 0.12/STR, armor 0.14/AGI, mana 12/INT, mana regen 0.05/INT and MR rating 0.12/INT, then recomputes attack speed from agility.
  - **No attack damage comes from attributes.**
  - `apply_delta` is the single delta path. It is used by level-ups (`progression.rs:79`), spawn (`units.rs:49-53`), item attributes (`items.rs:982`) and timed attributes (`vitals::sync_status_attributes`).
- **Level-1 definition.** `HeroDef::vitals()` (`heroes.rs:564`) = def bases + `apply_to(level-1 attributes)`.
- **Spawn at level 6.** `spawn_hero_entity` (`units.rs:46-53`) then applies `level_up()` + `apply_delta` 5 times (`STARTING_HERO_LEVEL = 6`). So **in-game totals are level-1 values + 5 levels of growth.**
- **Variant B.** CSV stat columns are bases back-solved from level-1 totals.

### Magic resistance
- `CombatStats.magic_resist` is a rating. `combat::mitigate` gives `0.06r / (1 + 0.06|r|)`. A level-1 rating of 6.321839 means 27.5%.
- Current level-1 MR:
  - QoP, Ursa, Slark: 27.5%.
  - Magnus: 27% (col 3.764384).
  - Sniper: 26.5% (col 4.20907).
  - Vanguard, Skirmisher, Arcanist, Warden, Hexer: low (col 0.6–1.2, about 12–20%).
- HeroGenerator default: 0.7, or 1.2 for INT heroes.

### Ability ranks
- Basics have 7 ranks and ultimates have 4 (`AbilityId::max_rank`, `can_rank_up`).
- **Every existing level list extrapolates by full arithmetic steps. No half-step convention exists in the repo**: every list with 5+ entries in `data/*.csv` was checked.
- This request's rank-5 values (337.5, 10.5, 32.5, 18.5, 360, 37.5, 1025, 57.5, 5) are each **rank 4 + half a step**. The spec adopts that as the new convention (D2).

### Systems reused as is
- **Timed attack range.** `stat_modifier,attack_range` also maintains `attack_range_bonus`, so the base range (and the derived projectile speed) is unchanged.
- **Phased.** `apply_status,phased`.
- **Burn.** `apply_status,burn` is per-second, magical, with id `<owner>_burn`, and refreshes in place. That is non-stacking.
- **Slow.** `apply_status,slow` (`<owner>_slow`).
- **Targeted projectile with splash.** `projectile_type=targeted` + `aoe_radius` gives `splash_radius = AreaRadius::Ability`.
  - `combat.rs:825-841` **excludes the primary hit from `splashed`**.
  - `on_projectile_hit,affected_units` therefore hits exactly "enemies in radius, excluding the primary". Splinter Blast needs no new mechanic.
- **Heal over time.** `apply_status,heal_over_time` (flat hps; heal amp folded in at `effects.rs:1162`, ticked in `status.rs:1310`).
- **Damage-type immunity.** `StatusEffect.immune_damage` (`DamageTypeMask::PHYSICAL` exists, used by ethereal).
- **Stun flag.** `StatusEffect.stunned` gives the MOVE/ATTACK/CAST/ITEMS blocks and interrupts channels.
- **Dispel and stun.** `dispel,buff` and `mini_stun` (`<owner>_stun`, so it never shortens another source's stun).
- **Taunt.** `Compulsion::Attack { target }`, enforced by `ai::enforce_compulsions`. It is removed when the target dies.
- **Area around a unit target.** `units_in_radius` on `on_cast` for a unit-target ability centres on `Aim`, the target's position (`AbilityGenerator.py` `on_cast_center`, `generator_common.py:2206`).
- **Break.** It suppresses only `AbilityBehavior::Passive` sources (`passive.rs:47`), so Arctic Burn's `on_attack_hit` rows keep working under Break, as in Dota.

### Gaps
- **P1.** No primary-attribute attack damage (request part 2).
- **P2.** MR is not uniform across heroes (request part 1).
- **G1.** No timed `attack_projectile_speed` stat. It exists only as an item passive.
- **G2.** No status that overrides the attack point.
- **G3.** No way to run `on_attack_hit` rows of an active ability only while its buff is up. On-hit rows of any learned ability always fire (`attack_triggers.rs`).
- **G4.** `heal_over_time` has no "% of max HP per second" part.
- **G5.** No "frozen" ally status: a Buff that disables the holder and blocks physical damage. `stun` is a Debuff, so the ally's own dispel would clear it and debuff immunity would reject it.
- **G6.** Taunt always targets the caster (`definition.rs:698`). On top of that, every basic-attack stage requires `target_team == team.enemy()` (`basic_attack.rs:206, 349, 385, 578`), so a unit cannot be forced to attack its ally.
- **G7.** HeroGenerator `to_pascal_case("Winter's Curse")` returns `WinterSCurse`, but AbilityGenerator derives `WintersCurse`. The ids would not match.

---

## 2. Part 1 and part 2: hero-wide changes

### P1. One attack damage per point of primary attribute (Gameplay rules / Modifiers)

**Rust**
- `HeroAttributes` gains `pub primary: Option<Attribute>`, reusing `crate::abilities::definition::Attribute`. `components.rs` already references `crate::abilities`.
  - Add `HeroAttributes::get(Attribute) -> f32` and `primary_value() -> f32` (0 when `None`).
  - `starter()` uses `primary: None`, so existing item and progression tests keep their numbers.
- `apply_to`: `stats.attack_damage += self.primary_value()`.
- `apply_delta`: `stats.attack_damage += after.primary_value() - before.primary_value()`.
  - Through `apply_delta`, level-ups, spawn growth, item attributes and timed attributes (Slark's Essence Shift agility) all add damage automatically, and revert exactly.
- Optionally replace the attribute match in `effects.rs:631-637` (`EffectScaling::CasterAttribute`) with `HeroAttributes::get`.
- `HeroDef::level_one_attack_damage() = combat.attack_damage + attributes.primary_value()`.
- `effects::base_attack_damage` (the `attack_damage_pct` snapshot, used by Empower) returns `def().level_one_attack_damage()` for heroes. This keeps today's numbers exactly (D3).
- `heroes.rs`: every `HeroAttributes { .. }` arm gains `primary: Some(Attribute::X)` matching `primary_label()`, and its `hero_combat` damage drops by the level-1 primary value (table below). Add `use crate::abilities::definition::Attribute;`.
- **HeroGenerator**
  - `hero_def_arm` emits `primary: Some(Attribute::Strength|Agility|Intelligence)`.
  - A blank `attack_damage` now defaults to `(55 if melee else 48) − primary attribute`, so a blank cell keeps its old level-1 total.
  - Reject a row whose stored damage + primary ≤ 0.

**Parity table** (level-1 total is unchanged; magic_resist is from P2):

| Hero | Primary (L1) | Old `attack_damage` | **New stored** | L6 spawn damage change (+5 × growth) | Old MR col | **New MR col** |
|---|---|---|---|---|---|---|
| Vanguard | STR 22 | 58 | **36** | +13 | 0.7 | **4.641839** |
| Skirmisher | AGI 24 | 52 | **28** | +14 | 0.6 | **4.641839** |
| Arcanist | INT 26 | 48 | **22** | +15 | 1.2 | **3.201839** |
| Warden | STR 24 | 55 | **31** | +15 | 0.7 | **4.881839** |
| Hexer | INT 25 | 48 | **23** | +15.5 | 1.2 | **3.321839** |
| Queen of Pain | INT 25 | 54 | **29** | +17 | 3.321839 | 3.321839 |
| Magnus | STR 23 | 61 | **38** | +15.5 | 3.764384 | **3.921839** |
| Sniper | AGI 27 | 43 | **16** | +16 | 4.20907 | **4.521839** |
| Ursa | AGI 18 | 44 | **26** | +14 | 4.401839 | 4.401839 |
| Slark | AGI 21 | 61 | **40** | +7.5 | 4.401839 | 4.401839 |
| Winter Wyvern | INT 26 | — | **20** (spec total 46) | +18 (spawns at 64) | — | **3.201839** |

**CSV.** Update `attack_damage` and `magic_resist` in `data/heroes.csv` and `data/heroes.example.csv` for Magnus, QoP, Sniper, Ursa and Slark. Fill the two cells explicitly on the Warden and Hexer rows (31 / 4.881839 and 23 / 3.321839). The Vanguard row ("should be skipped") stays as is.

**Tests**
- `components.rs` or `progression.rs`:
  - `primary_attribute_adds_attack_damage_on_apply_and_level_up`
  - `non_primary_attribute_adds_no_damage`
  - `starter_attributes_without_primary_add_no_damage`
- `heroes.rs`:
  - `every_hero_keeps_level_one_attack_damage_parity`: a table of `vitals()` totals 58, 52, 48, 55, 48, 54, 61, 43, 44, 61, 46.
  - `every_hero_primary_matches_primary_label`.
- `progression.rs`: `spawned_hero_damage_includes_five_levels_of_primary_growth` (Vanguard 58 + 13).
- `items.rs`: `strength_item_adds_damage_only_to_a_strength_primary_hero`.
- `status.rs`: `timed_agility_adds_damage_to_agility_primary_hero_and_reverts`.
- Update `abilities/mod.rs::attack_damage_pct_snapshots_hero_def_damage_and_ignores_item_bonuses` to use `level_one_attack_damage()`.
- Update the `*_matches_spec` damage asserts to the level-1 totals, which are unchanged.
- `test_generator_common.py` / `HeroGeneratorTests`:
  - The `primary: Some(Attribute::Intelligence)` emission.
  - A blank damage default subtracts the primary.
  - A non-positive total is rejected.

### P2. Every hero has 27.5% magic resistance at level 1 (D1)
- Set the `magic_resist` column / arm value to `6.321839 − INT × 0.12` for every hero (table above). INT growth still raises MR above 27.5% at level 6, as it does today.
- **HeroGenerator**
  - A blank `magic_resist` defaults to `HERO_MR_RATING − base_INT × 0.12`, where `HERO_MR_RATING = (0.275 / 0.725) / 0.06`, rounded to 6 decimals.
  - An explicit value whose level-1 fraction differs from 27.5% by more than 0.05 percentage points prints a warning. It is not an error.
- **Tests**
  - `heroes.rs::every_hero_level_one_magic_resist_is_27_5_percent`: `apply_damage(100, Magical)` ≈ 72.5 for every `HeroId::all()`.
  - Update the Magnus (27% → 27.5%) and Sniper (26.5% → 27.5%) spec tests.
  - Generator test for the default and the warning.

### Permanent documentation of the conventions
- **README.md**, new subsection "Hero stat conventions" under "Adding heroes":
  - Spec values are level-1 **totals**. CSV columns are **bases**:
    - `base_health = HP − STR×18`
    - `base_health_regen = regen − STR×0.12`
    - `base_mana = mana − INT×12`
    - `base_mana_regen = regen − INT×0.05`
    - `armor = armor − AGI×0.14`
    - `magic_resist = 6.321839 − INT×0.12` (27.5% for every hero)
    - **`attack_damage = total − primary attribute`**
  - Heroes spawn at level 6 with 5 levels of growth, so in-game values exceed the spec's level-1 values.
  - Give a worked example with Winter Wyvern.
- **README "Attributes":** add "Primary attribute: +1 attack damage per point (items, levels and timed attributes included)".
- **README "Ability ranks":** document the rank-extrapolation convention (D2).
- **`scripts/HeroGenerator.py` module docstring:** the same conventions and defaults.
- **`composition.md` §4:** one sentence noting that primary damage is part of `HeroAttributes::apply_to` / `apply_delta`.
- **`composition.md` §6.15:** amend the Slark note. Essence Shift agility now also adds damage, which supersedes Slark U10.

---

## 3. Winter Wyvern: scope and ranks (D2: half step from rank 5)

| Ability | Field | Ranks |
|---|---|---|
| Q | attack range bonus | 250, 275, 300, 325, 337.5, 350, 362.5 |
| Q | duration | 7, 8, 9, 10, 10.5, 11, 11.5 |
| Q | slow % | 15, 20, 25, 30, 32.5, 35, 37.5 |
| Q | burn DPS (magical) | 15, 30, 45, 60, **67.5, 75, 82.5** |
| Q | cooldown | 29, 26, 23, 20, 18.5, 17, 15.5 |
| Q | constants | mana 100; cast 0 / 0; projectile speed +500; attack point 0.1; **burn and slow last 5 s (D4)** |
| W | damage (magical) | 80, 160, 240, 320, 360, 400, 440 |
| W | slow % (4 s) | 27, 30, 33, 36, 37.5, 39, 40.5 |
| W | mana | 105, 115, 125, 135, **140, 145, 150** |
| W | constants | cd 6; range 1150; radius 500; cast 0.3 / 1.17; **projectile speed 650 (D5)** |
| E | cast range | 850, 900, 950, 1000, 1025, 1050, 1075 |
| E | heal per second | 40, 45, 50, 55, 57.5, 60, 62.5 |
| E | % max HP per second | 1.5, 2.5, 3.5, 4.5, 5, 5.5, 6 |
| E | cooldown | 24, 21, 18, 15, 13.5, 12, 10.5 |
| E | mana | 50, 60, 70, 80, **85, 90, 95** |
| E | constants | duration 4; cast 0.3 / 0.63 |
| R | attack speed bonus | 50, 65, 80, **87.5** |
| R | mana | 150, 200, 250, **275** |
| R | cooldown | 100, 90, 80, **75** |
| R | constants | duration 4; range 800; radius 525; cast 0.3 / 1.37 |

## 4. Mapping

| Spec item | Surface | Fidelity |
|---|---|---|
| Hero stats | Variant B + P1 + P2 (§8) | Exact |
| Q range bonus | `stat_modifier,attack_range` on caster | Exact |
| Q projectile speed +500 | timed `stat_modifier,attack_projectile_speed` | **G1** |
| Q attack point 0.1 | `apply_status,attack_point` | **G2** |
| Q phased | `apply_status,phased` | Exact |
| Q on-hit burn + slow only while active | `require_status` gate → `burn` + `slow` | **G3** |
| W projectile; splinters exclude primary | `targeted` + `aoe_radius` + `affected_units` | Exact (no gap) |
| E heal flat + % max HP | `heal_over_time` with `amount` = % | **G4** |
| E encase (disabled, physical immunity) | `apply_status,frozen` | **G5** |
| R dispel + stun target | `dispel,buff` + `mini_stun` | Exact |
| R taunt nearby enemies onto the target + attack speed | `apply_status,taunt_target` (value = AS) | **G6** |
| "Winter's Curse" id | HeroGenerator apostrophe fix | **G7** |

## 5. Gap proposals

### G1. Timed `attack_projectile_speed` (Modifiers; abilities and items)
- **CSV.** Add `"attack_projectile_speed": "attack_projectile_speed"` to `TIMED_STATS`. Negative values make a debuff.
- **Rust.**
  - `StatusSpec::Modifier` gains `attack_projectile_speed: RankValue`.
  - `StatusEffect` gains `attack_projectile_speed: f32`.
  - `add_status_stat_delta` / `subtract_status_stat_delta` (`status.rs:1380-1420`) add/subtract it on `stats.attack_projectile_speed_bonus`. This is the mutate-on-apply style, like `attack_range`.
  - Add a tooltip row.
- **Allowed generated diff.** `attack_projectile_speed: RankValue::ZERO` on the 10 `Modifier` sites in `generated.rs` and the 1 in `item_catalog.rs`. The hand-written sites gain it by hand (`definition.rs::armor_buff`, `catalog.rs`, the tests in `abilities/mod.rs`, `tooltip.rs`).
- **Tests.** `timed_attack_projectile_speed_raises_and_reverts` (status.rs); a `basic_attack.rs` test checking that an in-flight projectile uses base + bonus.

### G2. Attack-point override status (Gameplay rules)
- **CSV.** `apply_status,attack_point` with `value` = seconds (0..`MAX_ATTACK_SWING`). The id is `<owner>_attack_point`; add it to `OWNER_ID_STATUSES`. It is a Buff.
- **Rust.**
  - `StatusSpec::AttackPoint { id, seconds: RankValue }` builds `StatusEffect.attack_point_override: Option<f32>` (default `None` in the base constructor).
  - `StatusEffects::attack_point_override()` returns the lowest live override.
  - `CombatStats::effective_attack_point(Option<&StatusEffects>)` returns the override, else `attack_point`, clamped.
  - `start_basic_attack_windups` (`basic_attack.rs:247`) adds `Option<&StatusEffects>` to its query and uses it. Backswing is unchanged.
  - This is not a stat delta, so nothing is reverted. It follows the same pattern as `move_speed_cap`.
- **Tests.** `attack_point_override_shortens_windup_and_ends_on_expiry`; `lowest_override_wins`.
- **Generator.** Validate the range; `remove_status,attack_point` is rejected with a hint (as for the other owner-id statuses). Add invalid examples.

### G3. `require_status` gate (Triggers layer; abilities and items)
- **CSV.** `effect_type=require_status`, `effect_id=<on_custom_trigger>:<status_id>`. This follows the `:suffix` precedent.
  - The status id is resolved with `runtime_status_id`, so `<owner>_<stat>` ids work.
  - Targets: `caster`, `owner` or `attacker`, as for the other gates.
  - Add it to `GATE_TYPES`, so it is also in `CUSTOM_TRIGGER_FEEDERS`.
  - Errors: unknown status, a missing trigger consumer, or the gate firing its own trigger.
- **Rust.**
  - `AbilityEffect::RequireStatus { id: &'static str, then: AbilityTrigger }`, added to the `acts_on_targets` exclusions.
  - In `EffectRunner`, next to `Proc`: if `gate_open(ctx)` and `statuses_of.get(ctx.caster)` holds a live status with `id`, push `ctx.derived(then)`.
  - No RNG and no counters. The depth cap still applies; `on_attack_hit` runs at depth 0 and its derived context at depth 1.
- **Tooltip.** Mirror `Proc`.
- **Tests.**
  - `require_status_fires_only_while_status_held`
  - `require_status_gate_respects_depth_cap`
  - `item_and_ability_require_status_alike`
- **Generator tests.** `RequireStatusTests`. Add invalid examples: an unknown status, and a target of `trigger_unit`.

### G4. `heal_over_time` with % of max HP (Healing)
- **CSV.** On `heal_over_time`, `amount` = percent (0..100) of the holder's max HP per second. `value` (flat hps) becomes optional, but at least one of the two must be above 0.
- **Rust.**
  - `StatusSpec::HealOverTime { hps, max_health_pct }`.
  - `StatusEffect.heal_max_health_fraction: f32` (per second).
  - `tick_status_effects` heals `(heal_per_sec + fraction × health.max) × dt` with the same `hot_factor`.
  - `build_status_effect` folds the healer amp into both parts.
  - There are no generated `HealOverTime` sites; the 5 hand-written sites gain `max_health_pct: RankValue::ZERO`.
- **Tests.** `hot_max_health_percent_reads_current_max_each_tick`; `healer_amp_scales_both_parts`. Generator test for `amount` 0..100 and the "one of value or amount" rule.

### G5. `frozen` status (Gameplay rules)
- **CSV.** `apply_status,frozen` adds the STATUSES row `("StatusSpec::Frozen", "frozen", True)`.
- **Rust.**
  - `StatusEffect::frozen(d)` = `buff("frozen")` with `stunned: true` and `immune_damage: DamageTypeMask::PHYSICAL`.
  - It reuses the disable matrix (stunned blocks MOVE/ATTACK/CAST/ITEMS and interrupts channels) and the damage-type immunity path.
  - Because it is Buff-kind, an ally's `dispel,debuff` and debuff immunity do not touch it. An enemy `dispel,buff` (Basic) does remove it (D6).
- **Tests.**
  - `frozen_ally_cannot_move_attack_or_cast`
  - `frozen_blocks_physical_attacks_but_takes_magical`
  - `frozen_survives_ally_debuff_dispel_and_debuff_immunity`
  - `frozen_interrupts_channel`

### G6. `taunt_target` and forced attacks on an ally (Gameplay rules + basic attack)
- **CSV.** `apply_status,taunt_target` with optional `value` = attack-speed bonus (≥ 0) while taunted. The runtime id is `taunt` (same as taunt).
  - Valid only on a source with `target_type=unit` and a trigger that carries a cast target (`on_cast`, `on_projectile_hit`).
  - Rejected on `passive`, `while` and `while_toggled`.
- **Rust (status).**
  - `StatusSpec::TauntCastTarget { attack_speed: RankValue }`.
  - `StatusBuildContext` and `build_status_effect` gain `cast_target: Option<Entity>`. `effects.rs` passes `ctx.cast_target`; `passive.rs` and `reactive_triggers.rs` pass `None`.
  - Build: `Some(t)` gives `StatusEffect { attack_speed_flat: as, ..StatusEffect::taunted(d, t) }`. `None` gives a no-op `debuff("taunt")`, as Taunt does without a source.
  - In the `ApplyStatus` arm of `EffectRunner`, skip this spec for the target that **is** the cast target, so the cursed unit is never taunted onto itself.
  - The attack speed rides on the taunt status, so a dispel, expiry, or the death of the target (`enforce_compulsions`) ends both together.
- **Rust (attack permission).** New helper in `basic_attack.rs`:
  ```rust
  pub(crate) fn attack_allowed(attacker_team: Team, attacker: Option<&StatusEffects>, target: Entity, target_team: Team) -> bool
  // enemy, or the exact unit an active Compulsion::Attack forces it to attack
  ```
  Use it in:
  - `start_basic_attack_requests` (the `chosen` check, `:206`; the auto-acquire fallback `:217` stays enemy-only).
  - `resolve_attack_releases` (`:385`, via `statuses_of`).
  - `resolve_basic_attack_impacts` (`:578`; an attacker that has despawned counts as enemy-only).

  Keep enemy-only:
  - `release_extra_attacks` (`:349`).
  - Cleave and splash secondary targets (`:695`, `:752`). **Skip secondaries entirely when the primary target is not an enemy.**
  - `ai::acquire_targets`, which already skips compelled units (`orders_blocked`).
- **Tests.**
  - `status.rs` / `ai.rs`: `taunt_target_forces_attack_on_cast_target_not_caster`; `taunt_target_ends_when_target_dies`.
  - `basic_attack.rs`:
    - `taunted_unit_attacks_its_ally_and_deals_damage`
    - `untaunted_unit_never_attacks_ally` (regression)
    - `forced_ally_attack_does_not_cleave_or_splash`
    - `extra_attack_on_ally_dropped`
  - `abilities/mod.rs`: `taunt_target_skips_cast_target`; `taunt_target_attack_speed_reverts_on_dispel`.
- **Generator.** `TauntTargetTests`. Add invalid examples: on a `no_target` ability, and on a `while` row.

### G7. HeroGenerator apostrophes
- `to_pascal_case` strips `'` and `’` before splitting: `Winter's Curse` → `WintersCurse`.
- No existing name changes.
- Test in `HeroGeneratorTests`.

## 6. `data/abilities.csv` (+ `data/examples/abilities.csv`)

```csv
arctic_burn,Arctic Burn,Q,active,no_target,self,,,0,,29|26|23|20|18.5|17|15.5,100,,,none,,"Winter Wyvern takes flight, gaining attack range, faster attacks and phasing; her attacks burn and slow enemies",0,,,,,
splinter_blast,Splinter Blast,W,active,unit,enemy,1150,500,0.3,,6,105|115|125|135|140|145|150,,,targeted,650,"Launches a ball of ice at a unit that shatters on arrival, damaging and slowing every other enemy nearby",1.17,,,,,
cold_embrace,Cold Embrace,E,active,unit,ally,850|900|950|1000|1025|1050|1075,,0.3,,24|21|18|15|13.5|12|10.5,50|60|70|80|85|90|95,,,none,,"Encases an ally in ice: it cannot act, heals every second and is immune to physical damage",0.63,,,,,
winters_curse,Winter's Curse,R,active,unit,enemy,800,525,0.3,,100|90|80|75,150|200|250|275,,,none,,"Freezes an enemy in place and dispels it; nearby enemies are compelled to attack it with bonus attack speed",1.37,,,,,
```

## 7. `data/ability_effects.csv` (+ `data/examples/ability_effects.csv`)

```csv
arctic_burn,0,on_cast,caster,stat_modifier,attack_range,250|275|300|325|337.5|350|362.5,,7|8|9|10|10.5|11|11.5,,,,
arctic_burn,1,on_cast,caster,stat_modifier,attack_projectile_speed,500,,7|8|9|10|10.5|11|11.5,,,,
arctic_burn,2,on_cast,caster,apply_status,attack_point,0.1,,7|8|9|10|10.5|11|11.5,,,,
arctic_burn,3,on_cast,caster,apply_status,phased,,,7|8|9|10|10.5|11|11.5,,,,
arctic_burn,4,on_attack_hit,caster,require_status,on_arctic_burn_hit:arctic_burn_attack_range,,,,,,,
arctic_burn,5,on_arctic_burn_hit,trigger_unit,apply_status,burn,15|30|45|60|67.5|75|82.5,magical,5,,,,
arctic_burn,6,on_arctic_burn_hit,trigger_unit,apply_status,slow,,,5,,15|20|25|30|32.5|35|37.5,,
splinter_blast,0,on_projectile_hit,affected_units,damage,,80|160|240|320|360|400|440,magical,,,,,
splinter_blast,1,on_projectile_hit,affected_units,apply_status,slow,,,4,,27|30|33|36|37.5|39|40.5,,
cold_embrace,0,on_cast,cast_target,apply_status,frozen,,,4,,,,
cold_embrace,1,on_cast,cast_target,apply_status,heal_over_time,40|45|50|55|57.5|60|62.5,,4,,1.5|2.5|3.5|4.5|5|5.5|6,,
winters_curse,0,on_cast,cast_target,dispel,buff,,,,,,,
winters_curse,1,on_cast,cast_target,apply_status,mini_stun,,,4,,,,
winters_curse,2,on_cast,units_in_radius,apply_status,taunt_target,50|65|80|87.5,,4,,,,
```

Status ids:
- `arctic_burn_attack_range` (this is also the gate key), `arctic_burn_attack_projectile_speed`, `arctic_burn_attack_point`, `phased`, `arctic_burn_burn`, `arctic_burn_slow`.
- `splinter_blast_slow`.
- `frozen`, `heal_over_time`.
- `winters_curse_stun`, `taunt`.

Trigger: `on_arctic_burn_hit`.

## 8. `data/heroes.csv` and `data/heroes.example.csv`

```csv
Winter Wyvern,FALSE,Arctic Burn,Splinter Blast,Cold Embrace,Winter's Curse,22,16,26,2.5,1.7,3.6,INT,"Int disabler — Arctic Burn, Splinter Blast, Cold Embrace, Winter's Curse",208,75,20,450,285,-1.24,3.201839,-0.19,0.25,1.7,0.25,0.8,700,0.6,
```
(In `heroes.example.csv`, use `false`.)

| Field | Formula | Value |
|---|---|---|
| base_health | 604 − 22×18 | 208 |
| base_health_regen | 2.45 − 22×0.12 | −0.19 |
| base_mana | 387 − 26×12 | 75 |
| base_mana_regen | 1.55 − 26×0.05 | 0.25 |
| armor | 1 − 16×0.14 | −1.24 |
| magic_resist | 6.321839 − 26×0.12 | 3.201839 |
| attack_damage | 46 − 26 (INT primary) | 20 |
| attack_projectile_speed | absolute | 700 (bonus −340 vs the derived 1040) |
| base_attack_speed | blank | 100 → APS 116/100/1.7 = 0.682353 |

## 9. Steps (one commit each; `cargo fmt`, `cargo build`, `cargo test` green between them)

1. **P1**: Rust, the heroes.rs arms, HeroGenerator emission and default, tests.
2. **P2**: heroes.rs arms, HeroGenerator default and warning, tests. The CSV parity edits for P1 and P2 can go in either commit.
3. **G7.**
4. **G1** (regenerate; only `attack_projectile_speed: RankValue::ZERO` diffs).
5. **G2.**
6. **G3.**
7. **G4.**
8. **G5.**
9. **G6.**
10. **Data.**
    - Add the rows of §6–§8 to `data/` and `data/examples/`, plus invalid rows in `data/examples/invalid/`.
    - Run HeroGenerator on a scratch CSV (header + the WW row) with `--dry-run` first. Expect `ArcticBurn/SplinterBlast/ColdEmbrace/WintersCurse`.
    - Run AbilityGenerator: 4 additions, ranks 7/7/7/4.
    - `cargo fmt`.
11. **Docs.**
    - README: the hero row (drop "(stubs ...)"), the conventions subsection, the new keywords (`attack_point`, `frozen`, `taunt_target`, `require_status`, timed `attack_projectile_speed`, `heal_over_time` `amount`), attributes, ranks.
    - `composition.md`:
      - §3 gate list (`RequireStatus`).
      - §4 (primary damage via `apply_delta`; the attack-point override follows the cap pattern).
      - §5: frozen row in the disable matrix; `taunt_target` and the `attack_allowed` exception in the targeting text.
      - §6.15 amendment, and a new §6.16 Winter Wyvern.
      - §7 checklist unchanged.
    - Append implementation notes to this plan.

## 10. Tests (beyond the per-gap tests above)

- **`heroes.rs::winter_wyvern_matches_spec`:**
  - Identity: `from_cli("winter wyvern")`, "INT", abilities `[ArcticBurn, SplinterBlast, ColdEmbrace, WintersCurse]`.
  - Level 1: HP 604, regen 2.45, mana 387, regen 1.55, armor ≈ 1, magical 100 → 72.5, **damage 46**.
  - Attributes (22, 16, 26) / (2.5, 1.7, 3.6).
  - Combat: range 450 ranged, BAT 1.7, 0.25 / 0.8, MS 285, turn 0.6, projectile 700, APS 0.682353.
- **`catalog.rs::winter_wyvern_abilities_generate_expected_definitions`:**
  - Q: NoTarget/self, `RequireStatus { id: "arctic_burn_attack_range" }`, burn at(5) = 67.5, cd at(7) = 15.5.
  - W: targeted projectile 650, aoe 500, damage at(7) = 440.
  - E: range at(5) = 1025, `HealOverTime` hps at(5) = 57.5 and pct at(5) = 5, `Frozen`.
  - R: max rank 4, `TauntCastTarget` AS at(4) = 87.5, `OwnedStun`, dispel buff.
- **Integration (`abilities/mod.rs`):**
  - `arctic_burn_grants_range_projectile_speed_attack_point_and_phase_then_reverts`
  - `arctic_burn_attacks_burn_and_slow_only_while_active`: hit after expiry → no burn.
  - `arctic_burn_burn_does_not_stack_on_repeat_hits`: one `arctic_burn_burn` instance, refreshed.
  - `splinter_blast_hits_enemies_around_target_but_not_target`
  - `splinter_blast_ignores_allies_and_far_enemies`
  - `cold_embrace_freezes_heals_flat_plus_max_hp_and_blocks_physical`
  - `cold_embrace_self_cast_allowed`
  - `winters_curse_dispels_and_stuns_target`
  - `winters_curse_taunts_nearby_enemies_to_attack_target_with_bonus_attack_speed`
  - `winters_curse_cursed_target_not_taunted`
  - `winters_curse_taunt_ends_when_target_dies`
- **`scripts/test_generator_common.py`:** `RequireStatusTests`, `AttackPointStatusTests`, `TimedProjectileSpeedTests`, `HealOverTimePercentTests`, `FrozenStatusTests`, `TauntTargetTests`, plus the `HeroGeneratorTests` additions. `ExampleCatalogTests` must still pass.

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

## 11. Invariants and risks

- **Where each rule lives.**
  - Primary damage is applied only in `HeroAttributes::apply_to` / `apply_delta`.
  - The attack-point override is read only by `effective_attack_point`.
  - The ally-attack exception exists only in `basic_attack::attack_allowed`.
  - The gate decision is made only in `EffectRunner` (I2: gates emit no gameplay messages).
- **Behaviour changes for existing content (intended):**
  - Spawned (level 6) hero damage rises by 5 × primary growth.
  - Items and timed attributes on the primary attribute add damage (Slark's Essence Shift: +3 per stack).
  - The MR of Vanguard, Skirmisher, Arcanist, Warden, Hexer, Magnus and Sniper changes.
  - **Risk:** `sim_tests.rs` and `net` tests that spawn heroes may have damage- or MR-sensitive expectations; re-baseline them only where the cause is P1 or P2.
- **Allowed generated diffs:**
  - `attack_projectile_speed: RankValue::ZERO` (11 sites).
  - The 4 WW ability blocks.
  - The WW hero arms and regions.
- **Ordering.**
  - The taunt applies in the ability appliers. `enforce_compulsions` (Ai set) writes `AttackTarget` on its next run, which may be one frame later.
  - A forced ranged attack still in flight when the curse ends fizzles at impact, because the compulsion is gone (D8).
- **Duplicate triggers.** A `require_status` gate runs once per attack-hit context, and the burn refreshes, so multiple hits never stack.
- **`StatusEffects` is not replicated.** These statuses are correct only on the sim authority (an existing limitation).

## 12. Open decisions (each has a recommended default)

- **D1. MR meaning.** Recommended: every hero's **level-1 total** is 27.5% (Variant B, as in the QoP/Ursa/Slark specs). INT still grows it.
  - Alternative: a 27.5% base before INT for all heroes (column 6.321839 everywhere), which would raise existing INT heroes to about 33–36%.
- **D2. Rank extrapolation.** The user's rank-5 values are rank 4 + half a step. Recommended: **ranks 5, 6, 7 each add half a step**, and the ultimate's rank 4 adds half a step (87.5 AS, 275 mana, 75 s). Document this as the convention for new heroes; existing heroes keep their full-step lists.
  - Alternative: full steps after the given rank 5.
- **D3. `attack_damage_pct` base** (Empower). Recommended: the definition's level-1 total (`level_one_attack_damage`), which gives exact parity.
  - Alternative: the stored base + the *current* primary attribute (Dota-like; Empower grows with levels and items).
- **D4. Arctic Burn on-hit.** Recommended: the burn and slow last **5 s** (Dota's debuff duration) and **refresh** on every hit, with the existing `<owner>_burn` refresh semantics and flat DPS. Dota's no-refresh-per-cast rule and %-current-HP burn are not built.
- **D5. Splinter Blast.**
  - Projectile speed **650** (Dota value, unverified).
  - A spell block on the primary only shields the primary; the splinters still land.
  - If the primary dies mid-flight, the projectile resolves as `OnImpact` and **no splinters** fire (no `on_impact` rows).
- **D6. Cold Embrace.**
  - A single `frozen` keyword (Buff + stunned + physical immunity) rather than two primitives.
  - An enemy Basic `dispel,buff` removes it; the HoT is a separate status and stays.
  - `heal_over_time` keeps its shared id, so Cold Embrace replaces any other HoT on the target (existing limitation).
- **D7. Winter's Curse.**
  - Applied at **cast time only**; enemies entering later are not cursed.
  - The target has **no special damage immunity**.
  - Debuff-immune enemies are not taunted (existing debuff rules).
  - `mini_stun` (owner id). A spell block on the target shields only the target; nearby units are still taunted onto it.
  - Towers inside the radius are taunted if they carry `StatusEffects` (existing area rules).
- **D8. Forced ally attacks.**
  - No cleave or splash.
  - Extra attacks stay enemy-only.
  - The attacker's own `on_attack_hit` rows fire on the ally (Dota-like).
  - In-flight attacks fizzle once the curse ends.
- **D9. `require_status` CSV syntax** `on_<trigger>:<status_id>` (the `:suffix` precedent). Alternative: a dedicated column (rejected: schema churn).
- **D10.** Arctic Burn ends piecemeal under an enemy buff dispel: all four of its statuses are Basic buffs, so they go together. The gate keys on the attack-range status.

## 13. Approval amendments (user, 2026-11-10)

- **A1 (supersedes D1 and P2's formula).** Intelligence **no longer grants magic resistance**. Remove the `0.12 MR rating per INT` term from `HeroAttributes::apply_to` and `apply_delta`. Every hero's `magic_resist` column / arm becomes **6.321839** (27.5%), and stays 27.5% at every level unless items or statuses change it.
  - The P2 parity table's "New MR col" is replaced by 6.321839 for all heroes, Winter Wyvern included (§8 row: `magic_resist` = 6.321839).
  - HeroGenerator: a blank `magic_resist` defaults to `HERO_MR_RATING` (6.321839). An explicit value that differs prints a warning.
  - Test `every_hero_magic_resist_is_27_5_percent_at_level_1_and_level_6`. Update any test that expected INT to raise MR (e.g. item or progression tests on intelligence) so that INT gives no MR.
  - Docs: remove "MR 0.12/INT" from README "Attributes" and from the conventions subsection (`magic_resist = 6.321839` flat).
- **D2 approved:** half steps for unspecified ranks.
- **D3–D10 approved** as recommended: Arctic Burn burn/slow 5 s, refreshing; Splinter Blast projectile 650; Cold Embrace removable by an enemy buff dispel; taunted units may attack their ally, with no cleave or splash.

## 14. Implementation notes

Implemented in the order of §9, one commit per step, with `cargo fmt --check`, `cargo build` and `cargo test` green after each commit. The §13 amendments override earlier sections throughout (A1: Intelligence grants no magic resistance and every hero's `magic_resist` is 6.321839).

### Commits

1. P1: primary-attribute attack damage.
2. P2 with A1: uniform 27.5% magic resistance, `MR_PER_INT` removed.
3. G7: apostrophes in HeroGenerator identifiers.
4. G1: timed `attack_projectile_speed`.
5. G2: `attack_point` status.
6. G3: `require_status` gate.
7. G4: `heal_over_time` percent of max health.
8. G5: `frozen` status.
9. G6: `taunt_target` and `basic_attack::attack_allowed`.
10. Data: Winter Wyvern hero, four abilities, invalid examples, spec, catalog and integration tests.
11. Docs: README, `composition.md` (§3 gate note, §4, §5, §6.15 amendment, new §6.16), HeroGenerator docstring, these notes.

### Deviations and reasons

- **A1 is implemented as the amendment says, not as the P2 formula.** `HeroAttributes::MR_PER_INT` and its two `+=` lines are removed, every `hero_combat` arm uses 6.321839, and the Winter Wyvern row uses `magic_resist` 6.321839 (the §8 value 3.201839 is superseded). The P2 parity table's "New MR col" is replaced by 6.321839 everywhere. The HeroGenerator default and warning use `HERO_MR_RATING = 6.321839` with a 0.05 percentage point tolerance.
- **P1/P2 test placement.** The three `HeroAttributes` tests and `spawned_hero_damage_includes_five_levels_of_primary_growth` live in the `progression.rs` test module (it already imports the attribute and spawn helpers); `components.rs` has no test module.
- **Function names.** The plan's `start_basic_attack_requests` is `request_basic_attacks` in the code; the three `attack_allowed` call sites are in `request_basic_attacks`, `resolve_attack_releases` and `resolve_basic_attack_impacts`.
- **G6 tests.** `taunt_target_forces_attack_on_cast_target_not_caster` and `taunt_target_ends_when_target_dies` are whole-sim tests in `sim_tests.rs` (module `compulsions`), because they need `enforce_compulsions` and the full attack pipeline; there is no `ai.rs` test module with that harness. They build the compulsion with `StatusEffect::taunted(duration, ally)`; the ability-level path (`TauntCastTarget` from a cast) is covered by `taunt_target_skips_cast_target` and the Winter's Curse tests. One extra test, `forced_ranged_attack_fizzles_when_the_compulsion_ends`, covers D8.
- **Invalid examples** for G2 to G6 were added together in step 10 (as §9 lists them), not per gap; each gap's own unit tests in `test_generator_common.py` landed with the gap.
- **`require_status` target.** The runtime gate checks the status on `ctx.caster` whatever the row's `effect_target` (`caster`, `owner` or `attacker` are accepted by the generator, as for the other gates, and for items the caster is the owner).
- **Tooltip.** `RequireStatus` renders the text row "While the effect is active"; `AttackPoint`, `TauntCastTarget` and the two-part `HealOverTime` got rows too (the match is exhaustive).
- **Generator rule for `heal_over_time`.** `value` and `amount` are both optional, at least one must be above 0, negatives are rejected, and `amount` must be within 0..100.
- **`frozen` generator test.** `FrozenStatusTests` also checks that `remove_status,frozen` works by id (it is not an owner-id status).
- **Winter Wyvern integration tests** that need the real projectile landing (`splinter_blast_*`) spawn a `Projectile` at the target from the cast's `SpawnProjectileEvent` and run `combat::apply_projectile_hits`, so the primary exclusion and the team and radius filters are exercised by the shipped combat code. The taunt-death test adds `ai::enforce_compulsions` to its app.
- **Environment only.** The sandbox lacked the Linux development packages Bevy needs; `libwayland-dev`, `libasound2-dev`, `libudev-dev` and `libxkbcommon-dev` were installed with apt to build. No repository file was changed for that.

### Re-baselined tests (cause: P1 or A1 only)

- `heroes::tests::magnus_matches_spec`: magical damage taken 73.0 to 72.5 (27% to 27.5%).
- `heroes::tests::sniper_matches_spec`: 73.5 to 72.5 (26.5% to 27.5%).
- `status::tests::attributes::timed_strength_changes_max_health_like_items`: the intelligence-steal assertion changed from `-5 * MR_PER_INT` to "magic resist unchanged" (A1).
- `abilities::tests::attack_damage_pct_snapshots_hero_def_damage_and_ignores_item_bonuses`: uses `level_one_attack_damage()` (listed in §2).
- No `sim_tests.rs` or `net` expectation needed re-baselining; the QoP, Ursa and Slark spec tests and every `*_matches_spec` damage assert are unchanged because level-1 totals are preserved.

### Known limitations

- A basic-attack swing already under way when a taunt lands completes against its original target (`enforce_compulsions` only clears swings for fear and hypnosis). This predates this work and applies to every taunt.
- `StatusEffects` is not replicated, so these statuses are correct only on the sim authority (as before).
- Taunted ranged attacks in flight at the moment the curse ends fizzle at impact (D8).

