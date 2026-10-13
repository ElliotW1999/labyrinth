//! Ability layer of the shared activation pipeline (`crate::actions`):
//!
//! ```text
//! ActivationRequest { source: Ability(slot) }
//!   → ability_action_spec: castable (not passive), learned → ActionSpec at the slot's rank
//!   → actions pipeline: cooldown / charges, mana, targeting, range, cast point, channel
//!   → commit_ability: cooldown, charge, toggle state
//!   → ActionFiredEvent → emit_ability_casts → AbilityCastEvent (consumed by `mechanics`)
//!   → ActionChannelEndEvent → ability_channel_end_triggers → OnChannelEnd trigger
//! ```

use bevy::prelude::*;

use super::definition::{AbilityBehavior, AbilityDefinition, AbilityTrigger};
use super::effects::{AbilityTriggerEvent, EffectSource, TriggerContext};
use crate::actions::{
    ActionChannelEndEvent, ActionCost, ActionFiredEvent, ActionId, ActionSource, ActionSpec,
    DisableRule,
};
use crate::components::{AbilityId, AbilityState, CastTarget, Team};

/// A cast passed validation, finished its cast point, and paid its costs.
/// Effects and custom behaviors run from this event.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct AbilityCastEvent {
    pub caster: Entity,
    pub slot: usize,
    pub ability: AbilityId,
    pub rank: u32,
    pub target: CastTarget,
    pub aim: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastError {
    Passive,
    NotLearned,
}

/// The shared activation rules of `def` at the slot's rank. Fails for abilities that
/// can never be activated from this slot (passive, unlearned).
pub fn ability_action_spec(
    def: &AbilityDefinition,
    state: &AbilityState,
) -> Result<ActionSpec, CastError> {
    if !def.is_castable() {
        return Err(CastError::Passive);
    }
    if state.rank == 0 {
        return Err(CastError::NotLearned);
    }
    Ok(ActionSpec {
        action: ActionId::Ability(state.id),
        level: state.rank,
        target_type: def.target_type,
        target_team: def.target_team,
        cast_range: def.cast_range.at(state.rank),
        cast_time: def.cast_time,
        channel_time: def.channel_time,
        backswing: def.backswing,
        cost: ActionCost {
            mana: def.mana_cost.at(state.rank),
        },
        cooldown: def.cooldown.at(state.rank),
        cooldown_state: state.cooldown,
        disable: DisableRule::SilenceOrStun,
        occupies_caster: true,
    })
}

/// Ability bookkeeping once a cast commits (mana is already paid).
pub fn commit_ability(def: &AbilityDefinition, state: &mut AbilityState, spec: &ActionSpec) {
    state.cooldown.start(spec.cooldown);
    if def.behavior == AbilityBehavior::Toggle {
        state.toggled = !state.toggled;
    }
}

pub fn emit_ability_casts(
    mut fired: MessageReader<ActionFiredEvent>,
    mut casts: MessageWriter<AbilityCastEvent>,
) {
    for event in fired.read() {
        let (ActionSource::Ability(slot), ActionId::Ability(ability)) =
            (event.source, event.action)
        else {
            continue;
        };
        casts.write(AbilityCastEvent {
            caster: event.caster,
            slot,
            ability,
            rank: event.level,
            target: event.target,
            aim: event.aim,
        });
    }
}

