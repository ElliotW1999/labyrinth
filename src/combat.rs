//! Combat core: damage messages and mitigation, spell projectiles, death, bounty, and XP.
//! Basic attacks live in [`crate::basic_attack`] and feed [`DamageEvent`]s in here.

use bevy::prelude::*;

use crate::basic_attack::{
    BasicAttackEvent, BasicAttackImpactEvent, BasicAttackReleaseEvent, advance_attacks_in_flight,
    animate_melee_slashes, request_basic_attacks, resolve_attack_releases,
    resolve_basic_attack_impacts, start_basic_attack_windups, sync_attack_projectile_visuals,
    tick_attack_swings,
};
use crate::components::{
    AbilityId, AttackCooldown, BoundRadius, CombatStats, DamageType, GoldBounty, Health, HeroProgress,
    Lifetime, PlayerHero, PlayerWallet, Projectile, ProjectileHome, ProjectileStyle, Team,
    XpBounty,
};
use crate::dimensions::{area_contains, bounds_of};
use crate::progression::add_xp;
use crate::resources::SharedAssets;
use crate::scale;

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<BasicAttackEvent>()
            .add_message::<BasicAttackReleaseEvent>()
            .add_message::<BasicAttackImpactEvent>()
            .add_message::<DamageEvent>()
            .add_message::<SpawnProjectileEvent>()
            .add_message::<ProjectileHitEvent>()
            .add_systems(
                Update,
                (ensure_attack_range_ring, sync_attack_range_ring).chain(),
            )
            .add_systems(
                Update,
                (
                    tick_attack_cooldowns,
                    request_basic_attacks,
                    start_basic_attack_windups,
                    tick_attack_swings,
                    resolve_attack_releases,
                    advance_attacks_in_flight,
                    resolve_basic_attack_impacts,
                    animate_melee_slashes,
                    sync_attack_projectile_visuals,
                    fly_projectiles,
                    skillshot_contacts,
                    apply_projectile_hits,
                    apply_damage_events,
                    tick_lifetimes,
                    despawn_dead,
                )
                    .chain()
                    .run_if(crate::net::is_sim_authority),
            );
    }
}

/// Raw (pre-mitigation) damage dealt to `target`; armor / magic resist apply on receipt.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    pub source: Option<Entity>,
    pub target: Entity,
    pub amount: f32,
    pub damage_type: DamageType,
}

pub fn apply_damage_events(
    mut events: MessageReader<DamageEvent>,
    mut targets: Query<(&mut Health, &CombatStats)>,
) {
    for event in events.read() {
        let Ok((mut health, stats)) = targets.get_mut(event.target) else {
            continue;
        };
        if !health.is_alive() {
            continue;
        }
        health.current -= apply_damage(event.amount, event.damage_type, stats);
    }
}

fn tick_attack_cooldowns(time: Res<Time>, mut query: Query<&mut AttackCooldown>) {
    let dt = time.delta_secs();
    for mut cd in &mut query {
        cd.0 = (cd.0 - dt).max(0.0);
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
                home.last_pos = target_tf.translation() + Vec3::Y * scale::body(1.0);
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
pub struct GroundBoltImpact;

/// Homing projectile arrived at the target's position this frame.
#[derive(Component, Debug, Clone, Copy)]
pub struct ProjectileReachedTarget;

/// The cast that fired a spell projectile, so its impact can be reported as an ability trigger.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct ProjectilePayload {
    pub caster: Entity,
    pub ability: AbilityId,
    pub rank: u32,
    pub cast_target: Option<Entity>,
    pub aim: Vec3,
}

/// A spell projectile ended. Carries who was involved, not what happens to them:
/// the abilities layer turns this into an `OnProjectileHit` / `OnImpact` trigger.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct ProjectileHitEvent {
    pub payload: ProjectilePayload,
    pub team: Team,
    pub impact: Vec3,
    /// The homing target, when the projectile struck it.
    pub primary: Option<Entity>,
    /// Living enemies within the splash radius of the impact (excluding `primary`).
    pub splashed: Vec<Entity>,
}

/// Request to spawn a spell projectile (homing when `target` is set).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct SpawnProjectileEvent {
    pub team: Team,
    pub origin: Vec3,
    pub target: Option<Entity>,
    pub target_pos: Vec3,
    /// World units per second; `None` uses [`SPELL_PROJECTILE_SPEED`].
    pub speed: Option<f32>,
    /// Set for skillshots: hit every enemy within this distance of the path.
    pub skillshot_width: Option<f32>,
    pub splash_radius: f32,
    pub payload: ProjectilePayload,
}

