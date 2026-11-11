# Movement

### Teleport

`movement,teleport` (`on_cast` for abilities, `on_use` for items) is one shared effect: `AbilityEffect::Teleport` → `TeleportEvent` → `teleport.rs`. The destination is the trigger's aim point (the cast's point or target position). Item teleports take that aim from a point or direction activation, so they use the same range and targeting indicator as abilities. Cast time, cooldown, mana, and range stay on the shared activation.

- The destination must fit the target's `CollisionRadius` inside the map and clear of trees, towers, and ancients (`NavGrid`), and must not overlap a non-phased unit (phased targets ignore units, like normal collision). Otherwise nothing changes — no nearby fallback and no pushing — and a `TeleportRejectedEvent` is emitted.
- On success the unit moves instantly, keeps its Y, health, mana, statuses, and cooldowns, and loses its `MoveTarget`, `NavPath`, attack-move, attack-position goal, and in-flight forced movement. A `TeleportCompletedEvent` is emitted. Attack and queued-cast orders are kept (units re-path from the new spot).
- Costs are paid at cast / use time, so a rejected teleport still spends mana and starts the cooldown.
- **Bounded teleports:** with `value` (maximum distance, above 0) and/or `amount` (minimum distance, 0 or more, at most `value`) the destination is the aim point with its distance from the caster clamped to `min..=max` along the same line. An aim on the caster itself uses her facing. Both blank keeps the destination exactly at the aim point (Blink, Blink Dagger). Bounds need `effect_target` `caster` / `owner`. Pair a max distance with `cast_range=global` to make a blink that ignores range but not distance (Queen of Pain's blink).

### Forced movement

Knockback, pull, dash, charge, drag and cyclone are one shared primitive: `AbilityEffect::Displace { kind, distance, duration, speed, contact_radius, forward_offset, bound_stun }` → `DisplacementEvent` → `ForcedMovement` on the moved unit, ticked by `displacement.rs`, which is the only writer of a moving unit's `Transform`. Abilities and items use the same effect (`movement,<kind>` rows); `AbilityMechanic::Dash` writes the same event and only adds the ghost trail.

| Kind | CSV | Moves | Notes |
| --- | --- | --- | --- |
| `knockback` | `value` = distance, `duration` | target, away from the trigger point | |
| `pull` | `duration` (and optional `value` = max distance) | target, toward the caster | Tracks the caster until contact; the pull time is the time to close the starting gap |
| `pull_point` | `duration`, optional `value`, optional `amount` = forward offset | target, toward the trigger point | With `amount` (not negative) the target is pulled to a point that far in front of the caster, along its facing, measured from where the caster stood when the trigger fired; a crowd gathers there without teleport occupancy rejections |
| `dash` | `value` = max distance (required), `duration` = time at max distance (default 0.3), optional `radius` = contact radius | caster / owner only | Straight line at constant speed; shorter dashes take proportionally less time. An aim on the mover itself (a `no_target` cast, or a click on herself) dashes the full distance along its facing (`-Z` when it has none). `radius` (above 0; also on `charge`) is the reach from the mover's centre, plus the other unit's body radius, within which it contacts enemies; blank is body contact |
| `charge` | `amount` = speed, `duration` = max time (both required), optional `value` | caster / owner only | Chases the cast target (needs `target_type unit`) until contact |
| `drag` | optional `duration` | target (not the caster) | Rides along at a fixed offset with the caster's own dash or charge; blank duration = until that movement ends. Typically `on_unit_contact,trigger_unit` |
| `drag_stun` | optional `duration` | target (not the caster) | A `drag` that also stuns each dragged unit under the status id `<owner>_stun` (a debuff that behaves as a stun). The stun is bound to the drag (`ForcedMovement.bound_status`): exactly that status is removed when the carrying movement ends, however it ends, so no other stun is overwritten (refresh is by id) or removed |
| `cyclone` | `duration` (required) | target, held in place | Also applies the `cyclone` status |

- **Priority.** A unit has at most one motion. A new request replaces the current one only if its priority is at least as high: own movement (dash, charge) < external (knockback, pull, drag) < suspend (cyclone). The newest wins at equal priority. A cycloned unit rejects every other displacement and teleport; another cyclone refreshes it.
- **Own movement needs a unit that can move.** A rooted, stunned or sleeping caster pays for the cast but does not dash; an own dash or charge that is refused (cannot move, nothing to plan) still reports `on_movement_end` at the mover's position with no contacts, so landing effects (Earthshock) take effect in place. A stun or root during a dash ends it at the current spot. External motion continues through stuns and roots.
- **Hostile only.** Debuff immunity blocks hostile displacement; a unit's own and its allies' motion are allowed.
- **Terrain.** Motion stops at trees, towers, ancients and the map edge (unless the unit is flying or cycloned). A unit that starts inside geometry may move so it can get out.
- **Landing.** Every motion that ends normally sets the unit on valid ground (the nearest walkable spot if it ended inside geometry) and drops its stale `NavPath`, so its `MoveTarget` is replanned from the new position. Motion no longer roots the victim: move orders survive and resume afterwards. Units in forced motion ignore unit collision while they travel.
- **Triggers.** Own movement carrying a cast reports `OnUnitContact` for each enemy it newly touches and one `OnMovementEnd` when it ends by arrival, block, timeout, or the mover being disabled. Replacement, teleport, and death end it silently. Both resolve in the frame they happen.
- **Untargetable.** A cycloned unit cannot be picked as a unit target (casts, attack commands, basic attacks, AI acquisition), homing projectiles miss it, and skillshots, splash, cleave, and ability area effects skip it. Its own targeting is unaffected.
- **Flying.** Flying units path in a straight line and walk over trees; towers and ancients still block. When the status ends over geometry the unit is moved to the nearest walkable spot.
- **Leash / tether.** `enforce_leashes` clamps a leashed unit to its circle after walking and after forced motion every frame, so a knockback stops at the edge. A tether whose anchor is gone does nothing. Teleporting outside a circle is rejected (`TeleportError::Leashed`).

A hook is a composition: an ability with `projectile_type skillshot_single` and `on_projectile_hit,trigger_unit,movement,pull`. A skewer is `movement,dash` on cast plus `on_unit_contact,trigger_unit,movement,drag` (or `drag_stun`), with consequences on `on_movement_end`.
