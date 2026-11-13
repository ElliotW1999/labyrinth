# Abilities

### Ability ranks

- Level-ups grant skill points (start with 1 at level 1)
- Basics: up to 7 ranks; rank N needs hero level ≥ 2N−1 (max at 13)
- Ultimates: up to 4 ranks; unlocked at overall levels 6 / 12 / 18 / 24
- Each rank scales damage, cooldown, range, and mana cost
- **Rank extrapolation for new heroes:** when a spec gives values up to rank 5 (or 3 for an ultimate) and the rest are unspecified, ranks 5, 6, 7 (ultimate rank 4) each add **half a step** (rank 4 + half a step at rank 5 and so on); existing heroes keep their full-step lists
- **Unit-targeted** spells (Bolt, Execute) require clicking a creep or hero
- **Blink**, **Seismic Slam**, **Arcane Lance**, **Cataclysm**, **Stone Skin**, **Overcharge** are generated from `data/abilities.csv` + `data/ability_effects.csv` (Stone Skin's passive armor is applied in `abilities/passive.rs`; Overcharge is pure data: `toggle_mana_per_sec` plus `while_toggled` statuses). Seismic Slam, Arcane Lance, Cataclysm, Stone Skin and Overcharge are test content: they are not in any hero loadout

### Ability architecture

The six composition layers (activation, mechanics, triggers, effects, modifiers, gameplay rules), their interfaces, frame order, the gameplay-rule matrix, worked examples and the checklist for adding a mechanic live in [`docs/architecture/composition.md`](docs/architecture/composition.md).

Mechanics decide **when** something happens and **who** is involved; effects decide **what** happens to them.

```text
AbilityDefinition (abilities/catalog.rs, AbilityDefinitions resource)
AbilityState      (AbilityLoadout on the hero: level, cooldown, charges, toggled)
input / AI → ActivationRequest { caster, source: Ability(slot) | Item(slot), target }   (actions/)
           → validate (source available, cooldown + charges, mana, caster state, target, range)
           → cast point → ActionFiredEvent → AbilityCastEvent / item OnUse → channel → backswing
           → mechanics: Dash / Projectile / AttackUnitTarget / RingFx / custom behavior
           → AbilityTriggerEvent(TriggerContext)          OnCast, OnProjectileHit, OnImpact, OnChannelEnd / OnChannelInterrupted, WhileToggled, Custom("on_…")
                                                          (every context is built by TriggerContext::new)
           → effect entries whose trigger matches → resolve EffectTarget
           → DamageEvent / HealEvent / StatusEffectEvent  (shared systems apply mitigation, immunity, stats;
                                                          statuses live in status.rs and change only through StatusEffectEvent)
spell projectile → ProjectileHitEvent { primary, splashed } → OnProjectileHit (struck) / OnImpact (landed)
```

- **Definitions** (`abilities/definition.rs`) are data: behavior, target type, target team, rank-scaled cast range, AoE, cooldown, mana, cast time, a list of `AbilityMechanic`s, and a list of `AbilityEffectEntry { trigger, target, effect }`
- **Mechanics** (`abilities/mechanics.rs`) move units and spawn projectiles. They never apply consequences; they write an `AbilityTriggerEvent` whose `TriggerContext` carries the caster, cast target, trigger unit, affected units, and trigger point
- **Effects** (`abilities/effects.rs`): `resolve_ability_triggers` runs every entry tagged with the fired trigger. Targets: `Caster`, `CastTarget`, `TriggerUnit`, `AffectedUnits` (e.g. splash victims), `UnitsInRadius`. Effects: `Damage` (optional `EffectScaling`), `Heal`, `ApplyStatus`, `Dispel`, and `Custom { id }` for systems registered with `app.register_custom_effect(id, system)`
- **Projectiles** (`combat.rs`) know their source, target / aim, speed, and collision / splash radius only
- **Activation** (`actions/`) is shared by abilities and active items: target types (no target, self, unit, point, area, direction), team rules (enemy / ally / both / self), range, cast point, channel, mana cost, cooldown, and charges. Out-of-range activations walk into range and re-request. Mana and cooldown are paid when the action fires. `ActionSources` asks the ability layer (learned / castable, rank) or the item layer (the inventory slot holds that active, per-copy cooldown and charges). Items carry an instance id, so two copies keep separate state and an in-flight use follows its copy if it changes slots
- **Casting** (`abilities/casting.rs`) is the ability layer on top: learned / passive gating, the `ActionSpec` at the slot's rank, toggle state, and turning fired actions into `AbilityCastEvent`s and channel ends into `OnChannelEnd` triggers (`action_channel_end_triggers`, for both abilities and items). Abilities (`DisableRule::SilenceOrStun`, via `can_cast()`) are blocked and interrupted by silence, stun or sleep (hex counts as silence); mute alone does not stop them. Items (`DisableRule::MuteOrStun`) are blocked and interrupted by mute or stun; silence and sleep do not stop them
- **Passive abilities** (`abilities/passive.rs`): `ability_type=passive` with `passive` effect rows (`stat_modifier` → `AbilityEffect::PassiveStat`) apply on loadout init and rank-up, mirroring item passives. **Toggle upkeep:** `ability_type=toggle` flips `AbilityState.toggled` on each cast (cooldown starts each flip); a toggle drains `toggle_mana_per_sec` while on and turns off at 0 mana (no cooldown). Its `while_toggled` `stat_modifier` rows are ordinary permanent, undispellable statuses applied and removed through `StatusEffectEvent` by `tick_toggle_upkeep`; ranking up while on keeps the old values until the toggle is cycled
- **Derived triggers:** `AbilityEffect::Proc { chance, then }` and `Counter { every, then }` re-fire the same context under the custom trigger `then`. `resolve_ability_triggers` drains a FIFO queue, so derived contexts resolve in the same call; each carries `depth` and gates stop at `MAX_TRIGGER_DEPTH` (2). `Proc` makes one `CombatRng` draw per execution (one roll covers every entry under `then`); `Counter` state lives in `TriggerCounters`, per source and `then`
- **Chain gate:** `AbilityEffect::Chain { jumps, radius, falloff, team, then }` jumps from each start unit to the nearest valid unit (alive, targetable, not yet hit, ties broken by `Entity`) and re-fires the context under `then` per hop with `TriggerContext::magnitude *= falloff^hop`; it works for abilities and items. Hops clear the attack metadata, so chained damage is spell damage. Hops inherit the reaction of the context that started the chain (reactive hop damage fires no damage triggers), are **not** spell hits (`OnSpellHit` counts only the depth-0 target), and non-reactive hop damage does fire `on_damage_dealt` / `on_damage_taken` on the next frame
- **Zone gate:** `AbilityEffect::Zone { radius, team, delay, duration, interval, then }` spawns one `AbilityZone` per execution at the trigger point (the aim for `on_cast`) via `SpawnZoneEvent`. After `delay` seconds it re-fires the context under `then` every `interval` seconds, `floor(duration / interval)` times in total (tick `k` is due at `delay + k x interval`; overdue ticks catch up in one frame), with the living, targetable units of `team` inside `radius` (never the caster) as the affected units; the tick is written even when nobody is inside. The tick context is the spawning context one hop deeper with the trigger unit, affected units and attack cleared; it keeps the reaction flag, so a zone spawned by a reaction stays reactive. Ticks are depth 1 or more, hence not spell hits, and are not damage over time, so their damage fires `on_damage_dealt` / `on_damage_taken`. The zone is dropped at `delay + duration`, or when the caster entity no longer exists (it outlives the caster's death). With `follow_caster` (the `attached_zone` effect type) the zone is re-centred on the caster before every tick (its centre, transform and the context's point and origin), so `units_in_radius` rows under the tick centre on the caster too, and it is despawned as soon as the caster dies instead of outliving it. The gate does nothing at `MAX_TRIGGER_DEPTH`. It works for abilities and items; a nova ring marks the first tick when render assets exist
- **Attack triggers:** `abilities/attack_triggers.rs` fires `OnAttack` (every release) and `OnAttackHit` (landed attacks only) for every distinct item and every learned ability (rank above 0), with `TriggerContext::attack = Some(AttackMeta { extra, crit })`
- **Reactive triggers:** `abilities/reactive_triggers.rs` holds small dispatcher systems. Each reads one authoritative message and writes ordinary `AbilityTriggerEvent`s into the same queue (no dispatcher applies an effect). All of them share `owner_sources` (items in slot order, one per distinct item, then learned abilities), so every distinct item and learned ability gets one trigger per occurrence.
- **Autocast:** not implemented (no CSV column or AI heuristic yet)

#### Reactive triggers

| Trigger (CSV) | Fires when | Source message | Resolves |
| --- | --- | --- | --- |
| `on_damage_dealt` | the owner's damage hurt a unit (trigger unit = victim) | `DamageAppliedEvent` | next frame |
| `on_damage_taken` | the owner was hurt (trigger unit = attacker; `attacker` target) | `DamageAppliedEvent` | next frame |
| `on_health_below:P` | one hit took the owner from at or above P% to below it and it survived; rearms once health is back above P% (stateless crossing) | `DamageAppliedEvent` | next frame |
| `on_kill` / `on_death` | the hit that took health from above zero to zero (killer is the source of that hit, DoT sources included); once per death | `DamageAppliedEvent` | next frame |
| `on_spell_cast` | the owner finished casting an ability; every other source reacts, never the cast ability itself; item uses do not count | `AbilityCastEvent` | same frame |
| `on_spell_hit` | an effect of the owner's cast resolved onto an enemy (one per enemy per cast context, damage or not; derived, reactive and attack contexts never count) | `SpellHitEvent` | next frame |
| `on_status_applied[:buff\|debuff\|id]` / `on_status_removed[:...]` | a status was applied to / left the owner | `StatusLifecycleEvent` | applied next frame, expiry same frame |
| `on_cooldown_ready` | the source's cooldown finished (never for a zero cooldown or a charge restore) | `CooldownReadyEvent` | same frame |
| `on_interval:S` | every S seconds (at least 0.1) of the owner's life; copies of an item share one timer; paused while dead; at most once per frame | `TriggerTracking` | same frame |
| `aura` | pulsed every 0.25 s from the first frame the source is held (copies share one timer; paused while dead; an ability aura also while Break suppresses it, items ignore Break); rows apply short-lived silent statuses (`AURA_LINGER` 0.5 s) to units in a radius | `TriggerTracking` | same frame |
| `on_move` | the owner moved; `amount` is the distance this frame (teleports excluded, forced movement counts) | `TriggerTracking` | same frame |
| `on_enemy_enter:R` / `on_enemy_leave:R` | a living, targetable enemy came within / left R; units in range when the source is first held count as entering; a death or despawn drops silently | `TriggerTracking` | same frame |
| `while:<condition>` | held `apply_status` / `stat_modifier` rows while the condition is true (`health_below:P`, `moving`, `stationary`, `enemies_near:R:N`, `allies_near:R:N`, `has_status:id`) | `sync_conditional_statuses` | same frame |

**The reaction rule.** Triggers caused by damage or statuses (`on_damage_dealt`, `on_damage_taken`, `on_health_below`, `on_kill`, `on_death`, `on_status_applied`, `on_status_removed`) set `TriggerContext::reaction`, and derived contexts inherit it. Under a reaction `EffectRunner` stamps `reactive` on every `DamageEvent`, `ResourceDrainEvent`, `StatusEffectEvent` and applied `StatusEffect`. Reactive damage and statuses never fire dealt, taken or status triggers, so two thorn items cannot ping-pong. `PerformAttack` does nothing under a reaction. Kill, death and the health crossing still fire for reactive causes because each is a bounded edge. DoT ticks never fire dealt or taken but count for kill, death and low health. Custom effects receive `ctx.reaction` and must pass it on (a custom effect that emits damage or statuses should set `.reactive(ctx.reaction)`).

**Frame delays.** Damage-class reactions, status application and spell hits resolve one frame after the cause (the same delay `PerformAttack` has); a reaction therefore cannot cancel the lethal hit. Cast, attack, cooldown, interval, move, proximity and expiry reactions resolve in the same frame.

**Gates.** `RequireStatus { id, then }` (`require_status`, `effect_id` = `on_<trigger>:<status_id>`: fires `then` once per execution while the caster holds a live status with that id; no RNG or counters, so a refreshing status never stacks; this is how an active's `on_attack_hit` rows run only while its buff is up), `Proc { chance, then, pseudo }` (a pseudo-random proc raises the chance by a constant after every failure and resets on success, so the long-run rate is still `chance` with short streaks; failures are kept in `TriggerCounters`, per owner, source and `then`), `Counter { every, then }`, and `Accumulate { threshold, then }`, which sums `TriggerContext::amount` (damage, distance, or 1) and fires once each time the sum reaches the threshold, keeping the remainder. `amount` is post-mitigation damage for damage triggers, the landed damage for `on_attack_hit`, the distance for `on_move`, otherwise 1. All gates stop at `MAX_TRIGGER_DEPTH`.

### Attack modifiers

```text
request_basic_attacks -> windup -> tick_attack_swings -> BasicAttackReleaseEvent { extra: false } (sets cooldown)
release_extra_attacks (ExtraAttackEvent)  -> BasicAttackReleaseEvent { extra: true }  (no cooldown / swing / AttackTarget change)
resolve_attack_releases (melee: impact now, extra skips the range check | ranged: BasicAttackInFlight)
resolve_basic_attack_impacts = the attack-hit stage:
  validate -> hit roll -> crit roll -> main DamageEvent -> BasicAttackLandedEvent -> cleave / splash
attack_triggers (OnAttack / OnAttackHit) -> resolve_ability_triggers (Proc / Counter / PerformAttack)
```

Rules (binding):

- **Order:** base damage (snapshotted at windup; extra attacks use `attack_damage x ratio`), crit on the main hit only, cleave / splash = ratio x post-crit main hit, then `apply_damage_events` (amp, shield, block (main hit only), armor, incoming amp, mana shield, HP), then leech from health actually removed
- **Hit roll:** `miss = (1 - (1 - blind)(1 - evasion)) x (1 - accuracy)`; blind from the attacker, evasion from the target, accuracy from the attacker. One `CombatRng` draw; a miss stops everything (no damage, no landed event, no secondary hits, no `OnAttackHit`). An attacker that despawned mid-flight has no blind, accuracy or crit and does not cleave
- **Stacking:** item / passive stats are additive and undo exactly; chances combine as `1 - prod(1 - c)`; `crit_multiplier` passive is a bonus on top of 2.0
- **RNG:** only `CombatRng`, in fixed order (hit, crit per impact in message order; one draw per `Proc` in queue order); `CombatRng::roll` draws nothing at chance <= 0 or >= 1. A `pseudo_proc` row uses the same single draw per execution, with a chance that grows with failures (PRD)
- **Recursion guards:** `PerformAttack` does nothing under a trigger caused by an extra attack (extra attacks still fire `OnAttack`, `OnAttackHit`, procs and counters) and nothing under a reaction (see Reactive triggers); derived triggers stop at depth 2; cleave, splash, DoT, reflection and burn never send attack events; one trigger per distinct item and per learned ability per attack. Extra attacks start the frame after the effect that requested them

Recipes (CSV rows, `trigger,target,effect_type,effect_id,value,...`):

| Mechanic | Rows |
| --- | --- |
| Crit | passive `crit_chance` (fraction) and `crit_multiplier` (bonus on 2.0) |
| Bash | `on_attack_hit,attacker,proc,on_bash,25` + `on_bash,trigger_unit,apply_status,stun,,,1.2` + `on_bash,trigger_unit,damage,,40,physical` |
| Orb / bonus damage | `on_attack_hit,trigger_unit,damage,,40,magical` (attack bonus damage) plus any status (poison = `burn`, slow, `armor` timed modifier) |
| Mana break | `on_attack_hit,trigger_unit,resource,mana_burn,40,physical,,,,0.8` |
| Cleave / splash | passive `cleave` / `splash` ratios |
| Timed cleave | `stat_modifier,cleave` (fraction 0..1, with a duration) adds to the cleave ratio of the granting ability's own cleave instance (it does not merge with item cleave) and reverts exactly; only melee attackers cleave (base attack range up to 250 counts as melee) |
| Cone cleave | timed `cleave_radius` (half-width at the attacker), `cleave_end_radius` (half-width at the far end) and `cleave_distance` (reach), each above 0 and additive like every stat. A non-zero `cleave_distance` turns that instance's circular cleave into a trapezoid from the attacker towards the struck unit; units overlap it when their centre projects within `0..distance` (widened by their body radius) and lies within the half-width there. Without `cleave_distance` the circle (`cleave_radius`, default 300) is used. Cleave stats from one ability's statuses sum into one instance; item cleave stays a separate circle, so both can hit the same unit |
| Percent bonus damage | timed `stat_modifier,attack_damage_pct` (0..1000): that percent of the target's base attack damage (the hero definition's damage for heroes, excluding item bonuses; the unit's current damage for other units), snapshotted when applied and reverted with the exact delta |
| Evasion / accuracy / true strike | passive `evasion` / `accuracy`, or timed statuses `evasion`, `accuracy`, `true_strike` |
| Instant attack | `on_cast,cast_target,perform_attack,,100` |
| Double attack | `on_attack_hit,attacker,proc,on_double,25` + `on_double,trigger_unit,perform_attack,,100` |
| Multi-target attack | `on_attack,units_in_radius,perform_attack,exclude_trigger_unit,70,,,600,2` (value = damage %, radius, amount = max targets: whole numbers from 0, a level list allowed, 0 = all; the search area is always centred on the attacker, who the extra attacks launch from) |
| Limited-attack buff (Flak Cannon) | `on_cast,caster,apply_status,attack_charges,,,10,,4` + `on_attack,caster,require_status,on_split:<owner>_attack_charges` + `on_split,units_in_radius,perform_attack,exclude_trigger_unit,100,,,1250,4` |
| Attack counter | `on_attack_hit,attacker,counter,on_fourth,4` + `on_fourth,...` rows |
| Pseudo-random bash | `on_attack_hit,owner,pseudo_proc,on_bash,25` + `on_bash,...` rows (same columns as `proc`) |
| Range / projectile speed | passive or timed `attack_range` and `attack_projectile_speed` |
| Buff-gated on-hit (Arctic Burn) | `on_cast,caster,stat_modifier,attack_range,...` + `on_attack_hit,caster,require_status,on_burn_hit:<owner>_attack_range` + `on_burn_hit,trigger_unit,apply_status,burn,...` |

Samples: `basher`, `desolator`, `crit_blade`, `split_bow`, `mana_breaker`, `thunder_axe` in `data/examples/`. Out of scope: orb uniqueness, autocast orbs, bounce / pierce attack projectiles.

### Activation & targeting mechanics (reference)

Shared pipeline: `ActivationRequest` → `ActionSpec` (`src/actions/`) → cast point → fire → optional channel → backswing. Abilities and actives use the same fields; items use `ItemActivation` mapped into `ActionSpec` (`items.rs`).

| Mechanic (player-facing) | Code / data |
| --- | --- |
| Unit target | `TargetType::Unit`; CSV `target_type=unit`; pick filter `unit_only` in targeting UI |
| Point target | `TargetType::Point`; CSV `target_type=point` (with projectile → skillshot path) |
| Direction target | `TargetType::Direction`; CSV `target_type=direction`; `ignores_range()` for validation |
| No target | `TargetType::NoTarget`; instant hotkey when `is_instant()` |
| Self target | `TargetType::SelfTarget`; CSV `target_type=self` + usually `target_team=self` |
| Area target | `TargetType::Area`; CSV **`point` + `aoe_radius` + `projectile_type none`** (no CSV `area` alias) |
| Toggle | `AbilityBehavior::Toggle`; `AbilityState.toggled`; mana upkeep (`toggle_mana_per_sec`) and `WhileToggled` statuses in `abilities/passive.rs` |
| Autocast | *Not implemented* |
| Passive ability | `AbilityBehavior::Passive`; `AbilityTrigger::Passive` + `PassiveStat` effects; `sync_ability_passives` |
| Channeling | `ActionSpec.channel_time`, `Activation.channel_remaining`; `OnChannelEnd` (completed) and `OnChannelInterrupted` (ended by a disable) for abilities and items are produced by `action_channel_end_triggers` in `abilities/casting.rs` (`on_channel_end` / `on_channel_interrupted` in the effect CSVs) |
| Cast point | `ActionSpec.cast_time` / `point_remaining`; mana and cooldown on fire |
| Cast backswing | `ActionSpec.backswing`; items default to **0** backswing |
| Cast range | `ActionSpec.cast_range` (`RankValue` on abilities, flat on items); edge-to-edge via `dimensions` |
| Target team filter | `TargetTeam` + `team_allows` / `target_allowed`; CSV `target_team` enemy/ally/both/self |
| Hero / creep / building filter | *Not implemented* (`TargetUnitMask` planned) |
| Cooldown | `ActionCooldown.remaining`; starts on commit |
| Cooldown reduction / reset | *Not implemented* |
| Charges | `ActionCooldown.charges`; abilities may regen over time (`restore_ability_charges`); item charges per stack only |
| Resource cost | `ActionCost { mana }` only (health / gold / toggle upkeep via `toggle_mana_per_sec` in `passive.rs`) |
| Cast interruption | See table below |

#### Cast interruption

| Phase | Disable (abilities: silence / stun / sleep; items: mute / stun) | New move / stop / attack | Target dies | Source slot changes |
| --- | --- | --- | --- | --- |
| Cast point (before fire) | Cancel, no cost (a silenced ability is cancelled but an item still fires; a muted item is cancelled but an ability still fires) | Cancel, no cost | The aim follows a living unit target; if it dies or despawns the action still fires at its last known position (costs paid, effects keyed on the target resolve to nothing). A queued cast still walking into range is dropped instead if its unit-only target died | Cancel |
| After fire, channeling | End channel early and fire `OnChannelInterrupted` (abilities: silence, stun or sleep; items: mute or stun; sleep does not interrupt an item channel). A channel that runs its full time fires `OnChannelEnd` | Cancel channel (no item/ability trigger) | — | Cancel |
| Backswing | Does not remove backswing by itself | Removes `Activation` | — | Cancel |

A `CastTarget::Unit` that turns invisible or untargetable during the cast point is still tracked by `refresh_activation_aims`; only death or despawn stops the tracking. That is acceptable today and would need revisiting if fog-based targeting is added.
- **Custom mechanics** (`abilities/custom.rs`): `app.register_ability_behavior(id, system)` runs a one-shot system with the `CastContext`. It reports what happened with a trigger such as `AbilityTrigger::Custom("on_my_event")`, and the ability's effect entries apply the consequences

Adding a simple ability: add an `AbilityId` variant (or use the generators), then add an arm to `catalog::builtin`:

```rust
AbilityId::Fireball => def
    .targeting(TargetType::Unit, RankValue::linear(600.0, 25.0), RankValue::ZERO)
    .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
    .timing(0.3, 0.4)
    .mechanic(PROJECTILE)
    .on(OnProjectileHit, EffectTarget::TriggerUnit, magic_damage(RankValue::linear(150.0, 50.0)))
    .on(OnProjectileHit, EffectTarget::TriggerUnit, status(StatusSpec::Stun, RankValue::fixed(1.0))),
```
