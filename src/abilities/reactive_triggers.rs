//! Reactive triggers: owner-wide fan-out shared by every dispatcher.
//!
//! Each dispatcher reads one authoritative message (damage applied, status change, cooldown
//! ready, ...) and writes ordinary [`super::effects::AbilityTriggerEvent`]s, which the
//! existing `resolve_ability_triggers` queue resolves. Dispatchers never apply effects.

use bevy::prelude::*;

use super::casting::AbilityCastEvent;
use super::catalog::AbilityDefinitions;
use super::definition::AbilityTrigger;
use super::definition::{AbilityEffect, Condition};
use super::effects::build_status_effect;
use super::effects::{
    AbilityTriggerEvent, EffectSource, SpellHitEvent, StatusMeta, TriggerContext,
};
use super::passive::reconcile_held_status;
use crate::combat::flat_distance;
use crate::combat::{DamageAppliedEvent, DamageKind};
use crate::components::{AbilityLoadout, BoundRadius, CombatStats, Health, HeroAttributes, Team};
use crate::dimensions::{area_contains, bounds_of};
use crate::items::{Inventory, ItemId};
use crate::status::{StatusChange, StatusEffectEvent, StatusEffects, is_untargetable};
use crate::status::{StatusLifecycle, StatusLifecycleEvent};
use crate::teleport::TeleportCompletedEvent;
use crate::vitals::outgoing_heal_amp;

/// Every effect source the owner holds: items in slot order (one per distinct [`ItemId`],
/// however many copies are carried), then learned ability slots (rank > 0). Items use rank 1.
pub(crate) fn owner_sources<'a>(
    inventory: Option<&'a Inventory>,
    loadout: Option<&'a AbilityLoadout>,
) -> impl Iterator<Item = (EffectSource, u32)> + 'a {
    let mut seen: Vec<ItemId> = Vec::new();
    let items = inventory
        .into_iter()
        .flat_map(|inventory| inventory.slots.iter().flatten())
        .filter_map(move |item| {
            if seen.contains(&item.id) {
                return None;
            }
            seen.push(item.id);
            Some((EffectSource::Item(item.id), 1))
        });
    // Rank 0 is unlearned and never triggers.
    let abilities = loadout
        .into_iter()
        .flat_map(|loadout| loadout.slots.iter())
        .filter(|slot| slot.rank > 0)
        .map(|slot| (EffectSource::Ability(slot.id), slot.rank));
    items.chain(abilities)
}

/// The distinct triggers of `source` that satisfy `pred`, in entry order (used by the
/// parameterised triggers, where one source may hold several thresholds).
pub(crate) fn source_triggers(
    source: EffectSource,
    defs: &AbilityDefinitions,
    pred: impl Fn(AbilityTrigger) -> bool,
) -> Vec<AbilityTrigger> {
    let entries: &[super::definition::AbilityEffectEntry] = match source {
        EffectSource::Item(id) => id.definition().effects,
        EffectSource::Ability(id) => match defs.get(id) {
            Some(def) => &def.effects,
            None => return Vec::new(),
        },
    };
    let mut found: Vec<AbilityTrigger> = Vec::new();
    for entry in entries {
        if pred(entry.trigger) && !found.contains(&entry.trigger) {
            found.push(entry.trigger);
        }
    }
    found
}

/// Whether `source` has any entry for `trigger`.
pub(crate) fn source_has_trigger(
    source: EffectSource,
    defs: &AbilityDefinitions,
    trigger: AbilityTrigger,
) -> bool {
    !source_triggers(source, defs, |t| t == trigger).is_empty()
}

/// A source's cooldown just finished (written by the two cooldown tick systems).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CooldownReadyEvent {
    pub owner: Entity,
    pub source: EffectSource,
}

/// Per-unit state of the time- and position-based reactive triggers, next to
/// [`super::effects::TriggerCounters`]. Entries for sources the owner no longer holds are
/// pruned every frame.
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct TriggerTracking {
    /// Seconds accumulated towards each `OnInterval` entry, per (source, trigger).
    pub intervals: Vec<((EffectSource, AbilityTrigger), f32)>,
    /// Where the owner was when `track_travel` last looked (`None` until the first frame).
    pub last_pos: Option<Vec3>,
    /// Flat distance the owner covered last frame (teleports excluded).
    pub moved: f32,
    /// Enemies within the radius of each `OnEnemyEnter` / `OnEnemyLeave` entry, sorted by
    /// `Entity`, per (source, trigger).
    pub proximity: Vec<((EffectSource, AbilityTrigger), Vec<Entity>)>,
    /// Statuses `sync_conditional_statuses` applied for `While` entries, by source, so they
    /// can be taken off exactly when the source is lost.
    pub held: Vec<(EffectSource, &'static str)>,
}

/// Distances below this are not movement.
const MOVE_EPSILON: f32 = 0.01;

type OwnerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Team,
        Option<&'static Inventory>,
        Option<&'static AbilityLoadout>,
    ),
