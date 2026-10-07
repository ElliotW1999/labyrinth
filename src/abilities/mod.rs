//! Hero abilities: data-driven definitions, the shared cast pipeline, reusable
//! effects, and the player's targeting UI.
//!
//! ```text
//! AbilityDefinition (catalog / AbilityDefinitions)   AbilityState (AbilityLoadout on the hero)
//!          └──────────────┬──────────────────────────────────┘
//! input / AI → AbilityCastRequest → validate → cast point → AbilityCastEvent
//!          → mechanics (dash, projectile, custom behavior, …)   — WHEN and WHO
//!          → AbilityTriggerEvent(TriggerContext)
//!          → matching effect entries                              — WHAT
//!          → DamageEvent / HealEvent / ManaRestoreEvent / StatusEffectEvent / DisplacementEvent
//! ```
//!
//! Items resolve their effect entries through the same trigger pipeline
//! (`EffectSource::Item`).

pub mod casting;
pub mod catalog;
pub mod custom;
pub mod definition;
pub mod effects;
pub mod generated;
pub mod mechanics;

use bevy::prelude::*;

use crate::combat::{flat_distance, spawn_requested_projectiles};
use crate::components::{
    AbilityLoadout, CastTarget, Ground, Lifetime, Mana, PlayerHero, SpellFx, Team,
};
use crate::items::StatusEffects;
use crate::picking::{cursor_ground_hit, cursor_ray, ground_hit, pick_under_ray, PickableUnits};
use crate::resources::SharedAssets;
use crate::scale;
use crate::unit_commands::{IssueCommand, UnitCommand};
use casting::{
    check_usable, resolve_queued_ability_casts, team_allows, tick_ability_casting,
    validate_cast_requests, AbilityCastEvent, AbilityCastRequest,
};
use catalog::{register_loadout_definitions, restore_ability_charges, AbilityDefinitions};
use definition::{TargetTeam, TargetType};
use crate::displacement::{apply_displacement_events, tick_forced_movement, DisplacementEvent};
use effects::{
    apply_heal_events, apply_mana_restore_events, apply_status_effect_events,
    resolve_ability_triggers, AbilityTriggerEvent, CustomAbilityEffects, HealEvent,
    ManaRestoreEvent, StatusEffectEvent,
};
use mechanics::{execute_ability_casts, projectile_hit_triggers, CustomAbilityBehaviors};

pub struct AbilitiesPlugin;

impl Plugin for AbilitiesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AbilityTargeting>()
            .init_resource::<AbilityDefinitions>()
            .init_resource::<CustomAbilityBehaviors>()
            .init_resource::<CustomAbilityEffects>()
            .add_message::<AbilityCastRequest>()
            .add_message::<AbilityCastEvent>()
            .add_message::<AbilityTriggerEvent>()
            .add_message::<HealEvent>()
            .add_message::<ManaRestoreEvent>()
            .add_message::<StatusEffectEvent>()
            .add_message::<DisplacementEvent>()
            .add_systems(
                Update,
                (
                    tick_ability_cooldowns,
                    regen_mana,
                    register_loadout_definitions,
                    restore_ability_charges,
                    begin_or_cast_from_hotkeys,
                    update_targeting_indicators,
                    confirm_or_cancel_targeted_cast,
                    resolve_queued_ability_casts,
                    validate_cast_requests,
                    tick_ability_casting,
                    execute_ability_casts,
                    projectile_hit_triggers,
                    resolve_ability_triggers,
                    spawn_requested_projectiles,
                    (
                        apply_heal_events,
                        apply_mana_restore_events,
                        apply_displacement_events,
                        apply_status_effect_events,
                        tick_forced_movement,
                    )
                        .chain(),
                    despawn_indicators,
                    animate_spell_fx,
                )
                    .chain()
                    .run_if(crate::net::is_sim_authority)
                    .after(crate::movement::SimSet::Commands)
                    .after(crate::combat::apply_projectile_hits)
                    .before(crate::combat::apply_damage_events),
            );
        custom::register(app);
    }
}

