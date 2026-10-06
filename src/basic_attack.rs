//! Basic attack pipeline, driven by gameplay messages rather than projectile collision:
//!
//! ```text
//! request_basic_attacks       validate cooldown / statuses / target / range / facing
//!   → BasicAttackEvent
//! start_basic_attack_windups  AttackSwing::Windup (attack point)
//! tick_attack_swings          cooldown + backswing
//!   → BasicAttackReleaseEvent
//! resolve_attack_releases     melee: impact now (+ slash fx)
//!                             ranged: BasicAttackInFlight (+ projectile visual)
//! advance_attacks_in_flight   homes on the target; arrival is decided here, not by the mesh
//!   → BasicAttackImpactEvent
//! resolve_basic_attack_impacts  re-validate target
//!   → DamageEvent (applied by combat::apply_damage_events)
//! ```
//!
//! Ranged attacks ignore trees and other units: only the intended target can be hit.

use bevy::prelude::*;

use crate::combat::DamageEvent;
use crate::components::{
    AttackCooldown, AttackSwing, AttackTarget, BoundRadius, CombatStats, DamageType, Health,
    Lifetime, PlayerHero, ProjectileStyle, Team,
};
use crate::dimensions::{area_contains, bounds_of, edge_distance, within_range};
use crate::facing::turn_toward;
use crate::items::StatusEffects;
use crate::resources::SharedAssets;
use crate::scale;

/// Melee hits forgive a target stepping slightly away during the windup.
const MELEE_HIT_RANGE_LENIENCY: f32 = 1.15;
const RANGED_ATTACK_LIFETIME: f32 = 2.5;

/// A validated basic attack: `attacker` may begin its windup against `target`.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BasicAttackEvent {
    pub attacker: Entity,
    pub target: Entity,
}

/// The attacker reached its attack point; the attack leaves the attacker.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BasicAttackReleaseEvent {
    pub attacker: Entity,
    pub target: Entity,
    pub damage: f32,
}

/// The attack connects with its target (at release for melee, on arrival for ranged).
/// `attacker_team` is captured at release because the attacker may die mid-flight.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BasicAttackImpactEvent {
    pub attacker: Entity,
    pub attacker_team: Team,
    pub target: Entity,
    pub damage: f32,
}

/// Authoritative state of a ranged basic attack travelling to its target.
#[derive(Component, Debug, Clone, Copy)]
pub struct BasicAttackInFlight {
    pub attacker: Entity,
    pub attacker_team: Team,
    pub target: Entity,
    pub damage: f32,
    pub speed: f32,
    /// Contact radius added to the target's bounds radius.
    pub radius: f32,
    pub position: Vec3,
    pub direction: Vec3,
    pub last_target_pos: Vec3,
    /// Target despawned: finish at its last known position without hitting.
    pub target_lost: bool,
    pub remaining: f32,
}

/// Presentation-only mesh mirroring a [`BasicAttackInFlight`].
#[derive(Component, Debug, Clone, Copy)]
pub struct BasicAttackProjectileVisual {
    pub attack: Entity,
}

pub fn projectile_speed_for(attack_range: f32) -> f32 {
    // ~900–1500 u/s — scales with attack range in the new world units.
    (500.0 + attack_range * 1.2).clamp(700.0, 1500.0)
}

fn aim_point(target_origin: Vec3) -> Vec3 {
    target_origin + Vec3::Y * scale::body(1.0)
}

