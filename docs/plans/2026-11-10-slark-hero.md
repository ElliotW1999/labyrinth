# Slark hero: implementation spec

Status: **draft. Awaiting user approval.** Build nothing beyond the gaps that get approved. If implementation shows that something else is needed, stop and ask.

Related plans:
- `docs/plans/2026-11-09-ursa-hero.md`: the model for this spec. It covers the scratch-CSV HeroGenerator flow, G1 (a no-target dash goes along the facing), G7 (a refused dash reports `OnMovementEnd` in place), the `OWNER_ID_STATUSES` list, and the precedent for adding a field to every `StatusSpec::Modifier` site (G2 `attack_charges`).
- `docs/plans/2026-11-09-sniper-hero.md`: G2 zones (`AbilityEffect::Zone`, `abilities/zones.rs`) and G6 (slows measured against base speed, `move_slow_fraction`).
- `docs/plans/2026-11-08-magnus-hero.md`: the dash contact radius (G7, `radius` column on `movement,dash`), the `turn_rate` hero column, and the melee rule.
- `docs/plans/2026-11-08-queen-of-pain-hero.md`: the Variant B back-solve, and the precedent for overriding a cooldown that extrapolates to 0 (W `…|2|1`).
- `docs/plans/2026-11-10-status-effects-completion.md`: the `untargetable` field, `TargetIntent`, and D4 (stuns stay Basic-dispellable). Its step 7 proposed a per-row `effect_id:modifier` suffix syntax, which is the precedent for G2's suffix here.
- `docs/architecture/composition.md`: §3 frame order, §4 storage styles, §5 matrix, §6.13–6.14, §7 checklist.

## 1. Current state (verified)

**There is no Slark data or code.** `AbilityId` (`src/components.rs`, region `hero_generator:ability_enum`) has no `DarkPact`, `Pounce`, `EssenceShift` or `ShadowDance`. No file in `src/`, `data/`, `scripts/` or `docs/` mentions them, so no id collides.

**Reusable as is:**
- **Hero columns.** Every value fits except base attack speed (see G6). Melee at range 150, BAT, swings, `turn_rate` and AGI primary are all supported. `HeroAttributes::apply_to` adds no attack damage, so the CSV `attack_damage` is 61.
- **Delayed pulses.** `AbilityEffect::Zone` (`definition.rs:990`, `effects.rs:1043`, `zones.rs`) waits `delay`, then fires a custom trigger every `interval` for `duration` (`floor(duration/interval)` ticks). The affected units are the living, targetable units of `team` within the radius; the caster is never one. Each tick fires even when nobody is inside. The ticks run at depth ≥ 1 and are not DoT.
  - **Gap:** the centre is fixed at the spawn `ctx.point` (`effects.rs:1072`). It cannot follow Slark (G1).
- **Damage that heals for a share of what it dealt.** `resource,health_drain` → `AbilityEffect::DrainHealth { transfer_ratio }` → `DamageEvent::spell(..).drained(ratio)`. `combat.rs:505-523` heals the source by `ratio ×` the health actually removed: no overkill, nothing a shield soaked. This is exactly "heal 15% of damage dealt".
- **Self dispel.** `dispel,debuff` gives Basic strength (`amount` 1 means Strong). Under D4, stuns are Basic-dispellable.
- **Dash forward with no target.** `movement,dash` on a `no_target` cast travels the full distance along the facing (Ursa G1, `displacement.rs` `plan_motion`). The `radius` column on a dash is the contact radius (Magnus G7).
  - Contact (`displacement.rs:741-760`) fires `OnUnitContact` once per enemy that is alive and not untargetable, when it is within `contact_radius + other.bound`.
  - `OnMovementEnd` fires at the landing point. It also fires when a stun interrupts the dash, and in place when the dash is refused (Ursa G7).