/// `OnChannelEnd` for channelled abilities, whether the channel completed or the
/// caster was disabled.
pub fn ability_channel_end_triggers(
    mut ends: MessageReader<ActionChannelEndEvent>,
    casters: Query<(&Transform, &Team)>,
    mut triggers: MessageWriter<AbilityTriggerEvent>,
) {
    for end in ends.read() {
        let ActionId::Ability(ability) = end.action else {
            continue;
        };
        let Ok((transform, team)) = casters.get(end.caster) else {
            continue;
        };
        triggers.write(AbilityTriggerEvent(TriggerContext {
            trigger: AbilityTrigger::OnChannelEnd,
            caster: end.caster,
            team: *team,
            source: EffectSource::Ability(ability),
            rank: end.level,
            origin: transform.translation,
            aim: end.aim,
            point: end.aim,
            cast_target: end.target.unit(),
            trigger_unit: None,
            affected_units: Vec::new(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::catalog::builtin;
    use crate::actions::{
        ActionCooldown, ActionError, CasterInfo, UnitInfo, check_ready, resolve_target,
    };
    use crate::components::Mana;
    use crate::items::StatusEffects;

    #[derive(Debug, PartialEq)]
    enum Unusable {
        Cast(CastError),
        Action(ActionError),
    }

    /// What the hotkeys and the pipeline check before an ability activates.
    fn check_usable(
        def: &AbilityDefinition,
        state: &AbilityState,
        mana: &Mana,
        statuses: Option<&StatusEffects>,
    ) -> Result<(), Unusable> {
        let spec = ability_action_spec(def, state).map_err(Unusable::Cast)?;
        check_ready(&spec.cooldown_state, &spec.cost, spec.disable, mana, statuses)
            .map_err(Unusable::Action)
    }

    fn learned(id: AbilityId) -> AbilityState {
        AbilityState {
            rank: 1,
            ..AbilityState::fresh(id)
        }
    }

    #[test]
    fn usable_checks_rank_cooldown_mana_and_passive() {
        let bolt = builtin(AbilityId::Bolt);
        let mana = Mana::new(500.0, 0.0);
        assert_eq!(
            check_usable(&bolt, &learned(AbilityId::Bolt), &mana, None),
            Ok(())
        );
        assert_eq!(
            check_usable(&bolt, &AbilityState::fresh(AbilityId::Bolt), &mana, None),
            Err(Unusable::Cast(CastError::NotLearned))
        );
        let cooling = AbilityState {
            cooldown: ActionCooldown {
                remaining: 1.0,
                ..Default::default()
            },
            ..learned(AbilityId::Bolt)
        };
        assert_eq!(
            check_usable(&bolt, &cooling, &mana, None),
            Err(Unusable::Action(ActionError::OnCooldown))
        );
        assert_eq!(
            check_usable(
                &bolt,
                &learned(AbilityId::Bolt),
                &Mana::new(10.0, 0.0),
                None
            ),
            Err(Unusable::Action(ActionError::NotEnoughMana))
        );
        let stone = builtin(AbilityId::StoneSkin);
        assert_eq!(
            check_usable(&stone, &learned(AbilityId::StoneSkin), &mana, None),
            Err(Unusable::Cast(CastError::Passive))
        );
    }

    #[test]
    fn spec_reads_the_definition_at_the_slot_rank() {
        let bolt = builtin(AbilityId::Bolt);
        let state = AbilityState {
            rank: 3,
            ..AbilityState::fresh(AbilityId::Bolt)
        };
        let spec = ability_action_spec(&bolt, &state).unwrap();
        assert_eq!(spec.action, ActionId::Ability(AbilityId::Bolt));
        assert_eq!(spec.level, 3);
        assert_eq!(spec.cast_range, bolt.cast_range.at(3));
        assert_eq!(spec.cost.mana, bolt.mana_cost.at(3));
        assert_eq!(spec.cooldown, bolt.cooldown.at(3));
        assert_eq!(spec.target_type, bolt.target_type);
    }

    #[test]
    fn commit_starts_cooldown_spends_a_charge_and_flips_toggles() {
        let def = AbilityDefinition::new(AbilityId::Bolt)
            .costs(
                crate::abilities::definition::RankValue::fixed(4.0),
                crate::abilities::definition::RankValue::ZERO,
            )
            .behavior(AbilityBehavior::Toggle)
            .charges(2, 10.0);
        let mut state = learned(AbilityId::Bolt);
        state.cooldown.init_charges(2);
        let spec = ability_action_spec(&def, &state).unwrap();
        commit_ability(&def, &mut state, &spec);
        assert_eq!(state.cooldown.remaining, 4.0);
        assert_eq!(state.cooldown.charges, Some(1));
        assert!(state.toggled);
    }

    #[test]
    fn unit_targets_must_match_team_and_be_alive() {
        let bolt =
            ability_action_spec(&builtin(AbilityId::Bolt), &learned(AbilityId::Bolt)).unwrap();
        let caster = CasterInfo {
            entity: Entity::from_raw_u32(1).unwrap(),
            team: Team::Radiant,
            position: Vec3::ZERO,
            bounds: 0.0,
        };
        let enemy = Entity::from_raw_u32(7).unwrap();
        let info = |team, alive| {
            move |_| {
                Some(UnitInfo {
                    position: Vec3::X * 100.0,
                    team,
                    alive,
                    bounds: 10.0,
                })
            }
        };
        let ok = resolve_target(
            &bolt,
            &caster,
            CastTarget::Unit(enemy),
            info(Team::Dire, true),
        );
        assert_eq!(ok.unwrap().aim, Vec3::X * 100.0);
        let ally = resolve_target(
            &bolt,
            &caster,
            CastTarget::Unit(enemy),
            info(Team::Radiant, true),
        );
        assert_eq!(ally, Err(ActionError::InvalidTarget));
        let dead = resolve_target(
            &bolt,
            &caster,
            CastTarget::Unit(enemy),
            info(Team::Dire, false),
        );
        assert_eq!(dead, Err(ActionError::InvalidTarget));
        let point = resolve_target(
            &bolt,
            &caster,
            CastTarget::Point(Vec3::X),
            info(Team::Dire, true),
        );
        assert_eq!(point, Err(ActionError::InvalidTarget));

        let nova =
            ability_action_spec(&builtin(AbilityId::Nova), &learned(AbilityId::Nova)).unwrap();
        let ground = resolve_target(
            &nova,
            &caster,
            CastTarget::Point(Vec3::X),
            info(Team::Dire, true),
        );
        assert_eq!(ground.unwrap().aim, Vec3::X);
    }
}
