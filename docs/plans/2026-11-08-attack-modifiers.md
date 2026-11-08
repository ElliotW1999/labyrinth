# Attack modifiers: audit and implementation plan

**Date:** 2026-11-08
**Branch:** `claude/magical-bardeen-2oxkvv` (HEAD `3dd593b`)
**Based on:** `2026-11-06-damage-mechanics.md` (crit, cleave, `DamageKind`, `CombatRng`), `2026-11-06-shared-effects-pipeline.md` (one trigger bus, one executor), `2026-11-06-status-effects-catalog.md` (blind, attack-speed slow), `2026-11-07-item-ability-architecture-cleanup.md` (Phase 4 put `item_attack_triggers` in the abilities chain), `2026-11-08-healing-resource-mechanics.md` (leech rules, `ResourceDrainEvent`).
**Status:** Approved by coordinator with the defaults in §8.

---

## Executive summary

About half of the list already works. Crit, cleave, lifesteal, attack speed and attack damage modifiers are in the code. Poison, slow and armor reduction on hit can already be built from data: the item `OnAttackHit` trigger feeding existing statuses.

The attack pipeline has three defects that every modifier depends on:

1. **Blind is checked on the wrong unit.** It is read from the *defender's* statuses (`basic_attack.rs:465-467`). Blinding a unit makes attacks *against* it miss, so blind behaves like evasion.
2. **Blind does not use the shared RNG.** It uses a per-pair hash (`basic_attack.rs:32-41`), so a given attacker/target pair either always misses or never misses.
3. **`OnAttackHit` fires on misses.** Item triggers read `BasicAttackImpactEvent` (`items.rs:1150-1157`), which is sent *before* the miss check. Missed attacks, and attacks on invalid targets, still run on-hit effects.

Smaller defects:
- Cleave events inherit `can_crit: true` (`combat.rs:163-168`, `120`). Each cleave victim rolls its own crit.
- On-hit `Damage` entries become `DamageEvent::spell` (`effects.rs:340`). They get spell amp, spell lifesteal and a second damage block, instead of counting as attack damage.

**Design.** No new framework. Five small, reusable pieces:

1. **One attack-hit stage** in `basic_attack::resolve_basic_attack_impacts`. It runs: validate → hit roll (blind / evasion / accuracy, one `CombatRng` roll) → crit roll → main `DamageEvent` → new `BasicAttackLandedEvent` → cleave/splash. Crit moves here out of `combat::resolve_outgoing_amount`.
2. **One attack-trigger system**, `abilities/attack_triggers.rs`, replacing `items::item_attack_triggers`. It handles items *and* learned abilities, reads `OnAttackHit` from landed attacks only, and stamps `TriggerContext.attack`.
3. **Two "gate" effects** in the existing effect pipeline. `AbilityEffect::Proc { chance, then }` and `AbilityEffect::Counter { every, then }` re-fire the same context under a custom trigger. This covers bash, chance-based orbs, double attack and attack counters with no new trigger variants and no new definition fields.
4. **One `AbilityEffect::PerformAttack` effect** sending `ExtraAttackEvent`, which goes into the normal release path flagged `extra`. Instant attack, double attack and multi-target attack all use it. There is one recursion guard: effects triggered by extra attacks never run `PerformAttack`.
5. **Stat plumbing** through the existing `PassiveStat` / `ItemPassives` / `StatusSpec` paths: evasion, accuracy / true strike, crit sources, cleave/splash ratios, attack range, attack projectile speed.

---

## 1. Existence audit

Legend: **Exists** · **Exists (composable)** = works today by combining existing data rows · **Partial** · **Missing**