- **Leash.** `apply_status,leash` takes `value` = radius and is anchored at the `TriggerPoint` (`definition.rs:609-623`). On `on_movement_end`, that point is the landing position. Enforcement is `movement::enforce_leashes`.
- **Area at the landing point.** `units_in_radius` with a `radius` column gives `AreaRadius::Fixed`, centred on the trigger point. Membership is `unit_in_area`, i.e. `area_contains(center, r, pos, bound)`.
- **On-hit passive.** `on_attack_hit` with targets `trigger_unit` (the struck unit) and `caster`, as Fury Swipes does. It does not fire on misses, and Break suppresses it.
- **Timed health regen.** `stat_modifier,health_regen` → `StatusSpec::ResourceModifier { stat: HealthRegen }`, summed when read (`progression.rs:48`).
- **Untargetable flag.** `StatusEffect.untargetable` (`status.rs:265`) is read by `rejects(TargetIntent::*)`, `unit_in_area`, `valid_target` (the caster is exempt, `effects.rs:536`), dash contact, and homing projectiles (`combat.rs:794`). Only cyclone and banish set it. **No `StatusSpec` or generator keyword exists for it (G5).**
- **Reconcile-from-statuses pattern.** `vitals::sync_status_resource_caps` with `AppliedStatusCaps` (`vitals.rs:266-325`) folds the difference between the wanted and the applied value once a frame. It runs after `apply_status_effect_events`, and `tick_status_effects` is ordered `.before` it (`status.rs:21`). This is the model for G3.

**Gaps:**
- **G1. A zone cannot follow its caster.**
- **G2. Same-id statuses always replace each other.** `status::apply_status` (`status.rs:1404-1412`) refreshes by replacement, and `stacks` merges into one timer. There is no way to keep independent per-stack timers.
- **G3. No timed attribute modifiers.** `TIMED_STATS` and `RESOURCE_STATS` (`generator_common.py:313-346`) have no strength, agility or intelligence. Only `PassiveStat::Attribute` exists, and it is permanent (`items.rs:970-982`).
- **G4. No percent move-speed bonus.** Timed `move_speed` is flat. Only slows are percent-based (`move_slow_fraction`).
- **G5. No plain untargetable status keyword** (see above).
- **G6. Base attack speed is hard-coded to 100.** `heroes.rs:494` `hero_combat` sets `base_attack_speed: 100.0`, and HeroGenerator has no column for it. Slark needs 120.

## 2. Scope and level extrapolation

Basic abilities have 7 ranks and the ultimate has 4. Values extrapolate arithmetically and constants stay constant. The exceptions are marked U.

| Ability | Field | Ranks |
|---|---|---|
| Q | total damage (magical) | 75, 150, 225, 300, 375, 450, 525 |
| Q | **damage per pulse** (total / 10, U1) | 7.5, 15, 22.5, 30, 37.5, 45, 52.5 |
| Q | cooldown | 9, 8, 7, 6, 5, 4, 3 |
| Q | constants | mana 65; radius 325; cast 0 / 0; delay 1.5; interval 0.1; duration 1 (10 pulses); heal 15% |
| W | damage (magical) | 100, 160, 220, 280, 340, 400, 460 |
| W | leash duration | 2.5, 2.75, 3, 3.25, 3.5, 3.75, 4 |
| W | cooldown | 24, 20, 16, 12, 8, 4, **2** (U7: arithmetic gives 0) |
| W | constants | mana 75; cast 0 / 0.53; distance 700; contact radius 120; dash time 0.75 s (U5); leash radius 325 (U6) |
| E | constants (U9) | −1 attack damage per stack on the target; +3 agility per stack on Slark; 10 s per stack |
| R | duration | 4, 4.25, 4.5, 4.75 |
| R | cooldown | 50, 40, 30, 20 |
| R | move speed bonus % | 20, 30, 40, 50 |
| R | health regen bonus | 70, 100, 130, 160 |
| R | constants | mana 100; cast 0 / 0 |

## 3. Mapping onto existing features

