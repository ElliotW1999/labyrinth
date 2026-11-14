//! Creep lane following, lane-less hunting, and tower / creep aggro.
//!
//! The player hero is intentionally excluded — player orders must never be
//! overwritten by AI aggro/chase logic.

use bevy::prelude::*;

use crate::actions::QueuedActivation;
use crate::components::{
    AttackMoveOrder, AttackSwing, AttackTarget, BoundRadius, CombatStats, Creep, Health,
    MoveTarget, PlayerHero, Team, Tower, TrainingDummy,
};
use crate::dimensions::{bounds_of, center_distance as flat_distance, edge_distance, within_range};
use crate::movement::{FLEE_DISTANCE, SimSet, flee_direction};
use crate::navigation::attack_position::{AttackPositionGoal, assign_attack_positions};
use crate::navigation::{flat, ground};
use crate::status::{
    ActionBlocks, Compulsion, StatusChange, StatusEffectEvent, StatusEffects, TargetIntent,
    is_targetable,
};
use crate::unit_commands::CommandQueue;

/// A lane waypoint counts as passed within this distance, so a wave crowding the
/// same point doesn't stall on it.
const LANE_WAYPOINT_REACH: f32 = 160.0;

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                follow_lane_waypoints,
                hunt_enemies,
                acquire_targets,
                player_attack_move,
                enforce_compulsions,
                assign_attack_positions,
            )
                .chain()
                .in_set(SimSet::Ai)
                .run_if(crate::net::is_sim_authority),
        );
    }
}

#[derive(Component, Debug, Clone)]
pub struct LaneFollower {
    pub waypoints: Vec<Vec3>,
    pub index: usize,
}

impl LaneFollower {
    /// A follower that continues `waypoints` from `position` instead of from the start: it
    /// aims at the nearest waypoint, or the one after it when the unit is already past the
    /// nearest one (on the far side of it, seen along the lane), so it never walks back.
    pub fn resume_from(waypoints: Vec<Vec3>, position: Vec3) -> Self {
        let nearest = waypoints
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                flat_distance(**a, position).total_cmp(&flat_distance(**b, position))
            })
            .map_or(0, |(index, _)| index);
        let index = match waypoints.get(nearest + 1) {
            Some(next) => {
                let along = Vec2::new(next.x - waypoints[nearest].x, next.z - waypoints[nearest].z);
                let offset = Vec2::new(
                    position.x - waypoints[nearest].x,
                    position.z - waypoints[nearest].z,
                );
                if along.dot(offset) > 0.0 {
                    nearest + 1
                } else {
                    nearest
                }
            }
            None => nearest,
        };
        Self { waypoints, index }
    }
}

/// Aggro of a creep that follows no lane (placed by a game mode, e.g. Story rooms): it walks
/// to the nearest living enemy within `aggro_radius`, and [`acquire_targets`] attacks it
/// once it is in range.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Hunter {
    pub aggro_radius: f32,
}

/// Whether a status on the unit takes its orders away (a compulsion is in charge instead).
fn orders_blocked(statuses: Option<&StatusEffects>) -> bool {
    statuses.is_some_and(|s| s.blocks().contains(ActionBlocks::ORDERS))
}

fn follow_lane_waypoints(
    mut creeps: Query<
        (
            Entity,
            &Transform,
            &mut LaneFollower,
            Option<&AttackTarget>,
            Option<&StatusEffects>,
        ),
        With<Creep>,
    >,
    mut commands: Commands,
) {
    for (entity, transform, mut follower, attack, statuses) in &mut creeps {
        if attack.is_some() || orders_blocked(statuses) {
            continue;
        }
        if follower.index >= follower.waypoints.len() {
            continue;
        }
        let waypoint = follower.waypoints[follower.index];
        if flat_distance(transform.translation, waypoint) < LANE_WAYPOINT_REACH {
            follower.index += 1;
            if follower.index >= follower.waypoints.len() {
                commands.entity(entity).remove::<MoveTarget>();
                continue;
            }
        }
        let next = follower.waypoints[follower.index.min(follower.waypoints.len() - 1)];
        commands.entity(entity).insert(MoveTarget {
            position: Vec3::new(next.x, 0.0, next.z),
        });
    }
}