>;

/// Writes one trigger per source of `owner` that has entries for `trigger`. `build` fills
/// in the occurrence-specific parts of the context.
fn fan_out(
    owner: Entity,
    trigger: AbilityTrigger,
    except: Option<EffectSource>,
    owners: &OwnerQuery,
    defs: &AbilityDefinitions,
    triggers: &mut MessageWriter<AbilityTriggerEvent>,
    build: impl Fn(TriggerContext) -> TriggerContext,
) {
    let Ok((owner_tf, team, inventory, loadout)) = owners.get(owner) else {
        return;
    };
    for (source, rank) in owner_sources(inventory, loadout) {
        if Some(source) != except && source_has_trigger(source, defs, trigger) {
            let ctx = TriggerContext::new(
                trigger,
                source,
                owner,
                *team,
                rank,
                owner_tf.translation,
                owner_tf.translation,
            );
            triggers.write(AbilityTriggerEvent(build(ctx)));
        }
    }
}

/// What `eval_condition` needs to know about the owner and its surroundings.
struct ConditionView<'a> {
    owner: Entity,
    pos: Vec3,
    team: Team,
    health: Option<&'a Health>,
    statuses: &'a StatusEffects,
    moved: f32,
}

fn count_near(
    view: &ConditionView,
    radius: u32,
    wanted_team: Team,
    units: &Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    untargetable: &Query<&StatusEffects>,
) -> usize {
    units
        .iter()
        .filter(|(entity, tf, team, health, bound)| {
            *entity != view.owner
                && **team == wanted_team
                && health.is_alive()
                && !is_untargetable(untargetable, *entity)
                && area_contains(view.pos, radius as f32, tf.translation, bounds_of(*bound))
        })
        .count()
}

fn eval_condition(
    condition: Condition,
    view: &ConditionView,
    units: &Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    untargetable: &Query<&StatusEffects>,
) -> bool {
    match condition {
        Condition::HealthBelow { percent } => view
            .health
            .is_some_and(|h| h.current / h.max.max(1.0) * 100.0 < f32::from(percent)),
        Condition::Moving => view.moved > MOVE_EPSILON,
        Condition::Stationary => view.moved <= MOVE_EPSILON,
        Condition::EnemiesNear { radius, at_least } => {
            count_near(view, radius, view.team.enemy(), units, untargetable)
                >= usize::from(at_least)
        }
        Condition::AlliesNear { radius, at_least } => {
            count_near(view, radius, view.team, units, untargetable) >= usize::from(at_least)
        }
        Condition::HasStatus(id) => view
            .statuses
            .effects
            .iter()
            .any(|e| e.id == id && e.remaining > 0.0),
    }
}

