//! Systems of the shared activation pipeline (see the module docs in `actions`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{
    ActionChannelEndEvent, ActionError, ActionFiredEvent, ActionId, ActionSource, ActionSpec, Activation,
    ActivationRequest, CasterInfo, QueuedActivation, ResolvedTarget, UnitInfo, check_ready,
    in_range, resolve_target,
};
use crate::abilities::casting::{ability_action_spec, commit_ability};
use crate::abilities::catalog::AbilityDefinitions;
use crate::components::{
    AbilityLoadout, AttackMoveOrder, AttackSwing, AttackTarget, BoundRadius, CastTarget,
    CombatStats, Health, Mana, MoveTarget, Team,
};
use crate::dimensions::bounds_of;
use crate::items::{Inventory, StatusEffects, commit_item, item_action_spec};

/// Resolves an [`ActionSource`] to its [`ActionSpec`] and commits it, delegating the
/// source-specific rules to the owning layer.
#[derive(SystemParam)]
pub struct ActionSources<'w, 's> {
    ability_defs: Res<'w, AbilityDefinitions>,
    loadouts: Query<'w, 's, &'static mut AbilityLoadout>,
    inventories: Query<'w, 's, &'static mut Inventory>,
}

impl ActionSources<'_, '_> {
    pub fn spec(&self, caster: Entity, source: ActionSource) -> Result<ActionSpec, ActionError> {
        match source {
            ActionSource::Ability(slot) => {
                let state = self
                    .loadouts
                    .get(caster)
                    .ok()
                    .and_then(|loadout| loadout.slots.get(slot))
                    .ok_or(ActionError::Unavailable)?;
                let def = self
                    .ability_defs
                    .get(state.id)
                    .ok_or(ActionError::Unavailable)?;
                ability_action_spec(def, state).map_err(|_| ActionError::Unavailable)
            }
            ActionSource::Item(slot) => {
                let inventory = self.inventories.get(caster).map_err(|_| ActionError::Unavailable)?;
                item_action_spec(inventory, slot).map_err(|_| ActionError::Unavailable)
            }
        }
    }

    /// Start the cooldown / spend the charge and apply source rules (e.g. toggles).
    /// Costs are paid by the pipeline before this is called.
    pub fn commit(&mut self, caster: Entity, source: ActionSource, spec: &ActionSpec) {
        match source {
            ActionSource::Ability(slot) => {
                let Ok(mut loadout) = self.loadouts.get_mut(caster) else {
                    return;
                };
                let Some(state) = loadout.slot_mut(slot) else {
                    return;
                };
                if let Some(def) = self.ability_defs.get(state.id) {
                    commit_ability(def, state, spec);
                }
            }
            ActionSource::Item(slot) => {
                if let Ok(mut inventory) = self.inventories.get_mut(caster) {
                    commit_item(&mut inventory, slot, spec);
                }
            }
        }
    }

    /// Where `action` lives now and its current spec. Ability slots must still hold the
    /// same ability; items follow their instance even if it moved to another slot.
    fn current(
        &self,
        caster: Entity,
        source: ActionSource,
        action: ActionId,
    ) -> Option<(ActionSource, ActionSpec)> {
        let source = match (source, action) {
            (ActionSource::Item(_), ActionId::Item { instance, .. }) => {
                ActionSource::Item(self.inventories.get(caster).ok()?.slot_of(instance)?)
            }
            _ => source,
        };
        self.spec(caster, source)
            .ok()
            .filter(|spec| spec.action == action)
            .map(|spec| (source, spec))
    }
}

/// Pay, commit, and announce an action whose cast point is over.
#[allow(clippy::too_many_arguments)]
fn fire(
    sources: &mut ActionSources,
    fired: &mut MessageWriter<ActionFiredEvent>,
    mana: &mut Mana,
    statuses: Option<&StatusEffects>,
    caster: Entity,
    source: ActionSource,
    spec: &ActionSpec,
    target: CastTarget,
    aim: Vec3,
) -> bool {
    if check_ready(&spec.cooldown_state, &spec.cost, spec.disable, mana, statuses).is_err()
        || !spec.cost.pay(mana)
    {
        return false;
    }
    sources.commit(caster, source, spec);
    fired.write(ActionFiredEvent {
        caster,
        source,
        action: spec.action,
        level: spec.level,
        target,
        aim,
    });
    true
}

type CasterQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Team,
        &'static mut Mana,
        Option<&'static StatusEffects>,
        Option<&'static BoundRadius>,
    ),
>;

type UnitQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Team,
        &'static Health,
        Option<&'static BoundRadius>,
    ),
>;

fn unit_info(units: &UnitQuery, entity: Entity) -> Option<UnitInfo> {
    units
        .get(entity)
        .ok()
        .map(|(tf, team, health, bound)| UnitInfo {
            position: tf.translation,
            team: *team,
            alive: health.is_alive(),
            bounds: bounds_of(bound),
        })
}

