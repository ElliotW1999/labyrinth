# Shadowheart hero: implementation spec

Status: **approved 2026-11-11 with amendments (see §13, which overrides earlier sections).** Build only the gaps listed here (G1–G5). If implementation shows that anything else is needed, stop and ask.

Related plans and docs:
- `docs/plans/2026-11-10-winter-wyvern-and-attribute-damage.md` (this plan's template). It covers the Variant B back-solve with primary-attribute damage (P1), the flat 27.5% hero MR (A1), half-step rank extrapolation (D2), the `require_status` `:suffix` precedent, `StatusBuildContext.cast_target` (G6) and `OWNER_ID_STATUSES`.
- `docs/plans/2026-11-10-slark-hero.md`: `attached_zone` (G1). This is the model for the orbit mechanic's following, ticking and death rules. It also covers the scratch-CSV HeroGenerator flow and the reconcile/mutate storage rules.
- `docs/plans/2026-11-09-ursa-hero.md`: the `damage_reduction` status (`damage_taken_factor`). This is the model for G2's read-time multiplicative factor.
- `docs/plans/2026-11-08-magnus-hero.md`: dash contact (`OnUnitContact`, once per unit). G4 extends that contact idea to re-arming contact.
- `docs/plans/2026-11-08-healing-resource-mechanics.md`: the `heal` effect, heal amp, and the mana shield. The mana shield is the precedent for diverting damage inside `apply_damage_events`.
- `docs/architecture/composition.md`: §3 frame order, §4 storage rules, §5 matrix, §6.16, §7 checklist.

---

## 1. Current state (verified)

**Nothing named Shadowheart exists.** There is no `Purification`, `WardingBond`, `SpiritGuardians` or `SharranConversion` in `AbilityId` (`src/components.rs`), nor any `damage_share`, `orbit` or `convert_on_death` keyword in `scripts/generator_common.py`, `src/`, `data/` or `docs/`. Note: "conversion" already names `HealKind::Conversion` in `src/vitals.rs`. That is why the R status keyword is `convert_on_death`.

**Reusable as is:**
- **Hero data.** Every column fits: melee 150 with instant attack (blank `attack_projectile_speed`), BAT 1.7, swings 0.4 / 0.6 (≤ `MAX_ATTACK_SWING` 1.0), `turn_rate` 0.6, blank `base_attack_speed` (100), and STR primary.
- **Heal.** `heal` → `AbilityEffect::Heal` (`generator_common.py:1649`). An `ally` unit-target ability may be self-cast (`cold_embrace_self_cast_allowed`, `abilities/mod.rs:11729`).
- **Pure damage.** `DamageType::Pure` ignores armor and MR (`combat.rs:920-926`).
- **Area around a unit target.** `units_in_radius` on `on_cast` of a unit-target ability centres on `Aim`, the target's position (`AbilityGenerator.py:270`).
- **Armor buff.** `stat_modifier,armor` (`TIMED_STATS`).
- **Caster-following ticker.** `zones.rs` (`SpawnZoneEvent` → `AbilityZone` → `tick_zones`, with `follow_caster` and despawn on caster death). It is scheduled in `AbilitiesPlugin` (`abilities/mod.rs:123,133`) and mirrored in the test app (`:781,791`).
- **Damage diversion inside the damage pipeline.**
  - `apply_damage_events` (`combat.rs:399-556`) already diverts damage to the mana shield after mitigation (`:481-487`).
  - It re-emits reflected damage as `reactive` events that are read on the next frame (`:526-555`).
- **Creeps and AI.**
  - `units::spawn_creep(team, lane, pos, ranged)` (`units.rs:104`) gives full health, a bounty, and a `LaneFollower` built from `map::lane_path(team, lane)`.
  - `ai::acquire_targets` (`ai.rs:93`) makes every non-`PlayerHero` unit with an attack range attack the nearest enemy within range + 80.
  - Waves spawn creeps continuously (`waves.rs`), so there are always units that can be resurrected.
- **Death.** `combat::despawn_dead` (`combat.rs:877-918`) pays the gold and XP bounty, then despawns every dead non-`PlayerHero` unit. It runs in the combat chain after `SimSet::Movement`, on the sim authority only.

**Gaps:**
- **G1. Area team on ally/self abilities.**
  - `units_in_radius` takes `ctx.team` from the ability's `target_team` (`generator_common.py:2275`).
  - So an `ally`/`self` ability's damage area hits **allies**, and Purification's "damage enemies around the ally" cannot be expressed.
  - The example `war_cry` (`data/examples/ability_effects.csv:24-25`, a `self` ability that "knocks enemies back") is currently generated with `TargetTeam::Ally` on its knockback and pure damage. That is a latent bug with the same cause.
- **G2. No percent magic resistance.**
  - Timed `magic_resist` is a **rating** added to `CombatStats.magic_resist` (`mitigate`: 0.06r / (1 + 0.06|r|)).
  - "+5% MR" cannot be expressed exactly. Ethereal (`status.rs:719`) back-solves a rating against a zero base, which is wrong on a 27.5% hero.
- **G3. No damage share/redirect between units.** Only reflect (back to the source) and mana shield (to the holder's own mana) exist.
- **G4. No orbiting hitboxes.**
  - Dash contact (`displacement.rs`) fires once per unit per dash, and zones tick on an interval.
  - Nothing rotates around a unit, and nothing re-arms after contact ends.
- **G5. No "enemy dies near me" hook.** `OnKill` / `OnDeath` fan out to the killer and the victim only (`reactive_triggers.rs:751-772`). They are also dispatched **one frame after** `despawn_dead` has already removed the corpse, so a trigger-based resurrection would have nothing to read.

---

## 2. Scope and ranks (D2: half steps for unspecified ranks)

| Ability | Field | Ranks |
|---|---|---|
| Q | heal / pure damage | 90, 160, 230, 300, 335, 370, 405 |
| Q | cooldown | 15, 14, 13, 12, 11.5, 11, 10.5 |
| Q, W, E | mana | 90, 105, 120, 135, 142.5, 150, 157.5 |
| Q | constants | range 700; radius 260 around the target; cast 0.2 / 1.4 |
| W | armor | 1, 2, 3, 4, 4.5, 5, 5.5 |
| W | magic resistance % | 5, 10, 15, 20, 22.5, 25, 27.5 |
| W, E | cooldown | 40, 35, 30, 25, 22.5, 20, 17.5 |
| W | constants | range 700; cast 0.3 / 1.33; **duration 15 s (U1)**; **share 50% (U2)** |
| E | duration | 3, 4, 5, 6, 6.5, 7, 7.5 |
| E | pure damage per contact | 90, 160, 230, 300, 335, 370, 405 |
| E | constants | cast 0.3 / 0; 3 spirits; 1 revolution per 1.41 s; **orbit radius 180, hit radius 80, reach 260 (U8)** |
| R | duration | 5, 7, 9, **10** |
| R | mana | 125, 175, 225, **250** |
| R | cooldown | 100, 90, 80, **75** |
| R | constants | radius 300; **cast 0 / 0 (U14)** |

## 3. Mapping

| Spec item | Surface | Fidelity |
|---|---|---|
| Hero stats | Variant B back-solve (§8) | Exact |
| Q heal on the ally target | `on_cast,cast_target,heal` | Exact |
| Q pure damage to enemies within 260 of the target | `on_cast,units_in_radius,damage,pure` with `aoe_radius` 260 | **G1** |
| W armor on caster and target | `stat_modifier,armor` × 2 rows | Exact |
| W MR % on caster and target | `apply_status,magic_resistance` × 2 rows | **G2** |
| W damage shared with the caster | `apply_status,damage_share` on `cast_target` | **G3** |
| E three rotating spirits with contact damage that re-arms | `orbit` → custom trigger → `trigger_unit,damage,pure` | **G4** |
| R resurrect enemy deaths near the caster as allies | `apply_status,convert_on_death` on the caster | **G5** |

---

## 4. Gap proposals

### G1. Area team follows effect polarity on ally/self abilities (generator only; abilities)
- **Rule.** In `effect_to_rust`, the `units_in_radius` branch (`generator_common.py:2275`) becomes: `team = ctx.team if ctx.team in ("Enemy", "Any") else ("Ally" if beneficial else "Enemy")`.
  - Enemy and both abilities are unchanged.
  - On ally/self abilities, harmful rows (damage, debuffs, knockback, ...) now pick enemies. Beneficial rows still pick allies.
  - Items already use polarity (no `ctx.team`).
- **Rust.** No change.
- **Generated diff.**
  - `src/abilities/generated.rs`: none. No shipped ally/self ability has a `units_in_radius` row (verified against `data/abilities.csv`).
  - Examples: the `war_cry` knockback and damage rows flip from `Ally` to `Enemy`. This is intended, because it matches the row's description.
- **Rejected alternative.** New `enemies_in_radius` / `allies_in_radius` targets. That adds a keyword to every target check (`:2113`, `:2245`, the invalid combos) to fix something the polarity rule already expresses.
- **Tests.** `AreaPolarityTests`:
  - an ally ability with a damage area emits `Enemy`;
  - an ally ability with a heal area emits `Ally`;
  - an enemy ability with a heal area emits `Enemy` (unchanged);
  - a self ability with a `stun` area emits `Enemy`.

### G2. `magic_resistance` percent status (Gameplay rules / Modifiers; abilities and items)
- **CSV.** `apply_status,magic_resistance` takes the percent in `amount` (or `value`), with 0 < p < 100.
  - The id is `<owner>_magic_resistance`; add it to `OWNER_ID_STATUSES` and to `STATUSES` as a Buff.
  - `remove_status,magic_resistance` is rejected with the usual owner-id hint.
- **Rust.**
  - New `StatusSpec::MagicResistance { id, percent: RankValue }`.
  - New `StatusEffect.magic_resist_fraction: f32`, default 0.
  - New `StatusEffects::magic_damage_factor() -> f32`: the product of `(1 − f)` over live effects. This gives Dota-style multiplicative stacking, mirroring `damage_taken_factor` (`status.rs:917`).
  - The only call site: in `apply_damage_events`, the `taken_factor` argument becomes `damage_taken_factor() × (if event.damage_type == Magical { magic_damage_factor() } else { 1.0 })`.
  - It is a read-time factor, so nothing reverts (§4 "read when used" style).
  - A 27.5% hero with +20% takes 100 × 0.725 × 0.8 = 58.
- **Tooltip.** Rows "Magic resistance %" and "Magic resistance duration".
- **Known limitation.** The HUD MR stat (`ui.rs:1482`) still shows the rating only.
- **Rejected alternative.** Back-solving the percent into rating points with `stat_modifier,magic_resist`. That needs no code, but it gives +2.7% instead of +5% on a 27.5% hero, and the tooltip would show "0.877".
- **Tests (`status.rs` / `combat.rs`).**
  - `magic_resistance_multiplies_with_rating_mr`
  - `two_magic_resistance_sources_multiply`
  - `magic_resistance_ignores_physical_and_pure`
  - `magic_resistance_ends_on_expiry`
- **Generator.** `MagicResistanceStatusTests` (range checks, owner id).

### G3. `damage_share` status (Gameplay rules, damage pipeline; abilities and items)
- **CSV.** `apply_status,damage_share` with `amount` = percent of the holder's damage passed to the **unit that applied it** (the status source).
  - Range 0 < p ≤ 100. Id `damage_share` (a Buff). A later bond replaces an earlier one.
  - The generator rejects targets `caster` / `owner`, because sharing with oneself does nothing.
- **Rust (status).**
  - New `StatusSpec::DamageShare { percent }`. It builds `StatusEffect.damage_share: Option<DamageShare { partner: Entity, fraction: f32 }>` from `StatusBuildContext.source`.
  - If `source` is `None`, or equals the holder, it builds a no-op buff.
  - New `StatusEffects::damage_share() -> Option<DamageShare>`: the first live one.
- **Rust (`apply_damage_events`).** These steps run after the mana shield and before `health.current -= …`.
  1. Look up the target's share with read-only access before the existing `get_mut`.
  2. Apply only if all of these hold: the event is **not `reactive`**, the partner ≠ the target, the partner is alive, and the partner is not invulnerable.
  3. Compute `shared = health_delta × fraction`. Lower `health_delta` by `shared`.
  4. Push a redirected `DamageEvent` to a `shared` vec, written next to `reflected`:
     - `source` = the original source;
     - `target` = partner, `amount` = shared;
     - `damage_type: Pure`, `kind: Generic`;
     - `is_spell: false`, `applies_block: false`, `pierces_shields: false`;
     - `reflects: false`, `drain_ratio: 0`, `reactive: true`.
- **Consequences** (all intended, see U2–U5):
  - The partner's armor and MR do not reduce the shared damage. Its shields, damage reduction and invulnerability still do.
  - Because the redirected event is reactive, it is never shared again (no loops) and fires no damage triggers.
  - It lands one frame later, like reflect.
  - The attacker's lifesteal and drain on the target are computed from the reduced `health_delta`.
  - `DamageAppliedEvent.amount` for the target is the reduced amount.
- **Tooltip.** "Damage shared with caster %".
- **Tests (`combat.rs` `mod tests`).**
  - `damage_share_splits_post_mitigation_damage_with_partner_as_pure`
  - `damage_share_skips_dead_or_invulnerable_partner`
  - `reactive_damage_is_not_shared`
  - `mutual_shares_do_not_loop`
  - `self_sourced_share_is_noop`
  - `share_applies_after_mana_shield`
- **Generator.** `DamageShareStatusTests`.
- **Invalid examples.** `amount` 0; `amount` 150; target `caster`.

### G4. `orbit` mechanic gate (Triggers layer; abilities and items)
New module `src/abilities/orbits.rs`, structured like `zones.rs`.

- **CSV.** `effect_type=orbit`, target `caster` / `owner` only. Columns:

  | Column | Meaning |
  |---|---|
  | `effect_id` | `on_<trigger>:<hit_radius>` (custom trigger + hit radius, required, > 0) |
  | `value` | number of orbiters (one integer, ≥ 1) |
  | `duration` | seconds (> 0, rank list) |
  | `radius` | orbit radius; else the ability's `aoe_radius` |
  | `amount` | seconds per revolution (> 0) |

  - `damage_type` and scaling are unused.
  - Add `orbit` to `EFFECT_TYPES`, `CUSTOM_TRIGGER_FEEDERS`, `chain_fed_triggers` and the radius-warning exclusion (`:2245`).
  - `fed_trigger` already strips the `:suffix`.
  - Errors: the same consumer and self-trigger checks as `zone`.
- **Rust (definition).**
  - `AbilityEffect::Orbit { count: u32, radius: AreaRadius, hit_radius: f32, period: RankValue, duration: RankValue, team: TargetTeam, then: AbilityTrigger }`.
  - Add it to the `acts_on_targets` exclusions.
- **Rust (runner, `effects.rs`).** Beside `Zone`:
  - Check `gate_open`, then build the template the same way: `ctx.derived(then)`, no trigger unit, no affected units, no attack.
  - Write a `SpawnOrbitEvent { template, count, radius, hit_radius, period, duration, team, start_angle }`.
  - `start_angle` is the caster's facing yaw.
- **Rust (`orbits.rs`).**
  - `spawn_requested_orbits` spawns an `AbilityOrbit` entity.
    - It holds the fields above plus `elapsed` and `touching: Vec<Vec<Entity>>` (one list per orbiter).
    - Presentation: it adds `count` child spheres using `spell_bolt_mesh` / `spell_bolt_mat` when `SharedAssets` is present.
  - `tick_orbits` handles each live orbit as follows:
    1. **End conditions.** Despawn when the caster entity is gone or dead, or when `elapsed ≥ duration`. A dead caster ends the orbit; it does not pause.
    2. **Sub-steps.** Advance in sub-steps so that the arc per sub-step is ≤ `hit_radius`. The angular speed is ω = 2π / period, and `steps = ceil(ω·dt·radius / hit_radius)`, at least 1.
    3. **Orbiter position.** At each sub-step, orbiter k sits at `caster_pos + radius·(cos θk, 0, sin θk)`, where `θk = start_angle + ω·t + 2πk/count` (counter-clockwise).
    4. **Contact set.** `now` = the units passing `unit_in_area(team, template.team, …, orbiter_pos, hit_radius)`. This covers alive, team, bound, and fully-untargetable skipping; the caster is excluded.
    5. **Entry triggers.** For each unit in `now` but not in `touching[k]`, write `AbilityTriggerEvent(template.clone().trigger_unit(Some(u)).at(orbiter_pos))`.
    6. **Re-arm.** Set `touching[k] = now`. A unit re-arms only after it leaves that orbiter's hitbox. Orbiters are independent of each other.
    7. **Visuals.** Update the child sphere transforms.
- **Schedule.**
  - `tick_orbits` goes right after `zones::tick_zones`, and `spawn_requested_orbits` right after `zones::spawn_requested_zones`, in `AbilitiesPlugin` and in the test app (`abilities/mod.rs:781,791`).
  - Register `add_message::<SpawnOrbitEvent>()` in both.
- **Tooltip.** "Orbit: N × (hit R) at radius X, 1 revolution / Ps, for Ds".
- **Tests (`abilities/mod.rs`, a `mod orbits` block).**
  - `orbit_positions_follow_caster_and_angle`
  - `orbit_hits_stationary_enemy_once_per_orbiter_per_revolution`: over exactly one period, the count is `count`.
  - `orbit_does_not_rehit_while_contact_persists`: a unit with bound 400 is hit exactly once per orbiter for the whole duration.
  - `orbit_rehits_after_contact_ends`
  - `orbit_substeps_prevent_tunnelling_at_low_fps`: dt = 0.1.
  - `orbit_ignores_allies_caster_and_cycloned_units`
  - `orbit_ends_on_duration_and_on_caster_death`
  - `item_and_ability_orbit_alike`: an item's `OnUse` writes the same `SpawnOrbitEvent`.
- **Generator.** `OrbitTests`.
- **Invalid examples.** `value` 0; `amount` 0; a missing `:hit_radius`; target `cast_target`.

### G5. `convert_on_death` status (Gameplay rules, death; abilities and items)
- **CSV.** `apply_status,convert_on_death` with `value` = radius (> 0) and `duration` (the leash precedent for `value` = radius).
  - Id `convert_on_death`, a Buff with the default Basic dispel.
  - Rejected on `passive` and `while` rows (it is a timed aura).
- **Rust (status).**
  - New `StatusSpec::ConvertOnDeath { radius }`.
  - New `StatusEffect.convert_radius: f32`.
  - New `StatusEffects::convert_radius() -> f32`: the largest live value, or 0.
- **Rust (`Creep`).**
  - `Creep { lane, ranged: bool }`. Its only constructor is in `spawn_creep`; drop the `#[allow(dead_code)]` on `lane`.
  - `spawn_creep` returns its `Entity`. Existing callers in `waves.rs` and `sim_tests.rs` ignore the return value, so nothing else changes.
- **Rust (`combat::despawn_dead`).** Only additions:
  - The `dead` query gains `Option<&Creep>` and `Option<&BoundRadius>`.
  - A read-only query is added: `holders: Query<(&Transform, &Team, &Health, &StatusEffects)>`.
  - The bounty is paid as today. Then, for a dead unit **with `Creep`**, find the nearest **living** holder whose `team == victim.enemy()`, whose `convert_radius() > 0`, and for which `area_contains(holder_pos, r, victim_pos, victim_bound)` holds.
  - If one is found, write `CreepConvertedEvent { position, team: holder_team, lane, ranged }`. This is a new message, registered in `CombatPlugin`.
  - Despawn as today.
- **Rust (`units::spawn_converted_creeps`).**
  - Runs chained right after `despawn_dead` in `CombatPlugin`, on the sim authority. It takes `Option<Res<SharedAssets>>` and skips when the resource is absent.
  - For each event: `spawn_creep(team, lane, position, ranged)` gives full health, team materials, normal bounties and the AI.
  - It then overwrites the `LaneFollower` with `LaneFollower::resume_from(lane_path(team, lane), position)`, a new `ai.rs` helper. The helper picks the nearest waypoint and advances one if the creep is already past it toward the next. This stops the creep from walking back to its own base.
  - `acquire_targets` then makes it attack nearby enemies; no AI change is needed.
- **Why not a trigger.** `OnKill` / `OnDeath` are dispatched from `DamageAppliedEvent` on the next frame, after `despawn_dead` has removed the corpse. A trigger design would have to delay every despawn by a frame, which is a global behaviour change. Folding the check into the one place deaths are finalised keeps ownership single and the ordering deterministic.
- **Tooltip.** "Converts enemy creeps that die within R".
- **Tests.**
  - `sim_tests.rs`, new `mod conversion` using the full-sim harness and `spawn_creep_at`:
    - `enemy_creep_dying_near_holder_returns_as_ally_with_full_health`: team flipped, `Health.current == max`, same `ranged` and lane.
    - `bounty_still_paid_on_converted_death`
    - `creep_dying_outside_radius_or_after_expiry_is_not_converted`
    - `dead_holder_converts_nothing`
    - `heroes_towers_and_dummy_never_convert`
    - `allied_creep_dying_near_holder_is_not_converted`
    - `converted_creep_attacks_nearby_enemy`
    - `converted_creep_resumes_lane_forward`
  - `ai.rs` unit tests: `lane_follower_resume_from_picks_next_waypoint`.
- **Generator.** `ConvertOnDeathTests`.
- **Invalid examples.** `value` 0 and missing; on a `passive` row.

---

## 5. Steps (one commit each; `cargo fmt`, `cargo build` and `cargo test` green between them)

1. **G1.** Generator only. `AbilityGenerator --check` shows no change to `generated.rs`; the examples change only `war_cry`.
2. **G2.**
3. **G3.**
4. **G4.**
5. **G5.**
6. **Data.**
   - Add the rows of §6–§8 to `data/` and `data/examples/`, plus the invalid rows in `data/examples/invalid/`.
   - Run HeroGenerator on a scratch CSV (header plus the Shadowheart row) in the session scratchpad. Run `--dry-run` first and expect `Purification/WardingBond/SpiritGuardians/SharranConversion`, then the real run.
   - Run AbilityGenerator: 4 additions, ranks 7/7/7/4. Then `cargo fmt`.
7. **Docs.**
   - README: hero row (drop "(stubs …)"), and the keywords `magic_resistance`, `damage_share`, `convert_on_death` and `orbit`, plus the area polarity rule.
   - `composition.md`:
     - §3: `tick_orbits` beside `tick_zones`; conversion in `despawn_dead` → `spawn_converted_creeps`.
     - §4: `magic_resistance` is a read-time factor like `damage_reduction`.
     - §5: the damage-pipeline order with the share step last; the area polarity rule.
     - A new §6.17 Shadowheart.
   - Append implementation notes to this plan.

## 6. `data/abilities.csv` (+ `data/examples/abilities.csv`)

```csv
purification,Purification,Q,active,unit,ally,700,260,0.2,,15|14|13|12|11.5|11|10.5,90|105|120|135|142.5|150|157.5,,,none,,"Instantly heals an allied unit and deals pure damage to every enemy around it",1.4,,,,,
warding_bond,Warding Bond,W,active,unit,ally,700,,0.3,,40|35|30|25|22.5|20|17.5,90|105|120|135|142.5|150|157.5,,,none,,"Binds Shadowheart to an ally: both gain armor and magic resistance, and half of the damage the ally takes is dealt to Shadowheart instead",1.33,,,,,
spirit_guardians,Spirit Guardians,E,active,no_target,enemy,,260,0.3,,40|35|30|25|22.5|20|17.5,90|105|120|135|142.5|150|157.5,,,none,,"Three spirits circle Shadowheart, dealing pure damage to each enemy they touch",0,,,,,
sharran_conversion,Sharran Conversion,R,active,no_target,enemy,,300,0,,100|90|80|75,125|175|225|250,,,none,,"For a short time, enemy creeps that die near Shadowheart rise again as her allies at full health",0,,,,,
```

## 7. `data/ability_effects.csv` (+ `data/examples/ability_effects.csv`)

```csv
purification,0,on_cast,cast_target,heal,,90|160|230|300|335|370|405,,,,,,
purification,1,on_cast,units_in_radius,damage,,90|160|230|300|335|370|405,pure,,,,,
warding_bond,0,on_cast,caster,stat_modifier,armor,1|2|3|4|4.5|5|5.5,,15,,,,
warding_bond,1,on_cast,cast_target,stat_modifier,armor,1|2|3|4|4.5|5|5.5,,15,,,,
warding_bond,2,on_cast,caster,apply_status,magic_resistance,,,15,,5|10|15|20|22.5|25|27.5,,
warding_bond,3,on_cast,cast_target,apply_status,magic_resistance,,,15,,5|10|15|20|22.5|25|27.5,,
warding_bond,4,on_cast,cast_target,apply_status,damage_share,,,15,,50,,
spirit_guardians,0,on_cast,caster,orbit,on_spirit_guardians_touch:80,3,,3|4|5|6|6.5|7|7.5,180,1.41,,
spirit_guardians,1,on_spirit_guardians_touch,trigger_unit,damage,,90|160|230|300|335|370|405,pure,,,,,
sharran_conversion,0,on_cast,caster,apply_status,convert_on_death,300,,5|7|9|10,,,,
```

- **Status ids.** `warding_bond_armor` (on both units), `warding_bond_magic_resistance` (on both), `damage_share`, `convert_on_death`.
- **Trigger.** `on_spirit_guardians_touch`.
- **Purification.** With G1, row 1 resolves to `TargetTeam::Enemy`, centred on `Aim` (the ally), using `AreaRadius::Ability` (260). The healed ally is never damaged.
- **Self-cast Warding Bond.** Rows 0/1 and 2/3 refresh the same ids on one unit, so the buffs apply once. The share builds a no-op (U7).

## 8. `data/heroes.csv` and `data/heroes.example.csv` (Variant B + P1 + A1)

```csv
Shadowheart,TRUE,Purification,Warding Bond,Spirit Guardians,Sharran Conversion,22,15,18,3.1,1.7,2.1,STR,"Str support — Purification, Warding Bond, Spirit Guardians, Sharran Conversion",208,75,36,150,310,-0.1,6.321839,-0.19,0,1.7,0.4,0.6,,0.6,
```
In `heroes.example.csv`, use `true`.

| Field | Formula | Value |
|---|---|---|
| base_health | 604 − 22×18 | 208 |
| base_health_regen | 2.45 − 22×0.12 | −0.19 |
| base_mana | 291 − 18×12 | 75 |
| base_mana_regen | 0.9 − 18×0.05 | 0 |
| armor | 2 − 15×0.14 | −0.1 |
| magic_resist | flat (A1) | 6.321839 (27.5%) |
| attack_damage | 58 − 22 (STR primary) | 36 |
| attack_projectile_speed | blank (melee, instant) | — |
| base_attack_speed | blank | 100 → APS (100 + 15) / 100 / 1.7 = 0.676471 |

## 9. Tests (beyond the per-gap tests)

- **`heroes.rs::shadowheart_matches_spec`.**
  - Identity: `from_cli("shadowheart")`, "STR", abilities `[Purification, WardingBond, SpiritGuardians, SharranConversion]`.
  - Level 1: HP 604, regen 2.45, mana 291, regen 0.9, armor ≈ 2, magical 100 → 72.5, damage 58.
  - Attributes: (22, 15, 18) / (3.1, 1.7, 2.1).
  - Combat: range 150 and `is_melee()`, BAT 1.7, swings 0.4 / 0.6, MS 310, turn 0.6, APS 0.676471.
  - Add 58 to the `every_hero_keeps_level_one_attack_damage_parity` table.
- **`catalog.rs::shadowheart_abilities_generate_expected_definitions`.**
  - Q: Unit/Ally, range 700, aoe 260; `Heal` at(5) = 335; a `Pure` `Damage` on `UnitsInRadius { center: Aim, team: Enemy }`; cooldown at(7) = 10.5; mana at(5) = 142.5.
  - W: two armor modifiers (at(7) = 5.5), two `MagicResistance` (at(4) = 20), `DamageShare` 50, backswing 1.33.
  - E: `Orbit { count: 3, hit_radius: 80, radius 180, period 1.41 }`, duration at(7) = 7.5, pure damage on `TriggerUnit`.
  - R: max rank 4, `ConvertOnDeath` radius 300, duration at(4) = 10, cooldown at(4) = 75.
- **Integration (`abilities/mod.rs`, `qop_app()` where expiry matters).**
  - `purification_heals_ally_and_pure_damages_enemies_around_it`: an enemy at 250 from the ally takes 90 regardless of armor/MR; an enemy beside the caster but 400 from the ally is untouched; allies near the target are not damaged.
  - `purification_self_cast_heals_and_damages_around_caster`
  - `warding_bond_buffs_both_and_reverts_after_15s`
  - `warding_bond_shares_half_of_target_damage_with_caster`: use a magical 100 on a 0-MR unit with 5% → 95 → 47.5 each, the caster's half arriving one frame later.
  - `warding_bond_self_cast_buffs_once_without_share`
  - `spirit_guardians_three_spirits_hit_enemy_on_orbit_path`: over one 1.41 s period, 3 × 90 pure.
  - `spirit_guardians_ends_after_duration`
  - `spirit_guardians_follow_shadowheart`
  - `sharran_conversion_status_lasts_duration`: the full resurrection flow is in `sim_tests.rs`, as per G5.
- **`scripts/test_generator_common.py`.** `AreaPolarityTests`, `MagicResistanceStatusTests`, `DamageShareStatusTests`, `OrbitTests`, `ConvertOnDeathTests`, and a `HeroGeneratorTests` row for Shadowheart. `ExampleCatalogTests` must still pass.

Verification:
```bash
cargo fmt --check && cargo build && cargo test
python3 scripts/AbilityGenerator.py --check
python3 scripts/ItemGenerator.py --check
python3 scripts/AbilityGenerator.py data/examples/abilities.csv data/examples/ability_effects.csv --check
python3 scripts/ItemGenerator.py data/examples/items.csv data/examples/item_effects.csv --check
(cd scripts && python3 -m unittest test_generator_common)
python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run
git diff --stat src/abilities/generated.rs src/item_catalog.rs   # only the 4 Shadowheart blocks
```
Environment: install the Bevy system libraries with apt if they are missing (as for earlier heroes). No repository file changes are needed for that.

## 10. Invariants and risks

- **Where each rule lives.**
  - Share: only in `apply_damage_events`, as the last step before health changes.
  - Magic resistance percent: only in that function's `taken_factor`.
  - Orbit contact: only in `tick_orbits`, which emits only `AbilityTriggerEvent`. Damage is an ordinary effect row.
  - Conversion decision: only in `despawn_dead`. Unit creation: only in `spawn_converted_creeps`, which uses `spawn_creep`.
- **No duplicate triggers.**
  - A spirit fires once per contact entry.
  - Shared and reflected damage are `reactive`, so they are never re-shared and fire no damage triggers. Mutual bonds cannot ping-pong.
  - A dead creep is evaluated once, in the same pass that despawns it.
- **Ordering.**
  - Shared damage lands one frame after the source hit (like reflect).
  - The converted creep exists from the next command flush. The old entity is despawned in the same pass, so no stale `AttackTarget` / `MoveTarget` is carried over.
  - `OnKill` / `OnDeath` still fire for the original death (trigger unit = the despawned entity, as for every death today).
- **Allowed generated diffs.** The 4 Shadowheart ability blocks and the Shadowheart hero arms and regions. No field is added to `StatusSpec::Modifier`, so no generated or item sites churn. The examples' `war_cry` area team changes from `Ally` to `Enemy` (G1).
- **Behaviour changes for existing content.** None in shipped data. `Creep` gains a field, and `spawn_creep` now returns an `Entity`.
- **Performance.** `tick_orbits` sub-steps at about 13 u/frame at 60 fps (radius 180, ω ≈ 4.46 rad/s), so it normally takes 1 step. Per orbit the cost is count × units per step.
- **Not replicated.** `StatusEffects` and orbit entities exist on the sim authority only. Clients do not see the spirit visuals (an existing limitation for zones too).

## 11. Naming

| CSV id | `AbilityId` | Display |
|---|---|---|
| `purification` | `Purification` | Purification |
| `warding_bond` | `WardingBond` | Warding Bond |
| `spirit_guardians` | `SpiritGuardians` | Spirit Guardians |
| `sharran_conversion` | `SharranConversion` | Sharran Conversion |

## 12. Open decisions (each has a recommended default)

- **G1–G5.** Approval. All five are required for the kit.
- **U1. Warding Bond duration.** Not in the spec.
  - Recommended: **15 s at every rank** (uptime 37–86% across ranks).
  - Alternatives: until dispelled or until either unit dies, or a per-rank duration.
- **U2. "Shared".**
  - Recommended: a **50/50 split of the target's post-mitigation damage**. The caster's half arrives as pure: it is not reduced by the caster's armor/MR, but the caster's shields, damage reduction and invulnerability apply.
  - Alternatives: a BG3-style mirror (the target takes 100% and the caster also takes a copy), or a 100% redirect.
- **U3.** The share applies only while the caster is alive and not invulnerable. Otherwise the target takes everything.
  - There is no distance break, and the bond does not end early on the caster's death.
  - Shared damage **can kill the caster**.
- **U4.** Reactive damage (reflect, shared damage) is never shared. Everything else is shared: attacks, spells, DoTs, pure.
- **U5.** The share comes after the target's mana shield. The attacker's lifesteal on the target counts only the target's portion.
- **U6.** MR % stacks multiplicatively (Dota style) through a new status (G2), not a back-solved rating. The HUD keeps showing the rating.
- **U7.** Warding Bond may be self-cast. The buffs then apply once and there is no share. Alternative: reject self-targeting (needs an "other ally" team rule; not proposed).
- **U8. Spirit geometry.** The spec's "damage radius 260" is read as the **reach**: orbit radius 180 + hit radius 80. Melee attackers hugging Shadowheart (≥ 100 from her centre plus their bound) are still touched.
  - Alternative: orbit radius 260 with hit radius 80. That reaches 180–340 and misses adjacent melee units.
- **U9. Spirit motion.** Spirits start at the caster's facing + 0°/120°/240° and rotate counter-clockwise. Each spirit has its own contact memory, so the 3 spirits can each hit the same enemy: a stationary enemy on the path takes about 2.1 hits per second. Damage is per contact, not per second.
- **U10.** Spirits end on the caster's death. They keep going while she is stunned or untargetable. A recast while active adds a second set (cooldown > duration at every rank, so this only matters with future cooldown reduction).
- **U11. Conversion eligibility.** Only creeps (`Creep`) are converted. Heroes, towers, ancients and the training dummy are excluded. There are no neutrals or summons in the game today.
- **U12. Converted units.**
  - A fresh creep of Shadowheart's team at the death point, with the same melee/ranged type and lane, full health, normal bounties, and the existing creep AI.
  - It auto-attacks enemies within range + 80 and otherwise resumes pushing its lane forward.
  - **Permanent until killed.**
  - Alternative: a lifetime (e.g. 30 s) through `Lifetime`. Note that `tick_lifetimes` despawns without a bounty, which would be right for summons.
- **U13. What counts as a death near Shadowheart.**
  - Any enemy creep death in range counts, whoever killed it: Shadowheart, her allies, towers or creeps.
  - Range is the holder's centre to the victim's edge (`area_contains` with the victim's bound).
  - The holder must be alive. With several holders, the creep is converted once (by the nearest).
  - The original killer's team still gets the bounty.
- **U14. R cast animation.** Not given; uses 0 / 0.
- **U15.** `convert_on_death` is a Basic-dispellable buff. An enemy `dispel,buff` ends Sharran Conversion early.
- **U16. Ranks.** Half-step extrapolation (D2), including mana 142.5/150/157.5 and R rank 4 (10 s, 250 mana, 75 s).

## 13. Approval amendments (2026-11-11; these override §2–§12 where they conflict)

- **A1. Warding Bond duration (U1): 10 s fixed at every rank.** In §7 the `duration` of every `warding_bond` row is `10`. In the §9 test, rename `warding_bond_buffs_both_and_reverts_after_15s` to `..._after_10s`.
- **A2. Warding Bond sharing (U2): duplicate, not split.** This replaces the G3 design.
  - The target takes **100%** of its damage, as normal. Its own `health_delta` is **not** reduced.
  - The caster (the status source, the partner) also receives a **copy of the incoming damage before mitigation**: the same `amount` the event carried before the target's armor, MR, block, shields or damage reduction, and the **same `damage_type`**.
  - The partner's own mitigation then applies to that copy: armor for physical, rating and percent MR for magical, none for pure, plus its own damage reduction, shields and invulnerability.
  - The copy is a new `DamageEvent` with:
    - `source` = the original source, `target` = the partner;
    - `damage_type` = the original type, `amount` = the original pre-mitigation amount;
    - `kind: Generic`, `is_spell: false`, `applies_block: false`, `pierces_shields: false`;
    - `reflects: false`, `drain_ratio: 0`, `reactive: true`.
  - So the copy is never re-shared (no loops), fires no damage triggers, and grants no lifesteal or drain. It lands one frame later, like reflect.
  - The copy is emitted only when the event actually lands on the target: the target is alive and not invulnerable. If the target's shield soaked the damage, the copy is still emitted.
  - The partner must be alive and must not be the target. U3 and U4 still hold (no distance break, so the shared damage can kill the caster; reactive damage is never shared). U5's "share after mana shield" no longer applies, because the target's damage is untouched.
  - The CSV `amount` stays: it is now the percent of the incoming damage that is duplicated. Warding Bond uses `100`. The tooltip row becomes "Damage duplicated to caster %".
  - §4 G3 tests become:
    - `damage_share_duplicates_pre_mitigation_damage_to_partner_with_its_own_mitigation`. Example: physical 100 on a target with armor A and a partner with armor B; each takes `100 × its own armor factor`.
    - `damage_share_keeps_magical_type_for_partner_mr`
    - `damage_share_skips_dead_partner`
    - `damage_share_partner_invulnerable_takes_nothing`
    - `reactive_damage_is_not_shared`
    - `mutual_shares_do_not_loop`
    - `self_sourced_share_is_noop`
    - `invulnerable_target_emits_no_copy`
  - §9's `warding_bond_shares_half_of_target_damage_with_caster` becomes `warding_bond_duplicates_target_damage_to_caster`: a magical 100 on the target, where each unit takes 100 × its own MR factor, and the caster's copy arrives one frame later.
- **A3. Spirit geometry (U8): approved as recommended.** Orbit radius 180, hit radius 80, reach 260.
- **A4. Converted creeps (U12): 30 s lifetime.**
  - `spawn_converted_creeps` adds `Lifetime` (30 s, a constant `CONVERTED_CREEP_LIFETIME_SECS` in `units.rs`) to each converted creep.
  - `tick_lifetimes` despawns the creep without a bounty when the lifetime expires. If the creep is killed before then, normal bounties apply.
  - A converted creep is itself a `Creep`. If it dies near an enemy `convert_on_death` holder, it may be converted back. That is allowed, and the new copy gets a fresh lifetime.
  - Add the sim test `converted_creep_expires_after_30s_without_bounty`.
  - The tooltip says "rise again as allies for 30s".
  - In §6, the description becomes: "For a short time, enemy creeps that die near Shadowheart rise again as her allies at full health for 30 seconds".
- **A5.** The data descriptions for Warding Bond (§6) say: "both gain armor and magic resistance, and all damage the ally takes is also dealt to Shadowheart".

---

## 14. Implementation notes (2026-11-11)

Built in the plan's order, one commit per step (G1, G2, G3, G4, G5, data, docs). §13 was applied throughout: Warding Bond lasts 10 s, `damage_share` duplicates the incoming pre-mitigation damage (100 for Warding Bond) while the target still takes everything, and converted creeps get a 30 s `Lifetime`. Only the gaps G1–G5 were built.

### What was built, and where

| Step | Surface | Files |
|---|---|---|
| G1 | `units_in_radius` team: `ctx.team` wins only for enemy / both abilities, otherwise effect polarity. `AreaPolarityTests`. No generated Rust changed | `generator_common.py` |
| G2 | `StatusSpec::MagicResistance { id, percent }`, `StatusEffect.magic_resist_fraction`, `StatusEffects::magic_damage_factor()`, folded into `taken_factor` for magical events; tooltip rows; generator keyword and owner id | `status.rs`, `combat.rs`, `definition.rs`, `tooltip.rs`, `generator_common.py` |
| G3 | `StatusSpec::DamageShare { percent }`, `StatusEffect.damage_share: Option<DamageShare { partner, fraction }>`, `StatusEffects::damage_share()`; the copy is emitted from `apply_damage_events` | same files |
| G4 | `AbilityEffect::Orbit`, `SpawnOrbitEvent`, `AbilityOrbit`, `spawn_requested_orbits`, `tick_orbits`; scheduled beside the zone systems in the plugin and both test apps; `orbit` effect type | new `abilities/orbits.rs`, `effects.rs`, `definition.rs`, `mod.rs`, `tooltip.rs`, `generator_common.py` |
| G5 | `StatusSpec::ConvertOnDeath { radius }`, `StatusEffect.convert_radius`, `CreepConvertedEvent`, conversion check in `despawn_dead`, `units::spawn_converted_creeps`, `CONVERTED_CREEP_LIFETIME_SECS`, `Creep.ranged`, `spawn_creep` returns its `Entity`, `LaneFollower::resume_from` | `status.rs`, `combat.rs`, `units.rs`, `ai.rs`, `components.rs` |
| Data | The §6–§8 rows (with the §13 changes), the generated kit, the hero arm, invalid examples | `data/`, `generated.rs`, `heroes.rs`, `components.rs` |

### Deviations and judgement calls

1. **Amount of the shared copy (G3 / A2).** The copy's `amount` is the incoming amount before the target's mitigation, but the copy goes through `apply_damage_events` again, with the same source, and that pass re-applies the source's outgoing physical amp (`resolve_outgoing_amount`). So a physical copy carries the raw `event.amount`, and a magical or pure copy carries the amount after outgoing amps (`outgoing`, which already holds any spell amp; the copy is `is_spell: false`, so it is not amplified twice). In both cases the partner receives the source's amps exactly once. This is what "pre-mitigation, same damage type" means once the outgoing amp is taken into account; without it a physical hit would be amplified twice.
2. **Partner alive check.** A dead or despawned partner is not checked when the copy is emitted (the target's query borrow is held there); the copy is dropped when it is processed on the next frame (dead or missing target, or invulnerable). The result is the same as the plan's "partner must be alive" rule.
3. **Self-sourced bond.** `StatusBuildContext` does not know the holder, so "partner == target" is decided at damage time (no copy), not when the status is built. A bond with no source builds a plain `damage_share` buff.
4. **Damage-immune targets.** §13 says the copy is emitted when the target is alive and not invulnerable. A target that is only damage-immune (for example spell immunity against magic) still emits the copy, as the plan states. If that is not wanted, add `&& !damage_immune` to the condition in `apply_damage_events`.
5. **Orbit angle.** `start_angle` is `atan2(forward.z, forward.x)` of the caster's facing, so orbiter 0 starts in front of her; angles increase counter-clockwise in the XZ plane (as written in `AbilityOrbit::offset`). The orbit entity's `Transform` is moved to the caster every frame and the sphere visuals are its children (only when `SharedAssets` carries the meshes; `SharedAssets::default()` in the test apps does).
6. **System tuple size.** The abilities chain in `AbilitiesPlugin` and in the test app reached Bevy's 20-member tuple limit, so `spawn_requested_zones` and `spawn_requested_orbits` are grouped as one chained sub-tuple. Order is unchanged.
7. **Other test plumbing.** The items test app registers `SpawnOrbitEvent` (the effect runner takes it as a parameter), and the `combat::tests` XP helper registers `CreepConvertedEvent`.
8. **Test layout.** The orbit tests are flat functions under a `// ---- orbits ----` header in `abilities/mod.rs` (like the zone tests) instead of a nested `mod orbits`, which would shadow the `orbits` module. The G3 tests are in `combat.rs` (`mod damage_share`, an `App` is used so the copy is read on the next update), the G2 tests in `combat.rs` (`mod magic_resistance`).
9. **Invalid examples.** They reuse the existing `fireball` ability of `data/examples/invalid/` (indexes 48-61). The first two `damage_share` rows and the `cast_target` orbit row also report that fireball is not a unit ability; the intended error is reported as well.
10. **`war_cry`.** Its generated area team flips to Enemy only in the validation output of the examples (no example is generated into `src/`), so no file changed for it.
11. **Clippy attribute.** `despawn_dead` has `#[allow(clippy::type_complexity)]` after its query grew.

### Behaviour notes for reviewers

- Orbit damage is per contact entry. A unit standing still on the path takes one hit per orbiter pass (a rank 1 orbit of 3 s hits a stationary unit on the path 6 times: `spirit_guardians_ends_after_duration`).
- `convert_on_death` takes the nearest living enemy holder; a converted creep is a `Creep`, so it can be converted back, with a fresh 30 s `Lifetime`.
- The HUD MR stat still shows the rating only (known limitation from G2). Orbit spheres and `StatusEffects` exist on the sim authority only, so clients do not see the spirits (as for zones).

### Verification (all run on this branch)

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo build` | succeeds (the 4 warnings were already there) |
| `cargo test` | 832 passed, 0 failed |
| `python3 scripts/AbilityGenerator.py --check` | 34 abilities, 87 effect rows |
| `python3 scripts/ItemGenerator.py --check` | 60 items, 85 effect rows |
| `AbilityGenerator.py` / `ItemGenerator.py` on `data/examples/` with `--check` | 47 abilities, 113 rows / 28 items, 56 rows |
| `python3 -m unittest test_generator_common` (in `scripts/`) | 229 tests, OK |
| `python3 scripts/HeroGenerator.py data/heroes.example.csv --dry-run` | nothing to add (Shadowheart already present) |
| `git diff --stat src/abilities/generated.rs src/item_catalog.rs` against the plan commit | `generated.rs` +217 / -0 (the 4 Shadowheart blocks); `item_catalog.rs` unchanged |
