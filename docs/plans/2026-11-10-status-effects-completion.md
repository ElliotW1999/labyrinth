# Status effects completion: audit and implementation plan

**Date:** 2026-11-10
**Status:** Draft — awaiting approval of open decisions (section 7)
**Builds on:** `docs/plans/2026-11-06-status-effects-catalog.md` (its matrix is out of date; this plan replaces its "Mechanic matrix" and "Implementation status"), `docs/plans/2026-11-07-movement-mechanics.md` (leash, fear, forced movement), `docs/architecture/composition.md` (layers, invariants I1-I11, section 5 gameplay-rule matrix).

---

## 0. Bottom line

- **19 of 33** statuses are implemented and enforced at runtime.
- **4** are partial: Hex, Sleep, Fear and Spell immunity.
- **10** are missing: Break, Taunt, Hypnosis, Banish/Hide, Ethereal, Invisibility, Attack immunity, Spell block, Spell reflection and Reveal.

Every missing status can be built from **six small reusable primitives**, all stored on `StatusEffect` (still the one place statuses live):

1. **`ActionBlocks`**: one derived disable matrix (move, attack, cast, items, orders, passives). It replaces the scattered `is_rooted` / `can_cast` / `DisableRule` logic.
2. **`TargetIntent` filter**: generalises `is_untargetable` into Any / Attack / HostileSpell. It drives attack immunity, spell immunity, banish and invisibility.
3. **`DamageTypeMask` immunity**: replaces the unused `damage_immune: bool`. Ethereal is Physical, spell immunity is Magical.
4. **`Compulsion`**: replaces `fear_source`. Flee, Approach and Attack cover Fear, Hypnosis and Taunt, enforced by one AI-stage system.
5. **Concealment** (`invisible` / `revealed`): one predicate, read by the targeting filter and the fog renderer.
6. **Spell landing check**: one check in `resolve_ability_triggers` handles spell immunity, spell block and spell reflection.

Hex, Ethereal, Banish, Spell immunity and Attack immunity then become **composite constructors only**, with no new systems.

### Test status

At planning time `cargo test` could not run in the cloud environment: the build stops in `wayland-sys` (its build script needs `pkg-config` and `libwayland-client`). This is an environment problem, not a code problem. Install Bevy's Linux dependencies (`pkg-config libwayland-dev libxkbcommon-dev libudev-dev libasound2-dev libx11-dev`) to run the tests.

The generator tests pass: `python3 scripts/test_generator_common.py` ran 163 tests, all OK.

---

## 1. Audit (verified in code, not just docs)

"Enforced" means a runtime system actually reads the flag. Line numbers are as of 2026-11-10.