/// Active targeting mode for a targeted ability (E / R).
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct AbilityTargeting {
    pub active: Option<PendingTargetedCast>,
}

#[derive(Debug, Clone, Copy)]
pub struct PendingTargetedCast {
    pub slot: usize,
    pub cast_range: f32,
    /// AoE radius (area) or projectile width (point).
    pub aoe_radius: f32,
    /// When true, confirm requires a unit under the cursor.
    pub unit_only: bool,
    /// When true, show trajectory corridor instead of an AoE disc.
    pub point_target: bool,
    pub target_team: TargetTeam,
}

#[derive(Component)]
struct RangeIndicator;

#[derive(Component)]
struct AoeIndicator;

#[derive(Component)]
struct TrajectoryIndicator;

fn tick_ability_cooldowns(time: Res<Time>, mut query: Query<&mut AbilityLoadout>) {
    let dt = time.delta_secs();
    for mut loadout in &mut query {
        for slot in &mut loadout.slots {
            slot.cooldown_remaining = (slot.cooldown_remaining - dt).max(0.0);
        }
    }
}

fn regen_mana(time: Res<Time>, mut query: Query<&mut Mana>) {
    let dt = time.delta_secs();
    for mut mana in &mut query {
        mana.current = (mana.current + mana.regen_per_sec * dt).min(mana.max);
    }
}

/// QWER: no-target abilities issue a cast command immediately; targeted ones enter
/// targeting mode. Shift queues the cast behind the current command.
fn begin_or_cast_from_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    defs: Res<AbilityDefinitions>,
    mut issue: MessageWriter<IssueCommand>,
    hero: Query<(Entity, &Transform, &AbilityLoadout, &Mana, &StatusEffects), With<PlayerHero>>,
) {
    let Ok((hero_entity, transform, loadout, mana, statuses)) = hero.single() else {
        return;
    };

    if !statuses.can_cast() {
        return;
    }

    // Ctrl+QWER is reserved for spending skill points.
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        return;
    }

    let casts = [
        (KeyCode::KeyQ, 0usize),
        (KeyCode::KeyW, 1),
        (KeyCode::KeyE, 2),
        (KeyCode::KeyR, 3),
    ];

    for (key, index) in casts {
        if !keys.just_pressed(key) {
            continue;
        }

        // Switching hotkeys cancels the pending targeting mode. The current cast is
        // only replaced once the new command actually starts (see `unit_commands`).
        if targeting.active.is_some() {
            clear_targeting(&mut commands, &mut targeting);
        }

        let Some(slot) = loadout.slots.get(index) else {
            continue;
        };
        let Some(def) = defs.get(slot.id) else {
            continue;
        };
        if check_usable(def, slot, mana, Some(statuses)).is_err() {
            continue;
        }

        if def.target_type == TargetType::NoTarget {
            issue.write(IssueCommand {
                unit: hero_entity,
                command: UnitCommand::CastAbility {
                    slot: index,
                    target: CastTarget::None,
                },
                queue: crate::input::shift_held(&keys),
            });
            continue;
        }
        let pending = PendingTargetedCast {
            slot: index,
            cast_range: def.cast_range.at(slot.rank),
            aoe_radius: def.aoe_radius.at(slot.rank),
            unit_only: def.target_type == TargetType::Unit,
            point_target: def.target_type == TargetType::Point,
            target_team: def.target_team,
        };
        targeting.active = Some(pending);
        spawn_indicators(
            &mut commands,
            &assets,
            transform.translation,
            pending.cast_range,
            pending.aoe_radius,
            pending.point_target,
        );
    }
}