| Mechanic | Status | Evidence | Gap |
|---|---|---|---|
| Critical strike | **Partial** | `CombatStats.crit_chance/crit_multiplier` `components.rs:134-137`; rolled at damage application `combat.rs:231-233`; `CombatRng::roll_crit` `combat.rs:201-203`; `DamageTestHooks.force_crit` `combat.rs:206-209`; test `crit_doubles_with_test_hook` `combat.rs:1084` | No source can grant crit: no `PassiveStat` (`definition.rs:486-512`), heroes hard-code 0 (`heroes.rs:348`). Cleave rolls independent crits (`combat.rs:163-168`). The crit result is unknown to the attack stage, so cleave cannot use crit damage |
| Bash | **Partial** | `StatusSpec::Stun` `definition.rs:214`; item `OnAttackHit` `items.rs:1143-1186` | No chance gate anywhere (`AbilityEffectEntry` `definition.rs:603-607`). Abilities cannot use attack triggers (`ABILITY_TRIGGERS` `generator_common.py:53-65`; only items emit them) |
| Cleave | **Partial** | `basic_attack.rs:477-505`; `CombatStats.cleave_radius/cleave_damage_ratio` `components.rs:148-150` | Nothing sets the stats (`heroes.rs:355-356`, no `PassiveStat`). Uses pre-crit damage but rolls its own crit. Ranged units can cleave |
| Splash | **Partial** | Spell projectiles only: `ProjectileHitEvent.splashed` `combat.rs:566-577`; 45% ratio helper in `abilities/catalog.rs` | No attack splash |
| Lifesteal | **Exists** | Leech step `combat.rs:392-406` (post-mitigation, overkill-capped, shield-excluded); `ResourceStat::Lifesteal` `vitals.rs:24,78`; `PassiveStat::Lifesteal` `definition.rs:507`; tests `combat.rs:1303-1411` | On-hit bonus damage is `is_spell`, so it gets spell lifesteal, not lifesteal |
| Mana break | **Partial (composable)** | `AbilityEffect::DrainMana` `definition.rs:547-552`; CSV `resource,mana_burn` `generator_common.py:138-146`; `attacker` target `generator_common.py:785-786` | Burn damage is always `DamageEvent::spell` (`vitals.rs:219-226`). Fires on missed attacks |
| Orb / attack modifier | **Partial** | `AbilityTrigger::OnAttack/OnAttackHit` `definition.rs:139-142`; `item_attack_triggers` `items.rs:1143`; examples `data/examples/item_effects.csv:5,10`; tests `abilities/mod.rs:1152-1210` | Items only. Fires on misses. No chance gate |
| Poison attack | **Exists (composable)** | `StatusSpec::Burn` `definition.rs:235-238` → `DamageEvent::dot` `combat.rs:145`; CSV `burn` `generator_common.py:129` | Fires on misses (shared defect) |
| Slow on hit | **Exists (composable)** | `StatusSpec::Slow/AttackSpeedSlow` `definition.rs:254-260,382-397`; example `frost_hammer` | Fires on misses. Shared `"slow"` id refreshes and replaces any stronger slow (`status.rs:686-689`) |
| Armor reduction on hit | **Exists (composable)** | `StatusSpec::Modifier.armor` `definition.rs:292-301`; CSV timed `armor` `generator_common.py:199-206` | Fires on misses |
| Bonus damage on hit | **Partial** | `AbilityEffect::Damage` under `OnAttackHit` → `DamageEvent::spell` `effects.rs:340-345` | Spell amp, spell lifesteal and a second block apply. No "attack bonus" damage kind |
| True strike | **Missing** | No field on `StatusEffect` (`status.rs:76-130`) | — |
| Evasion | **Missing** | Blind read from the *defender* (broken, inverted evasion) | — |
| Accuracy | **Missing** | — | — |
| Double attack | **Missing** | Cooldown set once per release `basic_attack.rs:270-275` | — |
| Multi-target attack | **Missing** | A release has one target `basic_attack.rs:56-60` | — |
| Instant attack | **Missing** | `AbilityMechanic::AttackUnitTarget` `mechanics.rs:171-175` only sets `AttackTarget` | — |
| Attack range modifier | **Missing** | No `PassiveStat` or status field; melee/ranged decided from current range (`scale.rs:120`, `basic_attack.rs:320`) | — |
| Attack speed modifier | **Exists** | IAS formula `components.rs:214-225`; `PassiveStat::AttackSpeed`; `Modifier.attack_speed_*`, `AttackSpeedSlow` | — |
| Attack damage modifier | **Exists** | `PassiveStat::AttackDamage`; `Modifier.attack_damage`; `outgoing_physical_amp` | Damage snapshotted at windup start (unchanged) |
| Attack counter | **Missing** | — | — |
| Attack projectile modifier | **Missing** | Speed derived from range `basic_attack.rs:96-99,350`; fixed `ProjectileStyle::AutoAttack` | — |

---

## 2. Design

### 2.1 Attack lifecycle after this change

