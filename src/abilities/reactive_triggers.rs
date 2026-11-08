//! Reactive triggers: owner-wide fan-out shared by every dispatcher.
//!
//! Each dispatcher reads one authoritative message (damage applied, status change, cooldown
//! ready, ...) and writes ordinary [`super::effects::AbilityTriggerEvent`]s, which the
//! existing `resolve_ability_triggers` queue resolves. Dispatchers never apply effects.

use bevy::prelude::*;

use super::casting::AbilityCastEvent;
use super::catalog::AbilityDefinitions;
use super::definition::AbilityTrigger;
use super::effects::{
    AbilityTriggerEvent, EffectSource, SpellHitEvent, StatusMeta, TriggerContext,
};
use crate::combat::{DamageAppliedEvent, DamageKind};
use crate::components::{AbilityLoadout, Team};
use crate::items::{Inventory, ItemId};
use crate::status::{StatusLifecycle, StatusLifecycleEvent};

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