| Spec item | Surface | Fidelity |
|---|---|---|
| Vitals and attributes | Variant B back-solve (§8) | Exact |
| Attack 61, range 150 instant, BAT 1.7, swings 0.5 / 0.3, MS 300, turn 0.7 | Hero CSV columns | Exact |
| Attack speed 120 | — | **G6**. APS = (120 + 21) / 100 / 1.7 = 0.829412 |
| Q delay then 10 pulses around Slark | `zone` | **G1** (follows the caster) |
| Q damage + 15% heal | `resource,health_drain`, `amount` 0.15 | Exact |
| Q dispel each pulse | `dispel,debuff` on `caster` under the pulse trigger | Exact (Basic, U3) |
| W leap 700 forward | `movement,dash`, no target | Exact (Ursa G1) |
| W damage to enemies passed | `on_unit_contact,trigger_unit,damage`, contact radius 120 | Exact (Magnus G7) |
| W leash at the end | `on_movement_end,units_in_radius` (radius 120) `apply_status,leash` | Exact. The leash radius is not in the spec (U6) |
| E −1 damage per stack, independent timers | `stat_modifier,attack_damage` | **G2** |
| E +3 agility per stack, independent timers | — | **G2 + G3** |
| R untargetable | — | **G5** |
| R +% move speed | — | **G4** |
| R +health regen | `stat_modifier,health_regen` | Exact |

## 4. Gap proposals

### G1. Zone that follows the caster (Triggers layer; abilities and items)
- **CSV.** New effect type `attached_zone`. Its columns, rules and validation are identical to `zone` and it shares the `zone` generator branch. The only difference is that the zone is re-centred on the caster at every tick.
  - Add `attached_zone` to `EFFECT_TYPES` and `CUSTOM_TRIGGER_FEEDERS`.
  - Add it to `chain_fed_triggers` (`("chain", "zone", "attached_zone")`) and to the radius-warning exclusion at `generator_common.py:2133`.
  - Rejected alternatives: giving a new meaning to `caster` vs `owner` (they are synonyms), or reusing an unused column.
- **Rust.**
  - `AbilityEffect::Zone` gains `follow_caster: bool`, carried by `SpawnZoneEvent` and `AbilityZone`.
  - In `tick_zones`, for a following zone, before each due tick:
    - Read the caster from the existing `units` query.
    - If the caster is dead (`!health.is_alive()`), despawn the zone. No further ticks fire (U4).
    - Otherwise set `zone.center`, the zone entity's `Transform`, `template.point` and `template.origin` to the caster's current position. `units_in_radius` rows under the tick then centre on Slark as well.
  - Static zones are unchanged.
- **Allowed generated diff.** `follow_caster: false` on Shrapnel's Zone. Add the field to the 7 hand-written `AbilityEffect::Zone {` sites and the 2 `SpawnZoneEvent {` sites.
- **Tooltip.** "Zone (follows the caster): radius R, after Ds, every Is for Ts".
- **Tests (`zones.rs` / `abilities/mod.rs`).**
  - `attached_zone_recentres_on_caster_each_tick`
  - `attached_zone_stops_when_caster_dies`
  - `attached_zone_tick_point_is_caster_position`
  - `static_zone_unchanged` (Shrapnel tests stay green)
  - `item_and_ability_attached_zone_alike`
- **Generator tests.** `AttachedZoneTests` (valid row; same errors as `zone`).
- **Invalid examples.** An `attached_zone` with interval 0; an `attached_zone` targeting `affected_units`.

### G2. Independent status instances (Effects / status rules; abilities and items)
- **CSV.** On a timed `stat_modifier` row, the effect_id may carry the suffix `:independent` (e.g. `agility:independent`). Each application then becomes its own instance with its own timer, instead of refreshing the existing one.
  - Rejected on `passive`, `while` and `while_toggled` rows; unknown suffixes are errors.
  - Valid on both `TIMED_STATS` and `RESOURCE_STATS`.
  - The status id stays `<owner>_<stat>`. `held_status_id` and `runtime_status_id` strip the suffix.
