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
    AbilityId, AttackCooldown, BoundRadius, CombatStats, DamageType, GoldBounty, Health,
    HeroAttributes, HeroProgress, Lifetime, PlayerHero, PlayerWallet, Projectile, ProjectileHome,
    ProjectileStyle, ShieldPool, Team, XpBounty,
};
use crate::status::{StatusEffects, subtract_status_stat_delta};
use crate::dimensions::{area_contains, bounds_of};
use crate::progression::add_xp;
use crate::resources::SharedAssets;
use crate::scale;

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CombatRng>()
            .init_resource::<DamageTestHooks>()
            .add_message::<BasicAttackEvent>()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DamageKind {
    #[default]
    Generic,
    BasicAttack,
    Ability,
    Dot,
}

/// Raw (pre-mitigation) damage dealt to `target`; resolved in [`apply_damage_events`].
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    pub source: Option<Entity>,
    pub target: Entity,
    pub amount: f32,
    pub damage_type: DamageType,
    pub kind: DamageKind,
    pub can_crit: bool,
    pub is_spell: bool,
    pub applies_block: bool,
    pub pierces_shields: bool,
    pub reflects: bool,
}

impl DamageEvent {
    pub fn new(target: Entity, amount: f32, damage_type: DamageType) -> Self {
        Self {
            source: None,
            target,
            amount,
            damage_type,
            kind: DamageKind::Generic,
            can_crit: false,
            is_spell: false,
            applies_block: true,
            pierces_shields: false,
            reflects: true,
        }
    }

    pub fn basic_attack(source: Entity, target: Entity, amount: f32) -> Self {
        Self {
            source: Some(source),
            target,
            amount,
            damage_type: DamageType::Physical,
            kind: DamageKind::BasicAttack,
            can_crit: true,
            is_spell: false,
            applies_block: true,
            pierces_shields: false,
            reflects: true,
        }
    }

    pub fn spell(source: Entity, target: Entity, amount: f32, damage_type: DamageType) -> Self {
        Self {
            source: Some(source),
            target,
            amount,
            damage_type,
            kind: DamageKind::Ability,
            can_crit: false,
            is_spell: true,
            applies_block: true,
            pierces_shields: false,
            reflects: true,
        }
    }

    pub fn dot(source: Option<Entity>, target: Entity, amount: f32, damage_type: DamageType) -> Self {
        Self {
            source,
            target,
            amount,
            damage_type,
            kind: DamageKind::Dot,
            can_crit: false,
            is_spell: false,
            applies_block: true,
            pierces_shields: false,
            reflects: false,
        }
    }
}

/// Deterministic combat RNG (seed with [`CombatRng::seed`] in tests).
#[derive(Resource, Debug, Clone, Copy)]
pub struct CombatRng {
    state: u64,
}

impl Default for CombatRng {
    fn default() -> Self {
        Self { state: 0xDEAD_BEEF_CAFE_1234 }
    }
}

impl CombatRng {
    pub fn seed(&mut self, seed: u64) {
        self.state = seed.max(1);
    }

    fn next_f32(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state as f32) / u64::MAX as f32
    }

    pub fn roll_crit(&mut self, chance: f32) -> bool {
        chance > 0.0 && self.next_f32() < chance.clamp(0.0, 1.0)
    }
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct DamageTestHooks {
    pub force_crit: bool,
}

pub struct ResolvedDamage {
    pub health_delta: f32,
    pub shield_absorbed: f32,
}

pub fn resolve_outgoing_amount(
    raw: f32,
    event: &DamageEvent,
    source_stats: Option<&CombatStats>,
    rng: &mut CombatRng,
    hooks: &DamageTestHooks,
) -> f32 {
    let mut amount = raw.max(0.0);
    if let Some(stats) = source_stats {
        if event.damage_type == DamageType::Physical {
            amount *= 1.0 + stats.outgoing_physical_amp;
        }
        if event.is_spell && event.damage_type == DamageType::Magical {
            amount *= 1.0 + stats.outgoing_spell_amp;
        }
        if event.can_crit && (hooks.force_crit || rng.roll_crit(stats.crit_chance)) {
            amount *= stats.crit_multiplier.max(1.0);
        }
    }
    amount
}

