//! Reactive triggers: owner-wide fan-out shared by every dispatcher.
//!
//! Each dispatcher reads one authoritative message (damage applied, status change, cooldown
//! ready, ...) and writes ordinary [`super::effects::AbilityTriggerEvent`]s, which the
//! existing `resolve_ability_triggers` queue resolves. Dispatchers never apply effects.

use super::catalog::AbilityDefinitions;
use super::definition::AbilityTrigger;
use super::effects::EffectSource;
use crate::components::AbilityLoadout;
use crate::items::{Inventory, ItemId};

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