- **Rust.**
  - `StatusSpec::Modifier` and `StatusSpec::ResourceModifier` gain `independent: bool`.
  - `StatusEffect.independent: bool`, default false in `StatusEffect::buff`.
  - `status::apply_status` skips the same-id removal when `effect.independent`.
  - Removal by id, dispel and debuff-immunity purge already remove every matching instance (`remove_statuses` uses `retain`). Each instance reports its own `Applied` / `Expired` lifecycle.
- **Allowed generated diff.** `independent: false` on the 9 Modifier sites in `generated.rs` and the 1 in `item_catalog.rs`. The hand-written sites (`definition.rs::armor_buff`, `catalog.rs` ×3, `abilities/mod.rs` tests ×3) gain the field by hand.
- **Tests (`status.rs`).**
  - `independent_instances_coexist_and_expire_separately`
  - `independent_attack_damage_sums_and_reverts_exactly`
  - `remove_by_id_and_dispel_remove_every_instance`
  - `non_independent_modifier_still_refreshes` (regression)
- **Generator tests.** `IndependentModifierTests`.
- **Invalid examples.** `:independent` on a passive row; an unknown suffix `armor:forever`.

### G3. Timed attribute modifiers (Gameplay rules; abilities and items)
- **CSV.** Add `strength`, `agility` and `intelligence` to `RESOURCE_STATS`, mapped to new `ResourceStat::Strength / Agility / Intelligence`. Rows are `stat_modifier,agility,3,,10`. A negative value is a Debuff, so an attribute steal is expressible.
- **Rust.**
  - Add three `ResourceStat` variants. In `vitals::resource_stat` their permanent part is 0, because attributes live on `HeroAttributes`. Add `stat_label` arms and tooltip arms.
  - New `vitals::sync_status_attributes` with the component `AppliedStatusAttributes { strength, agility, intelligence }`, inserted lazily. It mirrors `sync_status_resource_caps`:
    - `want = statuses.resource_sum(..)`.
    - When the difference is non-zero: `before = *attrs`, `attrs += delta`, then `HeroAttributes::apply_delta(&before, &attrs, health, mana, stats)`, then `applied = want`.
    - Units without `HeroAttributes`, `Mana` or `CombatStats` are skipped.
  - Schedule it right after `sync_status_resource_caps` in the `AbilityPlugin` chain and in every test app that has that system. It therefore also runs after `tick_status_effects`.
- **Why reconcile instead of mutate-on-apply.**
  - `apply_status` and `subtract_status_stat_delta` only take `&mut CombatStats` and an `agility: f32`, with about 35 call sites.
  - A per-frame diff makes every removal path (expiry, dispel, purge, remove, charges) correct without revert bookkeeping. This is the documented §4 rule for max health and max mana.
  - For one frame the attack speed uses the old agility; the reconcile recomputes it.
- **Tests (`vitals.rs` / `status.rs`).**
  - `timed_agility_raises_attributes_armor_and_attack_speed_then_reverts`
  - `timed_strength_changes_max_health_like_items`
  - `dispelled_attribute_status_reverts`
  - `attribute_status_on_unit_without_attributes_is_ignored`
  - `two_instances_add`
- **Generator tests.** `AttributeStatModifierTests`.

### G4. Percent move-speed bonus (Gameplay rules; mirror of Slow)
- **CSV.** `apply_status,move_speed_bonus` with the percent in `amount` (or `value`), 0 < p ≤ 400. The id is `<owner>_move_speed_bonus`; add it to `OWNER_ID_STATUSES`. `remove_status,move_speed_bonus` is rejected with a hint, as for the other owner-id statuses.
- **Rust.**
  - New `StatusSpec::MoveSpeedBonus { id, percent }` builds a Buff with `move_bonus_fraction = p` (new `StatusEffect` field) and `move_speed = ctx.move_speed × p`.
  - In `status::apply_status`, the re-measure condition becomes "slow > 0 or bonus > 0", and `move_speed = (bonus − slow) × base`, where `base` is the speed without status deltas (the same rule as Sniper G6).