/// Hunters without an attack target walk toward the nearest enemy they may attack within
/// their aggro radius. With none in range they keep their last move and idle. Units whose
/// orders a compulsion has taken (fear, taunt) are left to `enforce_compulsions`.
fn hunt_enemies(
    hunters: Query<
        (Entity, &Transform, &Team, &Hunter, Option<&StatusEffects>),
        (With<Creep>, Without<AttackTarget>),
    >,
    candidates: Query<(Entity, &Transform, &Team, &Health)>,
    statuses_of: Query<&StatusEffects>,
    mut commands: Commands,
) {
    for (entity, transform, team, hunter, own_statuses) in &hunters {
        if orders_blocked(own_statuses) {
            continue;
        }
        let origin = transform.translation;
        let nearest = candidates
            .iter()
            .filter(|(e, _, other_team, health)| {
                **other_team == team.enemy()
                    && health.is_alive()
                    && is_targetable(&statuses_of, *e, TargetIntent::Attack)
            })
            .map(|(_, tf, _, _)| (tf.translation, flat_distance(origin, tf.translation)))
            .filter(|(_, distance)| *distance <= hunter.aggro_radius)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((position, _)) = nearest {
            commands.entity(entity).insert(MoveTarget {
                position: Vec3::new(position.x, 0.0, position.z),
            });
        }
    }
}

fn acquire_targets(
    mut seekers: Query<
        (
            Entity,
            &Transform,
            &Team,
            &CombatStats,
            Option<&BoundRadius>,
            Option<&AttackTarget>,
            Has<Tower>,
            Option<&StatusEffects>,
        ),
        (Without<PlayerHero>, Without<TrainingDummy>),
    >,
    candidates: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    statuses_of: Query<&StatusEffects>,
    mut commands: Commands,
) {
    let snaps: Vec<_> = candidates
        .iter()
        .filter(|(e, _, _, hp, _)| {
            hp.is_alive() && is_targetable(&statuses_of, *e, TargetIntent::Attack)
        })
        .map(|(e, t, team, _, radius)| (e, t.translation, *team, bounds_of(radius)))
        .collect();

    for (entity, transform, team, stats, self_bound, current, is_tower, own_statuses) in
        &mut seekers
    {
        if stats.attack_range <= 0.0 || orders_blocked(own_statuses) {
            continue;
        }
        let attacker_bound = bounds_of(self_bound);
        let origin = transform.translation;

        if let Some(AttackTarget(target)) = current {
            let still_valid = snaps.iter().any(|(e, pos, target_team, radius)| {
                *e == *target
                    && *target_team == team.enemy()
                    && within_range(
                        origin,
                        attacker_bound,
                        *pos,
                        *radius,
                        stats.attack_range + 40.0,
                    )
            });
            if still_valid {
                continue;
            }
            commands.entity(entity).remove::<AttackTarget>();
        }

        let aggro_pad = if is_tower { 0.0 } else { 80.0 };
        let best = snaps
            .iter()
            .filter(|(_, _, target_team, _)| *target_team == team.enemy())
            .filter(|(_, pos, _, radius)| {
                within_range(
                    origin,
                    attacker_bound,
                    *pos,
                    *radius,
                    stats.attack_range + aggro_pad,
                )
            })
            .min_by(|a, b| {
                edge_distance(origin, attacker_bound, a.1, a.3).total_cmp(&edge_distance(
                    origin,
                    attacker_bound,
                    b.1,
                    b.3,
                ))
            });

        if let Some((target, _, _, _)) = best {
            commands.entity(entity).insert(AttackTarget(*target));
            if !is_tower {
                commands.entity(entity).remove::<MoveTarget>();
            }
        }
    }
}