pub const SPELL_PROJECTILE_SPEED: f32 = scale::u(34.0);

/// Piercing projectile: reports each enemy it passes once.
#[derive(Component, Debug, Clone, Default)]
pub struct Skillshot {
    pub width: f32,
    pub hit: Vec<Entity>,
}

pub fn skillshot_contacts(
    mut hits: MessageWriter<ProjectileHitEvent>,
    mut projectiles: Query<(&Transform, &Projectile, &ProjectilePayload, &mut Skillshot)>,
    units: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
) {
    for (proj_tf, projectile, payload, mut skillshot) in &mut projectiles {
        let impact = proj_tf.translation;
        for (unit, unit_tf, team, health, bound) in &units {
            if *team == projectile.team
                || !health.is_alive()
                || skillshot.hit.contains(&unit)
                || !area_contains(impact, skillshot.width, unit_tf.translation, bounds_of(bound))
            {
                continue;
            }
            skillshot.hit.push(unit);
            hits.write(ProjectileHitEvent {
                payload: *payload,
                team: projectile.team,
                impact,
                primary: Some(unit),
                splashed: Vec::new(),
            });
        }
    }
}

pub fn spawn_requested_projectiles(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut requests: MessageReader<SpawnProjectileEvent>,
) {
    for request in requests.read() {
        spawn_spell_bolt(&mut commands, &assets, request);
    }
}