pub fn resolve_incoming_damage(
    raw: f32,
    damage_type: DamageType,
    target_stats: &CombatStats,
    shield: Option<&mut ShieldPool>,
    damage_immune: bool,
    applies_block: bool,
    pierces_shields: bool,
) -> ResolvedDamage {
    if damage_immune || raw <= 0.0 {
        return ResolvedDamage {
            health_delta: 0.0,
            shield_absorbed: 0.0,
        };
    }

    let mut remaining = raw;
    let mut shield_absorbed = 0.0;
    if !pierces_shields {
        if let Some(pool) = shield {
            let absorbed = remaining.min(pool.amount);
            pool.amount -= absorbed;
            remaining -= absorbed;
            shield_absorbed = absorbed;
        }
    }
    if applies_block && target_stats.damage_block > 0.0 {
        remaining = (remaining - target_stats.damage_block).max(0.0);
    }
    let mut mitigated = apply_damage(remaining, damage_type, target_stats);
    if target_stats.incoming_damage_amp != 0.0 {
        mitigated *= 1.0 + target_stats.incoming_damage_amp;
    }
    ResolvedDamage {
        health_delta: mitigated.max(0.0),
        shield_absorbed,
    }
}

pub fn apply_damage_events(
    mut damage: ParamSet<(
        MessageReader<DamageEvent>,
        MessageWriter<DamageEvent>,
    )>,
    mut rng: Option<ResMut<CombatRng>>,
    hooks: Option<Res<DamageTestHooks>>,
    mut unit_stats: ParamSet<(
        Query<&CombatStats>,
        Query<(
            &mut Health,
            &mut CombatStats,
            Option<&mut ShieldPool>,
            Option<&mut StatusEffects>,
            Option<&HeroAttributes>,
        )>,
    )>,
) {
    let hooks = hooks.as_deref().copied().unwrap_or_default();
    let mut fallback_rng = CombatRng::default();
    let rng = rng.as_deref_mut().unwrap_or(&mut fallback_rng);
    let mut reflected = Vec::new();
    for event in damage.p0().read() {
        let source_stats_snapshot = event
            .source
            .and_then(|e| unit_stats.p0().get(e).ok().copied());
        let mut target_query = unit_stats.p1();
        let Ok((mut health, mut target_stats, mut shield, mut statuses, attrs)) =
            target_query.get_mut(event.target)
        else {
            continue;
        };
        if !health.is_alive() {
            continue;
        }
        if statuses
            .as_ref()
            .is_some_and(|s| s.is_invulnerable())
        {
            continue;
        }

        // Sleep breaks on any damage receipt (before applying this damage).
        if let (Some(st), Some(attrs)) = (statuses.as_deref_mut(), attrs) {
            if st.is_asleep() {
                let agi = attrs.agility;
                while let Some(i) = st
                    .effects
                    .iter()
                    .position(|e| e.asleep && e.remaining > 0.0)
                {
                    let ef = st.effects.remove(i);
                    subtract_status_stat_delta(&ef, &mut target_stats, agi);
                }
            }
        }

        let damage_immune = statuses
            .as_ref()
            .is_some_and(|s| s.has_damage_immunity());
        let damage_reflect = target_stats.damage_reflect;
        let outgoing = resolve_outgoing_amount(
            event.amount,
            event,
            source_stats_snapshot.as_ref(),
            rng,
            &hooks,
        );
        let resolved = resolve_incoming_damage(
            outgoing,
            event.damage_type,
            &target_stats,
            shield.as_deref_mut(),
            damage_immune,
            event.applies_block,
            event.pierces_shields,
        );
        if resolved.shield_absorbed > 0.0 {
            if let Some(st) = statuses.as_deref_mut() {
                crate::status::consume_shields(st, resolved.shield_absorbed);
            }
        }
        health.current -= resolved.health_delta;

        if event.reflects && resolved.health_delta > 0.0 && damage_reflect > 0.0 {
            if let Some(source) = event.source {
                let amount = resolved.health_delta * damage_reflect;
                reflected.push(DamageEvent {
                    source: Some(event.target),
                    target: source,
                    amount,
                    damage_type: event.damage_type,
                    kind: DamageKind::Generic,
                    can_crit: false,
                    is_spell: false,
                    applies_block: true,
                    pierces_shields: false,
                    reflects: false,
                });
            }
        }
    }
    for event in reflected {
        damage.p1().write(event);
    }
}

