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

use crate::combat::{CombatRng, DamageEvent, DamageTestHooks, flat_distance};
use crate::components::{
    AttackCooldown, AttackSwing, AttackTarget, BoundRadius, CombatStats, Health, Lifetime,
    PlayerHero, ProjectileStyle, Team,
};
use crate::dimensions::{area_contains, bounds_of, edge_distance, within_range};
use crate::facing::turn_toward;
use crate::resources::SharedAssets;
use crate::scale;
use crate::status::{StatusEffects, is_untargetable};

/// Melee hits forgive a target stepping slightly away during the windup.
const MELEE_HIT_RANGE_LENIENCY: f32 = 1.15;
const RANGED_ATTACK_LIFETIME: f32 = 2.5;

/// A validated basic attack: `attacker` may begin its windup against `target`.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BasicAttackEvent {
    pub attacker: Entity,
    pub target: Entity,
}

/// Request for an extra basic attack (instant / double / multi-target attack). It ignores
/// cooldown, swing and range, and never changes them; see [`release_extra_attacks`].
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ExtraAttackEvent {
    pub attacker: Entity,
    pub target: Entity,
    /// Fraction of the attacker's attack damage the extra attack deals (1.0 = a full hit).
    pub damage_ratio: f32,
}

/// The attacker reached its attack point; the attack leaves the attacker.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BasicAttackReleaseEvent {
    pub attacker: Entity,
    pub target: Entity,
    pub damage: f32,
    /// An extra attack (instant / double / multi-target); never touches cooldown or swing.
    pub extra: bool,
}

/// The attack connects with its target (at release for melee, on arrival for ranged).
/// `attacker_team` is captured at release because the attacker may die mid-flight.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BasicAttackImpactEvent {
    pub attacker: Entity,
    pub attacker_team: Team,
    pub target: Entity,
    pub damage: f32,
    pub extra: bool,
}

/// A basic or extra attack passed its hit roll and dealt its main hit. Item and ability
/// `OnAttackHit` triggers read this, so missed attacks never fire on-hit effects.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BasicAttackLandedEvent {
    pub attacker: Entity,
    pub attacker_team: Team,
    pub target: Entity,
    /// Post-crit, pre-mitigation main-hit damage.
    pub damage: f32,
    pub crit: bool,
    pub extra: bool,
}

