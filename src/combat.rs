//! Auto-attack via animated projectiles, damage, death, bounty, and XP.

use bevy::prelude::*;

use crate::components::{
    AttackCooldown, AttackTarget, CombatStats, GoldBounty, Health, HeroProgress, Lifetime,
    PlayerHero, PlayerWallet, Projectile, ProjectileHome, Team, UnitRadius, XpBounty,
};
use crate::progression::add_xp;
use crate::resources::SharedAssets;

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                tick_attack_cooldowns,
                auto_attack,
                fly_projectiles,
                apply_projectile_hits,
                tick_lifetimes,
                despawn_dead,
            )
                .chain(),
        );
    }
}

fn tick_attack_cooldowns(time: Res<Time>, mut query: Query<&mut AttackCooldown>) {
    let dt = time.delta_secs();
    for mut cd in &mut query {
        cd.0 = (cd.0 - dt).max(0.0);
    }
}

fn auto_attack(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut attackers: Query<(
        Entity,
        &Transform,
        &Team,
        &CombatStats,
        &mut AttackCooldown,
        Option<&AttackTarget>,
        Has<PlayerHero>,
    )>,
    targets: Query<(Entity, &Transform, &Team, &Health, &CombatStats, Option<&UnitRadius>)>,
) {
    let target_snapshots: Vec<_> = targets
        .iter()
        .filter(|(_, _, _, hp, _, _)| hp.is_alive())
        .map(|(e, t, team, _, stats, radius)| {
            (
                e,
                t.translation,
                *team,
                stats.armor,
                radius.map(|r| r.0).unwrap_or(0.5),
            )
        })
        .collect();

    for (_entity, transform, team, stats, mut cooldown, current_target, is_player) in
        &mut attackers
    {
        if cooldown.0 > 0.0 || stats.attack_damage <= 0.0 || stats.attack_range <= 0.0 {
            continue;
        }

        // Players only attack an explicit right-clicked target.
        if is_player && current_target.is_none() {
            continue;
        }

        let origin = transform.translation;
        let chosen = current_target
            .and_then(|AttackTarget(id)| {
                target_snapshots
                    .iter()
                    .find(|(e, pos, target_team, _, radius)| {
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
                    .filter(|(_, _, target_team, _, _)| *target_team == team.enemy())
                    .filter(|(_, pos, _, _, radius)| {
                        flat_distance(origin, *pos) <= stats.attack_range + *radius
                    })
                    .min_by(|a, b| {
                        flat_distance(origin, a.1)
                            .partial_cmp(&flat_distance(origin, b.1))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .copied()
            });

        let Some((target_entity, target_pos, _, armor, _)) = chosen else {
            continue;
        };

        let damage = mitigate(stats.attack_damage, armor);
        spawn_projectile(
            &mut commands,
            &assets,
            *team,
            origin,
            target_entity,
            target_pos,
            damage,
            projectile_speed_for(stats.attack_range),
        );
        cooldown.0 = 1.0 / stats.attack_speed.max(0.1);
    }
}

fn fly_projectiles(
    time: Res<Time>,
    mut projectiles: Query<(Entity, &mut Transform, &Projectile, Option<&ProjectileHome>)>,
    homes: Query<&GlobalTransform, Without<Projectile>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, projectile, home) in &mut projectiles {
        let mut destination = transform.translation + *transform.forward() * projectile.speed;

        if let Some(ProjectileHome(target)) = home {
            if let Ok(target_tf) = homes.get(*target) {
                destination = target_tf.translation() + Vec3::Y * 1.0;
            } else {
                commands.entity(entity).remove::<ProjectileHome>();
            }
        }

        let to = destination - transform.translation;
        let dist = to.length();
        if dist <= f32::EPSILON {
            continue;
        }
        let step = projectile.speed * dt;
        let dir = to / dist;
        if step >= dist {
            transform.translation = destination;
        } else {
            transform.translation += dir * step;
        }
        transform.look_to(dir, Vec3::Y);
    }
}

fn apply_projectile_hits(
    mut commands: Commands,
    projectiles: Query<(Entity, &Transform, &Projectile, Option<&ProjectileHome>)>,
    mut units: Query<(Entity, &Transform, &Team, &mut Health, Option<&UnitRadius>)>,
) {
    for (proj_entity, proj_tf, projectile, home) in &projectiles {
        let mut hit = false;
        for (unit_entity, unit_tf, team, mut health, radius) in &mut units {
            if *team == projectile.team || !health.is_alive() {
                continue;
            }
            if let Some(ProjectileHome(target)) = home {
                if unit_entity != *target {
                    continue;
                }
            }
            let reach = projectile.radius + radius.map(|r| r.0).unwrap_or(0.5);
            let vertical = (proj_tf.translation.y - (unit_tf.translation.y + 1.0)).abs();
            if flat_distance(proj_tf.translation, unit_tf.translation) <= reach && vertical < 2.5 {
                health.current -= projectile.damage;
                hit = true;
                break;
            }
        }
        if hit {
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
        (
            &Transform,
            &Team,
            &mut PlayerWallet,
            &mut HeroProgress,
        ),
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
                // Solo-player prototype: gold always goes to the hero.
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

pub fn mitigate(raw_damage: f32, armor: f32) -> f32 {
    let factor = 1.0 - (0.06 * armor) / (1.0 + 0.06 * armor.abs());
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

pub fn spawn_projectile(
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
        Name::new("Projectile"),
        Mesh3d(assets.projectile_mesh.clone()),
        MeshMaterial3d(material),
        transform,
        Projectile {
            damage,
            speed,
            team,
            radius: 0.7,
            lifetime: 2.5,
        },
        ProjectileHome(target),
        Lifetime(2.5),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_reduces_damage() {
        let raw = 100.0;
        let mitigated = mitigate(raw, 10.0);
        assert!(mitigated < raw);
        assert!(mitigated > 50.0);
    }

    #[test]
    fn flat_distance_ignores_height() {
        let a = Vec3::new(0.0, 10.0, 0.0);
        let b = Vec3::new(3.0, -4.0, 4.0);
        assert!((flat_distance(a, b) - 5.0).abs() < 1e-4);
    }
}