/// Attack-move: walk toward the ordered destination; attack any enemy that enters range.
fn player_attack_move(
    hero: Query<
        (
            Entity,
            &Transform,
            &CombatStats,
            &Team,
            Option<&BoundRadius>,
            &AttackMoveOrder,
            Has<MoveTarget>,
        ),
        With<PlayerHero>,
    >,
    enemies: Query<(
        Entity,
        &Transform,
        &Team,
        &Health,
        Option<&BoundRadius>,
        &Visibility,
    )>,
    statuses_of: Query<&StatusEffects>,
    mut commands: Commands,
) {
    let Ok((entity, transform, stats, hero_team, self_bound, order, moving)) = hero.single() else {
        return;
    };
    if orders_blocked(statuses_of.get(entity).ok()) {
        return;
    }
    let attacker_bound = bounds_of(self_bound);

    let in_range = enemies
        .iter()
        .filter(|(e, _, team, hp, _, vis)| {
            **team == hero_team.enemy()
                && hp.is_alive()
                && !matches!(*vis, Visibility::Hidden)
                && is_targetable(&statuses_of, *e, TargetIntent::Attack)
        })
        .filter(|(_, tf, _, _, radius, _)| {
            within_range(
                transform.translation,
                attacker_bound,
                tf.translation,
                bounds_of(*radius),
                stats.attack_range,
            )
        })
        .min_by(|a, b| {
            edge_distance(
                transform.translation,
                attacker_bound,
                a.1.translation,
                bounds_of(a.4),
            )
            .total_cmp(&edge_distance(
                transform.translation,
                attacker_bound,
                b.1.translation,
                bounds_of(b.4),
            ))
        });

    if let Some((enemy, _, _, _, _, _)) = in_range {
        commands.entity(entity).insert(AttackTarget(enemy));
        commands.entity(entity).remove::<MoveTarget>();
        return;
    }

    commands.entity(entity).remove::<AttackTarget>();
    let dest = order.destination;
    let distance = flat_distance(transform.translation, dest);
    // Movement drops the MoveTarget when it arrives or gives up next to a crowded
    // destination; either way the attack-move is done once the hero is close.
    if distance < 10.0 || (!moving && distance < ATTACK_MOVE_SETTLE) {
        commands
            .entity(entity)
            .remove::<AttackMoveOrder>()
            .remove::<MoveTarget>();
    } else {
        commands
            .entity(entity)
            .insert(MoveTarget { position: dest });
    }
}

/// How far a flee `MoveTarget` may lag behind the freshly computed one before it is replaced
/// (replacing it every frame would re-plan the unit's path every frame).
const FLEE_TARGET_SLACK: f32 = 150.0;
/// Same for the position of a unit being approached.
const APPROACH_TARGET_SLACK: f32 = 40.0;

/// Whether `entity` is alive and may be picked as a basic attack target.
fn is_live_target(
    entity: Entity,
    lives: &Query<(&Transform, &Health)>,
    statuses_of: &Query<&StatusEffects>,
) -> bool {
    lives.get(entity).is_ok_and(|(_, hp)| hp.is_alive())
        && is_targetable(statuses_of, entity, TargetIntent::Attack)
}