pub fn validate_activation_requests(
    mut commands: Commands,
    mut sources: ActionSources,
    mut requests: MessageReader<ActivationRequest>,
    mut fired: MessageWriter<ActionFiredEvent>,
    mut casters: CasterQuery,
    units: UnitQuery,
) {
    for request in requests.read() {
        let Ok((transform, team, mut mana, statuses, bound)) = casters.get_mut(request.caster)
        else {
            continue;
        };
        let Ok(spec) = sources.spec(request.caster, request.source) else {
            continue;
        };
        if check_ready(&spec.cooldown_state, &spec.cost, spec.disable, &mana, statuses).is_err() {
            continue;
        }
        let caster = CasterInfo {
            entity: request.caster,
            team: *team,
            position: transform.translation,
            bounds: bounds_of(bound),
        };
        let Ok(resolved) = resolve_target(&spec, &caster, request.target, |e| unit_info(&units, e))
        else {
            continue;
        };

        let in_range = in_range(&spec, caster.position, caster.bounds, &resolved);
        if in_range && !spec.occupies_caster {
            fire(
                &mut sources,
                &mut fired,
                &mut mana,
                statuses,
                request.caster,
                request.source,
                &spec,
                resolved.target,
                resolved.aim,
            );
            continue;
        }

        let mut entity = commands.entity(request.caster);
        entity.remove::<(AttackTarget, AttackMoveOrder)>();
        if !in_range {
            entity.insert((
                QueuedActivation {
                    source: request.source,
                    action: spec.action,
                    target: resolved.target,
                    aim: resolved.aim,
                },
                MoveTarget {
                    position: resolved.aim,
                },
            ));
            continue;
        }
        entity
            .remove::<(MoveTarget, AttackSwing, QueuedActivation)>()
            .insert(Activation {
                source: request.source,
                action: spec.action,
                level: spec.level,
                target: resolved.target,
                aim: resolved.aim,
                disable: spec.disable,
                point_remaining: spec.cast_time,
                channel_remaining: spec.channel_time,
                backswing_remaining: spec.backswing,
                fired: false,
            });
    }
}

/// Walk queued activations into range, then hand them back to validation as a fresh request.
pub fn resolve_queued_activations(
    mut commands: Commands,
    sources: ActionSources,
    mut requests: MessageWriter<ActivationRequest>,
    mut casters: Query<(
        Entity,
        &Transform,
        &mut QueuedActivation,
        Option<&BoundRadius>,
    )>,
    units: UnitQuery,
) {
    for (caster, transform, mut queued, bound) in &mut casters {
        let Some((source, spec)) = sources.current(caster, queued.source, queued.action) else {
            commands
                .entity(caster)
                .remove::<(QueuedActivation, MoveTarget)>();
            continue;
        };
        queued.source = source;

        let mut target_bounds = None;
        if let CastTarget::Unit(entity) = queued.target {
            match unit_info(&units, entity).filter(|info| info.alive) {
                Some(info) => {
                    queued.aim = info.position;
                    target_bounds = Some(info.bounds);
                }
                None if spec.target_type.needs_unit() => {
                    commands
                        .entity(caster)
                        .remove::<(QueuedActivation, MoveTarget)>();
                    continue;
                }
                None => queued.target = CastTarget::Point(queued.aim),
            }
        }
        let resolved = ResolvedTarget {
            target: queued.target,
            aim: queued.aim,
            target_bounds,
        };
        if !in_range(&spec, transform.translation, bounds_of(bound), &resolved) {
            commands.entity(caster).insert(MoveTarget {
                position: queued.aim,
            });
            continue;
        }
        requests.write(ActivationRequest {
            caster,
            source: queued.source,
            target: queued.target,
        });
        commands
            .entity(caster)
            .remove::<(QueuedActivation, MoveTarget)>();
    }
}

/// Turn toward the aim, run the cast point, then commit costs and emit
/// [`ActionFiredEvent`]; then channel and backswing. Cancelling during the cast point
/// (new order, silence, stun) is free.
pub fn tick_activations(
    time: Res<Time>,
    mut commands: Commands,
    mut sources: ActionSources,
    mut fired: MessageWriter<ActionFiredEvent>,
    mut channel_ends: MessageWriter<ActionChannelEndEvent>,
    mut casters: Query<(
        Entity,
        &mut Transform,
        &mut Activation,
        &mut Mana,
        Option<&StatusEffects>,
        &CombatStats,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut activation, mut mana, statuses, stats) in &mut casters {
        let disabled = activation.disable.blocks(statuses);
        if activation.fired {
            if activation.channel_remaining > 0.0 {
                activation.channel_remaining = if disabled {
                    0.0
                } else {
                    (activation.channel_remaining - dt).max(0.0)
                };
                if activation.channel_remaining <= 0.0 {
                    channel_ends.write(ActionChannelEndEvent {
                        caster: entity,
                        source: activation.source,
                        action: activation.action,
                        level: activation.level,
                        target: activation.target,
                        aim: activation.aim,
                        completed: !disabled,
                    });
                }
                continue;
            }
            activation.backswing_remaining = (activation.backswing_remaining - dt).max(0.0);
            if activation.backswing_remaining <= 0.0 {
                commands.entity(entity).remove::<Activation>();
            }
            continue;
        }
        if disabled {
            commands.entity(entity).remove::<Activation>();
            continue;
        }
        let dir = activation.aim - transform.translation;
        if !crate::facing::turn_toward(&mut transform, dir, stats.turn_rate, dt) {
            continue;
        }
        activation.point_remaining = (activation.point_remaining - dt).max(0.0);
        if activation.point_remaining > 0.0 {
            continue;
        }

        let Some((source, spec)) = sources.current(entity, activation.source, activation.action)
        else {
            commands.entity(entity).remove::<Activation>();
            continue;
        };
        let (target, aim) = (activation.target, activation.aim);
        if !fire(
            &mut sources,
            &mut fired,
            &mut mana,
            statuses,
            entity,
            source,
            &spec,
            target,
            aim,
        ) {
            commands.entity(entity).remove::<Activation>();
            continue;
        }
        activation.source = source;
        activation.fired = true;
        activation.level = spec.level;
        if activation.channel_remaining <= 0.0 && activation.backswing_remaining <= 0.0 {
            commands.entity(entity).remove::<Activation>();
        }
    }
}