/// Off cooldown, able to attack, target valid and in range, and facing it within
/// the 11.5° cone ⇒ emit a [`BasicAttackEvent`]. Players only attack their ordered
/// target; AI falls back to the nearest enemy in range.
pub fn request_basic_attacks(
    time: Res<Time>,
    mut requests: MessageWriter<BasicAttackEvent>,
    mut attackers: Query<(
        Entity,
        &mut Transform,
        &Team,
        &CombatStats,
        Option<&BoundRadius>,
        &AttackCooldown,
        Option<&AttackTarget>,
        Option<&AttackSwing>,
        Option<&StatusEffects>,
        Has<PlayerHero>,
    )>,
    // GlobalTransform (not Transform) so this stays disjoint from attackers' &mut Transform.
    targets: Query<(Entity, &GlobalTransform, &Team, &Health, Option<&BoundRadius>)>,
) {
    let dt = time.delta_secs();
    let target_snapshots: Vec<_> = targets
        .iter()
        .filter(|(_, _, _, hp, _)| hp.is_alive())
        .map(|(e, t, team, _, radius)| (e, t.translation(), *team, bounds_of(radius)))
        .collect();

    for (
        entity,
        mut transform,
        team,
        stats,
        attacker_bound,
        cooldown,
        current_target,
        swing,
        statuses,
        is_player,
    ) in &mut attackers
    {
        if swing.is_some() {
            continue;
        }
        if cooldown.0 > 0.0 || stats.attack_damage <= 0.0 || stats.attack_range <= 0.0 {
            continue;
        }
        if statuses.is_some_and(|s| !s.can_attack()) {
            continue;
        }
        if is_player && current_target.is_none() {
            continue;
        }

        let origin = transform.translation;
        let self_bound = bounds_of(attacker_bound);
        let chosen = current_target
            .and_then(|AttackTarget(id)| {
                target_snapshots
                    .iter()
                    .find(|(e, pos, target_team, radius)| {
                        *e == *id
                            && *target_team == team.enemy()
                            && within_range(origin, self_bound, *pos, *radius, stats.attack_range)
                    })
                    .copied()
            })
            .or_else(|| {
                if is_player {
                    return None;
                }
                target_snapshots
                    .iter()
                    .filter(|(_, _, target_team, _)| *target_team == team.enemy())
                    .filter(|(_, pos, _, radius)| {
                        within_range(origin, self_bound, *pos, *radius, stats.attack_range)
                    })
                    .min_by(|a, b| {
                        edge_distance(origin, self_bound, a.1, a.3)
                            .total_cmp(&edge_distance(origin, self_bound, b.1, b.3))
                    })
                    .copied()
            });

        let Some((target_entity, target_pos, _, _)) = chosen else {
            continue;
        };

        if !turn_toward(&mut transform, target_pos - origin, stats.turn_rate, dt) {
            continue;
        }

        requests.write(BasicAttackEvent {
            attacker: entity,
            target: target_entity,
        });
    }
}

/// Begin the foreswing for each accepted [`BasicAttackEvent`]. Damage is snapshotted here.
pub fn start_basic_attack_windups(
    mut commands: Commands,
    mut requests: MessageReader<BasicAttackEvent>,
    attackers: Query<(&CombatStats, Has<AttackSwing>)>,
) {
    for request in requests.read() {
        let Ok((stats, swinging)) = attackers.get(request.attacker) else {
            continue;
        };
        if swinging {
            continue;
        }
        commands.entity(request.attacker).insert(AttackSwing::Windup {
            remaining: stats.attack_point.clamp(0.0, 0.5),
            target: request.target,
            damage: stats.attack_damage,
        });
    }
}

/// Advance foreswing / backswing timers. At the attack point the cooldown starts
/// and a [`BasicAttackReleaseEvent`] is emitted; disabled attackers lose their swing.
pub fn tick_attack_swings(
    time: Res<Time>,
    mut commands: Commands,
    mut releases: MessageWriter<BasicAttackReleaseEvent>,
    mut attackers: Query<(
        Entity,
        &CombatStats,
        &mut AttackCooldown,
        &mut AttackSwing,
        Option<&StatusEffects>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, stats, mut cooldown, mut swing, statuses) in &mut attackers {
        if statuses.is_some_and(|s| !s.can_attack()) {
            commands.entity(entity).remove::<AttackSwing>();
            continue;
        }
        match *swing {
            AttackSwing::Windup {
                remaining,
                target,
                damage,
            } => {
                let next = remaining - dt;
                if next > 0.0 {
                    *swing = AttackSwing::Windup {
                        remaining: next,
                        target,
                        damage,
                    };
                    continue;
                }
                releases.write(BasicAttackReleaseEvent {
                    attacker: entity,
                    target,
                    damage,
                });
                cooldown.0 = 1.0 / stats.attack_speed.max(0.05);
                let back = stats.attack_backswing.clamp(0.0, 0.5);
                if back > 0.0 {
                    *swing = AttackSwing::Backswing { remaining: back };
                } else {
                    commands.entity(entity).remove::<AttackSwing>();
                }
            }
            AttackSwing::Backswing { remaining } => {
                let next = remaining - dt;
                if next > 0.0 {
                    *swing = AttackSwing::Backswing { remaining: next };
                } else {
                    commands.entity(entity).remove::<AttackSwing>();
                }
            }
        }
    }
}