| # | Status | State | Data / representation | Runtime enforcement (evidence) | Gaps |
|---|---|---|---|---|---|
| 1 | Stun | IMPLEMENTED | `status.rs:431` `stunned()`, `:625` `is_stunned`; `StatusSpec::Stun` `definition.rs:358`; gen `stun` | Move: `is_rooted` includes stun `status.rs:633-637` → `movement.rs:166`, `displacement.rs:292,635`. Attack: `is_disarmed` `:639-643` → `basic_attack.rs:183,275,336`. Cast: `can_cast` `:728`. Items: `resources.rs:88`. Channel / cast point cancelled each frame `actions/pipeline.rs:392-421` | Default dispel tier is Basic (Dota: strong only); see D4 |
| 2 | Mini-stun | IMPLEMENTED | `StatusSpec::OwnedStun{id}` `definition.rs:361,559`; gen `mini_stun` → `<owner>_stun` `generator_common.py:1618-1622` | As stun; tests `abilities/mod.rs:7997`, `:8033` | `tick_status_effects` is unordered relative to `tick_activations` / `tick_attack_swings` (`status.rs:17-22`); a sub-frame stun can expire before any interrupt check (step 1) |
| 3 | Root | IMPLEMENTED | `status.rs:438`, `is_rooted` `:633` | `movement.rs:166`; `displacement.rs:292` | none |
| 4 | Leash | IMPLEMENTED | `Leash` / `LeashAnchor` `status.rs:61-75,406`; `StatusSpec::Leash`; gen `leash`, `tether` | `movement::enforce_leashes` `movement.rs:334-377`; teleport `teleport.rs:157`; tests `movement.rs:762`, `status.rs:2864` | none |
| 5 | Silence | IMPLEMENTED | `status.rs:424` | `DisableRule::SilenceOrStun` → `can_cast` `resources.rs:87`; channel interrupt `pipeline.rs:392` | none |
| 6 | Mute | IMPLEMENTED | `status.rs:461,645` | `DisableRule::MuteOrStun` `resources.rs:88` via `items.rs:482`; test `items.rs:3721` | none |
| 7 | Break | MISSING | none | Passive stats `abilities/passive.rs:88-121` and passive triggers (`reactive_triggers.rs:31`, `attack_triggers.rs:79`) are not gated | everything |
| 8 | Disarm | IMPLEMENTED | `status.rs:445,639` | `basic_attack.rs:183,275,336` | none |
| 9 | Hex | PARTIAL | `status.rs:518` = silence + disarm + mute, Strong dispel; gen `hex` | Via those three flags | No move-speed override, no passive break, no visual. `HexBolt` (`heroes.rs:323`) is a stub |
| 10 | Sleep | PARTIAL | `status.rs:468,629` | Move/attack/cast blocked; wakes on damage `combat.rs:443-458` → `wake_from_sleep` `status.rs:1132` | Items still usable while asleep (`resources.rs:80,88`); D1 |
| 11 | Fear | PARTIAL | `status.rs:509` (`fear_source`), `is_feared` `:649-660` | Steering only in `apply_move_targets` `movement.rs:176-190`, which only iterates units that already have a `MoveTarget` | Standing feared unit does not flee; can still attack/cast/use items; orders and AI overwrite it; phased/forced source fallback bug (`movement.rs:144-147`); no tests |
| 12 | Taunt | MISSING | `AbilityId::Taunt` stub only (`components.rs:697`, `heroes.rs:291`) | none | everything |
| 13 | Hypnosis | MISSING | none | none | everything |
| 14 | Slow | IMPLEMENTED | `StatusSpec::Slow{id,percent}`; `move_slow_fraction` `status.rs:1034-1040` | `effective_move_speed` `components.rs:359-367` → `movement.rs:162` | none |
| 15 | Attack speed slow | IMPLEMENTED | `StatusSpec::AttackSpeedSlow`; `Modifier.attack_speed_flat` | `recompute_attack_speed` `status.rs:975,997` | Fixed id → two sources overwrite (optional step 7) |
| 16 | Turn rate slow | IMPLEMENTED | `StatusSpec::TurnRateSlow` | `effective_turn_rate` `components.rs:371-392` → `movement.rs:280`, `pipeline.rs:438`, `basic_attack.rs:224` | none |
| 17 | Blind | IMPLEMENTED | `status.rs:502,662`; gen `blind` | Miss roll `basic_attack.rs:530-548,576`; tests `basic_attack.rs:1150,1160,1301` | none |
| 18 | Cyclone | IMPLEMENTED | `status.rs:487`; `DisplacementKind::Cyclone` | `is_untargetable` (`has_status("cyclone")`, `status.rs:716`) read by ~20 filters; tests `displacement.rs:1583+` | Untargetability keyed on id, not reusable (step 2) |
| 19 | Banish / Hide | MISSING | none | none | everything |
| 20 | Ethereal | MISSING | `damage_immune` field exists but is never set (`status.rs:157`, read `combat.rs:461`) | none | everything |
| 21 | Invisibility | MISSING | none | `fog.rs:189-251` is render-only and ignores statuses | everything |
| 22 | Invulnerability | IMPLEMENTED | `status.rs:475,703`; gen `invulnerable` | `combat.rs:439`; `vitals.rs:200`; test `abilities/mod.rs:5257` | none |
| 23 | Debuff immunity | IMPLEMENTED | `status.rs:452` (runtime id `"spell_immunity"`), `:686` | Rejects debuffs `status.rs:1022`; purge on gain `:1223-1238`; blocks hostile displacement/teleport | Id naming confusing |
| 24 | Spell immunity | PARTIAL | Alias of debuff immunity (`generator_common.py:203`) | Debuff blocking only | No magic damage immunity; hostile unit-target spells still target |
| 25 | Attack immunity | MISSING | none | none | everything |
| 26 | Spell block | MISSING | none | none | everything |
| 27 | Spell reflection | MISSING | none | none (`damage_reflect` is passive damage return) | everything |
| 28 | Phased | IMPLEMENTED | `status.rs:387,597` | `movement.rs:146,241,472`; `teleport.rs:173,185` | none |
| 29 | Reveal | MISSING | none | none | everything |
| 30 | Basic dispel | IMPLEMENTED | `DispelStrength` / `DispelType` `status.rs:39-59`; `status_dispel_matches` `:742` | `StatusChange::Dispel` `status.rs:1276-1292`; test `status.rs:1925` | none |
| 31 | Strong dispel | IMPLEMENTED | same, `amount=1` | same; test `abilities/mod.rs:9390` | none |
| 32 | Undispellable | IMPLEMENTED (runtime) | `undispellable` / `DispelType::None` | `status_dispel_matches` `:750` | CSV rows cannot set dispel tier (optional step 7) |
| 33 | Status resistance | IMPLEMENTED | `status.rs:297,569`; `StatusSpec::StatusResistance` | `apply_status_effect_events` `status.rs:1200-1222`; tests `status.rs:3222-3337` | none |

