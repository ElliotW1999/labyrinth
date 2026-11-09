//! Combat core: damage messages and mitigation, spell projectiles, death, bounty, and XP.
//! Basic attacks live in [`crate::basic_attack`] and feed [`DamageEvent`]s in here.

use bevy::prelude::*;

use crate::abilities::effects::EffectSource;
use crate::basic_attack::{
    BasicAttackEvent, BasicAttackImpactEvent, BasicAttackLandedEvent, BasicAttackReleaseEvent,
    ExtraAttackEvent, advance_attacks_in_flight, animate_melee_slashes, release_extra_attacks,
    request_basic_attacks, resolve_attack_releases, resolve_basic_attack_impacts,
    start_basic_attack_windups, sync_attack_projectile_visuals, tick_attack_swings,
};
use crate::components::{
    AttackCooldown, BoundRadius, CombatStats, DamageType, GoldBounty, Health, HeroAttributes,
    HeroProgress, Lifetime, Mana, PlayerHero, PlayerWallet, Projectile, ProjectileHome,
    ProjectileStyle, ShieldPool, Team, XpBounty,
};
use crate::dimensions::{area_contains, bounds_of};
use crate::progression::add_xp;
use crate::resources::SharedAssets;
use crate::scale;
use crate::status::{StatusEffects, is_untargetable};
use crate::vitals::{
    HealKind, ManaShield, ResourceStat, apply_heal, heal_factor, outgoing_heal_amp, resource_stat,
};

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CombatRng>()
            .init_resource::<DamageTestHooks>()
            .add_message::<BasicAttackEvent>()
            .add_message::<BasicAttackReleaseEvent>()
            .add_message::<BasicAttackImpactEvent>()
            .add_message::<BasicAttackLandedEvent>()
            .add_message::<ExtraAttackEvent>()
            .add_message::<DamageEvent>()
            .add_message::<DamageAppliedEvent>()
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
                    release_extra_attacks,
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
                    // After the AI and movement stages: their systems queue inserts on
                    // units (`MoveTarget`, `AttackTarget`, ...), and a unit despawned
                    // before those commands are applied panics the app.
                    despawn_dead.after(crate::movement::SimSet::Movement),
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
    /// Cleave from a basic attack (around the attacker); never leeched from.
    Cleave,
    /// Splash from a basic attack (around the struck unit); never leeched from.
    Splash,
    /// Bonus damage dealt by an on-hit effect of a landed attack. Counts as attack damage:
    /// no spell amp, no block, leeched like a basic attack.
    AttackBonus,
}

/// Raw (pre-mitigation) damage dealt to `target`; resolved in [`apply_damage_events`].
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    pub source: Option<Entity>,
    pub target: Entity,
    pub amount: f32,
    pub damage_type: DamageType,
    pub kind: DamageKind,
    /// Informational: the main hit rolled a critical strike (decided in the attack-hit stage).
    pub crit: bool,
    pub is_spell: bool,
    pub applies_block: bool,
    pub pierces_shields: bool,
    pub reflects: bool,
    /// Fraction of health actually removed that heals the source (health transfer).
    pub drain_ratio: f32,
    /// Caused by a reaction (a damage- or status-class trigger): never fires damage or
    /// status triggers itself, so reactions cannot ping-pong.
    pub reactive: bool,
}

impl DamageEvent {
    #[cfg(test)]
    pub fn new(target: Entity, amount: f32, damage_type: DamageType) -> Self {
        Self {
            source: None,
            target,
            amount,
            damage_type,
            kind: DamageKind::Generic,
            crit: false,
            is_spell: false,
            applies_block: true,
            pierces_shields: false,
            reflects: true,
            drain_ratio: 0.0,
            reactive: false,
        }
    }

    pub fn basic_attack(source: Entity, target: Entity, amount: f32) -> Self {
        Self {
            source: Some(source),
            target,
            amount,
            damage_type: DamageType::Physical,
            kind: DamageKind::BasicAttack,
            crit: false,
            is_spell: false,
            applies_block: true,
            pierces_shields: false,
            reflects: true,
            drain_ratio: 0.0,
            reactive: false,
        }
    }

    pub fn spell(source: Entity, target: Entity, amount: f32, damage_type: DamageType) -> Self {
        Self {
            source: Some(source),
            target,
            amount,
            damage_type,
            kind: DamageKind::Ability,
            crit: false,
            is_spell: true,
            applies_block: true,
            pierces_shields: false,
            reflects: true,
            drain_ratio: 0.0,
            reactive: false,
        }
    }

    /// The damage an ability or item effect deals: attack bonus damage under a trigger caused
    /// by a basic attack, spell damage otherwise. The one place that choice is made.
    pub fn from_effect(
        source: Entity,
        target: Entity,
        amount: f32,
        damage_type: DamageType,
        from_attack: bool,
    ) -> Self {
        if from_attack {
            Self::attack_bonus(source, target, amount, damage_type)
        } else {
            Self::spell(source, target, amount, damage_type)
        }
    }

    pub fn dot(
        source: Option<Entity>,
        target: Entity,
        amount: f32,
        damage_type: DamageType,
    ) -> Self {
        Self {
            source,
            target,
            amount,
            damage_type,
            kind: DamageKind::Dot,
            crit: false,
            is_spell: false,
            applies_block: true,
            pierces_shields: false,
            reflects: false,
            drain_ratio: 0.0,
            reactive: false,
        }
    }