```text
request_basic_attacks → start_basic_attack_windups → tick_attack_swings
  → BasicAttackReleaseEvent { extra: false }                    (normal swing, sets cooldown)
release_extra_attacks (NEW) reads ExtraAttackEvent
  → BasicAttackReleaseEvent { extra: true }                     (no cooldown or swing change)
resolve_attack_releases   melee: impact now (extra skips range check) | ranged: BasicAttackInFlight{extra}
advance_attacks_in_flight → BasicAttackImpactEvent{extra}
resolve_basic_attack_impacts  (THE attack-hit stage)
  1 validate target (existing)
  2 hit roll: miss = (1-(1-blind_attacker)(1-evasion_target)) × (1-accuracy_attacker); one CombatRng roll
      miss → stop (no damage, no landed event, no cleave/splash, no OnAttackHit)
  3 crit roll: force_crit || rng.roll(crit_chance) → mult = crit_multiplier.max(1)
  4 DamageEvent::basic_attack(amount = impact.damage × mult).with_crit(crit)
  5 BasicAttackLandedEvent { attacker, attacker_team, target, damage(post-crit, pre-mitigation), crit, extra }
  6 cleave (melee only, attacker-centred) + splash (target-centred): ratio × landed damage,
    DamageKind::Cleave / Splash, never crit, never leech, never trigger anything
[abilities chain, same frame]
attack_triggers (NEW, replaces items::item_attack_triggers)
  OnAttack from releases, OnAttackHit from landed, for every distinct item + every learned ability
  ctx.attack = Some(AttackMeta { extra, crit })
resolve_ability_triggers: FIFO queue; Proc/Counter push derived contexts (depth+1)
  Damage under an attack-derived ctx → DamageEvent::attack_bonus
  PerformAttack → ExtraAttackEvent (skipped when ctx.attack.extra)  → read NEXT frame
apply_status_effect_events (statuses land before this frame's damage)
combat::apply_damage_events: amp → shield/block/mitigation → mana shield → HP → leech → reflect
```

### 2.2 Stacking and ordering rules (binding)

**Damage order for one landed attack**
1. Base = `CombatStats.attack_damage` snapshotted at windup start. Extra attack: `attack_damage × damage_ratio` at release.
2. Crit multiplies **only the main hit**, not on-hit bonus damage, DoTs or burn.
3. Cleave and splash = `ratio × post-crit main damage`, pre-mitigation. Secondary targets do not roll evasion, never crit again, fire no triggers.
4. `apply_damage_events`, unchanged order: outgoing physical amp → shields → block (main hit only; `AttackBonus` has `applies_block: false`) → armor → incoming amp → mana shield → HP.
5. Lifesteal reads health actually removed. Applies to `BasicAttack` **and** `AttackBonus`; never `Cleave`, `Splash`, `Dot`, reflection. Spell lifesteal/amp never apply to `AttackBonus` (`is_spell: false`).

**Hit resolution**
- `blind` = strongest blind on the **attacker**.
- `evasion` = `1 − (1 − clamp(stats.evasion)) × Π(1 − status.evasion)` on the **target**.
- `accuracy` = same stacking on the **attacker**. True strike = accuracy 1.0.
- `miss = (1 − (1−blind)(1−evasion)) × (1 − accuracy)`. True strike ignores evasion and blind.
- Only basic and extra attacks roll to hit.
- Attacker despawned mid-flight: blind, accuracy, crit = 0; no cleave/splash.

**Permanent stat stacking**
- Item/passive stats stay additive (undo by subtraction). Chances and ratios summed, clamped 0..1 at read.
- `crit_multiplier` passive is an additive bonus on top of the 2.0 base.
- Cleave/splash radii use `stats.*_radius` when > 0, else `ATTACK_CLEAVE_RADIUS` / `ATTACK_SPLASH_RADIUS`.

**RNG and determinism**
- Everything uses `CombatRng`. New `CombatRng::roll(chance)`: no draw when `chance <= 0` (false) or `>= 1` (true). `roll_crit` delegates.
- Draw order: per impact in message order, hit roll then crit roll if hit. Per trigger context FIFO: one draw per `Proc`.
- The per-pair blind hash is deleted.

**Recursion guards**
- `PerformAttack` no-ops when `ctx.attack.is_some_and(|a| a.extra)`. Extra attacks still fire `OnAttack`, `OnAttackHit`, procs, counters.
- Derived triggers carry `depth`; `Proc`/`Counter` no-op at `ctx.depth >= MAX_TRIGGER_DEPTH` (2), one `warn!`.
- Cleave, splash, DoT, reflection and burn never send attack events.
- One trigger per distinct `ItemId` and per learned ability per attack.

