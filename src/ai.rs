//! Creep lane following and tower / creep aggro.
//!
//! The player hero is intentionally excluded — player orders must never be
//! overwritten by AI aggro/chase logic.

use bevy::prelude::*;

use crate::combat::flat_distance;
use crate::components::{
    AttackTarget, CombatStats, Creep, Health, MoveTarget, PlayerHero, Team, Tower, UnitRadius,
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
                player_chase_attack_target,
            )
                .chain(),
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
        if flat_distance(transform.translation, waypoint) < 1.2 {
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
            Option<&AttackTarget>,
            Has<Tower>,
        ),
        Without<PlayerHero>,
    >,
    candidates: Query<(Entity, &Transform, &Team, &Health, Option<&UnitRadius>)>,
    mut commands: Commands,
) {
    let snaps: Vec<_> = candidates
        .iter()
        .filter(|(_, _, _, hp, _)| hp.is_alive())
        .map(|(e, t, team, _, radius)| (e, t.translation, *team, radius.map(|r| r.0).unwrap_or(0.5)))
        .collect();

    for (entity, transform, team, stats, current, is_tower) in &mut seekers {
        if stats.attack_range <= 0.0 {
            continue;
        }

        if let Some(AttackTarget(target)) = current {
            let still_valid = snaps.iter().any(|(e, pos, target_team, radius)| {
                *e == *target
                    && *target_team == team.enemy()
                    && flat_distance(transform.translation, *pos)
                        <= stats.attack_range + *radius + 1.5
            });
            if still_valid {
                continue;
            }
            commands.entity(entity).remove::<AttackTarget>();
        }

        let aggro_range = if is_tower {
            stats.attack_range
        } else {
            stats.attack_range + 3.0
        };

        let best = snaps
            .iter()
            .filter(|(_, _, target_team, _)| *target_team == team.enemy())
            .filter(|(_, pos, _, radius)| {
                flat_distance(transform.translation, *pos) <= aggro_range + *radius
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

/// Player-only chase: keep pathing toward an attack target until in range.
pub fn player_chase_attack_target(
    hero: Query<(Entity, &Transform, &CombatStats, &AttackTarget), With<PlayerHero>>,
    targets: Query<(&Transform, Option<&UnitRadius>)>,
    mut commands: Commands,
) {
    let Ok((entity, transform, stats, AttackTarget(target))) = hero.single() else {
        return;
    };
    let Ok((target_tf, radius)) = targets.get(*target) else {
        commands.entity(entity).remove::<AttackTarget>();
        return;
    };
    let reach = stats.attack_range + radius.map(|r| r.0).unwrap_or(0.5);
    let dist = flat_distance(transform.translation, target_tf.translation);
    if dist > reach * 0.9 {
        commands.entity(entity).insert(MoveTarget {
            position: Vec3::new(target_tf.translation.x, 0.0, target_tf.translation.z),
        });
    } else {
        commands.entity(entity).remove::<MoveTarget>();
    }
}