    /// Splash from a basic attack (cleave). Identical to a basic attack except that it
    /// never leeches.
    pub fn cleave(source: Entity, target: Entity, amount: f32) -> Self {
        Self {
            kind: DamageKind::Cleave,
            ..Self::basic_attack(source, target, amount)
        }
    }

    /// Bonus damage from an on-hit effect of a landed attack (items, abilities, mana burn).
    pub fn attack_bonus(
        source: Entity,
        target: Entity,
        amount: f32,
        damage_type: DamageType,
    ) -> Self {
        Self {
            source: Some(source),
            target,
            amount,
            damage_type,
            kind: DamageKind::AttackBonus,
            crit: false,
            is_spell: false,
            applies_block: false,
            pierces_shields: false,
            reflects: true,
            drain_ratio: 0.0,
            reactive: false,
        }
    }

    /// Splash from a basic attack around the struck unit. Like cleave, it never leeches.
    pub fn splash(source: Entity, target: Entity, amount: f32) -> Self {
        Self {
            kind: DamageKind::Splash,
            ..Self::basic_attack(source, target, amount)
        }
    }

    /// Marks the main hit as critical (informational; the amount is already multiplied).
    pub fn with_crit(mut self, crit: bool) -> Self {
        self.crit = crit;
        self
    }

    /// Marks the damage as caused by a reaction (see [`DamageEvent::reactive`]).
    pub fn reactive(mut self, reactive: bool) -> Self {
        self.reactive = reactive;
        self
    }

    /// Marks the damage as a health transfer: the source heals `ratio` of the health removed.
    pub fn drained(mut self, ratio: f32) -> Self {
        self.drain_ratio = ratio;
        self
    }
}

/// What one [`DamageEvent`] did, reported by [`apply_damage_events`] (the only place health
/// goes down and so the only writer). The reactive-trigger dispatchers read it one frame
/// later. Events that never reach the target (dead, invulnerable) are not reported.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct DamageAppliedEvent {
    pub source: Option<Entity>,
    pub target: Entity,
    pub kind: DamageKind,
    pub is_spell: bool,
    /// The damage was caused by a reaction (see [`DamageEvent::reactive`]).
    pub reactive: bool,
    /// Health removed plus shield absorbed (post-mitigation).
    pub amount: f32,
    pub health_before: f32,
    pub health_after: f32,
    pub max_health: f32,
    pub target_pos: Vec3,
}

/// Deterministic combat RNG (seed with [`CombatRng::seed`] in tests).
#[derive(Resource, Debug, Clone, Copy)]
pub struct CombatRng {
    state: u64,
}