**Same-frame ordering**
- On-hit statuses apply before `apply_damage_events` in the same frame (armor reduction affects its own hit).
- `PerformAttack` attacks start one frame later.

### 2.3 Why gate effects instead of new triggers or definition fields

Adding chance fields to `AbilityEffectEntry`/`ItemDefinition` touches ~100 literals and cannot share one roll between "stun + damage". `Proc { chance, then: AbilityTrigger::Custom("on_bash") }` rolls once per source per attack, reuses existing custom-trigger support, and works on any trigger. `Counter` follows the same pattern.

---

## 3. Implementation steps (each phase compiles and passes tests)

### Phase 1: attack-hit stage, blind fix, crit moved, landed event
**Touches:** `src/basic_attack.rs`, `src/combat.rs`, `src/items.rs`, `src/abilities/mod.rs` (test helper)

1. `combat.rs`: add `CombatRng::roll`; replace `DamageEvent.can_crit` with informational `crit: bool` + `with_crit`; `resolve_outgoing_amount` drops rng/hooks and crit branch; `apply_damage_events` drops `CombatRng`/`DamageTestHooks`; register `BasicAttackLandedEvent`.
2. `basic_attack.rs`: add
   ```rust
   #[derive(Message, Debug, Clone, Copy, PartialEq)]
   pub struct BasicAttackLandedEvent {
       pub attacker: Entity, pub attacker_team: Team, pub target: Entity,
       /// Post-crit, pre-mitigation main-hit damage.
       pub damage: f32, pub crit: bool, pub extra: bool,
   }
   ```
   Add `extra: bool` to `BasicAttackReleaseEvent`, `BasicAttackImpactEvent`, `BasicAttackInFlight`. Add pure `attack_miss_chance(attacker, target) -> f32` and `stack_chances`. Rewrite `resolve_basic_attack_impacts` per §2.1 (params: `ResMut<CombatRng>`, `Option<Res<DamageTestHooks>>`, `MessageWriter<BasicAttackLandedEvent>`; attacker query includes `Option<&StatusEffects>`). Delete `basic_attack_misses_blind`. Extract `secondary_attack_targets(...)` sorted by `Entity`.
3. `items.rs::item_attack_triggers`: read `BasicAttackLandedEvent` for `OnAttackHit`.
4. Tests: test worlds add `CombatRng`, `DamageTestHooks`, `Messages<BasicAttackLandedEvent>`; `write_attack_hit` writes the landed event; move crit test to `basic_attack.rs`.

### Phase 2: damage kinds for attack-sourced effect damage
**Touches:** `src/combat.rs`, `src/abilities/definition.rs`, `src/abilities/effects.rs`, `src/vitals.rs`

1. `DamageKind::AttackBonus`, `DamageKind::Splash`; constructors `attack_bonus(source, target, amount, damage_type)` (is_spell false, applies_block false, reflects true) and `splash(source, target, amount)`.
2. Leech applies to `BasicAttack | AttackBonus`.
3. `AbilityTrigger::is_attack()`.
4. `TriggerContext.attack: Option<AttackMeta>` with `AttackMeta { extra, crit }` in `effects.rs`, setter `.attack(meta)`.
5. `EffectRunner` `Damage` arm uses `attack_bonus` when `ctx.attack.is_some()`.
6. `ResourceDrainEvent.from_attack: bool`; burn emits `attack_bonus` when set.
7. `item_attack_triggers` sets `AttackMeta`.

### Phase 3: stats
**Touches:** `components.rs`, `heroes.rs`, `status.rs`, `abilities/definition.rs`, `items.rs`, `abilities/tooltip.rs`, `basic_attack.rs`

1. `CombatStats` gains `evasion`, `accuracy`, `splash_radius`, `splash_damage_ratio`, `attack_range_bonus`, `attack_projectile_speed_bonus` (default 0). `attack_range` stays effective range; add `base_attack_range()` and `is_melee()`; use `is_melee()` at `basic_attack.rs:320`.
2. Projectile speed: `projectile_speed_for(base_attack_range()) + attack_projectile_speed_bonus`.
3. `StatusEffect` gains `evasion`, `accuracy`, `attack_range`; iterators `evasions()`/`accuracies()`; range delta applied to both `attack_range` and `attack_range_bonus`.
4. `StatusSpec::Evasion { percent }`, `Accuracy { percent }`, `TrueStrike`; `Modifier.attack_range`.
5. `PassiveStat`: `CritChance`, `CritMultiplier`, `Evasion`, `Accuracy`, `CleaveRatio`, `SplashRatio`, `AttackRange`, `AttackProjectileSpeed` wired through `ItemPassives`, `add_stat`, `AddAssign`, `adjust_passives`, tooltips.
6. `attack_miss_chance` adds evasion/accuracy; splash in the hit stage; cleave requires `is_melee()`; radius fallback constants.