/// Chain lightning: jump to nearby enemies with falloff per hop.
pub fn emit_chain_damage(
    damage: &mut MessageWriter<DamageEvent>,
    primary: Entity,
    source: Entity,
    amount: f32,
    damage_type: DamageType,
    team: Team,
    jumps: u32,
    radius: f32,
    falloff: f32,
    units: &Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
) {
    let mut hit = vec![primary];
    let mut current = primary;
    let mut current_amount = amount;
    for _ in 0..jumps {
        let Ok((_, current_tf, _, _, _)) = units.get(current) else {
            break;
        };
        let origin = current_tf.translation;
        let next = units
            .iter()
            .filter(|(entity, tf, unit_team, health, bound)| {
                **unit_team == team.enemy()
                    && health.is_alive()
                    && !hit.contains(entity)
                    && area_contains(origin, radius, tf.translation, bounds_of(*bound))
            })
            .min_by(|a, b| {
                flat_distance(origin, a.1.translation)
                    .total_cmp(&flat_distance(origin, b.1.translation))
            })
            .map(|(e, ..)| e);
        let Some(next) = next else {
            break;
        };
        current_amount *= falloff;
        damage.write(DamageEvent::spell(source, next, current_amount, damage_type));
        hit.push(next);
        current = next;
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
                commands
                    .entity(entity)
                    .insert(GroundBoltAim { position: last });
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
                || !area_contains(
                    impact,
                    skillshot.width,
                    unit_tf.translation,
                    bounds_of(bound),
                )
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
                    && area_contains(
                        impact,
                        projectile.radius,
                        unit_tf.translation,
                        bounds_of(radius),
                    )
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
    mut heroes: Query<(&Transform, &Team, &mut PlayerWallet, &mut HeroProgress), With<PlayerHero>>,
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
                Transform::from_xyz(0.0, 0.12, 0.0).with_scale(Vec3::new(range, 1.0, range)),
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

fn spawn_spell_bolt(
    commands: &mut Commands,
    assets: &SharedAssets,
    request: &SpawnProjectileEvent,
) {
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
            world.write_message(DamageEvent::new(
                target,
                100.0,
                DamageType::Physical,
            ));
        }
        world.run_system_once(apply_damage_events).unwrap();
        let expected = 500.0 - apply_damage(100.0, DamageType::Physical, &stats);
        assert!((world.get::<Health>(alive).unwrap().current - expected).abs() < 1e-3);
        assert_eq!(world.get::<Health>(dead).unwrap().current, 0.0);
    }

    #[test]
    fn shield_absorbs_before_health() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<CombatRng>();
        let stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        let target = world
            .spawn((Health::new(500.0), stats, ShieldPool { amount: 120.0 }))
            .id();
        world.write_message(DamageEvent::new(
            target,
            100.0,
            DamageType::Physical,
        ));
        world.run_system_once(apply_damage_events).unwrap();
        assert_eq!(world.get::<ShieldPool>(target).unwrap().amount, 20.0);
        assert_eq!(world.get::<Health>(target).unwrap().current, 500.0);
    }

    #[test]
    fn pure_damage_ignores_armor() {
        let stats = CombatStats::simple(0.0, 0.0, 1.0, 50.0, 50.0, 0.0);
        let pure = apply_damage(100.0, DamageType::Pure, &stats);
        let physical = apply_damage(100.0, DamageType::Physical, &stats);
        assert!((pure - 100.0).abs() < 1e-3);
        assert!(physical < pure);
    }

    #[test]
    fn crit_doubles_with_test_hook() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<CombatRng>();
        world.insert_resource(DamageTestHooks {
            force_crit: true,
        });
        let mut attacker_stats = CombatStats::simple(50.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        attacker_stats.crit_chance = 0.0;
        let defender = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        let attacker = world.spawn(attacker_stats).id();
        let target = world.spawn((Health::new(500.0), defender)).id();
        world.write_message(DamageEvent {
            source: Some(attacker),
            target,
            amount: 50.0,
            damage_type: DamageType::Physical,
            kind: DamageKind::BasicAttack,
            can_crit: true,
            is_spell: false,
            applies_block: true,
            pierces_shields: false,
            reflects: false,
        });
        world.run_system_once(apply_damage_events).unwrap();
        let expected = 500.0 - apply_damage(100.0, DamageType::Physical, &defender);
        assert!((world.get::<Health>(target).unwrap().current - expected).abs() < 1e-3);
    }

    #[test]
    fn spell_amp_increases_magical_spell_damage() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<CombatRng>();
        let mut caster_stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        caster_stats.outgoing_spell_amp = 0.25;
        let defender = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        let caster = world.spawn(caster_stats).id();
        let target = world.spawn((Health::new(500.0), defender)).id();
        world.write_message(DamageEvent::spell(caster, target, 100.0, DamageType::Magical));
        world.run_system_once(apply_damage_events).unwrap();
        let expected = 500.0 - apply_damage(125.0, DamageType::Magical, &defender);
        assert!((world.get::<Health>(target).unwrap().current - expected).abs() < 1e-3);
    }
}