impl Default for CombatRng {
    fn default() -> Self {
        Self {
            state: 0xDEAD_BEEF_CAFE_1234,
        }
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

    /// Bernoulli roll. Draws nothing when the outcome is certain (`chance <= 0` or `>= 1`).
    pub fn roll(&mut self, chance: f32) -> bool {
        if chance <= 0.0 {
            return false;
        }
        if chance >= 1.0 {
            return true;
        }
        self.next_f32() < chance
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
) -> f32 {
    let mut amount = raw.max(0.0);
    if let Some(stats) = source_stats {
        if event.damage_type == DamageType::Physical {
            amount *= 1.0 + stats.outgoing_physical_amp;
        }
        if event.is_spell && event.damage_type == DamageType::Magical {
            amount *= 1.0 + stats.outgoing_spell_amp;
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

/// Divert damage to mana: `min(damage x absorb, mana x damage_per_mana)` is absorbed and
/// mana is spent for it. Returns the damage absorbed.
pub fn absorb_with_mana_shield(damage: f32, mana: &mut Mana, shield: ManaShield) -> f32 {
    let per_mana = shield.damage_per_mana.max(f32::EPSILON);
    let absorbed = (damage * shield.absorb)
        .min(mana.current.max(0.0) * per_mana)
        .max(0.0);
    mana.current -= absorbed / per_mana;
    absorbed
}

pub fn apply_damage_events(
    mut damage: ParamSet<(MessageReader<DamageEvent>, MessageWriter<DamageEvent>)>,
    mut applied: MessageWriter<DamageAppliedEvent>,
    mut lifecycle: MessageWriter<crate::status::StatusLifecycleEvent>,
    positions: Query<&Transform>,
    mut unit_stats: ParamSet<(
        Query<(&CombatStats, Option<&StatusEffects>)>,
        Query<(
            &mut Health,
            &mut CombatStats,
            Option<&mut ShieldPool>,
            Option<&mut StatusEffects>,
            Option<&HeroAttributes>,
            Option<&mut Mana>,
        )>,
    )>,
) {
    let mut reflected = Vec::new();
    // (source, health to give back); applied after the whole batch, same frame.
    let mut leeches: Vec<(Entity, f32)> = Vec::new();
    for event in damage.p0().read() {
        let source_snapshot = event.source.and_then(|e| {
            unit_stats.p0().get(e).ok().map(|(stats, statuses)| {
                (
                    *stats,
                    resource_stat(Some(stats), statuses, ResourceStat::Lifesteal),
                    resource_stat(Some(stats), statuses, ResourceStat::SpellLifesteal),
                )
            })
        });
        let source_stats_snapshot = source_snapshot.map(|(stats, _, _)| stats);
        let mut target_query = unit_stats.p1();
        let Ok((mut health, mut target_stats, mut shield, mut statuses, attrs, mut mana)) =
            target_query.get_mut(event.target)
        else {
            continue;
        };
        if !health.is_alive() {
            continue;
        }
        if statuses.as_ref().is_some_and(|s| s.is_invulnerable()) {
            continue;
        }

        // Sleep breaks on any damage receipt (before applying this damage).
        if let (Some(st), Some(attrs)) = (statuses.as_deref_mut(), attrs) {
            let woken = crate::status::wake_from_sleep(
                st,
                &mut target_stats,
                attrs.agility,
                shield.as_deref_mut(),
            );
            crate::status::report_removed(
                &mut lifecycle,
                event.target,
                event.source,
                event.reactive,
                crate::status::StatusLifecycle::Removed,
                woken,
            );
        }

        let damage_immune = statuses.as_ref().is_some_and(|s| s.has_damage_immunity());
        let damage_reflect = target_stats.damage_reflect;
        let outgoing = resolve_outgoing_amount(event.amount, event, source_stats_snapshot.as_ref());
        let mut resolved = resolve_incoming_damage(
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
        // Mana shield: pays for post-mitigation damage, after any ShieldPool.
        if let (Some(shield), Some(mana)) = (
            statuses.as_ref().and_then(|s| s.mana_shield()),
            mana.as_deref_mut(),
        ) {
            resolved.health_delta -= absorb_with_mana_shield(resolved.health_delta, mana, shield);
        }
        let health_before = health.current;
        health.current -= resolved.health_delta;
        applied.write(DamageAppliedEvent {
            source: event.source,
            target: event.target,
            kind: event.kind,
            is_spell: event.is_spell,
            reactive: event.reactive,
            amount: resolved.health_delta.max(0.0) + resolved.shield_absorbed,
            health_before,
            health_after: health.current,
            max_health: health.max,
            target_pos: positions
                .get(event.target)
                .map_or(Vec3::ZERO, |tf| tf.translation),
        });

        // Leech: only health actually removed counts (no overkill, nothing a shield soaked).
        // Dots, reflection and cleave carry neither a drain ratio nor a leech-eligible kind.
        if let (Some(source), Some((_, lifesteal, spell_lifesteal))) =
            (event.source, source_snapshot)
        {
            let mut ratio = event.drain_ratio;
            if matches!(
                event.kind,
                DamageKind::BasicAttack | DamageKind::AttackBonus
            ) {
                ratio += lifesteal;
            }
            if event.is_spell {
                ratio += spell_lifesteal;
            }
            let dealt = resolved.health_delta.min(health_before.max(0.0));
            if source != event.target && ratio > 0.0 && dealt > 0.0 {
                leeches.push((source, dealt * ratio));
            }
        }

        if event.reflects && resolved.health_delta > 0.0 && damage_reflect > 0.0 {
            if let Some(source) = event.source {
                let amount = resolved.health_delta * damage_reflect;
                reflected.push(DamageEvent {
                    source: Some(event.target),
                    target: source,
                    amount,
                    damage_type: event.damage_type,
                    kind: DamageKind::Generic,
                    crit: false,
                    is_spell: false,
                    applies_block: true,
                    pierces_shields: false,
                    reflects: false,
                    drain_ratio: 0.0,
                    reactive: true,
                });
            }
        }
    }
    for (source, amount) in leeches {
        if let Ok((mut health, stats, _, statuses, _, _)) = unit_stats.p1().get_mut(source) {
            let outgoing = outgoing_heal_amp(Some(&stats), statuses.as_deref());
            let factor = heal_factor(HealKind::Leech, outgoing, Some(&stats), statuses.as_deref());
            apply_heal(&mut health, amount, factor);
        }
    }
    for event in reflected {
        damage.p1().write(event);
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
    /// The ability or item whose effect entries the impact resolves against.
    pub source: EffectSource,
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
    /// Skillshots only: the width the wave widens to by the end of its travel (`None` keeps
    /// `skillshot_width` the whole way).
    pub skillshot_end_width: Option<f32>,
    /// Skillshots only: the projectile ends once it has struck this many enemies.
    pub skillshot_max_hits: Option<u32>,
    pub splash_radius: f32,
    pub payload: ProjectilePayload,
}

pub const SPELL_PROJECTILE_SPEED: f32 = scale::u(34.0);

/// Piercing projectile: reports each enemy it passes once.
#[derive(Component, Debug, Clone, Default)]
pub struct Skillshot {
    /// Contact distance from the path at the start of the flight.
    pub width: f32,
    /// Where the flight began (ground plane).
    pub start: Vec3,
    /// Length of the flight; the width reaches `end_width` once the projectile has covered it.
    pub range: f32,
    /// Contact distance at the end of the flight (equal to `width` for a constant skillshot).
    pub end_width: f32,
    pub hit: Vec<Entity>,
    /// `Some(n)`: the projectile ends after striking `n` enemies (a hook uses 1).
    pub max_hits: Option<u32>,
}

impl Skillshot {
    /// Contact distance at `position`: `width` growing linearly to `end_width` over `range`.
    pub fn width_at(&self, position: Vec3) -> f32 {
        if self.range <= 0.0 || self.end_width == self.width {
            return self.width;
        }
        let travelled = flat_distance(self.start, position) / self.range;
        self.width + (self.end_width - self.width) * travelled.clamp(0.0, 1.0)
    }
}

pub fn skillshot_contacts(
    mut commands: Commands,
    mut hits: MessageWriter<ProjectileHitEvent>,
    mut projectiles: Query<(
        Entity,
        &Transform,
        &Projectile,
        &ProjectilePayload,
        &mut Skillshot,
    )>,
    units: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    statuses_of: Query<&StatusEffects>,
) {
    for (proj_entity, proj_tf, projectile, payload, mut skillshot) in &mut projectiles {
        let impact = proj_tf.translation;
        let width = skillshot.width_at(impact);
        let mut touched: Vec<(f32, Entity)> = units
            .iter()
            .filter(|(unit, unit_tf, team, health, bound)| {
                **team != projectile.team
                    && health.is_alive()
                    && !is_untargetable(&statuses_of, *unit)
                    && !skillshot.hit.contains(unit)
                    && area_contains(impact, width, unit_tf.translation, bounds_of(*bound))
            })
            .map(|(unit, unit_tf, ..)| (flat_distance(impact, unit_tf.translation), unit))
            .collect();
        // The nearest enemy is struck first, which matters when the projectile can only
        // strike a limited number of them.
        touched.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, unit) in touched {
            if skillshot
                .max_hits
                .is_some_and(|max| skillshot.hit.len() >= max as usize)
            {
                break;
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
        if skillshot
            .max_hits
            .is_some_and(|max| skillshot.hit.len() >= max as usize)
        {
            // It struck what it was after: no impact trigger follows.
            commands.entity(proj_entity).despawn();
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
    statuses_of: Query<&StatusEffects>,
) {
    for (proj_entity, proj_tf, projectile, payload, home, ground_impact, reached) in &projectiles {
        let impact = proj_tf.translation;
        let mut despawn = false;
        let mut primary_hit: Option<Entity> = None;

        if let Some(home) = home {
            // A target that is lifted out of play makes the projectile miss where it is.
            if is_untargetable(&statuses_of, home.target) {
                despawn = true;
            } else if let Ok((unit_entity, unit_tf, team, health, radius)) = units.get(home.target)
            {
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
                        && !is_untargetable(&statuses_of, *unit_entity)
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
        skillshot_end_width,
        skillshot_max_hits,
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
                start: origin,
                range: flat_distance(origin, target_pos),
                end_width: skillshot_end_width.unwrap_or(width),
                hit: Vec::new(),
                max_hits: skillshot_max_hits,
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
    use crate::components::AbilityId;
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
            source: EffectSource::Ability(AbilityId::Bolt),
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
    fn damage_applied_event_reports_shield_and_health() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<Messages<DamageAppliedEvent>>();
        world.init_resource::<Messages<crate::status::StatusLifecycleEvent>>();
        let stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        let pos = Vec3::new(3.0, 0.0, 4.0);
        let target = world
            .spawn((
                Health::new(500.0),
                stats,
                ShieldPool { amount: 30.0 },
                Transform::from_translation(pos),
            ))
            .id();
        let source = world.spawn_empty().id();
        world.write_message(
            DamageEvent::spell(source, target, 100.0, DamageType::Pure).reactive(true),
        );
        world.run_system_once(apply_damage_events).unwrap();

        let events: Vec<DamageAppliedEvent> = world
            .resource::<Messages<DamageAppliedEvent>>()
            .iter_current_update_messages()
            .copied()
            .collect();
        assert_eq!(events.len(), 1);
        let applied = events[0];
        assert_eq!((applied.source, applied.target), (Some(source), target));
        assert_eq!(applied.kind, DamageKind::Ability);
        assert!(applied.is_spell && applied.reactive);
        assert_eq!(applied.amount, 100.0, "70 health plus 30 absorbed");
        assert_eq!(applied.health_before, 500.0);
        assert_eq!(applied.health_after, 430.0);
        assert_eq!(applied.max_health, 500.0);
        assert_eq!(applied.target_pos, pos);
    }

    #[test]
    fn damage_events_apply_mitigation_and_skip_dead_targets() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<Messages<crate::combat::DamageAppliedEvent>>();
        world.init_resource::<Messages<crate::status::StatusLifecycleEvent>>();
        let stats = CombatStats::simple(0.0, 0.0, 1.0, 10.0, 0.0, 0.0);
        let alive = world.spawn((Health::new(500.0), stats)).id();
        let mut dead_hp = Health::new(500.0);
        dead_hp.current = 0.0;
        let dead = world.spawn((dead_hp, stats)).id();
        for target in [alive, dead] {
            world.write_message(DamageEvent::new(target, 100.0, DamageType::Physical));
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
        world.init_resource::<Messages<crate::combat::DamageAppliedEvent>>();
        world.init_resource::<Messages<crate::status::StatusLifecycleEvent>>();
        world.init_resource::<CombatRng>();
        let stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        let target = world
            .spawn((Health::new(500.0), stats, ShieldPool { amount: 120.0 }))
            .id();
        world.write_message(DamageEvent::new(target, 100.0, DamageType::Physical));
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
    fn crit_is_not_rolled_at_damage_application() {
        // Crit is decided in the attack-hit stage; the damage step only applies amplifiers.
        let mut world = World::new();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<Messages<crate::combat::DamageAppliedEvent>>();
        world.init_resource::<Messages<crate::status::StatusLifecycleEvent>>();
        let mut attacker_stats = CombatStats::simple(50.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        attacker_stats.crit_chance = 1.0;
        attacker_stats.crit_multiplier = 3.0;
        let defender = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        let attacker = world.spawn(attacker_stats).id();
        let target = world.spawn((Health::new(500.0), defender)).id();
        world.write_message(DamageEvent::basic_attack(attacker, target, 50.0));
        world.run_system_once(apply_damage_events).unwrap();
        let expected = 500.0 - apply_damage(50.0, DamageType::Physical, &defender);
        assert!((world.get::<Health>(target).unwrap().current - expected).abs() < 1e-3);
    }

    #[test]
    fn spell_amp_increases_magical_spell_damage() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageEvent>>();
        world.init_resource::<Messages<crate::combat::DamageAppliedEvent>>();
        world.init_resource::<Messages<crate::status::StatusLifecycleEvent>>();
        world.init_resource::<CombatRng>();
        let mut caster_stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        caster_stats.outgoing_spell_amp = 0.25;
        let defender = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
        let caster = world.spawn(caster_stats).id();
        let target = world.spawn((Health::new(500.0), defender)).id();
        world.write_message(DamageEvent::spell(
            caster,
            target,
            100.0,
            DamageType::Magical,
        ));
        world.run_system_once(apply_damage_events).unwrap();
        let expected = 500.0 - apply_damage(125.0, DamageType::Magical, &defender);
        assert!((world.get::<Health>(target).unwrap().current - expected).abs() < 1e-3);
    }

    #[test]
    fn in_flight_projectile_at_cycloned_unit_does_not_hit() {
        let mut world = World::new();
        world.init_resource::<Messages<ProjectileHitEvent>>();
        let mut cycloned = StatusEffects::default();
        cycloned
            .effects
            .push(crate::status::StatusEffect::cyclone(2.0, true));
        let target = world
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                Team::Dire,
                Health::new(500.0),
                cycloned,
            ))
            .id();
        let _cycloned_neighbour = world
            .spawn((
                Transform::from_xyz(30.0, 0.0, 0.0),
                Team::Dire,
                Health::new(500.0),
                {
                    let mut s = StatusEffects::default();
                    s.effects
                        .push(crate::status::StatusEffect::cyclone(2.0, true));
                    s
                },
            ))
            .id();
        let bystander = world
            .spawn((
                Transform::from_xyz(-30.0, 0.0, 0.0),
                Team::Dire,
                Health::new(500.0),
            ))
            .id();
        let caster = world
            .spawn((
                Transform::from_xyz(-500.0, 0.0, 0.0),
                Team::Radiant,
                Health::new(500.0),
            ))
            .id();
        let payload = ProjectilePayload {
            caster,
            source: EffectSource::Ability(AbilityId::Bolt),
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
        assert_eq!(
            hits[0].primary, None,
            "the projectile misses the cycloned unit"
        );
        assert_eq!(
            hits[0].splashed,
            vec![bystander],
            "splash skips cycloned units too"
        );
    }

    #[test]
    fn first_hit_skillshot_stops_on_first_enemy() {
        let payload = |caster| ProjectilePayload {
            caster,
            source: EffectSource::Ability(AbilityId::Bolt),
            rank: 1,
            cast_target: None,
            aim: Vec3::ZERO,
        };
        let run = |max_hits: Option<u32>| {
            let mut world = World::new();
            world.init_resource::<Messages<ProjectileHitEvent>>();
            let unit = |world: &mut World, team: Team, x: f32| {
                world
                    .spawn((Transform::from_xyz(x, 0.0, 0.0), team, Health::new(500.0)))
                    .id()
            };
            let caster = unit(&mut world, Team::Radiant, -500.0);
            let far = unit(&mut world, Team::Dire, 40.0);
            let near = unit(&mut world, Team::Dire, 10.0);
            let ally = unit(&mut world, Team::Radiant, 5.0);
            let bolt = world
                .spawn((
                    Transform::from_xyz(0.0, scale::body(1.0), 0.0),
                    Projectile {
                        speed: 1.0,
                        team: Team::Radiant,
                        radius: scale::body(0.85),
                        lifetime: 1.0,
                        splash_radius: 0.0,
                    },
                    payload(caster),
                    Skillshot {
                        width: 200.0,
                        start: Vec3::ZERO,
                        range: 0.0,
                        end_width: 200.0,
                        hit: Vec::new(),
                        max_hits,
                    },
                ))
                .id();
            world.run_system_once(skillshot_contacts).unwrap();
            let primaries: Vec<_> = world
                .resource::<Messages<ProjectileHitEvent>>()
                .iter_current_update_messages()
                .map(|hit| hit.primary.unwrap())
                .collect();
            (primaries, near, far, ally, world.get_entity(bolt).is_ok())
        };

        let (primaries, near, _, _, alive) = run(Some(1));
        assert_eq!(primaries, vec![near], "only the nearest enemy is struck");
        assert!(!alive, "the projectile ends at the first enemy");

        let (primaries, near, far, ally, alive) = run(None);
        assert_eq!(primaries, vec![near, far], "an ordinary skillshot pierces");
        assert!(!primaries.contains(&ally));
        assert!(alive);
    }

    #[test]
    fn expanding_skillshot_widens_with_distance_travelled() {
        let payload = |caster| ProjectilePayload {
            caster,
            source: EffectSource::Ability(AbilityId::Bolt),
            rank: 1,
            cast_target: None,
            aim: Vec3::ZERO,
        };
        // A wave from the origin that widens from 100 to 450 over 900 units. An enemy 300
        // to the side of the path is outside the starting width but inside the final one.
        let strikes = |expanding: bool, wave_x: f32| {
            let mut world = World::new();
            world.init_resource::<Messages<ProjectileHitEvent>>();
            let caster = world
                .spawn((
                    Transform::from_xyz(-500.0, 0.0, 0.0),
                    Team::Radiant,
                    Health::new(1.0),
                ))
                .id();
            world.spawn((
                Transform::from_xyz(wave_x, 0.0, 300.0),
                Team::Dire,
                Health::new(500.0),
            ));
            world.spawn((
                Transform::from_xyz(wave_x, scale::body(1.0), 0.0),
                Projectile {
                    speed: 1.0,
                    team: Team::Radiant,
                    radius: scale::body(0.85),
                    lifetime: 1.0,
                    splash_radius: 0.0,
                },
                payload(caster),
                Skillshot {
                    width: 100.0,
                    start: Vec3::ZERO,
                    range: 900.0,
                    end_width: if expanding { 450.0 } else { 100.0 },
                    hit: Vec::new(),
                    max_hits: None,
                },
            ));
            world.run_system_once(skillshot_contacts).unwrap();
            world
                .resource::<Messages<ProjectileHitEvent>>()
                .iter_current_update_messages()
                .count()
        };
        assert_eq!(strikes(true, 50.0), 0, "near the start the wave is narrow");
        assert_eq!(
            strikes(true, 850.0),
            1,
            "near the end the wave reaches 300 to the side"
        );
        assert_eq!(
            strikes(false, 850.0),
            0,
            "a constant-width skillshot never widens"
        );
        let wave = Skillshot {
            width: 100.0,
            start: Vec3::ZERO,
            range: 900.0,
            end_width: 450.0,
            ..Default::default()
        };
        assert!((wave.width_at(Vec3::new(450.0, 0.0, 0.0)) - 275.0).abs() < 1e-3);
        assert_eq!(
            wave.width_at(Vec3::new(5000.0, 0.0, 0.0)),
            450.0,
            "clamped at the end"
        );
    }

    #[test]
    fn skillshot_width_edge_cases() {
        let base = Skillshot {
            width: 100.0,
            start: Vec3::new(100.0, 0.0, 100.0),
            range: 0.0,
            end_width: 450.0,
            ..Default::default()
        };
        // A zero-length flight never divides by zero; it keeps the starting width.
        assert_eq!(base.width_at(Vec3::new(900.0, 0.0, 100.0)), 100.0);
        let wave = Skillshot {
            range: 800.0,
            ..base
        };
        assert_eq!(wave.width_at(wave.start), 100.0, "starting width at launch");
        // Behind the launch point is still distance 0..range, never narrower than the start.
        assert!(wave.width_at(Vec3::new(-300.0, 0.0, 100.0)) >= 100.0);
        // Height does not count towards distance travelled.
        let level = wave.width_at(Vec3::new(500.0, 0.0, 100.0));
        let raised = wave.width_at(Vec3::new(500.0, 400.0, 100.0));
        assert!((level - raised).abs() < 1e-3);
        assert!(
            (level - (100.0 + 350.0 * 0.5)).abs() < 1e-3,
            "halfway: {level}"
        );
    }

    mod leech {
        use super::*;
        use crate::status::StatusEffect;

        fn world() -> World {
            let mut world = World::new();
            world.init_resource::<Messages<DamageEvent>>();
            world.init_resource::<Messages<crate::combat::DamageAppliedEvent>>();
            world.init_resource::<Messages<crate::status::StatusLifecycleEvent>>();
            world.init_resource::<CombatRng>();
            world
        }

        fn plain() -> CombatStats {
            CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0)
        }

        /// A wounded attacker (500 / 1000) with the given lifesteal and spell lifesteal.
        fn attacker(world: &mut World, lifesteal: f32, spell_lifesteal: f32) -> Entity {
            let mut stats = plain();
            stats.lifesteal = lifesteal;
            stats.spell_lifesteal = spell_lifesteal;
            let mut health = Health::new(1000.0);
            health.current = 500.0;
            world.spawn((health, stats)).id()
        }

        fn victim(world: &mut World, hp: f32) -> Entity {
            world.spawn((Health::new(hp), plain())).id()
        }

        fn hp(world: &World, e: Entity) -> f32 {
            world.get::<Health>(e).unwrap().current
        }

        fn run(world: &mut World) {
            world.run_system_once(apply_damage_events).unwrap();
        }

        #[test]
        fn lifesteal_heals_fraction_of_mitigated_damage() {
            let mut world = world();
            let a = attacker(&mut world, 0.25, 0.0);
            let mut stats = plain();
            stats.armor = 10.0;
            let t = world.spawn((Health::new(1000.0), stats)).id();
            world.write_message(DamageEvent::basic_attack(a, t, 100.0));
            run(&mut world);
            let dealt = 1000.0 - hp(&world, t);
            assert!(dealt > 0.0 && dealt < 100.0, "armor mitigated the hit");
            assert!((hp(&world, a) - (500.0 + dealt * 0.25)).abs() < 1e-3);
        }

        #[test]
        fn lifesteal_capped_by_overkill_and_skips_shield_absorbed() {
            let mut world = world();
            let a = attacker(&mut world, 1.0, 0.0);
            let weak = victim(&mut world, 30.0);
            world.write_message(DamageEvent::basic_attack(a, weak, 500.0));
            run(&mut world);
            assert_eq!(hp(&world, a), 530.0, "only the 30 hp the target had");

            let shielded = world
                .spawn((Health::new(1000.0), plain(), ShieldPool { amount: 60.0 }))
                .id();
            world.write_message(DamageEvent::basic_attack(a, shielded, 100.0));
            run(&mut world);
            assert_eq!(hp(&world, shielded), 960.0);
            assert_eq!(
                hp(&world, a),
                570.0,
                "the 60 absorbed by the shield leeches nothing"
            );
        }

        #[test]
        fn cleave_reflect_and_dot_do_not_leech() {
            let mut world = world();
            let a = attacker(&mut world, 1.0, 1.0);
            let t = victim(&mut world, 1000.0);
            world.write_message(DamageEvent::cleave(a, t, 100.0));
            world.write_message(DamageEvent::splash(a, t, 100.0));
            world.write_message(DamageEvent::dot(Some(a), t, 100.0, DamageType::Physical));
            world.write_message(DamageEvent {
                kind: DamageKind::Generic,
                ..DamageEvent::new(t, 100.0, DamageType::Physical)
            });
            let reflected = DamageEvent {
                source: Some(a),
                reflects: false,
                ..DamageEvent::new(t, 100.0, DamageType::Physical)
            };
            world.write_message(reflected);
            run(&mut world);
            assert!(hp(&world, t) < 1000.0, "all of it landed");
            assert_eq!(hp(&world, a), 500.0, "none of it leeched");
        }

        #[test]
        fn attack_bonus_leeches_without_spell_amp_or_block() {
            let mut world = world();
            let a = attacker(&mut world, 0.5, 0.2);
            world.get_mut::<CombatStats>(a).unwrap().outgoing_spell_amp = 1.0;
            let t = world
                .spawn((Health::new(1000.0), {
                    let mut stats = plain();
                    stats.damage_block = 30.0;
                    stats
                }))
                .id();
            world.write_message(DamageEvent::attack_bonus(a, t, 100.0, DamageType::Magical));
            run(&mut world);
            assert_eq!(hp(&world, t), 900.0, "no spell amp (x2) and no block (-30)");
            assert!(
                (hp(&world, a) - 550.0).abs() < 1e-3,
                "lifesteal 0.5 of 100, not the spell lifesteal"
            );
        }

        #[test]
        fn spell_lifesteal_applies_to_ability_damage_only() {
            let mut spell_world = world();
            let a = attacker(&mut spell_world, 0.5, 0.2);
            let t = victim(&mut spell_world, 1000.0);
            spell_world.write_message(DamageEvent::spell(a, t, 100.0, DamageType::Pure));
            run(&mut spell_world);
            assert!(
                (hp(&spell_world, a) - 520.0).abs() < 1e-3,
                "spell lifesteal only, not the 50% attack lifesteal"
            );

            // A fresh world: each run of the system re-reads the unread messages.
            let mut attack_world = world();
            let a = attacker(&mut attack_world, 0.5, 0.2);
            let t = victim(&mut attack_world, 1000.0);
            attack_world.write_message(DamageEvent::basic_attack(a, t, 100.0));
            run(&mut attack_world);
            let dealt = 1000.0 - hp(&attack_world, t);
            assert!(
                (hp(&attack_world, a) - (500.0 + dealt * 0.5)).abs() < 1e-3,
                "basic attacks use lifesteal only"
            );
        }

        #[test]
        fn source_killed_in_same_batch_gets_no_leech() {
            let mut world = world();
            let a = attacker(&mut world, 1.0, 0.0);
            let t = victim(&mut world, 1000.0);
            let killer = victim(&mut world, 1000.0);
            world.write_message(DamageEvent::basic_attack(a, t, 100.0));
            world.write_message(DamageEvent::spell(killer, a, 9999.0, DamageType::Pure));
            run(&mut world);
            assert!(!world.get::<Health>(a).unwrap().is_alive());
        }

        #[test]
        fn leech_respects_heal_block_on_the_attacker() {
            let mut world = world();
            let a = attacker(&mut world, 1.0, 0.0);
            let mut blocked = StatusEffects::default();
            blocked.effects.push(StatusEffect::heal_block(5.0));
            world.entity_mut(a).insert(blocked);
            let t = victim(&mut world, 1000.0);
            world.write_message(DamageEvent::basic_attack(a, t, 100.0));
            run(&mut world);
            assert_eq!(hp(&world, a), 500.0);
        }

        #[test]
        fn timed_lifesteal_status_is_summed_with_permanent() {
            let mut world = world();
            let a = attacker(&mut world, 0.1, 0.0);
            let mut statuses = StatusEffects::default();
            statuses.effects.push(StatusEffect::resource_modifier(
                "vamp",
                crate::status::StatusKind::Buff,
                ResourceStat::Lifesteal,
                0.15,
                5.0,
            ));
            world.entity_mut(a).insert(statuses);
            let t = victim(&mut world, 1000.0);
            world.write_message(DamageEvent::basic_attack(a, t, 100.0));
            run(&mut world);
            let dealt = 1000.0 - hp(&world, t);
            assert!((hp(&world, a) - (500.0 + dealt * 0.25)).abs() < 1e-3);
        }
    }

    mod mana_shield {
        use super::*;
        use crate::status::StatusEffect;

        fn shielded(mana: f32, absorb: f32, per_mana: f32) -> (World, Entity) {
            let mut world = World::new();
            world.init_resource::<Messages<DamageEvent>>();
            world.init_resource::<Messages<crate::combat::DamageAppliedEvent>>();
            world.init_resource::<Messages<crate::status::StatusLifecycleEvent>>();
            world.init_resource::<CombatRng>();
            let mut statuses = StatusEffects::default();
            statuses.effects.push(StatusEffect::mana_shield(
                ManaShield {
                    absorb,
                    damage_per_mana: per_mana,
                },
                10.0,
            ));
            let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 10.0, 0.0, 0.0);
            stats.damage_block = 0.0;
            let target = world
                .spawn((Health::new(1000.0), stats, statuses, Mana::new(mana, 0.0)))
                .id();
            (world, target)
        }

        #[test]
        fn mana_shield_absorbs_post_mitigation_and_spends_mana() {
            let (mut world, t) = shielded(100.0, 0.5, 2.0);
            world.write_message(DamageEvent::new(t, 100.0, DamageType::Physical));
            world.run_system_once(apply_damage_events).unwrap();
            let armor = CombatStats::simple(0.0, 0.0, 1.0, 10.0, 0.0, 0.0);
            let mitigated = apply_damage(100.0, DamageType::Physical, &armor);
            assert!(mitigated < 100.0);
            let health = world.get::<Health>(t).unwrap().current;
            assert!((health - (1000.0 - mitigated * 0.5)).abs() < 1e-3);
            let mana = world.get::<Mana>(t).unwrap().current;
            assert!(
                (mana - (100.0 - mitigated * 0.5 / 2.0)).abs() < 1e-3,
                "mana pays for the mitigated damage at 2 damage per mana"
            );
        }

        #[test]
        fn mana_shield_runs_out_and_overflow_hits_health() {
            let (mut world, t) = shielded(10.0, 1.0, 2.0);
            world.write_message(DamageEvent::new(t, 100.0, DamageType::Pure));
            world.run_system_once(apply_damage_events).unwrap();
            assert_eq!(world.get::<Mana>(t).unwrap().current, 0.0);
            assert_eq!(
                world.get::<Health>(t).unwrap().current,
                1000.0 - (100.0 - 20.0),
                "10 mana absorbs 20 damage, the rest lands"
            );
            // No mana left: nothing is absorbed and the status stays.
            let mut again = World::new();
            again.init_resource::<Messages<DamageEvent>>();
            again.init_resource::<Messages<crate::combat::DamageAppliedEvent>>();
            again.init_resource::<Messages<crate::status::StatusLifecycleEvent>>();
            again.init_resource::<CombatRng>();
            let mut statuses = StatusEffects::default();
            statuses.effects.push(StatusEffect::mana_shield(
                ManaShield {
                    absorb: 1.0,
                    damage_per_mana: 2.0,
                },
                10.0,
            ));
            let t = again
                .spawn((
                    Health::new(1000.0),
                    CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0),
                    statuses,
                    Mana::new(0.0, 0.0),
                ))
                .id();
            again.write_message(DamageEvent::new(t, 50.0, DamageType::Pure));
            again.run_system_once(apply_damage_events).unwrap();
            assert_eq!(again.get::<Health>(t).unwrap().current, 950.0);
            assert_eq!(again.get::<StatusEffects>(t).unwrap().effects.len(), 1);
        }

        #[test]
        fn pool_absorbs_first_then_mana_shield_takes_the_rest() {
            let (mut world, t) = shielded(100.0, 1.0, 1.0);
            world.entity_mut(t).insert(ShieldPool { amount: 30.0 });
            world.get_mut::<CombatStats>(t).unwrap().armor = 0.0;
            world.write_message(DamageEvent::new(t, 100.0, DamageType::Pure));
            world.run_system_once(apply_damage_events).unwrap();
            assert_eq!(world.get::<ShieldPool>(t).unwrap().amount, 0.0);
            assert_eq!(world.get::<Mana>(t).unwrap().current, 30.0);
            assert_eq!(world.get::<Health>(t).unwrap().current, 1000.0);
        }

        #[test]
        fn absorbed_damage_does_not_leech_or_reflect() {
            let (mut world, t) = shielded(1000.0, 1.0, 1.0);
            let mut stats = CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 0.0);
            stats.lifesteal = 1.0;
            let mut hp = Health::new(1000.0);
            hp.current = 500.0;
            let attacker = world.spawn((hp, stats)).id();
            world.get_mut::<CombatStats>(t).unwrap().damage_reflect = 0.5;
            world.write_message(DamageEvent::basic_attack(attacker, t, 100.0));
            world.run_system_once(apply_damage_events).unwrap();
            assert_eq!(world.get::<Health>(attacker).unwrap().current, 500.0);
            assert_eq!(world.get::<Health>(t).unwrap().current, 1000.0);
        }
    }
}