### Phase 4: one attack-trigger system for items and abilities
New `src/abilities/attack_triggers.rs::attack_triggers` replaces `items::item_attack_triggers` in the same chain slot. Items in slot order (dedup by `ItemId`), then learned ability slots (rank > 0) with `EffectSource::Ability(id)`, `rank = slot.rank`.

### Phase 5: gate effects and derived-trigger queue
```rust
Proc { chance: RankValue, then: AbilityTrigger },
Counter { every: u32, then: AbilityTrigger },
```
`TriggerContext.depth: u8`, `MAX_TRIGGER_DEPTH = 2`. `TriggerCounters` component (`Vec<((EffectSource, AbilityTrigger), u32)>`), required by `CombatStats`. `EffectRunner` gains `ResMut<CombatRng>`, counters query, derived `Local` queue, `take_derived()`. `resolve_ability_triggers` processes a FIFO `VecDeque` in the same call.

Bash recipe: `on_attack_hit,attacker,proc,on_bash,25` + `on_bash,trigger_unit,apply_status,stun,...` + `on_bash,trigger_unit,damage,,40,physical`.

### Phase 6: `PerformAttack`
```rust
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ExtraAttackEvent { pub attacker: Entity, pub target: Entity, pub damage_ratio: f32 }
PerformAttack { damage_ratio: RankValue, max_targets: u32 /* 0 = all */, exclude_trigger_unit: bool },
```
`release_extra_attacks` between `tick_attack_swings` and `resolve_attack_releases`; ignores cooldown/swing/range, requires living `can_attack()` attacker and living targetable enemy. Never touches `AttackCooldown`, `AttackSwing`, `AttackTarget`. Melee extra attacks skip the range check. `execute`: skip if extra; drop caster (and trigger unit if flagged); enemies only; sort by distance then `Entity`; truncate.

Recipes: instant `on_cast,cast_target,perform_attack,,100`; double `on_attack_hit,attacker,proc,on_double,25` + `on_double,trigger_unit,perform_attack,,100`; multishot `on_attack,units_in_radius,perform_attack,exclude_trigger_unit,70,,2,radius=600`; every 4th `on_attack_hit,attacker,counter,on_fourth,4`.

### Phase 7: generators, CSV, examples
- `ABILITY_TRIGGERS` += `on_attack`, `on_attack_hit` (no affected units).
- `EFFECT_TYPES` += `proc` (custom trigger with rows; chance % in (0,100]; owner targets), `counter` (integer ≥ 2), `perform_attack` (damage % > 0, default 100; `amount` = max targets; `effect_id` ∈ {blank, `exclude_trigger_unit`}). Warn on unreachable custom triggers.
- `STATUSES` += `evasion`, `accuracy`, `true_strike`. `TIMED_STATS` += `attack_range`.
- `PASSIVE_STATS` += `crit_chance`, `crit_multiplier`, `evasion`, `accuracy`, `cleave`, `splash`, `attack_range`, `attack_projectile_speed` (fractions where applicable).
- Examples in `data/examples/item_effects.csv`: basher, desolator, crit blade, split bow, mana breaker. Regenerate catalogs.

### Phase 8: docs (§5)

---

## 4. Regression tests

**`src/basic_attack.rs`**: `blinded_attacker_misses`, `blind_on_target_does_not_protect_it`, `full_evasion_dodges_and_skips_cleave`, `true_strike_ignores_evasion_and_blind`, `attack_miss_chance_formula` (.5/.5/.5 → 0.375; .25+.5 → 0.625), `seeded_hit_rolls_are_reproducible`, `forced_crit_multiplies_main_hit_once`, `cleave_uses_crit_damage_without_rolling`, `cleave_is_melee_only_with_default_radius`, `splash_hits_units_around_the_target`, `extra_attack_ignores_cooldown_and_swing`, `extra_melee_attack_skips_range_check`, `ranged_extra_attack_flies_flagged_extra`, `attack_range_bonus_keeps_melee_classification`, `projectile_speed_bonus_shortens_travel`.

