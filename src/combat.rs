//! Auto-attack, damage mitigation, death, and projectile flight.

use bevy::prelude::*;

use crate::components::{
    AttackCooldown, AttackTarget, CombatStats, GoldBounty, Health, Lifetime, PlayerHero,
    PlayerWallet, Projectile, ProjectileTarget, Team, UnitRadius,
};
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
    mut attackers: Query<(
        Entity,
        &Transform,
        &Team,
        &CombatStats,
        &mut AttackCooldown,
        Option<&AttackTarget>,
    )>,
    mut target_access: ParamSet<(
        Query<(Entity, &Transform, &Team, &Health, &CombatStats, Option<&UnitRadius>)>,
        Query<&mut Health>,
    )>,
) {
    let target_snapshots: Vec<_> = target_access
        .p0()
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

    let mut hits: Vec<(Entity, f32)> = Vec::new();

    for (_entity, transform, team, stats, mut cooldown, current_target) in &mut attackers {
        if cooldown.0 > 0.0 || stats.attack_damage <= 0.0 || stats.attack_range <= 0.0 {
            continue;
        }

        let origin = transform.translation;
        let chosen = current_target
            .and_then(|AttackTarget(id)| {
                target_snapshots
                    .iter()
                    .find(|(e, _, target_team, _, _)| *e == *id && *target_team == team.enemy())
                    .copied()
            })
            .or_else(|| {
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

        let Some((target_entity, _, _, armor, _)) = chosen else {
            continue;
        };

        hits.push((target_entity, mitigate(stats.attack_damage, armor)));
        cooldown.0 = 1.0 / stats.attack_speed.max(0.1);
    }

    let mut health = target_access.p1();
    for (target_entity, damage) in hits {
        if let Ok(mut hp) = health.get_mut(target_entity) {
            hp.current -= damage;
        }
    }
}

fn fly_projectiles(
    time: Res<Time>,
    mut projectiles: Query<(&mut Transform, &Projectile, &ProjectileTarget)>,
) {
    let dt = time.delta_secs();
    for (mut transform, projectile, target) in &mut projectiles {
        let mut destination = target.position;
        destination.y = transform.translation.y;
        let to = destination - transform.translation;
        let dist = to.length();
        let step = projectile.speed * dt;
        if step >= dist {
            transform.translation = destination;
        } else if dist > f32::EPSILON {
            transform.translation += (to / dist) * step;
        }
    }
}

fn apply_projectile_hits(
    mut commands: Commands,
    projectiles: Query<(Entity, &Transform, &Projectile)>,
    mut units: Query<(Entity, &Transform, &Team, &mut Health, Option<&UnitRadius>)>,
) {
    for (proj_entity, proj_tf, projectile) in &projectiles {
        let mut hit_someone = false;
        for (_unit_entity, unit_tf, team, mut health, radius) in &mut units {
            if *team == projectile.team || !health.is_alive() {
                continue;
            }
            let reach = projectile.radius + radius.map(|r| r.0).unwrap_or(0.5);
            if flat_distance(proj_tf.translation, unit_tf.translation) <= reach {
                health.current -= projectile.damage;
                hit_someone = true;
            }
        }
        if hit_someone {
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
    dead: Query<(Entity, &Health, Option<&GoldBounty>, Option<&Team>), Without<PlayerHero>>,
    mut wallets: Query<&mut PlayerWallet, With<PlayerHero>>,
    hero_team: Query<&Team, With<PlayerHero>>,
) {
    let Ok(player_team) = hero_team.single() else {
        return;
    };
    let mut wallet = wallets.single_mut().ok();

    for (entity, health, bounty, team) in &dead {
        if health.is_alive() {
            continue;
        }
        if let (Some(bounty), Some(team), Some(wallet)) = (bounty, team, wallet.as_mut()) {
            if *team == player_team.enemy() {
                wallet.gold += bounty.0;
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

pub fn spawn_projectile(
    commands: &mut Commands,
    assets: &SharedAssets,
    origin: Vec3,
    target: Vec3,
    team: Team,
    damage: f32,
) {
    commands.spawn((
        Name::new("Projectile"),
        Mesh3d(assets.projectile_mesh.clone()),
        MeshMaterial3d(assets.projectile_mat.clone()),
        Transform::from_translation(origin + Vec3::Y * 1.0),
        Projectile {
            damage,
            speed: 28.0,
            team,
            radius: 0.6,
            lifetime: 3.0,
        },
        ProjectileTarget { position: target },
        Lifetime(3.0),
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