/// LMB confirms the pending targeted cast as a cast command (Shift queues it); range,
/// costs, and target rules are checked by `casting::validate_cast_requests`.
pub(crate) fn confirm_or_cancel_targeted_cast(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut targeting: ResMut<AbilityTargeting>,
    shop_ui: Res<crate::items::ShopUiState>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    mut commands: Commands,
    mut issue: MessageWriter<IssueCommand>,
    hero: Query<(Entity, &Team, &StatusEffects), With<PlayerHero>>,
    pickable: PickableUnits,
) {
    let Some(pending) = targeting.active else {
        return;
    };

    let cancel = keys.just_pressed(KeyCode::Escape)
        || keys.just_pressed(KeyCode::Space)
        || mouse.just_pressed(MouseButton::Right);
    if cancel {
        clear_targeting(&mut commands, &mut targeting);
        return;
    }

    // Don't confirm casts while the shop / detail UI is absorbing clicks.
    if shop_ui.open || shop_ui.detail.is_some() {
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok((hero_entity, team, statuses)) = hero.single() else {
        return;
    };

    if !statuses.can_cast() {
        clear_targeting(&mut commands, &mut targeting);
        return;
    }

    let Some(ray) = cursor_ray(&windows, &camera) else {
        return;
    };
    let Some(hit) = ground_hit(ray, &ground) else {
        return;
    };

    let caster_team = *team;
    let unit_target = pick_under_ray(ray, &pickable, |t| {
        team_allows(pending.target_team, caster_team, t)
    });
    if pending.unit_only && unit_target.is_none() {
        return;
    }

    clear_targeting(&mut commands, &mut targeting);
    issue.write(IssueCommand {
        unit: hero_entity,
        command: UnitCommand::CastAbility {
            slot: pending.slot,
            target: unit_target.map_or(CastTarget::Point(hit), CastTarget::Unit),
        },
        queue: crate::input::shift_held(&keys),
    });
}

fn update_targeting_indicators(
    targeting: Res<AbilityTargeting>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    hero: Query<&GlobalTransform, With<PlayerHero>>,
    mut range_q: Query<
        &mut Transform,
        (
            With<RangeIndicator>,
            Without<AoeIndicator>,
            Without<TrajectoryIndicator>,
            Without<PlayerHero>,
        ),
    >,
    mut aoe_q: Query<
        &mut Transform,
        (
            With<AoeIndicator>,
            Without<RangeIndicator>,
            Without<TrajectoryIndicator>,
            Without<PlayerHero>,
        ),
    >,
    mut traj_q: Query<
        &mut Transform,
        (
            With<TrajectoryIndicator>,
            Without<RangeIndicator>,
            Without<AoeIndicator>,
            Without<PlayerHero>,
        ),
    >,
) {
    let Some(pending) = targeting.active else {
        return;
    };
    let Ok(hero_gt) = hero.single() else {
        return;
    };
    let hero_pos = hero_gt.translation();
    let cursor = cursor_ground_hit(&windows, &camera, &ground).unwrap_or(hero_pos);

    for mut tf in &mut range_q {
        tf.translation = Vec3::new(hero_pos.x, scale::u(0.08), hero_pos.z);
        tf.scale = Vec3::new(pending.cast_range, 1.0, pending.cast_range);
    }

    let in_range = flat_distance(hero_pos, cursor) <= pending.cast_range;
    let aim = if in_range {
        cursor
    } else {
        let dir = cursor - hero_pos;
        let flat = Vec3::new(dir.x, 0.0, dir.z);
        if flat.length_squared() > 0.001 {
            hero_pos + flat.normalize() * pending.cast_range
        } else {
            hero_pos
        }
    };

    if pending.point_target {
        let flat = Vec3::new(aim.x - hero_pos.x, 0.0, aim.z - hero_pos.z);
        let length = flat.length().max(1.0);
        let mid = Vec3::new(
            (hero_pos.x + aim.x) * 0.5,
            scale::u(0.12),
            (hero_pos.z + aim.z) * 0.5,
        );
        let width = pending.aoe_radius.max(1.0);
        for mut tf in &mut traj_q {
            tf.translation = mid;
            tf.scale = Vec3::new(width, 1.0, length);
            if let Ok(dir) = Dir3::new(flat) {
                tf.look_to(dir, Vec3::Y);
            }
        }
    } else {
        for mut tf in &mut aoe_q {
            tf.translation = Vec3::new(aim.x, scale::u(0.1), aim.z);
            tf.scale = Vec3::new(pending.aoe_radius, 1.0, pending.aoe_radius);
        }
    }
}

fn spawn_indicators(
    commands: &mut Commands,
    assets: &SharedAssets,
    hero_pos: Vec3,
    cast_range: f32,
    radius_or_width: f32,
    point_target: bool,
) {
    commands.spawn((
        Name::new("Range Indicator"),
        RangeIndicator,
        Mesh3d(assets.indicator_ring_mesh.clone()),
        MeshMaterial3d(assets.indicator_range_mat.clone()),
        Transform::from_translation(Vec3::new(hero_pos.x, scale::u(0.08), hero_pos.z))
            .with_scale(Vec3::new(cast_range, 1.0, cast_range)),
    ));
    if point_target {
        commands.spawn((
            Name::new("Trajectory Indicator"),
            TrajectoryIndicator,
            Mesh3d(assets.indicator_beam_mesh.clone()),
            MeshMaterial3d(assets.indicator_aoe_mat.clone()),
            Transform::from_translation(Vec3::new(hero_pos.x, scale::u(0.12), hero_pos.z))
                .with_scale(Vec3::new(radius_or_width, 1.0, 1.0)),
        ));
    } else {
        commands.spawn((
            Name::new("AoE Indicator"),
            AoeIndicator,
            Mesh3d(assets.indicator_ring_mesh.clone()),
            MeshMaterial3d(assets.indicator_aoe_mat.clone()),
            Transform::from_translation(Vec3::new(hero_pos.x, scale::u(0.1), hero_pos.z))
                .with_scale(Vec3::new(radius_or_width, 1.0, radius_or_width)),
        ));
    }
}

pub fn clear_targeting(commands: &mut Commands, targeting: &mut AbilityTargeting) {
    targeting.active = None;
    commands.insert_resource(ClearIndicators);
}

#[derive(Resource, Default)]
struct ClearIndicators;

fn despawn_indicators(
    mut commands: Commands,
    clear: Option<Res<ClearIndicators>>,
    indicators: Query<
        Entity,
        Or<(
            With<RangeIndicator>,
            With<AoeIndicator>,
            With<TrajectoryIndicator>,
        )>,
    >,
) {
    if clear.is_none() {
        return;
    }
    for entity in &indicators {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<ClearIndicators>();
}

/// Called from input when the player issues move/attack/stop while targeting.
pub fn cancel_targeting_if_any(commands: &mut Commands, targeting: &mut AbilityTargeting) {
    if targeting.active.is_some() {
        clear_targeting(commands, targeting);
    }
}

pub(crate) fn spawn_dash_ghosts(
    commands: &mut Commands,
    assets: &SharedAssets,
    from: Vec3,
    to: Vec3,
) {
    for i in 1..5 {
        let t = i as f32 / 5.0;
        let pos = from.lerp(to, t) + Vec3::Y * scale::u(0.9);
        commands.spawn((
            Name::new("Dash Ghost"),
            Mesh3d(assets.hero_mesh.clone()),
            MeshMaterial3d(assets.dash_ghost_mat.clone()),
            // hero_mesh is already in world units — do not multiply by scale::u again.
            Transform::from_translation(pos).with_scale(Vec3::splat(0.9)),
            SpellFx {
                age: 0.0,
                lifetime: 0.35,
                start_scale: 0.9,
                end_scale: 0.2,
            },
            Lifetime(0.35),
        ));
    }
}

pub(crate) fn spawn_expanding_ring(
    commands: &mut Commands,
    assets: &SharedAssets,
    origin: Vec3,
    end_radius: f32,
    material: Handle<StandardMaterial>,
    lifetime: f32,
) {
    commands.spawn((
        Name::new("Spell Ring"),
        Mesh3d(assets.indicator_ring_mesh.clone()),
        MeshMaterial3d(material),
        Transform::from_translation(Vec3::new(origin.x, scale::u(0.12), origin.z))
            .with_scale(Vec3::new(0.4, 1.0, 0.4)),
        SpellFx {
            age: 0.0,
            lifetime,
            start_scale: 0.4,
            end_scale: end_radius,
        },
        Lifetime(lifetime),
    ));
}

fn animate_spell_fx(
    time: Res<Time>,
    mut query: Query<(Entity, &mut Transform, &mut SpellFx)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut fx) in &mut query {
        fx.age += dt;
        let t = (fx.age / fx.lifetime).clamp(0.0, 1.0);
        let scale = fx.start_scale + (fx.end_scale - fx.start_scale) * t;
        transform.scale = Vec3::new(scale, 1.0, scale);
        if fx.age >= fx.lifetime {
            commands.entity(entity).despawn();
        }
    }
}


#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::definition::{
        AbilityDefinition, AbilityEffect, AbilityMechanic, AbilityTrigger, AreaRadius,
        EffectScaling, EffectTarget, RankValue, StatusSpec,
    };
    use super::effects::{AbilityEffectAppExt, CustomEffectInput};
    use super::mechanics::{AbilityAppExt, CastContext};
    use super::*;
    use crate::combat::{
        apply_damage_events, DamageEvent, ProjectileHitEvent, ProjectilePayload,
        SpawnProjectileEvent,
    };
    use crate::components::{
        AbilityCasting, AbilityId, AttackTarget, CombatStats, DamageType, Health, MoveTarget,
        QueuedAbilityCast,
    };

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SharedAssets>()
            .init_resource::<AbilityDefinitions>()
            .init_resource::<CustomAbilityBehaviors>()
            .init_resource::<CustomAbilityEffects>()
            .add_message::<AbilityCastRequest>()
            .add_message::<AbilityCastEvent>()
            .add_message::<AbilityTriggerEvent>()
            .add_message::<HealEvent>()
            .add_message::<ManaRestoreEvent>()
            .add_message::<StatusEffectEvent>()
            .add_message::<DisplacementEvent>()
            .add_message::<DamageEvent>()
            .add_message::<SpawnProjectileEvent>()
            .add_message::<ProjectileHitEvent>()
            .add_systems(
                Update,
                (
                    register_loadout_definitions,
                    resolve_queued_ability_casts,
                    validate_cast_requests,
                    tick_ability_casting,
                    execute_ability_casts,
                    projectile_hit_triggers,
                    resolve_ability_triggers,
                    apply_heal_events,
                    apply_status_effect_events,
                    apply_damage_events,
                )
                    .chain(),
            );
        custom::register(&mut app);
        app
    }

    fn step(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(0.05));
        app.update();
    }

    fn spawn_unit(app: &mut App, team: Team, pos: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Transform::from_translation(pos),
                team,
                Health::new(1000.0),
                CombatStats::simple(0.0, 0.0, 1.0, 0.0, 0.0, 300.0),
                StatusEffects::default(),
            ))
            .id()
    }

    fn spawn_caster(app: &mut App, abilities: [AbilityId; 4]) -> Entity {
        let caster = spawn_unit(app, Team::Radiant, Vec3::ZERO);
        let mut loadout = AbilityLoadout::from_abilities(abilities);
        for slot in &mut loadout.slots {
            slot.rank = 1;
        }
        app.world_mut()
            .entity_mut(caster)
            .insert((loadout, Mana::new(1000.0, 0.0)));
        app.update();
        caster
    }

    fn request(app: &mut App, caster: Entity, slot: usize, target: CastTarget) {
        app.world_mut().write_message(AbilityCastRequest {
            caster,
            slot,
            target,
        });
    }

    const KIT: [AbilityId; 4] = [
        AbilityId::Shockwave,
        AbilityId::Bolt,
        AbilityId::Execute,
        AbilityId::Bulwark,
    ];

    #[test]
    fn no_target_cast_runs_effects_through_shared_events() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(5000.0, 0.0, 0.0));

        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        let world = app.world();
        assert_eq!(world.get::<Health>(near).unwrap().current, 1000.0, "cast point delays effects");
        for _ in 0..8 {
            step(&mut app);
        }
        let world = app.world();
        assert!(world.get::<Health>(near).unwrap().current < 1000.0);
        assert_eq!(world.get::<Health>(far).unwrap().current, 1000.0);
        let shockwave = catalog::builtin(AbilityId::Shockwave);
        assert_eq!(world.get::<Mana>(caster).unwrap().current, 1000.0 - shockwave.mana_cost.at(1));
        assert!(world.get::<AbilityLoadout>(caster).unwrap().slots[0].cooldown_remaining > 0.0);
        assert!(world.get::<StatusEffects>(caster).unwrap().can_push_units());
    }

    #[test]
    fn out_of_range_unit_cast_queues_an_approach_without_paying() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(5000.0, 0.0, 0.0));
        request(&mut app, caster, 1, CastTarget::Unit(enemy));
        step(&mut app);
        let world = app.world();
        assert!(world.get::<QueuedAbilityCast>(caster).is_some());
        assert!(world.get::<MoveTarget>(caster).is_some());
        assert!(world.get::<AbilityCasting>(caster).is_none());
        assert_eq!(world.get::<Mana>(caster).unwrap().current, 1000.0);

        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(50.0, 0.0, 0.0));
        app.world_mut().entity_mut(caster).remove::<QueuedAbilityCast>();
        request(&mut app, caster, 1, CastTarget::Unit(ally));
        step(&mut app);
        assert!(app.world().get::<AbilityCasting>(caster).is_none(), "allies are invalid Bolt targets");
    }

    fn hit(payload: ProjectilePayload, impact: Vec3, primary: Option<Entity>, splashed: Vec<Entity>) -> ProjectileHitEvent {
        ProjectileHitEvent {
            payload,
            team: Team::Radiant,
            impact,
            primary,
            splashed,
        }
    }

    fn health(app: &App, entity: Entity) -> f32 {
        app.world().get::<Health>(entity).unwrap().current
    }

    fn cast_and_collect_projectiles(app: &mut App, caster: Entity, slot: usize, target: Entity) -> Vec<SpawnProjectileEvent> {
        request(app, caster, slot, CastTarget::Unit(target));
        let mut spawned = Vec::new();
        for _ in 0..12 {
            step(app);
            let messages = app.world().resource::<Messages<SpawnProjectileEvent>>();
            spawned.extend(messages.iter_current_update_messages().copied());
        }
        spawned
    }

    #[test]
    fn execute_bonus_comes_from_effect_data() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let wounded = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let healthy = spawn_unit(&mut app, Team::Dire, Vec3::new(210.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(wounded).unwrap().current = 300.0;

        let spawned = cast_and_collect_projectiles(&mut app, caster, 2, wounded);
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].target, Some(wounded));
        assert!(app.world().get::<AttackTarget>(caster).is_some(), "mechanics still run");
        assert_eq!(health(&app, wounded), 300.0, "the projectile itself deals nothing");

        app.world_mut()
            .write_message(hit(spawned[0].payload, Vec3::X * 200.0, Some(wounded), vec![healthy]));
        step(&mut app);
        let base = catalog::EXECUTE_DAMAGE.at(1);
        assert!((health(&app, wounded) - (300.0 - base * catalog::EXECUTE_MULTIPLIER)).abs() < 1e-3);
        assert!((health(&app, healthy) - (1000.0 - base * 0.45)).abs() < 1e-3, "splash, no bonus");
    }

    #[test]
    fn projectile_landing_without_a_hit_splashes_full_damage() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let payload = ProjectilePayload {
            caster,
            ability: AbilityId::Bolt,
            rank: 1,
            cast_target: None,
            aim: Vec3::X * 200.0,
        };
        app.world_mut().write_message(hit(payload, Vec3::X * 200.0, None, vec![enemy]));
        step(&mut app);
        assert!((health(&app, enemy) - (1000.0 - 120.0)).abs() < 1e-3);
    }

    #[test]
    fn data_only_ability_resolves_on_hit_effects() {
        let mut app = test_app();
        let fireball = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(600.0), RankValue::ZERO)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .mechanic(AbilityMechanic::Projectile {
                splash_radius: AreaRadius::Ability,
                speed: None,
                skillshot: false,
            })
            .on(
                AbilityTrigger::OnProjectileHit,
                EffectTarget::TriggerUnit,
                AbilityEffect::Damage {
                    amount: RankValue::fixed(200.0),
                    damage_type: DamageType::Magical,
                    scaling: EffectScaling::None,
                },
            )
            .on(
                AbilityTrigger::OnProjectileHit,
                EffectTarget::TriggerUnit,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::Stun,
                    duration: RankValue::fixed(2.0),
                },
            );
        app.world_mut().resource_mut::<AbilityDefinitions>().insert(fireball);
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));

        let spawned = cast_and_collect_projectiles(&mut app, caster, 3, enemy);
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].payload.cast_target, Some(enemy));
        assert!(!app.world().get::<StatusEffects>(enemy).unwrap().is_stunned());

        app.world_mut()
            .write_message(hit(spawned[0].payload, Vec3::X * 300.0, Some(enemy), Vec::new()));
        step(&mut app);
        assert!(app.world().get::<StatusEffects>(enemy).unwrap().is_stunned());
        assert_eq!(health(&app, enemy), 800.0);
    }

    fn skewer_end(
        In(ctx): In<CastContext>,
        units: Query<(Entity, &Team)>,
        mut triggers: MessageWriter<AbilityTriggerEvent>,
    ) {
        let mut end = ctx.trigger(AbilityTrigger::Custom("on_skewer_end"), ctx.origin);
        end.affected_units = units
            .iter()
            .filter(|(_, team)| **team != ctx.team)
            .map(|(e, _)| e)
            .collect();
        triggers.write(AbilityTriggerEvent(end));
    }

    fn drain(In(input): In<CustomEffectInput>, mut heals: MessageWriter<HealEvent>) {
        for target in input.targets {
            heals.write(HealEvent {
                source: Some(input.ctx.caster),
                target,
                amount: input.value,
            });
        }
    }

    #[test]
    fn custom_mechanic_triggers_generic_and_custom_effects() {
        let mut app = test_app();
        app.register_ability_behavior(AbilityId::Bulwark, skewer_end)
            .register_custom_effect("drain", drain);
        let skewer = AbilityDefinition::new(AbilityId::Bulwark)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .on(
                AbilityTrigger::Custom("on_skewer_end"),
                EffectTarget::AffectedUnits,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::Stun,
                    duration: RankValue::fixed(1.0),
                },
            )
            .on(
                AbilityTrigger::Custom("on_skewer_end"),
                EffectTarget::Caster,
                AbilityEffect::Custom {
                    id: "drain",
                    value: RankValue::fixed(50.0),
                    amount: RankValue::ZERO,
                    duration: RankValue::ZERO,
                },
            );
        app.world_mut().resource_mut::<AbilityDefinitions>().insert(skewer);
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(4000.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(app.world().get::<StatusEffects>(enemy).unwrap().is_stunned());
        assert_eq!(health(&app, caster), 550.0);
    }
}
