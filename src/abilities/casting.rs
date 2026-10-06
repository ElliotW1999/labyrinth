//! Shared casting pipeline for every caster (player input, AI, network, tests):
//!
//! ```text
//! AbilityCastRequest → validate_cast_requests ─┬─ out of range → QueuedAbilityCast (walk, re-request)
//!                                              └─ ok → AbilityCasting (cast point)
//! tick_ability_casting: cast point done → re-validate, spend mana, start cooldown
//!   → AbilityCastEvent (consumed by `effects::execute_ability_casts`)
//! ```

use bevy::prelude::*;

use super::catalog::AbilityDefinitions;
use super::definition::{AbilityBehavior, AbilityDefinition, TargetTeam, TargetType};
use crate::components::{
    AbilityCasting, AbilityId, AbilityLoadout, AbilityState, AttackMoveOrder, AttackSwing,
    AttackTarget, BoundRadius, CastTarget, CombatStats, Health, Mana, MoveTarget,
    QueuedAbilityCast, Team,
};
use crate::dimensions::{bounds_of, cast_distance};
use crate::items::StatusEffects;

/// Attempt to cast the ability in `slot` of `caster`'s loadout.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct AbilityCastRequest {
    pub caster: Entity,
    pub slot: usize,
    pub target: CastTarget,
}

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
    OnCooldown,
    NotEnoughMana,
    CasterDisabled,
    InvalidTarget,
}

/// Ability-level checks shared by hotkeys (feedback), request validation, and the
/// cast-point commit: castable, learned, off cooldown, affordable, caster not silenced/stunned.
pub fn check_usable(
    def: &AbilityDefinition,
    state: &AbilityState,
    mana: &Mana,
    statuses: Option<&StatusEffects>,
) -> Result<(), CastError> {
    if !def.is_castable() {
        return Err(CastError::Passive);
    }
    if state.rank == 0 {
        return Err(CastError::NotLearned);
    }
    if state.cooldown_remaining > 0.0 || state.charges == Some(0) {
        return Err(CastError::OnCooldown);
    }
    if mana.current < def.mana_cost.at(state.rank) {
        return Err(CastError::NotEnoughMana);
    }
    if statuses.is_some_and(|s| !s.can_cast()) {
        return Err(CastError::CasterDisabled);
    }
    Ok(())
}

pub fn team_allows(rule: TargetTeam, caster: Team, target: Team) -> bool {
    match rule {
        TargetTeam::Enemy => target == caster.enemy(),
        TargetTeam::Ally => target == caster,
        TargetTeam::Any => true,
    }
}

/// Snapshot of a candidate unit target.
#[derive(Debug, Clone, Copy)]
pub struct UnitInfo {
    pub position: Vec3,
    pub team: Team,
    pub alive: bool,
    pub bounds: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedTarget {
    pub target: CastTarget,
    pub aim: Vec3,
    pub target_bounds: Option<f32>,
}

/// Validate `target` against the definition's targeting rules.
pub fn resolve_target(
    def: &AbilityDefinition,
    caster_team: Team,
    caster_pos: Vec3,
    target: CastTarget,
    lookup: impl Fn(Entity) -> Option<UnitInfo>,
) -> Result<ResolvedTarget, CastError> {
    if def.target_type == TargetType::NoTarget {
        return Ok(ResolvedTarget {
            target: CastTarget::None,
            aim: caster_pos,
            target_bounds: None,
        });
    }
    match target {
        CastTarget::Unit(entity) => {
            let info = lookup(entity)
                .filter(|info| info.alive && team_allows(def.target_team, caster_team, info.team))
                .ok_or(CastError::InvalidTarget)?;
            Ok(ResolvedTarget {
                target,
                aim: info.position,
                target_bounds: Some(info.bounds),
            })
        }
        CastTarget::Point(point) if def.target_type != TargetType::Unit => Ok(ResolvedTarget {
            target,
            aim: point,
            target_bounds: None,
        }),
        _ => Err(CastError::InvalidTarget),
    }
}

pub fn in_cast_range(def: &AbilityDefinition, rank: u32, caster_pos: Vec3, caster_bounds: f32, resolved: &ResolvedTarget) -> bool {
    def.target_type == TargetType::NoTarget
        || cast_distance(caster_pos, caster_bounds, resolved.aim, resolved.target_bounds)
            <= def.cast_range.at(rank)
}

type CasterQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Team,
        &'static AbilityLoadout,
        &'static Mana,
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
    units.get(entity).ok().map(|(tf, team, health, bound)| UnitInfo {
        position: tf.translation,
        team: *team,
        alive: health.is_alive(),
        bounds: bounds_of(bound),
    })
}