/// Authoritative state of a ranged basic attack travelling to its target.
#[derive(Component, Debug, Clone, Copy)]
pub struct BasicAttackInFlight {
    pub attacker: Entity,
    pub attacker_team: Team,
    pub target: Entity,
    pub damage: f32,
    pub extra: bool,
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

/// Travel speed of an attacker's projectile: derived from its base range, plus its bonus.
fn attack_projectile_speed(stats: &CombatStats) -> f32 {
    (projectile_speed_for(stats.base_attack_range()) + stats.attack_projectile_speed_bonus).max(1.0)
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
    targets: Query<(
        Entity,
        &GlobalTransform,
        &Team,
        &Health,
        Option<&BoundRadius>,
    )>,
    statuses_of: Query<&StatusEffects>,
) {
    let dt = time.delta_secs();
    let target_snapshots: Vec<_> = targets
        .iter()
        .filter(|(e, _, _, hp, _)| hp.is_alive() && !is_untargetable(&statuses_of, *e))
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

        let turn = stats.effective_turn_rate(statuses);
        if !turn_toward(&mut transform, target_pos - origin, turn, dt) {
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
        commands
            .entity(request.attacker)
            .insert(AttackSwing::Windup {
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
                    extra: false,
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

/// Turn each [`ExtraAttackEvent`] into a release flagged `extra`. The attacker must be alive
/// and able to attack, the target a living, targetable enemy; anything else is dropped whole.
/// Cooldown, swing, `AttackTarget` and movement are never touched.
pub fn release_extra_attacks(
    mut requests: MessageReader<ExtraAttackEvent>,
    mut releases: MessageWriter<BasicAttackReleaseEvent>,
    attackers: Query<(&CombatStats, &Health, &Team, Option<&StatusEffects>)>,
    targets: Query<(&Team, &Health)>,
    statuses_of: Query<&StatusEffects>,
) {
    for request in requests.read() {
        let Ok((stats, health, team, statuses)) = attackers.get(request.attacker) else {
            continue;
        };
        if !health.is_alive()
            || stats.attack_damage <= 0.0
            || statuses.is_some_and(|s| !s.can_attack())
        {
            continue;
        }
        let target_ok = targets.get(request.target).is_ok_and(|(target_team, hp)| {
            *target_team == team.enemy()
                && hp.is_alive()
                && !is_untargetable(&statuses_of, request.target)
        });
        if !target_ok {
            continue;
        }
        releases.write(BasicAttackReleaseEvent {
            attacker: request.attacker,
            target: request.target,
            damage: stats.attack_damage * request.damage_ratio.max(0.0),
            extra: true,
        });
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
    statuses_of: Query<&StatusEffects>,
) {
    for release in releases.read() {
        let Ok((transform, team, stats, attacker_bound)) = attackers.get(release.attacker) else {
            continue;
        };
        let origin = transform.translation;
        let target = targets
            .get(release.target)
            .ok()
            .filter(|(_, target_team, health, _)| {
                **target_team == team.enemy()
                    && health.is_alive()
                    && !is_untargetable(&statuses_of, release.target)
            });

        if stats.is_melee() {
            spawn_melee_slash(&mut commands, &assets, *team, transform, stats.attack_range);
            let Some((target_tf, _, _, target_bound)) = target else {
                continue;
            };
            // Extra attacks reach their target wherever it stands.
            if release.extra
                || within_range(
                    origin,
                    bounds_of(attacker_bound),
                    target_tf.translation(),
                    bounds_of(target_bound),
                    stats.attack_range * MELEE_HIT_RANGE_LENIENCY,
                )
            {
                impacts.write(BasicAttackImpactEvent {
                    attacker: release.attacker,
                    attacker_team: *team,
                    target: release.target,
                    damage: release.damage,
                    extra: release.extra,
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
                        extra: release.extra,
                        speed: attack_projectile_speed(stats),
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
    statuses_of: Query<&StatusEffects>,
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
            Vec3::new(
                attack.last_target_pos.x,
                attack.position.y,
                attack.last_target_pos.z,
            )
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

        let contact = targets.get(attack.target).is_ok_and(|(tf, health, bound)| {
            let target_origin = tf.translation();
            let vertical = (attack.position.y - aim_point(target_origin).y).abs();
            health.is_alive()
                && !is_untargetable(&statuses_of, attack.target)
                && vertical < scale::body(2.5)
                && area_contains(
                    attack.position,
                    attack.radius,
                    target_origin,
                    bounds_of(bound),
                )
        });
        if contact {
            impacts.write(BasicAttackImpactEvent {
                attacker: attack.attacker,
                attacker_team: attack.attacker_team,
                target: attack.target,
                damage: attack.damage,
                extra: attack.extra,
            });
        }
        if contact || arrived {
            commands.entity(entity).despawn();
        }
    }
}

/// Chance that an attack misses: the attacker's blind and the target's evasion combine as
/// independent dodges, then the attacker's accuracy rerolls the miss (true strike = 1.0).
pub fn attack_miss_chance(blind: f32, evasion: f32, accuracy: f32) -> f32 {
    let dodge = 1.0 - (1.0 - blind.clamp(0.0, 1.0)) * (1.0 - evasion.clamp(0.0, 1.0));
    dodge * (1.0 - accuracy.clamp(0.0, 1.0))
}

/// Independent chances combine as `1 - (1 - base) x prod(1 - chance)`, each clamped to 0..=1.
pub fn stack_chances(base: f32, others: impl Iterator<Item = f32>) -> f32 {
    let missing = others.fold(1.0 - base.clamp(0.0, 1.0), |acc, c| {
        acc * (1.0 - c.clamp(0.0, 1.0))
    });
    1.0 - missing
}

/// Miss chance of `attacker` against `target`: blind (attacker), evasion (target) and
/// accuracy (attacker) from stats and statuses.
pub fn attack_miss_chance_between(
    attacker: Option<(&CombatStats, Option<&StatusEffects>)>,
    target: (Option<&CombatStats>, Option<&StatusEffects>),
) -> f32 {
    let blind = attacker
        .and_then(|(_, statuses)| statuses)
        .map_or(0.0, |s| s.blind_miss_chance());
    let accuracy = attacker.map_or(0.0, |(stats, statuses)| {
        stack_chances(
            stats.accuracy,
            statuses.into_iter().flat_map(|s| s.accuracies()),
        )
    });
    let evasion = stack_chances(
        target.0.map_or(0.0, |stats| stats.evasion),
        target.1.into_iter().flat_map(|s| s.evasions()),
    );
    attack_miss_chance(blind, evasion, accuracy)
}

/// The attack-hit stage. Re-validates the target, rolls to hit (one [`CombatRng`] draw), rolls
/// the crit (one more draw, only on a hit), emits the main [`DamageEvent`] and a
/// [`BasicAttackLandedEvent`], then cleaves. Misses stop here: no damage, no landed event.
pub fn resolve_basic_attack_impacts(
    mut impacts: MessageReader<BasicAttackImpactEvent>,
    mut damage: MessageWriter<DamageEvent>,
    mut landed: MessageWriter<BasicAttackLandedEvent>,
    mut rng: ResMut<CombatRng>,
    hooks: Option<Res<DamageTestHooks>>,
    targets: Query<(&Team, &Health, Option<&StatusEffects>, Option<&CombatStats>)>,
    attackers: Query<(&CombatStats, &Transform, Option<&StatusEffects>)>,
    cleave_candidates: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    statuses_of: Query<&StatusEffects>,
) {
    let force_crit = hooks.is_some_and(|h| h.force_crit);
    for impact in impacts.read() {
        let Ok((team, health, defender_status, defender_stats)) = targets.get(impact.target) else {
            continue;
        };
        if *team != impact.attacker_team.enemy()
            || !health.is_alive()
            || defender_status.is_some_and(|s| s.is_untargetable())
        {
            continue;
        }
        // The attacker may have despawned mid-flight: no blind, no crit, no cleave.
        let attacker = attackers.get(impact.attacker).ok();
        let miss = attack_miss_chance_between(
            attacker.map(|(stats, _, statuses)| (stats, statuses)),
            (defender_stats, defender_status),
        );
        if rng.roll(miss) {
            continue;
        }
        let crit_chance = attacker.map_or(0.0, |(stats, _, _)| stats.crit_chance);
        let crit = force_crit || rng.roll(crit_chance);
        let multiplier = match attacker {
            Some((stats, _, _)) if crit => stats.crit_multiplier.max(1.0),
            _ => 1.0,
        };
        let amount = impact.damage * multiplier;
        damage.write(
            DamageEvent::basic_attack(impact.attacker, impact.target, amount).with_crit(crit),
        );
        landed.write(BasicAttackLandedEvent {
            attacker: impact.attacker,
            attacker_team: impact.attacker_team,
            target: impact.target,
            damage: amount,
            crit,
            extra: impact.extra,
        });

        let Some((stats, attacker_tf, _)) = attacker else {
            continue;
        };
        // Secondary hits: never crit, never roll to hit, never leech, never fire triggers.
        if stats.is_melee() && stats.cleave_damage_ratio > 0.0 {
            let radius = effective_radius(stats.cleave_radius, scale::ATTACK_CLEAVE_RADIUS);
            let cleave_amount = amount * stats.cleave_damage_ratio;
            for entity in secondary_attack_targets(
                attacker_tf.translation,
                radius,
                impact.target,
                impact.attacker_team,
                &cleave_candidates,
                &statuses_of,
            ) {
                damage.write(DamageEvent::cleave(impact.attacker, entity, cleave_amount));
            }
        }
        if stats.splash_damage_ratio > 0.0 {
            if let Ok((_, target_tf, ..)) = cleave_candidates.get(impact.target) {
                let radius = effective_radius(stats.splash_radius, scale::ATTACK_SPLASH_RADIUS);
                let splash_amount = amount * stats.splash_damage_ratio;
                for entity in secondary_attack_targets(
                    target_tf.translation,
                    radius,
                    impact.target,
                    impact.attacker_team,
                    &cleave_candidates,
                    &statuses_of,
                ) {
                    damage.write(DamageEvent::splash(impact.attacker, entity, splash_amount));
                }
            }
        }
    }
}

/// A stat radius when set, otherwise the default.
fn effective_radius(stat: f32, default: f32) -> f32 {
    if stat > 0.0 { stat } else { default }
}

/// Living, targetable enemies of `team` whose bounds overlap the circle (`center`, `radius`),
/// excluding `primary`, sorted by [`Entity`] so message order is deterministic.
fn secondary_attack_targets(
    center: Vec3,
    radius: f32,
    primary: Entity,
    team: Team,
    candidates: &Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    statuses_of: &Query<&StatusEffects>,
) -> Vec<Entity> {
    let mut found: Vec<Entity> = candidates
        .iter()
        .filter(|(entity, tf, unit_team, hp, bound)| {
            *entity != primary
                && **unit_team == team.enemy()
                && hp.is_alive()
                && !is_untargetable(statuses_of, *entity)
                && flat_distance(center, tf.translation) <= radius
                && area_contains(center, radius, tf.translation, bounds_of(*bound))
        })
        .map(|(entity, ..)| entity)
        .collect();
    found.sort();
    found
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
    use crate::components::DamageType;

    const DT: f32 = 0.05;

    fn world_with_pipeline() -> (World, Schedule) {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<SharedAssets>();
        world.init_resource::<Messages<BasicAttackEvent>>();
        world.init_resource::<Messages<ExtraAttackEvent>>();
        world.init_resource::<Messages<BasicAttackReleaseEvent>>();
        world.init_resource::<Messages<BasicAttackImpactEvent>>();
        world.init_resource::<Messages<BasicAttackLandedEvent>>();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<Messages<crate::combat::DamageAppliedEvent>>();
        world.init_resource::<CombatRng>();
        world.init_resource::<DamageTestHooks>();
        let mut schedule = Schedule::default();
        schedule.add_systems(
            (
                request_basic_attacks,
                start_basic_attack_windups,
                tick_attack_swings,
                release_extra_attacks,
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
        world.entity_mut(attacker).insert((
            Transform::default().looking_at(target_pos, Vec3::Y),
            AttackTarget(target),
        ));
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
        let blocker = spawn_unit(
            &mut world,
            Team::Dire,
            Vec3::new(200.0, 0.0, 0.0),
            passive_stats(),
        );
        world.entity_mut(blocker).insert(BoundRadius(60.0));
        spawn_attacker(&mut world, 500.0, target, target_pos);

        let mut released_at = None;
        for frame in 0..40 {
            step(&mut world, &mut schedule);
            let in_flight = world.query::<&BasicAttackInFlight>().iter(&world).count();
            if in_flight > 0 && released_at.is_none() {
                released_at = Some(frame);
                assert_eq!(
                    hp(&world, target),
                    1000.0,
                    "ranged damage waits for arrival"
                );
            }
            if hp(&world, target) < 1000.0 {
                break;
            }
        }
        assert!(released_at.is_some());
        assert!((hp(&world, target) - expected_hit()).abs() < 1e-3);
        assert_eq!(
            hp(&world, blocker),
            1000.0,
            "units in the path never absorb the attack"
        );
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
        assert_eq!(
            world.query::<&BasicAttackInFlight>().iter(&world).count(),
            0
        );
        assert_eq!(
            world
                .query::<&BasicAttackProjectileVisual>()
                .iter(&world)
                .count(),
            0
        );
        assert!(world.resource::<Messages<DamageEvent>>().is_empty());
    }

    #[test]
    fn cycloned_unit_cannot_be_targeted_or_attacked() {
        let (mut world, mut schedule) = world_with_pipeline();
        let target_pos = Vec3::new(100.0, 0.0, 0.0);
        let target = spawn_unit(&mut world, Team::Dire, target_pos, passive_stats());
        let mut statuses = StatusEffects::default();
        statuses.effects.push(crate::status::StatusEffect {
            // Isolate untargetability from the damage block.
            invulnerable: false,
            ..crate::status::StatusEffect::cyclone(5.0, true)
        });
        world.entity_mut(target).insert(statuses);

        // An ordered attack and an AI attacker that would auto-acquire the nearest enemy.
        let ordered = spawn_attacker(&mut world, scale::MELEE_ATTACK_RANGE, target, target_pos);
        let seeker = spawn_unit(
            &mut world,
            Team::Radiant,
            Vec3::new(0.0, 0.0, 40.0),
            CombatStats::simple(50.0, scale::MELEE_ATTACK_RANGE, 1.0, 0.0, 0.0, 300.0),
        );
        for _ in 0..20 {
            step(&mut world, &mut schedule);
        }
        assert_eq!(
            hp(&world, target),
            1000.0,
            "nothing can hit a cycloned unit"
        );
        assert!(world.get::<AttackSwing>(ordered).is_none());
        assert!(world.get::<AttackSwing>(seeker).is_none());

        // Once the cyclone ends the same attackers connect again.
        world
            .get_mut::<StatusEffects>(target)
            .unwrap()
            .effects
            .clear();
        for _ in 0..20 {
            step(&mut world, &mut schedule);
        }
        assert!(hp(&world, target) < 1000.0);
    }

    fn run_frames(world: &mut World, schedule: &mut Schedule, frames: usize) {
        for _ in 0..frames {
            step(world, schedule);
        }
    }

    fn blind_status(chance: f32) -> StatusEffects {
        let mut statuses = StatusEffects::default();
        statuses
            .effects
            .push(crate::status::StatusEffect::blind(chance, 30.0));
        statuses
    }

    /// Melee attacker at the origin ordered onto a Dire target 100 units away.
    fn melee_duel(world: &mut World) -> (Entity, Entity) {
        let target_pos = Vec3::new(100.0, 0.0, 0.0);
        let target = spawn_unit(world, Team::Dire, target_pos, passive_stats());
        let attacker = spawn_attacker(world, scale::MELEE_ATTACK_RANGE, target, target_pos);
        (attacker, target)
    }

    fn landed_count(world: &World) -> usize {
        world.resource::<Messages<BasicAttackLandedEvent>>().len()
    }

    #[test]
    fn attack_miss_chance_formula() {
        assert!((attack_miss_chance(0.5, 0.5, 0.5) - 0.375).abs() < 1e-6);
        assert!((attack_miss_chance(0.25, 0.5, 0.0) - 0.625).abs() < 1e-6);
        assert_eq!(attack_miss_chance(0.0, 0.0, 0.0), 0.0);
        assert_eq!(
            attack_miss_chance(1.0, 1.0, 1.0),
            0.0,
            "accuracy 1 never misses"
        );
    }

    #[test]
    fn blinded_attacker_misses() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, target) = melee_duel(&mut world);
        world.entity_mut(attacker).insert(blind_status(1.0));
        run_frames(&mut world, &mut schedule, 12);
        assert_eq!(hp(&world, target), 1000.0);
        assert_eq!(landed_count(&world), 0, "a miss is not a landed attack");
    }

    #[test]
    fn blind_on_target_does_not_protect_it() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (_, target) = melee_duel(&mut world);
        world.entity_mut(target).insert(blind_status(1.0));
        run_frames(&mut world, &mut schedule, 12);
        assert!((hp(&world, target) - expected_hit()).abs() < 1e-3);
    }

    #[test]
    fn seeded_hit_rolls_are_reproducible() {
        // Messages are never swapped in these worlds, so the counters are cumulative.
        fn outcome(seed: u64) -> (usize, usize) {
            let (mut world, mut schedule) = world_with_pipeline();
            world.resource_mut::<CombatRng>().seed(seed);
            let (attacker, _) = melee_duel(&mut world);
            world.entity_mut(attacker).insert(blind_status(0.5));
            for _ in 0..200 {
                // The cooldown tick is not part of this schedule: re-arm every frame.
                world.get_mut::<AttackCooldown>(attacker).unwrap().0 = 0.0;
                step(&mut world, &mut schedule);
            }
            (
                world.resource::<Messages<BasicAttackImpactEvent>>().len(),
                landed_count(&world),
            )
        }
        let (impacts, landed) = outcome(7);
        assert_eq!((impacts, landed), outcome(7));
        assert!(
            landed > 0 && landed < impacts,
            "half blind: {landed}/{impacts}"
        );
    }

    #[test]
    fn forced_crit_multiplies_main_hit_once() {
        let (mut world, mut schedule) = world_with_pipeline();
        world.insert_resource(DamageTestHooks { force_crit: true });
        let (_, target) = melee_duel(&mut world);
        run_frames(&mut world, &mut schedule, 9);
        let defender = passive_stats();
        let expected = 1000.0 - apply_damage(100.0, DamageType::Physical, &defender);
        assert!((hp(&world, target) - expected).abs() < 1e-3);
    }

    #[test]
    fn crit_chance_one_crits_and_reports_it() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, target) = melee_duel(&mut world);
        {
            let mut stats = world.get_mut::<CombatStats>(attacker).unwrap();
            stats.crit_chance = 1.0;
            stats.crit_multiplier = 3.0;
        }
        run_frames(&mut world, &mut schedule, 12);
        let seen = world
            .resource::<Messages<BasicAttackLandedEvent>>()
            .iter_current_update_messages()
            .next()
            .copied();
        let event = seen.expect("attack landed");
        assert!(event.crit);
        assert_eq!(event.damage, 150.0);
        assert!(hp(&world, target) < 1000.0);
    }

    #[test]
    fn cleave_uses_crit_damage_without_rolling() {
        let (mut world, mut schedule) = world_with_pipeline();
        world.insert_resource(DamageTestHooks { force_crit: true });
        let (attacker, _target) = melee_duel(&mut world);
        world
            .get_mut::<CombatStats>(attacker)
            .unwrap()
            .cleave_radius = 400.0;
        world
            .get_mut::<CombatStats>(attacker)
            .unwrap()
            .cleave_damage_ratio = 0.5;
        let bystander = spawn_unit(
            &mut world,
            Team::Dire,
            Vec3::new(0.0, 0.0, 150.0),
            passive_stats(),
        );
        // Crit chance is zero: only the forced crit can inflate the main hit, and cleave
        // must reuse that result (50 x 2 x 0.5) instead of rolling for itself.
        run_frames(&mut world, &mut schedule, 9);
        let expected = 1000.0 - apply_damage(50.0, DamageType::Physical, &passive_stats());
        assert!((hp(&world, bystander) - expected).abs() < 1e-3);
    }

    /// Dire bystander at `pos` (default stats, 1000 hp).
    fn bystander(world: &mut World, pos: Vec3) -> Entity {
        spawn_unit(world, Team::Dire, pos, passive_stats())
    }

    fn with_ratios(world: &mut World, attacker: Entity, cleave: f32, splash: f32) {
        let mut stats = world.get_mut::<CombatStats>(attacker).unwrap();
        stats.cleave_damage_ratio = cleave;
        stats.splash_damage_ratio = splash;
    }

    fn secondary_hit() -> f32 {
        // 50 attack damage x 0.5 ratio.
        1000.0 - apply_damage(25.0, DamageType::Physical, &passive_stats())
    }

    #[test]
    fn full_evasion_dodges_and_skips_cleave() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, target) = melee_duel(&mut world);
        world.get_mut::<CombatStats>(target).unwrap().evasion = 1.0;
        with_ratios(&mut world, attacker, 0.5, 0.5);
        let near = bystander(&mut world, Vec3::new(100.0, 0.0, 100.0));
        run_frames(&mut world, &mut schedule, 12);
        assert_eq!(hp(&world, target), 1000.0);
        assert_eq!(
            hp(&world, near),
            1000.0,
            "no secondary hits on a dodged attack"
        );
        assert_eq!(landed_count(&world), 0);
    }

    #[test]
    fn evasion_status_dodges_attacks() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (_, target) = melee_duel(&mut world);
        let mut statuses = StatusEffects::default();
        statuses.effects.push(crate::status::StatusEffect {
            evasion: 1.0,
            ..crate::status::StatusEffect::buff("evasion", 30.0)
        });
        world.entity_mut(target).insert(statuses);
        run_frames(&mut world, &mut schedule, 12);
        assert_eq!(hp(&world, target), 1000.0);
    }

    #[test]
    fn true_strike_ignores_evasion_and_blind() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, target) = melee_duel(&mut world);
        world.get_mut::<CombatStats>(target).unwrap().evasion = 1.0;
        let mut statuses = blind_status(1.0);
        statuses.effects.push(crate::status::StatusEffect {
            accuracy: 1.0,
            ..crate::status::StatusEffect::buff("true_strike", 30.0)
        });
        world.entity_mut(attacker).insert(statuses);
        run_frames(&mut world, &mut schedule, 12);
        assert!((hp(&world, target) - expected_hit()).abs() < 1e-3);
    }

    #[test]
    fn stat_true_strike_beats_partial_evasion_and_blind_end_to_end() {
        fn landed_of_forty(accuracy: f32, blind: bool) -> usize {
            let (mut world, mut schedule) = world_with_pipeline();
            let (attacker, target) = melee_duel(&mut world);
            world.get_mut::<CombatStats>(target).unwrap().evasion = 0.5;
            world.get_mut::<CombatStats>(attacker).unwrap().accuracy = accuracy;
            if blind {
                world.entity_mut(attacker).insert(blind_status(0.5));
            }
            world.entity_mut(attacker).remove::<AttackTarget>();
            for _ in 0..40 {
                request_extra(&mut world, attacker, target, 0.01);
            }
            step(&mut world, &mut schedule);
            landed_count(&world)
        }
        let dodged = landed_of_forty(0.0, false);
        assert!(
            (1..40).contains(&dodged),
            "50% evasion misses some of 40 attacks, landed {dodged}"
        );
        assert_eq!(landed_of_forty(1.0, false), 40, "true strike never misses");
        assert_eq!(landed_of_forty(1.0, true), 40, "and ignores blind too");
    }

    #[test]
    fn stat_accuracy_and_stacking_follow_the_formula() {
        assert!((stack_chances(0.5, [0.5].into_iter()) - 0.75).abs() < 1e-6);
        assert_eq!(stack_chances(0.0, std::iter::empty()), 0.0);
        assert_eq!(stack_chances(2.0, [-1.0].into_iter()), 1.0, "clamped");
    }

    #[test]
    fn cleave_is_melee_only_with_default_radius() {
        // Melee: the default radius reaches 250 away but not 350.
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, target) = melee_duel(&mut world);
        with_ratios(&mut world, attacker, 0.5, 0.0);
        let within = bystander(&mut world, Vec3::new(0.0, 0.0, 250.0));
        let beyond = bystander(&mut world, Vec3::new(0.0, 0.0, 350.0));
        run_frames(&mut world, &mut schedule, 12);
        assert!(hp(&world, target) < 1000.0);
        assert!((hp(&world, within) - secondary_hit()).abs() < 1e-3);
        assert_eq!(hp(&world, beyond), 1000.0);

        // Ranged: the same ratio does nothing.
        let (mut world, mut schedule) = world_with_pipeline();
        let target_pos = Vec3::new(450.0, 0.0, 0.0);
        let target = spawn_unit(&mut world, Team::Dire, target_pos, passive_stats());
        let attacker = spawn_attacker(&mut world, 500.0, target, target_pos);
        with_ratios(&mut world, attacker, 0.5, 0.0);
        let beside = bystander(&mut world, Vec3::new(450.0, 0.0, 100.0));
        let near_attacker = bystander(&mut world, Vec3::new(0.0, 0.0, 100.0));
        run_frames(&mut world, &mut schedule, 40);
        assert!(hp(&world, target) < 1000.0);
        assert_eq!(hp(&world, beside), 1000.0);
        assert_eq!(hp(&world, near_attacker), 1000.0);
    }

    #[test]
    fn cleave_radius_stat_overrides_the_default() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, _) = melee_duel(&mut world);
        with_ratios(&mut world, attacker, 0.5, 0.0);
        world
            .get_mut::<CombatStats>(attacker)
            .unwrap()
            .cleave_radius = 200.0;
        let within = bystander(&mut world, Vec3::new(0.0, 0.0, 150.0));
        let beyond = bystander(&mut world, Vec3::new(0.0, 0.0, 250.0));
        run_frames(&mut world, &mut schedule, 12);
        assert!(hp(&world, within) < 1000.0);
        assert_eq!(hp(&world, beyond), 1000.0);
    }

    #[test]
    fn splash_hits_units_around_the_target() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, target) = melee_duel(&mut world);
        with_ratios(&mut world, attacker, 0.0, 0.5);
        // Target is at x=100: 200 away is inside the 250 splash, 400 away is outside.
        let inside = bystander(&mut world, Vec3::new(100.0, 0.0, 200.0));
        let outside = bystander(&mut world, Vec3::new(100.0, 0.0, 400.0));
        let ally = spawn_unit(
            &mut world,
            Team::Radiant,
            Vec3::new(100.0, 0.0, 100.0),
            passive_stats(),
        );
        run_frames(&mut world, &mut schedule, 12);
        assert!((hp(&world, inside) - secondary_hit()).abs() < 1e-3);
        assert_eq!(hp(&world, outside), 1000.0);
        assert_eq!(hp(&world, ally), 1000.0, "splash never hurts allies");
        assert!(
            (hp(&world, target) - expected_hit()).abs() < 1e-3,
            "no splash on itself"
        );
    }

    #[test]
    fn splash_never_crits_or_leeches() {
        let (mut world, mut schedule) = world_with_pipeline();
        world.insert_resource(DamageTestHooks { force_crit: true });
        let (attacker, _) = melee_duel(&mut world);
        with_ratios(&mut world, attacker, 0.0, 0.5);
        world.get_mut::<CombatStats>(attacker).unwrap().lifesteal = 1.0;
        world.get_mut::<Health>(attacker).unwrap().current = 500.0;
        let inside = bystander(&mut world, Vec3::new(100.0, 0.0, 200.0));
        run_frames(&mut world, &mut schedule, 12);
        // Splash = 0.5 x the post-crit main hit (100); the unit never rolls a crit itself.
        let expected = 1000.0 - apply_damage(50.0, DamageType::Physical, &passive_stats());
        assert!((hp(&world, inside) - expected).abs() < 1e-3);
        let main = 1000.0 - apply_damage(100.0, DamageType::Physical, &passive_stats());
        let gain = hp(&world, attacker) - 500.0;
        assert!(
            (gain - (1000.0 - main)).abs() < 1e-3,
            "leech reads the main hit only"
        );
    }

    #[test]
    fn attack_range_bonus_keeps_melee_classification() {
        let mut stats = CombatStats::simple(50.0, scale::MELEE_ATTACK_RANGE, 1.0, 0.0, 0.0, 300.0);
        stats.attack_range += 200.0;
        stats.attack_range_bonus += 200.0;
        assert!(stats.is_melee());
        assert_eq!(stats.base_attack_range(), scale::MELEE_ATTACK_RANGE);
        let mut ranged = CombatStats::simple(50.0, 500.0, 1.0, 0.0, 0.0, 300.0);
        ranged.attack_range_bonus = -100.0;
        ranged.attack_range -= 100.0;
        assert!(
            !ranged.is_melee(),
            "a range penalty keeps a ranged unit ranged"
        );
    }

    #[test]
    fn projectile_speed_bonus_shortens_travel() {
        fn frames_to_hit(bonus: f32) -> usize {
            let (mut world, mut schedule) = world_with_pipeline();
            let target_pos = Vec3::new(450.0, 0.0, 0.0);
            let target = spawn_unit(&mut world, Team::Dire, target_pos, passive_stats());
            let attacker = spawn_attacker(&mut world, 500.0, target, target_pos);
            world
                .get_mut::<CombatStats>(attacker)
                .unwrap()
                .attack_projectile_speed_bonus = bonus;
            for frame in 0..80 {
                step(&mut world, &mut schedule);
                if hp(&world, target) < 1000.0 {
                    return frame;
                }
            }
            panic!("never hit");
        }
        assert!(frames_to_hit(700.0) < frames_to_hit(0.0));
    }

    fn request_extra(world: &mut World, attacker: Entity, target: Entity, ratio: f32) {
        world.write_message(ExtraAttackEvent {
            attacker,
            target,
            damage_ratio: ratio,
        });
    }

    #[test]
    fn extra_attack_ignores_cooldown_and_swing() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, target) = melee_duel(&mut world);
        // Mid-backswing with a long cooldown: a normal attack could not start.
        world.get_mut::<AttackCooldown>(attacker).unwrap().0 = 5.0;
        world
            .entity_mut(attacker)
            .insert(AttackSwing::Backswing { remaining: 0.4 });
        world.entity_mut(attacker).remove::<AttackTarget>();

        request_extra(&mut world, attacker, target, 0.5);
        step(&mut world, &mut schedule);

        let expected = 1000.0 - apply_damage(25.0, DamageType::Physical, &passive_stats());
        assert!((hp(&world, target) - expected).abs() < 1e-3);
        assert_eq!(world.get::<AttackCooldown>(attacker).unwrap().0, 5.0);
        assert!(matches!(
            world.get::<AttackSwing>(attacker),
            Some(AttackSwing::Backswing { remaining }) if *remaining > 0.3
        ));
        assert!(world.get::<AttackTarget>(attacker).is_none());
        let landed = world
            .resource::<Messages<BasicAttackLandedEvent>>()
            .iter_current_update_messages()
            .next()
            .copied()
            .expect("an extra attack lands like any other");
        assert!(landed.extra);
    }

    #[test]
    fn extra_melee_attack_skips_range_check() {
        let (mut world, mut schedule) = world_with_pipeline();
        let far = Vec3::new(2000.0, 0.0, 0.0);
        let target = spawn_unit(&mut world, Team::Dire, far, passive_stats());
        let attacker = spawn_unit(
            &mut world,
            Team::Radiant,
            Vec3::ZERO,
            CombatStats::simple(50.0, scale::MELEE_ATTACK_RANGE, 1.0, 0.0, 0.0, 300.0),
        );
        request_extra(&mut world, attacker, target, 1.0);
        step(&mut world, &mut schedule);
        assert!((hp(&world, target) - expected_hit()).abs() < 1e-3);
    }

    #[test]
    fn extra_attack_needs_a_living_enabled_attacker_and_a_valid_target() {
        let (mut world, mut schedule) = world_with_pipeline();
        let (attacker, target) = melee_duel(&mut world);
        let ally = spawn_unit(&mut world, Team::Radiant, Vec3::X * 50.0, passive_stats());
        let mut statuses = StatusEffects::default();
        statuses
            .effects
            .push(crate::status::StatusEffect::disarmed(10.0));
        world.entity_mut(attacker).insert(statuses);

        request_extra(&mut world, attacker, target, 1.0);
        step(&mut world, &mut schedule);
        assert_eq!(
            hp(&world, target),
            1000.0,
            "disarmed attackers cannot attack"
        );

        world.entity_mut(attacker).remove::<StatusEffects>();
        request_extra(&mut world, attacker, ally, 1.0);
        request_extra(&mut world, attacker, Entity::PLACEHOLDER, 1.0);
        step(&mut world, &mut schedule);
        assert_eq!(hp(&world, ally), 1000.0, "never against allies");
        assert_eq!(landed_count(&world), 0);
    }

    #[test]
    fn ranged_extra_attack_flies_flagged_extra() {
        let (mut world, mut schedule) = world_with_pipeline();
        let target_pos = Vec3::new(450.0, 0.0, 0.0);
        let target = spawn_unit(&mut world, Team::Dire, target_pos, passive_stats());
        let attacker = spawn_unit(
            &mut world,
            Team::Radiant,
            Vec3::ZERO,
            CombatStats::simple(50.0, 500.0, 1.0, 0.0, 0.0, 300.0),
        );
        // Keep the AI attacker from also swinging normally.
        world.get_mut::<AttackCooldown>(attacker).unwrap().0 = 100.0;
        request_extra(&mut world, attacker, target, 1.0);
        step(&mut world, &mut schedule);
        let flights: Vec<bool> = world
            .query::<&BasicAttackInFlight>()
            .iter(&world)
            .map(|flight| flight.extra)
            .collect();
        assert_eq!(flights, vec![true]);
        assert_eq!(hp(&world, target), 1000.0, "it still has to travel");
        run_frames(&mut world, &mut schedule, 40);
        assert!((hp(&world, target) - expected_hit()).abs() < 1e-3);
        let landed = world
            .resource::<Messages<BasicAttackLandedEvent>>()
            .iter_current_update_messages()
            .next()
            .copied()
            .unwrap();
        assert!(landed.extra);
    }
}