- **Tooltip.** "Move speed +P%".
- **Tests.**
  - `move_speed_bonus_is_percent_of_base_and_reverts`
  - `refreshed_bonus_does_not_compound`
  - `bonus_and_slow_from_two_ids_add`
- **Generator tests.** `MoveSpeedBonusTests`.
- **Invalid example.** A percent of 0 or 500.

### G5. `untargetable` status keyword (Gameplay rules)
- New `StatusSpec::Untargetable` → `StatusEffect::untargetable(duration)` = `buff("untargetable")` with `untargetable: true`.
- Add the `STATUSES` row `("StatusSpec::Untargetable", "untargetable", True)` and a tooltip name.
- No new runtime rule: it reuses `rejects(Any|Attack|HostileSpell)`, area skipping, contact skipping and the projectile fizzle.
- **Tests.**
  - `untargetable_status_rejects_attacks_spells_and_areas`
  - `attackers_drop_an_untargetable_target`
  - `holder_still_runs_own_caster_effects` (caster-targeted rows and a following zone still work)
- **Generator test.** One row in `UntargetableStatusTests`.

### G6. Per-hero base attack speed (hero data)
- **Column.** Optional `base_attack_speed` hero column, appended after `turn_rate` in the `data/heroes.csv` and `data/heroes.example.csv` headers. Short rows parse as blank.
  - Validation: above 0 and within 20..700, or else `SystemExit`.
  - Blank keeps 100.
- **Emission.** When the column is set, HeroGenerator emits `combat: CombatStats { base_attack_speed: X, ..hero_combat(...) }`. When it is blank, the emission is unchanged, so no existing arm changes and `hero_combat` keeps its signature.
- **Tests.** In `HeroGeneratorTests`: blank emits plain `hero_combat(`; 120 emits the struct-update form; 0 is rejected. A `heroes.rs` test checks that the APS uses 120.

### G7 (optional, cosmetic). HUD groups same-id statuses
- `ui.rs::refresh_buffs_text` prints `+essence_shift_agility x7 10s` instead of 7 entries. A single Essence Shift fight produces 15–20 instances.

## 5. Steps (one commit each; `cargo fmt`, `cargo build`, `cargo test` green between them)

