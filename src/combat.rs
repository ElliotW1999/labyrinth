//! Auto-attack via animated projectiles, damage types, death, bounty, and XP.

use bevy::prelude::*;

use crate::components::{
    AttackCooldown, AttackSwing, AttackTarget, CombatStats, DamageType, GoldBounty, Ground, Health,
    HeroProgress, Lifetime, PlayerHero, PlayerWallet, Projectile, ProjectileHome, ProjectileStyle,
    Team, UnitRadius, XpBounty,
};
use crate::facing::turn_toward;
use crate::items::StatusEffects;
use crate::progression::add_xp;
use crate::resources::SharedAssets;

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                tick_attack_cooldowns,
                begin_attack_windups,
                tick_attack_swings,
                fly_projectiles,
                apply_projectile_hits,
                tick_lifetimes,
                despawn_dead,
            )
                .chain()
                .run_if(crate::net::is_sim_authority),
        );
    }
}

fn tick_attack_cooldowns(time: Res<Time>, mut query: Query<&mut AttackCooldown>) {
    let dt = time.delta_secs();
    for mut cd in &mut query {
        cd.0 = (cd.0 - dt).max(0.0);
    }
}

/// Start foreswing when off cooldown, able to attack, and facing the target
/// within the 11.5° cone.
fn begin_attack_windups(
    time: Res<Time>,
    mut commands: Commands,
    mut attackers: Query<(
        Entity,
        &mut Transform,
        &Team,
        &CombatStats,
        &AttackCooldown,
        Option<&AttackTarget>,
        Option<&AttackSwing>,
        Option<&StatusEffects>,
        Has<PlayerHero>,
    )>,
    // GlobalTransform (not Transform) so this stays disjoint from attackers' &mut Transform.
    targets: Query<(Entity, &GlobalTransform, &Team, &Health, Option<&UnitRadius>)>,
) {
    let dt = time.delta_secs();
    let target_snapshots: Vec<_> = targets
        .iter()
        .filter(|(_, _, _, hp, _)| hp.is_alive())
        .map(|(e, t, team, _, radius)| {
            (
                e,
                t.translation(),
                *team,
                radius.map(|r| r.0).unwrap_or(0.5),
            )
        })
        .collect();

    for (
        entity,
        mut transform,
        team,
        stats,
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
        let chosen = current_target
            .and_then(|AttackTarget(id)| {
                target_snapshots
                    .iter()
                    .find(|(e, pos, target_team, radius)| {
                        *e == *id
                            && *target_team == team.enemy()
                            && flat_distance(origin, *pos) <= stats.attack_range + *radius
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
                        flat_distance(origin, *pos) <= stats.attack_range + *radius
                    })
                    .min_by(|a, b| {
                        flat_distance(origin, a.1)
                            .partial_cmp(&flat_distance(origin, b.1))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .copied()
            });

        let Some((target_entity, target_pos, _, _)) = chosen else {
            continue;
        };

        let dir = target_pos - origin;
        let facing = turn_toward(&mut transform, dir, stats.turn_rate, dt);
        if !facing {
            continue;
        }

        let point = stats.attack_point.clamp(0.0, 0.5);
        commands.entity(entity).insert(AttackSwing::Windup {
            remaining: point,
            target: target_entity,
            damage: stats.attack_damage,
        });
    }
}

fn tick_attack_swings(
    time: Res<Time>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut attackers: Query<(
        Entity,
        &Transform,
        &Team,
        &CombatStats,
        &mut AttackCooldown,
        &mut AttackSwing,
        Option<&StatusEffects>,
    )>,
    target_tfs: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();
    for (entity, transform, team, stats, mut cooldown, mut swing, statuses) in &mut attackers {
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
                let target_pos = target_tfs
                    .get(target)
                    .map(|tf| tf.translation())
                    .unwrap_or(transform.translation + *transform.forward() * 4.0);
                spawn_auto_attack(
                    &mut commands,
                    &assets,
                    *team,
                    transform.translation,
                    target,
                    target_pos,
                    damage,
                    projectile_speed_for(stats.attack_range),
                );
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

fn fly_projectiles(
    time: Res<Time>,
    mut projectiles: Query<(
        Entity,
        &mut Transform,
        &Projectile,
        Option<&mut ProjectileHome>,
        Option<&GroundBoltAim>,
    )>,
    homes: Query<&GlobalTransform, Without<Projectile>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, projectile, mut home, ground_aim) in &mut projectiles {
        let mut destination = transform.translation + *transform.forward() * projectile.speed;

        if let Some(ref mut home) = home {
            if let Ok(target_tf) = homes.get(home.target) {
                home.last_pos = target_tf.translation() + Vec3::Y * 1.0;
                destination = home.last_pos;
            } else {
                // Target died or despawned — finish at last known location.
                let last = home.last_pos;
                commands.entity(entity).remove::<ProjectileHome>();
                commands.entity(entity).insert(GroundBoltAim { position: last });
                destination = Vec3::new(last.x, transform.translation.y, last.z);
            }
        } else if let Some(aim) = ground_aim {
            destination = Vec3::new(aim.position.x, transform.translation.y, aim.position.z);
        }

        let to = destination - transform.translation;
        let dist = to.length();
        if dist <= f32::EPSILON {
            if ground_aim.is_some() {
                commands.entity(entity).insert(GroundBoltImpact);
            }
            continue;
        }
        let step = projectile.speed * dt;
        let dir = to / dist;
        if step >= dist {
            transform.translation = destination;
            // Arrived: hit home target check next system, or ground impact if aim bolt.
            if ground_aim.is_some() || home.is_none() {
                commands.entity(entity).insert(GroundBoltImpact);
            } else {
                // Homing projectile reached target position — force a hit check via impact tag.
                commands.entity(entity).insert(ProjectileReachedTarget);
            }
        } else {
            transform.translation += dir * step;
        }
        transform.look_to(dir, Vec3::Y);
    }
}

#[derive(Component, Debug, Clone, Copy)]
struct GroundBoltImpact;

/// Homing projectile arrived at the target's position this frame.
#[derive(Component, Debug, Clone, Copy)]
struct ProjectileReachedTarget;

fn apply_projectile_hits(
    mut commands: Commands,
    projectiles: Query<(
        Entity,
        &Transform,
        &Projectile,
        Option<&ProjectileHome>,
        Option<&GroundBoltImpact>,
        Option<&ProjectileReachedTarget>,
    )>,
    mut units: Query<(Entity, &Transform, &Team, &mut Health, &CombatStats, Option<&UnitRadius>)>,
) {
    for (proj_entity, proj_tf, projectile, home, ground_impact, reached) in &projectiles {
        let impact = proj_tf.translation;
        let mut despawn = false;
        let mut primary_hit: Option<Entity> = None;

        if let Some(home) = home {
            for (unit_entity, unit_tf, team, mut health, stats, radius) in &mut units {
                if unit_entity != home.target || *team == projectile.team || !health.is_alive() {
                    continue;
                }
                let reach = projectile.radius + radius.map(|r| r.0).unwrap_or(0.5);
                let vertical = (impact.y - (unit_tf.translation.y + 1.0)).abs();
                if flat_distance(impact, unit_tf.translation) <= reach && vertical < 2.5 {
                    let dmg = apply_damage(projectile.damage, projectile.damage_type, stats);
                    health.current -= dmg;
                    primary_hit = Some(unit_entity);
                    despawn = true;
                }
            }
            // Reached last known position but target already gone — end the projectile.
            if reached.is_some() && !despawn {
                despawn = true;
            }
        } else if ground_impact.is_some() {
            despawn = true;
        }

        if despawn && projectile.splash_radius > 0.0 {
            for (unit_entity, unit_tf, team, mut health, stats, _) in &mut units {
                if *team == projectile.team || !health.is_alive() {
                    continue;
                }
                if primary_hit == Some(unit_entity) {
                    continue;
                }
                if flat_distance(impact, unit_tf.translation) <= projectile.splash_radius {
                    let ratio = if primary_hit.is_some() { 0.45 } else { 1.0 };
                    let dmg =
                        apply_damage(projectile.damage * ratio, projectile.damage_type, stats);
                    health.current -= dmg;
                }
            }
        }

        if despawn {
            commands.entity(proj_entity).despawn();
        }
    }
}

fn tick_lifetimes(
    time: Res<Time>,
    mut query: Query<(Entity, &mut Lifetime, Option<&mut Projectile>)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut lifetime, mut projectile) in &mut query {
        lifetime.0 -= dt;
        if let Some(ref mut proj) = projectile {
            proj.lifetime -= dt;
            if proj.lifetime <= 0.0 {
                commands.entity(entity).despawn();
                continue;
            }
        }
        if lifetime.0 <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

fn despawn_dead(
    mut commands: Commands,
    dead: Query<
        (
            Entity,
            &Transform,
            &Health,
            Option<&GoldBounty>,
            Option<&XpBounty>,
            Option<&Team>,
        ),
        Without<PlayerHero>,
    >,
    mut heroes: Query<
        (&Transform, &Team, &mut PlayerWallet, &mut HeroProgress),
        With<PlayerHero>,
    >,
) {
    const XP_SHARE_RADIUS: f32 = 18.0;

    for (entity, transform, health, gold_bounty, xp_bounty, team) in &dead {
        if health.is_alive() {
            continue;
        }

        if let Some(victim_team) = team {
            let death_pos = transform.translation;
            for (hero_tf, hero_team, mut wallet, mut progress) in &mut heroes {
                if *hero_team != victim_team.enemy() {
                    continue;
                }
                if let Some(gold) = gold_bounty {
                    wallet.gold += gold.0;
                }
                let dx = hero_tf.translation.x - death_pos.x;
                let dz = hero_tf.translation.z - death_pos.z;
                if (dx * dx + dz * dz).sqrt() <= XP_SHARE_RADIUS {
                    if let Some(xp) = xp_bounty {
                        add_xp(&mut progress, xp.0);
                    }
                }
            }
        }

        commands.entity(entity).despawn();
    }
}

pub fn apply_damage(raw: f32, damage_type: DamageType, stats: &CombatStats) -> f32 {
    match damage_type {
        DamageType::Physical => mitigate(raw, stats.armor),
        DamageType::Magical => mitigate(raw, stats.magic_resist),
    }
}

pub fn mitigate(raw_damage: f32, resistance: f32) -> f32 {
    let factor = 1.0 - (0.06 * resistance) / (1.0 + 0.06 * resistance.abs());
    (raw_damage * factor).max(0.0)
}

pub fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

fn projectile_speed_for(attack_range: f32) -> f32 {
    (18.0 + attack_range * 1.5).clamp(20.0, 45.0)
}

pub fn cursor_ground_hit(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform)>,
    ground: &Query<&GlobalTransform, With<Ground>>,
) -> Option<Vec3> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, cam_transform) = camera.single().ok()?;
    let ray = camera.viewport_to_world(cam_transform, cursor).ok()?;
    let ground_tf = ground.single().ok()?;
    let plane = InfinitePlane3d::new(Dir3::Y);
    let distance = ray.intersect_plane(ground_tf.translation(), plane)?;
    Some(ray.get_point(distance))
}

fn spawn_auto_attack(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    origin: Vec3,
    target: Entity,
    target_pos: Vec3,
    damage: f32,
    speed: f32,
) {
    let start = origin + Vec3::Y * 1.1;
    let aim = (target_pos + Vec3::Y * 1.0) - start;
    let mut transform = Transform::from_translation(start);
    if let Ok(dir) = Dir3::new(aim) {
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
        Projectile {
            damage,
            speed,
            team,
            radius: 0.7,
            lifetime: 2.5,
            damage_type: DamageType::Physical,
            splash_radius: 0.0,
        },
        ProjectileHome {
            target,
            last_pos: target_pos + Vec3::Y * 1.0,
        },
        ProjectileStyle::AutoAttack,
        Lifetime(2.5),
    ));
}