/// Reconcile `While(Condition)` entries: each held `ApplyStatus` is present exactly while
/// its condition holds, the owner lives and the source is still held. Runs last among the
/// dispatchers, after movement and proximity are fresh. The statuses are applied through
/// `StatusEffectEvent` (reactive, so silent) like toggle statuses.
pub(crate) fn sync_conditional_statuses(
    defs: Res<AbilityDefinitions>,
    mut owners: Query<(
        Entity,
        &Transform,
        &Team,
        Option<&Health>,
        &StatusEffects,
        &CombatStats,
        Option<&HeroAttributes>,
        Option<&Inventory>,
        Option<&AbilityLoadout>,
        &mut TriggerTracking,
    )>,
    units: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    untargetable: Query<&StatusEffects>,
    mut status_events: MessageWriter<StatusEffectEvent>,
) {
    for (owner, owner_tf, team, health, statuses, stats, attrs, inventory, loadout, mut tracking) in
        &mut owners
    {
        if inventory.is_none() && loadout.is_none() && tracking.held.is_empty() {
            continue;
        }
        let alive = health.is_none_or(|h| h.is_alive());
        let view = ConditionView {
            owner,
            pos: owner_tf.translation,
            team: *team,
            health,
            statuses,
            moved: tracking.moved,
        };
        let healer_amp = outgoing_heal_amp(Some(stats), Some(statuses));
        let mut still_held: Vec<(EffectSource, &'static str)> = Vec::new();
        for (source, rank) in owner_sources(inventory, loadout) {
            let conditions =
                source_triggers(source, &defs, |t| matches!(t, AbilityTrigger::While(_)));
            for trigger in conditions {
                let AbilityTrigger::While(condition) = trigger else {
                    continue;
                };
                let wanted = alive && eval_condition(condition, &view, &units, &untargetable);
                let Some(params) = super::effects::source_params(source, &defs, rank) else {
                    continue;
                };
                for entry in params.entries.iter().filter(|e| e.trigger == trigger) {
                    let AbilityEffect::ApplyStatus { status, .. } = entry.effect else {
                        continue;
                    };
                    let built = build_status_effect(
                        status,
                        rank,
                        f32::INFINITY,
                        Some(owner),
                        owner_tf.translation,
                        Some(stats),
                        attrs,
                        healer_amp,
                    );
                    let id = built.id;
                    reconcile_held_status(&mut status_events, owner, statuses, built, wanted);
                    if wanted {
                        still_held.push((source, id));
                    }
                }
            }
        }
        // Statuses of sources that are gone (sold, unlearned): take them back off. A source
        // that is still held but whose condition is false was reconciled above.
        for (source, id) in tracking.held.iter() {
            let source_gone = !owner_sources(inventory, loadout).any(|(s, _)| s == *source);
            if source_gone
                && statuses.effects.iter().any(|e| e.id == *id)
                && !still_held.iter().any(|(_, held_id)| held_id == id)
            {
                status_events.write(
                    StatusEffectEvent::new(Some(owner), owner, StatusChange::Remove(id))
                        .reactive(true),
                );
            }
        }
        tracking.held = still_held;
    }
}

/// `OnMove`: the flat distance each owner covered since last frame, as the context amount.
/// Teleports re-anchor the position instead of counting. Runs for every owner with a
/// source (the distance also feeds the `Moving` / `Stationary` conditions).
pub(crate) fn track_travel(
    mut teleported: MessageReader<TeleportCompletedEvent>,
    defs: Res<AbilityDefinitions>,
    mut owners: Query<(
        Entity,
        &Transform,
        &Team,
        Option<&Health>,
        Option<&Inventory>,
        Option<&AbilityLoadout>,
        &mut TriggerTracking,
    )>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    let jumped: Vec<Entity> = teleported.read().map(|event| event.target).collect();
    for (owner, owner_tf, team, health, inventory, loadout, mut tracking) in &mut owners {
        if inventory.is_none() && loadout.is_none() {
            continue;
        }
        let here = owner_tf.translation;
        let distance = match tracking.last_pos {
            Some(previous) if !jumped.contains(&owner) => flat_distance(previous, here),
            _ => 0.0,
        };
        tracking.last_pos = Some(here);
        tracking.moved = distance;
        if distance <= MOVE_EPSILON || health.is_some_and(|health| !health.is_alive()) {
            continue;
        }
        for (source, rank) in owner_sources(inventory, loadout) {
            if source_has_trigger(source, &defs, AbilityTrigger::OnMove) {
                triggers.write(AbilityTriggerEvent(
                    TriggerContext::new(
                        AbilityTrigger::OnMove,
                        source,
                        owner,
                        *team,
                        rank,
                        here,
                        here,
                    )
                    .amount(distance),
                ));
            }
        }
    }
}

/// `OnEnemyEnter` / `OnEnemyLeave`: each entry tracks the living, targetable enemies within
/// its radius and fires for the difference since last frame, in `Entity` order. A tracked
/// enemy that died or despawned is dropped without a leave.
pub(crate) fn proximity_triggers(
    defs: Res<AbilityDefinitions>,
    mut owners: Query<(
        Entity,
        &Transform,
        &Team,
        Option<&Health>,
        Option<&Inventory>,
        Option<&AbilityLoadout>,
        &mut TriggerTracking,
    )>,
    units: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    statuses: Query<&StatusEffects>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for (owner, owner_tf, team, health, inventory, loadout, mut tracking) in &mut owners {
        if inventory.is_none() && loadout.is_none() {
            continue;
        }
        let mut held: Vec<(EffectSource, u32, AbilityTrigger)> = Vec::new();
        for (source, rank) in owner_sources(inventory, loadout) {
            for trigger in source_triggers(source, &defs, |t| {
                matches!(
                    t,
                    AbilityTrigger::OnEnemyEnter { radius }
                        | AbilityTrigger::OnEnemyLeave { radius } if radius > 0
                )
            }) {
                held.push((source, rank, trigger));
            }
        }
        tracking
            .proximity
            .retain(|(key, _)| held.iter().any(|(s, _, t)| *key == (*s, *t)));
        if health.is_some_and(|health| !health.is_alive()) {
            continue;
        }
        for (source, rank, trigger) in held {
            let (AbilityTrigger::OnEnemyEnter { radius } | AbilityTrigger::OnEnemyLeave { radius }) =
                trigger
            else {
                continue;
            };
            let mut inside: Vec<Entity> = units
                .iter()
                .filter(|(entity, tf, unit_team, unit_health, bound)| {
                    **unit_team == team.enemy()
                        && unit_health.is_alive()
                        && !is_untargetable(&statuses, *entity)
                        && area_contains(
                            owner_tf.translation,
                            radius as f32,
                            tf.translation,
                            bounds_of(*bound),
                        )
                })
                .map(|(entity, ..)| entity)
                .collect();
            inside.sort();

            let key = (source, trigger);
            let index = match tracking.proximity.iter().position(|(k, _)| *k == key) {
                Some(index) => index,
                None => {
                    tracking.proximity.push((key, Vec::new()));
                    tracking.proximity.len() - 1
                }
            };
            let previous = std::mem::replace(&mut tracking.proximity[index].1, inside.clone());
            let changed: Vec<Entity> = match trigger {
                AbilityTrigger::OnEnemyEnter { .. } => inside
                    .iter()
                    .copied()
                    .filter(|e| !previous.contains(e))
                    .collect(),
                _ => previous
                    .iter()
                    .copied()
                    // Dead or despawned members fall out of `units`' living set silently.
                    .filter(|e| !inside.contains(e) && units.get(*e).is_ok_and(|u| u.3.is_alive()))
                    .collect(),
            };
            for unit in changed {
                let at = units
                    .get(unit)
                    .map_or(owner_tf.translation, |u| u.1.translation);
                triggers.write(AbilityTriggerEvent(
                    TriggerContext::new(
                        trigger,
                        source,
                        owner,
                        *team,
                        rank,
                        owner_tf.translation,
                        at,
                    )
                    .trigger_unit(Some(unit))
                    .at(at),
                ));
            }
        }
    }
}

/// `OnCooldownReady` for the source whose cooldown just finished. Copies of one item raise
/// one trigger per frame.
pub(crate) fn cooldown_triggers(
    mut ready: MessageReader<CooldownReadyEvent>,
    defs: Res<AbilityDefinitions>,
    owners: OwnerQuery,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    let mut seen: Vec<(Entity, EffectSource)> = Vec::new();
    for event in ready.read() {
        if seen.contains(&(event.owner, event.source)) {
            continue;
        }
        seen.push((event.owner, event.source));
        let Ok((owner_tf, team, inventory, loadout)) = owners.get(event.owner) else {
            continue;
        };
        // Only a source the owner still holds (and has learned) reacts.
        let Some((source, rank)) =
            owner_sources(inventory, loadout).find(|(source, _)| *source == event.source)
        else {
            continue;
        };
        if source_has_trigger(source, &defs, AbilityTrigger::OnCooldownReady) {
            triggers.write(AbilityTriggerEvent(TriggerContext::new(
                AbilityTrigger::OnCooldownReady,
                source,
                event.owner,
                *team,
                rank,
                owner_tf.translation,
                owner_tf.translation,
            )));
        }
    }
}

/// `OnInterval { millis }` for each source the owner holds. Timers run on game time, are
/// shared by copies of an item, pause while the owner is dead, and fire at most once per
/// frame (any excess carries to the next frame).
pub(crate) fn interval_triggers(
    time: Res<Time>,
    defs: Res<AbilityDefinitions>,
    mut owners: Query<(
        Entity,
        &Transform,
        &Team,
        Option<&Health>,
        Option<&Inventory>,
        Option<&AbilityLoadout>,
        &mut TriggerTracking,
    )>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    let dt = time.delta_secs();
    for (owner, owner_tf, team, health, inventory, loadout, mut tracking) in &mut owners {
        if inventory.is_none() && loadout.is_none() {
            continue;
        }
        let alive = health.is_none_or(|health| health.is_alive());
        let mut held: Vec<(EffectSource, u32, AbilityTrigger)> = Vec::new();
        for (source, rank) in owner_sources(inventory, loadout) {
            for trigger in source_triggers(
                source,
                &defs,
                |t| matches!(t, AbilityTrigger::OnInterval { millis } if millis > 0),
            ) {
                held.push((source, rank, trigger));
            }
        }
        // Drop timers of sources the owner no longer holds.
        tracking
            .intervals
            .retain(|(key, _)| held.iter().any(|(s, _, t)| *key == (*s, *t)));
        if !alive {
            continue;
        }
        for (source, rank, trigger) in held {
            let AbilityTrigger::OnInterval { millis } = trigger else {
                continue;
            };
            let period = millis as f32 / 1000.0;
            let key = (source, trigger);
            let index = match tracking.intervals.iter().position(|(k, _)| *k == key) {
                Some(index) => index,
                None => {
                    tracking.intervals.push((key, 0.0));
                    tracking.intervals.len() - 1
                }
            };
            let elapsed = &mut tracking.intervals[index].1;
            *elapsed += dt;
            if *elapsed >= period {
                *elapsed -= period;
                triggers.write(AbilityTriggerEvent(TriggerContext::new(
                    trigger,
                    source,
                    owner,
                    *team,
                    rank,
                    owner_tf.translation,
                    owner_tf.translation,
                )));
            }
        }
    }
}

/// `OnSpellCast` from this frame's casts and `OnSpellHit` from last frame's resolved cast
/// contexts. A cast fans out to every source the owner holds except the cast ability, so an
/// ability never triggers itself; item uses emit no cast and so never count.
pub(crate) fn cast_triggers(
    mut casts: MessageReader<AbilityCastEvent>,
    mut hits: MessageReader<SpellHitEvent>,
    defs: Res<AbilityDefinitions>,
    owners: OwnerQuery,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for cast in casts.read() {
        let unit = cast.target.unit();
        fan_out(
            cast.caster,
            AbilityTrigger::OnSpellCast,
            Some(EffectSource::Ability(cast.ability)),
            &owners,
            &defs,
            &mut triggers,
            |mut ctx| {
                ctx.aim = cast.aim;
                ctx.cast_target(unit).trigger_unit(unit).at(cast.aim)
            },
        );
    }
    for hit in hits.read() {
        fan_out(
            hit.caster,
            AbilityTrigger::OnSpellHit,
            None,
            &owners,
            &defs,
            &mut triggers,
            |ctx| ctx.trigger_unit(Some(hit.target)).at(hit.point),
        );
    }
}

/// `OnStatusApplied` / `OnStatusRemoved` on the status holder, from [`StatusLifecycleEvent`].
/// Each distinct filter a source holds that matches fires once. Pinned after
/// `tick_status_effects`, so expiries are seen the frame they happen.
pub(crate) fn status_triggers(
    mut changes: MessageReader<StatusLifecycleEvent>,
    defs: Res<AbilityDefinitions>,
    owners: OwnerQuery,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for change in changes.read() {
        let Ok((owner_tf, team, inventory, loadout)) = owners.get(change.target) else {
            continue;
        };
        let applied = change.change == StatusLifecycle::Applied;
        for (source, rank) in owner_sources(inventory, loadout) {
            let matching = source_triggers(source, &defs, |trigger| match trigger {
                AbilityTrigger::OnStatusApplied(filter) => {
                    applied && filter.matches(change.id, change.kind)
                }
                AbilityTrigger::OnStatusRemoved(filter) => {
                    !applied && filter.matches(change.id, change.kind)
                }
                _ => false,
            });
            for trigger in matching {
                triggers.write(AbilityTriggerEvent(
                    TriggerContext::new(
                        trigger,
                        source,
                        change.target,
                        *team,
                        rank,
                        owner_tf.translation,
                        owner_tf.translation,
                    )
                    .trigger_unit(change.source.filter(|&s| s != change.target))
                    .status(StatusMeta {
                        id: change.id,
                        kind: change.kind,
                    }),
                ));
            }
        }
    }
}

/// Damage, kill, death and low-health triggers from [`DamageAppliedEvent`] (written by last
/// frame's `apply_damage_events`).
///
/// - Dealt / taken skip reactive damage, DoT ticks, nothing-done hits and self damage, so a
///   reaction can never start another.
/// - Low health fires on the hit that crosses the line while the unit survives.
/// - Kill and death fire once, on the hit that takes health from above zero to zero.
///   They ignore `reactive` and the damage kind: they are bounded edges.
pub(crate) fn damage_triggers(
    mut applied: MessageReader<DamageAppliedEvent>,
    defs: Res<AbilityDefinitions>,
    owners: OwnerQuery,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for hit in applied.read() {
        let at = hit.target_pos;
        let amount = hit.amount;
        let ordinary = !hit.reactive
            && hit.kind != DamageKind::Dot
            && hit.amount > 0.0
            && hit.source != Some(hit.target);

        if ordinary {
            if let Some(source) = hit.source {
                fan_out(
                    source,
                    AbilityTrigger::OnDamageDealt,
                    None,
                    &owners,
                    &defs,
                    &mut triggers,
                    |ctx| ctx.trigger_unit(Some(hit.target)).at(at).amount(amount),
                );
            }
            fan_out(
                hit.target,
                AbilityTrigger::OnDamageTaken,
                None,
                &owners,
                &defs,
                &mut triggers,
                |ctx| ctx.trigger_unit(hit.source).at(at).amount(amount),
            );
        }

        let max = hit.max_health.max(1.0);
        let died = hit.health_before > 0.0 && hit.health_after <= 0.0;
        if hit.health_after > 0.0 && hit.health_after < hit.health_before {
            let (before, after) = (
                hit.health_before / max * 100.0,
                hit.health_after / max * 100.0,
            );
            if let Ok((_, _, inventory, loadout)) = owners.get(hit.target) {
                // Each distinct threshold of each source fires once for the hit.
                for (source, rank) in owner_sources(inventory, loadout) {
                    for trigger in source_triggers(source, &defs, |t| {
                        matches!(t, AbilityTrigger::OnHealthBelow { percent }
                            if before >= f32::from(percent) && f32::from(percent) > after)
                    }) {
                        let Ok((owner_tf, team, ..)) = owners.get(hit.target) else {
                            continue;
                        };
                        triggers.write(AbilityTriggerEvent(
                            TriggerContext::new(
                                trigger,
                                source,
                                hit.target,
                                *team,
                                rank,
                                owner_tf.translation,
                                at,
                            )
                            .trigger_unit(hit.source)
                            .amount(amount),
                        ));
                    }
                }
            }
        }

        if died {
            if let Some(killer) = hit.source.filter(|&k| k != hit.target) {
                fan_out(
                    killer,
                    AbilityTrigger::OnKill,
                    None,
                    &owners,
                    &defs,
                    &mut triggers,
                    |ctx| ctx.trigger_unit(Some(hit.target)).at(at).amount(amount),
                );
            }
            fan_out(
                hit.target,
                AbilityTrigger::OnDeath,
                None,
                &owners,
                &defs,
                &mut triggers,
                |ctx| ctx.trigger_unit(hit.source).at(at).amount(amount),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::AbilityId;

    #[test]
    fn owner_sources_dedups_items_then_learned_abilities() {
        let mut inventory = Inventory::empty();
        for id in [
            ItemId::HeartwoodBand,
            ItemId::SparkPendant,
            ItemId::HeartwoodBand,
        ] {
            assert!(inventory.try_add(id));
        }
        let mut loadout = AbilityLoadout::from_abilities([
            AbilityId::Shockwave,
            AbilityId::Bolt,
            AbilityId::Execute,
            AbilityId::Bulwark,
        ]);
        loadout.slots[0].rank = 2;
        loadout.slots[2].rank = 1;

        let sources: Vec<_> = owner_sources(Some(&inventory), Some(&loadout)).collect();
        assert_eq!(
            sources,
            vec![
                (EffectSource::Item(ItemId::HeartwoodBand), 1),
                (EffectSource::Item(ItemId::SparkPendant), 1),
                (EffectSource::Ability(AbilityId::Shockwave), 2),
                (EffectSource::Ability(AbilityId::Execute), 1),
            ],
            "items in slot order once each, then learned abilities only"
        );
        assert_eq!(owner_sources(None, None).count(), 0);
    }
}