pub fn validate_cast_requests(
    mut commands: Commands,
    defs: Res<AbilityDefinitions>,
    mut requests: MessageReader<AbilityCastRequest>,
    casters: CasterQuery,
    units: UnitQuery,
) {
    for request in requests.read() {
        let Ok((transform, team, loadout, mana, statuses, bound)) = casters.get(request.caster) else {
            continue;
        };
        let Some(state) = loadout.slots.get(request.slot) else {
            continue;
        };
        let Some(def) = defs.get(state.id) else {
            continue;
        };
        if check_usable(def, state, mana, statuses).is_err() {
            continue;
        }
        let caster_pos = transform.translation;
        let Ok(resolved) = resolve_target(def, *team, caster_pos, request.target, |e| unit_info(&units, e))
        else {
            continue;
        };

        let mut caster = commands.entity(request.caster);
        caster.remove::<(AttackTarget, AttackMoveOrder)>();
        if !in_cast_range(def, state.rank, caster_pos, bounds_of(bound), &resolved) {
            caster.insert((
                QueuedAbilityCast {
                    slot: request.slot,
                    ability: state.id,
                    target: resolved.target,
                    aim: resolved.aim,
                },
                MoveTarget {
                    position: resolved.aim,
                },
            ));
            continue;
        }
        caster
            .remove::<(MoveTarget, AttackSwing, QueuedAbilityCast)>()
            .insert(AbilityCasting {
                slot: request.slot,
                ability: state.id,
                target: resolved.target,
                aim: resolved.aim,
                point_remaining: def.cast_time,
                backswing_remaining: def.backswing,
                fired: false,
            });
    }
}

