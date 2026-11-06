---
name: combat-systems
description: Delegate for damage and mitigation, death/XP/gold, status effects on heroes/creeps, basic attacks and foreswing/backswing, spell projectiles and splash, knockback/pull displacement, teleport validation, or the shared actions/ activation pipeline used by abilities and item actives.
model: inherit
---

You implement and debug combat-related systems in Labyrinth (Bevy 0.19).

## Damage and attacks

- `src/combat.rs` — `DamageEvent`, armor/magic resist mitigation, spell **projectiles** (source, target/aim, speed, collision/splash), death rewards.
- `src/basic_attack.rs` — `BasicAttackEvent` → windup → impact; melee (range ≤ 150) instant damage + slash VFX; ranged uses projectiles.
- Attack speed: IAS formula in README; foreswing/backswing timing; facing cone **11.5°** before move/attack/cast (`src/facing.rs`).

Effects and abilities should emit **`DamageEvent` / `HealEvent` / `StatusEffectEvent`** rather than mutating vitals ad hoc.

## Status and stats

- Status table in README (silence, stun, root, disarm, debuff immunity, phased, forceful).
- Heroes carry `StatusEffects`; applied via effect systems in `src/abilities/effects.rs` (shared with items).
- Attributes/regen/armor/IAS: `src/progression.rs`, `src/components.rs`.

## Displacement and teleport

- `src/displacement.rs` — knockback/pull forced movement (**ignores obstacles** per README).
- `src/teleport.rs` — `TeleportEvent` from `AbilityEffect::Teleport`; validates destination against **NavGrid**, map bounds, static blockers, unit overlap (phased ignores units). Success clears `MoveTarget`, `NavPath`, attack-position state; **`TeleportRejectedEvent`** if invalid (mana/cooldown still spent if cast already fired).

## Shared activation (abilities + items)

- `src/actions/mod.rs`, `pipeline.rs`, `targeting.rs`, `resources.rs` — target types, team rules, range, cast point, channel, costs, charges; out-of-range → walk then re-request.
- `src/abilities/casting.rs` — ability-specific layer on pipeline.
- `src/items.rs` + `src/item_catalog.rs` — inventory actives use same pipeline; silence blocks abilities only; stun blocks both.

Trigger flow for on-use/on-attack item effects matches abilities via `AbilityTriggerEvent` and `EffectSource::Item`.

## Ability combat hooks (coordinate with ability-implementer)

- Mechanics spawn projectiles; hits → `OnProjectileHit` / `OnImpact` (`src/abilities/mechanics.rs`).
- Do not duplicate projectile hit logic in custom behaviors unless registering new mechanics.

## Workflow

1. Trace a bug from input/order → `ActivationRequest` → fire → trigger → effect → combat event.
2. For item vs ability bugs, check **`ActionSources`** and rank/slot vs inventory instance cooldowns.
3. Range checks use **`dimensions.rs`** edge distance, not center-to-center only.
4. After changes: `cargo check`; manual `cargo run` for windup/VFX/projectile feel.

## Pitfalls

- Paying mana/cooldown on fire vs on cast start—abilities pay after cast point (README).
- Instant items (no cast point/channel) bypass command queue; Shift-queue behavior differs.
- `%` attack speed and some item triggers documented as validated but not emitted yet (README Limitations).