pub fn spawn_spell_bolt(
    commands: &mut Commands,
    assets: &SharedAssets,
    team: Team,
    origin: Vec3,
    target: Option<Entity>,
    target_pos: Vec3,
    damage: f32,
    splash_radius: f32,
) {
    let start = origin + Vec3::Y * 1.2;
    let aim = (target_pos + Vec3::Y * 1.0) - start;
    let mut transform = Transform::from_translation(start);
    if let Ok(dir) = Dir3::new(aim) {
        transform.look_to(dir, Vec3::Y);
    }

    let mut entity = commands.spawn((
        Name::new("Spell Bolt"),
        Mesh3d(assets.spell_bolt_mesh.clone()),
        MeshMaterial3d(assets.spell_bolt_mat.clone()),
        transform,
        Projectile {
            damage,
            speed: 34.0,
            team,
            radius: 0.85,
            lifetime: 2.5,
            damage_type: DamageType::Magical,
            splash_radius,
        },
        ProjectileStyle::SpellBolt,
        Lifetime(2.5),
    ));

    if let Some(target) = target {
        entity.insert(ProjectileHome {
            target,
            last_pos: target_pos + Vec3::Y * 1.0,
        });
    } else {
        let dist = flat_distance(origin, target_pos);
        let travel = (dist / 34.0) + 0.05;
        entity.insert(Lifetime(travel));
        entity.insert(GroundBoltAim {
            position: target_pos,
        });
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct GroundBoltAim {
    pub position: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_reduces_physical() {
        let stats = CombatStats::simple(0.0, 0.0, 1.0, 10.0, 0.0, 0.0);
        let raw = 100.0;
        let mitigated = apply_damage(raw, DamageType::Physical, &stats);
        assert!(mitigated < raw);
        assert!(mitigated > 50.0);
    }

    #[test]
    fn magic_resist_reduces_magical() {
        let stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 10.0, 0.0);
        let raw = 100.0;
        let phys = apply_damage(raw, DamageType::Physical, &stats);
        let mag = apply_damage(raw, DamageType::Magical, &stats);
        assert!((phys - raw).abs() < 0.01);
        assert!(mag < raw);
    }

    #[test]
    fn flat_distance_ignores_height() {
        let a = Vec3::new(0.0, 10.0, 0.0);
        let b = Vec3::new(3.0, -4.0, 4.0);
        assert!((flat_distance(a, b) - 5.0).abs() < 1e-4);
    }

    /// Regression: turn-rate windups need &mut Transform on attackers without
    /// conflicting with target position reads (Bevy B0001).
    #[test]
    fn begin_attack_windups_system_initializes() {
        let mut world = World::new();
        world.init_resource::<Time>();
        let mut system = IntoSystem::into_system(begin_attack_windups);
        system.initialize(&mut world);
    }
}
