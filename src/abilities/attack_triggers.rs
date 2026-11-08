//! Basic-attack triggers for items and learned abilities.
//!
//! `OnAttack` fires when an attack is released (hit or not); `OnAttackHit` fires only for
//! attacks that landed ([`BasicAttackLandedEvent`]). Every distinct item the attacker
//! carries, and every learned ability, with entries for the trigger gets one
//! [`AbilityTriggerEvent`] per attack, stamped with [`AttackMeta`] so damage dealt under it
//! is attack bonus damage.

use bevy::prelude::*;

use super::catalog::AbilityDefinitions;
use super::definition::AbilityTrigger;
use super::effects::{AbilityTriggerEvent, AttackMeta, EffectSource, TriggerContext};
use super::reactive_triggers::{owner_sources, source_has_trigger};
use crate::basic_attack::{BasicAttackLandedEvent, BasicAttackReleaseEvent};
use crate::components::{AbilityLoadout, Team};
use crate::items::Inventory;

pub(crate) fn attack_triggers(
    mut releases: MessageReader<BasicAttackReleaseEvent>,
    mut landed: MessageReader<BasicAttackLandedEvent>,
    defs: Res<AbilityDefinitions>,
    owners: Query<(
        &Transform,
        &Team,
        Option<&Inventory>,
        Option<&AbilityLoadout>,
    )>,
    targets: Query<&Transform>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    let fired = releases
        .read()
        .map(|e| {
            let meta = AttackMeta {
                extra: e.extra,
                crit: false,
            };
            (AbilityTrigger::OnAttack, e.attacker, e.target, meta, 1.0)
        })
        .chain(landed.read().map(|e| {
            let meta = AttackMeta {
                extra: e.extra,
                crit: e.crit,
            };
            (
                AbilityTrigger::OnAttackHit,
                e.attacker,
                e.target,
                meta,
                e.damage,
            )
        }));
    for (trigger, attacker, target, meta, amount) in fired {
        debug_assert!(trigger.is_attack());
        let Ok((owner_tf, team, inventory, loadout)) = owners.get(attacker) else {
            continue;
        };
        let point = targets
            .get(target)
            .map_or(owner_tf.translation, |tf| tf.translation);
        let mut fire = |source: EffectSource, rank: u32| {
            triggers.write(AbilityTriggerEvent(
                TriggerContext::new(
                    trigger,
                    source,
                    attacker,
                    *team,
                    rank,
                    owner_tf.translation,
                    point,
                )
                .trigger_unit(Some(target))
                .attack(meta)
                .amount(amount),
            ));
        };

        for (source, rank) in owner_sources(inventory, loadout) {
            if source_has_trigger(source, &defs, trigger) {
                fire(source, rank);
            }
        }
    }
}
