//! Auto-attack via animated projectiles, damage types, death, bounty, and XP.

use bevy::prelude::*;

use crate::components::{
    AttackCooldown, AttackTarget, CombatStats, DamageType, GoldBounty, Ground, Health, HeroProgress,
    Lifetime, PlayerHero, PlayerWallet, Projectile, ProjectileHome, ProjectileStyle, Team,
    UnitRadius, XpBounty,
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
        .map(|(e, t, team, _, _stats, radius)| {
            (e, t.translation, *team, radius.map(|r| r.0).unwrap_or(0.5))
        })
        .collect();

    for (_entity, transform, team, stats, mut cooldown, current_target, is_player) in
        &mut attackers
    {
        if cooldown.0 > 0.0 || stats.attack_damage <= 0.0 || stats.attack_range <= 0.0 {
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

        let damage = stats.attack_damage;
        spawn_auto_attack(
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
    mut projectiles: Query<(
        Entity,
        &mut Transform,
        &Projectile,
        Option<&ProjectileHome>,
        Option<&GroundBoltAim>,
    )>,
    homes: Query<&GlobalTransform, Without<Projectile>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, projectile, home, ground_aim) in &mut projectiles {
        let mut destination = transform.translation + *transform.forward() * projectile.speed;

        if let Some(ProjectileHome(target)) = home {
            if let Ok(target_tf) = homes.get(*target) {
                destination = target_tf.translation() + Vec3::Y * 1.0;
            } else {
                commands.entity(entity).remove::<ProjectileHome>();
            }
        } else if let Some(aim) = ground_aim {
            destination = Vec3::new(aim.position.x, transform.translation.y, aim.position.z);
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
            if ground_aim.is_some() {
                // Trigger splash at the aim point by tagging for impact next frame.
                commands.entity(entity).insert(GroundBoltImpact);
            }
        } else {
            transform.translation += dir * step;
        }
        transform.look_to(dir, Vec3::Y);
    }
}

#[derive(Component, Debug, Clone, Copy)]
struct GroundBoltImpact;

fn apply_projectile_hits(
    mut commands: Commands,
    projectiles: Query<(
        Entity,
        &Transform,
        &Projectile,
        Option<&ProjectileHome>,
        Option<&GroundBoltImpact>,
    )>,
    mut units: Query<(Entity, &Transform, &Team, &mut Health, &CombatStats, Option<&UnitRadius>)>,
) {
    for (proj_entity, proj_tf, projectile, home, ground_impact) in &projectiles {
        let impact = proj_tf.translation;
        let mut despawn = false;
        let mut primary_hit: Option<Entity> = None;

        if let Some(ProjectileHome(target)) = home {
            for (unit_entity, unit_tf, team, mut health, stats, radius) in &mut units {
                if unit_entity != *target || *team == projectile.team || !health.is_alive() {
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
        ProjectileHome(target),
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
        entity.insert(ProjectileHome(target));
    } else {
        // Non-homing ground bolt: store aim via a short-lived move toward point using home-less flight.
        // Face already set; fly_projectiles will go forward. Nudge lifetime by distance.
        let dist = flat_distance(origin, target_pos);
        let travel = (dist / 34.0) + 0.05;
        entity.insert(Lifetime(travel));
        // Also mark a one-shot impact by reducing projectile lifetime.
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
        let stats = CombatStats {
            attack_damage: 0.0,
            attack_range: 0.0,
            attack_speed: 1.0,
            armor: 10.0,
            magic_resist: 0.0,
            move_speed: 0.0,
        };
        let raw = 100.0;
        let mitigated = apply_damage(raw, DamageType::Physical, &stats);
        assert!(mitigated < raw);
        assert!(mitigated > 50.0);
    }

    #[test]
    fn magic_resist_reduces_magical() {
        let stats = CombatStats {
            attack_damage: 0.0,
            attack_range: 0.0,
            attack_speed: 1.0,
            armor: 0.0,
            magic_resist: 10.0,
            move_speed: 0.0,
        };
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
}