/// Melee attacks connect immediately if the target is still within lenient reach;
/// ranged attacks start an authoritative [`BasicAttackInFlight`] plus a projectile visual.
pub fn resolve_attack_releases(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut releases: MessageReader<BasicAttackReleaseEvent>,
    mut impacts: MessageWriter<BasicAttackImpactEvent>,
    attackers: Query<(&Transform, &Team, &CombatStats, Option<&BoundRadius>)>,
    targets: Query<(&GlobalTransform, &Team, &Health, Option<&BoundRadius>)>,
) {
    for release in releases.read() {
        let Ok((transform, team, stats, attacker_bound)) = attackers.get(release.attacker) else {
            continue;
        };
        let origin = transform.translation;
        let target = targets
            .get(release.target)
            .ok()
            .filter(|(_, target_team, health, _)| **target_team == team.enemy() && health.is_alive());

        if scale::is_melee_attack_range(stats.attack_range) {
            spawn_melee_slash(&mut commands, &assets, *team, transform, stats.attack_range);
            let Some((target_tf, _, _, target_bound)) = target else {
                continue;
            };
            if within_range(
                origin,
                bounds_of(attacker_bound),
                target_tf.translation(),
                bounds_of(target_bound),
                stats.attack_range * MELEE_HIT_RANGE_LENIENCY,
            ) {
                impacts.write(BasicAttackImpactEvent {
                    attacker: release.attacker,
                    attacker_team: *team,
                    target: release.target,
                    damage: release.damage,
                });
            }
        } else if let Some((target_tf, _, _, _)) = target {
            let start = origin + Vec3::Y * scale::body(1.1);
            let aim = aim_point(target_tf.translation());
            let attack = commands
                .spawn((
                    Name::new("Basic Attack In Flight"),
                    BasicAttackInFlight {
                        attacker: release.attacker,
                        attacker_team: *team,
                        target: release.target,
                        damage: release.damage,
                        speed: projectile_speed_for(stats.attack_range),
                        radius: scale::body(0.7),
                        position: start,
                        direction: (aim - start).normalize_or_zero(),
                        last_target_pos: aim,
                        target_lost: false,
                        remaining: RANGED_ATTACK_LIFETIME,
                    },
                ))
                .id();
            spawn_projectile_visual(&mut commands, &assets, *team, attack, start, aim);
        }
    }
}

/// Home each ranged attack on its target. Contact with the target's bounds is the
/// only way to hit; a despawned target turns the attack into a miss at its last position.
pub fn advance_attacks_in_flight(
    time: Res<Time>,
    mut commands: Commands,
    mut impacts: MessageWriter<BasicAttackImpactEvent>,
    mut attacks: Query<(Entity, &mut BasicAttackInFlight)>,
    targets: Query<(&GlobalTransform, &Health, Option<&BoundRadius>)>,
) {
    let dt = time.delta_secs();
    for (entity, mut attack) in &mut attacks {
        attack.remaining -= dt;
        if attack.remaining <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }

        if !attack.target_lost {
            match targets.get(attack.target) {
                Ok((tf, _, _)) => attack.last_target_pos = aim_point(tf.translation()),
                Err(_) => attack.target_lost = true,
            }
        }
        let destination = if attack.target_lost {
            Vec3::new(attack.last_target_pos.x, attack.position.y, attack.last_target_pos.z)
        } else {
            attack.last_target_pos
        };

        let to = destination - attack.position;
        let dist = to.length();
        let arrived = dist <= attack.speed * dt;
        if dist > f32::EPSILON {
            attack.direction = to / dist;
        }
        if arrived {
            attack.position = destination;
        } else {
            let step = attack.direction * attack.speed * dt;
            attack.position += step;
        }

        if attack.target_lost {
            if arrived {
                commands.entity(entity).despawn();
            }
            continue;
        }

        let contact = targets
            .get(attack.target)
            .is_ok_and(|(tf, health, bound)| {
                let target_origin = tf.translation();
                let vertical = (attack.position.y - aim_point(target_origin).y).abs();
                health.is_alive()
                    && vertical < scale::body(2.5)
                    && area_contains(attack.position, attack.radius, target_origin, bounds_of(bound))
            });
        if contact {
            impacts.write(BasicAttackImpactEvent {
                attacker: attack.attacker,
                attacker_team: attack.attacker_team,
                target: attack.target,
                damage: attack.damage,
            });
        }
        if contact || arrived {
            commands.entity(entity).despawn();
        }
    }
}

/// Final validation at impact time: the target must still exist, be alive, and be an enemy.
pub fn resolve_basic_attack_impacts(
    mut impacts: MessageReader<BasicAttackImpactEvent>,
    mut damage: MessageWriter<DamageEvent>,
    targets: Query<(&Team, &Health)>,
) {
    for impact in impacts.read() {
        let Ok((team, health)) = targets.get(impact.target) else {
            continue;
        };
        if *team != impact.attacker_team.enemy() || !health.is_alive() {
            continue;
        }
        damage.write(DamageEvent {
            source: Some(impact.attacker),
            target: impact.target,
            amount: impact.damage,
            damage_type: DamageType::Physical,
        });
    }
}

