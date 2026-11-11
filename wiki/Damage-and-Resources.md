# Damage and Resources

### Damage mechanics (player terms → code)

| Term | Code / behavior |
| --- | --- |
| Physical / magical / pure | `DamageType`; pure skips armor and MR in `apply_damage` |
| Mitigation | Dota-style armor and MR in `combat::mitigate` |
| Outgoing amp | `CombatStats::outgoing_physical_amp` / `outgoing_spell_amp` on `DamageEvent` resolution |
| Incoming amp | `CombatStats::incoming_damage_amp` after mitigation |
| Spell amp | Item passive `PassiveStat::SpellAmp` → `outgoing_spell_amp` |
| Crit | `crit_chance` / `crit_multiplier`, rolled **once per landed attack** in `basic_attack::resolve_basic_attack_impacts` (one `CombatRng` draw, after the hit roll); the main hit is multiplied there and `DamageEvent::crit` is informational. Crit never applies to bonus damage, DoTs, cleave or splash |
| Block | Flat `CombatStats::damage_block` per hit |
| Shield / barrier | `ShieldPool` consumed in `resolve_incoming_damage` (Barrier ability: debuff immunity + armor + shield) |
| Cleave | Melee only: each cleave instance deals its ratio x the post-crit main hit to enemies in its own area (circle radius default `scale::ATTACK_CLEAVE_RADIUS` = 300, or a cone); `DamageKind::Cleave`. Instances are independent: item passives share one (ratios add), each ability with timed cleave statuses has its own, and a unit inside several areas takes each |
| Splash (attack) | `splash_damage_ratio` x the post-crit main hit to enemies around the struck unit (`splash_radius`, default `scale::ATTACK_SPLASH_RADIUS` = 250); any attack; `DamageKind::Splash`. Secondary hits never crit, roll to hit, leech or fire triggers |
| Attack bonus damage | `Damage` / mana-burn effects under `on_attack` / `on_attack_hit` become `DamageKind::AttackBonus`: no spell amp, no block, leeched like a basic attack, reflectable |
| Zone | `AbilityEffect::Zone` mechanic gate (`abilities/zones.rs`): a persistent area spawned at the trigger point that, after a delay and then every interval, re-fires an `on_<name>` trigger with the units inside as the affected units. Shrapnel uses it; the `zone` CSV effect type; `attached_zone` is the same gate re-centred on the caster before every tick (Slark's Dark Pact) |
| Orbit | `AbilityEffect::Orbit` mechanic gate (`abilities/orbits.rs`): N hitboxes circle the caster (radius, period, duration) and re-fire an `on_<name>` trigger with the unit touched as the trigger unit whenever a hitbox starts touching a unit; a unit re-arms only after it leaves that hitbox, and every hitbox keeps its own contact list. Spirit Guardians uses it; the `orbit` CSV effect type |
| Chain | `AbilityEffect::Chain` mechanic gate (`mechanics::chain_hops`) re-fires an `on_<name>` trigger per hop; ordinary damage rows tagged with it deal the (falloff-scaled) damage. Bolt and the `chain` CSV effect type use it |
| Random pick | `AbilityEffect::PickRandom { count, then }` mechanic gate: picks `count` distinct living units out of the resolved targets (`affected_units` or `units_in_radius`) with `CombatRng::index` (a partial Fisher-Yates in message order, so a seeded RNG is deterministic) and re-fires an `on_<name>` trigger once per pick with the pick as the trigger unit. An empty pool fires nothing. Rocket Barrage and the `pick_random` CSV effect type use it |
| Seeking projectile | `ProjectileSeek { delay, acceleration, retarget_radius }` on a homing `AbilityMechanic::Projectile` (the `projectile_delay` / `projectile_acceleration` / `projectile_retarget_radius` columns). All the rules live in `combat::fly_projectiles`: the missile waits `delay` seconds at its launch point facing the target (and cannot strike during it), then gains `acceleration` u/s^2, and when its target is dead or gone it retargets the nearest living, targetable enemy within `retarget_radius` of the missile (the payload's `cast_target` follows, so spell block, spell immunity and reflection judge the new target); with none it ends at the last known position as `on_impact`. It lives up to `SEEKING_PROJECTILE_MAX_LIFETIME` (15 s). Homing Missile uses it |
| DoT | Status `damage_per_sec` ticks in `tick_status_effects` |
| Reflection | `damage_reflect` fraction of dealt damage returned to attacker |
| Health scaling | `EffectScaling::{TargetMissingHealth, TargetMaxHealth, TargetCurrentHealth}` |
| Distance falloff | `EffectScaling::DistanceFromCaster { falloff }` vs ability cast range |

### Healing and resources (player terms → code)

Every health increase goes through `vitals::apply_heal`; `combat::apply_damage_events` is the only place damage changes health. Rules live in `src/vitals.rs`.

| Term | Code / behavior |
| --- | --- |
| Instant heal | `AbilityEffect::Heal` → `HealEvent` → `apply_heal_events` → `heal_factor(Direct)` + `apply_heal` (clamped at max, overheal discarded, dead units are never healed) |
| Heal over time | `StatusSpec::HealOverTime { hps, max_health_pct }` (generator `apply_status,heal_over_time`): flat HP per second plus a percent of the holder's current max health per second; the healer's amp scales both parts |
| Health regeneration | `Health::regen_per_sec` + timed `ResourceStat::HealthRegen`; `regen_health` uses `HealKind::Regen` (ignores amps, respects reduction and block) |
| Healing amplification | `CombatStats::heal_amp` (provided) / `heal_received_amp` (received) plus timed modifiers; one additive bucket `1 + healer + target` |
| Healing reduction / prevention | Heal reduction (strongest, multiplicative after amp) and heal block (factor 0, wins over amp) statuses |
| Lifesteal | `CombatStats::lifesteal` on `DamageKind::BasicAttack` and `DamageKind::AttackBonus` hits: fraction of health actually removed (no overkill, nothing a shield or mana shield absorbed) |
| Spell lifesteal | `CombatStats::spell_lifesteal` on `DamageEvent::is_spell` hits |
| Health transfer | `AbilityEffect::DrainHealth` → `DamageEvent::spell(..).drained(ratio)`: damage that heals the caster for `ratio ×` health removed |
| Leech rules | Applied after the damage batch in the same frame via `HealKind::Leech`; DoT, reflected damage, `DamageKind::Cleave` and `DamageKind::Splash` never leech; a source killed in the same batch gets nothing |
| Mana restoration | `AbilityEffect::RestoreMana` (generator `heal,mana`) |
| Mana regeneration | `Mana::regen_per_sec` + timed `ResourceStat::ManaRegen`; skips dead units |
| Mana burn / drain | `AbilityEffect::DrainMana` → `ResourceDrainEvent` → `vitals::apply_resource_drain_events` (burn deals spell damage, or attack bonus damage when `ResourceDrainEvent::from_attack` is set by an attack trigger; drain gives mana to the caster, clamped to max). Blocked only by invulnerability, not debuff immunity |
| Health sacrifice | `AbilityEffect::SpendResource { Health }`: non-lethal (floor 1 HP), bypasses shields, reflection and leech |
| Resource conversion | `SpendResource { convert_to }`: health → mana, or mana → health (through the central heal path) |
| Mana shield | `StatusSpec::ManaShield` → `combat::absorb_with_mana_shield` |
| Mana cost modification | `CombatStats::mana_cost_amp` + timed `ResourceStat::ManaCostAmp`, applied once in `ActionSources::spec` (abilities, items, toggle upkeep); evaluated again when the cast fires |
| Max health / mana modifier | Permanent: `Health::change_max` with `MaxChangeRule::KeepMissing` (items). Timed: `PreserveRatio` via `sync_status_resource_caps` |
