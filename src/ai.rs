//! Creep lane following and tower / creep aggro.
//!
//! The player hero is intentionally excluded — player orders must never be
//! overwritten by AI aggro/chase logic.

use bevy::prelude::*;

use crate::combat::flat_distance;
use crate::scale;
use crate::components::{
    AttackMoveOrder, AttackTarget, CombatStats, Creep, Health, MoveTarget, PlayerHero, Team, Tower,
    BoundRadius,
};

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                follow_lane_waypoints,
                acquire_targets,
                chase_attack_targets,
                player_attack_move,
                player_chase_attack_target,
            )
                .chain()
                .run_if(crate::net::is_sim_authority),
        );
    }
}

#[derive(Component, Debug, Clone)]
pub struct LaneFollower {
    pub waypoints: Vec<Vec3>,
    pub index: usize,
}

fn follow_lane_waypoints(
    mut creeps: Query<
        (Entity, &Transform, &mut LaneFollower, Option<&AttackTarget>),
        With<Creep>,
    >,
    mut commands: Commands,
) {
    for (entity, transform, mut follower, attack) in &mut creeps {
        if attack.is_some() {
            continue;
        }
        if follower.index >= follower.waypoints.len() {
            continue;
        }
        let waypoint = follower.waypoints[follower.index];
        if flat_distance(transform.translation, waypoint) < scale::u(1.2) {
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
        ),
        Without<PlayerHero>,
    >,
    candidates: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    mut commands: Commands,
) {
    let snaps: Vec<_> = candidates
        .iter()
        .filter(|(_, _, _, hp, _)| hp.is_alive())
        .map(|(e, t, team, _, radius)| {
            (
                e,
                t.translation,
                *team,
                radius.map(|r| r.0).unwrap_or(scale::HERO_BOUND),
            )
        })
        .collect();

    for (entity, transform, team, stats, self_bound, current, is_tower) in &mut seekers {
        if stats.attack_range <= 0.0 {
            continue;
        }
        let attacker_bound = self_bound.map(|r| r.0).unwrap_or(scale::HERO_BOUND);

        if let Some(AttackTarget(target)) = current {
            let still_valid = snaps.iter().any(|(e, pos, target_team, radius)| {
                *e == *target
                    && *target_team == team.enemy()
                    && flat_distance(transform.translation, *pos)
                        <= scale::attack_reach(attacker_bound, stats.attack_range, *radius) + 40.0
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
                flat_distance(transform.translation, *pos)
                    <= scale::attack_reach(attacker_bound, stats.attack_range, *radius) + aggro_pad
            })
            .min_by(|a, b| {
                flat_distance(transform.translation, a.1)
                    .partial_cmp(&flat_distance(transform.translation, b.1))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some((target, _, _, _)) = best {
            commands.entity(entity).insert(AttackTarget(*target));
            if !is_tower {
                commands.entity(entity).remove::<MoveTarget>();
            }
        }
    }
}

fn chase_attack_targets(
    attackers: Query<
        (Entity, &Transform, &CombatStats, &AttackTarget),
        (Without<Tower>, Without<PlayerHero>),
    >,
    targets: Query<&Transform>,
    mut commands: Commands,
) {
    for (entity, transform, stats, AttackTarget(target)) in &attackers {
        let Ok(target_tf) = targets.get(*target) else {
            commands.entity(entity).remove::<AttackTarget>();
            continue;
        };
        let dist = flat_distance(transform.translation, target_tf.translation);
        if dist > stats.attack_range * 0.85 {
            commands.entity(entity).insert(MoveTarget {
                position: Vec3::new(target_tf.translation.x, 0.0, target_tf.translation.z),
            });
        } else {
            commands.entity(entity).remove::<MoveTarget>();
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
        ),
        With<PlayerHero>,
    >,
    enemies: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>, &Visibility)>,
    mut commands: Commands,
) {
    let Ok((entity, transform, stats, hero_team, self_bound, order)) = hero.single() else {
        return;
    };
    let attacker_bound = self_bound.map(|r| r.0).unwrap_or(scale::HERO_BOUND);

    let in_range = enemies
        .iter()
        .filter(|(_, _, team, hp, _, vis)| {
            **team == hero_team.enemy() && hp.is_alive() && !matches!(*vis, Visibility::Hidden)
        })
        .filter(|(_, tf, _, _, radius, _)| {
            let r = radius.map(|r| r.0).unwrap_or(scale::HERO_BOUND);
            flat_distance(transform.translation, tf.translation)
                <= scale::attack_reach(attacker_bound, stats.attack_range, r)
        })
        .min_by(|a, b| {
            flat_distance(transform.translation, a.1.translation)
                .partial_cmp(&flat_distance(transform.translation, b.1.translation))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    if let Some((enemy, _, _, _, _, _)) = in_range {
        commands.entity(entity).insert(AttackTarget(enemy));
        commands.entity(entity).remove::<MoveTarget>();
        return;
    }

    commands.entity(entity).remove::<AttackTarget>();
    let dest = order.destination;
    if flat_distance(transform.translation, dest) < 10.0 {
        commands
            .entity(entity)
            .remove::<AttackMoveOrder>()
            .remove::<MoveTarget>();
    } else {
        commands.entity(entity).insert(MoveTarget { position: dest });
    }
}

/// Player-only chase: keep pathing toward an attack target until in range.
/// Skipped while an attack-move order is active (handled above).
pub fn player_chase_attack_target(
    hero: Query<
        (Entity, &Transform, &CombatStats, Option<&BoundRadius>, &AttackTarget),
        (With<PlayerHero>, Without<AttackMoveOrder>),
    >,
    targets: Query<(&Transform, Option<&BoundRadius>)>,
    mut commands: Commands,
) {
    let Ok((entity, transform, stats, self_bound, AttackTarget(target))) = hero.single() else {
        return;
    };
    let Ok((target_tf, radius)) = targets.get(*target) else {
        commands.entity(entity).remove::<AttackTarget>();
        return;
    };
    let reach = scale::attack_reach(
        self_bound.map(|r| r.0).unwrap_or(scale::HERO_BOUND),
        stats.attack_range,
        radius.map(|r| r.0).unwrap_or(scale::HERO_BOUND),
    );
    let dist = flat_distance(transform.translation, target_tf.translation);
    if dist > reach * 0.9 {
        commands.entity(entity).insert(MoveTarget {
            position: Vec3::new(target_tf.translation.x, 0.0, target_tf.translation.z),
        });
    } else {
        commands.entity(entity).remove::<MoveTarget>();
    }
}