fn spawn_projectile_visual(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    attack: Entity,
    start: Vec3,
    aim: Vec3,
) {
    let mut transform = Transform::from_translation(start);
    if let Ok(dir) = Dir3::new(aim - start) {
        transform.look_to(dir, Vec3::Y);
    }
    let material = match team {
        Team::Radiant => assets.projectile_radiant_mat.clone(),
        Team::Dire => assets.projectile_dire_mat.clone(),
    };
    commands.spawn((
        Name::new("Auto Attack"),
        Mesh3d(assets.projectile_mesh.clone()),
        MeshMaterial3d(material),
        transform,
        BasicAttackProjectileVisual { attack },
        ProjectileStyle::AutoAttack,
        Lifetime(RANGED_ATTACK_LIFETIME),
    ));
}

/// Mirror gameplay state onto projectile meshes; drop meshes whose attack resolved.
pub fn sync_attack_projectile_visuals(
    mut commands: Commands,
    mut visuals: Query<(Entity, &mut Transform, &BasicAttackProjectileVisual)>,
    attacks: Query<&BasicAttackInFlight>,
) {
    for (entity, mut transform, visual) in &mut visuals {
        let Ok(attack) = attacks.get(visual.attack) else {
            commands.entity(entity).despawn();
            continue;
        };
        transform.translation = attack.position;
        if let Ok(dir) = Dir3::new(attack.direction) {
            transform.look_to(dir, Vec3::Y);
        }
    }
}

/// Crude sword slash: a rectangular block the length of attack range, pivoted at
/// the unit midsection, swinging 90° over a short lifetime.
fn spawn_melee_slash(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    attacker: &Transform,
    attack_range: f32,
) {
    let mid_y = scale::body(0.9);
    let thickness = scale::body(0.22);
    let height = scale::body(0.35);
    let length = attack_range.max(scale::body(0.5));

    let facing_yaw = attacker.rotation.to_euler(EulerRot::YXZ).0;
    let start_yaw = facing_yaw - std::f32::consts::FRAC_PI_4;

    let material = match team {
        Team::Radiant => assets.melee_slash_radiant_mat.clone(),
        Team::Dire => assets.melee_slash_dire_mat.clone(),
    };

    // Pivot at midsection; cuboid extends along local -Z (forward).
    let mut transform = Transform::from_translation(attacker.translation + Vec3::Y * mid_y)
        .with_rotation(Quat::from_rotation_y(start_yaw))
        .with_scale(Vec3::new(thickness, height, length));
    // Shift so the near end sits at the pivot (mesh is centered on Z).
    transform.translation += transform.forward() * (length * 0.5);

    commands.spawn((
        Name::new("Melee Slash"),
        Mesh3d(assets.melee_slash_mesh.clone()),
        MeshMaterial3d(material),
        transform,
        MeleeSlashFx {
            elapsed: 0.0,
            duration: 0.18,
            yaw_start: start_yaw,
            pivot: attacker.translation + Vec3::Y * mid_y,
            length,
        },
        Lifetime(0.2),
    ));
}