/// The only order writer for units under a compulsion (fear, taunt, hypnosis). Each frame it
/// throws away what the player or AI ordered (command queue, attack-move, queued cast, swing
/// target) and writes the forced behaviour instead:
///
/// - `Flee` / `Approach`: no attack target, and a `MoveTarget` away from / onto the source (kept
///   while it is still close to where it should be, so the path is not replanned every frame;
///   `movement::apply_move_targets` steers by the live source position anyway). An idle unit
///   gets one too, which is what makes a standing feared unit run.
/// - `Attack`: the taunter becomes the `AttackTarget`; approach and range are handled by
///   `assign_attack_positions` as for any attack order.
///
/// A taunt or hypnosis whose source died or despawned is removed. Runs after the AI systems
/// that could write orders and before `assign_attack_positions`.
pub fn enforce_compulsions(
    mut units: Query<(
        Entity,
        &Transform,
        &StatusEffects,
        Option<&MoveTarget>,
        Option<&mut CommandQueue>,
    )>,
    lives: Query<(&Transform, &Health)>,
    statuses_of: Query<&StatusEffects>,
    mut status_events: MessageWriter<StatusEffectEvent>,
    mut commands: Commands,
) {
    for (entity, transform, statuses, move_target, queue) in &mut units {
        let Some(compulsion) = statuses.compulsion() else {
            continue;
        };
        // The source is gone: end the status instead of leaving the unit without orders.
        if let Compulsion::Attack { target: source } | Compulsion::Approach { toward: source } =
            compulsion
        {
            let source_alive = lives.get(source).is_ok_and(|(_, hp)| hp.is_alive());
            if !source_alive {
                for effect in statuses.effects.iter().filter(|e| {
                    e.remaining > 0.0
                        && matches!(
                            e.compulsion,
                            Some(Compulsion::Attack { target: s } | Compulsion::Approach { toward: s })
                                if s == source
                        )
                }) {
                    status_events.write(StatusEffectEvent::new(
                        None,
                        entity,
                        StatusChange::Remove(effect.id),
                    ));
                }
                continue;
            }
        }

        if let Some(mut queue) = queue {
            queue.current = None;
            queue.queued.clear();
        }
        let mut unit = commands.entity(entity);
        // A cast already underway is cancelled by the pipeline itself (CAST is blocked).
        unit.remove::<(AttackMoveOrder, QueuedActivation)>();
        let pos = flat(transform.translation);

        match compulsion {
            Compulsion::Attack { target } => {
                if is_live_target(target, &lives, &statuses_of) {
                    unit.insert(AttackTarget(target));
                } else {
                    unit.remove::<(AttackTarget, AttackPositionGoal)>();
                }
            }
            Compulsion::Flee { .. } | Compulsion::Approach { .. } => {
                unit.remove::<(AttackTarget, AttackPositionGoal, AttackSwing)>();
                if !statuses.can_move() {
                    continue;
                }
                let (wanted, slack) = match compulsion {
                    Compulsion::Flee { from } => {
                        let source = from
                            .and_then(|e| lives.get(e).ok())
                            .map(|(tf, _)| flat(tf.translation));
                        let dir = flee_direction(pos, source, flat(*transform.forward()));
                        (pos + dir * FLEE_DISTANCE, FLEE_TARGET_SLACK)
                    }
                    Compulsion::Approach { toward } => {
                        let Ok((tf, _)) = lives.get(toward) else {
                            continue;
                        };
                        (flat(tf.translation), APPROACH_TARGET_SLACK)
                    }
                    Compulsion::Attack { .. } => unreachable!("handled above"),
                };
                let stale = move_target.is_none_or(|t| flat(t.position).distance(wanted) > slack);
                if stale {
                    unit.insert(MoveTarget {
                        position: ground(wanted),
                    });
                }
            }
        }
    }
}

/// An idle attack-moving hero this close to its destination has finished the order.
const ATTACK_MOVE_SETTLE: f32 = 250.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> Vec<Vec3> {
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1000.0, 0.0, 0.0),
            Vec3::new(2000.0, 0.0, 0.0),
            Vec3::new(3000.0, 0.0, 0.0),
        ]
    }

    #[test]
    fn lane_follower_resume_from_picks_next_waypoint() {
        // Before the nearest waypoint: aim at it.
        assert_eq!(
            LaneFollower::resume_from(lane(), Vec3::new(900.0, 0.0, 100.0)).index,
            1
        );
        // Past the nearest waypoint (toward the next one): aim at the next one.
        assert_eq!(
            LaneFollower::resume_from(lane(), Vec3::new(1100.0, 0.0, 100.0)).index,
            2
        );
        // At the start of the lane, and beyond the end, there is nothing to skip to.
        assert_eq!(
            LaneFollower::resume_from(lane(), Vec3::new(-100.0, 0.0, 0.0)).index,
            0
        );
        assert_eq!(
            LaneFollower::resume_from(lane(), Vec3::new(3500.0, 0.0, 0.0)).index,
            3
        );
        assert!(
            LaneFollower::resume_from(Vec::new(), Vec3::ZERO)
                .waypoints
                .is_empty()
        );
    }
}