**`src/combat.rs`**: `crit_is_not_rolled_at_damage_application`, `attack_bonus_leeches_without_spell_amp_or_block`, extend `cleave_reflect_and_dot_do_not_leech` with `Splash`.

**`src/abilities/mod.rs`**: `missed_attack_fires_on_attack_but_not_on_attack_hit`, `on_attack_hit_damage_is_attack_bonus`, `ability_on_attack_hit_needs_a_learned_rank`, `proc_chance_one_always_and_zero_never`, `proc_rolls_once_for_all_derived_entries`, `counter_fires_every_nth_landed_attack`, `counters_are_per_source`, `derived_trigger_depth_is_capped`, `perform_attack_on_cast_is_an_instant_attack`, `double_attack_does_not_chain`, `multishot_excludes_primary_and_caps_targets`, `mana_break_burns_as_attack_bonus`; existing item attack trigger tests still pass.

**`src/items.rs` / `src/status.rs`**: `attack_modifier_passives_apply_and_remove_symmetrically`, `attack_range_modifier_status_reverts_on_expire`, `evasion_status_is_basic_dispellable`.

**`scripts/test_generator_common.py`**: `test_ability_attack_triggers_accepted`, `test_proc_requires_custom_trigger_with_rows`, `test_proc_chance_range`, `test_counter_requires_integer_at_least_two`, `test_perform_attack_columns`, `test_evasion_accuracy_true_strike_statuses`, `test_attack_passive_stat_ranges`, `test_timed_attack_range_modifier`.

```bash
cargo fmt --check && cargo build && cargo test
python3 scripts/test_generator_common.py
python3 scripts/AbilityGenerator.py --check
python3 scripts/ItemGenerator.py --check
```

---

## 5. Permanent docs (README.md)

- Status effects table: fix Blind (attacker-side, `CombatRng`); add Evasion, Accuracy / True strike, Attack range.
- Damage mechanics table: crit rolled once per landed attack; add Splash (attack), Attack bonus damage.
- New **Attack modifiers** section: lifecycle, ordering/stacking rules, recursion guards, recipes.
- Healing and resources: Lifesteal includes `AttackBonus`; Mana burn `from_attack`.
- Ability architecture: derived triggers (`Proc`/`Counter`, FIFO, `MAX_TRIGGER_DEPTH`); abilities receive `OnAttack`/`OnAttackHit`.
- Adding abilities/items: new effect types, statuses, passive stats.
- `2026-11-06-damage-mechanics.md`: note that this plan supersedes crit-at-application.

## 6. Invariants

1. `apply_damage_events` is the only place health changes.
2. One hit roll and at most one crit roll per impact.
3. `OnAttackHit` fires only for landed attacks, once per distinct source.
4. Cleave, splash, DoT, reflect, burn never send attack events.
5. An extra attack never causes another extra attack.
6. All chances use `CombatRng` in fixed order.
7. Extra attacks never change cooldown, swing, `AttackTarget` or movement.
8. Passive stats stay additive and undo exactly.
9. No new `AbilityTrigger` variants; `ItemDefinition`, `AbilityDefinition`, `AbilityEffectEntry` unchanged.

## 7. Risks

- RNG stream change; re-pin seeded crit tests.
- One-frame delay for `PerformAttack`.
- Same-frame statuses before damage (intended).
- Hand-built test apps must register `BasicAttackLandedEvent`, `ExtraAttackEvent`, `CombatRng`.
- `EffectRunner` gains `&mut TriggerCounters` and `ResMut<CombatRng>`; check param conflicts.
- `abilities` imports `basic_attack`; not the reverse.
- Regenerate both catalogs in the same commit.

## 8. Decisions (defaults accepted)

1. True strike also ignores blind — yes.
2. Lifesteal on `AttackBonus` — yes.
3. Extra attacks fire procs, counters, cleave, splash — yes; only `PerformAttack` guarded.
4. Disabled attackers cannot take instant attacks — `can_attack()` required.
5. Plain random, no PRD.
6. Orb uniqueness, autocast orbs, bounce/pierce projectiles — out of scope.
7. Content: examples only in `data/examples/`.
8. Cleave/splash radius constants picked and documented by implementer.
9. Cleave stays a circle around the attacker.