pub fn animate_melee_slashes(
    time: Res<Time>,
    mut slashes: Query<(&mut Transform, &mut MeleeSlashFx)>,
) {
    let dt = time.delta_secs();
    for (mut transform, mut fx) in &mut slashes {
        fx.elapsed = (fx.elapsed + dt).min(fx.duration);
        let t = if fx.duration <= 1e-4 {
            1.0
        } else {
            (fx.elapsed / fx.duration).clamp(0.0, 1.0)
        };
        // Ease-out swing through 90°.
        let eased = 1.0 - (1.0 - t) * (1.0 - t);
        let yaw = fx.yaw_start + eased * std::f32::consts::FRAC_PI_2;
        transform.rotation = Quat::from_rotation_y(yaw);
        transform.translation = fx.pivot + *transform.forward() * (fx.length * 0.5);
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct MeleeSlashFx {
    elapsed: f32,
    duration: f32,
    yaw_start: f32,
    pivot: Vec3,
    length: f32,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::combat::{apply_damage, apply_damage_events};

    const DT: f32 = 0.05;

    fn world_with_pipeline() -> (World, Schedule) {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<SharedAssets>();
        world.init_resource::<Messages<BasicAttackEvent>>();
        world.init_resource::<Messages<BasicAttackReleaseEvent>>();
        world.init_resource::<Messages<BasicAttackImpactEvent>>();
        world.init_resource::<Messages<DamageEvent>>();
        let mut schedule = Schedule::default();
        schedule.add_systems(
            (
                request_basic_attacks,
                start_basic_attack_windups,
                tick_attack_swings,
                resolve_attack_releases,
                advance_attacks_in_flight,
                resolve_basic_attack_impacts,
                sync_attack_projectile_visuals,
                apply_damage_events,
            )
                .chain(),
        );
        (world, schedule)
    }

    fn step(world: &mut World, schedule: &mut Schedule) {
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(DT));
        schedule.run(world);
    }

    fn spawn_unit(world: &mut World, team: Team, pos: Vec3, stats: CombatStats) -> Entity {
        let transform = Transform::from_translation(pos);
        world
            .spawn((
                transform,
                GlobalTransform::from(transform),
                team,
                stats,
                Health::new(1000.0),
                AttackCooldown(0.0),
            ))
            .id()
    }

    fn spawn_attacker(world: &mut World, range: f32, target: Entity, target_pos: Vec3) -> Entity {
        let stats = CombatStats::simple(50.0, range, 1.0, 0.0, 0.0, 300.0);
        let attacker = spawn_unit(world, Team::Radiant, Vec3::ZERO, stats);
        world
            .entity_mut(attacker)
            .insert((Transform::default().looking_at(target_pos, Vec3::Y), AttackTarget(target)));
        attacker
    }

    fn hp(world: &World, entity: Entity) -> f32 {
        world.get::<Health>(entity).unwrap().current
    }

    fn expected_hit() -> f32 {
        let defender = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        1000.0 - apply_damage(50.0, DamageType::Physical, &defender)
    }

    fn passive_stats() -> CombatStats {
        CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0)
    }

    #[test]
    fn melee_attack_damages_at_attack_point_not_on_request() {
        let (mut world, mut schedule) = world_with_pipeline();
        let target_pos = Vec3::new(100.0, 0.0, 0.0);
        let target = spawn_unit(&mut world, Team::Dire, target_pos, passive_stats());
        spawn_attacker(&mut world, scale::MELEE_ATTACK_RANGE, target, target_pos);

        step(&mut world, &mut schedule);
        assert_eq!(hp(&world, target), 1000.0, "windup must delay the hit");
        for _ in 0..8 {
            step(&mut world, &mut schedule);
        }
        assert!((hp(&world, target) - expected_hit()).abs() < 1e-3);
    }

    #[test]
    fn ranged_attack_hits_only_its_target_after_travel() {
        let (mut world, mut schedule) = world_with_pipeline();
        let target_pos = Vec3::new(450.0, 0.0, 0.0);
        let target = spawn_unit(&mut world, Team::Dire, target_pos, passive_stats());
        let blocker = spawn_unit(&mut world, Team::Dire, Vec3::new(200.0, 0.0, 0.0), passive_stats());
        world.entity_mut(blocker).insert(BoundRadius(60.0));
        spawn_attacker(&mut world, 500.0, target, target_pos);

        let mut released_at = None;
        for frame in 0..40 {
            step(&mut world, &mut schedule);
            let in_flight = world.query::<&BasicAttackInFlight>().iter(&world).count();
            if in_flight > 0 && released_at.is_none() {
                released_at = Some(frame);
                assert_eq!(hp(&world, target), 1000.0, "ranged damage waits for arrival");
            }
            if hp(&world, target) < 1000.0 {
                break;
            }
        }
        assert!(released_at.is_some());
        assert!((hp(&world, target) - expected_hit()).abs() < 1e-3);
        assert_eq!(hp(&world, blocker), 1000.0, "units in the path never absorb the attack");
    }

    #[test]
    fn ranged_attack_misses_when_target_despawns_mid_flight() {
        let (mut world, mut schedule) = world_with_pipeline();
        let target_pos = Vec3::new(450.0, 0.0, 0.0);
        let target = spawn_unit(&mut world, Team::Dire, target_pos, passive_stats());
        spawn_attacker(&mut world, 500.0, target, target_pos);

        while world.query::<&BasicAttackInFlight>().iter(&world).count() == 0 {
            step(&mut world, &mut schedule);
        }
        world.despawn(target);
        for _ in 0..20 {
            step(&mut world, &mut schedule);
        }
        assert_eq!(world.query::<&BasicAttackInFlight>().iter(&world).count(), 0);
        assert_eq!(
            world.query::<&BasicAttackProjectileVisual>().iter(&world).count(),
            0
        );
        assert!(world.resource::<Messages<DamageEvent>>().is_empty());
    }
}