/// Walk queued casts into range, then hand them back to validation as a fresh request.
pub fn resolve_queued_ability_casts(
    mut commands: Commands,
    defs: Res<AbilityDefinitions>,
    mut requests: MessageWriter<AbilityCastRequest>,
    mut casters: Query<(
        Entity,
        &Transform,
        &mut QueuedAbilityCast,
        &AbilityLoadout,
        Option<&BoundRadius>,
    )>,
    units: UnitQuery,
) {
    for (caster, transform, mut queued, loadout, bound) in &mut casters {
        let def = defs.get(queued.ability);
        let rank = loadout.slots.get(queued.slot).map(|s| s.rank);
        let (Some(def), Some(rank)) = (def, rank) else {
            commands.entity(caster).remove::<(QueuedAbilityCast, MoveTarget)>();
            continue;
        };

        let mut target_bounds = None;
        if let CastTarget::Unit(entity) = queued.target {
            match unit_info(&units, entity).filter(|info| info.alive) {
                Some(info) => {
                    queued.aim = info.position;
                    target_bounds = Some(info.bounds);
                }
                None if def.target_type == TargetType::Unit => {
                    commands.entity(caster).remove::<(QueuedAbilityCast, MoveTarget)>();
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
        if !in_cast_range(def, rank, transform.translation, bounds_of(bound), &resolved) {
            commands.entity(caster).insert(MoveTarget {
                position: queued.aim,
            });
            continue;
        }
        requests.write(AbilityCastRequest {
            caster,
            slot: queued.slot,
            target: queued.target,
        });
        commands.entity(caster).remove::<(QueuedAbilityCast, MoveTarget)>();
    }
}

/// Turn toward the aim, run the cast point, then commit costs and emit [`AbilityCastEvent`].
/// Cancelling during the cast point (new order, silence, stun) is free.
pub fn tick_ability_casting(
    time: Res<Time>,
    mut commands: Commands,
    defs: Res<AbilityDefinitions>,
    mut cast_events: MessageWriter<AbilityCastEvent>,
    mut casters: Query<(
        Entity,
        &mut Transform,
        &mut AbilityCasting,
        &mut AbilityLoadout,
        &mut Mana,
        Option<&StatusEffects>,
        &CombatStats,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut casting, mut loadout, mut mana, statuses, stats) in &mut casters {
        if casting.fired {
            casting.backswing_remaining = (casting.backswing_remaining - dt).max(0.0);
            if casting.backswing_remaining <= 0.0 {
                commands.entity(entity).remove::<AbilityCasting>();
            }
            continue;
        }
        if statuses.is_some_and(|s| !s.can_cast()) {
            commands.entity(entity).remove::<AbilityCasting>();
            continue;
        }
        let dir = casting.aim - transform.translation;
        if !crate::facing::turn_toward(&mut transform, dir, stats.turn_rate, dt) {
            continue;
        }
        casting.point_remaining = (casting.point_remaining - dt).max(0.0);
        if casting.point_remaining > 0.0 {
            continue;
        }

        let Some(state) = loadout.slot_mut(casting.slot).filter(|s| s.id == casting.ability) else {
            commands.entity(entity).remove::<AbilityCasting>();
            continue;
        };
        let Some(def) = defs.get(state.id) else {
            commands.entity(entity).remove::<AbilityCasting>();
            continue;
        };
        if check_usable(def, state, &mana, statuses).is_err()
            || !mana.try_spend(def.mana_cost.at(state.rank))
        {
            commands.entity(entity).remove::<AbilityCasting>();
            continue;
        }
        state.cooldown_remaining = def.cooldown.at(state.rank);
        if let Some(charges) = state.charges.as_mut() {
            *charges -= 1;
        }
        if def.behavior == AbilityBehavior::Toggle {
            state.toggled = !state.toggled;
        }

        casting.fired = true;
        cast_events.write(AbilityCastEvent {
            caster: entity,
            slot: casting.slot,
            ability: state.id,
            rank: state.rank,
            target: casting.target,
            aim: casting.aim,
        });
        if casting.backswing_remaining <= 0.0 {
            commands.entity(entity).remove::<AbilityCasting>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::catalog::builtin;

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
        assert_eq!(check_usable(&bolt, &learned(AbilityId::Bolt), &mana, None), Ok(()));
        assert_eq!(
            check_usable(&bolt, &AbilityState::fresh(AbilityId::Bolt), &mana, None),
            Err(CastError::NotLearned)
        );
        let cooling = AbilityState {
            cooldown_remaining: 1.0,
            ..learned(AbilityId::Bolt)
        };
        assert_eq!(check_usable(&bolt, &cooling, &mana, None), Err(CastError::OnCooldown));
        assert_eq!(
            check_usable(&bolt, &learned(AbilityId::Bolt), &Mana::new(10.0, 0.0), None),
            Err(CastError::NotEnoughMana)
        );
        let stone = builtin(AbilityId::StoneSkin);
        assert_eq!(
            check_usable(&stone, &learned(AbilityId::StoneSkin), &mana, None),
            Err(CastError::Passive)
        );
    }

    #[test]
    fn unit_targets_must_match_team_and_be_alive() {
        let bolt = builtin(AbilityId::Bolt);
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
        let ok = resolve_target(&bolt, Team::Radiant, Vec3::ZERO, CastTarget::Unit(enemy), info(Team::Dire, true));
        assert_eq!(ok.unwrap().aim, Vec3::X * 100.0);
        let ally = resolve_target(&bolt, Team::Radiant, Vec3::ZERO, CastTarget::Unit(enemy), info(Team::Radiant, true));
        assert_eq!(ally, Err(CastError::InvalidTarget));
        let dead = resolve_target(&bolt, Team::Radiant, Vec3::ZERO, CastTarget::Unit(enemy), info(Team::Dire, false));
        assert_eq!(dead, Err(CastError::InvalidTarget));
        let point = resolve_target(&bolt, Team::Radiant, Vec3::ZERO, CastTarget::Point(Vec3::X), info(Team::Dire, true));
        assert_eq!(point, Err(CastError::InvalidTarget));

        let nova = builtin(AbilityId::Nova);
        let ground = resolve_target(&nova, Team::Radiant, Vec3::ZERO, CastTarget::Point(Vec3::X), info(Team::Dire, true));
        assert_eq!(ground.unwrap().aim, Vec3::X);
    }
}