pub fn apply_projectile_hits(
    mut commands: Commands,
    mut hits: MessageWriter<ProjectileHitEvent>,
    projectiles: Query<(
        Entity,
        &Transform,
        &Projectile,
        &ProjectilePayload,
        Option<&ProjectileHome>,
        Option<&GroundBoltImpact>,
        Option<&ProjectileReachedTarget>,
    )>,
    units: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
) {
    for (proj_entity, proj_tf, projectile, payload, home, ground_impact, reached) in &projectiles {
        let impact = proj_tf.translation;
        let mut despawn = false;
        let mut primary_hit: Option<Entity> = None;

        if let Some(home) = home {
            if let Ok((unit_entity, unit_tf, team, health, radius)) = units.get(home.target) {
                // Aim point is unit.y + body(1); keep the vertical pad in world units.
                let vertical = (impact.y - (unit_tf.translation.y + scale::body(1.0))).abs();
                if *team != projectile.team
                    && health.is_alive()
                    && area_contains(impact, projectile.radius, unit_tf.translation, bounds_of(radius))
                    && vertical < scale::body(2.5)
                {
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

        if !despawn {
            continue;
        }
        let splashed = if projectile.splash_radius > 0.0 {
            units
                .iter()
                .filter(|(unit_entity, unit_tf, team, health, radius)| {
                    **team != projectile.team
                        && health.is_alive()
                        && primary_hit != Some(*unit_entity)
                        && area_contains(
                            impact,
                            projectile.splash_radius,
                            unit_tf.translation,
                            bounds_of(*radius),
                        )
                })
                .map(|(unit_entity, ..)| unit_entity)
                .collect()
        } else {
            Vec::new()
        };
        hits.write(ProjectileHitEvent {
            payload: *payload,
            team: projectile.team,
            impact,
            primary: primary_hit,
            splashed,
        });
        commands.entity(proj_entity).despawn();
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
    const XP_SHARE_RADIUS: f32 = scale::u(18.0);

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
        DamageType::Pure => raw.max(0.0),
    }
}

pub fn mitigate(raw_damage: f32, resistance: f32) -> f32 {
    let factor = 1.0 - (0.06 * resistance) / (1.0 + 0.06 * resistance.abs());
    (raw_damage * factor).max(0.0)
}

pub use crate::dimensions::center_distance as flat_distance;

#[derive(Component, Debug, Clone, Copy)]
struct AttackRangeRing;

#[derive(Component, Debug, Clone, Copy)]
struct HasAttackRangeRing;

fn ensure_attack_range_ring(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    heroes: Query<(Entity, &CombatStats), (With<PlayerHero>, Without<HasAttackRangeRing>)>,
) {
    for (hero, stats) in &heroes {
        let range = stats.attack_range.max(1.0);
        commands.entity(hero).insert(HasAttackRangeRing);
        commands.entity(hero).with_children(|parent| {
            parent.spawn((
                Name::new("Attack Range Ring"),
                AttackRangeRing,
                Mesh3d(assets.indicator_ring_mesh.clone()),
                MeshMaterial3d(assets.attack_range_ring_mat.clone()),
                Transform::from_xyz(0.0, 0.12, 0.0)
                    .with_scale(Vec3::new(range, 1.0, range)),
            ));
        });
    }
}

fn sync_attack_range_ring(
    heroes: Query<&CombatStats, With<PlayerHero>>,
    mut rings: Query<(&mut Transform, &ChildOf), With<AttackRangeRing>>,
) {
    for (mut transform, child_of) in &mut rings {
        let Ok(stats) = heroes.get(child_of.parent()) else {
            continue;
        };
        let range = stats.attack_range.max(1.0);
        transform.scale = Vec3::new(range, 1.0, range);
        transform.translation.y = 0.12;
    }
}

fn spawn_spell_bolt(commands: &mut Commands, assets: &SharedAssets, request: &SpawnProjectileEvent) {
    let SpawnProjectileEvent {
        team,
        origin,
        target,
        target_pos,
        speed,
        skillshot_width,
        splash_radius,
        payload,
    } = *request;
    let speed = speed.unwrap_or(SPELL_PROJECTILE_SPEED).max(1.0);
    let travel = flat_distance(origin, target_pos) / speed + 0.25;
    let start = origin + Vec3::Y * scale::body(1.2);
    let aim = (target_pos + Vec3::Y * scale::body(1.0)) - start;
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
            speed,
            team,
            radius: scale::body(0.85),
            lifetime: travel.max(2.5),
            splash_radius,
        },
        payload,
        ProjectileStyle::SpellBolt,
        Lifetime(travel.max(2.5)),
    ));

    if let Some(target) = target {
        entity.insert(ProjectileHome {
            target,
            last_pos: target_pos + Vec3::Y * scale::body(1.0),
        });
    } else {
        entity.insert((
            Lifetime(travel),
            GroundBoltAim {
                position: target_pos,
            },
        ));
        if let Some(width) = skillshot_width {
            entity.insert(Skillshot {
                width,
                hit: Vec::new(),
            });
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct GroundBoltAim {
    pub position: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

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

    #[test]
    fn projectile_reports_its_victims_without_dealing_damage() {
        let mut world = World::new();
        world.init_resource::<Messages<ProjectileHitEvent>>();
        let unit = |world: &mut World, team: Team, x: f32| {
            world
                .spawn((Transform::from_xyz(x, 0.0, 0.0), team, Health::new(500.0)))
                .id()
        };
        let target = unit(&mut world, Team::Dire, 0.0);
        let splashed = unit(&mut world, Team::Dire, 30.0);
        let far = unit(&mut world, Team::Dire, 900.0);
        let ally = unit(&mut world, Team::Radiant, 20.0);
        let caster = unit(&mut world, Team::Radiant, -500.0);
        let payload = ProjectilePayload {
            caster,
            ability: AbilityId::Bolt,
            rank: 1,
            cast_target: Some(target),
            aim: Vec3::ZERO,
        };
        world.spawn((
            Transform::from_xyz(0.0, scale::body(1.0), 0.0),
            Projectile {
                speed: 1.0,
                team: Team::Radiant,
                radius: scale::body(0.85),
                lifetime: 1.0,
                splash_radius: 60.0,
            },
            payload,
            ProjectileHome {
                target,
                last_pos: Vec3::ZERO,
            },
        ));
        world.run_system_once(apply_projectile_hits).unwrap();

        let hits: Vec<_> = world
            .resource::<Messages<ProjectileHitEvent>>()
            .iter_current_update_messages()
            .cloned()
            .collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].primary, Some(target));
        assert_eq!(hits[0].splashed, vec![splashed]);
        assert_eq!(hits[0].payload, payload);
        for entity in [target, splashed, far, ally] {
            assert_eq!(world.get::<Health>(entity).unwrap().current, 500.0);
        }
    }

    #[test]
    fn damage_events_apply_mitigation_and_skip_dead_targets() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageEvent>>();
        let stats = CombatStats::simple(0.0, 0.0, 1.0, 10.0, 0.0, 0.0);
        let alive = world.spawn((Health::new(500.0), stats)).id();
        let mut dead_hp = Health::new(500.0);
        dead_hp.current = 0.0;
        let dead = world.spawn((dead_hp, stats)).id();
        for target in [alive, dead] {
            world.write_message(DamageEvent {
                source: None,
                target,
                amount: 100.0,
                damage_type: DamageType::Physical,
            });
        }
        world.run_system_once(apply_damage_events).unwrap();
        let expected = 500.0 - apply_damage(100.0, DamageType::Physical, &stats);
        assert!((world.get::<Health>(alive).unwrap().current - expected).abs() < 1e-3);
        assert_eq!(world.get::<Health>(dead).unwrap().current, 0.0);
    }
}