1. G6.
2. G5.
3. G4.
4. G3.
5. G2 (regenerate; only `independent: false` diffs).
6. G1 (regenerate; only Shrapnel's `follow_caster: false`).
7. G7, if approved.
8. **Data.**
   - Add the rows of §6–§8 to `data/` and `data/examples/`, plus invalid rows in `data/examples/invalid/`.
   - Run HeroGenerator on a scratch CSV holding the header plus the Slark row, kept in the session scratchpad. Run `--dry-run` first, expecting `DarkPact/Pounce/EssenceShift/ShadowDance`, then the real run.
   - Run AbilityGenerator: 4 additions, ranks 7/7/7/4.
   - Run `cargo fmt`.
9. **Docs.**
   - README: hero row display names; the status table (`untargetable`, `move_speed_bonus`); timed attribute stats; the `:independent` suffix; `attached_zone`; the `base_attack_speed` column.
   - `composition.md`: §3 (zone row: an attached zone re-centres each tick), §4 (an "independent instances" note under Counter/refresh, and attributes in the reconcile style next to max health / mana), §5 (untargetable status in the targeting table), and a new §6.15 "Slark".
   - Append implementation notes to this plan.

## 6. `data/abilities.csv` (+ `data/examples/abilities.csv`)

```csv
dark_pact,Dark Pact,Q,active,no_target,enemy,,325,0,,9|8|7|6|5|4|3,65,,,none,,"After a short delay, Slark sheds most debuffs and damages nearby enemies in quick pulses, healing for part of the damage dealt",0,,,,,
pounce,Pounce,W,active,no_target,enemy,,,0,,24|20|16|12|8|4|2,75,,,none,,"Slark leaps forward, damaging every enemy he passes; enemies near his landing point are leashed",0.53,,,,,
essence_shift,Essence Shift,E,passive,no_target,self,,,,,,,,,none,,"Each successful attack drains the target's attack damage and grants Slark agility; every stack wears off on its own",,,,,,
shadow_dance,Shadow Dance,R,active,no_target,self,,,0,,50|40|30|20,100,,,none,,"Slark hides in a cloud of shadows, becoming untargetable and gaining bonus move speed and health regeneration",0,,,,,
```

## 7. `data/ability_effects.csv` (+ `data/examples/ability_effects.csv`)

```csv
dark_pact,0,on_cast,caster,attached_zone,on_dark_pact_pulse,0.1,,1,,1.5,,
dark_pact,1,on_dark_pact_pulse,caster,dispel,debuff,,,,,,,
dark_pact,2,on_dark_pact_pulse,affected_units,resource,health_drain,7.5|15|22.5|30|37.5|45|52.5,magical,,,0.15,,
pounce,0,on_cast,caster,movement,dash,700,,0.75,120,,,
pounce,1,on_unit_contact,trigger_unit,damage,,100|160|220|280|340|400|460,magical,,,,,
pounce,2,on_movement_end,units_in_radius,apply_status,leash,325,,2.5|2.75|3|3.25|3.5|3.75|4,120,,,
essence_shift,0,on_attack_hit,trigger_unit,stat_modifier,attack_damage:independent,-1,,10,,,,
essence_shift,1,on_attack_hit,caster,stat_modifier,agility:independent,3,,10,,,,
shadow_dance,0,on_cast,caster,apply_status,untargetable,,,4|4.25|4.5|4.75,,,,
shadow_dance,1,on_cast,caster,apply_status,move_speed_bonus,,,4|4.25|4.5|4.75,,20|30|40|50,,
shadow_dance,2,on_cast,caster,stat_modifier,health_regen,70|100|130|160,,4|4.25|4.5|4.75,,,,
```

- **Dark Pact.** Pulses at 1.5, 1.6, …, 2.4 s and the zone despawns at 2.5 s. Each pulse dispels Slark (rows run in `effect_index` order), then drains every enemy within 325 of his **current** position. The zone fires even with nobody inside, so the dispel always happens. Slark is never an affected unit, so there is no self-damage (U2).
- **Pounce.** Contact damage hits each enemy once. Every enemy within 120 + bound at landing was also contacted on the final frame, so every leashed unit was also damaged. The leash is anchored at the landing point.
- **Essence Shift.** Status ids are `essence_shift_attack_damage` (Debuff) and `essence_shift_agility` (Buff, a `ResourceModifier`). A debuff-immune target gets no drain debuff, but Slark still gains agility.

## 8. `data/heroes.csv` and `data/heroes.example.csv` (Variant B; the header gains `,base_attack_speed`)

```csv
Slark,TRUE,Dark Pact,Pounce,Essence Shift,Shadow Dance,20,21,16,2.1,1.5,1.9,AGI,"Agi assassin — Dark Pact, Pounce, Essence Shift, Shadow Dance",200,75,61,150,300,-2.94,4.401839,-0.15,0,1.7,0.5,0.3,,0.7,120
```
(`heroes.example.csv`: the same row with `true`.)

| Field | Formula | Value |
|---|---|---|
| base_health | 560 − 20×18 | 200 |
| base_health_regen | 2.25 − 20×0.12 | −0.15 |
| base_mana | 267 − 16×12 | 75 |
| base_mana_regen | 0.8 − 16×0.05 | 0 |
| armor | 0 − 21×0.14 | −2.94 |
| magic_resist | (0.275/0.725)/0.06 − 16×0.12 | 4.401839 (as for Ursa) |
| attack_projectile_speed | blank (melee, instant) | — |
| base_attack_speed | G6 | 120 |

## 9. Tests

- **`heroes.rs::slark_matches_spec`:**
  - Identity: `from_cli("slark")`, name, "AGI", abilities `[DarkPact, Pounce, EssenceShift, ShadowDance]`.
  - Level-1 vitals: HP 560, regen 2.25, mana 267, regen 0.8, armor ≈ 0, magical 100 → 72.5.
  - Attributes: (20, 21, 16) / (2.1, 1.5, 1.9).
  - Combat: damage 61, range 150 and `is_melee()`, BAT 1.7, swings 0.5 / 0.3, turn 0.7, MS 300, `base_attack_speed` 120, APS 0.829412.
- **`catalog.rs::slark_abilities_generate_expected_definitions`:**
  - Q: NoTarget, AoE 325, Zone with `follow_caster` true (delay 1.5, interval 0.1, duration 1), DrainHealth at(7) = 52.5 with ratio 0.15, cooldown at(7) = 3.
  - W: backswing 0.53, Dash 700 with contact 120, leash radius 325, cooldown at(7) = 2.
  - E: passive, two independent statuses.
  - R: ultimate, max rank 4, untargetable, bonus at(4) = 50, regen at(4) = 160.
- **Integration tests (`abilities/mod.rs`; use `qop_app()` where expiry is needed):**
  - `dark_pact_waits_then_pulses_ten_times_around_slark`: nothing before 1.5 s; 10 × 7.5 pre-mitigation magical; an enemy at 400 is untouched.
  - `dark_pact_dispels_debuffs_each_pulse`: a slow applied at 1.55 s is gone after the next pulse; Slark's own buffs survive.
  - `dark_pact_heals_fifteen_percent_of_damage_dealt`
  - `dark_pact_follows_slark_after_pounce`
  - `dark_pact_never_damages_slark`
  - `dark_pact_stops_when_slark_dies`
  - `pounce_leaps_forward_damages_passed_enemies_and_leashes_those_at_landing`: an enemy passed early is damaged but not leashed; an enemy at the landing point is both.
  - `pounce_leash_holds_target_within_radius_of_landing_point`
  - `essence_shift_stacks_expire_independently`: hits at t = 0 and t = 5; at t = 10.05 there is one instance each, agility +3 and target damage −1; at t = 15.05 there are none.
  - `essence_shift_agility_raises_attack_speed_and_armor`
  - `essence_shift_does_nothing_on_miss`
  - `shadow_dance_makes_slark_untargetable_hasted_and_regenerating`, including expiry back to base.
  - `shadow_dance_ranks_scale`
  - `shadow_dance_with_dark_pact_still_pulses`
- **Per-gap unit tests** as listed in §4.
- **`scripts/test_generator_common.py`:** `AttachedZoneTests`, `IndependentModifierTests`, `AttributeStatModifierTests`, `MoveSpeedBonusTests`, `UntargetableStatusTests`, plus the `HeroGeneratorTests` additions. The `ExampleCatalogTests` must still pass.

Verification commands:
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

Environment: the earlier hero work needed the Bevy system libraries (`pkg-config libwayland-dev libxkbcommon-dev libudev-dev libasound2-dev libx11-dev`) installed with apt. No repository file changes for this.

## 10. Invariants and risks

- **Where each rule lives:**
  - Instance replacement is decided only in `status::apply_status`.
  - Attribute deltas are applied only by `vitals::sync_status_attributes`. Nothing else writes `HeroAttributes` from statuses.
  - Zone re-centring happens only in `tick_zones`. The zone still only emits `AbilityTriggerEvent`.
  - The percent bonus is re-measured only in `apply_status`, next to the slow.
- **Only allowed generated diffs:** `independent: false` (10 sites), Shrapnel's `follow_caster: false`, the 4 Slark ability blocks, and the Slark hero arms and regions.
- **Ordering.**
  - `sync_status_attributes` must run after `apply_status_effect_events` and after `tick_status_effects`. Placing it after `sync_status_resource_caps` guarantees both.
  - Attack speed lags agility by up to one frame.
- **Following zone with a dead caster:** it despawns. It is not paused.
- **Event volume.** Essence Shift creates 1 status per hit on each side. The `StatusEffects` vec grows by about 15–20 entries per target in a long fight, and lifecycle events fire per instance. That is acceptable; G7 tidies the HUD.
- **Behaviour changes for existing content:** none. Shrapnel stays static, and statuses without `:independent` refresh as before.
- **`StatusEffects` is not replicated.** Untargetable and the speed bonus are correct only on the sim authority (an existing limitation).

## 11. Naming

| CSV id | `AbilityId` | Display |
|---|---|---|
| `dark_pact` | `DarkPact` | Dark Pact |
| `pounce` | `Pounce` | Pounce |
| `essence_shift` | `EssenceShift` | Essence Shift |
| `shadow_dance` | `ShadowDance` | Shadow Dance |

- Status ids: `pounce_leash` is not used, because leash has the fixed id `leash`. The others are `essence_shift_attack_damage`, `essence_shift_agility`, `untargetable`, `shadow_dance_move_speed_bonus` and `shadow_dance_health_regen`.
- Trigger: `on_dark_pact_pulse`.

## 12. Open decisions (each has a recommended default)

- **G1–G6:** approval. All six are required for the kit as specified. **G7:** optional, cosmetic.
- **U1.** Dark Pact deals total / 10 per pulse: 10 pulses at 1.5 … 2.4 s, the first at the end of the delay.
- **U2.** No self-damage. The spec does not mention it; Dota's 50% non-lethal self-damage is not built.
- **U3.** The dispel is Basic ("most debuffs"). Under D4 this also removes stuns and other Basic-dispellable disables. Strong (`amount` 1) is the alternative.
- **U4.** Dark Pact follows Slark, keeps pulsing while he is untargetable or dashing, and ends if he dies.
- **U5.** The Pounce dash takes 0.75 s (about 933 u/s; the spec gives no speed). Pounce does not stop at the first hero hit.
- **U6.** Pounce damages on contact (pass-through) and leashes only enemies within 120 of the landing point. The leash radius is 325, which is not in the spec.
- **U7.** Pounce rank-7 cooldown is 2 s. Arithmetic extrapolation gives 0; this follows the QoP precedent.
- **U8.** A rooted or refused Pounce reports its end in place (Ursa G7), so it leashes enemies within 120 of Slark with no damage. Alternative: block casting while rooted (not proposed).
- **U9.** Essence Shift values are constant across its 7 ranks (rank-ups add nothing). The alternative is to extrapolate the duration.
- **U10.** +3 agility gives armor (+0.42) and attack speed (+3) only, because the engine has no primary-attribute damage. An optional extra row, `caster,stat_modifier,attack_damage:independent,3,,10`, would emulate Dota's damage; it is not added by default.
- **U11.** Essence Shift instances keep the default Basic dispel type (orthogonal and smallest). Dota makes them undispellable; that would need the deferred per-row dispel override (status plan step 7) or making `:independent` imply undispellable.
- **U12.** Essence Shift works on every enemy unit (the spec says "enemy"), creeps included. It shares Fury Swipes' current `on_attack_hit` targeting.
- **U13.** **Shadow Dance uses the engine's `untargetable`**, which also excludes Slark from **all area effects** (enemy and ally) and makes in-flight homing projectiles fizzle. Alternative: a "targeting-only" variant (attack + hostile-spell rejection, areas still hit) built from the existing `attack_immune` and `spell_immune` flags. Recommended: literal `untargetable`, confirm with the user.
- **U14.** Shadow Dance rank 4 is extrapolated: duration 4.75, cooldown 20, +50% move speed, +160 regen.
- **U15.** The move-speed bonus is measured against speed without status deltas (items included). It adds to slows and has no upper speed cap.
- **U16.** The Dark Pact `aoe_radius` ring is drawn at cast time (cosmetic), as well as the zone's ring at the first pulse.