---

## 2. Design: reusable primitives

All new fields go on `StatusEffect` with defaults in `StatusEffect::buff` (`status.rs:197-255`). New statuses are **constructors** on `StatusEffect` plus a `StatusSpec` variant, not new systems, unless noted.

### P1. `ActionBlocks`: the disable matrix (`status.rs`)

`u8` newtype (no `bitflags` dependency):

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActionBlocks(u8);
impl ActionBlocks {
    pub const MOVE: Self; pub const ATTACK: Self; pub const CAST: Self;
    pub const ITEMS: Self; pub const ORDERS: Self; pub const PASSIVES: Self;
    pub fn contains(self, other: Self) -> bool; pub fn union(self, other: Self) -> Self;
}
impl StatusEffect   { pub fn blocks(&self) -> ActionBlocks }   // derived from flags
impl StatusEffects  { pub fn blocks(&self) -> ActionBlocks }   // union over live effects
```

| Flag / field | MOVE | ATTACK | CAST | ITEMS | ORDERS | PASSIVES |
|---|---|---|---|---|---|---|
| `stunned` | x | x | x | x | | |
| `asleep` | x | x | x | x (D1) | | |
| `rooted` | x | | | | | |
| `silenced` | | | x | | | |
| `muted` | | | | x | | |
| `disarmed` | | x | | | | |
| `breaks_passives` (new) | | | | | | x |
| `compulsion` Flee / Approach (new) | | x | x | x | x | |
| `compulsion` Attack (taunt, new) | | | x | x | x | |

Rewrite `is_rooted` (MOVE), `is_disarmed` (ATTACK), `can_move`, `can_attack`, `can_cast` (!CAST) and `DisableRule::blocks` (`resources.rs:85-91`: `SilenceOrStun` → CAST, `MuteOrStun` → ITEMS) on top of `blocks()`, keeping signatures.

### P2. Targeting filter (`status.rs`)

New fields: `untargetable`, `attack_immune`, `spell_immune`, `hidden`, `invisible`, `revealed`, `invis_breaks_on_action`.

```rust
pub enum TargetIntent { Any, Attack, HostileSpell }
impl StatusEffects {
    pub fn is_concealed(&self) -> bool;          // any invisible && !any revealed
    pub fn rejects(&self, intent: TargetIntent) -> bool;
    // Any:          untargetable
    // Attack:       untargetable || attack_immune || concealed
    // HostileSpell: untargetable || spell_immune  || concealed
}
pub fn is_targetable(statuses: &Query<&StatusEffects>, e: Entity, intent: TargetIntent) -> bool;
```

- `is_untargetable()` reads the `untargetable` field instead of `has_status("cyclone")`; the free function stays as the `Any` wrapper, so area call sites are unchanged. Area effects still hit invisible units.
- `TargetIntent::Attack` at `basic_attack.rs:160,343,379,490,572,689,746`, `ai.rs:91,187`, `unit_commands.rs:150-155` (split `target_alive` into Attack for `UnitCommand::Attack` and Any for `Activate`).
- `TargetIntent::HostileSpell`: `actions/targeting.rs` `UnitInfo` gains `rejects_hostile_spell`, filled at `pipeline.rs:184`; `resolve_target` rejects enemy targets with it set.

### P3. Damage-type immunity (`status.rs`, `combat.rs`)

Replace `damage_immune: bool` with `immune_damage: DamageTypeMask` (`PHYSICAL`, `MAGICAL`, `PURE`, `ALL`). `has_damage_immunity()` → `is_immune_to(DamageType)`; `combat.rs:461` passes the event's damage type.

### P4. Compulsion (`status.rs`, `ai.rs`, `movement.rs`, `unit_commands.rs`, `basic_attack.rs`)

```rust
pub enum Compulsion { Flee { from: Option<Entity> }, Approach { toward: Entity }, Attack { target: Entity } }
// StatusEffect: `compulsion: Option<Compulsion>` replaces `fear_source`.
impl StatusEffects { pub fn compulsion(&self) -> Option<Compulsion> } // last-applied live one wins
```

- Constructors `fear` (Flee), `taunted` (Attack), `hypnotized` (Approach). Delete `is_feared` / `fear_flee_source`.
- New `enforce_compulsions` in `ai.rs` between `player_attack_move` and `assign_attack_positions`; the sole order writer for compelled units: clear `CommandQueue`, remove `AttackMoveOrder`; Flee/Approach remove `AttackTarget`/`AttackPositionGoal` and insert `MoveTarget` (900 units away / source position) if `can_move()`; Attack inserts `AttackTarget(target)` if alive and attack-targetable.
- Skip compelled units in `acquire_targets` and `player_attack_move`.
- `process_issued_commands` drops commands under ORDERS.
- `apply_move_targets`: generalise `fear_waypoint` → `compelled_waypoint`; Approach stops at contact distance; build source positions from all units (fixes phased-source fallback). Keep the path bypass for Flee/Approach.
- `start_basic_attack_requests`: taunted units may only pick the taunt target.
- Remove `StatusBuildContext.fear_source`; use `ctx.source`.

### P5. Concealment and reveal (`status.rs`, `fog.rs`)

- Targeting through P2.
- New `break_invisibility` system (pattern of `consume_attack_charges`), reading `BasicAttackReleaseEvent` (non-extra) and `ActionFiredEvent`, emitting `StatusChange::Remove` for `invisible && invis_breaks_on_action` statuses. Scheduled next to `consume_attack_charges`.
- `fog.rs::update_unit_fog_visibility`: `hidden` → Hidden for everyone; concealed enemies Hidden even inside vision.
- True sight is content: interval/proximity trigger + `units_in_radius` + `apply_status,reveal`.

### P6. Spell landing: block, reflect, immunity (`abilities/effects.rs`, `mechanics.rs`)

- New fields `spell_block`, `spell_reflect`; `reflected: bool` on `TriggerContext` (inherited), `CastContext`, `ProjectilePayload`.
- `landing_target(ctx, defs)` returns `ctx.cast_target` when depth 0, not reaction, source target type is `Unit`, and either `OnProjectileHit` on the cast target or `OnCast` with no homing projectile mechanic.
- In `resolve_ability_triggers`, for a hostile landing target: spell immune → shield; else unused live `spell_block` → shield + `StatusChange::Remove`; live `spell_reflect` and `!reflected` → `SpellReflectEvent`. Shielded target is removed from every entry's targets.
- `SpellReflectEvent` consumed by `execute_ability_casts` / `execute_item_mechanics`, building a `CastContext { caster: reflector, unit_target: original caster, reflected: true, .. }`; no `AbilityCastEvent`, no cooldown, skips `CustomAbilityBehaviors`. Reflected casts are never re-reflected.

### Composite constructors

| Status | Composition | Spec / generator id | Default kind |
|---|---|---|---|
| Spell immunity | `debuff_immune + spell_immune + immune_damage=MAGICAL`, undispellable, id `spell_immunity` | `StatusSpec::SpellImmunity`, `spell_immunity` | buff |
| Debuff immunity | unchanged, id → `debuff_immunity` | `debuff_immunity` | buff |
| Attack immunity | `attack_immune` | `StatusSpec::AttackImmunity`, `attack_immunity` | buff |
| Ethereal | `disarmed + attack_immune + immune_damage=PHYSICAL + magic_resist -= amp` | `StatusSpec::Ethereal { magic_amp_percent }`, `ethereal` | follows hostility |
| Banish | `stunned + invulnerable + untargetable + hidden + ignore_unit_collision`, `DispelType::None` (D5) | `StatusSpec::Banish`, `banish` | follows hostility |
| Cyclone | add `untargetable: true` | existing | n/a |
| Hex | add `breaks_passives`, `move_speed_cap: HEX_MOVE_SPEED` | existing `hex` | debuff |
| Break | `breaks_passives` | `StatusSpec::Break`, `break` | debuff |
| Fear / Taunt / Hypnosis | `compulsion` | `Fear`, `Taunt`, `Hypnosis` | debuff |
| Invisibility | `invisible`, `invis_breaks_on_action` (default true; `amount=0` keeps it) | `StatusSpec::Invisibility { breaks_on_action }`, `invisibility` | buff |
| Reveal | `revealed` | `StatusSpec::Reveal`, `reveal` | debuff |
| Spell block | `spell_block` | `StatusSpec::SpellBlock`, `spell_block` | buff |
| Spell reflect | `spell_reflect` | `StatusSpec::SpellReflect`, `spell_reflect` | buff |

- **Move-speed cap:** `move_speed_cap: f32` (0 = none); `effective_move_speed` applies `.min(lowest live cap)` after the floor. Add `scale::HEX_MOVE_SPEED`.
- **Kind follows hostility:** `StatusSpec::kind_follows_hostility()`; `EffectRunner::execute` `ApplyStatus` sets Debuff if hostile else Buff (mirrors `cyclone(duration, hostile)`).
- **Break enforcement** via `passive_suppressed(source, defs, statuses)` in `abilities/passive.rs` (abilities with `AbilityBehavior::Passive` only; D2): in `sync_ability_passives` (sync key includes broken flag), in `resolve_ability_triggers` (drop root non-cast-phase contexts from suppressed sources), in `sync_conditional_statuses`.

---

## 3. Ordered incremental steps

### Step 1: ActionBlocks, sleep/items, mini-stun ordering (P1)
- `src/status.rs`, `src/actions/resources.rs`; order `tick_status_effects` `.after(actions::tick_activations).after(basic_attack::tick_attack_swings)`.
- Tests: `blocks_matrix_matches_documented_table`, `sleep_blocks_items`, `asleep_unit_cannot_use_item`, `sub_frame_stun_still_interrupts_channel`; regression `items.rs:3721,3739`, `abilities/mod.rs:4774,4845,8033`.

### Step 2: Targeting and damage-type primitives (P2, P3) + composite immunities
- `status.rs`, `combat.rs`, `basic_attack.rs`, `ai.rs`, `unit_commands.rs`, `actions/targeting.rs`, `actions/pipeline.rs`, `abilities/definition.rs`, `abilities/effects.rs`, `fog.rs`, `abilities/tooltip.rs`, `scripts/generator_common.py`, `scripts/test_generator_common.py`.
- Tests: `cyclone_untargetable_via_field`, `damage_mask_contains`, `spell_immunity_rejects_debuffs_and_magic_only`, `ethereal_kind_follows_hostility`, `ethereal_takes_no_physical_and_amplified_magic`, `spell_immune_takes_physical_not_magical`, `attack_immune_target_not_acquired_and_swing_cancelled`, `ethereal_attacker_cannot_attack`, `hostile_spell_rejected_on_spell_immune_but_ally_cast_allowed`, `area_spell_still_hits_spell_immune_but_deals_no_magic`, `banished_unit_is_untargetable_invulnerable_and_phased`, fog hidden test.

### Step 3: Break and Hex completion
- `status.rs`, `components.rs`, `scale.rs`, `abilities/passive.rs`, `abilities/effects.rs`, `abilities/reactive_triggers.rs`, `definition.rs`, `tooltip.rs`, `generator_common.py`.
- Tests: `break_removes_passive_ability_stats_and_restores_on_expiry`, `break_suppresses_on_attack_hit_passive`, `break_drops_while_condition_status`, `break_does_not_affect_item_passives`, `hexed_unit_capped_move_speed_and_broken`.

### Step 4: Compulsions (Fear fix, Taunt, Hypnosis)
- `status.rs`, `definition.rs`, `effects.rs`, `ai.rs`, `movement.rs`, `unit_commands.rs`, `basic_attack.rs`, `tooltip.rs`, `generator_common.py`.
- Tests: `feared_idle_unit_flees_from_source`, `fear_from_phased_source_flees_away_not_backward`, `hypnotized_unit_walks_to_source_and_stops_at_contact`, `rooted_feared_unit_does_not_move`, `orders_dropped_while_compelled`, `taunted_creep_attacks_only_source_even_when_others_closer`, `taunted_hero_cannot_cast_or_use_items`, `taunt_ends_when_source_dies`, `feared_unit_cannot_attack_or_cast`.

### Step 5: Invisibility and Reveal
- `status.rs`, `abilities/mod.rs` scheduling, `fog.rs`, `definition.rs`, `tooltip.rs`, `generator_common.py`.
- Tests: `revealed_invisible_is_not_concealed`, `attack_release_breaks_invisibility`, `cast_breaks_invisibility_unless_flagged`, `creeps_do_not_acquire_invisible_enemy`, `cannot_unit_target_concealed_enemy`, `area_damage_hits_invisible_unit`, `concealed_enemy_hidden_inside_vision`.

### Step 6: Spell block and reflection
- `status.rs`, `abilities/effects.rs`, `abilities/mechanics.rs`, `combat.rs`, `abilities/mod.rs`, `definition.rs`, `tooltip.rs`, `generator_common.py`.
- Tests: `spell_block_negates_unit_target_and_is_consumed`, `spell_block_blocks_projectile_on_hit_not_on_launch`, `two_spells_same_frame_only_first_blocked`, `spell_block_ignores_aoe_and_ally_casts`, `reflect_recasts_on_original_caster_once`, `two_reflectors_do_not_ping_pong`, `reflected_cast_fires_no_cast_triggers_or_cooldown`, `item_and_ability_sources_both_blockable`.

### Step 7 (optional): authoring polish
- Per-row dispel override (`apply_status,stun:strong` / `:undispellable`) via `StatusSpec::WithDispel { inner: &'static StatusSpec, dispel }`.
- Per-owner `attack_speed_slow` id.
- Generator tests for each new keyword.

After generator changes, regenerate (`python3 scripts/AbilityGenerator.py`, `ItemGenerator.py`, `HeroGenerator.py`) and confirm `git diff src/abilities/generated.rs src/item_catalog.rs` shows only intended changes.

---

## 4. Invariants to preserve

- I1/I4: statuses change only via `StatusEffectEvent`, `tick_status_effects`, `wake_from_sleep`. Block consumption and invisibility breaking use `StatusChange::Remove`.
- One disable matrix: callers use `blocks()`, `rejects(intent)`, `is_immune_to`.
- Single order writer for compelled units (`enforce_compulsions`).
- Reflection is finite; blocked targets produce no `SpellHitEvent`.
- Area effects ignore concealment, spell immunity and attack immunity; only `untargetable` removes units from areas.
- Every new `StatusSpec` ships with a generator keyword, a tooltip arm and a `STATUSES` row.

## 5. Risks

- Reflect resolves one frame late; same-frame double landings handled by a per-call consumed list.
- Compulsion end leaves units idle; keep path bypass for Flee/Approach to avoid per-frame replanning.
- Break root filter must not drop cast-phase contexts of active abilities.
- Behaviour changes: sleep blocks items (D1); debuff-immunity runtime id rename.
- `StatusEffects` is not replicated (`src/net/`), so concealment rendering is only correct on the sim authority (known limitation).

## 6. Docs to update

- `docs/architecture/composition.md`: section 5 matrix (ActionBlocks table, immunity/concealment/spell-landing rows), section 3 frame order, section 4 `move_speed_cap`, section 1 gameplay-rules row.
- `docs/plans/2026-11-06-status-effects-catalog.md`: header note pointing here; matrix superseded.
- Doc comments: `DisableRule`, `StatusSpec` variants, `fog.rs` module doc.

## 7. Open decisions

- **D1:** Sleep blocks item use? Recommended: yes.
- **D2:** Break suppresses item passives? Recommended: no (abilities only).
- **D3:** Reflection model: Lotus-style (both affected) or Mirror-style (reflect instead of land)? Plan assumes Lotus.
- **D4:** Stun/sleep/taunt/fear default to Strong dispel? Today Basic.
- **D5:** Banish undispellable? Plan assumes yes.
- **D6:** Hex model swap visual: out of scope?
- **D7:** Fear "along route" (`Compulsion::FleeTo(Vec2)`): now or later?
- **D8:** Give `HexBolt` / `Taunt` hero stubs content rows now?

## 8. How to run the tests

```bash
cargo test
cargo test status::
cargo clippy --all-targets
python3 scripts/test_generator_common.py
```
