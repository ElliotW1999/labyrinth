//! Hero abilities: data-driven definitions, the shared cast pipeline, reusable
//! effects, and the player's targeting UI.
//!
//! ```text
//! AbilityDefinition (catalog / AbilityDefinitions)   AbilityState (AbilityLoadout on the hero)
//!          └──────────────┬──────────────────────────────────┘
//! input / AI → ActivationRequest (shared `crate::actions` pipeline: validate → cast point
//!   → channel) → AbilityCastEvent
//!          → mechanics (dash, projectile, custom behavior, …)   — WHEN and WHO
//!          → AbilityTriggerEvent(TriggerContext)
//!          → matching effect entries                              — WHAT
//!          → DamageEvent / HealEvent / ManaRestoreEvent / StatusEffectEvent / DisplacementEvent
//! ```
//!
//! Items resolve their effect entries through the same trigger pipeline
//! (`EffectSource::Item`).

pub mod attack_triggers;
pub mod casting;
pub mod catalog;
pub mod custom;
pub mod definition;
pub mod effects;
pub mod generated;
pub mod mechanics;
pub mod orbits;
pub mod passive;
pub mod reactive_triggers;
pub mod tooltip;
pub mod zones;

use bevy::prelude::*;

use crate::actions::{
    ActionSource, ActionSources, ActionsPlugin, ActivationRequest, DisableRule, check_ready,
    refresh_activation_aims, resolve_queued_activations, team_allows, tick_activations,
    validate_activation_requests,
};
use crate::combat::{flat_distance, spawn_requested_projectiles};
use crate::components::{
    AbilityLoadout, CastTarget, Ground, Health, Lifetime, Mana, PlayerHero, SpellFx, Team,
};
use crate::displacement::{
    DisplacementEvent, apply_displacement_events, settle_grounded_units, tick_forced_movement,
};
use crate::picking::{PickableUnits, cursor_ground_hit, cursor_ray, ground_hit, pick_under_ray};
use crate::resources::SharedAssets;
use crate::scale;
use crate::status::{StatusEffectEvent, StatusEffects, apply_status_effect_events};
use crate::teleport::{
    TeleportCompletedEvent, TeleportEvent, TeleportRejectedEvent, apply_teleport_events,
};
use crate::unit_commands::{IssueCommand, UnitCommand};
use crate::vitals;
use casting::{AbilityCastEvent, action_channel_end_triggers, emit_ability_casts};
use catalog::{AbilityDefinitions, register_loadout_definitions, restore_ability_charges};
use definition::{TargetTeam, TargetType};
use effects::{
    AbilityTriggerEvent, CustomAbilityEffects, HealEvent, ManaRestoreEvent, apply_heal_events,
    apply_mana_restore_events, resolve_ability_triggers, source_params,
};
use mechanics::{
    CustomAbilityBehaviors, execute_ability_casts, execute_item_mechanics, projectile_hit_triggers,
};
use passive::{sync_ability_passives, tick_toggle_upkeep};

pub struct AbilitiesPlugin;

impl Plugin for AbilitiesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AbilityTargeting>()
            .init_resource::<AbilityDefinitions>()
            .init_resource::<CustomAbilityBehaviors>()
            .init_resource::<CustomAbilityEffects>()
            .add_message::<AbilityCastEvent>()
            .add_message::<AbilityTriggerEvent>()
            .add_message::<crate::abilities::effects::SpellHitEvent>()
            .add_message::<crate::abilities::effects::SpellReflectEvent>()
            .add_message::<zones::SpawnZoneEvent>()
            .add_message::<orbits::SpawnOrbitEvent>()
            .add_message::<crate::abilities::reactive_triggers::CooldownReadyEvent>()
            .add_message::<HealEvent>()
            .add_message::<ManaRestoreEvent>()
            .add_message::<vitals::ResourceDrainEvent>()
            .add_message::<StatusEffectEvent>()
            .add_message::<crate::status::StatusLifecycleEvent>()
            .add_message::<DisplacementEvent>()
            .add_message::<TeleportEvent>()
            .add_message::<TeleportCompletedEvent>()
            .add_message::<TeleportRejectedEvent>()
            .add_systems(
                Update,
                (
                    (
                        tick_ability_cooldowns,
                        crate::items::tick_item_cooldowns,
                        regen_mana,
                        tick_toggle_upkeep,
                        sync_ability_passives,
                    )
                        .chain(),
                    register_loadout_definitions,
                    restore_ability_charges,
                    begin_or_cast_from_hotkeys,
                    update_targeting_indicators,
                    confirm_or_cancel_targeted_cast,
                    resolve_queued_activations,
                    validate_activation_requests,
                    (refresh_activation_aims, tick_activations).chain(),
                    emit_ability_casts,
                    (
                        crate::items::resolve_item_uses,
                        crate::items::sync_item_attribute_toggles,
                        execute_item_mechanics,
                    )
                        .chain(),
                    execute_ability_casts,
                    action_channel_end_triggers,
                    projectile_hit_triggers,
                    // Before trigger resolution, so contact / end / reactive triggers fire this
                    // frame. The reactive dispatchers run last, in this order.
                    (
                        attack_triggers::attack_triggers,
                        crate::status::consume_attack_charges,
                        crate::status::break_invisibility,
                        (tick_forced_movement, settle_grounded_units).chain(),
                        reactive_triggers::track_travel,
                        reactive_triggers::proximity_triggers,
                        reactive_triggers::interval_triggers,
                        zones::tick_zones,
                        orbits::tick_orbits,
                        reactive_triggers::cooldown_triggers,
                        reactive_triggers::cast_triggers,
                        reactive_triggers::damage_triggers,
                        reactive_triggers::status_triggers
                            .after(crate::status::tick_status_effects),
                        reactive_triggers::sync_conditional_statuses,
                    )
                        .chain(),
                    resolve_ability_triggers,
                    (zones::spawn_requested_zones, orbits::spawn_requested_orbits).chain(),
                    spawn_requested_projectiles,
                    (
                        apply_heal_events,
                        apply_mana_restore_events,
                        vitals::apply_resource_drain_events,
                        apply_displacement_events,
                        apply_status_effect_events,
                        crate::vitals::sync_status_resource_caps,
                        crate::vitals::sync_status_attributes,
                        crate::vitals::sync_temporary_mana,
                        // Before the AI and navigation stages, so the orders it drops
                        // are gone before they are replanned or re-issued.
                        apply_teleport_events.before(crate::movement::SimSet::Ai),
                    )
                        .chain(),
                    (despawn_indicators, animate_spell_fx).chain(),
                )
                    .chain()
                    .run_if(crate::net::is_sim_authority)
                    .after(crate::movement::SimSet::Commands)
                    .after(crate::combat::apply_projectile_hits)
                    .before(crate::combat::apply_damage_events),
            );
        if !app.is_plugin_added::<ActionsPlugin>() {
            app.add_plugins(ActionsPlugin);
        }
        custom::register(app);
    }
}

/// Active targeting mode for a targeted ability or item.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct AbilityTargeting {
    pub active: Option<PendingTargetedCast>,
}

#[derive(Debug, Clone, Copy)]
pub struct PendingTargetedCast {
    pub source: ActionSource,
    pub cast_range: f32,
    /// AoE radius (area) or projectile width (point).
    pub aoe_radius: f32,
    /// When true, confirm requires a unit under the cursor.
    pub unit_only: bool,
    /// When true, show trajectory corridor instead of an AoE disc.
    pub point_target: bool,
    pub target_team: TargetTeam,
    pub disable: DisableRule,
}

#[derive(Component)]
struct RangeIndicator;

#[derive(Component)]
struct AoeIndicator;

#[derive(Component)]
struct TrajectoryIndicator;

fn tick_ability_cooldowns(
    time: Res<Time>,
    mut query: Query<(Entity, &mut AbilityLoadout)>,
    mut ready: MessageWriter<reactive_triggers::CooldownReadyEvent>,
) {
    let dt = time.delta_secs();
    for (owner, mut loadout) in &mut query {
        for slot in &mut loadout.slots {
            if slot.cooldown.tick(dt) {
                ready.write(reactive_triggers::CooldownReadyEvent {
                    owner,
                    source: effects::EffectSource::Ability(slot.id),
                });
            }
        }
    }
}

fn regen_mana(
    time: Res<Time>,
    mut query: Query<(&mut Mana, Option<&Health>, Option<&StatusEffects>)>,
) {
    let dt = time.delta_secs();
    for (mut mana, health, statuses) in &mut query {
        if health.is_some_and(|h| !h.is_alive()) {
            continue;
        }
        let regen = (mana.regen_per_sec
            + statuses
                .map(|s| s.resource_sum(crate::vitals::ResourceStat::ManaRegen))
                .unwrap_or(0.0))
        .max(0.0);
        mana.current = (mana.current + regen * dt).min(mana.max);
    }
}

/// Ability (QWER) and item (A S D Z X C) hotkeys, in slot order.
const ACTION_HOTKEYS: [(KeyCode, ActionSource); 10] = [
    (KeyCode::KeyQ, ActionSource::Ability(0)),
    (KeyCode::KeyW, ActionSource::Ability(1)),
    (KeyCode::KeyE, ActionSource::Ability(2)),
    (KeyCode::KeyR, ActionSource::Ability(3)),
    (KeyCode::KeyA, ActionSource::Item(0)),
    (KeyCode::KeyS, ActionSource::Item(1)),
    (KeyCode::KeyD, ActionSource::Item(2)),
    (KeyCode::KeyZ, ActionSource::Item(3)),
    (KeyCode::KeyX, ActionSource::Item(4)),
    (KeyCode::KeyC, ActionSource::Item(5)),
];

/// QWER / ASDZXC: no-target and self-target actions activate immediately; targeted ones
/// enter targeting mode. Shift queues the activation behind the current command. Actions
/// that don't occupy the caster (instant items) skip the command queue so current
/// orders keep running.
fn begin_or_cast_from_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    defs: Res<AbilityDefinitions>,
    sources: ActionSources,
    mut issue: MessageWriter<IssueCommand>,
    mut requests: MessageWriter<ActivationRequest>,
    hero: Query<(Entity, &Transform, &Mana, &StatusEffects), With<PlayerHero>>,
) {
    let Ok((hero_entity, transform, mana, statuses)) = hero.single() else {
        return;
    };

    // Ctrl+QWER is reserved for spending skill points.
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    for (key, source) in ACTION_HOTKEYS {
        if !keys.just_pressed(key) || (ctrl && matches!(source, ActionSource::Ability(_))) {
            continue;
        }
        // Switching hotkeys cancels the pending targeting mode. The current cast is
        // only replaced once the new command actually starts (see `unit_commands`).
        if targeting.active.is_some() {
            clear_targeting(&mut commands, &mut targeting);
        }
        let Ok(spec) = sources.spec(hero_entity, source) else {
            continue;
        };
        if spec.disable.blocks(Some(statuses)) {
            continue;
        }

        if check_ready(
            &spec.cooldown_state,
            &spec.cost,
            spec.disable,
            mana,
            Some(statuses),
        )
        .is_err()
        {
            continue;
        }

        if spec.target_type.is_instant() {
            if spec.occupies_caster {
                issue.write(IssueCommand {
                    unit: hero_entity,
                    command: UnitCommand::Activate {
                        source,
                        target: CastTarget::None,
                    },
                    queue: crate::input::shift_held(&keys),
                });
            } else {
                requests.write(ActivationRequest {
                    caster: hero_entity,
                    source,
                    target: CastTarget::None,
                });
            }
            continue;
        }
        let aoe_radius = source_params(spec.action.into(), &defs, spec.level)
            .map_or(0.0, |params| params.aoe_radius);
        let pending = PendingTargetedCast {
            source,
            cast_range: spec.cast_range,
            aoe_radius,
            unit_only: spec.target_type == TargetType::Unit,
            point_target: matches!(spec.target_type, TargetType::Point | TargetType::Direction),
            target_team: spec.target_team,
            disable: spec.disable,
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

/// LMB confirms the pending targeted cast as an activate command (Shift queues it);
/// range, costs, and target rules are checked by the shared activation pipeline.
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

    if pending.disable.blocks(Some(statuses)) {
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
        command: UnitCommand::Activate {
            source: pending.source,
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

/// Radius of the blue ring shown while an ability's HUD icon is hovered: the cast range
/// for abilities aimed at a unit, point or direction (for skillshots that is how far they
/// travel), or the effect radius for no-target / self-target abilities, which act around
/// the caster. Unlearned abilities preview their first rank. `None` when the ability has
/// no meaningful distance (passives without a radius, global casts).
pub fn hover_range_radius(def: &definition::AbilityDefinition, rank: u32) -> Option<f32> {
    let rank = rank.max(1);
    let cast_range = def.cast_range.at(rank);
    let aoe_radius = def.aoe_radius.at(rank);
    let radius = if def.target_type.is_instant() || cast_range <= 0.0 {
        aoe_radius
    } else {
        cast_range
    };
    (radius > 0.0 && !is_global_range(radius)).then_some(radius)
}

/// Whether a cast range is `cast_range=global` (the whole map).
fn is_global_range(cast_range: f32) -> bool {
    cast_range >= scale::GLOBAL_CAST_RANGE
}

fn spawn_indicators(
    commands: &mut Commands,
    assets: &SharedAssets,
    hero_pos: Vec3,
    cast_range: f32,
    radius_or_width: f32,
    point_target: bool,
) {
    // A global range covers the map: a ring that big is just noise, so it is not drawn.
    if !is_global_range(cast_range) {
        commands.spawn((
            Name::new("Range Indicator"),
            RangeIndicator,
            Mesh3d(assets.indicator_ring_mesh.clone()),
            MeshMaterial3d(assets.indicator_range_mat.clone()),
            Transform::from_translation(Vec3::new(hero_pos.x, scale::u(0.08), hero_pos.z))
                .with_scale(Vec3::new(cast_range, 1.0, cast_range)),
        ));
    }
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
        AbilityDefinition, AbilityEffect, AbilityEffectEntry, AbilityMechanic, AbilityTrigger,
        AreaRadius, DisplacementKind, EffectScaling, EffectTarget, RankValue, StatusSpec,
        TargetTeam,
    };
    use super::effects::{AbilityEffectAppExt, CustomEffectInput, EffectSource, TriggerContext};
    use super::mechanics::{AbilityAppExt, CastContext};
    use super::*;

    #[test]
    fn hover_range_uses_cast_range_for_aimed_and_radius_for_instant_abilities() {
        use crate::abilities::catalog::builtin;
        let strike = builtin(AbilityId::ShadowStrike);
        assert_eq!(
            hover_range_radius(&strike, 1),
            Some(strike.cast_range.at(1))
        );
        let wave = builtin(AbilityId::MagnusShockwave);
        assert_eq!(hover_range_radius(&wave, 2), Some(wave.cast_range.at(2)));
        // No-target AoE around the caster: the ring shows the effect radius.
        let scream = builtin(AbilityId::ScreamOfPain);
        assert!(scream.target_type.is_instant());
        assert_eq!(
            hover_range_radius(&scream, 1),
            Some(scream.aoe_radius.at(1))
        );
        // Unlearned abilities preview rank 1.
        assert_eq!(
            hover_range_radius(&strike, 0),
            hover_range_radius(&strike, 1)
        );
    }

    #[test]
    fn hover_range_skips_global_and_rangeless_abilities() {
        let mut def = definition::AbilityDefinition::new(AbilityId::ShadowStrike);
        assert_eq!(hover_range_radius(&def, 1), None);
        def.target_type = crate::actions::TargetType::Unit;
        def.cast_range = definition::RankValue::fixed(scale::GLOBAL_CAST_RANGE);
        assert_eq!(hover_range_radius(&def, 1), None);
    }
    use crate::actions::{
        ActionChannelEndEvent, ActionFiredEvent, Activation, ActivationRequest, QueuedActivation,
    };
    use crate::combat::{
        DamageEvent, ProjectileHitEvent, ProjectilePayload, SpawnProjectileEvent,
        apply_damage_events,
    };
    use crate::components::CollisionRadius;
    use crate::components::{
        AbilityId, AbilityState, AttackTarget, BoundRadius, CombatStats, DamageType, Health,
        MoveTarget,
    };
    use crate::displacement::ForcedMovement;
    use crate::items::{
        Inventory, ItemActivation, ItemDefinition, ItemId, ItemRarity, ItemType,
        install_test_item_definitions,
    };
    use crate::navigation::{NavGrid, NavShape};
    use crate::teleport::TeleportError;

    fn test_app() -> App {
        let mut app = App::new();
        app.edit_schedule(Update, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        app.init_resource::<Time>()
            .init_resource::<SharedAssets>()
            .init_resource::<AbilityDefinitions>()
            .init_resource::<crate::combat::CombatRng>()
            .init_resource::<CustomAbilityBehaviors>()
            .init_resource::<CustomAbilityEffects>()
            .add_message::<ActivationRequest>()
            .add_message::<ActionFiredEvent>()
            .add_message::<ActionChannelEndEvent>()
            .add_message::<AbilityCastEvent>()
            .add_message::<AbilityTriggerEvent>()
            .add_message::<crate::abilities::effects::SpellHitEvent>()
            .add_message::<crate::abilities::effects::SpellReflectEvent>()
            .add_message::<zones::SpawnZoneEvent>()
            .add_message::<orbits::SpawnOrbitEvent>()
            .add_message::<crate::abilities::reactive_triggers::CooldownReadyEvent>()
            .add_message::<HealEvent>()
            .add_message::<ManaRestoreEvent>()
            .add_message::<vitals::ResourceDrainEvent>()
            .add_message::<StatusEffectEvent>()
            .add_message::<crate::status::StatusLifecycleEvent>()
            .add_message::<DisplacementEvent>()
            .add_message::<TeleportEvent>()
            .add_message::<TeleportCompletedEvent>()
            .add_message::<TeleportRejectedEvent>()
            .init_resource::<NavGrid>()
            .add_message::<DamageEvent>()
            .add_message::<crate::combat::DamageAppliedEvent>()
            .add_message::<crate::status::StatusLifecycleEvent>()
            .add_message::<SpawnProjectileEvent>()
            .add_message::<ProjectileHitEvent>()
            .add_message::<crate::basic_attack::BasicAttackReleaseEvent>()
            .add_message::<crate::basic_attack::BasicAttackImpactEvent>()
            .add_message::<crate::basic_attack::BasicAttackLandedEvent>()
            .add_message::<crate::basic_attack::ExtraAttackEvent>()
            .add_systems(
                Update,
                (
                    register_loadout_definitions,
                    crate::items::tick_item_cooldowns,
                    resolve_queued_activations,
                    validate_activation_requests,
                    refresh_activation_aims,
                    tick_activations,
                    emit_ability_casts,
                    (
                        crate::items::resolve_item_uses,
                        crate::items::sync_item_attribute_toggles,
                        execute_item_mechanics,
                    )
                        .chain(),
                    execute_ability_casts,
                    action_channel_end_triggers,
                    projectile_hit_triggers,
                    (
                        attack_triggers::attack_triggers,
                        crate::status::consume_attack_charges,
                        crate::status::break_invisibility,
                        (tick_forced_movement, settle_grounded_units).chain(),
                        reactive_triggers::track_travel,
                        reactive_triggers::proximity_triggers,
                        reactive_triggers::interval_triggers,
                        zones::tick_zones,
                        orbits::tick_orbits,
                        reactive_triggers::cooldown_triggers,
                        reactive_triggers::cast_triggers,
                        reactive_triggers::damage_triggers,
                        reactive_triggers::status_triggers
                            .after(crate::status::tick_status_effects),
                        reactive_triggers::sync_conditional_statuses,
                    )
                        .chain(),
                    resolve_ability_triggers,
                    (zones::spawn_requested_zones, orbits::spawn_requested_orbits).chain(),
                    apply_heal_events,
                    (
                        apply_mana_restore_events,
                        crate::vitals::apply_resource_drain_events,
                    )
                        .chain(),
                    apply_displacement_events,
                    (
                        apply_status_effect_events,
                        crate::vitals::sync_status_resource_caps,
                        crate::vitals::sync_status_attributes,
                        crate::vitals::sync_temporary_mana,
                    )
                        .chain(),
                    apply_teleport_events,
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

    fn give_item(app: &mut App, caster: Entity, id: ItemId) {
        let mut inventory = app.world_mut().get_mut::<Inventory>(caster).unwrap();
        assert!(inventory.try_add(id), "inventory rejected {id:?}");
    }

    fn spawn_caster_with_inventory(app: &mut App, abilities: [AbilityId; 4]) -> Entity {
        let caster = spawn_caster(app, abilities);
        app.world_mut().entity_mut(caster).insert((
            Inventory::empty(),
            crate::components::HeroAttributes::starter(),
        ));
        caster
    }

    fn request_item(app: &mut App, caster: Entity, slot: usize, target: CastTarget) {
        app.world_mut()
            .write_message(ActivationRequest::item(caster, slot, target));
    }

    fn fixed_magical_damage(amount: f32) -> AbilityEffect {
        AbilityEffect::Damage {
            amount: RankValue::fixed(amount),
            damage_type: DamageType::Magical,
            scaling: EffectScaling::None,
        }
    }

    fn parity_damage_item(amount: f32) -> ItemDefinition {
        static EFFECTS: std::sync::OnceLock<&'static [AbilityEffectEntry]> =
            std::sync::OnceLock::new();
        let effects = EFFECTS.get_or_init(|| {
            Box::leak(Box::new([AbilityEffectEntry {
                trigger: AbilityTrigger::OnUse,
                target: EffectTarget::CastTarget,
                effect: fixed_magical_damage(amount),
            }]))
        });
        ItemDefinition {
            id: ItemId::HeartwoodBand,
            name: "Parity Damage",
            short_label: "PD",
            item_type: ItemType::Miscellaneous,
            cost: 1,
            sell_value: 0,
            tier: 1,
            rarity: ItemRarity::Common,
            max_stack: 1,
            consumable: false,
            max_charges: None,
            cooldown: 0.0,
            activation: ItemActivation {
                target_type: TargetType::Unit,
                target_team: TargetTeam::Enemy,
                cast_range: 500.0,
                cast_time: 0.0,
                channel_time: 0.0,
                mana_cost: 0.0,
            },
            attribute_toggle: None,
            components: &[],
            color: [1.0, 1.0, 1.0],
            description: "test",
            effects,
            mechanics: &[],
        }
    }

    fn attack_hit_heal_item() -> ItemDefinition {
        static EFFECTS: std::sync::OnceLock<&'static [AbilityEffectEntry]> =
            std::sync::OnceLock::new();
        let effects = EFFECTS.get_or_init(|| {
            Box::leak(Box::new([AbilityEffectEntry {
                trigger: AbilityTrigger::OnAttackHit,
                target: EffectTarget::Caster,
                effect: AbilityEffect::Heal {
                    amount: RankValue::fixed(50.0),
                    scaling: EffectScaling::None,
                },
            }]))
        });
        ItemDefinition {
            id: ItemId::HeartwoodBand,
            name: "Attack Heal",
            short_label: "AH",
            item_type: ItemType::Miscellaneous,
            cost: 1,
            sell_value: 0,
            tier: 1,
            rarity: ItemRarity::Common,
            max_stack: 1,
            consumable: false,
            max_charges: None,
            cooldown: 0.0,
            activation: ItemActivation {
                target_type: TargetType::NoTarget,
                target_team: TargetTeam::SelfOnly,
                cast_range: 0.0,
                cast_time: 0.0,
                channel_time: 0.0,
                mana_cost: 0.0,
            },
            attribute_toggle: None,
            components: &[],
            color: [1.0, 1.0, 1.0],
            description: "test",
            effects,
            mechanics: &[],
        }
    }

    fn item_with(
        effects: &'static [AbilityEffectEntry],
        target_type: TargetType,
        target_team: TargetTeam,
        cast_range: f32,
    ) -> ItemDefinition {
        ItemDefinition {
            activation: ItemActivation {
                target_type,
                target_team,
                cast_range,
                cast_time: 0.0,
                channel_time: 0.0,
                mana_cost: 0.0,
            },
            effects,
            ..attack_hit_heal_item()
        }
    }

    #[test]
    fn item_source_params_use_activation_range_and_fixed_radius() {
        let effects: &'static [AbilityEffectEntry] = Box::leak(Box::new([AbilityEffectEntry {
            trigger: AbilityTrigger::OnUse,
            target: EffectTarget::UnitsInRadius {
                center: super::definition::AreaCenter::Aim,
                radius: AreaRadius::Fixed(RankValue::fixed(143.0)),
                team: TargetTeam::Enemy,
            },
            effect: fixed_magical_damage(10.0),
        }]));
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::Point,
            TargetTeam::Enemy,
            800.0,
        )]);
        let defs = AbilityDefinitions::default();
        let params =
            super::effects::source_params(EffectSource::Item(ItemId::HeartwoodBand), &defs, 1)
                .expect("items always resolve");
        assert_eq!(params.aoe_radius, 143.0);
        assert_eq!(params.cast_range, 800.0);
        assert_eq!(params.entries.len(), 1);
    }

    #[test]
    fn item_distance_falloff_uses_the_item_cast_range() {
        let effects: &'static [AbilityEffectEntry] = Box::leak(Box::new([AbilityEffectEntry {
            trigger: AbilityTrigger::OnUse,
            target: EffectTarget::CastTarget,
            effect: AbilityEffect::Damage {
                amount: RankValue::fixed(100.0),
                damage_type: DamageType::Magical,
                scaling: EffectScaling::DistanceFromCaster { falloff: 0.5 },
            },
        }]));
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::Unit,
            TargetTeam::Enemy,
            600.0,
        )]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));

        request_item(&mut app, caster, 0, CastTarget::Unit(target));
        for _ in 0..4 {
            step(&mut app);
        }

        assert!(
            (health(&app, target) - 925.0).abs() < 1e-3,
            "300 / 600 of the range at falloff 0.5 leaves 75% of 100 damage, got {}",
            1000.0 - health(&app, target)
        );
    }

    fn write_attack_hit(app: &mut App, attacker: Entity, target: Entity) {
        app.world_mut()
            .write_message(crate::basic_attack::BasicAttackLandedEvent {
                attacker,
                attacker_team: Team::Radiant,
                target,
                damage: 10.0,
                crit: false,
                extra: false,
            });
    }

    /// An item whose only content is the given effect entries (see [`attack_hit_heal_item`]).
    fn item_with_effects(entries: Vec<AbilityEffectEntry>) -> ItemDefinition {
        ItemDefinition {
            effects: Box::leak(entries.into_boxed_slice()),
            ..attack_hit_heal_item()
        }
    }

    fn write_attack_release(app: &mut App, attacker: Entity, target: Entity) {
        app.world_mut()
            .write_message(crate::basic_attack::BasicAttackReleaseEvent {
                attacker,
                target,
                damage: 10.0,
                extra: false,
            });
    }

    fn cast_point_projectile() -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(600.0), RankValue::ZERO)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .timing(0.3, 0.0)
            .mechanic(AbilityMechanic::Projectile {
                splash_radius: AreaRadius::Fixed(RankValue::fixed(20.0)),
                speed: None,
                skillshot: false,
                first_hit: false,
                end_radius: None,
                origin_offset: None,
                seek: None,
            })
    }

    fn fired_events(app: &App) -> Vec<(CastTarget, Vec3)> {
        app.world()
            .resource::<Messages<ActionFiredEvent>>()
            .iter_current_update_messages()
            .map(|event| (event.target, event.aim))
            .collect()
    }

    #[test]
    fn unit_target_death_during_cast_point_still_fires_at_last_position() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(cast_point_projectile());
        let caster = spawn_caster(&mut app, KIT);
        let last_position = Vec3::new(200.0, 0.0, 0.0);
        let target = spawn_unit(&mut app, Team::Dire, last_position);

        request(&mut app, caster, 3, CastTarget::Unit(target));
        step(&mut app);
        assert!(!app.world().get::<Activation>(caster).unwrap().fired);
        app.world_mut().get_mut::<Health>(target).unwrap().current = 0.0;
        app.world_mut().entity_mut(target).despawn();

        let mut fired = Vec::new();
        let mut projectiles = Vec::new();
        for _ in 0..12 {
            step(&mut app);
            fired.extend(fired_events(&app));
            projectiles.extend(
                app.world()
                    .resource::<Messages<SpawnProjectileEvent>>()
                    .iter_current_update_messages()
                    .copied(),
            );
        }

        assert_eq!(
            fired,
            vec![(CastTarget::Point(last_position), last_position)]
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 900.0);
        let slot = &app.world().get::<AbilityLoadout>(caster).unwrap().slots[3];
        assert!(slot.cooldown.remaining > 0.0, "cooldown started");
        assert_eq!(projectiles.len(), 1);
        assert_eq!(projectiles[0].target, None);
        assert_eq!(projectiles[0].target_pos, last_position);
    }

    #[test]
    fn dead_but_not_despawned_target_also_fires_at_last_position() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(cast_point_projectile());
        let caster = spawn_caster(&mut app, KIT);
        let last_position = Vec3::new(200.0, 0.0, 0.0);
        let target = spawn_unit(&mut app, Team::Dire, last_position);

        request(&mut app, caster, 3, CastTarget::Unit(target));
        step(&mut app);
        app.world_mut().get_mut::<Health>(target).unwrap().current = 0.0;
        let mut fired = Vec::new();
        for _ in 0..12 {
            step(&mut app);
            fired.extend(fired_events(&app));
        }
        assert_eq!(
            fired,
            vec![(CastTarget::Point(last_position), last_position)]
        );
    }

    #[test]
    fn cast_point_aim_tracks_a_moving_unit_target() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(cast_point_projectile());
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::Unit(target));
        step(&mut app);
        let moved = Vec3::new(260.0, 0.0, 60.0);
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = moved;

        let mut fired = Vec::new();
        for _ in 0..12 {
            step(&mut app);
            fired.extend(fired_events(&app));
        }
        assert_eq!(fired, vec![(CastTarget::Unit(target), moved)]);
    }

    #[test]
    fn item_cooldown_ticks_before_the_activation_request_in_the_same_frame() {
        let mut app = test_app();
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::VialOfLight);
        app.world_mut().get_mut::<Inventory>(caster).unwrap().slots[0]
            .as_mut()
            .unwrap()
            .cooldown
            .remaining = 0.04;
        request_item(&mut app, caster, 0, CastTarget::None);
        step(&mut app); // dt 0.05: the tick must clear 0.04 before the request is validated
        let fired = app
            .world()
            .resource::<Messages<ActionFiredEvent>>()
            .iter_current_update_messages()
            .count();
        assert_eq!(fired, 1, "the request saw the freshly ticked cooldown");
    }

    #[test]
    fn overcharge_toggles_off_through_a_real_cast_and_removes_the_bonus() {
        let mut app = test_app();
        // The shared test app lists neither upkeep nor ability cooldowns; add them in order.
        app.add_systems(
            Update,
            (passive::tick_toggle_upkeep, tick_ability_cooldowns)
                .chain()
                .after(register_loadout_definitions)
                .before(validate_activation_requests),
        );
        let caster = spawn_caster(
            &mut app,
            [
                AbilityId::Overcharge,
                AbilityId::Bolt,
                AbilityId::Execute,
                AbilityId::Bulwark,
            ],
        );
        let base = *app.world().get::<CombatStats>(caster).unwrap();
        let toggled =
            |app: &App| app.world().get::<AbilityLoadout>(caster).unwrap().slots[0].toggled;
        let cooldown = |app: &App| {
            app.world().get::<AbilityLoadout>(caster).unwrap().slots[0]
                .cooldown
                .clone()
        };

        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(toggled(&app), "the first cast turns it on");
        let on = *app.world().get::<CombatStats>(caster).unwrap();
        assert!((on.move_speed - base.move_speed - 10.0).abs() < 1e-4);
        assert!((on.attack_speed_mult - base.attack_speed_mult - 0.2).abs() < 1e-4);
        assert!(
            cooldown(&app).remaining > 0.0,
            "each flip starts the cooldown"
        );
        let charges_on = cooldown(&app).charges;

        // Wait out the 1s cooldown, then cast again.
        for _ in 0..25 {
            step(&mut app);
        }
        assert_eq!(cooldown(&app).remaining, 0.0);
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(!toggled(&app), "the second cast turns it off");
        let off = *app.world().get::<CombatStats>(caster).unwrap();
        assert!(
            (off.move_speed - base.move_speed).abs() < 1e-4,
            "bonus removed"
        );
        assert!((off.attack_speed_mult - base.attack_speed_mult).abs() < 1e-4);
        assert!(
            app.world()
                .get::<StatusEffects>(caster)
                .unwrap()
                .effects
                .is_empty()
        );
        assert!(
            cooldown(&app).remaining > 0.0,
            "the off flip also starts the cooldown"
        );
        assert_eq!(cooldown(&app).charges, charges_on, "no charge anomaly");
        let mana = app.world().get::<Mana>(caster).unwrap().current;
        assert!(
            mana < 1000.0 - 2.0 * 15.0 + 1e-3,
            "two casts paid, got {mana}"
        );
    }

    #[test]
    fn item_cooldowns_tick_in_the_abilities_chain() {
        let mut app = test_app();
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::VialOfLight);
        request_item(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        let started = app.world().get::<Inventory>(caster).unwrap().slots[0]
            .unwrap()
            .cooldown
            .remaining;
        assert!(started > 0.0);
        for _ in 0..10 {
            step(&mut app);
        }
        let later = app.world().get::<Inventory>(caster).unwrap().slots[0]
            .unwrap()
            .cooldown
            .remaining;
        assert!(later < started - 0.4, "ticked from {started} to {later}");
    }

    #[test]
    fn ghost_scepter_makes_the_owner_ethereal_as_a_buff() {
        use crate::status::{StatusKind, TargetIntent};
        let mut app = test_app();
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::GhostScepter);
        request_item(&mut app, caster, 0, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        let statuses = app.world().get::<StatusEffects>(caster).unwrap();
        let ethereal = statuses
            .effects
            .iter()
            .find(|e| e.id == "ethereal")
            .expect("ethereal applied to the owner");
        assert_eq!(ethereal.kind, StatusKind::Buff);
        assert!(
            (ethereal.remaining - 4.0).abs() < 1e-3,
            "{}",
            ethereal.remaining
        );
        assert!(statuses.is_immune_to(DamageType::Physical));
        assert!(!statuses.is_immune_to(DamageType::Magical));
        assert!(!statuses.can_attack(), "ethereal units are disarmed");
        assert!(
            statuses.rejects(TargetIntent::Attack),
            "and cannot be attacked"
        );
        assert!((statuses.magic_damage_factor() - 1.3).abs() < 1e-5);
        let cooldown = app.world().get::<Inventory>(caster).unwrap().slots[0]
            .unwrap()
            .cooldown
            .remaining;
        assert!((cooldown - 22.0).abs() < 0.5, "{cooldown}");
    }

    /// A Soul Ring owner with the given vitals, ready to use slot 0.
    fn soul_ring_owner(health: f32, mana: (f32, f32)) -> (App, Entity) {
        let mut app = qop_app();
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::SoulRing);
        {
            let mut entity = app.world_mut().entity_mut(caster);
            *entity.get_mut::<Health>().unwrap() = Health {
                current: health,
                max: 500.0,
                regen_per_sec: 0.0,
            };
            *entity.get_mut::<Mana>().unwrap() = Mana {
                current: mana.0,
                max: mana.1,
                regen_per_sec: 0.0,
            };
        }
        (app, caster)
    }

    fn vitals_of(app: &App, caster: Entity) -> (f32, f32, f32) {
        let health = app.world().get::<Health>(caster).unwrap().current;
        let mana = app.world().get::<Mana>(caster).unwrap();
        (health, mana.current, mana.max)
    }

    #[test]
    fn soul_ring_spends_health_and_grants_temporary_mana_for_ten_seconds() {
        let (mut app, caster) = soul_ring_owner(500.0, (450.0, 500.0));
        request_item(&mut app, caster, 0, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        assert_eq!(vitals_of(&app, caster), (350.0, 600.0, 600.0));
        let cooldown = app.world().get::<Inventory>(caster).unwrap().slots[0]
            .unwrap()
            .cooldown
            .remaining;
        assert!((cooldown - 25.0).abs() < 0.5, "{cooldown}");
        // The bonus runs out after 10 seconds: unspent mana and the overflow go.
        for _ in 0..205 {
            step(&mut app);
        }
        assert_eq!(vitals_of(&app, caster), (350.0, 450.0, 500.0));
    }

    #[test]
    fn soul_ring_cannot_kill_but_still_grants_mana() {
        let (mut app, caster) = soul_ring_owner(100.0, (100.0, 500.0));
        request_item(&mut app, caster, 0, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        let (health, mana, max) = vitals_of(&app, caster);
        assert_eq!(health, 1.0, "the cost floors health at 1");
        assert_eq!((mana, max), (250.0, 500.0));
        assert!(app.world().get::<Health>(caster).unwrap().is_alive());
    }

    #[test]
    fn soul_ring_cost_is_not_damage() {
        let (mut app, caster) = soul_ring_owner(500.0, (450.0, 500.0));
        request_item(&mut app, caster, 0, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        let applied = app
            .world()
            .resource::<Messages<crate::combat::DamageAppliedEvent>>()
            .iter_current_update_messages()
            .count();
        assert_eq!(applied, 0, "spending health fires no damage facts");
    }

    #[test]
    fn item_on_attack_hit_resolves_in_the_same_frame() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([attack_hit_heal_item()]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;

        write_attack_hit(&mut app, caster, target);
        step(&mut app);

        assert_eq!(health(&app, caster), 550.0);
    }

    #[test]
    fn missed_attack_fires_on_attack_but_not_on_attack_hit() {
        let mut app = test_app();
        let heal = |trigger, amount| AbilityEffectEntry {
            trigger,
            target: EffectTarget::Caster,
            effect: AbilityEffect::Heal {
                amount: RankValue::fixed(amount),
                scaling: EffectScaling::None,
            },
        };
        let _defs = install_test_item_definitions([item_with_effects(vec![
            heal(AbilityTrigger::OnAttack, 10.0),
            heal(AbilityTrigger::OnAttackHit, 50.0),
        ])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;

        // A missed attack releases but never lands.
        write_attack_release(&mut app, caster, target);
        step(&mut app);
        assert_eq!(health(&app, caster), 510.0);

        write_attack_release(&mut app, caster, target);
        write_attack_hit(&mut app, caster, target);
        step(&mut app);
        assert_eq!(health(&app, caster), 570.0, "release 10 + landed 50");
    }

    #[test]
    fn on_attack_hit_damage_is_attack_bonus() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![AbilityEffectEntry {
            trigger: AbilityTrigger::OnAttackHit,
            target: EffectTarget::TriggerUnit,
            effect: fixed_magical_damage(100.0),
        }])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        {
            let mut stats = app.world_mut().get_mut::<CombatStats>(caster).unwrap();
            stats.outgoing_spell_amp = 1.0;
            stats.spell_lifesteal = 1.0;
            stats.lifesteal = 0.5;
        }
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;

        write_attack_hit(&mut app, caster, target);
        step(&mut app);

        assert_eq!(health(&app, target), 900.0, "no spell amp");
        assert_eq!(
            health(&app, caster),
            550.0,
            "lifesteal, not spell lifesteal"
        );
    }

    #[test]
    fn mana_break_burns_as_attack_bonus() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![AbilityEffectEntry {
            trigger: AbilityTrigger::OnAttackHit,
            target: EffectTarget::TriggerUnit,
            effect: AbilityEffect::DrainMana {
                amount: RankValue::fixed(50.0),
                burn_ratio: 1.0,
                damage_type: DamageType::Magical,
                transfer_ratio: 0.0,
            },
        }])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        app.world_mut()
            .get_mut::<CombatStats>(caster)
            .unwrap()
            .outgoing_spell_amp = 1.0;
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut()
            .entity_mut(target)
            .insert(Mana::new(200.0, 0.0));

        write_attack_hit(&mut app, caster, target);
        step(&mut app);

        assert_eq!(app.world().get::<Mana>(target).unwrap().current, 150.0);
        assert_eq!(
            health(&app, target),
            950.0,
            "burn is attack damage: no spell amp"
        );
    }

    #[test]
    fn ability_on_attack_hit_needs_a_learned_rank() {
        let mut app = test_app();
        app.world_mut().resource_mut::<AbilityDefinitions>().insert(
            AbilityDefinition::new(KIT[0]).on(
                AbilityTrigger::OnAttackHit,
                EffectTarget::Caster,
                AbilityEffect::Heal {
                    amount: RankValue::fixed(50.0),
                    scaling: EffectScaling::None,
                },
            ),
        );
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;

        app.world_mut()
            .get_mut::<AbilityLoadout>(caster)
            .unwrap()
            .slots[0]
            .rank = 0;
        write_attack_hit(&mut app, caster, target);
        step(&mut app);
        assert_eq!(
            health(&app, caster),
            500.0,
            "unlearned abilities never trigger"
        );

        app.world_mut()
            .get_mut::<AbilityLoadout>(caster)
            .unwrap()
            .slots[0]
            .rank = 1;
        write_attack_hit(&mut app, caster, target);
        step(&mut app);
        assert_eq!(health(&app, caster), 550.0);

        let ctx = app
            .world()
            .resource::<Messages<AbilityTriggerEvent>>()
            .iter_current_update_messages()
            .next()
            .unwrap()
            .0
            .clone();
        assert_eq!(ctx.source, EffectSource::Ability(KIT[0]));
        assert_eq!(ctx.rank, 1);
        assert_eq!(
            ctx.attack,
            Some(effects::AttackMeta {
                extra: false,
                crit: false
            })
        );
    }

    fn heal_entry(trigger: AbilityTrigger, amount: f32) -> AbilityEffectEntry {
        AbilityEffectEntry {
            trigger,
            target: EffectTarget::Caster,
            effect: AbilityEffect::Heal {
                amount: RankValue::fixed(amount),
                scaling: EffectScaling::None,
            },
        }
    }

    fn proc_entry(chance: f32, then: &'static str) -> AbilityEffectEntry {
        AbilityEffectEntry {
            trigger: AbilityTrigger::OnAttackHit,
            target: EffectTarget::Caster,
            effect: AbilityEffect::Proc {
                chance: RankValue::fixed(chance),
                then: AbilityTrigger::Custom(then),
                pseudo: false,
            },
        }
    }

    fn counter_entry(every: u32, then: &'static str) -> AbilityEffectEntry {
        AbilityEffectEntry {
            trigger: AbilityTrigger::OnAttackHit,
            target: EffectTarget::Caster,
            effect: AbilityEffect::Counter {
                every,
                then: AbilityTrigger::Custom(then),
            },
        }
    }

    /// Land `hits` attacks, one per frame.
    fn land_hits(app: &mut App, attacker: Entity, target: Entity, hits: usize) {
        for _ in 0..hits {
            write_attack_hit(app, attacker, target);
            step(app);
        }
    }

    fn attack_duel(app: &mut App) -> (Entity, Entity) {
        let caster = spawn_caster_with_inventory(app, KIT);
        give_item(app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 100.0;
        (caster, target)
    }

    #[test]
    fn proc_chance_one_always_and_zero_never() {
        for (chance, healed) in [(1.0_f32, 50.0_f32), (0.0, 0.0)] {
            let mut app = test_app();
            let _defs = install_test_item_definitions([item_with_effects(vec![
                proc_entry(chance, "on_bash"),
                heal_entry(AbilityTrigger::Custom("on_bash"), 1.0),
            ])]);
            let (caster, target) = attack_duel(&mut app);
            app.world_mut().get_mut::<Health>(caster).unwrap().current = 1.0;
            land_hits(&mut app, caster, target, 50);
            assert_eq!(health(&app, caster), 1.0 + healed, "chance {chance}");
        }
    }

    #[test]
    fn proc_rolls_once_for_all_derived_entries() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<crate::combat::CombatRng>()
            .seed(99);
        let _defs = install_test_item_definitions([item_with_effects(vec![
            proc_entry(0.5, "on_bash"),
            AbilityEffectEntry {
                trigger: AbilityTrigger::Custom("on_bash"),
                target: EffectTarget::TriggerUnit,
                effect: fixed_magical_damage(10.0),
            },
            heal_entry(AbilityTrigger::Custom("on_bash"), 10.0),
        ])]);
        let (caster, target) = attack_duel(&mut app);
        let (mut procs, mut dry) = (0, 0);
        for _ in 0..40 {
            app.world_mut().get_mut::<Health>(caster).unwrap().current = 100.0;
            app.world_mut().get_mut::<Health>(target).unwrap().current = 1000.0;
            land_hits(&mut app, caster, target, 1);
            let damaged = health(&app, target) < 1000.0;
            let healed = health(&app, caster) > 100.0;
            assert_eq!(damaged, healed, "one roll decides every derived entry");
            if damaged {
                procs += 1;
            } else {
                dry += 1;
            }
        }
        assert!(procs > 0 && dry > 0, "a 50% proc both fires and misses");
    }

    #[test]
    fn counter_fires_every_nth_landed_attack() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            counter_entry(3, "on_third"),
            heal_entry(AbilityTrigger::Custom("on_third"), 50.0),
        ])]);
        let (caster, target) = attack_duel(&mut app);
        land_hits(&mut app, caster, target, 2);
        assert_eq!(health(&app, caster), 100.0);
        land_hits(&mut app, caster, target, 1);
        assert_eq!(health(&app, caster), 150.0, "the third landed attack");
        land_hits(&mut app, caster, target, 3);
        assert_eq!(health(&app, caster), 200.0, "and the sixth");
    }

    #[test]
    fn counters_are_per_source() {
        let mut app = test_app();
        let every_two = item_with_effects(vec![
            counter_entry(2, "on_second"),
            heal_entry(AbilityTrigger::Custom("on_second"), 10.0),
        ]);
        let every_three = ItemDefinition {
            id: ItemId::SparkPendant,
            ..item_with_effects(vec![
                counter_entry(3, "on_third"),
                heal_entry(AbilityTrigger::Custom("on_third"), 100.0),
            ])
        };
        let _defs = install_test_item_definitions([every_two, every_three]);
        let (caster, target) = attack_duel(&mut app);
        give_item(&mut app, caster, ItemId::SparkPendant);
        land_hits(&mut app, caster, target, 6);
        // 3 x 10 from the first item, 2 x 100 from the second.
        assert_eq!(health(&app, caster), 100.0 + 30.0 + 200.0);
    }

    #[test]
    fn derived_trigger_depth_is_capped() {
        let mut app = test_app();
        let looping = |name: &'static str| AbilityEffectEntry {
            trigger: AbilityTrigger::Custom(name),
            target: EffectTarget::Caster,
            effect: AbilityEffect::Proc {
                chance: RankValue::fixed(1.0),
                then: AbilityTrigger::Custom(name),
                pseudo: false,
            },
        };
        let _defs = install_test_item_definitions([item_with_effects(vec![
            proc_entry(1.0, "on_loop"),
            looping("on_loop"),
            heal_entry(AbilityTrigger::Custom("on_loop"), 10.0),
        ])]);
        let (caster, target) = attack_duel(&mut app);
        land_hits(&mut app, caster, target, 1);
        // Depth 1 and depth 2 run their entries; the proc at depth 2 stops the chain.
        assert_eq!(health(&app, caster), 100.0 + 2.0 * 10.0);
    }

    fn extra_attack_events(app: &App) -> Vec<crate::basic_attack::ExtraAttackEvent> {
        app.world()
            .resource::<Messages<crate::basic_attack::ExtraAttackEvent>>()
            .iter_current_update_messages()
            .copied()
            .collect()
    }

    fn perform_attack(max_targets: u32, exclude_trigger_unit: bool) -> AbilityEffect {
        AbilityEffect::PerformAttack {
            damage_ratio: RankValue::fixed(0.7),
            max_targets: RankValue::fixed(max_targets as f32),
            exclude_trigger_unit,
        }
    }

    #[test]
    fn perform_attack_on_cast_is_an_instant_attack() {
        let ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                perform_attack(1, false),
            );
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::Unit(enemy));
        let mut seen = Vec::new();
        for _ in 0..6 {
            step(&mut app);
            seen.extend(extra_attack_events(&app));
        }
        assert_eq!(
            seen,
            vec![crate::basic_attack::ExtraAttackEvent {
                attacker: caster,
                target: enemy,
                damage_ratio: 0.7,
            }]
        );
    }

    #[test]
    fn perform_attack_max_targets_scales_with_rank() {
        let ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::NoTarget, RankValue::ZERO, RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::UnitsInRadius {
                    center: definition::AreaCenter::Caster,
                    radius: AreaRadius::Fixed(RankValue::fixed(600.0)),
                    team: TargetTeam::Enemy,
                },
                AbilityEffect::PerformAttack {
                    damage_ratio: RankValue::fixed(1.0),
                    max_targets: RankValue::levels(&[1.0, 2.0, 3.0]),
                    exclude_trigger_unit: false,
                },
            );
        for (rank, expected) in [(1, 1), (2, 2), (3, 3)] {
            let mut app = test_app();
            app.world_mut()
                .resource_mut::<AbilityDefinitions>()
                .insert(ability.clone());
            let caster = spawn_caster(&mut app, KIT);
            set_rank(&mut app, caster, 3, rank);
            for i in 0..4 {
                spawn_unit(
                    &mut app,
                    Team::Dire,
                    Vec3::new(100.0 + 50.0 * i as f32, 0.0, 0.0),
                );
            }
            request(&mut app, caster, 3, CastTarget::None);
            let mut count = 0;
            for _ in 0..6 {
                step(&mut app);
                count += extra_attack_events(&app).len();
            }
            assert_eq!(count, expected, "rank {rank}");
        }
    }

    #[test]
    fn double_attack_does_not_chain() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            proc_entry(1.0, "on_double"),
            AbilityEffectEntry {
                trigger: AbilityTrigger::Custom("on_double"),
                target: EffectTarget::TriggerUnit,
                effect: perform_attack(0, false),
            },
        ])]);
        let (caster, target) = attack_duel(&mut app);

        // A normal landed attack procs one extra attack...
        write_attack_hit(&mut app, caster, target);
        step(&mut app);
        assert_eq!(extra_attack_events(&app).len(), 1);

        // ...but an extra attack that lands runs the proc and performs nothing.
        app.world_mut()
            .write_message(crate::basic_attack::BasicAttackLandedEvent {
                attacker: caster,
                attacker_team: Team::Radiant,
                target,
                damage: 10.0,
                crit: false,
                extra: true,
            });
        step(&mut app);
        assert!(
            extra_attack_events(&app).is_empty(),
            "extra attacks never chain"
        );
    }

    #[test]
    fn multishot_excludes_primary_and_caps_targets() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![AbilityEffectEntry {
            trigger: AbilityTrigger::OnAttack,
            target: EffectTarget::UnitsInRadius {
                center: definition::AreaCenter::Caster,
                radius: AreaRadius::Fixed(RankValue::fixed(600.0)),
                team: TargetTeam::Enemy,
            },
            effect: perform_attack(2, true),
        }])]);
        let (caster, primary) = attack_duel(&mut app);
        let nearest = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let middle = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 200.0));
        let _far = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        let _out_of_radius = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 900.0));
        let _ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(10.0, 0.0, 0.0));

        write_attack_release(&mut app, caster, primary);
        step(&mut app);

        let targets: Vec<Entity> = extra_attack_events(&app)
            .iter()
            .map(|event| event.target)
            .collect();
        assert_eq!(
            targets,
            vec![nearest, middle],
            "nearest two, never the primary"
        );
        assert!(!targets.contains(&primary) && !targets.contains(&caster));
    }

    // --- End to end: the real attack pipeline feeding the ability layer ---

    /// `test_app` plus the real basic-attack systems, running before the attack triggers.
    fn attack_pipeline_app() -> App {
        use crate::basic_attack::*;
        let mut app = test_app();
        app.add_message::<BasicAttackEvent>().add_systems(
            Update,
            (
                request_basic_attacks,
                start_basic_attack_windups,
                tick_attack_swings,
                release_extra_attacks,
                resolve_attack_releases,
                advance_attacks_in_flight,
                resolve_basic_attack_impacts,
            )
                .chain()
                .before(attack_triggers::attack_triggers),
        );
        app
    }

    fn with_world_transform(app: &mut App, entity: Entity) {
        let tf = *app.world().get::<Transform>(entity).unwrap();
        app.world_mut()
            .entity_mut(entity)
            .insert(GlobalTransform::from(tf));
    }

    /// A melee attacker (50 damage) ordered onto a Dire target 80 units away.
    fn melee_attacker_and_target(app: &mut App, caster: Entity) -> Entity {
        let target = spawn_unit(app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        with_world_transform(app, target);
        app.world_mut().entity_mut(caster).insert((
            CombatStats::simple(50.0, crate::scale::MELEE_ATTACK_RANGE, 1.0, 0.0, 0.0, 300.0),
            Transform::default().looking_at(Vec3::new(80.0, 0.0, 0.0), Vec3::Y),
            crate::components::AttackCooldown(0.0),
            AttackTarget(target),
        ));
        with_world_transform(app, caster);
        target
    }

    #[derive(Default)]
    struct AttackLog {
        releases: Vec<crate::basic_attack::BasicAttackReleaseEvent>,
        landed: Vec<crate::basic_attack::BasicAttackLandedEvent>,
    }

    /// Let the first normal attack happen (then drop the order so no second one starts), and
    /// keep running long enough for every consequence to play out.
    fn run_one_normal_attack(app: &mut App, caster: Entity) -> AttackLog {
        use crate::basic_attack::{BasicAttackLandedEvent, BasicAttackReleaseEvent};
        let mut log = AttackLog::default();
        let mut ordered = true;
        for _ in 0..40 {
            step(app);
            let releases: Vec<_> = app
                .world()
                .resource::<Messages<BasicAttackReleaseEvent>>()
                .iter_current_update_messages()
                .copied()
                .collect();
            if ordered && releases.iter().any(|r| !r.extra) {
                app.world_mut().entity_mut(caster).remove::<AttackTarget>();
                ordered = false;
            }
            log.releases.extend(releases);
            log.landed.extend(
                app.world()
                    .resource::<Messages<BasicAttackLandedEvent>>()
                    .iter_current_update_messages()
                    .copied(),
            );
        }
        assert!(!ordered, "the normal attack was released");
        log
    }

    fn proc_then_perform_attack(ratio: f32) -> Vec<AbilityEffectEntry> {
        vec![
            proc_entry(1.0, "on_double"),
            AbilityEffectEntry {
                trigger: AbilityTrigger::Custom("on_double"),
                target: EffectTarget::TriggerUnit,
                effect: AbilityEffect::PerformAttack {
                    damage_ratio: RankValue::fixed(ratio),
                    max_targets: RankValue::ZERO,
                    exclude_trigger_unit: false,
                },
            },
        ]
    }

    fn assert_one_normal_and_one_extra(app: &App, log: &AttackLog, target: Entity, ratio: f32) {
        let normal: Vec<_> = log.landed.iter().filter(|l| !l.extra).collect();
        let extra: Vec<_> = log.landed.iter().filter(|l| l.extra).collect();
        assert_eq!(
            normal.len(),
            1,
            "one normal attack landed: {:?}",
            log.landed
        );
        assert_eq!(
            extra.len(),
            1,
            "exactly one extra attack, and it did not chain: {:?}",
            log.landed
        );
        assert_eq!(log.releases.iter().filter(|r| r.extra).count(), 1);
        assert_eq!(log.releases.iter().filter(|r| !r.extra).count(), 1);
        assert_eq!(extra[0].target, target);
        assert!((extra[0].damage - 50.0 * ratio).abs() < 1e-3);
        let expected = 1000.0 - 50.0 - 50.0 * ratio;
        assert!(
            (health(app, target) - expected).abs() < 1e-3,
            "two hits and no more, got {}",
            health(app, target)
        );
    }

    #[test]
    fn item_proc_perform_attack_lands_one_extra_attack_through_the_real_pipeline() {
        let mut app = attack_pipeline_app();
        let _defs =
            install_test_item_definitions([item_with_effects(proc_then_perform_attack(0.5))]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = melee_attacker_and_target(&mut app, caster);

        let log = run_one_normal_attack(&mut app, caster);

        assert_one_normal_and_one_extra(&app, &log, target, 0.5);
    }

    #[test]
    fn ability_proc_perform_attack_lands_one_extra_attack_through_the_real_pipeline() {
        let mut app = attack_pipeline_app();
        let mut ability = AbilityDefinition::new(KIT[0]);
        for entry in proc_then_perform_attack(1.0) {
            ability = ability.on(entry.trigger, entry.target, entry.effect);
        }
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        let target = melee_attacker_and_target(&mut app, caster);

        let log = run_one_normal_attack(&mut app, caster);

        assert_one_normal_and_one_extra(&app, &log, target, 1.0);
    }

    #[test]
    fn extra_attack_leaves_cooldown_swing_and_order_alone_and_does_not_chain_with_cleave() {
        let mut app = attack_pipeline_app();
        let _defs =
            install_test_item_definitions([item_with_effects(proc_then_perform_attack(1.0))]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = melee_attacker_and_target(&mut app, caster);
        // A bystander beside the target gets cleave damage, which must fire nothing.
        let bystander = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 100.0));
        with_world_transform(&mut app, bystander);
        app.world_mut()
            .get_mut::<CombatStats>(caster)
            .unwrap()
            .cleaves
            .adjust(
                crate::components::CleaveSource::Items,
                0.5,
                0.0,
                0.0,
                0.0,
                1,
            );

        let log = run_one_normal_attack(&mut app, caster);

        assert_eq!(
            log.landed.len(),
            2,
            "cleave is not an attack: only the normal and the extra attack land"
        );
        // Each of the two main hits cleaves 50% of 50.
        assert!(
            (health(&app, bystander) - (1000.0 - 2.0 * 25.0)).abs() < 1e-3,
            "cleave from both hits, no other damage, got {}",
            health(&app, bystander)
        );
        assert!((health(&app, target) - 900.0).abs() < 1e-3);
        assert!(
            app.world().get::<AttackTarget>(caster).is_none(),
            "an extra attack never re-issues the attack order"
        );
    }

    #[test]
    fn two_copies_of_one_item_fire_one_attack_trigger() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([attack_hit_heal_item()]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;

        write_attack_hit(&mut app, caster, target);
        step(&mut app);

        assert_eq!(health(&app, caster), 550.0, "one trigger, not one per copy");
    }

    #[test]
    fn item_attack_trigger_keeps_the_attack_point_aim_and_origin() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([attack_hit_heal_item()]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        app.world_mut()
            .get_mut::<Transform>(caster)
            .unwrap()
            .translation = Vec3::new(10.0, 0.0, 5.0);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target_at = Vec3::new(80.0, 0.0, 40.0);
        let target = spawn_unit(&mut app, Team::Dire, target_at);

        write_attack_hit(&mut app, caster, target);
        step(&mut app);

        let contexts: Vec<_> = app
            .world()
            .resource::<Messages<AbilityTriggerEvent>>()
            .iter_current_update_messages()
            .map(|event| event.0.clone())
            .collect();
        assert_eq!(contexts.len(), 1);
        let ctx = &contexts[0];
        assert_eq!(ctx.trigger, AbilityTrigger::OnAttackHit);
        assert_eq!(ctx.source, EffectSource::Item(ItemId::HeartwoodBand));
        assert_eq!(ctx.rank, 1);
        assert_eq!(ctx.origin, Vec3::new(10.0, 0.0, 5.0));
        assert_eq!(ctx.aim, target_at);
        assert_eq!(ctx.point, target_at);
        assert_eq!(ctx.trigger_unit, Some(target));
        assert_eq!(ctx.cast_target, None);
        assert!(ctx.affected_units.is_empty());
    }

    #[test]
    fn cast_point_target_leaving_range_still_fires_and_aim_follows() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(cast_point_projectile());
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::Unit(target));
        step(&mut app);
        let far = Vec3::new(5000.0, 0.0, 0.0);
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = far;
        let mut fired = Vec::new();
        for _ in 0..12 {
            step(&mut app);
            fired.extend(fired_events(&app));
        }
        // Range is only checked when the request is accepted, not again at the cast point.
        assert_eq!(fired, vec![(CastTarget::Unit(target), far)]);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 900.0);
    }

    #[test]
    fn queued_unit_cast_is_dropped_when_the_target_dies_before_it_is_in_range() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(cast_point_projectile());
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(3000.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::Unit(target));
        step(&mut app);
        assert!(
            app.world()
                .get::<crate::actions::QueuedActivation>(caster)
                .is_some(),
            "out of range: the cast walks first"
        );
        app.world_mut().entity_mut(target).despawn();
        let mut fired = Vec::new();
        for _ in 0..6 {
            step(&mut app);
            fired.extend(fired_events(&app));
        }
        assert!(fired.is_empty());
        assert!(
            app.world()
                .get::<crate::actions::QueuedActivation>(caster)
                .is_none()
        );
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 1000.0);
    }

    fn request(app: &mut App, caster: Entity, slot: usize, target: CastTarget) {
        app.world_mut()
            .write_message(ActivationRequest::ability(caster, slot, target));
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
        assert_eq!(
            world.get::<Health>(near).unwrap().current,
            1000.0,
            "cast point delays effects"
        );
        for _ in 0..8 {
            step(&mut app);
        }
        let world = app.world();
        assert!(world.get::<Health>(near).unwrap().current < 1000.0);
        assert_eq!(world.get::<Health>(far).unwrap().current, 1000.0);
        let shockwave = catalog::builtin(AbilityId::Shockwave);
        assert_eq!(
            world.get::<Mana>(caster).unwrap().current,
            1000.0 - shockwave.mana_cost.at(1)
        );
        assert!(
            world.get::<AbilityLoadout>(caster).unwrap().slots[0]
                .cooldown
                .remaining
                > 0.0
        );
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
        assert!(world.get::<QueuedActivation>(caster).is_some());
        assert!(world.get::<MoveTarget>(caster).is_some());
        assert!(world.get::<Activation>(caster).is_none());
        assert_eq!(world.get::<Mana>(caster).unwrap().current, 1000.0);

        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(50.0, 0.0, 0.0));
        app.world_mut()
            .entity_mut(caster)
            .remove::<QueuedActivation>();
        request(&mut app, caster, 1, CastTarget::Unit(ally));
        step(&mut app);
        assert!(
            app.world().get::<Activation>(caster).is_none(),
            "allies are invalid Bolt targets"
        );
    }

    fn hit(
        payload: ProjectilePayload,
        impact: Vec3,
        primary: Option<Entity>,
        splashed: Vec<Entity>,
    ) -> ProjectileHitEvent {
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

    #[test]
    fn heal_event_does_not_revive_dead_units() {
        let mut app = test_app();
        let dead = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO);
        let alive = spawn_unit(&mut app, Team::Radiant, Vec3::X);
        app.world_mut().get_mut::<Health>(dead).unwrap().current = 0.0;
        app.world_mut().get_mut::<Health>(alive).unwrap().current = 100.0;
        for target in [dead, alive] {
            app.world_mut().write_message(HealEvent {
                source: None,
                target,
                amount: 250.0,
            });
        }
        app.update();
        assert_eq!(health(&app, dead), 0.0);
        assert_eq!(health(&app, alive), 350.0);
    }

    #[test]
    fn timed_mana_regen_modifier_adds_and_expires() {
        use crate::vitals::ResourceStat;
        let mut app = test_app();
        let unit = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO);
        app.world_mut().entity_mut(unit).insert(Mana {
            current: 0.0,
            max: 1000.0,
            regen_per_sec: 1.0,
        });
        app.world_mut()
            .get_mut::<StatusEffects>(unit)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::resource_modifier(
                "mana_buff",
                crate::status::StatusKind::Buff,
                ResourceStat::ManaRegen,
                4.0,
                5.0,
            ));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(1));
        bevy::ecs::system::RunSystemOnce::run_system_once(app.world_mut(), regen_mana).unwrap();
        assert_eq!(app.world().get::<Mana>(unit).unwrap().current, 5.0);
        app.world_mut()
            .get_mut::<StatusEffects>(unit)
            .unwrap()
            .effects
            .clear();
        bevy::ecs::system::RunSystemOnce::run_system_once(app.world_mut(), regen_mana).unwrap();
        assert_eq!(app.world().get::<Mana>(unit).unwrap().current, 6.0);
    }

    #[test]
    fn dead_units_do_not_regenerate_mana() {
        let mut app = test_app();
        let unit = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO);
        app.world_mut().get_mut::<Health>(unit).unwrap().current = 0.0;
        app.world_mut().entity_mut(unit).insert(Mana {
            current: 0.0,
            max: 100.0,
            regen_per_sec: 10.0,
        });
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(1));
        bevy::ecs::system::RunSystemOnce::run_system_once(app.world_mut(), regen_mana).unwrap();
        assert_eq!(app.world().get::<Mana>(unit).unwrap().current, 0.0);
    }

    fn cast_and_collect_projectiles(
        app: &mut App,
        caster: Entity,
        slot: usize,
        target: Entity,
    ) -> Vec<SpawnProjectileEvent> {
        request(app, caster, slot, CastTarget::Unit(target));
        let mut spawned = Vec::new();
        for _ in 0..12 {
            step(app);
            let messages = app.world().resource::<Messages<SpawnProjectileEvent>>();
            spawned.extend(messages.iter_current_update_messages().copied());
        }
        spawned
    }

    fn scream_ability() -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(
                TargetType::NoTarget,
                RankValue::ZERO,
                RankValue::fixed(600.0),
            )
            .team(TargetTeam::Enemy)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .mechanic(AbilityMechanic::ProjectileToEach {
                center: super::definition::AreaCenter::Caster,
                team: TargetTeam::Enemy,
                speed: Some(900.0),
            })
            .on(
                AbilityTrigger::OnProjectileHit,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(100.0),
            )
    }

    /// Casts the scream from the origin; returns the projectiles spawned.
    fn cast_scream(app: &mut App, caster: Entity) -> Vec<SpawnProjectileEvent> {
        request(app, caster, 3, CastTarget::None);
        let mut spawned = Vec::new();
        for _ in 0..12 {
            step(app);
            let messages = app.world().resource::<Messages<SpawnProjectileEvent>>();
            spawned.extend(messages.iter_current_update_messages().copied());
        }
        spawned
    }

    #[test]
    fn projectile_to_each_sends_one_homing_projectile_per_valid_unit() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(scream_ability());
        let caster = spawn_caster(&mut app, KIT);
        let inside: Vec<_> = [100.0, 200.0, 300.0]
            .into_iter()
            .map(|x| spawn_unit(&mut app, Team::Dire, Vec3::new(x, 0.0, 0.0)))
            .collect();
        let _outside = spawn_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0));
        let _ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(150.0, 0.0, 0.0));
        let cycloned = spawn_unit(&mut app, Team::Dire, Vec3::new(250.0, 0.0, 0.0));
        app.world_mut()
            .entity_mut(cycloned)
            .insert(crate::status::StatusEffects {
                effects: vec![crate::status::StatusEffect::cyclone(5.0, true)],
            });

        let spawned = cast_scream(&mut app, caster);
        let mut targets: Vec<_> = spawned.iter().filter_map(|p| p.target).collect();
        targets.sort();
        let mut expected = inside.clone();
        expected.sort();
        assert_eq!(
            targets, expected,
            "3 inside; outside, ally, cycloned and caster skipped"
        );
        for projectile in &spawned {
            assert_eq!(projectile.splash_radius, 0.0);
            assert_eq!(projectile.speed, Some(900.0));
            assert_eq!(projectile.skillshot_width, None);
            assert_eq!(projectile.payload.caster, caster);
            assert_eq!(
                projectile.payload.source,
                EffectSource::Ability(AbilityId::Bulwark)
            );
            assert_eq!(projectile.payload.cast_target, projectile.target);
        }
    }

    #[test]
    fn projectile_to_each_hits_fire_on_projectile_hit_once_per_unit() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(scream_ability());
        let caster = spawn_caster(&mut app, KIT);
        let inside: Vec<_> = [100.0, 200.0, 300.0]
            .into_iter()
            .map(|x| spawn_unit(&mut app, Team::Dire, Vec3::new(x, 0.0, 0.0)))
            .collect();
        let spawned = cast_scream(&mut app, caster);
        assert_eq!(spawned.len(), 3);
        for projectile in &spawned {
            let unit = projectile.target.unwrap();
            app.world_mut().write_message(hit(
                projectile.payload,
                Vec3::X * 100.0,
                Some(unit),
                Vec::new(),
            ));
        }
        step(&mut app);
        for unit in inside {
            assert_eq!(health(&app, unit), 900.0, "each unit is hit exactly once");
        }
    }

    const QOP: [AbilityId; 4] = [
        AbilityId::ShadowStrike,
        AbilityId::QueenOfPainBlink,
        AbilityId::ScreamOfPain,
        AbilityId::SonicWave,
    ];

    /// The ability test app plus the status ticker (ticking burns are written there).
    fn qop_app() -> App {
        let mut app = test_app();
        app.add_systems(
            Update,
            crate::status::tick_status_effects.before(reactive_triggers::status_triggers),
        );
        app
    }

    /// Casts `slot` and returns the projectiles it spawned plus every damage event it
    /// produced while the cast played out.
    fn cast_for_projectiles(
        app: &mut App,
        caster: Entity,
        slot: usize,
        target: CastTarget,
    ) -> Vec<SpawnProjectileEvent> {
        request(app, caster, slot, target);
        let mut spawned = Vec::new();
        for _ in 0..16 {
            step(app);
            let messages = app.world().resource::<Messages<SpawnProjectileEvent>>();
            spawned.extend(messages.iter_current_update_messages().copied());
        }
        spawned
    }

    #[test]
    fn sonic_wave_deals_fourteen_instances_and_one_knockback() {
        use crate::combat::DamageKind;
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, QOP);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        let spawned = cast_for_projectiles(&mut app, caster, 3, CastTarget::Point(Vec3::X * 800.0));
        assert_eq!(spawned.len(), 1);
        let wave = spawned[0];
        assert_eq!(wave.skillshot_width, Some(100.0));
        assert_eq!(wave.skillshot_end_width, Some(450.0));
        assert!(
            (wave.speed.unwrap() - 900.0 / 1.4).abs() < 0.01,
            "900 units in 1.4s"
        );

        app.world_mut()
            .write_message(hit(wave.payload, Vec3::X * 400.0, Some(enemy), Vec::new()));
        let mut dots = Vec::new();
        let mut knockbacks = 0;
        for _ in 0..40 {
            step(&mut app);
            dots.extend(
                current_messages::<DamageEvent>(&app)
                    .into_iter()
                    .filter(|d| d.kind == DamageKind::Dot),
            );
            knockbacks += current_messages::<DisplacementEvent>(&app)
                .iter()
                .filter(|d| d.kind == DisplacementKind::Knockback)
                .count();
        }
        assert_eq!(dots.len(), 14, "14 instances over 1.4s");
        assert!(
            dots.iter()
                .all(|d| d.amount == 23.0 && d.damage_type == DamageType::Pure)
        );
        assert_eq!(knockbacks, 1, "one knockback");
        assert!((health(&app, enemy) - (1000.0 - 14.0 * 23.0)).abs() < 1e-2);
    }

    #[test]
    fn shadow_strike_and_sonic_wave_dots_coexist() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, QOP);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));
        let dagger = cast_for_projectiles(&mut app, caster, 0, CastTarget::Unit(enemy));
        let wave = cast_for_projectiles(&mut app, caster, 3, CastTarget::Point(Vec3::X * 800.0));
        assert_eq!((dagger.len(), wave.len()), (1, 1));
        let landed_at = Vec3::X * 300.0;
        app.world_mut()
            .write_message(hit(dagger[0].payload, landed_at, Some(enemy), Vec::new()));
        app.world_mut()
            .write_message(hit(wave[0].payload, landed_at, Some(enemy), Vec::new()));
        step(&mut app);
        let ids: Vec<_> = app
            .world()
            .get::<StatusEffects>(enemy)
            .unwrap()
            .effects
            .iter()
            .map(|e| e.id)
            .collect();
        assert!(ids.contains(&"shadow_strike_burn"), "{ids:?}");
        assert!(ids.contains(&"sonic_wave_burn"), "{ids:?}");
        assert!(ids.contains(&"shadow_strike_slow"), "{ids:?}");
        // Run past both durations (16s): 50 hit + 5 x 20 + 14 x 23.
        for _ in 0..340 {
            step(&mut app);
        }
        assert!(
            (health(&app, enemy) - (1000.0 - 50.0 - 100.0 - 322.0)).abs() < 1e-2,
            "{}",
            health(&app, enemy)
        );
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
        assert!(
            app.world().get::<AttackTarget>(caster).is_some(),
            "mechanics still run"
        );
        assert_eq!(
            health(&app, wounded),
            300.0,
            "the projectile itself deals nothing"
        );

        app.world_mut().write_message(hit(
            spawned[0].payload,
            Vec3::X * 200.0,
            Some(wounded),
            vec![healthy],
        ));
        step(&mut app);
        let base = catalog::EXECUTE_DAMAGE.at(1);
        assert!(
            (health(&app, wounded) - (300.0 - base * catalog::EXECUTE_MULTIPLIER)).abs() < 1e-3
        );
        assert!(
            (health(&app, healthy) - (1000.0 - base * 0.45)).abs() < 1e-3,
            "splash, no bonus"
        );
    }

    #[test]
    fn projectile_landing_without_a_hit_splashes_full_damage() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let payload = ProjectilePayload {
            caster,
            source: EffectSource::Ability(AbilityId::Bolt),
            rank: 1,
            cast_target: None,
            aim: Vec3::X * 200.0,
            reflected: false,
        };
        app.world_mut()
            .write_message(hit(payload, Vec3::X * 200.0, None, vec![enemy]));
        step(&mut app);
        assert!((health(&app, enemy) - (1000.0 - 120.0)).abs() < 1e-3);
    }

    /// Land a rank-1 Bolt on `primary` (no splash) and run one frame.
    fn land_bolt(app: &mut App, caster: Entity, primary: Entity) {
        let payload = ProjectilePayload {
            caster,
            source: EffectSource::Ability(AbilityId::Bolt),
            rank: 1,
            cast_target: Some(primary),
            aim: Vec3::X * 200.0,
            reflected: false,
        };
        app.world_mut()
            .write_message(hit(payload, Vec3::X * 200.0, Some(primary), Vec::new()));
        step(app);
    }

    fn enemy_at(app: &mut App, x: f32) -> Entity {
        spawn_unit(app, Team::Dire, Vec3::new(scale::u(x), 0.0, 0.0))
    }

    #[test]
    fn bolt_chain_hits_the_two_nearest_enemies_with_compounding_falloff() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let primary = enemy_at(&mut app, 300.0);
        let near = enemy_at(&mut app, 500.0);
        let next = enemy_at(&mut app, 800.0);
        let spare = enemy_at(&mut app, 1100.0);

        land_bolt(&mut app, caster, primary);

        // Rank 1: projectile 120; chain base 45 + 12 = 57, x0.75 per hop.
        assert!((health(&app, primary) - (1000.0 - 120.0)).abs() < 1e-3);
        assert!((health(&app, near) - (1000.0 - 57.0 * 0.75)).abs() < 1e-3);
        assert!((health(&app, next) - (1000.0 - 57.0 * 0.75 * 0.75)).abs() < 1e-3);
        assert_eq!(health(&app, spare), 1000.0, "only two jumps");
    }

    #[test]
    fn bolt_chain_stops_when_no_enemy_is_within_the_jump_radius() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let primary = enemy_at(&mut app, 300.0);
        let far = enemy_at(&mut app, 300.0 + 460.0);

        land_bolt(&mut app, caster, primary);

        assert_eq!(health(&app, far), 1000.0, "beyond the 450 jump radius");
        assert!((health(&app, primary) - (1000.0 - 120.0)).abs() < 1e-3);
    }

    #[test]
    fn bolt_chain_never_revisits_a_unit() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let primary = enemy_at(&mut app, 300.0);
        let only = enemy_at(&mut app, 500.0);

        land_bolt(&mut app, caster, primary);

        assert!(
            (health(&app, only) - (1000.0 - 57.0 * 0.75)).abs() < 1e-3,
            "hit once, not again on the second jump"
        );
        assert!(
            (health(&app, primary) - (1000.0 - 120.0)).abs() < 1e-3,
            "the primary target is not chained back to"
        );
    }

    fn chain_entry(
        trigger: AbilityTrigger,
        target: EffectTarget,
        jumps: u32,
        falloff: f32,
        then: &'static str,
    ) -> AbilityEffectEntry {
        AbilityEffectEntry {
            trigger,
            target,
            effect: AbilityEffect::Chain {
                jumps,
                radius: RankValue::fixed(scale::u(450.0)),
                falloff,
                team: TargetTeam::Enemy,
                then: AbilityTrigger::Custom(then),
            },
        }
    }

    fn damage_entry(
        trigger: AbilityTrigger,
        target: EffectTarget,
        amount: f32,
    ) -> AbilityEffectEntry {
        AbilityEffectEntry {
            trigger,
            target,
            effect: fixed_magical_damage(amount),
        }
    }

    /// The chain rows an ability (under `OnCast`) and an item (under `OnUse`) share.
    fn chain_rows(start: AbilityTrigger) -> Vec<AbilityEffectEntry> {
        vec![
            chain_entry(start, EffectTarget::CastTarget, 2, 0.5, "on_chain_hit"),
            damage_entry(
                AbilityTrigger::Custom("on_chain_hit"),
                EffectTarget::TriggerUnit,
                100.0,
            ),
        ]
    }

    fn chain_line(app: &mut App) -> (Entity, Entity, Entity) {
        (
            enemy_at(app, 300.0),
            enemy_at(app, 500.0),
            enemy_at(app, 800.0),
        )
    }

    #[test]
    fn bolt_chain_skips_cycloned_units() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let primary = enemy_at(&mut app, 300.0);
        let lifted = enemy_at(&mut app, 400.0);
        let next = enemy_at(&mut app, 600.0);
        app.world_mut()
            .get_mut::<StatusEffects>(lifted)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::cyclone(10.0, true));

        land_bolt(&mut app, caster, primary);

        assert_eq!(
            health(&app, lifted),
            1000.0,
            "untargetable units are skipped"
        );
        assert!((health(&app, next) - (1000.0 - 57.0 * 0.75)).abs() < 1e-3);
    }

    #[test]
    fn chain_hops_break_ties_by_entity() {
        use super::mechanics::{ChainCandidate, chain_hops};
        let mut world = World::new();
        let (a, b, start) = (
            world.spawn_empty().id(),
            world.spawn_empty().id(),
            world.spawn_empty().id(),
        );
        let at = |entity, x: f32, z: f32| ChainCandidate {
            entity,
            position: Vec3::new(x, 0.0, z),
            bounds: 0.0,
        };
        // Equally near on opposite sides: the lower entity wins whatever the list order.
        let first = chain_hops(
            start,
            Vec3::ZERO,
            &[at(b, 10.0, 0.0), at(a, -10.0, 0.0)],
            1,
            50.0,
        );
        let second = chain_hops(
            start,
            Vec3::ZERO,
            &[at(a, -10.0, 0.0), at(b, 10.0, 0.0)],
            1,
            50.0,
        );
        let lower = a.min(b);
        assert_eq!(
            first.iter().map(|c| c.entity).collect::<Vec<_>>(),
            vec![lower]
        );
        assert_eq!(
            second.iter().map(|c| c.entity).collect::<Vec<_>>(),
            vec![lower]
        );
        // The start is never revisited and hops measure from the previous unit.
        let line = chain_hops(
            start,
            Vec3::ZERO,
            &[at(start, 0.0, 0.0), at(a, 40.0, 0.0), at(b, 80.0, 0.0)],
            5,
            50.0,
        );
        assert_eq!(
            line.iter().map(|c| c.entity).collect::<Vec<_>>(),
            vec![a, b]
        );
    }

    #[test]
    fn item_and_ability_chain_emit_identical_damage() {
        // Ability: an OnCast chain from the cast target.
        let mut ability_app = test_app();
        let chain_ability = chain_rows(AbilityTrigger::OnCast).into_iter().fold(
            AbilityDefinition::new(AbilityId::Bulwark)
                .targeting(
                    TargetType::Unit,
                    RankValue::fixed(scale::u(600.0)),
                    RankValue::ZERO,
                )
                .costs(RankValue::fixed(8.0), RankValue::fixed(1.0)),
            |def, entry| def.on(entry.trigger, entry.target, entry.effect),
        );
        ability_app
            .world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(chain_ability);
        let caster = spawn_caster(&mut ability_app, KIT);
        let (p, n1, n2) = chain_line(&mut ability_app);
        request(&mut ability_app, caster, 3, CastTarget::Unit(p));
        for _ in 0..6 {
            step(&mut ability_app);
        }
        let ability_hp = [p, n1, n2].map(|e| health(&ability_app, e));

        // Item: the same rows under OnUse.
        let mut item_app = test_app();
        let effects: &'static [AbilityEffectEntry] =
            Box::leak(chain_rows(AbilityTrigger::OnUse).into_boxed_slice());
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::Unit,
            TargetTeam::Enemy,
            scale::u(600.0),
        )]);
        let caster = spawn_caster_with_inventory(&mut item_app, KIT);
        give_item(&mut item_app, caster, ItemId::HeartwoodBand);
        let (p, n1, n2) = chain_line(&mut item_app);
        request_item(&mut item_app, caster, 0, CastTarget::Unit(p));
        for _ in 0..6 {
            step(&mut item_app);
        }
        let item_hp = [p, n1, n2].map(|e| health(&item_app, e));

        assert_eq!(ability_hp, item_hp);
        assert_eq!(ability_hp[0], 1000.0, "the start unit is not re-fired");
        assert!((ability_hp[1] - 950.0).abs() < 1e-3, "first hop at 50%");
        assert!((ability_hp[2] - 975.0).abs() < 1e-3, "second hop at 25%");
    }

    #[test]
    fn chain_respects_max_trigger_depth() {
        let mut app = test_app();
        let entries = vec![
            chain_entry(
                AbilityTrigger::OnUse,
                EffectTarget::CastTarget,
                1,
                1.0,
                "on_a",
            ),
            chain_entry(
                AbilityTrigger::Custom("on_a"),
                EffectTarget::TriggerUnit,
                1,
                1.0,
                "on_b",
            ),
            damage_entry(
                AbilityTrigger::Custom("on_b"),
                EffectTarget::TriggerUnit,
                10.0,
            ),
            chain_entry(
                AbilityTrigger::Custom("on_b"),
                EffectTarget::TriggerUnit,
                1,
                1.0,
                "on_c",
            ),
            damage_entry(
                AbilityTrigger::Custom("on_c"),
                EffectTarget::TriggerUnit,
                10.0,
            ),
        ];
        let effects: &'static [AbilityEffectEntry] = Box::leak(entries.into_boxed_slice());
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::Unit,
            TargetTeam::Enemy,
            scale::u(600.0),
        )]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        // Each nested chain starts fresh, so it may step back; the spacing makes every
        // nearest choice unambiguous.
        let start = enemy_at(&mut app, 300.0);
        let first = enemy_at(&mut app, 500.0);
        let second = enemy_at(&mut app, 650.0);
        let third = enemy_at(&mut app, 900.0);
        request_item(&mut app, caster, 0, CastTarget::Unit(start));
        for _ in 0..6 {
            step(&mut app);
        }

        assert_eq!(health(&app, second), 990.0, "depth 2 still resolves");
        assert_eq!(health(&app, start), 1000.0, "on_a has no damage row");
        assert_eq!(health(&app, first), 1000.0);
        // The chain gate under on_b sits at the depth cap, so on_c never fires.
        assert_eq!(health(&app, third), 1000.0);
    }

    #[test]
    fn chain_entry_emits_no_gameplay_messages() {
        let mut app = test_app();
        let effects: &'static [AbilityEffectEntry] = Box::leak(
            vec![chain_entry(
                AbilityTrigger::OnUse,
                EffectTarget::CastTarget,
                2,
                0.5,
                "on_chain_hit",
            )]
            .into_boxed_slice(),
        );
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::Unit,
            TargetTeam::Enemy,
            scale::u(600.0),
        )]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let (start, _, _) = chain_line(&mut app);
        request_item(&mut app, caster, 0, CastTarget::Unit(start));
        let mut triggers = 0;
        for _ in 0..6 {
            step(&mut app);
            let world = app.world();
            assert_eq!(
                world
                    .resource::<Messages<DamageEvent>>()
                    .iter_current_update_messages()
                    .count(),
                0
            );
            assert_eq!(
                world
                    .resource::<Messages<HealEvent>>()
                    .iter_current_update_messages()
                    .count(),
                0
            );
            assert_eq!(
                world
                    .resource::<Messages<StatusEffectEvent>>()
                    .iter_current_update_messages()
                    .count(),
                0
            );
            assert_eq!(
                world
                    .resource::<Messages<DisplacementEvent>>()
                    .iter_current_update_messages()
                    .count(),
                0
            );
            assert_eq!(
                world
                    .resource::<Messages<TeleportEvent>>()
                    .iter_current_update_messages()
                    .count(),
                0
            );
            triggers += world
                .resource::<Messages<AbilityTriggerEvent>>()
                .iter_current_update_messages()
                .count();
        }
        assert!(triggers >= 1, "the item did fire");
        assert_eq!(health(&app, start), 1000.0);
    }

    #[test]
    fn chain_hop_damage_is_spell_damage_not_attack_bonus() {
        let mut app = test_app();
        let entries = vec![
            chain_entry(
                AbilityTrigger::OnAttackHit,
                EffectTarget::TriggerUnit,
                1,
                1.0,
                "on_chain_hit",
            ),
            damage_entry(AbilityTrigger::OnAttackHit, EffectTarget::TriggerUnit, 10.0),
            damage_entry(
                AbilityTrigger::Custom("on_chain_hit"),
                EffectTarget::TriggerUnit,
                20.0,
            ),
        ];
        let _defs = install_test_item_definitions([item_with_effects(entries)]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = enemy_at(&mut app, 80.0);
        let other = enemy_at(&mut app, 300.0);
        write_attack_hit(&mut app, caster, target);
        step(&mut app);

        let kinds: Vec<_> = app
            .world()
            .resource::<Messages<DamageEvent>>()
            .iter_current_update_messages()
            .map(|d| (d.target, d.kind, d.is_spell))
            .collect();
        assert!(kinds.contains(&(target, crate::combat::DamageKind::AttackBonus, false)));
        let hop = kinds.iter().find(|k| k.0 == other).expect("chained damage");
        assert_ne!(hop.1, crate::combat::DamageKind::AttackBonus);
        assert!(hop.2, "spell damage");
    }

    #[test]
    fn ally_chain_skips_enemies_and_scales_heal_by_falloff() {
        let mut app = test_app();
        let entries = vec![
            AbilityEffectEntry {
                trigger: AbilityTrigger::OnUse,
                target: EffectTarget::CastTarget,
                effect: AbilityEffect::Chain {
                    jumps: 2,
                    radius: RankValue::fixed(scale::u(450.0)),
                    falloff: 0.5,
                    team: TargetTeam::Ally,
                    then: AbilityTrigger::Custom("on_chain_hit"),
                },
            },
            AbilityEffectEntry {
                trigger: AbilityTrigger::Custom("on_chain_hit"),
                target: EffectTarget::TriggerUnit,
                effect: AbilityEffect::Heal {
                    amount: RankValue::fixed(100.0),
                    scaling: EffectScaling::None,
                },
            },
        ];
        let effects: &'static [AbilityEffectEntry] = Box::leak(entries.into_boxed_slice());
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::Unit,
            TargetTeam::Ally,
            scale::u(5000.0),
        )]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let ally_at = |app: &mut App, x: f32| {
            let unit = spawn_unit(app, Team::Radiant, Vec3::new(scale::u(x), 0.0, 0.0));
            app.world_mut().get_mut::<Health>(unit).unwrap().current = 500.0;
            unit
        };
        let start = ally_at(&mut app, 2000.0);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(scale::u(2100.0), 0.0, 0.0));
        let first = ally_at(&mut app, 2200.0);
        let second = ally_at(&mut app, 2500.0);

        request_item(&mut app, caster, 0, CastTarget::Unit(start));
        for _ in 0..6 {
            step(&mut app);
        }

        assert_eq!(health(&app, start), 500.0, "the start unit is not re-fired");
        assert_eq!(health(&app, enemy), 1000.0, "enemies are never healed");
        assert!(
            (health(&app, first) - 550.0).abs() < 1e-3,
            "first hop heals 50%"
        );
        assert!(
            (health(&app, second) - 525.0).abs() < 1e-3,
            "second hop heals 25%"
        );
    }

    #[test]
    fn magnitude_defaults_to_one_for_non_chain_triggers() {
        let ctx = TriggerContext::new(
            AbilityTrigger::OnCast,
            EffectSource::Ability(AbilityId::Bolt),
            Entity::PLACEHOLDER,
            Team::Radiant,
            1,
            Vec3::ZERO,
            Vec3::ZERO,
        );
        assert_eq!(ctx.magnitude, 1.0);

        let mut app = test_app();
        let entries = vec![damage_entry(
            AbilityTrigger::OnUse,
            EffectTarget::CastTarget,
            100.0,
        )];
        let effects: &'static [AbilityEffectEntry] = Box::leak(entries.into_boxed_slice());
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::Unit,
            TargetTeam::Enemy,
            scale::u(600.0),
        )]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = enemy_at(&mut app, 300.0);
        request_item(&mut app, caster, 0, CastTarget::Unit(target));
        for _ in 0..4 {
            step(&mut app);
        }
        assert!((health(&app, target) - 900.0).abs() < 1e-3);
    }

    #[test]
    fn while_toggled_and_on_cast_build_the_same_status() {
        use super::definition::StatusSpec;
        use crate::status::{DispelType, StatusEffect};
        let spec = StatusSpec::Burn {
            id: "burn",
            amount: RankValue::fixed(12.0),
            damage_type: DamageType::Magical,
            tick_interval: RankValue::ZERO,
        };
        let healing = StatusSpec::Modifier {
            id: "twin_bonus",
            kind: crate::status::StatusKind::Buff,
            armor: RankValue::fixed(3.0),
            magic_resist: RankValue::ZERO,
            move_speed: RankValue::fixed(15.0),
            attack_damage: RankValue::ZERO,
            attack_speed_flat: RankValue::ZERO,
            attack_speed_mult: RankValue::fixed(0.1),
            attack_range: RankValue::ZERO,
            attack_projectile_speed: RankValue::ZERO,
            attack_damage_pct: RankValue::ZERO,
            cleave: RankValue::ZERO,
            cleave_radius: RankValue::ZERO,
            cleave_end_radius: RankValue::ZERO,
            cleave_distance: RankValue::ZERO,
            attack_charges: RankValue::ZERO,
            independent: false,
        };
        let mut app = test_app();
        let toggle = AbilityDefinition::new(AbilityId::Overcharge)
            .behavior(super::definition::AbilityBehavior::Toggle)
            .toggle_upkeep(RankValue::fixed(1.0))
            .on(
                AbilityTrigger::WhileToggled,
                EffectTarget::Caster,
                AbilityEffect::ApplyStatus {
                    status: spec,
                    duration: RankValue::ZERO,
                },
            )
            .on(
                AbilityTrigger::WhileToggled,
                EffectTarget::Caster,
                AbilityEffect::ApplyStatus {
                    status: healing,
                    duration: RankValue::ZERO,
                },
            );
        let cast = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::NoTarget, RankValue::ZERO, RankValue::ZERO)
            .costs(RankValue::fixed(1.0), RankValue::fixed(1.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::Caster,
                AbilityEffect::ApplyStatus {
                    status: spec,
                    duration: RankValue::fixed(f32::INFINITY),
                },
            )
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::Caster,
                AbilityEffect::ApplyStatus {
                    status: healing,
                    duration: RankValue::fixed(f32::INFINITY),
                },
            );
        {
            let mut defs = app.world_mut().resource_mut::<AbilityDefinitions>();
            defs.insert(toggle);
            defs.insert(cast);
        }
        let toggler = spawn_caster(
            &mut app,
            [
                AbilityId::Overcharge,
                AbilityId::Shockwave,
                AbilityId::Execute,
                AbilityId::Dash,
            ],
        );
        let caster = spawn_caster(
            &mut app,
            [
                AbilityId::Bulwark,
                AbilityId::Shockwave,
                AbilityId::Execute,
                AbilityId::Dash,
            ],
        );
        for unit in [toggler, caster] {
            app.world_mut()
                .entity_mut(unit)
                .insert(crate::components::HeroAttributes::starter());
        }
        app.world_mut()
            .get_mut::<AbilityLoadout>(toggler)
            .unwrap()
            .slots[0]
            .toggled = true;
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..12 {
            bevy::ecs::system::RunSystemOnce::run_system_once(
                app.world_mut(),
                passive::tick_toggle_upkeep,
            )
            .unwrap();
            step(&mut app);
        }

        let status_of = |app: &App, unit: Entity| -> Vec<StatusEffect> {
            let mut effects = app
                .world()
                .get::<StatusEffects>(unit)
                .unwrap()
                .effects
                .clone();
            effects.sort_by_key(|e| e.id);
            effects
        };
        let (toggled, cast) = (status_of(&app, toggler), status_of(&app, caster));
        assert_eq!(toggled.len(), 2);
        assert_eq!(cast.len(), 2);
        for (mut a, mut b) in toggled.into_iter().zip(cast) {
            // The toggle keeps its bonus through dispels; nothing else differs.
            assert!(a.undispellable && a.dispel_type == DispelType::None);
            // Held statuses are reactive (silent); a cast's status is not.
            assert!(a.reactive && !b.reactive);
            a.undispellable = b.undispellable;
            a.dispel_type = b.dispel_type;
            a.reactive = b.reactive;
            a.dot_source = a.dot_source.map(|_| Entity::PLACEHOLDER);
            b.dot_source = b.dot_source.map(|_| Entity::PLACEHOLDER);
            assert_eq!(format!("{a:?}"), format!("{b:?}"));
        }
    }

    #[test]
    fn projectile_hit_trigger_carries_the_payload_source() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let victim = enemy_at(&mut app, 300.0);
        for (source, trigger) in [
            (
                EffectSource::Ability(AbilityId::Bolt),
                AbilityTrigger::OnProjectileHit,
            ),
            (
                EffectSource::Item(ItemId::StormRod),
                AbilityTrigger::OnProjectileHit,
            ),
            (
                EffectSource::Item(ItemId::StormRod),
                AbilityTrigger::OnImpact,
            ),
        ] {
            let payload = ProjectilePayload {
                caster,
                source,
                rank: 1,
                cast_target: None,
                aim: Vec3::X,
                reflected: false,
            };
            let primary = (trigger == AbilityTrigger::OnProjectileHit).then_some(victim);
            app.world_mut()
                .write_message(hit(payload, Vec3::X, primary, Vec::new()));
            // Run only the emitter, so no effect resolves.
            bevy::ecs::system::RunSystemOnce::run_system_once(
                app.world_mut(),
                projectile_hit_triggers,
            )
            .unwrap();
            let triggers: Vec<_> = app
                .world()
                .resource::<Messages<AbilityTriggerEvent>>()
                .iter_current_update_messages()
                .map(|e| (e.0.source, e.0.trigger))
                .collect();
            assert_eq!(triggers.last(), Some(&(source, trigger)));
            step(&mut app);
        }
    }

    fn projectile_item(entries: Vec<AbilityEffectEntry>) -> ItemDefinition {
        static MECHANICS: [AbilityMechanic; 1] = [AbilityMechanic::Projectile {
            splash_radius: AreaRadius::Ability,
            speed: None,
            skillshot: false,
            first_hit: false,
            end_radius: None,
            origin_offset: None,
            seek: None,
        }];
        let effects: &'static [AbilityEffectEntry] = Box::leak(entries.into_boxed_slice());
        ItemDefinition {
            mechanics: &MECHANICS,
            ..item_with(
                effects,
                TargetType::Unit,
                TargetTeam::Enemy,
                scale::u(600.0),
            )
        }
    }

    fn use_projectile_item(
        app: &mut App,
        caster: Entity,
        target: Entity,
    ) -> Vec<SpawnProjectileEvent> {
        give_item(app, caster, ItemId::HeartwoodBand);
        request_item(app, caster, 0, CastTarget::Unit(target));
        let mut spawned = Vec::new();
        for _ in 0..6 {
            step(app);
            spawned.extend(
                app.world()
                    .resource::<Messages<SpawnProjectileEvent>>()
                    .iter_current_update_messages()
                    .copied(),
            );
        }
        spawned
    }

    #[test]
    fn item_projectile_fires_on_projectile_hit_for_the_item() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([projectile_item(vec![damage_entry(
            AbilityTrigger::OnProjectileHit,
            EffectTarget::TriggerUnit,
            75.0,
        )])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        let target = enemy_at(&mut app, 300.0);
        let spawned = use_projectile_item(&mut app, caster, target);

        assert_eq!(spawned.len(), 1, "the item launched its projectile");
        assert_eq!(
            spawned[0].payload.source,
            EffectSource::Item(ItemId::HeartwoodBand)
        );
        assert_eq!(spawned[0].target, Some(target));
        assert_eq!(health(&app, target), 1000.0, "nothing lands before the hit");

        app.world_mut().write_message(hit(
            spawned[0].payload,
            Vec3::X * 300.0,
            Some(target),
            Vec::new(),
        ));
        step(&mut app);
        assert!((health(&app, target) - 925.0).abs() < 1e-3);
    }

    #[test]
    fn item_projectile_splash_uses_the_item_radius() {
        let mut app = test_app();
        let entries = vec![AbilityEffectEntry {
            trigger: AbilityTrigger::OnImpact,
            target: EffectTarget::UnitsInRadius {
                center: super::definition::AreaCenter::TriggerPoint,
                radius: AreaRadius::Fixed(RankValue::fixed(143.0)),
                team: TargetTeam::Enemy,
            },
            effect: fixed_magical_damage(10.0),
        }];
        let _defs = install_test_item_definitions([projectile_item(entries)]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        let target = enemy_at(&mut app, 300.0);
        let spawned = use_projectile_item(&mut app, caster, target);

        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].splash_radius, 143.0);
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
                first_hit: false,
                end_radius: None,
                origin_offset: None,
                seek: None,
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
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(fireball);
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));

        let spawned = cast_and_collect_projectiles(&mut app, caster, 3, enemy);
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].payload.cast_target, Some(enemy));
        assert!(
            !app.world()
                .get::<StatusEffects>(enemy)
                .unwrap()
                .is_stunned()
        );

        app.world_mut().write_message(hit(
            spawned[0].payload,
            Vec3::X * 300.0,
            Some(enemy),
            Vec::new(),
        ));
        step(&mut app);
        assert!(
            app.world()
                .get::<StatusEffects>(enemy)
                .unwrap()
                .is_stunned()
        );
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
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(skewer);
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(4000.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(
            app.world()
                .get::<StatusEffects>(enemy)
                .unwrap()
                .is_stunned()
        );
        assert_eq!(health(&app, caster), 550.0);
    }

    #[test]
    fn ability_and_item_damage_share_the_same_executor() {
        const AMOUNT: f32 = 80.0;
        let ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                fixed_magical_damage(AMOUNT),
            );

        let mut ability_app = test_app();
        ability_app
            .world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let ability_caster = spawn_caster(&mut ability_app, KIT);
        let ability_target = spawn_unit(&mut ability_app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        request(
            &mut ability_app,
            ability_caster,
            3,
            CastTarget::Unit(ability_target),
        );
        for _ in 0..4 {
            step(&mut ability_app);
        }

        let mut item_app = test_app();
        let _defs = install_test_item_definitions([parity_damage_item(AMOUNT)]);
        let item_caster = spawn_caster_with_inventory(&mut item_app, KIT);
        give_item(&mut item_app, item_caster, ItemId::HeartwoodBand);
        let item_target = spawn_unit(&mut item_app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        request_item(&mut item_app, item_caster, 0, CastTarget::Unit(item_target));
        for _ in 0..4 {
            step(&mut item_app);
        }

        let expected = 1000.0 - AMOUNT;
        assert!((health(&ability_app, ability_target) - expected).abs() < 1e-3);
        assert!((health(&item_app, item_target) - expected).abs() < 1e-3);
        assert_eq!(
            health(&ability_app, ability_target),
            health(&item_app, item_target)
        );
    }

    fn stacking_hit(offset: u32) -> [AbilityEffect; 2] {
        [
            AbilityEffect::Damage {
                amount: RankValue::fixed(10.0),
                damage_type: DamageType::Pure,
                scaling: EffectScaling::TargetStacks {
                    id: "tester_stacks",
                    offset,
                },
            },
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Stacks {
                    id: "tester_stacks",
                },
                duration: RankValue::fixed(30.0),
            },
        ]
    }

    fn stacking_ability(offset: u32) -> AbilityDefinition {
        let [damage, stack] = stacking_hit(offset);
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(AbilityTrigger::OnCast, EffectTarget::CastTarget, damage)
            .on(AbilityTrigger::OnCast, EffectTarget::CastTarget, stack)
    }

    fn cast_at(app: &mut App, caster: Entity, target: Entity) -> f32 {
        let before = health(app, target);
        request(app, caster, 3, CastTarget::Unit(target));
        for _ in 0..4 {
            step(app);
        }
        before - health(app, target)
    }

    #[test]
    fn target_stacks_scales_by_stacks_plus_offset() {
        for (offset, expected) in [(1, [10.0, 20.0, 30.0]), (0, [0.0, 10.0, 20.0])] {
            let mut app = test_app();
            app.world_mut()
                .resource_mut::<AbilityDefinitions>()
                .insert(stacking_ability(offset));
            let caster = spawn_caster(&mut app, KIT);
            let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
            let dealt: Vec<f32> = (0..3).map(|_| cast_at(&mut app, caster, target)).collect();
            for (got, want) in dealt.iter().zip(expected) {
                assert!((got - want).abs() < 1e-3, "offset {offset}: {dealt:?}");
            }
            assert_eq!(
                app.world()
                    .get::<StatusEffects>(target)
                    .unwrap()
                    .stacks("tester_stacks"),
                3
            );
        }
    }

    #[test]
    fn different_targets_keep_separate_stacks() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(stacking_ability(1));
        let caster = spawn_caster(&mut app, KIT);
        let a = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let b = spawn_unit(&mut app, Team::Dire, Vec3::new(-80.0, 0.0, 0.0));
        assert_eq!(cast_at(&mut app, caster, a), 10.0);
        assert_eq!(cast_at(&mut app, caster, a), 20.0);
        assert_eq!(cast_at(&mut app, caster, b), 10.0, "b has its own count");
        assert_eq!(cast_at(&mut app, caster, a), 30.0);
    }

    #[test]
    fn item_and_ability_target_stacks_scale_alike() {
        let effects: &'static [AbilityEffectEntry] = Box::leak(Box::new({
            let [damage, stack] = stacking_hit(1);
            [
                AbilityEffectEntry {
                    trigger: AbilityTrigger::OnUse,
                    target: EffectTarget::CastTarget,
                    effect: damage,
                },
                AbilityEffectEntry {
                    trigger: AbilityTrigger::OnUse,
                    target: EffectTarget::CastTarget,
                    effect: stack,
                },
            ]
        }));
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::Unit,
            TargetTeam::Enemy,
            500.0,
        )]);
        let mut app = test_app();
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let mut dealt = Vec::new();
        for _ in 0..2 {
            let before = health(&app, target);
            request_item(&mut app, caster, 0, CastTarget::Unit(target));
            for _ in 0..4 {
                step(&mut app);
            }
            dealt.push(before - health(&app, target));
        }
        assert_eq!(dealt, [10.0, 20.0]);
    }

    fn costed_self_cast(mana_cost: f32, cast_time: f32) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .costs(RankValue::ZERO, RankValue::fixed(mana_cost))
            .timing(cast_time, 0.0)
    }

    fn cost_buff(amount: f32) -> crate::status::StatusEffect {
        crate::status::StatusEffect::resource_modifier(
            "cost_buff",
            crate::status::StatusKind::Buff,
            crate::vitals::ResourceStat::ManaCostAmp,
            amount,
            60.0,
        )
    }

    #[test]
    fn mana_cost_amp_reduces_ability_costs() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(costed_self_cast(100.0, 0.0));
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut()
            .get_mut::<CombatStats>(caster)
            .unwrap()
            .mana_cost_amp = -0.25;
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(cost_buff(-0.25));
        request(&mut app, caster, 3, CastTarget::Point(Vec3::ZERO));
        for _ in 0..4 {
            step(&mut app);
        }
        assert_eq!(
            app.world().get::<Mana>(caster).unwrap().current,
            1000.0 - 50.0,
            "permanent and timed modifiers add: 100 x (1 - 0.5)"
        );
    }

    #[test]
    fn cost_is_reevaluated_when_the_cast_fires() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(costed_self_cast(100.0, 0.3));
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(cost_buff(-0.5));
        request(&mut app, caster, 3, CastTarget::Point(Vec3::ZERO));
        step(&mut app);
        step(&mut app);
        assert_eq!(
            app.world().get::<Mana>(caster).unwrap().current,
            1000.0,
            "nothing is paid during the cast point"
        );
        // The buff ends before the cast point does.
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .clear();
        for _ in 0..10 {
            step(&mut app);
        }
        assert_eq!(
            app.world().get::<Mana>(caster).unwrap().current,
            900.0,
            "the full cost is paid at fire time"
        );
    }

    #[test]
    fn cost_buff_expiring_makes_an_unaffordable_cast_cancel_for_free() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(costed_self_cast(100.0, 0.3));
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Mana>(caster).unwrap().current = 60.0;
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(cost_buff(-0.5));
        request(&mut app, caster, 3, CastTarget::Point(Vec3::ZERO));
        step(&mut app);
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .clear();
        for _ in 0..10 {
            step(&mut app);
        }
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 60.0);
        assert_eq!(
            app.world().get::<AbilityLoadout>(caster).unwrap().slots[3]
                .cooldown
                .remaining,
            0.0
        );
    }

    #[test]
    fn mana_burn_deals_damage_equal_to_removed_mana() {
        let ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::DrainMana {
                    amount: RankValue::fixed(80.0),
                    burn_ratio: 1.0,
                    damage_type: DamageType::Pure,
                    transfer_ratio: 0.0,
                },
            );
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let caster = spawn_caster(&mut app, KIT);
        let rich = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let poor = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 80.0));
        for (unit, mana) in [(rich, 200.0), (poor, 30.0)] {
            app.world_mut()
                .entity_mut(unit)
                .insert(Mana::new(mana, 0.0));
        }
        for target in [rich, poor] {
            request(&mut app, caster, 3, CastTarget::Unit(target));
            for _ in 0..6 {
                step(&mut app);
            }
        }
        assert_eq!(
            health(&app, rich),
            920.0,
            "80 mana removed, 80 damage dealt"
        );
        assert_eq!(app.world().get::<Mana>(rich).unwrap().current, 120.0);
        assert_eq!(health(&app, poor), 970.0, "capped by the 30 mana it had");
        assert_eq!(app.world().get::<Mana>(poor).unwrap().current, 0.0);
    }

    #[test]
    fn drain_health_ability_damages_and_heals_caster_same_frame() {
        let ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::DrainHealth {
                    amount: RankValue::fixed(100.0),
                    damage_type: DamageType::Pure,
                    scaling: EffectScaling::None,
                    transfer_ratio: 0.5,
                },
            );
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 400.0;
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Unit(enemy));
        for _ in 0..8 {
            step(&mut app);
            if health(&app, enemy) < 1000.0 {
                break;
            }
        }
        assert_eq!(health(&app, enemy), 900.0);
        assert_eq!(
            health(&app, caster),
            450.0,
            "the heal lands in the frame the damage does"
        );
    }

    #[test]
    fn heal_over_time_takes_the_casters_heal_amp_once_at_application() {
        let ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .team(definition::TargetTeam::Ally)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::HealOverTime {
                        hps: RankValue::fixed(40.0),
                        max_health_pct: RankValue::ZERO,
                    },
                    duration: RankValue::fixed(5.0),
                },
            );
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut()
            .get_mut::<CombatStats>(caster)
            .unwrap()
            .heal_amp = 0.5;
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(80.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Unit(ally));
        for _ in 0..4 {
            step(&mut app);
        }
        let statuses = app.world().get::<StatusEffects>(ally).unwrap();
        let hot = statuses
            .effects
            .iter()
            .find(|e| e.id == "heal_over_time")
            .expect("heal over time applied");
        assert!((hot.heal_per_sec - 60.0).abs() < 1e-3, "40 hps x 1.5");
    }

    #[test]
    fn healer_amp_scales_both_parts() {
        let ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .team(definition::TargetTeam::Ally)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::HealOverTime {
                        hps: RankValue::fixed(40.0),
                        max_health_pct: RankValue::fixed(4.0),
                    },
                    duration: RankValue::fixed(5.0),
                },
            );
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut()
            .get_mut::<CombatStats>(caster)
            .unwrap()
            .heal_amp = 0.5;
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(80.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Unit(ally));
        for _ in 0..4 {
            step(&mut app);
        }
        let statuses = app.world().get::<StatusEffects>(ally).unwrap();
        let hot = statuses
            .effects
            .iter()
            .find(|e| e.id == "heal_over_time")
            .expect("heal over time applied");
        assert!((hot.heal_per_sec - 60.0).abs() < 1e-3, "40 hps x 1.5");
        assert!(
            (hot.heal_max_health_fraction - 0.06).abs() < 1e-6,
            "4% x 1.5"
        );
    }

    /// A unit-target enemy ability that taunts every enemy around the cast target onto it.
    fn curse_ability() -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(
                TargetType::Unit,
                RankValue::fixed(1000.0),
                RankValue::fixed(500.0),
            )
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .team(definition::TargetTeam::Enemy)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::enemies_around(definition::AreaCenter::Aim),
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::TauntCastTarget {
                        attack_speed: RankValue::fixed(50.0),
                    },
                    duration: RankValue::fixed(4.0),
                },
            )
    }

    fn taunt_of(app: &App, unit: Entity) -> Option<crate::status::StatusEffect> {
        app.world()
            .get::<StatusEffects>(unit)
            .unwrap()
            .effects
            .iter()
            .find(|e| e.id == "taunt")
            .cloned()
    }

    #[test]
    fn taunt_target_skips_cast_target() {
        let mut app = test_app();
        with_definitions(&mut app, [curse_ability()]);
        let caster = spawn_caster(&mut app, KIT);
        let cursed = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(3000.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(250.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Unit(cursed));
        for _ in 0..4 {
            step(&mut app);
        }
        let taunt = taunt_of(&app, near).expect("nearby enemy is taunted");
        assert_eq!(
            taunt.compulsion,
            Some(crate::status::Compulsion::Attack { target: cursed }),
            "forced onto the cast target, not the caster"
        );
        assert!(taunt_of(&app, cursed).is_none(), "never onto itself");
        assert!(taunt_of(&app, far).is_none(), "out of the radius");
        assert!(taunt_of(&app, ally).is_none(), "allies are not taunted");
        assert!(taunt_of(&app, caster).is_none());
    }

    #[test]
    fn taunt_target_attack_speed_reverts_on_dispel() {
        let mut app = test_app();
        with_definitions(&mut app, [curse_ability()]);
        let caster = spawn_caster(&mut app, KIT);
        let cursed = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));
        let speed = |app: &App| {
            app.world()
                .get::<CombatStats>(near)
                .unwrap()
                .attack_speed_flat
        };
        let before = speed(&app);
        request(&mut app, caster, 3, CastTarget::Unit(cursed));
        for _ in 0..4 {
            step(&mut app);
        }
        assert_eq!(
            speed(&app),
            before + 50.0,
            "bonus attack speed while taunted"
        );
        app.world_mut().write_message(StatusEffectEvent::new(
            None,
            near,
            crate::status::StatusChange::Dispel {
                kind: crate::status::StatusKind::Debuff,
                strength: crate::status::DispelStrength::Basic,
            },
        ));
        step(&mut app);
        assert!(
            taunt_of(&app, near).is_none(),
            "the dispel removed the taunt"
        );
        assert_eq!(speed(&app), before, "and the attack speed with it");
    }

    #[test]
    fn ability_and_item_status_effects_share_the_same_executor() {
        let status = AbilityEffect::ApplyStatus {
            status: StatusSpec::Stun,
            duration: RankValue::fixed(1.5),
        };
        let ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(AbilityTrigger::OnCast, EffectTarget::CastTarget, status);

        static ITEM_EFFECTS: std::sync::OnceLock<&'static [AbilityEffectEntry]> =
            std::sync::OnceLock::new();
        let item_effects = ITEM_EFFECTS.get_or_init(|| {
            Box::leak(Box::new([AbilityEffectEntry {
                trigger: AbilityTrigger::OnUse,
                target: EffectTarget::CastTarget,
                effect: AbilityEffect::ApplyStatus {
                    status: StatusSpec::Stun,
                    duration: RankValue::fixed(1.5),
                },
            }]))
        });
        let item = ItemDefinition {
            id: ItemId::StormRod,
            name: "Parity Stun",
            short_label: "PS",
            item_type: ItemType::Miscellaneous,
            cost: 1,
            sell_value: 0,
            tier: 1,
            rarity: ItemRarity::Common,
            max_stack: 1,
            consumable: false,
            max_charges: None,
            cooldown: 0.0,
            activation: ItemActivation {
                target_type: TargetType::Unit,
                target_team: TargetTeam::Enemy,
                cast_range: 500.0,
                cast_time: 0.0,
                channel_time: 0.0,
                mana_cost: 0.0,
            },
            attribute_toggle: None,
            components: &[],
            color: [1.0, 1.0, 1.0],
            description: "test",
            effects: item_effects,
            mechanics: &[],
        };

        let mut ability_app = test_app();
        ability_app
            .world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let ability_caster = spawn_caster(&mut ability_app, KIT);
        let ability_target = spawn_unit(&mut ability_app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        request(
            &mut ability_app,
            ability_caster,
            3,
            CastTarget::Unit(ability_target),
        );
        for _ in 0..4 {
            step(&mut ability_app);
        }

        let mut item_app = test_app();
        let _defs = install_test_item_definitions([item]);
        let item_caster = spawn_caster_with_inventory(&mut item_app, KIT);
        give_item(&mut item_app, item_caster, ItemId::StormRod);
        let item_target = spawn_unit(&mut item_app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        request_item(&mut item_app, item_caster, 0, CastTarget::Unit(item_target));
        for _ in 0..4 {
            step(&mut item_app);
        }

        assert!(
            ability_app
                .world()
                .get::<StatusEffects>(ability_target)
                .unwrap()
                .is_stunned()
        );
        assert!(
            item_app
                .world()
                .get::<StatusEffects>(item_target)
                .unwrap()
                .is_stunned()
        );
    }

    #[test]
    fn item_on_use_resolves_through_the_shared_trigger_pipeline() {
        let mut app = test_app();
        let owner = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Mana>(owner).unwrap().current = 500.0;
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(60.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(2000.0, 0.0, 0.0));
        app.world_mut()
            .write_message(AbilityTriggerEvent(TriggerContext {
                trigger: AbilityTrigger::OnUse,
                caster: owner,
                team: Team::Radiant,
                source: EffectSource::Item(ItemId::SparkPendant),
                rank: 1,
                origin: Vec3::ZERO,
                aim: Vec3::ZERO,
                point: Vec3::ZERO,
                cast_target: None,
                trigger_unit: None,
                affected_units: Vec::new(),
                attack: None,
                depth: 0,
                magnitude: 1.0,
                amount: 1.0,
                reaction: false,
                status: None,
                reflected: false,
            }));
        step(&mut app);
        assert!((health(&app, near) - 910.0).abs() < 1e-3);
        assert_eq!(health(&app, far), 1000.0);
        assert_eq!(app.world().get::<Mana>(owner).unwrap().current, 540.0);
    }

    #[test]
    fn displace_effect_pulls_the_target_toward_the_caster() {
        let mut app = test_app();
        let hook = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(600.0), RankValue::ZERO)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::Displace {
                    kind: DisplacementKind::Pull,
                    distance: None,
                    duration: RankValue::fixed(0.3),
                    speed: None,
                    contact_radius: None,
                    forward_offset: None,
                    bound_stun: None,
                },
            );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(hook);
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        for unit in [caster, enemy] {
            app.world_mut().entity_mut(unit).insert(BoundRadius(24.0));
        }

        request(&mut app, caster, 3, CastTarget::Unit(enemy));
        let mut was_moving = false;
        for _ in 0..30 {
            step(&mut app);
            was_moving |= app.world().get::<ForcedMovement>(enemy).is_some();
        }
        let x = app.world().get::<Transform>(enemy).unwrap().translation.x;
        assert!(
            was_moving,
            "displaced units are in forced motion while moving"
        );
        assert!(
            !app.world().get::<StatusEffects>(enemy).unwrap().is_rooted(),
            "forced motion no longer roots the victim"
        );
        assert!(
            (x - 48.0).abs() < 1e-2,
            "pulled into contact, not onto the caster: {x}"
        );
        assert!(app.world().get::<ForcedMovement>(enemy).is_none());
    }

    #[test]
    fn hook_composition_pulls_the_skillshot_victim_to_the_caster() {
        let mut app = test_app();
        let hook = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Point, RankValue::fixed(900.0), RankValue::ZERO)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .on(
                AbilityTrigger::OnProjectileHit,
                EffectTarget::TriggerUnit,
                AbilityEffect::Displace {
                    kind: DisplacementKind::Pull,
                    distance: None,
                    duration: RankValue::fixed(0.3),
                    speed: None,
                    contact_radius: None,
                    forward_offset: None,
                    bound_stun: None,
                },
            );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(hook);
        let caster = spawn_caster(&mut app, KIT);
        let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(700.0, 0.0, 0.0));
        let bystander = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 700.0));
        for unit in [caster, victim, bystander] {
            app.world_mut().entity_mut(unit).insert(BoundRadius(24.0));
        }
        let payload = ProjectilePayload {
            caster,
            source: EffectSource::Ability(AbilityId::Bulwark),
            rank: 1,
            cast_target: None,
            aim: Vec3::new(900.0, 0.0, 0.0),
            reflected: false,
        };
        app.world_mut().write_message(hit(
            payload,
            Vec3::new(700.0, 0.0, 0.0),
            Some(victim),
            Vec::new(),
        ));
        for _ in 0..20 {
            step(&mut app);
        }
        let world = app.world();
        assert!((world.get::<Transform>(victim).unwrap().translation.x - 48.0).abs() < 1e-2);
        assert_eq!(
            world.get::<Transform>(bystander).unwrap().translation,
            Vec3::new(0.0, 0.0, 700.0)
        );
    }

    fn dash_ability(distance: f32) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Point, RankValue::fixed(2000.0), RankValue::ZERO)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .mechanic(AbilityMechanic::Dash {
                distance: RankValue::fixed(distance),
            })
    }

    fn caster_x(app: &App, caster: Entity) -> f32 {
        app.world().get::<Transform>(caster).unwrap().translation.x
    }

    /// Steps until the caster's forced movement starts (after the cast point).
    fn step_until_moving(app: &mut App, caster: Entity) {
        for _ in 0..20 {
            if app.world().get::<ForcedMovement>(caster).is_some() {
                return;
            }
            step(app);
        }
        panic!("the motion never started");
    }

    #[test]
    fn dash_ability_moves_at_dash_speed_and_caps_distance() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(dash_ability(360.0));
        let caster = spawn_caster(&mut app, KIT);
        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        step_until_moving(&mut app, caster);
        let mut samples = Vec::new();
        for _ in 0..10 {
            step(&mut app);
            samples.push(caster_x(&app, caster));
        }
        // 0.05 s frames at 1200 units/s: 60 units per frame, well above the 300 walk speed.
        assert!((samples[0] - 60.0).abs() < 1.0, "{samples:?}");
        assert!((samples[1] - 120.0).abs() < 1.0, "{samples:?}");
        assert!(
            (samples.last().unwrap() - 360.0).abs() < 1e-2,
            "capped: {samples:?}"
        );
        assert!(app.world().get::<ForcedMovement>(caster).is_none());
        assert!(app.world().get::<MoveTarget>(caster).is_none());
    }

    #[test]
    fn dash_is_not_cancelled_by_a_new_move_order() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(dash_ability(360.0));
        let caster = spawn_caster(&mut app, KIT);
        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        step_until_moving(&mut app, caster);
        step(&mut app);
        app.world_mut().entity_mut(caster).insert(MoveTarget {
            position: Vec3::new(-2000.0, 0.0, 0.0),
        });
        for _ in 0..8 {
            step(&mut app);
        }
        assert!((caster_x(&app, caster) - 360.0).abs() < 1e-2);
        assert!(
            app.world().get::<MoveTarget>(caster).is_some(),
            "the new order waits for the dash to finish"
        );
    }

    #[test]
    fn rooted_caster_pays_but_does_not_dash() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(dash_ability(360.0));
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::rooted(5.0));
        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        for _ in 0..10 {
            step(&mut app);
        }
        assert_eq!(caster_x(&app, caster), 0.0);
        assert!(app.world().get::<ForcedMovement>(caster).is_none());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 900.0);
    }

    #[test]
    fn stun_mid_dash_ends_it_and_fires_movement_end() {
        let mut app = test_app();
        let dash = dash_ability(600.0).on(
            AbilityTrigger::OnMovementEnd,
            EffectTarget::Caster,
            fixed_magical_damage(10.0),
        );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(dash);
        let caster = spawn_caster(&mut app, KIT);
        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        step_until_moving(&mut app, caster);
        step(&mut app);
        step(&mut app);
        app.world_mut().write_message(StatusEffectEvent {
            reactive: false,
            source: None,
            target: caster,
            change: crate::status::StatusChange::Apply(crate::status::StatusEffect::stunned(2.0)),
        });
        for _ in 0..4 {
            step(&mut app);
        }
        let x = caster_x(&app, caster);
        assert!(x > 0.0 && x < 600.0, "stopped part-way: {x}");
        assert!(app.world().get::<ForcedMovement>(caster).is_none());
        assert!(
            health(&app, caster) < 1000.0,
            "movement end fired its effects"
        );
        let after = health(&app, caster);
        for _ in 0..6 {
            step(&mut app);
        }
        assert_eq!(health(&app, caster), after, "fires once");
    }

    #[test]
    fn drag_carries_contacted_units_with_the_dash() {
        let mut app = test_app();
        let skewer = dash_ability(600.0).on(
            AbilityTrigger::OnUnitContact,
            EffectTarget::TriggerUnit,
            AbilityEffect::Displace {
                kind: DisplacementKind::Drag,
                distance: None,
                duration: RankValue::ZERO,
                speed: None,
                contact_radius: None,
                forward_offset: None,
                bound_stun: None,
            },
        );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(skewer);
        let caster = spawn_caster(&mut app, KIT);
        let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(350.0, 0.0, 0.0));
        for unit in [caster, victim, ally] {
            app.world_mut().entity_mut(unit).insert(BoundRadius(30.0));
        }
        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        for _ in 0..30 {
            step(&mut app);
        }
        let dasher = caster_x(&app, caster);
        assert!((dasher - 600.0).abs() < 1e-2);
        let lead = caster_x(&app, victim) - dasher;
        assert!(
            lead > 0.0 && lead <= 60.0 + 1e-2,
            "dragged along at the front of the dash: {lead}"
        );
        assert!(app.world().get::<ForcedMovement>(victim).is_none());
        assert_eq!(caster_x(&app, ally), 350.0, "allies are not contacted");
    }

    #[test]
    fn unit_cast_on_a_cycloned_target_is_rejected_and_area_effects_skip_it() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let hidden = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let visible = spawn_unit(&mut app, Team::Dire, Vec3::new(-100.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(hidden)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect {
                // Isolate untargetability from the damage block.
                invulnerable: false,
                ..crate::status::StatusEffect::cyclone(5.0, true)
            });

        // Bolt is a unit-targeted spell.
        request(&mut app, caster, 1, CastTarget::Unit(hidden));
        step(&mut app);
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 1000.0);

        // Shockwave hits everything in its radius except the cycloned unit.
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..9 {
            step(&mut app);
        }
        assert!(health(&app, visible) < 1000.0, "the visible enemy was hit");
        assert_eq!(health(&app, hidden), 1000.0);
    }

    fn dash_item() -> ItemDefinition {
        static EFFECTS: std::sync::OnceLock<&'static [AbilityEffectEntry]> =
            std::sync::OnceLock::new();
        let effects = EFFECTS.get_or_init(|| {
            Box::leak(Box::new([
                AbilityEffectEntry {
                    trigger: AbilityTrigger::OnUse,
                    target: EffectTarget::Caster,
                    effect: AbilityEffect::Displace {
                        kind: DisplacementKind::Dash,
                        distance: Some(RankValue::fixed(300.0)),
                        duration: RankValue::fixed(0.25),
                        speed: None,
                        contact_radius: None,
                        forward_offset: None,
                        bound_stun: None,
                    },
                },
                AbilityEffectEntry {
                    trigger: AbilityTrigger::OnUnitContact,
                    target: EffectTarget::TriggerUnit,
                    effect: fixed_magical_damage(50.0),
                },
            ]))
        });
        ItemDefinition {
            effects,
            name: "Parity Dash",
            ..teleport_item()
        }
    }

    #[test]
    fn item_dash_uses_the_same_effect() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([dash_item()]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::WardstoneCloak);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(150.0, 0.0, 0.0));
        for unit in [caster, enemy] {
            app.world_mut().entity_mut(unit).insert(BoundRadius(30.0));
        }
        request_item(
            &mut app,
            caster,
            0,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        for _ in 0..12 {
            step(&mut app);
        }
        assert!((caster_x(&app, caster) - 300.0).abs() < 1e-2);
        assert_eq!(
            health(&app, enemy),
            950.0,
            "the item's contact row hit the enemy once"
        );
    }

    #[test]
    fn charge_tracks_target_and_fires_end_with_trigger_unit() {
        let mut app = test_app();
        let charge = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(2000.0), RankValue::ZERO)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::Caster,
                AbilityEffect::Displace {
                    kind: DisplacementKind::Charge,
                    distance: None,
                    duration: RankValue::fixed(3.0),
                    speed: Some(RankValue::fixed(900.0)),
                    contact_radius: None,
                    forward_offset: None,
                    bound_stun: None,
                },
            )
            .on(
                AbilityTrigger::OnMovementEnd,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(100.0),
            );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(charge);
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(800.0, 0.0, 0.0));
        for unit in [caster, enemy] {
            app.world_mut().entity_mut(unit).insert(BoundRadius(30.0));
        }
        // The target keeps running away at walking speed.
        app.world_mut().entity_mut(enemy).insert(MoveTarget {
            position: Vec3::new(800.0, 0.0, 3000.0),
        });
        request(&mut app, caster, 3, CastTarget::Unit(enemy));
        let mut started = false;
        let mut gap_at_end = None;
        for _ in 0..40 {
            step(&mut app);
            let moving = app.world().get::<ForcedMovement>(caster).is_some();
            started |= moving;
            let world = app.world();
            let c = world.get::<Transform>(caster).unwrap().translation;
            let e = world.get::<Transform>(enemy).unwrap().translation;
            if started && !moving && gap_at_end.is_none() {
                gap_at_end = Some((c.distance(e), c.z));
            }
            // No movement plugin in this app: walk the target by hand at 300 units/s.
            app.world_mut()
                .get_mut::<Transform>(enemy)
                .unwrap()
                .translation
                .z += 15.0;
        }
        let (gap, z) = gap_at_end.expect("the charge ended");
        assert!(z > 50.0, "the charge followed the target sideways: {z}");
        assert!(gap < 70.0, "ended in contact: {gap}");
        assert_eq!(
            health(&app, enemy),
            900.0,
            "end trigger hit the charged unit once"
        );
    }

    fn blink_ability() -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Point, RankValue::fixed(2000.0), RankValue::ZERO)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::Caster,
                AbilityEffect::Teleport {
                    min_distance: RankValue::ZERO,
                    max_distance: None,
                },
            )
    }

    #[test]
    fn teleport_ability_relocates_the_caster_after_paying_costs() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(blink_ability());
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().entity_mut(caster).insert((
            CollisionRadius(40.0),
            MoveTarget {
                position: Vec3::new(-900.0, 0.0, 0.0),
            },
        ));

        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(1500.0, 0.0, 300.0)),
        );
        for _ in 0..12 {
            step(&mut app);
        }
        let world = app.world();
        assert_eq!(
            world.get::<Transform>(caster).unwrap().translation,
            Vec3::new(1500.0, 0.0, 300.0)
        );
        assert!(world.get::<MoveTarget>(caster).is_none());
        assert_eq!(world.get::<Mana>(caster).unwrap().current, 900.0);
        assert!(
            world.get::<AbilityLoadout>(caster).unwrap().slots[3]
                .cooldown
                .remaining
                > 0.0
        );
    }

    fn bounded_blink_ability() -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(
                TargetType::Point,
                RankValue::fixed(scale::GLOBAL_CAST_RANGE),
                RankValue::ZERO,
            )
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::Caster,
                AbilityEffect::Teleport {
                    min_distance: RankValue::fixed(200.0),
                    max_distance: Some(RankValue::fixed(1075.0)),
                },
            )
    }

    /// Casts the bounded blink at `click` from the origin, facing +X, and returns where the
    /// caster ends up (plus whether it was ever ordered to walk).
    fn bounded_blink_to(click: Vec3) -> (Vec3, f32) {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(bounded_blink_ability());
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().entity_mut(caster).insert((
            CollisionRadius(40.0),
            Transform::from_translation(Vec3::ZERO).looking_to(Vec3::X, Vec3::Y),
        ));
        request(&mut app, caster, 3, CastTarget::Point(click));
        let mut walked = false;
        for _ in 0..12 {
            step(&mut app);
            walked |= app.world().get::<MoveTarget>(caster).is_some();
        }
        let world = app.world();
        let mana = world.get::<Mana>(caster).unwrap().current;
        assert!(
            !walked,
            "a global cast range never walks the caster into range"
        );
        (world.get::<Transform>(caster).unwrap().translation, mana)
    }

    #[test]
    fn bounded_teleport_clamps_a_far_click_to_the_maximum_distance() {
        let (at, mana) = bounded_blink_to(Vec3::new(3000.0, 0.0, 0.0));
        assert!(at.distance(Vec3::new(1075.0, 0.0, 0.0)) < 1e-2, "{at}");
        assert_eq!(mana, 900.0, "the cast was paid for");
    }

    #[test]
    fn bounded_teleport_pushes_a_near_click_out_to_the_minimum_distance() {
        let (at, _) = bounded_blink_to(Vec3::new(50.0, 0.0, 0.0));
        assert!(at.distance(Vec3::new(200.0, 0.0, 0.0)) < 1e-2, "{at}");
        let (at, _) = bounded_blink_to(Vec3::new(0.0, 0.0, 500.0));
        assert!(
            at.distance(Vec3::new(0.0, 0.0, 500.0)) < 1e-2,
            "an in-bounds click is exact: {at}"
        );
    }

    #[test]
    fn bounded_teleport_clicked_on_the_caster_moves_along_her_facing() {
        let (at, _) = bounded_blink_to(Vec3::ZERO);
        assert!(at.distance(Vec3::new(200.0, 0.0, 0.0)) < 1e-2, "{at}");
    }

    #[test]
    fn teleport_destination_applies_bounds_and_leaves_unbounded_aims_alone() {
        use super::effects::teleport_destination;
        let origin = Vec3::new(10.0, 0.0, 10.0);
        let aim = Vec3::new(3010.0, 5.0, 10.0);
        assert_eq!(teleport_destination(origin, aim, Vec3::X, 0.0, None), aim);
        let far = teleport_destination(origin, aim, Vec3::X, 200.0, Some(1075.0));
        assert!(far.distance(Vec3::new(1085.0, 5.0, 10.0)) < 1e-3, "{far}");
        let on_self = teleport_destination(origin, origin, Vec3::Z, 200.0, Some(1075.0));
        assert!(
            on_self.distance(Vec3::new(10.0, 0.0, 210.0)) < 1e-3,
            "{on_self}"
        );
        let no_min = teleport_destination(origin, origin, Vec3::Z, 0.0, Some(1075.0));
        assert!(no_min.distance(origin) < 1e-3, "no minimum: stays put");
    }

    #[test]
    fn teleport_ability_into_an_obstacle_leaves_the_caster_in_place() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(blink_ability());
        app.world_mut()
            .resource_mut::<NavGrid>()
            .rebuild(vec![NavShape::Circle {
                center: Vec2::new(1500.0, 0.0),
                radius: 120.0,
            }]);
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut()
            .entity_mut(caster)
            .insert(CollisionRadius(40.0));

        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(1450.0, 0.0, 0.0)),
        );
        let mut rejections = Vec::new();
        for _ in 0..12 {
            step(&mut app);
            let messages = app.world().resource::<Messages<TeleportRejectedEvent>>();
            rejections.extend(messages.iter_current_update_messages().map(|e| e.reason));
        }
        let world = app.world();
        assert_eq!(rejections, vec![TeleportError::Blocked]);
        assert_eq!(
            world.get::<Transform>(caster).unwrap().translation,
            Vec3::ZERO
        );
    }

    fn teleport_item() -> ItemDefinition {
        static EFFECTS: std::sync::OnceLock<&'static [AbilityEffectEntry]> =
            std::sync::OnceLock::new();
        let effects = EFFECTS.get_or_init(|| {
            Box::leak(Box::new([AbilityEffectEntry {
                trigger: AbilityTrigger::OnUse,
                target: EffectTarget::Caster,
                effect: AbilityEffect::Teleport {
                    min_distance: RankValue::ZERO,
                    max_distance: None,
                },
            }]))
        });
        ItemDefinition {
            id: ItemId::WardstoneCloak,
            name: "Parity Blink",
            short_label: "PB",
            item_type: ItemType::Miscellaneous,
            cost: 1,
            sell_value: 0,
            tier: 1,
            rarity: ItemRarity::Common,
            max_stack: 1,
            consumable: false,
            max_charges: None,
            cooldown: 0.0,
            activation: ItemActivation {
                target_type: TargetType::Point,
                target_team: TargetTeam::Any,
                cast_range: 5000.0,
                cast_time: 0.0,
                channel_time: 0.0,
                mana_cost: 0.0,
            },
            attribute_toggle: None,
            components: &[],
            color: [1.0, 1.0, 1.0],
            description: "test",
            effects,
            mechanics: &[],
        }
    }

    #[test]
    fn items_and_abilities_teleport_through_the_same_effect() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(blink_ability());
        let _defs = install_test_item_definitions([teleport_item()]);
        let by_ability = spawn_caster(&mut app, KIT);
        let by_item = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, by_item, ItemId::WardstoneCloak);
        app.world_mut()
            .entity_mut(by_item)
            .insert(Transform::from_translation(Vec3::new(0.0, 0.0, 800.0)));
        for unit in [by_ability, by_item] {
            app.world_mut()
                .entity_mut(unit)
                .insert(CollisionRadius(40.0));
        }

        request(
            &mut app,
            by_ability,
            3,
            CastTarget::Point(Vec3::new(2000.0, 0.0, 0.0)),
        );
        for _ in 0..12 {
            step(&mut app);
        }

        request_item(
            &mut app,
            by_item,
            0,
            CastTarget::Point(Vec3::new(-2000.0, 0.0, 0.0)),
        );
        for _ in 0..8 {
            step(&mut app);
        }

        let world = app.world();
        assert_eq!(
            world.get::<Transform>(by_ability).unwrap().translation,
            Vec3::new(2000.0, 0.0, 0.0)
        );
        assert_eq!(
            world.get::<Transform>(by_item).unwrap().translation,
            Vec3::new(-2000.0, 0.0, 0.0)
        );
    }

    fn channelled_heal() -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .timing(0.1, 0.0)
            .channel(0.5)
            .on(
                AbilityTrigger::OnChannelEnd,
                EffectTarget::Caster,
                AbilityEffect::Heal {
                    amount: RankValue::fixed(200.0),
                    scaling: EffectScaling::None,
                },
            )
            .on(
                AbilityTrigger::OnChannelInterrupted,
                EffectTarget::Caster,
                AbilityEffect::Heal {
                    amount: RankValue::fixed(50.0),
                    scaling: EffectScaling::None,
                },
            )
    }

    /// Start `channelled_heal` on a caster at 500 health and run it into its channel.
    fn channelling_caster(app: &mut App) -> Entity {
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(channelled_heal());
        let caster = spawn_caster(app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        request(app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(app);
        }
        caster
    }

    #[test]
    fn completed_ability_channel_fires_only_on_channel_end() {
        let mut app = test_app();
        let caster = channelling_caster(&mut app);
        let mut fired = Vec::new();
        for _ in 0..12 {
            step(&mut app);
            fired.extend(
                app.world()
                    .resource::<Messages<AbilityTriggerEvent>>()
                    .iter_current_update_messages()
                    .map(|event| event.0.trigger),
            );
        }
        assert_eq!(fired, vec![AbilityTrigger::OnChannelEnd]);
        assert_eq!(health(&app, caster), 700.0, "only the 200 heal");
    }

    #[test]
    fn order_cancelled_channel_fires_neither_trigger() {
        let mut app = test_app();
        let caster = channelling_caster(&mut app);
        // A new order removes the activation (see `unit_commands`) without an end event.
        app.world_mut().entity_mut(caster).remove::<Activation>();
        for _ in 0..12 {
            step(&mut app);
            let triggers = app
                .world()
                .resource::<Messages<AbilityTriggerEvent>>()
                .iter_current_update_messages()
                .filter(|event| {
                    matches!(
                        event.0.trigger,
                        AbilityTrigger::OnChannelEnd | AbilityTrigger::OnChannelInterrupted
                    )
                })
                .count();
            assert_eq!(triggers, 0);
        }
        assert_eq!(health(&app, caster), 500.0);
    }

    #[test]
    fn channelled_abilities_hold_the_activation_then_fire_on_channel_end() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(channelled_heal());
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;

        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        let activation = *app.world().get::<Activation>(caster).unwrap();
        assert!(
            activation.fired && !activation.released(),
            "channelling after the cast point"
        );
        assert_eq!(
            app.world().get::<Mana>(caster).unwrap().current,
            900.0,
            "paid at the cast point"
        );
        assert_eq!(health(&app, caster), 500.0);

        for _ in 0..12 {
            step(&mut app);
        }
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(health(&app, caster), 700.0, "OnChannelEnd effects ran once");
    }

    #[test]
    fn slept_ability_channel_fires_only_on_channel_interrupted() {
        let mut app = test_app();
        let caster = channelling_caster(&mut app);
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::sleep(5.0));
        let mut fired = Vec::new();
        for _ in 0..12 {
            step(&mut app);
            fired.extend(
                app.world()
                    .resource::<Messages<AbilityTriggerEvent>>()
                    .iter_current_update_messages()
                    .map(|event| event.0.trigger),
            );
        }
        assert_eq!(fired, vec![AbilityTrigger::OnChannelInterrupted]);
        assert_eq!(health(&app, caster), 550.0);
    }

    #[test]
    fn unit_channel_end_after_the_target_despawned_still_fires_once() {
        let mut app = test_app();
        let def = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(600.0), RankValue::ZERO)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .timing(0.1, 0.0)
            .channel(0.5)
            .on(
                AbilityTrigger::OnChannelEnd,
                EffectTarget::CastTarget,
                fixed_magical_damage(10.0),
            )
            .on(
                AbilityTrigger::OnChannelEnd,
                EffectTarget::Caster,
                AbilityEffect::Heal {
                    amount: RankValue::fixed(200.0),
                    scaling: EffectScaling::None,
                },
            );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(def);
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::Unit(target));
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(app.world().get::<Activation>(caster).unwrap().fired);
        app.world_mut().entity_mut(target).despawn();
        let mut ends = 0;
        for _ in 0..12 {
            step(&mut app);
            ends += app
                .world()
                .resource::<Messages<AbilityTriggerEvent>>()
                .iter_current_update_messages()
                .filter(|event| event.0.trigger == AbilityTrigger::OnChannelEnd)
                .count();
        }
        assert_eq!(ends, 1);
        assert_eq!(health(&app, caster), 700.0, "the caster effect still lands");
    }

    #[test]
    fn stunned_ability_channel_fires_only_on_channel_interrupted() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(channelled_heal());
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::stunned(1.0));
        step(&mut app);
        let ends = app.world().resource::<Messages<ActionChannelEndEvent>>();
        let ends: Vec<_> = ends
            .iter_current_update_messages()
            .map(|e| e.completed)
            .collect();
        assert_eq!(ends, vec![false]);
        assert!(
            app.world()
                .get::<Activation>(caster)
                .is_none_or(|a| a.released())
        );
        assert_eq!(
            health(&app, caster),
            550.0,
            "the 50 interrupted heal, not the 200 end heal and not both"
        );
    }

    #[test]
    fn frozen_interrupts_channel() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(channelled_heal());
        let caster = spawn_caster(&mut app, KIT);
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::frozen(1.0));
        step(&mut app);
        let ends: Vec<_> = app
            .world()
            .resource::<Messages<ActionChannelEndEvent>>()
            .iter_current_update_messages()
            .map(|e| e.completed)
            .collect();
        assert_eq!(ends, vec![false], "interrupted, not completed");
    }

    #[test]
    fn self_target_abilities_target_the_caster() {
        let mut app = test_app();
        let def = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::SelfTarget, RankValue::ZERO, RankValue::ZERO)
            .team(TargetTeam::SelfOnly)
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::Heal {
                    amount: RankValue::fixed(100.0),
                    scaling: EffectScaling::None,
                },
            );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(def);
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::X * 50.0);
        request(&mut app, caster, 3, CastTarget::Unit(ally));
        for _ in 0..12 {
            step(&mut app);
        }
        assert_eq!(health(&app, caster), 600.0);
    }

    #[test]
    fn direction_abilities_fire_at_any_distance_along_the_direction() {
        let mut app = test_app();
        let def = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(
                TargetType::Direction,
                RankValue::fixed(400.0),
                RankValue::ZERO,
            )
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0));
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(def);
        let caster = spawn_caster(&mut app, KIT);
        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(0.0, 0.0, 9000.0)),
        );
        let mut aims = Vec::new();
        for _ in 0..30 {
            step(&mut app);
            let casts = app.world().resource::<Messages<AbilityCastEvent>>();
            aims.extend(casts.iter_current_update_messages().map(|c| c.aim));
        }
        assert!(
            app.world().get::<QueuedActivation>(caster).is_none(),
            "never walks"
        );
        assert_eq!(aims, vec![Vec3::new(0.0, 0.0, 400.0)]);
    }

    #[test]
    fn item_activation_without_an_inventory_does_nothing() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().write_message(ActivationRequest {
            caster,
            source: crate::actions::ActionSource::Item(0),
            target: CastTarget::None,
        });
        step(&mut app);
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 1000.0);
    }

    #[test]
    fn reskilled_slots_cancel_the_in_flight_cast() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        request(&mut app, caster, 0, CastTarget::None);
        step(&mut app);
        assert!(app.world().get::<Activation>(caster).is_some());
        app.world_mut()
            .get_mut::<AbilityLoadout>(caster)
            .unwrap()
            .slots[0] = AbilityState {
            rank: 1,
            ..AbilityState::fresh(AbilityId::Bolt)
        };
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(
            app.world().get::<Mana>(caster).unwrap().current,
            1000.0,
            "nothing was paid"
        );
    }

    // --- Reactive triggers ---

    fn current_messages<M: Message + Clone>(app: &App) -> Vec<M> {
        app.world()
            .resource::<Messages<M>>()
            .iter_current_update_messages()
            .cloned()
            .collect()
    }

    fn entry(
        trigger: AbilityTrigger,
        target: EffectTarget,
        effect: AbilityEffect,
    ) -> AbilityEffectEntry {
        AbilityEffectEntry {
            trigger,
            target,
            effect,
        }
    }

    /// A trigger context for the test item, rooted at the caster, aimed at `target`.
    fn item_ctx(
        app: &App,
        caster: Entity,
        trigger: AbilityTrigger,
        target: Entity,
    ) -> TriggerContext {
        let origin = app.world().get::<Transform>(caster).unwrap().translation;
        TriggerContext::new(
            trigger,
            EffectSource::Item(ItemId::HeartwoodBand),
            caster,
            Team::Radiant,
            1,
            origin,
            origin,
        )
        .trigger_unit(Some(target))
    }

    fn fire(app: &mut App, ctx: TriggerContext) {
        app.world_mut().write_message(AbilityTriggerEvent(ctx));
    }

    #[test]
    fn reaction_context_stamps_damage_status_and_drain_events() {
        let stun = AbilityEffect::ApplyStatus {
            status: StatusSpec::Stun,
            duration: RankValue::fixed(1.0),
        };
        let burn = AbilityEffect::DrainMana {
            amount: RankValue::fixed(10.0),
            burn_ratio: 1.0,
            damage_type: DamageType::Magical,
            transfer_ratio: 0.0,
        };
        let mut entries = Vec::new();
        for trigger in [AbilityTrigger::OnDamageTaken, AbilityTrigger::OnAttackHit] {
            for effect in [fixed_magical_damage(10.0), stun, burn] {
                entries.push(entry(trigger, EffectTarget::TriggerUnit, effect));
            }
        }
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(entries)]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        for (trigger, reactive) in [
            (AbilityTrigger::OnDamageTaken, true),
            (AbilityTrigger::OnAttackHit, false),
        ] {
            let ctx = item_ctx(&app, caster, trigger, target);
            assert_eq!(ctx.reaction, reactive);
            fire(&mut app, ctx);
            step(&mut app);

            let damage = current_messages::<DamageEvent>(&app);
            let drains = current_messages::<vitals::ResourceDrainEvent>(&app);
            let statuses = current_messages::<StatusEffectEvent>(&app);
            assert!(!damage.is_empty() && !drains.is_empty() && !statuses.is_empty());
            assert!(damage.iter().all(|e| e.reactive == reactive), "{trigger:?}");
            assert!(drains.iter().all(|e| e.reactive == reactive), "{trigger:?}");
            for event in &statuses {
                assert_eq!(event.reactive, reactive, "{trigger:?}");
                let crate::status::StatusChange::Apply(built) = &event.change else {
                    panic!("expected an apply");
                };
                assert_eq!(built.reactive, reactive, "the status itself is stamped");
            }
        }
    }

    #[test]
    fn perform_attack_is_inert_under_reaction() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            entry(
                AbilityTrigger::OnDamageTaken,
                EffectTarget::TriggerUnit,
                perform_attack(0, false),
            ),
            entry(
                AbilityTrigger::OnAttackHit,
                EffectTarget::TriggerUnit,
                perform_attack(0, false),
            ),
        ])]);
        let (caster, target) = attack_duel(&mut app);

        let ctx = item_ctx(&app, caster, AbilityTrigger::OnDamageTaken, target);
        fire(&mut app, ctx);
        step(&mut app);
        assert!(
            extra_attack_events(&app).is_empty(),
            "a reaction never attacks"
        );

        let ctx = item_ctx(&app, caster, AbilityTrigger::OnAttackHit, target);
        fire(&mut app, ctx);
        step(&mut app);
        assert_eq!(
            extra_attack_events(&app).len(),
            1,
            "the same effect works outside"
        );
    }

    #[test]
    fn attack_hit_amount_is_the_landed_damage() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([attack_hit_heal_item()]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        write_attack_hit(&mut app, caster, target);
        step(&mut app);

        let contexts = current_messages::<AbilityTriggerEvent>(&app);
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].0.amount, 10.0);
        assert!(!contexts[0].0.reaction);
    }

    // --- Damage triggers ---

    /// Run `frames` frames, collecting every trigger context that was written.
    fn run_collecting(app: &mut App, frames: usize) -> Vec<TriggerContext> {
        let mut seen = Vec::new();
        for _ in 0..frames {
            step(app);
            seen.extend(
                current_messages::<AbilityTriggerEvent>(app)
                    .into_iter()
                    .map(|event| event.0),
            );
        }
        seen
    }

    fn of_trigger(contexts: &[TriggerContext], trigger: AbilityTrigger) -> Vec<&TriggerContext> {
        contexts.iter().filter(|c| c.trigger == trigger).collect()
    }

    /// A unit holding the (overridden) test item.
    fn spawn_item_holder(app: &mut App, team: Team, pos: Vec3) -> Entity {
        let unit = spawn_unit(app, team, pos);
        app.world_mut().entity_mut(unit).insert(Inventory::empty());
        give_item(app, unit, ItemId::HeartwoodBand);
        unit
    }

    fn marker_effect(trigger: AbilityTrigger) -> AbilityEffectEntry {
        entry(trigger, EffectTarget::Caster, fixed_magical_damage(1.0))
    }

    fn strike(app: &mut App, source: Option<Entity>, target: Entity, amount: f32) {
        app.world_mut().write_message(match source {
            Some(source) => DamageEvent::spell(source, target, amount, DamageType::Pure),
            None => DamageEvent::new(target, amount, DamageType::Pure),
        });
    }

    #[test]
    fn damage_taken_fires_with_attacker_as_trigger_unit() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![marker_effect(
            AbilityTrigger::OnDamageTaken,
        )])]);
        let victim = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let attacker = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        strike(&mut app, Some(attacker), victim, 100.0);
        step(&mut app);
        assert!(
            current_messages::<AbilityTriggerEvent>(&app).is_empty(),
            "reactions resolve one frame after the damage"
        );
        let fired = run_collecting(&mut app, 1);
        let taken = of_trigger(&fired, AbilityTrigger::OnDamageTaken);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].caster, victim);
        assert_eq!(taken[0].trigger_unit, Some(attacker));
        assert_eq!(taken[0].amount, 100.0);
        assert!(taken[0].reaction);
    }

    #[test]
    fn damage_dealt_fires_for_source_owner_next_frame() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![marker_effect(
            AbilityTrigger::OnDamageDealt,
        )])]);
        let attacker = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        strike(&mut app, Some(attacker), victim, 60.0);
        step(&mut app);
        let fired = run_collecting(&mut app, 1);
        let dealt = of_trigger(&fired, AbilityTrigger::OnDamageDealt);
        assert_eq!(dealt.len(), 1);
        assert_eq!(dealt[0].caster, attacker);
        assert_eq!(dealt[0].trigger_unit, Some(victim));
        assert_eq!(dealt[0].point, Vec3::new(80.0, 0.0, 0.0));
        assert_eq!(dealt[0].amount, 60.0);
    }

    #[test]
    fn reactive_damage_fires_no_damage_triggers() {
        // Two units wearing thorns: each reflects damage taken at its attacker.
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            entry(
                AbilityTrigger::OnDamageTaken,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(10.0),
            ),
            marker_effect(AbilityTrigger::OnDamageDealt),
        ])]);
        let a = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let b = spawn_item_holder(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        strike(&mut app, Some(a), b, 100.0);
        run_collecting(&mut app, 10);

        // b took the opening hit (100). a took exactly one thorn (10) plus the one-point
        // marker its own on-dealt entry deals to itself: the thorn is reactive, so neither
        // a's thorns nor b's on-dealt entry answer it, and nothing ping-pongs.
        assert_eq!(health(&app, b), 900.0);
        assert_eq!(health(&app, a), 989.0);
    }

    #[test]
    fn dot_ticks_do_not_fire_damage_taken() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(AbilityTrigger::OnDamageTaken),
            marker_effect(AbilityTrigger::OnDamageDealt),
        ])]);
        let victim = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let source = spawn_item_holder(&mut app, Team::Dire, Vec3::X);

        app.world_mut().write_message(DamageEvent::dot(
            Some(source),
            victim,
            50.0,
            DamageType::Magical,
        ));
        let fired = run_collecting(&mut app, 3);
        assert_eq!(health(&app, victim), 950.0, "the tick landed");
        assert!(fired.is_empty(), "but fired nothing: {fired:?}");
    }

    #[test]
    fn fully_absorbed_by_invulnerability_fires_nothing() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(AbilityTrigger::OnDamageTaken),
            marker_effect(AbilityTrigger::OnDamageDealt),
        ])]);
        let victim = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let attacker = spawn_item_holder(&mut app, Team::Dire, Vec3::X);
        app.world_mut()
            .get_mut::<StatusEffects>(victim)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::invulnerable(10.0));

        strike(&mut app, Some(attacker), victim, 500.0);
        let fired = run_collecting(&mut app, 3);
        assert_eq!(health(&app, victim), 1000.0);
        assert!(fired.is_empty(), "{fired:?}");
    }

    #[test]
    fn kill_and_death_fire_once_with_killer_attribution() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(AbilityTrigger::OnKill),
            marker_effect(AbilityTrigger::OnDeath),
        ])]);
        let killer = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let victim = spawn_item_holder(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(victim).unwrap().current = 100.0;

        // Overkill, then a second hit on the corpse in the same batch.
        strike(&mut app, Some(killer), victim, 500.0);
        strike(&mut app, Some(killer), victim, 500.0);
        let fired = run_collecting(&mut app, 5);

        let kills = of_trigger(&fired, AbilityTrigger::OnKill);
        assert_eq!(kills.len(), 1, "once per death");
        assert_eq!(kills[0].caster, killer);
        assert_eq!(kills[0].trigger_unit, Some(victim));
        assert_eq!(kills[0].point, Vec3::new(80.0, 0.0, 0.0));
        let deaths = of_trigger(&fired, AbilityTrigger::OnDeath);
        assert_eq!(deaths.len(), 1);
        assert_eq!(deaths[0].caster, victim);
        assert_eq!(deaths[0].trigger_unit, Some(killer));
    }

    #[test]
    fn dot_kill_credits_dot_source() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(AbilityTrigger::OnKill),
            marker_effect(AbilityTrigger::OnDeath),
        ])]);
        let source = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let victim = spawn_item_holder(&mut app, Team::Dire, Vec3::X);
        app.world_mut().get_mut::<Health>(victim).unwrap().current = 10.0;

        app.world_mut().write_message(DamageEvent::dot(
            Some(source),
            victim,
            50.0,
            DamageType::Magical,
        ));
        let fired = run_collecting(&mut app, 3);
        let kills = of_trigger(&fired, AbilityTrigger::OnKill);
        assert_eq!(kills.len(), 1, "a DoT kill still counts");
        assert_eq!(kills[0].caster, source);
        assert_eq!(of_trigger(&fired, AbilityTrigger::OnDeath).len(), 1);
    }

    fn below(percent: u8) -> AbilityTrigger {
        AbilityTrigger::OnHealthBelow { percent }
    }

    #[test]
    fn health_below_fires_on_crossing_only_and_rearms_after_heal() {
        let mut app = test_app();
        let _defs =
            install_test_item_definitions([item_with_effects(vec![marker_effect(below(50))])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let foe = spawn_unit(&mut app, Team::Dire, Vec3::X);
        let mut crossings = |app: &mut App, damage: f32| {
            strike(app, Some(foe), unit, damage);
            step(app);
            run_collecting(app, 1).len()
        };

        assert_eq!(crossings(&mut app, 400.0), 0, "60% is still above the line");
        assert_eq!(crossings(&mut app, 200.0), 1, "40%: crossed");
        assert_eq!(
            crossings(&mut app, 100.0),
            0,
            "30%: already below, no refire"
        );

        app.world_mut().get_mut::<Health>(unit).unwrap().current = 800.0;
        assert_eq!(crossings(&mut app, 400.0), 1, "healed above, so it rearmed");
    }

    #[test]
    fn lethal_hit_does_not_fire_health_below() {
        let mut app = test_app();
        let _defs =
            install_test_item_definitions([item_with_effects(vec![marker_effect(below(50))])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let foe = spawn_unit(&mut app, Team::Dire, Vec3::X);

        strike(&mut app, Some(foe), unit, 5000.0);
        let fired = run_collecting(&mut app, 3);
        assert!(health(&app, unit) <= 0.0);
        assert!(of_trigger(&fired, below(50)).is_empty());
    }

    // --- Gates: pseudo-random procs and accumulators ---

    fn accumulate_entry(threshold: f32, then: &'static str) -> AbilityEffectEntry {
        entry(
            AbilityTrigger::OnAttackHit,
            EffectTarget::Caster,
            AbilityEffect::Accumulate {
                threshold: RankValue::fixed(threshold),
                then: AbilityTrigger::Custom(then),
            },
        )
    }

    #[test]
    fn accumulate_on_count_trigger_equals_counter() {
        // OnAttack carries the default amount of 1 per occurrence, so accumulating to 3 is
        // "every third attack".
        let heals = |gate: AbilityEffect| {
            let mut app = test_app();
            let _defs = install_test_item_definitions([item_with_effects(vec![
                entry(AbilityTrigger::OnAttack, EffectTarget::Caster, gate),
                heal_entry(AbilityTrigger::Custom("on_third"), 50.0),
            ])]);
            let (caster, target) = attack_duel(&mut app);
            let mut totals = Vec::new();
            for _ in 0..7 {
                write_attack_release(&mut app, caster, target);
                step(&mut app);
                totals.push(health(&app, caster));
            }
            totals
        };
        let third = AbilityTrigger::Custom("on_third");
        let accumulated = heals(AbilityEffect::Accumulate {
            threshold: RankValue::fixed(3.0),
            then: third,
        });
        assert_eq!(
            accumulated,
            heals(AbilityEffect::Counter {
                every: 3,
                then: third
            })
        );
        assert_eq!(accumulated[2] - accumulated[1], 50.0, "fires on the third");
    }

    #[test]
    fn accumulate_sums_attack_damage() {
        // write_attack_hit lands 10 damage: a threshold of 25 fires on the third hit.
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            accumulate_entry(25.0, "on_full"),
            heal_entry(AbilityTrigger::Custom("on_full"), 50.0),
        ])]);
        let (caster, target) = attack_duel(&mut app);
        land_hits(&mut app, caster, target, 2);
        assert_eq!(health(&app, caster), 100.0);
        land_hits(&mut app, caster, target, 1);
        assert_eq!(health(&app, caster), 150.0);
    }

    #[test]
    fn gates_respect_max_depth() {
        // Every gate kind re-fires its own custom trigger. Depth 1 and 2 run their entries;
        // the gate at depth 2 stops the chain, whichever kind it is.
        let kinds: [(&str, AbilityEffect); 3] = [
            (
                "pseudo",
                AbilityEffect::Proc {
                    chance: RankValue::fixed(1.0),
                    then: AbilityTrigger::Custom("on_loop"),
                    pseudo: true,
                },
            ),
            (
                "accumulate",
                AbilityEffect::Accumulate {
                    threshold: RankValue::fixed(1.0),
                    then: AbilityTrigger::Custom("on_loop"),
                },
            ),
            (
                "counter",
                AbilityEffect::Counter {
                    every: 1,
                    then: AbilityTrigger::Custom("on_loop"),
                },
            ),
        ];
        for (name, gate) in kinds {
            let mut app = test_app();
            let _defs = install_test_item_definitions([item_with_effects(vec![
                entry(AbilityTrigger::OnAttackHit, EffectTarget::Caster, gate),
                entry(
                    AbilityTrigger::Custom("on_loop"),
                    EffectTarget::Caster,
                    gate,
                ),
                heal_entry(AbilityTrigger::Custom("on_loop"), 10.0),
            ])]);
            let (caster, target) = attack_duel(&mut app);
            land_hits(&mut app, caster, target, 1);
            assert_eq!(health(&app, caster), 100.0 + 2.0 * 10.0, "{name}");
        }
    }

    // --- Gates: require_status ---

    fn require_status_entry(status: &'static str, then: &'static str) -> AbilityEffectEntry {
        entry(
            AbilityTrigger::OnAttackHit,
            EffectTarget::Caster,
            AbilityEffect::RequireStatus {
                id: status,
                then: AbilityTrigger::Custom(then),
            },
        )
    }

    fn hold_status(app: &mut App, unit: Entity, id: &'static str, remaining: f32) {
        app.world_mut()
            .get_mut::<StatusEffects>(unit)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::buff(id, remaining));
    }

    #[test]
    fn require_status_fires_only_while_status_held() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            require_status_entry("gate_status", "on_gated"),
            heal_entry(AbilityTrigger::Custom("on_gated"), 10.0),
        ])]);
        let (caster, target) = attack_duel(&mut app);

        land_hits(&mut app, caster, target, 2);
        assert_eq!(
            health(&app, caster),
            100.0,
            "no status: the gate stays shut"
        );

        hold_status(&mut app, caster, "gate_status", 0.3);
        land_hits(&mut app, caster, target, 1);
        assert_eq!(health(&app, caster), 110.0, "status held: the gate opens");
        // Held again on the next hit: one firing per hit, no stacking or counters.
        land_hits(&mut app, caster, target, 1);
        assert_eq!(health(&app, caster), 120.0);

        // The status runs out (the test app has no status ticker, so expire it by hand): the
        // gate closes again.
        for status in &mut app
            .world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
        {
            status.remaining = 0.0;
        }
        let before = health(&app, caster);
        land_hits(&mut app, caster, target, 2);
        assert_eq!(health(&app, caster), before);
    }

    #[test]
    fn require_status_gate_respects_depth_cap() {
        // The gate re-fires its own custom trigger. Depth 1 and 2 run their entries; the gate
        // at depth 2 stops the chain, exactly like the other gate kinds.
        let mut app = test_app();
        let looping = AbilityEffect::RequireStatus {
            id: "gate_status",
            then: AbilityTrigger::Custom("on_loop"),
        };
        let _defs = install_test_item_definitions([item_with_effects(vec![
            entry(AbilityTrigger::OnAttackHit, EffectTarget::Caster, looping),
            entry(
                AbilityTrigger::Custom("on_loop"),
                EffectTarget::Caster,
                looping,
            ),
            heal_entry(AbilityTrigger::Custom("on_loop"), 10.0),
        ])]);
        let (caster, target) = attack_duel(&mut app);
        hold_status(&mut app, caster, "gate_status", 10.0);
        land_hits(&mut app, caster, target, 1);
        assert_eq!(health(&app, caster), 100.0 + 2.0 * 10.0);
    }

    #[test]
    fn item_and_ability_require_status_alike() {
        // Item: the gate under an attack hit.
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            require_status_entry("gate_status", "on_gated"),
            heal_entry(AbilityTrigger::Custom("on_gated"), 10.0),
        ])]);
        let (caster, target) = attack_duel(&mut app);
        hold_status(&mut app, caster, "gate_status", 10.0);
        land_hits(&mut app, caster, target, 1);
        assert_eq!(health(&app, caster), 110.0, "item");

        // Ability: the same gate under its cast.
        let mut app = test_app();
        with_definitions(
            &mut app,
            [AbilityDefinition::new(AbilityId::Bulwark)
                .targeting(TargetType::NoTarget, RankValue::ZERO, RankValue::ZERO)
                .costs(RankValue::ZERO, RankValue::ZERO)
                .timing(0.0, 0.0)
                .on(
                    AbilityTrigger::OnCast,
                    EffectTarget::Caster,
                    AbilityEffect::RequireStatus {
                        id: "gate_status",
                        then: AbilityTrigger::Custom("on_gated"),
                    },
                )
                .on(
                    AbilityTrigger::Custom("on_gated"),
                    EffectTarget::Caster,
                    AbilityEffect::Heal {
                        amount: RankValue::fixed(10.0),
                        scaling: EffectScaling::None,
                    },
                )],
        );
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 100.0;
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..6 {
            step(&mut app);
        }
        assert_eq!(health(&app, caster), 100.0, "ability without the status");
        hold_status(&mut app, caster, "gate_status", 10.0);
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..6 {
            step(&mut app);
        }
        assert_eq!(health(&app, caster), 110.0, "ability with the status");
    }

    // --- Cast triggers ---

    /// An instant no-target ability whose `OnCast` entries hit enemies within 500.
    fn nova(id: AbilityId, effect: AbilityEffect) -> AbilityDefinition {
        AbilityDefinition::new(id)
            .targeting(
                TargetType::NoTarget,
                RankValue::ZERO,
                RankValue::fixed(500.0),
            )
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::enemies_around(definition::AreaCenter::Caster),
                effect,
            )
    }

    fn with_definitions(app: &mut App, defs: impl IntoIterator<Item = AbilityDefinition>) {
        let mut registry = app.world_mut().resource_mut::<AbilityDefinitions>();
        for def in defs {
            registry.insert(def);
        }
    }

    #[test]
    fn spell_cast_fires_for_other_sources_not_the_cast_ability() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![marker_effect(
            AbilityTrigger::OnSpellCast,
        )])]);
        with_definitions(
            &mut app,
            [
                nova(AbilityId::Bulwark, fixed_magical_damage(1.0)).on(
                    AbilityTrigger::OnSpellCast,
                    EffectTarget::Caster,
                    fixed_magical_damage(1.0),
                ),
                AbilityDefinition::new(AbilityId::Shockwave).on(
                    AbilityTrigger::OnSpellCast,
                    EffectTarget::Caster,
                    fixed_magical_damage(1.0),
                ),
            ],
        );
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::None);
        let fired = run_collecting(&mut app, 8);

        let casts = of_trigger(&fired, AbilityTrigger::OnSpellCast);
        let sources: Vec<_> = casts.iter().map(|c| c.source).collect();
        assert_eq!(
            sources,
            vec![
                EffectSource::Item(ItemId::HeartwoodBand),
                EffectSource::Ability(AbilityId::Shockwave)
            ],
            "the item and the other ability react; the cast ability does not"
        );
        assert!(casts.iter().all(|c| c.caster == caster));
        assert!(health(&app, enemy) < 1000.0, "the cast itself resolved");
    }

    #[test]
    fn item_use_is_not_a_spell_cast() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(AbilityTrigger::OnUse),
            marker_effect(AbilityTrigger::OnSpellCast),
        ])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);

        request_item(&mut app, caster, 0, CastTarget::None);
        let fired = run_collecting(&mut app, 8);
        assert_eq!(of_trigger(&fired, AbilityTrigger::OnUse).len(), 1);
        assert!(of_trigger(&fired, AbilityTrigger::OnSpellCast).is_empty());
    }

    #[test]
    fn spell_hit_once_per_enemy_per_cast_context() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![marker_effect(
            AbilityTrigger::OnSpellHit,
        )])]);
        // Two entries hit every enemy in range: still one spell hit per enemy.
        with_definitions(
            &mut app,
            [nova(AbilityId::Bulwark, fixed_magical_damage(5.0)).on(
                AbilityTrigger::OnCast,
                EffectTarget::enemies_around(definition::AreaCenter::Caster),
                fixed_magical_damage(5.0),
            )],
        );
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let also_near = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 120.0));
        let _far = spawn_unit(&mut app, Team::Dire, Vec3::new(4000.0, 0.0, 0.0));
        let _ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(10.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::None);
        let fired = run_collecting(&mut app, 8);

        let hits = of_trigger(&fired, AbilityTrigger::OnSpellHit);
        let mut hit_units: Vec<_> = hits.iter().filter_map(|c| c.trigger_unit).collect();
        hit_units.sort();
        let mut expected = vec![near, also_near];
        expected.sort();
        assert_eq!(
            hit_units, expected,
            "each enemy once, no allies, nobody out of range"
        );
        assert!(
            hits.iter()
                .all(|c| c.caster == caster && c.reaction == false)
        );
    }

    #[test]
    fn stun_only_spell_counts_as_hit() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![marker_effect(
            AbilityTrigger::OnSpellHit,
        )])]);
        with_definitions(
            &mut app,
            [nova(
                AbilityId::Bulwark,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::Stun,
                    duration: RankValue::fixed(1.0),
                },
            )],
        );
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::None);
        let fired = run_collecting(&mut app, 8);
        let hits = of_trigger(&fired, AbilityTrigger::OnSpellHit);
        assert_eq!(hits.len(), 1, "no damage involved");
        assert_eq!(hits[0].trigger_unit, Some(enemy));
        assert_eq!(health(&app, enemy), 1000.0);
    }

    #[test]
    fn reactive_and_proc_contexts_never_spell_hit() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(AbilityTrigger::OnSpellHit),
            // Attack-class item entries are not spells either.
            entry(
                AbilityTrigger::OnAttackHit,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(1.0),
            ),
        ])]);
        let enemies_around = EffectTarget::enemies_around(definition::AreaCenter::Caster);
        with_definitions(
            &mut app,
            [
                // The cast only rolls a proc; the damage happens in the derived context.
                AbilityDefinition::new(AbilityId::Bulwark)
                    .targeting(
                        TargetType::NoTarget,
                        RankValue::ZERO,
                        RankValue::fixed(500.0),
                    )
                    .costs(RankValue::ZERO, RankValue::ZERO)
                    .timing(0.0, 0.0)
                    .on(
                        AbilityTrigger::OnCast,
                        EffectTarget::Caster,
                        AbilityEffect::Proc {
                            chance: RankValue::fixed(1.0),
                            then: AbilityTrigger::Custom("on_echo"),
                            pseudo: false,
                        },
                    )
                    .on(
                        AbilityTrigger::Custom("on_echo"),
                        enemies_around,
                        fixed_magical_damage(5.0),
                    ),
                // A reaction of an ability source.
                AbilityDefinition::new(AbilityId::Shockwave).on(
                    AbilityTrigger::OnDamageTaken,
                    EffectTarget::TriggerUnit,
                    fixed_magical_damage(5.0),
                ),
            ],
        );
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        request(&mut app, caster, 3, CastTarget::None);
        strike(&mut app, Some(enemy), caster, 10.0);
        write_attack_hit(&mut app, caster, enemy);
        let fired = run_collecting(&mut app, 10);

        assert!(
            health(&app, enemy) < 1000.0,
            "the echo and the thorns landed"
        );
        assert!(
            of_trigger(&fired, AbilityTrigger::OnSpellHit).is_empty(),
            "{:?}",
            of_trigger(&fired, AbilityTrigger::OnSpellHit)
        );
    }

    // --- Status triggers ---

    fn status_filter_item(trigger: AbilityTrigger) -> Vec<AbilityEffectEntry> {
        vec![marker_effect(trigger)]
    }

    fn send_status(
        app: &mut App,
        source: Option<Entity>,
        target: Entity,
        change: crate::status::StatusChange,
    ) {
        app.world_mut()
            .write_message(StatusEffectEvent::new(source, target, change));
    }

    #[test]
    fn status_applied_filter_debuff_ignores_buffs() {
        use crate::status::{StatusChange, StatusEffect};
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(status_filter_item(
            AbilityTrigger::OnStatusApplied(definition::StatusFilter::Debuff),
        ))]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let foe = spawn_unit(&mut app, Team::Dire, Vec3::X);

        send_status(
            &mut app,
            Some(holder),
            holder,
            StatusChange::Apply(StatusEffect::buff("haste", 5.0)),
        );
        assert!(
            run_collecting(&mut app, 3).is_empty(),
            "a buff is no debuff"
        );

        send_status(
            &mut app,
            Some(foe),
            holder,
            StatusChange::Apply(StatusEffect::stunned(5.0)),
        );
        let fired = run_collecting(&mut app, 3);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].caster, holder);
        assert_eq!(fired[0].trigger_unit, Some(foe), "the applier");
        let meta = fired[0].status.expect("status meta");
        assert_eq!(meta.id, "stun");
        assert_eq!(meta.kind, crate::status::StatusKind::Debuff);
        assert!(fired[0].reaction);
    }

    #[test]
    fn status_removed_on_expiry_and_dispel() {
        use crate::status::{DispelStrength, StatusChange, StatusEffect, StatusKind};
        let mut app = test_app();
        app.add_systems(
            Update,
            crate::status::tick_status_effects.before(reactive_triggers::status_triggers),
        );
        let _defs = install_test_item_definitions([item_with_effects(status_filter_item(
            AbilityTrigger::OnStatusRemoved(definition::StatusFilter::Id("stun")),
        ))]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);

        // Expiry: a 0.08 s stun is gone after two 0.05 s frames.
        send_status(
            &mut app,
            None,
            holder,
            StatusChange::Apply(StatusEffect::stunned(0.08)),
        );
        let fired = run_collecting(&mut app, 4);
        assert_eq!(fired.len(), 1, "{fired:?}");

        // Dispel.
        send_status(
            &mut app,
            None,
            holder,
            StatusChange::Apply(StatusEffect::stunned(10.0)),
        );
        step(&mut app);
        send_status(
            &mut app,
            None,
            holder,
            StatusChange::Dispel {
                kind: StatusKind::Debuff,
                strength: DispelStrength::Basic,
            },
        );
        let fired = run_collecting(&mut app, 3);
        assert_eq!(fired.len(), 1, "{fired:?}");

        // Other statuses do not match the id filter.
        send_status(
            &mut app,
            None,
            holder,
            StatusChange::Apply(StatusEffect::debuff("weak", 0.08)),
        );
        assert!(run_collecting(&mut app, 4).is_empty());
    }

    #[test]
    fn status_reaction_does_not_loop() {
        use crate::status::{StatusChange, StatusEffect};
        // Whenever afflicted, stun yourself (a reactive stun): afflicting would loop forever
        // if the reaction's own status fired status triggers again.
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![entry(
            AbilityTrigger::OnStatusApplied(definition::StatusFilter::Any),
            EffectTarget::Caster,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::Stun,
                duration: RankValue::fixed(100.0),
            },
        )])]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);

        send_status(
            &mut app,
            None,
            holder,
            StatusChange::Apply(StatusEffect::debuff("weak", 100.0)),
        );
        let fired = run_collecting(&mut app, 10);
        assert_eq!(fired.len(), 1, "one reaction, no loop: {fired:?}");
        let statuses = app.world().get::<StatusEffects>(holder).unwrap();
        assert!(statuses.is_stunned(), "the reaction itself took effect");
    }

    // --- Cooldown and interval triggers ---

    fn interval(millis: u32) -> AbilityTrigger {
        AbilityTrigger::OnInterval { millis }
    }

    fn set_item_cooldowns(app: &mut App, unit: Entity, remaining: f32) {
        let mut inventory = app.world_mut().get_mut::<Inventory>(unit).unwrap();
        for item in inventory.slots.iter_mut().flatten() {
            item.cooldown.remaining = remaining;
        }
    }

    #[test]
    fn cooldown_ready_fires_once_when_reaching_zero() {
        let mut app = test_app();
        app.add_systems(
            Update,
            tick_ability_cooldowns.before(reactive_triggers::cooldown_triggers),
        );
        let _defs = install_test_item_definitions([item_with_effects(vec![marker_effect(
            AbilityTrigger::OnCooldownReady,
        )])]);
        with_definitions(
            &mut app,
            [AbilityDefinition::new(AbilityId::Shockwave).on(
                AbilityTrigger::OnCooldownReady,
                EffectTarget::Caster,
                fixed_magical_damage(1.0),
            )],
        );
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        // Two copies, each on its own cooldown: still one trigger for the item.
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        set_item_cooldowns(&mut app, caster, 0.12);
        app.world_mut()
            .get_mut::<AbilityLoadout>(caster)
            .unwrap()
            .slots[0]
            .cooldown
            .remaining = 0.12;

        let fired = run_collecting(&mut app, 2);
        assert!(fired.is_empty(), "0.10 s of a 0.12 s cooldown: not yet");
        let fired = run_collecting(&mut app, 6);
        let ready = of_trigger(&fired, AbilityTrigger::OnCooldownReady);
        let sources: Vec<_> = ready.iter().map(|c| c.source).collect();
        assert_eq!(
            sources.len(),
            2,
            "each source once, not again on later frames"
        );
        assert!(sources.contains(&EffectSource::Item(ItemId::HeartwoodBand)));
        assert!(sources.contains(&EffectSource::Ability(AbilityId::Shockwave)));
    }

    #[test]
    fn zero_cooldown_never_fires_ready() {
        let mut app = test_app();
        app.add_systems(
            Update,
            tick_ability_cooldowns.before(reactive_triggers::cooldown_triggers),
        );
        let _defs = install_test_item_definitions([item_with_effects(vec![marker_effect(
            AbilityTrigger::OnCooldownReady,
        )])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        set_item_cooldowns(&mut app, caster, 0.0);
        assert!(run_collecting(&mut app, 6).is_empty());
    }

    #[test]
    fn interval_fires_every_period_and_pauses_while_dead() {
        let mut app = test_app();
        let _defs =
            install_test_item_definitions([item_with_effects(vec![marker_effect(interval(140))])]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);

        // 50 ms frames against a 140 ms period: fires on frames 3, 6 and 9.
        let pattern: Vec<usize> = (0..9).map(|_| run_collecting(&mut app, 1).len()).collect();
        assert_eq!(pattern, [0, 0, 1, 0, 0, 1, 0, 0, 1]);

        // A dead owner's timer neither fires nor advances (30 ms in when it died)...
        app.world_mut().get_mut::<Health>(holder).unwrap().current = 0.0;
        assert!(run_collecting(&mut app, 8).is_empty());
        // ...so after reviving it needs three more frames, not one.
        app.world_mut().get_mut::<Health>(holder).unwrap().current = 100.0;
        let pattern: Vec<usize> = (0..3).map(|_| run_collecting(&mut app, 1).len()).collect();
        assert_eq!(pattern, [0, 0, 1]);
    }

    #[test]
    fn two_copies_of_an_item_share_one_interval() {
        let mut app = test_app();
        let _defs =
            install_test_item_definitions([item_with_effects(vec![marker_effect(interval(100))])]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        give_item(&mut app, holder, ItemId::HeartwoodBand);

        let fired = run_collecting(&mut app, 10);
        assert_eq!(fired.len(), 5, "one trigger per 100 ms, not two");
        let tracking = app
            .world()
            .get::<reactive_triggers::TriggerTracking>(holder);
        assert_eq!(tracking.unwrap().intervals.len(), 1);
    }

    #[test]
    fn sold_item_interval_is_pruned() {
        let mut app = test_app();
        let _defs =
            install_test_item_definitions([item_with_effects(vec![marker_effect(interval(100))])]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        run_collecting(&mut app, 1);
        let tracked = |app: &App| {
            app.world()
                .get::<reactive_triggers::TriggerTracking>(holder)
                .unwrap()
                .intervals
                .len()
        };
        assert_eq!(tracked(&app), 1);

        app.world_mut().get_mut::<Inventory>(holder).unwrap().slots = Default::default();
        assert!(run_collecting(&mut app, 4).is_empty());
        assert_eq!(tracked(&app), 0, "the timer left with the item");
    }

    // --- Movement and proximity triggers ---

    fn far_gate(threshold: f32) -> AbilityEffectEntry {
        entry(
            AbilityTrigger::OnMove,
            EffectTarget::Caster,
            AbilityEffect::Accumulate {
                threshold: RankValue::fixed(threshold),
                then: AbilityTrigger::Custom("on_far"),
            },
        )
    }

    fn walk_to(app: &mut App, unit: Entity, x: f32) {
        app.world_mut()
            .get_mut::<Transform>(unit)
            .unwrap()
            .translation
            .x = x;
    }

    #[test]
    fn distance_traveled_fires_every_n_units_via_accumulate() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            far_gate(100.0),
            marker_effect(AbilityTrigger::Custom("on_far")),
        ])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        run_collecting(&mut app, 1); // the first frame only records the starting point

        let mut fired = Vec::new();
        for i in 1..=10 {
            walk_to(&mut app, unit, 30.0 * i as f32);
            fired.extend(run_collecting(&mut app, 1));
        }
        // Derived contexts never leave the resolver; each fire cost the unit one health.
        assert_eq!(health(&app, unit), 997.0, "300 units walked, one every 100");
        let moves = of_trigger(&fired, AbilityTrigger::OnMove);
        assert_eq!(moves.len(), 10);
        assert!(moves.iter().all(|c| (c.amount - 30.0).abs() < 1e-3));
        // Standing still is not movement.
        assert!(run_collecting(&mut app, 3).is_empty());
    }

    #[test]
    fn teleport_distance_not_counted() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            far_gate(100.0),
            marker_effect(AbilityTrigger::Custom("on_far")),
        ])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        run_collecting(&mut app, 1);

        walk_to(&mut app, unit, 5000.0);
        app.world_mut().write_message(TeleportCompletedEvent {
            target: unit,
            caster: unit,
            from: Vec3::ZERO,
            to: Vec3::new(5000.0, 0.0, 0.0),
        });
        assert!(
            run_collecting(&mut app, 2).is_empty(),
            "a jump is not a walk"
        );

        walk_to(&mut app, unit, 5040.0);
        let fired = run_collecting(&mut app, 1);
        assert_eq!(of_trigger(&fired, AbilityTrigger::OnMove)[0].amount, 40.0);
    }

    #[test]
    fn forced_movement_counts() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            far_gate(100.0),
            marker_effect(AbilityTrigger::Custom("on_far")),
        ])]);
        with_definitions(
            &mut app,
            [AbilityDefinition::new(AbilityId::Bulwark)
                .targeting(TargetType::Unit, RankValue::fixed(600.0), RankValue::ZERO)
                .costs(RankValue::ZERO, RankValue::ZERO)
                .timing(0.0, 0.0)
                .on(
                    AbilityTrigger::OnCast,
                    EffectTarget::CastTarget,
                    AbilityEffect::Displace {
                        kind: DisplacementKind::Pull,
                        distance: None,
                        duration: RankValue::fixed(0.3),
                        speed: None,
                        contact_radius: None,
                        forward_offset: None,
                        bound_stun: None,
                    },
                )],
        );
        let caster = spawn_caster(&mut app, KIT);
        let victim = spawn_item_holder(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        for unit in [caster, victim] {
            app.world_mut().entity_mut(unit).insert(BoundRadius(24.0));
        }

        request(&mut app, caster, 3, CastTarget::Unit(victim));
        let fired = run_collecting(&mut app, 30);
        let walked: f32 = of_trigger(&fired, AbilityTrigger::OnMove)
            .iter()
            .map(|c| c.amount)
            .sum();
        assert!((walked - 352.0).abs() < 1.0, "pulled 400 -> 48: {walked}");
        assert_eq!(
            health(&app, victim),
            997.0,
            "352 units pulled, one fire every 100"
        );
    }

    fn enter(radius: u32) -> AbilityTrigger {
        AbilityTrigger::OnEnemyEnter { radius }
    }

    fn leave(radius: u32) -> AbilityTrigger {
        AbilityTrigger::OnEnemyLeave { radius }
    }

    fn trigger_units(contexts: &[&TriggerContext]) -> Vec<Entity> {
        contexts.iter().filter_map(|c| c.trigger_unit).collect()
    }

    #[test]
    fn enemy_enter_and_leave_fire_once_in_entity_order() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(enter(600)),
            marker_effect(leave(600)),
        ])]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let first = spawn_unit(&mut app, Team::Dire, Vec3::new(2000.0, 0.0, 0.0));
        let second = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 2000.0));
        assert!(run_collecting(&mut app, 2).is_empty());

        // Both walk in on the same frame: one enter each, lowest Entity first, no repeats.
        walk_to(&mut app, first, 300.0);
        app.world_mut()
            .get_mut::<Transform>(second)
            .unwrap()
            .translation = Vec3::new(0.0, 0.0, 400.0);
        let fired = run_collecting(&mut app, 3);
        let enters = of_trigger(&fired, enter(600));
        let mut expected = vec![first, second];
        expected.sort();
        assert_eq!(trigger_units(&enters), expected);
        assert!(
            enters
                .iter()
                .all(|c| c.caster == holder && c.reaction == false)
        );
        assert!(of_trigger(&fired, leave(600)).is_empty());

        // And both walk out again.
        walk_to(&mut app, first, 3000.0);
        app.world_mut()
            .get_mut::<Transform>(second)
            .unwrap()
            .translation = Vec3::new(0.0, 0.0, 3000.0);
        let fired = run_collecting(&mut app, 3);
        assert_eq!(trigger_units(&of_trigger(&fired, leave(600))), expected);
        assert!(of_trigger(&fired, enter(600)).is_empty());
    }

    #[test]
    fn units_in_range_at_acquire_fire_enter() {
        let mut app = test_app();
        let _defs =
            install_test_item_definitions([item_with_effects(vec![marker_effect(enter(600))])]);
        let holder = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO);
        app.world_mut()
            .entity_mut(holder)
            .insert(Inventory::empty());
        let already_there = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        assert!(run_collecting(&mut app, 2).is_empty(), "nothing held yet");

        give_item(&mut app, holder, ItemId::HeartwoodBand);
        let fired = run_collecting(&mut app, 3);
        let enters = of_trigger(&fired, enter(600));
        assert_eq!(trigger_units(&enters), vec![already_there]);
    }

    #[test]
    fn dead_member_dropped_silently() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(enter(600)),
            marker_effect(leave(600)),
        ])]);
        let _holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let dying = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let despawning = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 100.0));
        let fired = run_collecting(&mut app, 2);
        assert_eq!(of_trigger(&fired, enter(600)).len(), 2);

        app.world_mut().get_mut::<Health>(dying).unwrap().current = 0.0;
        app.world_mut().despawn(despawning);
        assert!(
            of_trigger(&run_collecting(&mut app, 3), leave(600)).is_empty(),
            "death and despawn are not leaving"
        );
    }

    #[test]
    fn allies_never_tracked() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(enter(600)),
            marker_effect(leave(600)),
        ])]);
        let _holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(100.0, 0.0, 0.0));
        assert!(run_collecting(&mut app, 2).is_empty());
        walk_to(&mut app, ally, 5000.0);
        assert!(run_collecting(&mut app, 2).is_empty());
    }

    // --- Conditional bonuses ---

    fn while_entry(condition: definition::Condition, status: &'static str) -> AbilityEffectEntry {
        entry(
            AbilityTrigger::While(condition),
            EffectTarget::Caster,
            AbilityEffect::ApplyStatus {
                status: StatusSpec::armor_buff(status, RankValue::fixed(10.0)),
                duration: RankValue::ZERO,
            },
        )
    }

    fn armor(app: &App, unit: Entity) -> f32 {
        app.world().get::<CombatStats>(unit).unwrap().armor
    }

    fn has_status(app: &App, unit: Entity, id: &str) -> bool {
        app.world()
            .get::<StatusEffects>(unit)
            .unwrap()
            .effects
            .iter()
            .any(|e| e.id == id)
    }

    fn set_health(app: &mut App, unit: Entity, current: f32) {
        app.world_mut().get_mut::<Health>(unit).unwrap().current = current;
    }

    #[test]
    fn low_health_bonus_applies_below_and_reverts_above_exactly() {
        use definition::Condition;
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![while_entry(
            Condition::HealthBelow { percent: 50 },
            "berserk",
        )])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        step(&mut app);
        let base = armor(&app, unit);

        set_health(&mut app, unit, 500.0);
        step(&mut app);
        assert!(
            !has_status(&app, unit, "berserk"),
            "exactly 50% is not below 50%"
        );

        set_health(&mut app, unit, 499.0);
        step(&mut app);
        assert!(has_status(&app, unit, "berserk"));
        assert_eq!(armor(&app, unit), base + 10.0);
        for _ in 0..3 {
            step(&mut app);
        }
        assert_eq!(
            armor(&app, unit),
            base + 10.0,
            "applied once, never stacked"
        );

        set_health(&mut app, unit, 500.0);
        step(&mut app);
        assert!(!has_status(&app, unit, "berserk"));
        assert_eq!(armor(&app, unit), base, "the stat reverts exactly");
    }

    #[test]
    fn moving_and_stationary_conditions() {
        use definition::Condition;
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            while_entry(Condition::Moving, "sprint"),
            while_entry(Condition::Stationary, "fortify"),
        ])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        step(&mut app);
        step(&mut app);
        assert!(has_status(&app, unit, "fortify") && !has_status(&app, unit, "sprint"));

        walk_to(&mut app, unit, 20.0);
        step(&mut app);
        assert!(has_status(&app, unit, "sprint") && !has_status(&app, unit, "fortify"));

        step(&mut app);
        assert!(!has_status(&app, unit, "sprint") && has_status(&app, unit, "fortify"));
    }

    #[test]
    fn enemies_near_counts_only_living_targetable_enemies() {
        use definition::Condition;
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![while_entry(
            Condition::EnemiesNear {
                radius: 600,
                at_least: 2,
            },
            "outnumbered",
        )])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let _alive = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let dead = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 100.0));
        set_health(&mut app, dead, 0.0);
        let lifted = spawn_unit(&mut app, Team::Dire, Vec3::new(-100.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(lifted)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::cyclone(100.0, true));
        let _far = spawn_unit(&mut app, Team::Dire, Vec3::new(3000.0, 0.0, 0.0));
        let _ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(0.0, 0.0, -100.0));
        step(&mut app);
        step(&mut app);
        assert!(
            !has_status(&app, unit, "outnumbered"),
            "one living, targetable enemy in range"
        );

        let _second = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -300.0));
        step(&mut app);
        assert!(has_status(&app, unit, "outnumbered"));
    }

    #[test]
    fn held_status_removed_when_item_sold() {
        use definition::Condition;
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![while_entry(
            Condition::Stationary,
            "fortify",
        )])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        step(&mut app);
        let with_bonus = armor(&app, unit);
        step(&mut app);
        assert!(has_status(&app, unit, "fortify"));

        app.world_mut().get_mut::<Inventory>(unit).unwrap().slots = Default::default();
        step(&mut app);
        assert!(
            !has_status(&app, unit, "fortify"),
            "never outlives its source"
        );
        assert_eq!(armor(&app, unit), with_bonus - 10.0);
    }

    #[test]
    fn held_status_is_undispellable_and_fires_no_status_triggers() {
        use crate::status::{DispelStrength, StatusChange, StatusKind};
        use definition::{Condition, StatusFilter};
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            while_entry(Condition::Stationary, "fortify"),
            marker_effect(AbilityTrigger::OnStatusApplied(StatusFilter::Any)),
            marker_effect(AbilityTrigger::OnStatusRemoved(StatusFilter::Any)),
        ])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let mut fired = run_collecting(&mut app, 3);
        assert!(has_status(&app, unit, "fortify"));

        send_status(
            &mut app,
            None,
            unit,
            StatusChange::Dispel {
                kind: StatusKind::Buff,
                strength: DispelStrength::Strong,
            },
        );
        fired.extend(run_collecting(&mut app, 3));
        assert!(has_status(&app, unit, "fortify"), "dispels cannot take it");

        app.world_mut().get_mut::<Inventory>(unit).unwrap().slots = Default::default();
        fired.extend(run_collecting(&mut app, 3));
        assert!(!has_status(&app, unit, "fortify"));
        assert!(
            fired.is_empty(),
            "applying and removing a held status is silent: {fired:?}"
        );
    }

    // --- Verification additions: guards, attribution, exactness ---

    /// Every context an event batch produced, as `(trigger, caster)` pairs for brevity.
    fn damage_taken_sources(contexts: &[TriggerContext]) -> Vec<EffectSource> {
        of_trigger(contexts, AbilityTrigger::OnDamageTaken)
            .iter()
            .map(|c| c.source)
            .collect()
    }

    #[test]
    fn item_copies_fire_once_and_items_precede_abilities() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![marker_effect(
            AbilityTrigger::OnDamageTaken,
        )])]);
        with_definitions(
            &mut app,
            [AbilityDefinition::new(AbilityId::Shockwave).on(
                AbilityTrigger::OnDamageTaken,
                EffectTarget::Caster,
                fixed_magical_damage(1.0),
            )],
        );
        let victim = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, victim, ItemId::HeartwoodBand);
        give_item(&mut app, victim, ItemId::HeartwoodBand);
        let foe = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        strike(&mut app, Some(foe), victim, 100.0);
        step(&mut app);
        let fired = run_collecting(&mut app, 1);
        assert_eq!(
            damage_taken_sources(&fired),
            vec![
                EffectSource::Item(ItemId::HeartwoodBand),
                EffectSource::Ability(AbilityId::Shockwave)
            ],
            "one trigger per distinct item, items first, then learned abilities"
        );
    }

    #[test]
    fn reflected_damage_is_reactive_and_credits_the_reflector() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(AbilityTrigger::OnDamageTaken),
            marker_effect(AbilityTrigger::OnDamageDealt),
            marker_effect(AbilityTrigger::OnKill),
            marker_effect(AbilityTrigger::OnDeath),
        ])]);
        let attacker = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let reflector = spawn_item_holder(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<CombatStats>(reflector)
            .unwrap()
            .damage_reflect = 1.0;
        app.world_mut().get_mut::<Health>(attacker).unwrap().current = 50.0;

        strike(&mut app, Some(attacker), reflector, 100.0);
        let fired = run_collecting(&mut app, 6);

        assert!(health(&app, attacker) <= 0.0, "the reflection was lethal");
        let taken = of_trigger(&fired, AbilityTrigger::OnDamageTaken);
        assert_eq!(taken.len(), 1, "only the opening hit counts as taken");
        assert_eq!(taken[0].caster, reflector);
        let dealt = of_trigger(&fired, AbilityTrigger::OnDamageDealt);
        assert_eq!(dealt.len(), 1, "reflected damage is never 'dealt'");
        assert_eq!(dealt[0].caster, attacker);
        let kills = of_trigger(&fired, AbilityTrigger::OnKill);
        assert_eq!(kills.len(), 1);
        assert_eq!(kills[0].caster, reflector, "the reflector gets the kill");
        assert_eq!(kills[0].trigger_unit, Some(attacker));
        let deaths = of_trigger(&fired, AbilityTrigger::OnDeath);
        assert_eq!(deaths.len(), 1);
        assert_eq!(deaths[0].caster, attacker);
        assert_eq!(deaths[0].trigger_unit, Some(reflector));
    }

    #[test]
    fn health_below_thresholds_fire_independently_once_per_crossing() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            marker_effect(below(75)),
            marker_effect(below(50)),
            marker_effect(below(25)),
        ])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let foe = spawn_unit(&mut app, Team::Dire, Vec3::X);
        let count = |fired: &[TriggerContext], percent: u8| of_trigger(fired, below(percent)).len();

        // 100% -> 40% in one hit crosses both 75 and 50, each once.
        strike(&mut app, Some(foe), unit, 600.0);
        step(&mut app);
        let fired = run_collecting(&mut app, 1);
        assert_eq!(
            (count(&fired, 75), count(&fired, 50), count(&fired, 25)),
            (1, 1, 0)
        );

        // Two hits in one frame over the 25% line: one crossing, not two. The second is
        // reactive DoT-kind damage, which health-below still sees but cannot repeat.
        strike(&mut app, Some(foe), unit, 200.0);
        app.world_mut().write_message(
            DamageEvent::dot(Some(foe), unit, 50.0, DamageType::Pure).reactive(true),
        );
        step(&mut app);
        let fired = run_collecting(&mut app, 1);
        assert_eq!(
            (count(&fired, 75), count(&fired, 50), count(&fired, 25)),
            (0, 0, 1)
        );

        // A reactive DoT tick alone is enough to cross: heal, then tick across 75.
        app.world_mut().get_mut::<Health>(unit).unwrap().current = 1000.0;
        app.world_mut().write_message(
            DamageEvent::dot(Some(foe), unit, 300.0, DamageType::Pure).reactive(true),
        );
        step(&mut app);
        let fired = run_collecting(&mut app, 1);
        assert_eq!(count(&fired, 75), 1, "kind and reactive flag are ignored");
    }

    #[test]
    fn interval_never_fires_twice_in_a_frame_after_a_long_hitch() {
        let mut app = test_app();
        let _defs =
            install_test_item_definitions([item_with_effects(vec![marker_effect(interval(100))])]);
        spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        run_collecting(&mut app, 1);

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(1.0));
        app.update();
        let first = current_messages::<AbilityTriggerEvent>(&app)
            .into_iter()
            .filter(|e| e.0.trigger == interval(100))
            .count();
        assert_eq!(first, 1, "a one second hitch is still one firing");
        for _ in 0..30 {
            step(&mut app);
            let n = current_messages::<AbilityTriggerEvent>(&app)
                .into_iter()
                .filter(|e| e.0.trigger == interval(100))
                .count();
            assert!(n <= 1, "at most once per frame");
        }
    }

    #[test]
    fn held_modifier_reverts_every_stat_exactly_across_cycles_and_sale() {
        use definition::Condition;
        let all_stats = AbilityEffectEntry {
            trigger: AbilityTrigger::While(Condition::HealthBelow { percent: 50 }),
            target: EffectTarget::Caster,
            effect: AbilityEffect::ApplyStatus {
                status: StatusSpec::Modifier {
                    id: "berserk_all",
                    kind: crate::status::StatusKind::Buff,
                    armor: RankValue::fixed(7.0),
                    magic_resist: RankValue::fixed(5.0),
                    move_speed: RankValue::fixed(40.0),
                    attack_damage: RankValue::fixed(33.0),
                    attack_speed_flat: RankValue::fixed(20.0),
                    attack_speed_mult: RankValue::fixed(0.35),
                    attack_range: RankValue::fixed(100.0),
                    attack_projectile_speed: RankValue::ZERO,
                    attack_damage_pct: RankValue::ZERO,
                    cleave: RankValue::ZERO,
                    cleave_radius: RankValue::ZERO,
                    cleave_end_radius: RankValue::ZERO,
                    cleave_distance: RankValue::ZERO,
                    attack_charges: RankValue::ZERO,
                    independent: false,
                },
                duration: RankValue::ZERO,
            },
        };
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![all_stats])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::components::HeroAttributes::starter());
        step(&mut app);
        let snapshot = |app: &App| format!("{:?}", app.world().get::<CombatStats>(unit).unwrap());
        // One throwaway cycle first: the first status change re-derives `attack_speed` from
        // the hero's agility, which is not part of what is being checked here.
        set_health(&mut app, unit, 100.0);
        step(&mut app);
        set_health(&mut app, unit, 900.0);
        step(&mut app);
        let base = snapshot(&app);

        for cycle in 0..5 {
            set_health(&mut app, unit, 100.0);
            step(&mut app);
            assert!(has_status(&app, unit, "berserk_all"), "cycle {cycle}");
            assert_ne!(snapshot(&app), base);
            step(&mut app);
            set_health(&mut app, unit, 900.0);
            step(&mut app);
            assert!(!has_status(&app, unit, "berserk_all"));
            assert_eq!(snapshot(&app), base, "cycle {cycle} left residue");
        }

        // Sold while active.
        set_health(&mut app, unit, 100.0);
        step(&mut app);
        assert!(has_status(&app, unit, "berserk_all"));
        app.world_mut().get_mut::<Inventory>(unit).unwrap().slots = Default::default();
        step(&mut app);
        assert!(!has_status(&app, unit, "berserk_all"));
        assert_eq!(snapshot(&app), base, "selling reverts exactly");
    }

    #[test]
    fn held_status_is_removed_when_its_owner_dies() {
        use definition::Condition;
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![while_entry(
            Condition::Stationary,
            "fortify",
        )])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        step(&mut app);
        step(&mut app);
        assert!(has_status(&app, unit, "fortify"));
        set_health(&mut app, unit, 0.0);
        step(&mut app);
        step(&mut app);
        assert!(!has_status(&app, unit, "fortify"), "a corpse holds nothing");
    }

    /// A chain started by a reaction: every hop inherits the reaction, so hop damage is
    /// reactive and fires no damage triggers (replaces the old `bolt_chain` custom test).
    #[test]
    fn chain_under_a_reaction_emits_reactive_hop_damage() {
        let mut app = test_app();
        let mut rows = vec![AbilityEffectEntry {
            trigger: AbilityTrigger::OnDamageTaken,
            target: EffectTarget::TriggerUnit,
            effect: AbilityEffect::Chain {
                jumps: 2,
                radius: RankValue::fixed(450.0),
                falloff: 0.75,
                team: TargetTeam::Enemy,
                then: AbilityTrigger::Custom("on_chain_hit"),
            },
        }];
        rows.push(AbilityEffectEntry {
            trigger: AbilityTrigger::Custom("on_chain_hit"),
            target: EffectTarget::TriggerUnit,
            effect: AbilityEffect::Damage {
                amount: RankValue::fixed(50.0),
                damage_type: DamageType::Magical,
                scaling: EffectScaling::None,
            },
        });
        let mut def = AbilityDefinition::new(AbilityId::Bolt);
        for row in rows {
            def = def.on(row.trigger, row.target, row.effect);
        }
        with_definitions(&mut app, [def]);
        let caster = spawn_caster(&mut app, KIT);
        let primary = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let _second = spawn_unit(&mut app, Team::Dire, Vec3::new(120.0, 0.0, 0.0));
        let ctx = TriggerContext::new(
            AbilityTrigger::OnDamageTaken,
            EffectSource::Ability(AbilityId::Bolt),
            caster,
            Team::Radiant,
            1,
            Vec3::ZERO,
            Vec3::ZERO,
        )
        .trigger_unit(Some(primary));
        assert!(ctx.reaction);
        fire(&mut app, ctx);
        step(&mut app);
        let damage = current_messages::<DamageEvent>(&app);
        assert!(!damage.is_empty(), "the chain jumped");
        assert!(
            damage.iter().all(|e| e.reactive),
            "chain damage under a reaction must be reactive: {damage:?}"
        );
    }

    #[test]
    fn production_plugins_build_an_acyclic_update_schedule() {
        // The test app re-declares the chain by hand; this builds the real plugins so a
        // cycle between `status_triggers` / `tick_status_effects` / the combat chain is
        // caught at graph-build time.
        let mut app = App::new();
        app.init_resource::<Time>().add_plugins((
            crate::combat::CombatPlugin,
            crate::status::StatusPlugin,
            AbilitiesPlugin,
        ));
        app.finish();
        app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule
                .initialize(world)
                .expect("the Update schedule must have no ordering cycle");
        });
    }

    /// A cyclone launched by a reaction applies a reactive status, so it fires no status
    /// triggers that could start another reaction.
    #[test]
    fn cyclone_launched_by_a_reaction_applies_a_reactive_status() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![entry(
            AbilityTrigger::OnDamageTaken,
            EffectTarget::TriggerUnit,
            AbilityEffect::Displace {
                kind: DisplacementKind::Cyclone,
                distance: None,
                duration: RankValue::fixed(1.0),
                speed: None,
                contact_radius: None,
                forward_offset: None,
                bound_stun: None,
            },
        )])]);
        let victim = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let attacker = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        strike(&mut app, Some(attacker), victim, 100.0);
        let mut statuses: Vec<StatusEffectEvent> = Vec::new();
        for _ in 0..4 {
            step(&mut app);
            statuses.extend(current_messages::<StatusEffectEvent>(&app));
        }
        assert!(
            statuses
                .iter()
                .any(|e| matches!(&e.change, crate::status::StatusChange::Apply(_))),
            "the cyclone status was applied"
        );
        assert!(
            statuses.iter().all(|e| e.reactive),
            "statuses started by a reaction must be reactive: {statuses:?}"
        );
    }

    /// Fires `trigger` for a dash item whose contact row damages enemies, with the owner
    /// aiming along +X, and returns the contact `DamageEvent`s seen over the dash.
    fn dash_contact_damage(trigger: AbilityTrigger) -> Vec<DamageEvent> {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            entry(
                trigger,
                EffectTarget::Caster,
                AbilityEffect::Displace {
                    kind: DisplacementKind::Dash,
                    distance: Some(RankValue::fixed(300.0)),
                    duration: RankValue::fixed(0.25),
                    speed: None,
                    contact_radius: None,
                    forward_offset: None,
                    bound_stun: None,
                },
            ),
            entry(
                AbilityTrigger::OnUnitContact,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(50.0),
            ),
        ])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(150.0, 0.0, 0.0));
        for unit in [caster, enemy] {
            app.world_mut().entity_mut(unit).insert(BoundRadius(30.0));
        }
        let mut ctx = item_ctx(&app, caster, trigger, enemy);
        ctx.aim = Vec3::new(1000.0, 0.0, 0.0);
        fire(&mut app, ctx);
        let mut seen = Vec::new();
        for _ in 0..12 {
            step(&mut app);
            seen.extend(current_messages::<DamageEvent>(&app));
        }
        assert!(
            app.world().get::<Transform>(caster).unwrap().translation.x > 100.0,
            "the dash ran"
        );
        seen
    }

    #[test]
    fn contact_damage_of_a_reaction_launched_dash_is_reactive() {
        let damage = dash_contact_damage(AbilityTrigger::OnDamageTaken);
        assert_eq!(damage.len(), 1, "one contact hit: {damage:?}");
        assert!(damage[0].reactive, "{damage:?}");
    }

    #[test]
    fn contact_damage_of_an_ordinary_dash_stays_non_reactive() {
        let damage = dash_contact_damage(AbilityTrigger::OnAttackHit);
        assert_eq!(damage.len(), 1, "one contact hit: {damage:?}");
        assert!(!damage[0].reactive, "{damage:?}");
    }

    #[test]
    fn ordinary_cyclone_status_stays_non_reactive() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![entry(
            AbilityTrigger::OnAttackHit,
            EffectTarget::TriggerUnit,
            AbilityEffect::Displace {
                kind: DisplacementKind::Cyclone,
                distance: None,
                duration: RankValue::fixed(1.0),
                speed: None,
                contact_radius: None,
                forward_offset: None,
                bound_stun: None,
            },
        )])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let ctx = item_ctx(&app, caster, AbilityTrigger::OnAttackHit, enemy);
        fire(&mut app, ctx);
        let mut statuses: Vec<StatusEffectEvent> = Vec::new();
        for _ in 0..3 {
            step(&mut app);
            statuses.extend(current_messages::<StatusEffectEvent>(&app));
        }
        let cyclone: Vec<_> = statuses
            .iter()
            .filter(
                |e| matches!(&e.change, crate::status::StatusChange::Apply(s) if s.id == "cyclone"),
            )
            .collect();
        assert_eq!(cyclone.len(), 1);
        assert!(!cyclone[0].reactive);
    }

    // --- Chain under the reaction rule, and Storm Mail parity ---

    /// `OnDamageTaken` -> chain from the attacker -> custom damage: the hop rows of a thorns
    /// style reaction.
    fn reaction_chain_rows(hop_damage: f32) -> Vec<AbilityEffectEntry> {
        vec![
            chain_entry(
                AbilityTrigger::OnDamageTaken,
                EffectTarget::TriggerUnit,
                2,
                0.75,
                "on_chain_hit",
            ),
            damage_entry(
                AbilityTrigger::Custom("on_chain_hit"),
                EffectTarget::TriggerUnit,
                hop_damage,
            ),
        ]
    }

    #[test]
    fn reactive_chain_between_holders_does_not_ping_pong() {
        // a and c (Radiant) and b (Dire) all wear the item. a strikes b; b reacts by chaining
        // from a to c. c is a holder too, but reactive hop damage fires no damage-taken
        // trigger, so nothing answers and the exchange ends.
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(reaction_chain_rows(50.0))]);
        let a = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let b = spawn_item_holder(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let c = spawn_item_holder(&mut app, Team::Radiant, Vec3::new(160.0, 0.0, 0.0));

        strike(&mut app, Some(a), b, 100.0);
        let mut applied = Vec::new();
        let mut taken = 0;
        for _ in 0..10 {
            step(&mut app);
            applied.extend(current_messages::<crate::combat::DamageAppliedEvent>(&app));
            taken += current_messages::<AbilityTriggerEvent>(&app)
                .iter()
                .filter(|e| e.0.trigger == AbilityTrigger::OnDamageTaken)
                .count();
        }
        assert_eq!(taken, 1, "only the original strike is a damage-taken event");
        let ordinary: Vec<_> = applied.iter().filter(|d| !d.reactive).collect();
        let reactive: Vec<_> = applied.iter().filter(|d| d.reactive).collect();
        assert_eq!(ordinary.len(), 1, "{applied:?}");
        assert_eq!(ordinary[0].target, b);
        assert_eq!(reactive.len(), 1, "one hop, then silence: {applied:?}");
        assert_eq!(reactive[0].target, c);
        assert!(reactive[0].is_spell);
        assert!(
            (health(&app, c) - (1000.0 - 50.0 * 0.75)).abs() < 1e-3,
            "hop 1 keeps 75%"
        );
        assert_eq!(health(&app, a), 1000.0, "the start unit is not re-fired");
    }

    #[test]
    fn chain_hop_damage_fires_damage_dealt_next_frame() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            chain_entry(
                AbilityTrigger::OnAttackHit,
                EffectTarget::TriggerUnit,
                1,
                1.0,
                "on_chain_hit",
            ),
            damage_entry(
                AbilityTrigger::Custom("on_chain_hit"),
                EffectTarget::TriggerUnit,
                20.0,
            ),
            marker_effect(AbilityTrigger::OnDamageDealt),
        ])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = enemy_at(&mut app, 80.0);
        let other = enemy_at(&mut app, 300.0);
        write_attack_hit(&mut app, caster, target);

        let mut hop_applied = None;
        let mut dealt_for_hop = None;
        for frame in 0..6 {
            step(&mut app);
            if app
                .world()
                .resource::<Messages<crate::combat::DamageAppliedEvent>>()
                .iter_current_update_messages()
                .any(|d| d.target == other)
            {
                hop_applied.get_or_insert(frame);
            }
            for e in current_messages::<AbilityTriggerEvent>(&app) {
                if e.0.trigger == AbilityTrigger::OnDamageDealt && e.0.trigger_unit == Some(other) {
                    assert!((e.0.amount - 20.0).abs() < 1e-3, "post-mitigation amount");
                    dealt_for_hop.get_or_insert(frame);
                }
            }
        }
        let (applied, dealt) = (
            hop_applied.expect("hop landed"),
            dealt_for_hop.expect("fired"),
        );
        assert_eq!(dealt, applied + 1, "damage dealt fires the frame after");
    }

    #[test]
    fn chain_hops_are_not_spell_hits() {
        let mut app = test_app();
        let mut def = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(
                TargetType::Unit,
                RankValue::fixed(scale::u(600.0)),
                RankValue::ZERO,
            )
            .costs(RankValue::fixed(8.0), RankValue::fixed(1.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                fixed_magical_damage(10.0),
            );
        for row in chain_rows(AbilityTrigger::OnCast) {
            def = def.on(row.trigger, row.target, row.effect);
        }
        with_definitions(&mut app, [def]);
        let caster = spawn_caster(&mut app, KIT);
        let (primary, n1, n2) = chain_line(&mut app);
        request(&mut app, caster, 3, CastTarget::Unit(primary));
        let mut hits = Vec::new();
        for _ in 0..8 {
            step(&mut app);
            hits.extend(current_messages::<effects::SpellHitEvent>(&app));
        }
        assert!(
            health(&app, n1) < 1000.0 && health(&app, n2) < 1000.0,
            "hopped"
        );
        assert_eq!(
            hits.iter().map(|h| h.target).collect::<Vec<_>>(),
            vec![primary],
            "only the depth-0 target is a spell hit"
        );
    }

    #[test]
    fn chain_hops_inherit_amount_reaction_and_status() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(reaction_chain_rows(1.0))]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let attacker = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let _other = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let mut ctx = item_ctx(&app, holder, AbilityTrigger::OnDamageTaken, attacker);
        ctx.amount = 321.0;
        let meta = effects::StatusMeta {
            id: "stun",
            kind: crate::status::StatusKind::Debuff,
        };
        ctx.status = Some(meta);
        let derived = ctx.derived(AbilityTrigger::Custom("on_chain_hit"));
        assert_eq!(derived.amount, 321.0);
        assert!(derived.reaction);
        assert_eq!(derived.status, Some(meta));
        assert_eq!(derived.depth, ctx.depth + 1);
    }

    #[test]
    fn accumulate_then_chain_under_damage_taken_resolves_within_depth_cap() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(storm_mail_rows())]);
        let holder = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let attacker = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(100_000.0, 0.0, 0.0));

        strike(&mut app, Some(attacker), holder, 299.0);
        run_collecting(&mut app, 4);
        assert_eq!(
            health(&app, near),
            1000.0,
            "below the threshold nothing arcs"
        );
        strike(&mut app, Some(attacker), holder, 1.0);
        // Derived contexts never leave the resolver, so the damage is what is observable.
        run_collecting(&mut app, 6);
        assert!(
            (health(&app, attacker) - 920.0).abs() < 1e-3,
            "80 to the attacker"
        );
        assert!((health(&app, near) - 940.0).abs() < 1e-3, "60 on the hop");
        assert_eq!(health(&app, far), 1000.0, "out of range");
    }

    /// Storm Mail's rows: every 300 damage taken overloads, striking the attacker and arcing
    /// to nearby enemies. Used as an item and as a passive ability.
    fn storm_mail_rows() -> Vec<AbilityEffectEntry> {
        vec![
            entry(
                AbilityTrigger::OnDamageTaken,
                EffectTarget::Caster,
                AbilityEffect::Accumulate {
                    threshold: RankValue::fixed(300.0),
                    then: AbilityTrigger::Custom("on_overload"),
                },
            ),
            chain_entry(
                AbilityTrigger::Custom("on_overload"),
                EffectTarget::TriggerUnit,
                2,
                0.75,
                "on_storm_arc",
            ),
            damage_entry(
                AbilityTrigger::Custom("on_overload"),
                EffectTarget::TriggerUnit,
                80.0,
            ),
            damage_entry(
                AbilityTrigger::Custom("on_storm_arc"),
                EffectTarget::TriggerUnit,
                80.0,
            ),
        ]
    }

    #[test]
    fn storm_mail_item_and_passive_ability_emit_identical_damage() {
        let run = |as_item: bool| {
            let mut app = test_app();
            let _defs = as_item
                .then(|| install_test_item_definitions([item_with_effects(storm_mail_rows())]));
            let holder = if as_item {
                spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO)
            } else {
                let mut def = AbilityDefinition::new(AbilityId::Bolt);
                for row in storm_mail_rows() {
                    def = def.on(row.trigger, row.target, row.effect);
                }
                with_definitions(&mut app, [def]);
                spawn_caster(&mut app, KIT)
            };
            let attacker = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
            let near = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
            let order = [holder, attacker, near];
            strike(&mut app, Some(attacker), holder, 300.0);
            let mut damage = Vec::new();
            for _ in 0..8 {
                step(&mut app);
                damage.extend(current_messages::<DamageEvent>(&app).into_iter().map(|d| {
                    (
                        order.iter().position(|e| *e == d.target),
                        d.amount,
                        d.damage_type,
                        d.is_spell,
                        d.reactive,
                    )
                }));
            }
            damage
        };
        let item = run(true);
        let ability = run(false);
        assert_eq!(item, ability);
        assert_eq!(
            item.iter().filter(|d| d.4).count(),
            2,
            "attacker and one hop, both reactive: {item:?}"
        );
    }

    #[test]
    fn item_projectile_use_is_neither_spell_cast_nor_spell_hit() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([projectile_item(vec![damage_entry(
            AbilityTrigger::OnProjectileHit,
            EffectTarget::TriggerUnit,
            75.0,
        )])]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        let target = enemy_at(&mut app, 300.0);
        let spawned = use_projectile_item(&mut app, caster, target);
        assert_eq!(spawned.len(), 1);
        app.world_mut().write_message(hit(
            spawned[0].payload,
            Vec3::X * 300.0,
            Some(target),
            Vec::new(),
        ));
        let mut casts = 0;
        let mut spell_hits = 0;
        let mut spell_triggers = 0;
        for _ in 0..4 {
            step(&mut app);
            casts += current_messages::<AbilityCastEvent>(&app).len();
            spell_hits += current_messages::<effects::SpellHitEvent>(&app).len();
            spell_triggers += current_messages::<AbilityTriggerEvent>(&app)
                .iter()
                .filter(|e| {
                    matches!(
                        e.0.trigger,
                        AbilityTrigger::OnSpellCast | AbilityTrigger::OnSpellHit
                    )
                })
                .count();
        }
        assert!(
            (health(&app, target) - 925.0).abs() < 1e-3,
            "the hit landed"
        );
        assert_eq!((casts, spell_hits, spell_triggers), (0, 0, 0));
    }

    #[test]
    fn while_status_is_built_like_a_toggle_status() {
        use crate::status::StatusChange;
        use definition::Condition;
        // A held burn and a toggled burn must agree on source, kind and tick damage.
        let mut app = test_app();
        let held = AbilityEffectEntry {
            trigger: AbilityTrigger::While(Condition::Stationary),
            target: EffectTarget::Caster,
            effect: AbilityEffect::ApplyStatus {
                status: StatusSpec::Burn {
                    id: "burn",
                    amount: RankValue::fixed(12.0),
                    damage_type: DamageType::Magical,
                    tick_interval: RankValue::ZERO,
                },
                duration: RankValue::ZERO,
            },
        };
        let _defs = install_test_item_definitions([item_with_effects(vec![held])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let mut seen = Vec::new();
        for _ in 0..4 {
            step(&mut app);
            seen.extend(current_messages::<StatusEffectEvent>(&app));
        }
        let applied: Vec<_> = seen
            .iter()
            .filter_map(|e| match &e.change {
                StatusChange::Apply(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(applied.len(), 1, "{seen:?}");
        let status = &applied[0];
        // Same builder: a held burn is attributed to its owner (like a toggle's), and reactive.
        assert_eq!(status.dot_source, Some(unit));
        assert!(status.reactive);
        assert_eq!(status.kind, crate::status::StatusKind::Debuff);
    }

    #[test]
    fn acts_on_targets_excludes_every_gate() {
        let then = AbilityTrigger::Custom("on_x");
        let gates = [
            AbilityEffect::Proc {
                chance: RankValue::fixed(0.5),
                then,
                pseudo: false,
            },
            AbilityEffect::Proc {
                chance: RankValue::fixed(0.5),
                then,
                pseudo: true,
            },
            AbilityEffect::Counter { every: 3, then },
            AbilityEffect::RequireStatus { id: "held", then },
            AbilityEffect::Accumulate {
                threshold: RankValue::fixed(10.0),
                then,
            },
            AbilityEffect::Chain {
                jumps: 1,
                radius: RankValue::fixed(100.0),
                falloff: 1.0,
                team: TargetTeam::Enemy,
                then,
            },
            AbilityEffect::PickRandom { count: 1, then },
        ];
        for gate in gates {
            assert!(!gate.acts_on_targets(), "{gate:?}");
        }
        assert!(fixed_magical_damage(1.0).acts_on_targets());
    }

    fn travel_item(with_teleport: bool) -> ItemDefinition {
        let mut rows = vec![
            far_gate(10.0),
            marker_effect(AbilityTrigger::Custom("on_far")),
        ];
        if with_teleport {
            rows.push(entry(
                AbilityTrigger::OnUse,
                EffectTarget::Caster,
                AbilityEffect::Teleport {
                    min_distance: RankValue::ZERO,
                    max_distance: None,
                },
            ));
        }
        let effects: &'static [AbilityEffectEntry] = Box::leak(rows.into_boxed_slice());
        ItemDefinition {
            effects,
            ..teleport_item()
        }
    }

    #[test]
    fn rejected_hostile_teleport_does_not_count_as_travel() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([travel_item(false)]);
        let victim = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, victim, ItemId::WardstoneCloak);
        app.world_mut().entity_mut(victim).insert(Team::Dire);
        let enemy = spawn_unit(&mut app, Team::Radiant, Vec3::new(500.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(victim)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::debuff_immunity(10.0));
        run_collecting(&mut app, 1);

        app.world_mut().write_message(TeleportEvent {
            target: victim,
            caster: enemy,
            destination: Vec3::new(3000.0, 0.0, 0.0),
        });
        let mut rejected = Vec::new();
        let mut fired = Vec::new();
        for _ in 0..4 {
            step(&mut app);
            rejected.extend(
                current_messages::<TeleportRejectedEvent>(&app)
                    .into_iter()
                    .map(|e| e.reason),
            );
            fired.extend(
                current_messages::<AbilityTriggerEvent>(&app)
                    .into_iter()
                    .map(|e| e.0),
            );
        }
        assert_eq!(rejected, vec![crate::teleport::TeleportError::Immune]);
        assert_eq!(
            app.world().get::<Transform>(victim).unwrap().translation,
            Vec3::ZERO,
            "the unit never moved"
        );
        assert!(of_trigger(&fired, AbilityTrigger::OnMove).is_empty());
    }

    #[test]
    fn blink_does_not_count_as_travel() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([travel_item(true)]);
        let unit = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, unit, ItemId::WardstoneCloak);
        app.world_mut()
            .entity_mut(unit)
            .insert(CollisionRadius(40.0));
        run_collecting(&mut app, 1);

        request_item(
            &mut app,
            unit,
            0,
            CastTarget::Point(Vec3::new(2000.0, 0.0, 0.0)),
        );
        let fired = run_collecting(&mut app, 8);
        assert_eq!(
            app.world().get::<Transform>(unit).unwrap().translation,
            Vec3::new(2000.0, 0.0, 0.0),
            "the blink happened"
        );
        assert!(of_trigger(&fired, AbilityTrigger::OnMove).is_empty());
    }

    /// A unit-targeted ally buff applying `status` for `duration` seconds from slot 3.
    fn ally_buff_ability(status: StatusSpec, duration: f32) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(800.0), RankValue::ZERO)
            .team(TargetTeam::Ally)
            .costs(RankValue::fixed(8.0), RankValue::fixed(10.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::ApplyStatus {
                    status,
                    duration: RankValue::fixed(duration),
                },
            )
    }

    fn pct_modifier(id: &'static str, pct: f32) -> StatusSpec {
        StatusSpec::Modifier {
            id,
            kind: crate::status::StatusKind::Buff,
            armor: RankValue::ZERO,
            magic_resist: RankValue::ZERO,
            move_speed: RankValue::ZERO,
            attack_damage: RankValue::ZERO,
            attack_speed_flat: RankValue::ZERO,
            attack_speed_mult: RankValue::ZERO,
            attack_range: RankValue::ZERO,
            attack_projectile_speed: RankValue::ZERO,
            attack_damage_pct: RankValue::fixed(pct),
            cleave: RankValue::ZERO,
            cleave_radius: RankValue::ZERO,
            cleave_end_radius: RankValue::ZERO,
            cleave_distance: RankValue::ZERO,
            attack_charges: RankValue::ZERO,
            independent: false,
        }
    }

    fn attack_damage(app: &App, unit: Entity) -> f32 {
        app.world().get::<CombatStats>(unit).unwrap().attack_damage
    }

    #[test]
    fn attack_damage_pct_snapshots_hero_def_damage_and_ignores_item_bonuses() {
        let mut app = qop_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ally_buff_ability(pct_modifier("empower_test", 50.0), 1.0));
        let caster = spawn_caster(&mut app, KIT);
        let hero_base = crate::heroes::HeroId::Vanguard
            .def()
            .level_one_attack_damage();

        // A hero holding +100 damage from items: the bonus is 50% of the definition's damage.
        let hero = spawn_unit(&mut app, Team::Radiant, Vec3::new(100.0, 0.0, 0.0));
        app.world_mut()
            .entity_mut(hero)
            .insert(crate::heroes::HeroKind(crate::heroes::HeroId::Vanguard));
        app.world_mut()
            .get_mut::<CombatStats>(hero)
            .unwrap()
            .attack_damage = hero_base + 100.0;
        request(&mut app, caster, 3, CastTarget::Unit(hero));
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(has_status(&app, hero, "empower_test"));
        assert!((attack_damage(&app, hero) - (hero_base + 100.0 + 0.5 * hero_base)).abs() < 1e-3);

        // Expiry reverts the snapshot exactly.
        for _ in 0..40 {
            step(&mut app);
        }
        assert!(!has_status(&app, hero, "empower_test"));
        assert!((attack_damage(&app, hero) - (hero_base + 100.0)).abs() < 1e-3);

        // A unit without a hero definition uses its current damage.
        let caster = spawn_caster(&mut app, KIT);
        let creep = spawn_unit(&mut app, Team::Radiant, Vec3::new(150.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<CombatStats>(creep)
            .unwrap()
            .attack_damage = 40.0;
        request(&mut app, caster, 3, CastTarget::Unit(creep));
        for _ in 0..12 {
            step(&mut app);
        }
        assert!((attack_damage(&app, creep) - 60.0).abs() < 1e-3);
    }

    /// A no-target ability that pulls every enemy in its 430 radius to `forward_offset` in
    /// front of the caster.
    fn gather_ability(forward_offset: Option<f32>) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(
                TargetType::NoTarget,
                RankValue::ZERO,
                RankValue::fixed(430.0),
            )
            .team(TargetTeam::Enemy)
            .costs(RankValue::fixed(8.0), RankValue::fixed(10.0))
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::UnitsInRadius {
                    center: super::definition::AreaCenter::Caster,
                    radius: super::definition::AreaRadius::Ability,
                    team: TargetTeam::Enemy,
                },
                AbilityEffect::Displace {
                    kind: DisplacementKind::PullToPoint,
                    distance: None,
                    duration: RankValue::fixed(0.05),
                    speed: None,
                    contact_radius: None,
                    forward_offset: forward_offset.map(RankValue::fixed),
                    bound_stun: None,
                },
            )
    }

    fn position(app: &App, unit: Entity) -> Vec3 {
        app.world().get::<Transform>(unit).unwrap().translation
    }

    #[test]
    fn pull_point_with_a_forward_offset_gathers_enemies_in_front_of_the_caster() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(gather_ability(Some(250.0)));
        let caster = spawn_caster(&mut app, KIT);
        // Face +X.
        app.world_mut()
            .get_mut::<Transform>(caster)
            .unwrap()
            .rotation = Transform::default().looking_to(Vec3::X, Vec3::Y).rotation;
        let behind = spawn_unit(&mut app, Team::Dire, Vec3::new(-200.0, 0.0, 0.0));
        let side = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 400.0));
        let ahead = spawn_unit(&mut app, Team::Dire, Vec3::new(420.0, 0.0, 0.0));
        let outside = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -520.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(0.0, 0.0, 300.0));
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..16 {
            step(&mut app);
        }
        let gather = Vec3::new(250.0, 0.0, 0.0);
        for unit in [behind, side, ahead] {
            assert!(
                position(&app, unit).distance(gather) < 1.0,
                "{unit:?} ended at {}",
                position(&app, unit)
            );
        }
        assert_eq!(position(&app, outside), Vec3::new(0.0, 0.0, -520.0));
        assert_eq!(position(&app, ally), Vec3::new(0.0, 0.0, 300.0));
        assert_eq!(position(&app, caster), Vec3::ZERO);
    }

    #[test]
    fn pull_point_without_an_offset_still_pulls_to_the_trigger_point() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(gather_ability(None));
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 400.0));
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..16 {
            step(&mut app);
        }
        // The trigger point of a no-target cast is the caster's own position.
        assert!(position(&app, enemy).distance(Vec3::ZERO) < 1.0);
    }

    #[test]
    fn forward_point_follows_the_flat_facing() {
        use super::effects::forward_point;
        let origin = Vec3::new(10.0, 3.0, 10.0);
        assert!(forward_point(origin, Vec3::X, 250.0).distance(Vec3::new(260.0, 3.0, 10.0)) < 1e-4);
        assert!(
            forward_point(origin, Vec3::new(0.0, 5.0, 2.0), 100.0)
                .distance(Vec3::new(10.0, 3.0, 110.0))
                < 1e-4
        );
        assert!(forward_point(origin, Vec3::Y, 100.0).distance(Vec3::new(10.0, 3.0, -90.0)) < 1e-4);
    }

    fn offset_wave(origin_offset: Option<f32>) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(
                TargetType::Point,
                RankValue::fixed(1200.0),
                RankValue::fixed(200.0),
            )
            .team(TargetTeam::Enemy)
            .costs(RankValue::fixed(8.0), RankValue::fixed(10.0))
            .mechanic(AbilityMechanic::Projectile {
                splash_radius: super::definition::AreaRadius::Ability,
                speed: Some(1200.0),
                skillshot: true,
                first_hit: false,
                end_radius: None,
                origin_offset: origin_offset.map(RankValue::fixed),
                seek: None,
            })
    }

    #[test]
    fn skillshot_origin_offset_starts_the_wave_in_front_and_keeps_the_full_range() {
        for (offset, expected_start) in [(Some(48.0), 48.0), (None, 0.0)] {
            let mut app = test_app();
            app.world_mut()
                .resource_mut::<AbilityDefinitions>()
                .insert(offset_wave(offset));
            let caster = spawn_caster(&mut app, KIT);
            let spawned =
                cast_for_projectiles(&mut app, caster, 3, CastTarget::Point(Vec3::X * 800.0));
            assert_eq!(spawned.len(), 1);
            let wave = spawned[0];
            assert!(
                wave.origin.distance(Vec3::new(expected_start, 0.0, 0.0)) < 1e-3,
                "starts at {} for {offset:?}",
                wave.origin
            );
            assert!(
                wave.target_pos
                    .distance(Vec3::new(expected_start + 1200.0, 0.0, 0.0))
                    < 1e-3,
                "flies the full cast range from the start: {}",
                wave.target_pos
            );
            assert_eq!(wave.skillshot_width, Some(200.0));
        }
    }

    #[test]
    fn skillshot_origin_offset_follows_the_aim_direction_not_the_facing() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(offset_wave(Some(48.0)));
        let caster = spawn_caster(&mut app, KIT);
        let spawned = cast_for_projectiles(&mut app, caster, 3, CastTarget::Point(Vec3::Z * 600.0));
        let wave = spawned[0];
        assert!(
            wave.origin.distance(Vec3::new(0.0, 0.0, 48.0)) < 1e-3,
            "{}",
            wave.origin
        );
        assert!(wave.target_pos.distance(Vec3::new(0.0, 0.0, 1248.0)) < 1e-3);
    }

    const MAGNUS: [AbilityId; 4] = [
        AbilityId::MagnusShockwave,
        AbilityId::Empower,
        AbilityId::Skewer,
        AbilityId::ReversePolarity,
    ];

    fn status_of<'a>(
        app: &'a App,
        unit: Entity,
        id: &str,
    ) -> Option<&'a crate::status::StatusEffect> {
        app.world()
            .get::<StatusEffects>(unit)
            .unwrap()
            .effects
            .iter()
            .find(|e| e.id == id)
    }

    #[test]
    fn magnus_shockwave_pulls_toward_wave_center_and_slows() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, MAGNUS);
        // Two enemies beside the wave's path: one close to the wave's centre, one far.
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 100.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 300.0));
        let spawned = cast_for_projectiles(&mut app, caster, 0, CastTarget::Point(Vec3::X * 800.0));
        assert_eq!(spawned.len(), 1);
        let wave = spawned[0];
        assert!(
            wave.origin.distance(Vec3::new(48.0, 0.0, 0.0)) < 1e-3,
            "the wave starts 48 units in front: {}",
            wave.origin
        );
        assert_eq!(wave.skillshot_width, Some(200.0));
        assert_eq!(wave.speed, Some(1200.0));

        let centre = Vec3::new(400.0, 0.0, 0.0);
        app.world_mut()
            .write_message(hit(wave.payload, centre, Some(near), vec![]));
        app.world_mut()
            .write_message(hit(wave.payload, centre, Some(far), vec![]));
        for _ in 0..6 {
            step(&mut app);
        }
        // The close unit stops at the centre; the far one is pulled the full 150.
        assert!(
            position(&app, near).distance(centre) < 1.0,
            "{}",
            position(&app, near)
        );
        assert!(
            (position(&app, far).distance(centre) - 150.0).abs() < 1.0,
            "{}",
            position(&app, far)
        );
        for unit in [near, far] {
            assert_eq!(
                health(&app, unit),
                1000.0 - 75.0,
                "75 magical, no resistance"
            );
            let slow = status_of(&app, unit, "magnus_shockwave_slow").expect("slowed");
            assert!(
                slow.remaining <= 0.55 && slow.remaining > 0.1,
                "{}",
                slow.remaining
            );
            assert!(
                (slow.move_speed + 300.0 * 0.75).abs() < 1e-3,
                "75% of 300: {}",
                slow.move_speed
            );
        }
        // The slow is brief: it is gone after another second.
        for _ in 0..20 {
            step(&mut app);
        }
        assert!(status_of(&app, near, "magnus_shockwave_slow").is_none());
    }

    #[test]
    fn skewer_drags_stunned_enemy_and_hits_at_destination() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, MAGNUS);
        let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 100.0));
        let bystander = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 400.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(300.0, 0.0, 50.0));
        // The victim already carries someone else's long stun.
        app.world_mut()
            .get_mut::<StatusEffects>(victim)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::stunned(30.0));
        request(&mut app, caster, 2, CastTarget::Point(Vec3::X * 600.0));

        let mut seen_bound = false;
        let mut carried_with_both_stuns = false;
        for _ in 0..80 {
            step(&mut app);
            if status_of(&app, victim, "skewer_stun").is_some() {
                seen_bound = true;
                carried_with_both_stuns |= status_of(&app, victim, "stun").is_some()
                    && app.world().get::<ForcedMovement>(victim).is_some();
            }
        }
        assert!(
            seen_bound,
            "the contact stunned the victim while it was dragged"
        );
        assert!(
            carried_with_both_stuns,
            "the foreign stun was never overwritten"
        );
        assert!(
            app.world().get::<ForcedMovement>(caster).is_none()
                && app.world().get::<ForcedMovement>(victim).is_none(),
            "the dash and the drag are over"
        );

        // At the destination: dragged along, the bound stun removed, the other stun intact.
        assert!(position(&app, caster).distance(Vec3::X * 600.0) < 2.0);
        assert!(position(&app, victim).distance(position(&app, caster)) < 2.0);
        assert!(status_of(&app, victim, "skewer_stun").is_none());
        assert!(
            status_of(&app, victim, "stun").is_some(),
            "the other stun is untouched"
        );
        // Damage and slow land at the destination.
        assert_eq!(health(&app, victim), 1000.0 - 80.0);
        let slow = status_of(&app, victim, "skewer_slow").expect("slowed at the end");
        assert!(
            (slow.move_speed + 300.0 * 0.20).abs() < 1e-3,
            "{}",
            slow.move_speed
        );
        assert!(
            slow.remaining > 0.0 && slow.remaining <= 3.25,
            "{}",
            slow.remaining
        );
        // Outside the 145 reach, and allies, are not touched.
        assert_eq!(health(&app, bystander), 1000.0);
        assert_eq!(position(&app, bystander), Vec3::new(300.0, 0.0, 400.0));
        assert_eq!(health(&app, ally), 1000.0);
        assert_eq!(position(&app, ally), Vec3::new(300.0, 0.0, 50.0));
    }

    #[test]
    fn reverse_polarity_pulls_damages_and_stuns_in_radius() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, MAGNUS);
        app.world_mut()
            .get_mut::<Transform>(caster)
            .unwrap()
            .rotation = Transform::default().looking_to(Vec3::X, Vec3::Y).rotation;
        let behind = spawn_unit(&mut app, Team::Dire, Vec3::new(-200.0, 0.0, 0.0));
        let side = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 400.0));
        let outside = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -500.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(100.0, 0.0, 100.0));
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..24 {
            step(&mut app);
        }
        let gather = Vec3::new(250.0, 0.0, 0.0);
        for unit in [behind, side] {
            assert!(
                position(&app, unit).distance(gather) < 1.0,
                "{}",
                position(&app, unit)
            );
            assert_eq!(health(&app, unit), 1000.0 - 100.0);
            let stun = status_of(&app, unit, "stun").expect("stunned");
            assert!(stun.stunned);
            assert!(
                stun.remaining <= 2.5 && stun.remaining > 1.0,
                "{}",
                stun.remaining
            );
        }
        assert_eq!(position(&app, outside), Vec3::new(0.0, 0.0, -500.0));
        assert_eq!(health(&app, outside), 1000.0);
        assert!(status_of(&app, outside, "stun").is_none());
        assert_eq!(position(&app, ally), Vec3::new(100.0, 0.0, 100.0));
        assert!(status_of(&app, ally, "stun").is_none());
    }

    #[test]
    fn empower_grants_percent_damage_and_cone_cleave_to_self_and_ally() {
        use crate::heroes::{HeroId, HeroKind};
        let mut app = qop_app();
        let magnus = spawn_caster(&mut app, MAGNUS);
        let helper = spawn_caster(&mut app, MAGNUS);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(150.0, 0.0, 0.0));
        for unit in [magnus, helper, ally] {
            app.world_mut()
                .entity_mut(unit)
                .insert(HeroKind(HeroId::Magnus));
            app.world_mut()
                .get_mut::<CombatStats>(unit)
                .unwrap()
                .attack_damage = 61.0 + 100.0; // 100 of it from items
        }

        // Self-cast, and a cast on an ally by someone else.
        request(&mut app, magnus, 1, CastTarget::Unit(magnus));
        request(&mut app, helper, 1, CastTarget::Unit(ally));
        for _ in 0..12 {
            step(&mut app);
        }
        for unit in [magnus, ally] {
            let stats = app.world().get::<CombatStats>(unit).unwrap();
            // 16% of the hero definition's 61, not of the 161 the unit currently has.
            assert!(
                (stats.attack_damage - (161.0 + 0.16 * 61.0)).abs() < 1e-3,
                "{}",
                stats.attack_damage
            );
            // The five Empower statuses form ONE cleave instance keyed by the ability.
            assert_eq!(stats.cleaves.iter().count(), 1);
            let cleave = stats
                .cleaves
                .get(crate::components::CleaveSource::Effect(
                    crate::abilities::effects::EffectSource::Ability(AbilityId::Empower),
                ))
                .expect("empower cleave instance");
            assert!((cleave.ratio - 0.16).abs() < 1e-6);
            assert_eq!(
                (cleave.radius, cleave.end_radius, cleave.distance),
                (150.0, 360.0, 650.0)
            );
        }

        // After the 30 seconds everything is exactly back.
        for _ in 0..620 {
            step(&mut app);
        }
        assert!(status_of(&app, ally, "empower_attack_damage_pct").is_none());
        let after = app.world().get::<CombatStats>(ally).unwrap();
        assert!((after.attack_damage - 161.0).abs() < 1e-3);
        assert!(
            after.cleaves.is_empty(),
            "the instance leaves with its last status"
        );
    }

    #[test]
    fn empower_recast_while_active_keeps_one_instance_and_dispel_clears_it() {
        use crate::components::CleaveSource;
        use crate::heroes::{HeroId, HeroKind};
        use crate::status::{DispelStrength, StatusChange, StatusEffectEvent, StatusKind};
        let mut app = qop_app();
        let a = spawn_caster(&mut app, MAGNUS);
        let b = spawn_caster(&mut app, MAGNUS);
        let target = spawn_unit(&mut app, Team::Radiant, Vec3::new(150.0, 0.0, 0.0));
        for unit in [a, b, target] {
            app.world_mut()
                .entity_mut(unit)
                .insert(HeroKind(HeroId::Magnus));
            app.world_mut()
                .get_mut::<CombatStats>(unit)
                .unwrap()
                .attack_damage = 161.0;
        }
        let key = CleaveSource::Effect(crate::abilities::effects::EffectSource::Ability(
            AbilityId::Empower,
        ));
        let expected_damage = 161.0 + 0.16 * 61.0;
        let check = |app: &App, label: &str| {
            let stats = app.world().get::<CombatStats>(target).unwrap();
            assert_eq!(stats.cleaves.iter().count(), 1, "{label}: one instance");
            let cone = stats.cleaves.get(key).expect("empower instance");
            assert!(
                (cone.ratio - 0.16).abs() < 1e-5,
                "{label}: ratio {}",
                cone.ratio
            );
            assert_eq!(
                (cone.radius, cone.end_radius, cone.distance),
                (150.0, 360.0, 650.0),
                "{label}: shape"
            );
            assert!(
                (stats.attack_damage - expected_damage).abs() < 1e-3,
                "{label}: damage {}",
                stats.attack_damage
            );
        };

        request(&mut app, a, 1, CastTarget::Unit(target));
        for _ in 0..12 {
            step(&mut app);
        }
        check(&app, "first cast");
        // A second caster re-casts it while the first is still running.
        request(&mut app, b, 1, CastTarget::Unit(target));
        for _ in 0..12 {
            step(&mut app);
        }
        check(&app, "recast while active");

        // A dispel (Strong, buffs) must remove statuses and the instance together.
        app.world_mut().write_message(StatusEffectEvent::new(
            None,
            target,
            StatusChange::Dispel {
                kind: StatusKind::Buff,
                strength: DispelStrength::Strong,
            },
        ));
        step(&mut app);
        let left = app
            .world()
            .get::<StatusEffects>(target)
            .unwrap()
            .effects
            .iter()
            .filter(|e| e.carries_cleave())
            .count();
        let stats = app.world().get::<CombatStats>(target).unwrap();
        assert_eq!(
            stats.cleaves.is_empty(),
            left == 0,
            "instance exists iff a cleave status remains"
        );
        if left == 0 {
            assert!((stats.attack_damage - 161.0).abs() < 1e-3);
        }
        // Explicit removal of every Empower status id also leaves nothing behind.
        let ids: Vec<&'static str> = app
            .world()
            .get::<StatusEffects>(target)
            .unwrap()
            .effects
            .iter()
            .map(|e| e.id)
            .filter(|id| id.starts_with("empower"))
            .collect();
        for id in ids {
            app.world_mut().write_message(StatusEffectEvent::new(
                None,
                target,
                StatusChange::Remove(id),
            ));
        }
        step(&mut app);
        let stats = app.world().get::<CombatStats>(target).unwrap();
        assert!(stats.cleaves.is_empty());
        assert!((stats.attack_damage - 161.0).abs() < 1e-3);
    }

    fn buffered_lock_on(buffer: f32) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(600.0), RankValue::ZERO)
            .cast_range_buffer(RankValue::fixed(buffer))
            .costs(RankValue::fixed(8.0), RankValue::fixed(100.0))
            .timing(1.0, 0.0)
    }

    fn lock_on_and_move_target(buffer: f32, moved_to: f32) -> (App, Entity, Entity) {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(buffered_lock_on(buffer));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(500.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Unit(target));
        step(&mut app);
        assert!(app.world().get::<Activation>(caster).is_some());
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(moved_to, 0.0, 0.0);
        (app, caster, target)
    }

    #[test]
    fn target_beyond_the_cast_range_buffer_cancels_the_cast_free() {
        let (mut app, caster, _) = lock_on_and_move_target(200.0, 850.0);
        let mut fired = Vec::new();
        for _ in 0..30 {
            step(&mut app);
            fired.extend(fired_events(&app));
        }
        assert!(fired.is_empty(), "{fired:?}");
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 1000.0);
        let slot = &app.world().get::<AbilityLoadout>(caster).unwrap().slots[3];
        assert_eq!(
            slot.cooldown.remaining, 0.0,
            "no cooldown for a cancelled cast"
        );
    }

    #[test]
    fn target_inside_the_cast_range_buffer_still_fires() {
        let (mut app, caster, target) = lock_on_and_move_target(200.0, 750.0);
        let mut fired = Vec::new();
        for _ in 0..30 {
            step(&mut app);
            fired.extend(fired_events(&app));
        }
        assert_eq!(
            fired,
            vec![(CastTarget::Unit(target), Vec3::new(750.0, 0.0, 0.0))]
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 900.0);
    }

    #[test]
    fn without_a_buffer_the_target_may_leave_the_cast_range() {
        let (mut app, _, target) = lock_on_and_move_target(0.0, 5000.0);
        let mut fired = Vec::new();
        for _ in 0..30 {
            step(&mut app);
            fired.extend(fired_events(&app));
        }
        assert_eq!(
            fired,
            vec![(CastTarget::Unit(target), Vec3::new(5000.0, 0.0, 0.0))]
        );
    }

    #[test]
    fn dead_target_with_a_buffer_still_fires_at_its_last_position() {
        let (mut app, _, _) = lock_on_and_move_target(200.0, 500.0);
        let target = app
            .world_mut()
            .query_filtered::<Entity, With<Health>>()
            .iter(app.world())
            .find(|e| app.world().get::<Team>(*e) == Some(&Team::Dire))
            .unwrap();
        app.world_mut().get_mut::<Health>(target).unwrap().current = 0.0;
        // Moving the corpse far away does not cancel: it is a point target now.
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(5000.0, 0.0, 0.0);
        let mut fired = Vec::new();
        for _ in 0..30 {
            step(&mut app);
            fired.extend(fired_events(&app));
        }
        assert_eq!(fired.len(), 1);
        assert!(matches!(fired[0].0, CastTarget::Point(_)));
    }

    fn short_owned_stun_ability(duration: f32) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::OwnedStun { id: "test_stun" },
                    duration: RankValue::fixed(duration),
                },
            )
    }

    fn apply_status_ability(spec: StatusSpec, duration: f32) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .team(TargetTeam::Any)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::ApplyStatus {
                    status: spec,
                    duration: RankValue::fixed(duration),
                },
            )
    }

    /// Casts `spec` from the Radiant caster on a unit of `team` and returns the status it left.
    fn cast_status_on(spec: StatusSpec, team: Team) -> crate::status::StatusEffect {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(apply_status_ability(spec, 5.0));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, team, Vec3::new(80.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Unit(target));
        for _ in 0..3 {
            step(&mut app);
        }
        let id = match spec {
            StatusSpec::Ethereal { .. } => "ethereal",
            _ => "banish",
        };
        status_of(&app, target, id)
            .unwrap_or_else(|| panic!("{id} applied to a {team:?} unit"))
            .clone()
    }

    #[test]
    fn ethereal_kind_follows_hostility() {
        let ethereal = StatusSpec::Ethereal {
            magic_amp_percent: RankValue::fixed(40.0),
        };
        assert!(ethereal.kind_follows_hostility() && StatusSpec::Banish.kind_follows_hostility());
        assert!(!StatusSpec::Stun.kind_follows_hostility());
        assert_eq!(
            cast_status_on(ethereal, Team::Dire).kind,
            crate::status::StatusKind::Debuff,
            "on an enemy"
        );
        assert_eq!(
            cast_status_on(ethereal, Team::Radiant).kind,
            crate::status::StatusKind::Buff,
            "on an ally"
        );
        assert_eq!(
            cast_status_on(StatusSpec::Banish, Team::Dire).kind,
            crate::status::StatusKind::Debuff
        );
        assert_eq!(
            cast_status_on(StatusSpec::Banish, Team::Radiant).kind,
            crate::status::StatusKind::Buff
        );
    }

    #[test]
    fn hostile_ethereal_is_blocked_by_debuff_immunity() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(apply_status_ability(
                StatusSpec::Ethereal {
                    magic_amp_percent: RankValue::fixed(40.0),
                },
                5.0,
            ));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(target)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::debuff_immunity(10.0));
        request(&mut app, caster, 3, CastTarget::Unit(target));
        for _ in 0..3 {
            step(&mut app);
        }
        assert!(status_of(&app, target, "ethereal").is_none());
    }

    #[test]
    fn area_spell_still_hits_spell_immune_but_deals_no_magic() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let immune = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let plain = spawn_unit(&mut app, Team::Dire, Vec3::new(-100.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(immune)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::spell_immunity(30.0));
        request(&mut app, caster, 0, CastTarget::None);
        let mut hits = Vec::new();
        for _ in 0..10 {
            step(&mut app);
            hits.extend(current_messages::<DamageEvent>(&app));
        }
        assert!(
            hits.iter().any(|e| e.target == immune),
            "the area still reaches the spell immune unit"
        );
        let world = app.world();
        assert_eq!(world.get::<Health>(immune).unwrap().current, 1000.0);
        assert!(world.get::<Health>(plain).unwrap().current < 1000.0);
    }

    #[test]
    fn unit_target_spell_cannot_pick_a_spell_immune_enemy() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let immune = spawn_unit(&mut app, Team::Dire, Vec3::new(50.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(immune)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::spell_immunity(30.0));
        request(&mut app, caster, 1, CastTarget::Unit(immune));
        for _ in 0..4 {
            step(&mut app);
        }
        let world = app.world();
        assert!(world.get::<Activation>(caster).is_none());
        assert!(world.get::<QueuedActivation>(caster).is_none());
        assert_eq!(world.get::<Mana>(caster).unwrap().current, 1000.0);

        // Without the immunity the same cast starts and is paid for.
        app.world_mut()
            .get_mut::<StatusEffects>(immune)
            .unwrap()
            .effects
            .clear();
        request(&mut app, caster, 1, CastTarget::Unit(immune));
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(app.world().get::<Mana>(caster).unwrap().current < 1000.0);
    }

    #[test]
    fn cannot_unit_target_concealed_enemy() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let hidden = spawn_unit(&mut app, Team::Dire, Vec3::new(50.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(hidden)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::invisibility(30.0, true));
        request(&mut app, caster, 1, CastTarget::Unit(hidden));
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(app.world().get::<Activation>(caster).is_none());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 1000.0);

        // True sight makes it targetable again.
        app.world_mut()
            .get_mut::<StatusEffects>(hidden)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::reveal(30.0));
        request(&mut app, caster, 1, CastTarget::Unit(hidden));
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(app.world().get::<Mana>(caster).unwrap().current < 1000.0);
    }

    #[test]
    fn targeting_only_untargetable_still_hit_by_areas() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let dancer = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(dancer)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::untargetable(30.0));
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..10 {
            step(&mut app);
        }
        assert!(
            app.world().get::<Health>(dancer).unwrap().current < 1000.0,
            "an area still damages a targeting-only untargetable unit"
        );

        // A unit-target spell cannot pick it.
        let before = app.world().get::<Mana>(caster).unwrap().current;
        for _ in 0..60 {
            step(&mut app);
        }
        request(&mut app, caster, 1, CastTarget::Unit(dancer));
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(app.world().get::<Activation>(caster).is_none());
        assert!(app.world().get::<Mana>(caster).unwrap().current >= before);
    }

    #[test]
    fn holder_still_runs_own_caster_effects() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(apply_status_ability(StatusSpec::Untargetable, 5.0));
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        // Cast on itself: the status lands on the caster.
        request(&mut app, caster, 3, CastTarget::Unit(caster));
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(status_of(&app, caster, "untargetable").is_some());
        // The holder still casts its own area ability and damages enemies.
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..10 {
            step(&mut app);
        }
        assert!(app.world().get::<Health>(enemy).unwrap().current < 1000.0);
    }

    #[test]
    fn area_damage_hits_invisible_unit() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        let hidden = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(hidden)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::invisibility(30.0, true));
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..10 {
            step(&mut app);
        }
        assert!(app.world().get::<Health>(hidden).unwrap().current < 1000.0);
    }

    #[test]
    fn cast_by_invisible_caster_ends_invisibility() {
        let mut app = test_app();
        let caster = spawn_caster(&mut app, KIT);
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::invisibility(30.0, true));
        let _enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..10 {
            step(&mut app);
        }
        assert!(status_of(&app, caster, "invisibility").is_none());
    }

    // --- Spell block and reflection ---

    fn unit_spell(cooldown: f32) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::fixed(cooldown), RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::CastTarget,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::OwnedStun { id: "test_stun" },
                    duration: RankValue::fixed(3.0),
                },
            )
    }

    fn give_status(app: &mut App, unit: Entity, effect: crate::status::StatusEffect) {
        app.world_mut()
            .get_mut::<StatusEffects>(unit)
            .unwrap()
            .effects
            .push(effect);
    }

    fn cast_unit_spell(app: &mut App, caster: Entity, target: Entity, frames: usize) {
        request(app, caster, 3, CastTarget::Unit(target));
        for _ in 0..frames {
            step(app);
        }
    }

    #[test]
    fn spell_block_negates_unit_target_and_is_consumed() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(unit_spell(0.0));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        give_status(
            &mut app,
            target,
            crate::status::StatusEffect::spell_block(30.0),
        );
        cast_unit_spell(&mut app, caster, target, 4);
        assert!(status_of(&app, target, "test_stun").is_none(), "blocked");
        assert!(status_of(&app, target, "spell_block").is_none(), "consumed");
        cast_unit_spell(&mut app, caster, target, 4);
        assert!(
            status_of(&app, target, "test_stun").is_some(),
            "second lands"
        );
    }

    #[test]
    fn spell_block_blocks_projectile_on_hit_not_on_launch() {
        let mut app = test_app();
        let def = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::Unit, RankValue::fixed(500.0), RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .mechanic(AbilityMechanic::Projectile {
                splash_radius: AreaRadius::Fixed(RankValue::fixed(0.0)),
                speed: None,
                skillshot: false,
                first_hit: false,
                end_radius: None,
                origin_offset: None,
                seek: None,
            })
            .on(
                AbilityTrigger::OnProjectileHit,
                EffectTarget::TriggerUnit,
                AbilityEffect::ApplyStatus {
                    status: StatusSpec::OwnedStun { id: "test_stun" },
                    duration: RankValue::fixed(3.0),
                },
            );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(def);
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        give_status(
            &mut app,
            target,
            crate::status::StatusEffect::spell_block(30.0),
        );
        cast_unit_spell(&mut app, caster, target, 4);
        assert!(
            status_of(&app, target, "spell_block").is_some(),
            "launching the projectile does not spend the block"
        );
        let payload = ProjectilePayload {
            caster,
            source: EffectSource::Ability(AbilityId::Bulwark),
            rank: 1,
            cast_target: Some(target),
            aim: Vec3::X * 80.0,
            reflected: false,
        };
        app.world_mut()
            .write_message(hit(payload, Vec3::X * 80.0, Some(target), Vec::new()));
        for _ in 0..3 {
            step(&mut app);
        }
        assert!(
            status_of(&app, target, "spell_block").is_none(),
            "hit spends it"
        );
        assert!(status_of(&app, target, "test_stun").is_none(), "and blocks");
    }

    #[test]
    fn two_spells_same_frame_only_first_blocked() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(unit_spell(0.0));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        give_status(
            &mut app,
            target,
            crate::status::StatusEffect::spell_block(30.0),
        );
        for _ in 0..2 {
            app.world_mut().write_message(AbilityTriggerEvent(
                TriggerContext::new(
                    AbilityTrigger::OnCast,
                    EffectSource::Ability(AbilityId::Bulwark),
                    caster,
                    Team::Radiant,
                    1,
                    Vec3::ZERO,
                    Vec3::X * 80.0,
                )
                .cast_target(Some(target)),
            ));
        }
        step(&mut app);
        step(&mut app);
        assert!(status_of(&app, target, "spell_block").is_none());
        let stuns = app
            .world()
            .get::<StatusEffects>(target)
            .unwrap()
            .effects
            .iter()
            .filter(|e| e.id == "test_stun")
            .count();
        assert_eq!(stuns, 1, "the first spell was blocked, the second landed");
    }

    #[test]
    fn spell_block_ignores_aoe_and_ally_casts() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(apply_status_ability(
                StatusSpec::OwnedStun { id: "test_stun" },
                3.0,
            ));
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(-100.0, 0.0, 0.0));
        for unit in [enemy, ally] {
            give_status(
                &mut app,
                unit,
                crate::status::StatusEffect::spell_block(30.0),
            );
        }
        // Area (Shockwave, slot 0) hits the blocked enemy and leaves the block alone.
        request(&mut app, caster, 0, CastTarget::None);
        for _ in 0..10 {
            step(&mut app);
        }
        assert!(health(&app, enemy) < 1000.0, "area damage lands");
        assert!(status_of(&app, enemy, "spell_block").is_some());
        // A unit spell on an ally is not hostile: it lands and the block stays.
        cast_unit_spell(&mut app, caster, ally, 4);
        assert!(status_of(&app, ally, "test_stun").is_some());
        assert!(status_of(&app, ally, "spell_block").is_some());
    }

    #[test]
    fn reflect_recasts_on_original_caster_once() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(unit_spell(0.0));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        give_status(
            &mut app,
            target,
            crate::status::StatusEffect::spell_reflect(30.0),
        );
        let mut reflects = 0;
        request(&mut app, caster, 3, CastTarget::Unit(target));
        for _ in 0..8 {
            step(&mut app);
            reflects +=
                current_messages::<crate::abilities::effects::SpellReflectEvent>(&app).len();
        }
        assert_eq!(reflects, 1);
        assert!(
            status_of(&app, target, "test_stun").is_some(),
            "Lotus style: the original still lands"
        );
        assert!(
            status_of(&app, caster, "test_stun").is_some(),
            "and it is recast on the original caster"
        );
        assert!(
            status_of(&app, target, "spell_reflect").is_some(),
            "not consumed"
        );
    }

    #[test]
    fn two_reflectors_do_not_ping_pong() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(unit_spell(0.0));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        for unit in [caster, target] {
            give_status(
                &mut app,
                unit,
                crate::status::StatusEffect::spell_reflect(30.0),
            );
        }
        let mut reflects = 0;
        request(&mut app, caster, 3, CastTarget::Unit(target));
        for _ in 0..20 {
            step(&mut app);
            reflects +=
                current_messages::<crate::abilities::effects::SpellReflectEvent>(&app).len();
        }
        assert_eq!(reflects, 1, "the reflected cast is not reflected again");
        assert!(status_of(&app, caster, "test_stun").is_some());
    }

    #[test]
    fn reflected_cast_fires_no_cast_triggers_or_cooldown() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(unit_spell(8.0));
        let caster = spawn_caster(&mut app, KIT);
        let reflector = spawn_caster(&mut app, KIT);
        app.world_mut()
            .entity_mut(reflector)
            .insert((Team::Dire, Transform::from_xyz(80.0, 0.0, 0.0)));
        give_status(
            &mut app,
            reflector,
            crate::status::StatusEffect::spell_reflect(30.0),
        );
        let mut casts = 0;
        request(&mut app, caster, 3, CastTarget::Unit(reflector));
        for _ in 0..8 {
            step(&mut app);
            casts += current_messages::<AbilityCastEvent>(&app).len();
        }
        assert_eq!(
            casts, 1,
            "only the original cast; the reflection is no cast"
        );
        assert!(
            status_of(&app, caster, "test_stun").is_some(),
            "it was reflected"
        );
        let slot = &app.world().get::<AbilityLoadout>(reflector).unwrap().slots[3];
        assert_eq!(
            slot.cooldown.remaining, 0.0,
            "the reflector pays no cooldown"
        );
        let slot = &app.world().get::<AbilityLoadout>(caster).unwrap().slots[3];
        assert!(slot.cooldown.remaining > 0.0, "the caster does");
    }

    #[test]
    fn item_and_ability_sources_both_blockable() {
        // Ability.
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(unit_spell(0.0));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        give_status(
            &mut app,
            target,
            crate::status::StatusEffect::spell_block(30.0),
        );
        cast_unit_spell(&mut app, caster, target, 4);
        assert!(status_of(&app, target, "spell_block").is_none());
        assert!(status_of(&app, target, "test_stun").is_none());

        // Item with a unit-target active.
        let mut app = test_app();
        let _defs = install_test_item_definitions([parity_damage_item(100.0)]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        give_status(
            &mut app,
            target,
            crate::status::StatusEffect::spell_block(30.0),
        );
        request_item(&mut app, caster, 0, CastTarget::Unit(target));
        for _ in 0..4 {
            step(&mut app);
        }
        assert_eq!(health(&app, target), 1000.0, "the item spell was blocked");
        assert!(
            status_of(&app, target, "spell_block").is_none(),
            "and spent the block"
        );
    }

    #[test]
    fn reflected_item_spell_hits_original_caster_once_with_no_cooldown() {
        let mut app = test_app();
        let _defs = install_test_item_definitions([parity_damage_item(100.0)]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        give_status(
            &mut app,
            target,
            crate::status::StatusEffect::spell_reflect(30.0),
        );
        // A reflector on both sides: the recast must not bounce back.
        give_status(
            &mut app,
            caster,
            crate::status::StatusEffect::spell_reflect(30.0),
        );
        let before = health(&app, caster);
        let mut reflects = 0;
        request_item(&mut app, caster, 0, CastTarget::Unit(target));
        for _ in 0..12 {
            step(&mut app);
            reflects +=
                current_messages::<crate::abilities::effects::SpellReflectEvent>(&app).len();
        }
        assert_eq!(reflects, 1, "reflected once, never ping-pongs");
        assert!(health(&app, target) < 1000.0, "the original still lands");
        assert!(
            health(&app, caster) < before,
            "the reflected item spell hits the original caster"
        );
    }

    #[test]
    fn mini_stun_spares_a_longer_foreign_stun() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(short_owned_stun_ability(0.1));
        let caster = spawn_caster(&mut app, KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(target)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::stunned(3.0));
        request(&mut app, caster, 3, CastTarget::Unit(target));
        for _ in 0..3 {
            step(&mut app);
        }
        let mini = status_of(&app, target, "test_stun").expect("mini stun applied");
        assert!(mini.stunned && mini.remaining <= 0.1);
        let foreign = status_of(&app, target, "stun").expect("foreign stun kept");
        assert!(foreign.remaining > 2.5, "{}", foreign.remaining);
        // Distinct ids: when the mini stun ends the foreign one is still in force.
        app.world_mut()
            .get_mut::<StatusEffects>(target)
            .unwrap()
            .effects
            .retain(|e| e.id != "test_stun");
        assert!(status_of(&app, target, "stun").is_some());
        assert!(
            app.world()
                .get::<StatusEffects>(target)
                .unwrap()
                .is_stunned()
        );
    }

    #[test]
    fn mini_stun_interrupts_a_channelling_target() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(channelled_heal());
        let channeller = spawn_caster(&mut app, KIT);
        request(&mut app, channeller, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        let mini = StatusSpec::OwnedStun { id: "test_stun" }.build(
            1,
            0.1,
            definition::StatusBuildContext {
                move_speed: 0.0,
                agility: 0.0,
                source: None,
                point: Vec3::ZERO,
                cast_target: None,
                base_attack_damage: 0.0,
            },
        );
        app.world_mut()
            .get_mut::<StatusEffects>(channeller)
            .unwrap()
            .effects
            .push(mini);
        step(&mut app);
        let ends: Vec<_> = app
            .world()
            .resource::<Messages<ActionChannelEndEvent>>()
            .iter_current_update_messages()
            .map(|e| e.completed)
            .collect();
        assert_eq!(
            ends,
            vec![false],
            "the channel was interrupted, not completed"
        );
    }

    /// A stun that expires within the frame it lands in is still seen by the interrupt
    /// checks, because production orders `tick_status_effects` after `tick_activations`.
    #[test]
    fn sub_frame_stun_still_interrupts_channel() {
        let mut app = test_app();
        // Mirror the production ordering: the status ticker runs after the activation tick.
        app.add_systems(
            Update,
            crate::status::tick_status_effects
                .after(tick_activations)
                .before(reactive_triggers::status_triggers),
        );
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(channelled_heal());
        let channeller = spawn_caster(&mut app, KIT);
        request(&mut app, channeller, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        // Shorter than one 0.05s frame: it is gone after this frame's status tick.
        app.world_mut()
            .get_mut::<StatusEffects>(channeller)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::stunned(0.01));
        step(&mut app);
        let ends: Vec<_> = app
            .world()
            .resource::<Messages<ActionChannelEndEvent>>()
            .iter_current_update_messages()
            .map(|e| e.completed)
            .collect();
        assert_eq!(ends, vec![false], "interrupted despite the stun expiring");
        assert!(
            !app.world()
                .get::<StatusEffects>(channeller)
                .unwrap()
                .is_stunned(),
            "the stun has expired"
        );
    }

    // ---- zones (AbilityEffect::Zone) ----

    const ZONE_TICK: AbilityTrigger = AbilityTrigger::Custom("on_zone_tick");

    fn zone_effect(team: TargetTeam, delay: f32, duration: f32, interval: f32) -> AbilityEffect {
        follow_zone_effect(team, delay, duration, interval, false)
    }

    fn follow_zone_effect(
        team: TargetTeam,
        delay: f32,
        duration: f32,
        interval: f32,
        follow_caster: bool,
    ) -> AbilityEffect {
        AbilityEffect::Zone {
            radius: AreaRadius::Fixed(RankValue::fixed(300.0)),
            team,
            delay: RankValue::fixed(delay),
            duration: RankValue::fixed(duration),
            interval: RankValue::fixed(interval),
            then: ZONE_TICK,
            follow_caster,
        }
    }

    /// An instant no-target cast that drops a zone on the caster, damaging whoever stands in
    /// it on every tick.
    fn zone_ability(effect: AbilityEffect) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::NoTarget, RankValue::ZERO, RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(AbilityTrigger::OnCast, EffectTarget::Caster, effect)
            .on(
                ZONE_TICK,
                EffectTarget::AffectedUnits,
                fixed_magical_damage(10.0),
            )
    }

    fn zone_count(app: &mut App) -> usize {
        app.world_mut()
            .query::<&zones::AbilityZone>()
            .iter(app.world())
            .count()
    }

    /// The zone ticks written in the last update.
    fn zone_ticks(app: &App) -> Vec<TriggerContext> {
        app.world()
            .resource::<Messages<AbilityTriggerEvent>>()
            .iter_current_update_messages()
            .filter(|event| event.0.trigger == ZONE_TICK)
            .map(|event| event.0.clone())
            .collect()
    }

    /// Steps until `seconds` of game time passed and returns the elapsed time of each tick.
    fn run_zone(app: &mut App, seconds: f32) -> Vec<f32> {
        let mut ticks = Vec::new();
        let steps = (seconds / 0.05).round() as usize;
        for index in 1..=steps {
            step(app);
            ticks.extend(zone_ticks(app).iter().map(|_| index as f32 * 0.05));
        }
        ticks
    }

    #[test]
    fn zone_waits_for_its_delay_then_ticks_on_schedule() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(zone_effect(TargetTeam::Enemy, 1.2, 10.0, 1.0)));
        let caster = spawn_caster(&mut app, KIT);
        let inside = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let outside = spawn_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);

        let ticks = run_zone(&mut app, 14.0);
        assert_eq!(ticks.len(), 10, "{ticks:?}");
        assert!(
            ticks[0] >= 1.2 && ticks[0] < 1.5,
            "no tick before the delay: {}",
            ticks[0]
        );
        for pair in ticks.windows(2) {
            assert!(
                (pair[1] - pair[0] - 1.0).abs() < 0.11,
                "one tick a second: {ticks:?}"
            );
        }
        assert_eq!(health(&app, inside), 1000.0 - 100.0);
        assert_eq!(health(&app, outside), 1000.0);
        assert_eq!(
            zone_count(&mut app),
            0,
            "the zone ended after delay + duration"
        );
    }

    #[test]
    fn zone_tick_count_is_floor_of_duration_over_interval() {
        for (duration, interval, expected) in [(10.0, 3.0, 3), (1.4, 0.1, 14), (2.0, 2.0, 1)] {
            let mut app = test_app();
            app.world_mut()
                .resource_mut::<AbilityDefinitions>()
                .insert(zone_ability(zone_effect(
                    TargetTeam::Enemy,
                    0.0,
                    duration,
                    interval,
                )));
            let caster = spawn_caster(&mut app, KIT);
            request(&mut app, caster, 3, CastTarget::None);
            let ticks = run_zone(&mut app, duration + 1.0);
            assert_eq!(ticks.len(), expected, "{duration}/{interval}: {ticks:?}");
        }
    }

    #[test]
    fn zone_with_no_delay_ticks_on_the_frame_after_it_spawns() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(zone_effect(TargetTeam::Enemy, 0.0, 2.0, 1.0)));
        let caster = spawn_caster(&mut app, KIT);
        request(&mut app, caster, 3, CastTarget::None);
        let ticks = run_zone(&mut app, 3.0);
        assert_eq!(ticks.len(), 2);
        assert!(
            ticks[0] <= 0.15,
            "the first tick is immediate: {}",
            ticks[0]
        );
    }

    #[test]
    fn zone_reports_nothing_when_empty_and_never_the_caster() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(zone_effect(TargetTeam::Any, 0.0, 2.0, 1.0)));
        let caster = spawn_caster(&mut app, KIT);
        request(&mut app, caster, 3, CastTarget::None);
        let mut contexts = Vec::new();
        for _ in 0..50 {
            step(&mut app);
            contexts.extend(zone_ticks(&app));
        }
        assert_eq!(contexts.len(), 2, "empty ticks are still written");
        assert!(contexts.iter().all(|ctx| ctx.affected_units.is_empty()));
        assert_eq!(
            health(&app, caster),
            1000.0,
            "the caster is never a zone target"
        );
    }

    #[test]
    fn zone_tick_context_is_derived_without_unit_or_attack() {
        let mut app = test_app();
        app.world_mut().resource_mut::<AbilityDefinitions>().insert(
            zone_ability(zone_effect(TargetTeam::Enemy, 0.0, 1.0, 1.0)).on(
                AbilityTrigger::OnDamageTaken,
                EffectTarget::Caster,
                zone_effect(TargetTeam::Enemy, 0.0, 1.0, 1.0),
            ),
        );
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        app.world_mut().write_message(AbilityTriggerEvent(
            TriggerContext::new(
                AbilityTrigger::OnDamageTaken,
                EffectSource::Ability(AbilityId::Bulwark),
                caster,
                Team::Radiant,
                1,
                Vec3::ZERO,
                Vec3::new(5.0, 0.0, 7.0),
            )
            .trigger_unit(Some(enemy))
            .affected(vec![enemy])
            .attack(effects::AttackMeta {
                extra: false,
                crit: false,
            }),
        ));
        let mut contexts = Vec::new();
        for _ in 0..30 {
            step(&mut app);
            contexts.extend(zone_ticks(&app));
        }
        assert_eq!(contexts.len(), 1);
        let tick = &contexts[0];
        assert!(
            tick.reaction,
            "a zone spawned by a reaction stays a reaction"
        );
        assert_eq!(tick.depth, 1);
        assert_eq!(tick.trigger_unit, None);
        assert_eq!(tick.attack, None);
        assert_eq!(
            tick.affected_units,
            vec![enemy],
            "the units inside, not the spawn's"
        );
        assert_eq!(tick.source, EffectSource::Ability(AbilityId::Bulwark));
        assert_eq!(tick.rank, 1);
    }

    #[test]
    fn zone_is_a_no_op_past_the_depth_cap() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(zone_effect(TargetTeam::Enemy, 0.0, 2.0, 1.0)));
        let caster = spawn_caster(&mut app, KIT);
        let mut deep = TriggerContext::new(
            AbilityTrigger::OnCast,
            EffectSource::Ability(AbilityId::Bulwark),
            caster,
            Team::Radiant,
            1,
            Vec3::ZERO,
            Vec3::ZERO,
        );
        deep.depth = effects::MAX_TRIGGER_DEPTH;
        app.world_mut().write_message(AbilityTriggerEvent(deep));
        for _ in 0..5 {
            step(&mut app);
        }
        assert_eq!(zone_count(&mut app), 0);

        // One hop below the cap still spawns it.
        let mut shallow = TriggerContext::new(
            AbilityTrigger::OnCast,
            EffectSource::Ability(AbilityId::Bulwark),
            caster,
            Team::Radiant,
            1,
            Vec3::ZERO,
            Vec3::ZERO,
        );
        shallow.depth = effects::MAX_TRIGGER_DEPTH - 1;
        app.world_mut().write_message(AbilityTriggerEvent(shallow));
        step(&mut app);
        assert_eq!(zone_count(&mut app), 1);
    }

    // ---- random picks (AbilityEffect::PickRandom) ----

    const PICK_HIT: AbilityTrigger = AbilityTrigger::Custom("on_pick_hit");

    /// A zone on the caster whose every tick strikes `count` random enemies inside it.
    fn pick_ability(count: u32, duration: f32) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::NoTarget, RankValue::ZERO, RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(
                AbilityTrigger::OnCast,
                EffectTarget::Caster,
                zone_effect(TargetTeam::Enemy, 0.0, duration, 1.0),
            )
            .on(
                ZONE_TICK,
                EffectTarget::AffectedUnits,
                AbilityEffect::PickRandom {
                    count,
                    then: PICK_HIT,
                },
            )
            .on(
                PICK_HIT,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(10.0),
            )
    }

    /// Runs the pick ability and returns the targets struck in each frame that struck any.
    fn run_picks(app: &mut App, count: u32, duration: f32) -> Vec<Vec<Entity>> {
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(pick_ability(count, duration));
        let caster = spawn_caster(app, KIT);
        request(app, caster, 3, CastTarget::None);
        let mut frames = Vec::new();
        for _ in 0..((duration + 1.0) / 0.05) as usize {
            step(app);
            let hit: Vec<Entity> = current_messages::<DamageEvent>(app)
                .iter()
                .map(|d| d.target)
                .collect();
            if !hit.is_empty() {
                frames.push(hit);
            }
        }
        frames
    }

    #[test]
    fn pick_random_fires_once_per_tick_on_one_pool_member() {
        let mut app = test_app();
        let pool = [
            spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0)),
            spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 150.0)),
            spawn_unit(&mut app, Team::Dire, Vec3::new(-120.0, 0.0, 0.0)),
        ];
        let outside = spawn_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(50.0, 0.0, 0.0));
        let frames = run_picks(&mut app, 1, 3.0);
        assert_eq!(frames.len(), 3, "one strike per tick, three ticks");
        for hit in &frames {
            assert_eq!(hit.len(), 1);
            assert!(pool.contains(&hit[0]));
            assert!(hit[0] != outside && hit[0] != ally);
        }
    }

    #[test]
    fn pick_random_with_empty_pool_fires_nothing() {
        let mut app = test_app();
        spawn_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0));
        spawn_unit(&mut app, Team::Radiant, Vec3::new(50.0, 0.0, 0.0));
        assert!(run_picks(&mut app, 1, 3.0).is_empty());
    }

    #[test]
    fn pick_random_count_caps_at_pool_and_is_distinct() {
        let mut app = test_app();
        for x in [100.0, 150.0, 200.0] {
            spawn_unit(&mut app, Team::Dire, Vec3::new(x, 0.0, 0.0));
        }
        for frame in run_picks(&mut app, 2, 3.0) {
            assert_eq!(frame.len(), 2);
            assert_ne!(frame[0], frame[1], "picks are distinct");
        }
        let mut app = test_app();
        for x in [100.0, 150.0] {
            spawn_unit(&mut app, Team::Dire, Vec3::new(x, 0.0, 0.0));
        }
        for frame in run_picks(&mut app, 5, 3.0) {
            assert_eq!(frame.len(), 2, "capped at the pool size");
            assert_ne!(frame[0], frame[1]);
        }
    }

    #[test]
    fn pick_random_is_deterministic_for_a_seed() {
        let run = |seed: u64| {
            let mut app = test_app();
            app.world_mut().resource_mut::<CombatRng>().seed(seed);
            let pool: Vec<Entity> = (0..4)
                .map(|i| {
                    spawn_unit(
                        &mut app,
                        Team::Dire,
                        Vec3::new(60.0 + 40.0 * i as f32, 0.0, 0.0),
                    )
                })
                .collect();
            run_picks(&mut app, 1, 20.0)
                .iter()
                .map(|hit| pool.iter().position(|e| *e == hit[0]).unwrap())
                .collect::<Vec<_>>()
        };
        let (a, b) = (run(7), run(7));
        assert_eq!(a.len(), 20);
        assert_eq!(a, b, "same seed, same picks");
        assert!(
            a.iter().any(|i| *i != a[0]),
            "the picks vary across ticks, not always the same unit"
        );
        assert_ne!(a, run(99), "another seed picks differently");
    }

    #[test]
    fn pick_random_respects_depth_cap() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(pick_ability(1, 1.0));
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let tick_at = |depth: u8| {
            let mut ctx = TriggerContext::new(
                ZONE_TICK,
                EffectSource::Ability(AbilityId::Bulwark),
                caster,
                Team::Radiant,
                1,
                Vec3::ZERO,
                Vec3::ZERO,
            );
            ctx.depth = depth;
            ctx.affected_units = vec![enemy];
            AbilityTriggerEvent(ctx)
        };
        app.world_mut()
            .write_message(tick_at(effects::MAX_TRIGGER_DEPTH));
        step(&mut app);
        step(&mut app);
        assert!(current_messages::<DamageEvent>(&app).is_empty());

        // A zone tick sits at depth 1, one hop below the cap, and still strikes.
        app.world_mut()
            .write_message(tick_at(effects::MAX_TRIGGER_DEPTH - 1));
        let mut struck = 0;
        for _ in 0..3 {
            step(&mut app);
            struck += current_messages::<DamageEvent>(&app).len();
        }
        assert_eq!(struck, 1);
    }

    #[test]
    fn item_and_ability_pick_random_alike() {
        let rows = |trigger| -> Vec<AbilityEffectEntry> {
            vec![
                AbilityEffectEntry {
                    trigger,
                    target: EffectTarget::UnitsInRadius {
                        center: definition::AreaCenter::Caster,
                        radius: AreaRadius::Fixed(RankValue::fixed(500.0)),
                        team: TargetTeam::Enemy,
                    },
                    effect: AbilityEffect::PickRandom {
                        count: 1,
                        then: PICK_HIT,
                    },
                },
                AbilityEffectEntry {
                    trigger: PICK_HIT,
                    target: EffectTarget::TriggerUnit,
                    effect: fixed_magical_damage(10.0),
                },
            ]
        };

        // Ability.
        let mut app = test_app();
        let mut ability = AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::NoTarget, RankValue::ZERO, RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0);
        for entry in rows(AbilityTrigger::OnCast) {
            ability = ability.on(entry.trigger, entry.target, entry.effect);
        }
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(ability);
        let caster = spawn_caster(&mut app, KIT);
        let pool = [
            spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0)),
            spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 200.0)),
        ];
        request(&mut app, caster, 3, CastTarget::None);
        let mut ability_hits = Vec::new();
        for _ in 0..6 {
            step(&mut app);
            ability_hits.extend(current_messages::<DamageEvent>(&app));
        }

        // Item.
        let mut app = test_app();
        let effects: &'static [AbilityEffectEntry] =
            Box::leak(rows(AbilityTrigger::OnUse).into_boxed_slice());
        let _defs = install_test_item_definitions([item_with(
            effects,
            TargetType::NoTarget,
            TargetTeam::Enemy,
            0.0,
        )]);
        let caster = spawn_caster_with_inventory(&mut app, KIT);
        give_item(&mut app, caster, ItemId::HeartwoodBand);
        let item_pool = [
            spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0)),
            spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 200.0)),
        ];
        request_item(&mut app, caster, 0, CastTarget::None);
        let mut item_hits = Vec::new();
        for _ in 0..6 {
            step(&mut app);
            item_hits.extend(current_messages::<DamageEvent>(&app));
        }

        assert_eq!(ability_hits.len(), 1);
        assert_eq!(item_hits.len(), 1);
        assert!(pool.contains(&ability_hits[0].target));
        assert!(item_pool.contains(&item_hits[0].target));
        assert_eq!(ability_hits[0].amount, item_hits[0].amount);
    }

    fn move_unit(app: &mut App, unit: Entity, to: Vec3) {
        app.world_mut()
            .get_mut::<Transform>(unit)
            .unwrap()
            .translation = to;
    }

    /// Steps until a tick is written and returns it.
    fn next_zone_tick(app: &mut App) -> TriggerContext {
        for _ in 0..60 {
            step(app);
            if let Some(tick) = zone_ticks(app).into_iter().next() {
                return tick;
            }
        }
        panic!("the zone never ticked");
    }

    #[test]
    fn attached_zone_recentres_on_caster_each_tick() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(follow_zone_effect(
                TargetTeam::Enemy,
                0.0,
                3.0,
                1.0,
                true,
            )));
        let caster = spawn_caster(&mut app, KIT);
        // 700 away from where the caster starts: outside the 300 radius.
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(700.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        let first = next_zone_tick(&mut app);
        assert!(first.affected_units.is_empty());
        assert_eq!(health(&app, enemy), 1000.0);

        // The caster walks next to the enemy; the next tick is centred on its new position.
        move_unit(&mut app, caster, Vec3::new(600.0, 0.0, 0.0));
        let second = next_zone_tick(&mut app);
        assert_eq!(second.affected_units, vec![enemy]);
        assert_eq!(health(&app, enemy), 990.0);
        let center = app
            .world_mut()
            .query::<&zones::AbilityZone>()
            .single(app.world())
            .unwrap()
            .center;
        assert_eq!(center, Vec3::new(600.0, 0.0, 0.0));

        // And back out of reach again.
        move_unit(&mut app, caster, Vec3::new(-400.0, 0.0, 0.0));
        let third = next_zone_tick(&mut app);
        assert!(third.affected_units.is_empty());
        assert_eq!(health(&app, enemy), 990.0);
    }

    #[test]
    fn attached_zone_stops_when_caster_dies() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(follow_zone_effect(
                TargetTeam::Enemy,
                0.0,
                5.0,
                1.0,
                true,
            )));
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        next_zone_tick(&mut app);
        assert_eq!(health(&app, enemy), 990.0);
        assert_eq!(zone_count(&mut app), 1);

        app.world_mut().get_mut::<Health>(caster).unwrap().current = 0.0;
        let mut ticks = 0;
        for _ in 0..120 {
            step(&mut app);
            ticks += zone_ticks(&app).len();
        }
        assert_eq!(ticks, 0, "no tick fires once the caster is dead");
        assert_eq!(zone_count(&mut app), 0, "the zone is despawned, not paused");
        assert_eq!(health(&app, enemy), 990.0);
    }

    #[test]
    fn attached_zone_tick_point_is_caster_position() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(follow_zone_effect(
                TargetTeam::Enemy,
                0.0,
                3.0,
                1.0,
                true,
            )));
        let caster = spawn_caster(&mut app, KIT);
        request(&mut app, caster, 3, CastTarget::None);
        next_zone_tick(&mut app);
        let to = Vec3::new(250.0, 0.0, -80.0);
        move_unit(&mut app, caster, to);
        let tick = next_zone_tick(&mut app);
        assert_eq!(tick.point, to);
        assert_eq!(tick.origin, to);
        let zone_at = app
            .world_mut()
            .query::<(&zones::AbilityZone, &Transform)>()
            .single(app.world())
            .unwrap()
            .1
            .translation;
        assert_eq!(zone_at, to);
    }

    #[test]
    fn static_zone_unchanged() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(zone_effect(TargetTeam::Enemy, 0.0, 5.0, 1.0)));
        let caster = spawn_caster(&mut app, KIT);
        let near_spawn = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let near_walker = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        next_zone_tick(&mut app);
        move_unit(&mut app, caster, Vec3::new(900.0, 0.0, 0.0));
        let tick = next_zone_tick(&mut app);
        assert_eq!(tick.affected_units, vec![near_spawn], "it stayed put");
        assert_eq!(health(&app, near_walker), 1000.0);

        // A static zone outlives its caster's death.
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 0.0;
        let tick = next_zone_tick(&mut app);
        assert_eq!(tick.affected_units, vec![near_spawn]);
        assert_eq!(zone_count(&mut app), 1);
    }

    #[test]
    fn ability_attached_zone_requests_a_following_zone() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(follow_zone_effect(
                TargetTeam::Enemy,
                0.0,
                2.0,
                1.0,
                true,
            )));
        let caster = spawn_caster(&mut app, KIT);
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        let zone = app
            .world_mut()
            .query::<&zones::AbilityZone>()
            .single(app.world())
            .unwrap()
            .clone();
        assert!(zone.follow_caster);
    }

    #[test]
    fn zone_ignores_a_zero_interval_duration_or_radius() {
        for effect in [
            zone_effect(TargetTeam::Enemy, 0.0, 2.0, 0.0),
            zone_effect(TargetTeam::Enemy, 0.0, 0.0, 1.0),
            zone_effect(TargetTeam::Enemy, 0.0, 0.5, 1.0),
            AbilityEffect::Zone {
                radius: AreaRadius::Ability,
                team: TargetTeam::Enemy,
                delay: RankValue::ZERO,
                duration: RankValue::fixed(2.0),
                interval: RankValue::fixed(1.0),
                then: ZONE_TICK,
                follow_caster: false,
            },
        ] {
            let mut app = test_app();
            app.world_mut()
                .resource_mut::<AbilityDefinitions>()
                .insert(zone_ability(effect));
            let caster = spawn_caster(&mut app, KIT);
            request(&mut app, caster, 3, CastTarget::None);
            for _ in 0..10 {
                step(&mut app);
            }
            assert_eq!(zone_count(&mut app), 0, "{effect:?}");
        }
    }

    #[test]
    fn zone_outlives_its_casters_death_but_not_its_despawn() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(zone_effect(TargetTeam::Enemy, 0.0, 5.0, 1.0)));
        let caster = spawn_caster(&mut app, KIT);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        step(&mut app);
        step(&mut app);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 0.0;
        let ticks = run_zone(&mut app, 2.0);
        assert!(!ticks.is_empty(), "a dead caster's zone keeps ticking");
        assert_eq!(zone_count(&mut app), 1);
        app.world_mut().entity_mut(caster).despawn();
        step(&mut app);
        assert_eq!(zone_count(&mut app), 0);
        assert!(health(&app, enemy) < 1000.0);
    }

    #[test]
    fn overdue_zone_ticks_catch_up_in_one_frame() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(zone_effect(TargetTeam::Enemy, 1.2, 10.0, 1.0)));
        let caster = spawn_caster(&mut app, KIT);
        request(&mut app, caster, 3, CastTarget::None);
        step(&mut app);
        step(&mut app);
        assert_eq!(zone_count(&mut app), 1);
        // One 5 s frame: the ticks due at 1.2, 2.2, 3.2 and 4.2 all fire.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(5.0));
        app.update();
        assert_eq!(zone_ticks(&app).len(), 4);
        // A huge frame never fires more than the zone's total.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(60.0));
        app.update();
        assert_eq!(zone_ticks(&app).len(), 6, "10 ticks in total, never more");
        assert_eq!(zone_count(&mut app), 0);
    }

    #[test]
    fn item_and_ability_zone_emit_identical_ticks() {
        static ITEM_EFFECTS: std::sync::OnceLock<&'static [AbilityEffectEntry]> =
            std::sync::OnceLock::new();
        let item_effects = ITEM_EFFECTS.get_or_init(|| {
            Box::leak(Box::new([
                AbilityEffectEntry {
                    trigger: AbilityTrigger::OnUse,
                    target: EffectTarget::Caster,
                    effect: zone_effect(TargetTeam::Enemy, 0.5, 3.0, 1.0),
                },
                AbilityEffectEntry {
                    trigger: ZONE_TICK,
                    target: EffectTarget::AffectedUnits,
                    effect: fixed_magical_damage(10.0),
                },
            ]))
        });
        let item = ItemDefinition {
            id: ItemId::StormRod,
            name: "Parity Zone",
            short_label: "PZ",
            item_type: ItemType::Miscellaneous,
            cost: 1,
            sell_value: 0,
            tier: 1,
            rarity: ItemRarity::Common,
            max_stack: 1,
            consumable: false,
            max_charges: None,
            cooldown: 0.0,
            activation: ItemActivation {
                target_type: TargetType::NoTarget,
                target_team: TargetTeam::Enemy,
                cast_range: 0.0,
                cast_time: 0.0,
                channel_time: 0.0,
                mana_cost: 0.0,
            },
            attribute_toggle: None,
            components: &[],
            color: [1.0, 1.0, 1.0],
            description: "test",
            effects: item_effects,
            mechanics: &[],
        };

        let mut ability_app = test_app();
        ability_app
            .world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(zone_ability(zone_effect(TargetTeam::Enemy, 0.5, 3.0, 1.0)));
        let ability_caster = spawn_caster(&mut ability_app, KIT);
        let ability_enemy = spawn_unit(&mut ability_app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        request(&mut ability_app, ability_caster, 3, CastTarget::None);

        let mut item_app = test_app();
        let _defs = install_test_item_definitions([item]);
        let item_caster = spawn_caster_with_inventory(&mut item_app, KIT);
        give_item(&mut item_app, item_caster, ItemId::StormRod);
        let item_enemy = spawn_unit(&mut item_app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        request_item(&mut item_app, item_caster, 0, CastTarget::None);

        let shape = |app: &mut App, enemy: Entity| {
            let mut ticks = Vec::new();
            for index in 0..100 {
                step(app);
                for tick in zone_ticks(app) {
                    ticks.push((
                        index,
                        tick.affected_units == vec![enemy],
                        tick.depth,
                        tick.reaction,
                        tick.attack,
                    ));
                }
            }
            ticks
        };
        let from_ability = shape(&mut ability_app, ability_enemy);
        let from_item = shape(&mut item_app, item_enemy);
        assert_eq!(from_ability.len(), 3);
        assert_eq!(from_ability, from_item);
        assert_eq!(health(&ability_app, ability_enemy), 970.0);
        assert_eq!(health(&item_app, item_enemy), 970.0);
    }

    // ---- orbits (AbilityEffect::Orbit) ----

    const ORBIT_TOUCH: AbilityTrigger = AbilityTrigger::Custom("on_orbit_touch");
    const ORBIT_PERIOD: f32 = 1.41;

    fn orbit_effect(count: u32, duration: f32, team: TargetTeam) -> AbilityEffect {
        AbilityEffect::Orbit {
            count,
            radius: AreaRadius::Fixed(RankValue::fixed(180.0)),
            hit_radius: 80.0,
            period: RankValue::fixed(ORBIT_PERIOD),
            duration: RankValue::fixed(duration),
            team,
            then: ORBIT_TOUCH,
        }
    }

    /// An instant no-target cast that starts an orbit on the caster; each touch deals 10.
    fn orbit_ability(effect: AbilityEffect) -> AbilityDefinition {
        AbilityDefinition::new(AbilityId::Bulwark)
            .targeting(TargetType::NoTarget, RankValue::ZERO, RankValue::ZERO)
            .costs(RankValue::ZERO, RankValue::ZERO)
            .timing(0.0, 0.0)
            .on(AbilityTrigger::OnCast, EffectTarget::Caster, effect)
            .on(
                ORBIT_TOUCH,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(10.0),
            )
    }

    fn orbit_app(effect: AbilityEffect) -> (App, Entity) {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(orbit_ability(effect));
        let caster = spawn_caster(&mut app, KIT);
        (app, caster)
    }

    fn orbit_count(app: &mut App) -> usize {
        app.world_mut()
            .query::<&orbits::AbilityOrbit>()
            .iter(app.world())
            .count()
    }

    fn orbit_touches(app: &App) -> Vec<TriggerContext> {
        app.world()
            .resource::<Messages<AbilityTriggerEvent>>()
            .iter_current_update_messages()
            .filter(|event| event.0.trigger == ORBIT_TOUCH)
            .map(|event| event.0.clone())
            .collect()
    }

    fn step_by(app: &mut App, dt: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(dt));
        app.update();
    }

    /// Runs `seconds` in frames of `dt` and returns every touch written.
    fn run_orbit(app: &mut App, seconds: f32, dt: f32) -> Vec<TriggerContext> {
        let mut touches = Vec::new();
        for _ in 0..(seconds / dt).round() as usize {
            step_by(app, dt);
            touches.extend(orbit_touches(app));
        }
        touches
    }

    #[test]
    fn orbit_positions_follow_caster_and_angle() {
        let (mut app, caster) = orbit_app(orbit_effect(3, 5.0, TargetTeam::Enemy));
        request(&mut app, caster, 3, CastTarget::None);
        step(&mut app);
        step(&mut app);
        let orbit = app
            .world_mut()
            .query::<&orbits::AbilityOrbit>()
            .single(app.world())
            .unwrap()
            .clone();
        // The first orbiter starts in front of the caster (-Z), the others 120 degrees apart.
        let near = |a: Vec3, b: Vec3| a.distance(b) < 0.01;
        assert!(near(orbit.offset(0, 0.0), Vec3::new(0.0, 0.0, -180.0)));
        let third = std::f32::consts::TAU / 3.0;
        for index in 0..3 {
            let expected = Vec3::new(
                (-std::f32::consts::FRAC_PI_2 + third * index as f32).cos(),
                0.0,
                (-std::f32::consts::FRAC_PI_2 + third * index as f32).sin(),
            ) * 180.0;
            assert!(near(orbit.offset(index, 0.0), expected), "orbiter {index}");
            // Every orbiter is on the circle, and a quarter period turns it a quarter turn.
            assert!((orbit.offset(index, 0.3).length() - 180.0).abs() < 0.01);
        }
        assert!(near(
            orbit.offset(0, ORBIT_PERIOD / 4.0),
            Vec3::new(180.0, 0.0, 0.0)
        ));
        // The orbit and its spheres follow the caster.
        move_unit(&mut app, caster, Vec3::new(500.0, 0.0, -300.0));
        step(&mut app);
        let (orbit, transform) = app
            .world_mut()
            .query::<(&orbits::AbilityOrbit, &Transform)>()
            .single(app.world())
            .unwrap();
        assert_eq!(transform.translation, Vec3::new(500.0, 0.0, -300.0));
        assert_eq!(orbit.template.origin, Vec3::new(500.0, 0.0, -300.0));
        let visuals = orbit.visuals.clone();
        assert_eq!(visuals.len(), 3);
        let local = app
            .world()
            .get::<Transform>(visuals[0])
            .unwrap()
            .translation;
        assert!((Vec2::new(local.x, local.z).length() - 180.0).abs() < 0.01);
    }

    #[test]
    fn orbit_starts_at_the_casters_facing() {
        let (mut app, caster) = orbit_app(orbit_effect(3, 5.0, TargetTeam::Enemy));
        app.world_mut()
            .get_mut::<Transform>(caster)
            .unwrap()
            .look_to(Dir3::X, Dir3::Y);
        request(&mut app, caster, 3, CastTarget::None);
        step(&mut app);
        step(&mut app);
        let orbit = app
            .world_mut()
            .query::<&orbits::AbilityOrbit>()
            .single(app.world())
            .unwrap();
        assert!((orbit.offset(0, 0.0) - Vec3::new(180.0, 0.0, 0.0)).length() < 0.01);
    }

    #[test]
    fn orbit_hits_stationary_enemy_once_per_orbiter_per_revolution() {
        let (mut app, caster) = orbit_app(orbit_effect(3, ORBIT_PERIOD, TargetTeam::Enemy));
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(180.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        let touches = run_orbit(&mut app, 3.0, 0.05);
        assert_eq!(touches.len(), 3, "one hit per orbiter");
        assert!(touches.iter().all(|t| t.trigger_unit == Some(enemy)));
        assert_eq!(health(&app, enemy), 1000.0 - 30.0);
        assert_eq!(orbit_count(&mut app), 0);
    }

    #[test]
    fn orbit_does_not_rehit_while_contact_persists() {
        let (mut app, caster) = orbit_app(orbit_effect(3, 4.0, TargetTeam::Enemy));
        // A huge unit on top of the caster is inside every hitbox the whole time.
        let giant = spawn_unit(&mut app, Team::Dire, Vec3::ZERO);
        app.world_mut().entity_mut(giant).insert(BoundRadius(400.0));
        request(&mut app, caster, 3, CastTarget::None);
        let touches = run_orbit(&mut app, 5.0, 0.05);
        assert_eq!(touches.len(), 3, "once per orbiter for the whole duration");
        assert_eq!(health(&app, giant), 1000.0 - 30.0);
    }

    #[test]
    fn orbit_rehits_after_contact_ends() {
        let (mut app, caster) = orbit_app(orbit_effect(3, ORBIT_PERIOD * 2.0, TargetTeam::Enemy));
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(180.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        let touches = run_orbit(&mut app, 5.0, 0.05);
        assert_eq!(touches.len(), 6, "each orbiter passes the enemy twice");
        assert_eq!(health(&app, enemy), 1000.0 - 60.0);
    }

    #[test]
    fn orbit_substeps_prevent_tunnelling_at_low_fps() {
        for dt in [0.1, 0.25, 0.5] {
            let (mut app, caster) = orbit_app(orbit_effect(3, ORBIT_PERIOD, TargetTeam::Enemy));
            let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(180.0, 0.0, 0.0));
            request(&mut app, caster, 3, CastTarget::None);
            // The cast frame, then frames of `dt`: the hitbox moves more than its own size
            // between frames at the larger steps.
            step(&mut app);
            let touches = run_orbit(&mut app, 3.0, dt);
            assert_eq!(touches.len(), 3, "dt {dt}");
            assert_eq!(health(&app, enemy), 1000.0 - 30.0, "dt {dt}");
        }
    }

    #[test]
    fn orbit_ignores_allies_caster_and_cycloned_units() {
        let (mut app, caster) = orbit_app(orbit_effect(3, 3.0, TargetTeam::Enemy));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(180.0, 0.0, 0.0));
        let cycloned = spawn_unit(&mut app, Team::Dire, Vec3::new(-180.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(cycloned)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::cyclone(10.0, true));
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 180.0));
        request(&mut app, caster, 3, CastTarget::None);
        let touches = run_orbit(&mut app, 3.0, 0.05);
        assert!(touches.iter().all(|t| t.trigger_unit == Some(enemy)));
        assert!(!touches.is_empty());
        assert_eq!(health(&app, ally), 1000.0);
        assert_eq!(health(&app, cycloned), 1000.0);

        // Whoever the team rule allows, the caster is never touched by its own orbit.
        let (mut app, caster) = orbit_app(AbilityEffect::Orbit {
            count: 3,
            radius: AreaRadius::Fixed(RankValue::fixed(100.0)),
            hit_radius: 300.0,
            period: RankValue::fixed(ORBIT_PERIOD),
            duration: RankValue::fixed(2.0),
            team: TargetTeam::Any,
            then: ORBIT_TOUCH,
        });
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(50.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        let touches = run_orbit(&mut app, 2.0, 0.05);
        assert!(touches.iter().any(|t| t.trigger_unit == Some(ally)));
        assert!(touches.iter().all(|t| t.trigger_unit != Some(caster)));
        assert_eq!(health(&app, caster), 1000.0);
    }

    #[test]
    fn orbit_ends_on_duration_and_on_caster_death() {
        let (mut app, caster) = orbit_app(orbit_effect(3, 2.0, TargetTeam::Enemy));
        request(&mut app, caster, 3, CastTarget::None);
        run_orbit(&mut app, 1.0, 0.05);
        assert_eq!(orbit_count(&mut app), 1);
        run_orbit(&mut app, 1.5, 0.05);
        assert_eq!(orbit_count(&mut app), 0, "ended with its duration");

        let (mut app, caster) = orbit_app(orbit_effect(3, 10.0, TargetTeam::Enemy));
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(180.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        run_orbit(&mut app, 0.3, 0.05);
        assert_eq!(orbit_count(&mut app), 1);
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 0.0;
        let touches = run_orbit(&mut app, 3.0, 0.05);
        assert!(touches.is_empty(), "a dead caster's spirits do not hit");
        assert_eq!(orbit_count(&mut app), 0, "despawned, not paused");
        assert_eq!(health(&app, enemy), 1000.0);
    }

    #[test]
    fn item_and_ability_orbit_alike() {
        static ITEM_EFFECTS: std::sync::OnceLock<&'static [AbilityEffectEntry]> =
            std::sync::OnceLock::new();
        let item_effects = ITEM_EFFECTS.get_or_init(|| {
            Box::leak(Box::new([
                AbilityEffectEntry {
                    trigger: AbilityTrigger::OnUse,
                    target: EffectTarget::Caster,
                    effect: orbit_effect(3, 3.0, TargetTeam::Enemy),
                },
                AbilityEffectEntry {
                    trigger: ORBIT_TOUCH,
                    target: EffectTarget::TriggerUnit,
                    effect: fixed_magical_damage(10.0),
                },
            ]))
        });
        let item = ItemDefinition {
            id: ItemId::StormRod,
            name: "Parity Orbit",
            short_label: "PO",
            item_type: ItemType::Miscellaneous,
            cost: 1,
            sell_value: 0,
            tier: 1,
            rarity: ItemRarity::Common,
            max_stack: 1,
            consumable: false,
            max_charges: None,
            cooldown: 0.0,
            activation: ItemActivation {
                target_type: TargetType::NoTarget,
                target_team: TargetTeam::Enemy,
                cast_range: 0.0,
                cast_time: 0.0,
                channel_time: 0.0,
                mana_cost: 0.0,
            },
            attribute_toggle: None,
            components: &[],
            color: [1.0, 1.0, 1.0],
            description: "test",
            effects: item_effects,
            mechanics: &[],
        };

        let (mut ability_app, ability_caster) = orbit_app(orbit_effect(3, 3.0, TargetTeam::Enemy));
        let ability_enemy = spawn_unit(&mut ability_app, Team::Dire, Vec3::new(180.0, 0.0, 0.0));
        request(&mut ability_app, ability_caster, 3, CastTarget::None);

        let mut item_app = test_app();
        let _defs = install_test_item_definitions([item]);
        let item_caster = spawn_caster_with_inventory(&mut item_app, KIT);
        give_item(&mut item_app, item_caster, ItemId::StormRod);
        let item_enemy = spawn_unit(&mut item_app, Team::Dire, Vec3::new(180.0, 0.0, 0.0));
        request_item(&mut item_app, item_caster, 0, CastTarget::None);

        let shape = |app: &mut App, enemy: Entity| {
            let mut touches = Vec::new();
            for index in 0..100 {
                step(app);
                for touch in orbit_touches(app) {
                    touches.push((
                        index,
                        touch.trigger_unit == Some(enemy),
                        touch.depth,
                        touch.reaction,
                        touch.attack,
                    ));
                }
            }
            touches
        };
        let from_ability = shape(&mut ability_app, ability_enemy);
        let from_item = shape(&mut item_app, item_enemy);
        assert!(from_ability.len() >= 3);
        assert_eq!(from_ability, from_item);
        assert_eq!(
            health(&ability_app, ability_enemy),
            health(&item_app, item_enemy)
        );
    }

    // ---- Sniper ----

    const SNIPER: [AbilityId; 4] = [
        AbilityId::Shrapnel,
        AbilityId::Headshot,
        AbilityId::TakeAim,
        AbilityId::Assassinate,
    ];

    fn sniper_app() -> App {
        let mut app = qop_app();
        app.world_mut().resource_mut::<CombatRng>().seed(7);
        app
    }

    use crate::combat::CombatRng;

    /// The `on_shrapnel_tick` contexts written in the last update.
    fn shrapnel_ticks(app: &App) -> Vec<TriggerContext> {
        app.world()
            .resource::<Messages<AbilityTriggerEvent>>()
            .iter_current_update_messages()
            .filter(|event| event.0.trigger == AbilityTrigger::Custom("on_shrapnel_tick"))
            .map(|event| event.0.clone())
            .collect()
    }

    /// Casts Shrapnel (slot 0) on `point` and steps until `seconds` after the cast request,
    /// returning the elapsed time of every tick.
    fn cast_shrapnel(app: &mut App, caster: Entity, point: Vec3, seconds: f32) -> Vec<f32> {
        request(app, caster, 0, CastTarget::Point(point));
        let mut ticks = Vec::new();
        for index in 1..=(seconds / 0.05).round() as usize {
            step(app);
            ticks.extend(shrapnel_ticks(app).iter().map(|_| index as f32 * 0.05));
        }
        ticks
    }

    #[test]
    fn shrapnel_zone_waits_then_ticks_ten_times() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let inside = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 200.0));
        let outside = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 700.0));
        let ticks = cast_shrapnel(&mut app, caster, Vec3::new(1000.0, 0.0, 0.0), 14.0);

        assert_eq!(ticks.len(), 10, "{ticks:?}");
        // 0.3 s cast point plus the 1.2 s delay (plus the frames spent turning).
        assert!(
            ticks[0] >= 1.5,
            "nothing lands before the delay: {}",
            ticks[0]
        );
        assert!(ticks[0] < 2.2, "{}", ticks[0]);
        for pair in ticks.windows(2) {
            assert!((pair[1] - pair[0] - 1.0).abs() < 0.11, "{ticks:?}");
        }
        assert_eq!(
            health(&app, inside),
            1000.0 - 10.0 * 30.0,
            "10 x rank-1 DPS"
        );
        assert_eq!(health(&app, outside), 1000.0, "400 radius at rank 1");
        assert_eq!(zone_count(&mut app), 0);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 925.0);
    }

    #[test]
    fn shrapnel_ticks_scale_with_rank() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        app.world_mut()
            .get_mut::<AbilityLoadout>(caster)
            .unwrap()
            .slots[0]
            .rank = 7;
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 500.0));
        *app.world_mut().get_mut::<Health>(near).unwrap() = Health::new(5000.0);
        cast_shrapnel(&mut app, caster, Vec3::new(1000.0, 0.0, 0.0), 14.0);
        assert_eq!(
            health(&app, near),
            5000.0 - 10.0 * 120.0,
            "rank 7: 120 per tick, 550 radius reaches 500 away"
        );
    }

    #[test]
    fn shrapnel_zone_slows_units_inside_and_releases_after_leaving() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 0.0));
        request(
            &mut app,
            caster,
            0,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        for _ in 0..60 {
            step(&mut app);
            if status_of(&app, enemy, "shrapnel_slow").is_some() {
                break;
            }
        }
        let slow = status_of(&app, enemy, "shrapnel_slow").expect("slowed by a tick");
        assert!(
            (slow.move_speed + 36.0).abs() < 1e-3,
            "12% of 300: {}",
            slow.move_speed
        );
        assert!(
            slow.remaining <= 1.1 && slow.remaining > 1.0,
            "{}",
            slow.remaining
        );
        assert_eq!(
            app.world().get::<CombatStats>(enemy).unwrap().move_speed,
            264.0
        );

        // Leave the zone: the slow runs out 1.1 s after the last tick and nothing refreshes it.
        app.world_mut()
            .get_mut::<Transform>(enemy)
            .unwrap()
            .translation = Vec3::new(1000.0, 0.0, 900.0);
        for _ in 0..30 {
            step(&mut app);
        }
        assert!(status_of(&app, enemy, "shrapnel_slow").is_none());
        assert_eq!(
            app.world().get::<CombatStats>(enemy).unwrap().move_speed,
            300.0,
            "back to base speed exactly"
        );
    }

    #[test]
    fn shrapnel_slow_is_refreshed_not_compounded_while_inside() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 0.0));
        let ticks = cast_shrapnel(&mut app, caster, Vec3::new(1000.0, 0.0, 0.0), 8.0);
        assert!(ticks.len() >= 5);
        assert_eq!(
            app.world().get::<CombatStats>(enemy).unwrap().move_speed,
            264.0,
            "five refreshes later it is still 12% of base"
        );
    }

    /// Writes attack hits until the Headshot proc lands (a pseudo-random 40%), returning
    /// the number of hits it took.
    fn hit_until_headshot(app: &mut App, attacker: Entity, target: Entity) -> usize {
        for hits in 1..=40 {
            write_attack_hit(app, attacker, target);
            step(app);
            if status_of(app, target, "headshot_slow").is_some() {
                return hits;
            }
        }
        panic!("Headshot never proc'd in 40 hits");
    }

    #[test]
    fn headshot_proc_damages_slows_and_knocks_back() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        let hits = hit_until_headshot(&mut app, caster, enemy);
        assert!(
            hits <= 8,
            "a pseudo-random 40% proc cannot streak long: {hits}"
        );

        assert_eq!(
            health(&app, enemy),
            1000.0 - 20.0,
            "rank-1 physical damage, no armor"
        );
        let slow = status_of(&app, enemy, "headshot_slow").unwrap();
        assert!((slow.move_speed + 300.0).abs() < 1e-3, "a 100% slow");
        assert!(slow.remaining <= 0.2);
        let aps = status_of(&app, enemy, "headshot_attack_speed_flat").expect("attack speed cut");
        assert_eq!(aps.attack_speed_flat, -100.0);
        assert_eq!(
            app.world()
                .get::<CombatStats>(enemy)
                .unwrap()
                .effective_move_speed(app.world().get::<StatusEffects>(enemy)),
            scale::MIN_MOVE_SPEED,
            "floored, not stopped"
        );

        // Knocked 50 away from Sniper over 0.05 s.
        for _ in 0..3 {
            step(&mut app);
        }
        let x = position(&app, enemy).x;
        assert!(
            (x - 450.0).abs() < 1.0,
            "pushed away from the attacker: {x}"
        );

        // Everything but the knockback runs out with the 0.2 s window.
        for _ in 0..6 {
            step(&mut app);
        }
        assert!(status_of(&app, enemy, "headshot_slow").is_none());
        assert!(status_of(&app, enemy, "headshot_attack_speed_flat").is_none());
        let stats = app.world().get::<CombatStats>(enemy).unwrap();
        assert_eq!(stats.move_speed, 300.0);
        assert_eq!(stats.attack_speed_flat, 0.0);
    }

    #[test]
    fn headshot_and_shrapnel_slows_coexist() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(500.0, 0.0, 0.0));
        request(
            &mut app,
            caster,
            0,
            CastTarget::Point(Vec3::new(500.0, 0.0, 0.0)),
        );
        for _ in 0..60 {
            step(&mut app);
            if status_of(&app, enemy, "shrapnel_slow").is_some() {
                break;
            }
        }
        assert!(status_of(&app, enemy, "shrapnel_slow").is_some());
        hit_until_headshot(&mut app, caster, enemy);

        let ids: Vec<_> = app
            .world()
            .get::<StatusEffects>(enemy)
            .unwrap()
            .effects
            .iter()
            .map(|e| e.id)
            .collect();
        assert!(
            ids.contains(&"shrapnel_slow") && ids.contains(&"headshot_slow"),
            "{ids:?}"
        );
        assert_eq!(
            app.world().get::<CombatStats>(enemy).unwrap().move_speed,
            300.0 - 36.0 - 300.0,
            "the slows add; the walking speed floors at the minimum"
        );

        for _ in 0..8 {
            step(&mut app);
        }
        assert!(status_of(&app, enemy, "headshot_slow").is_none());
        assert!(
            status_of(&app, enemy, "shrapnel_slow").is_some(),
            "the 100% slow ending leaves Shrapnel's slow in place"
        );
        assert_eq!(
            app.world().get::<CombatStats>(enemy).unwrap().move_speed,
            264.0
        );
    }

    #[test]
    fn take_aim_adds_attack_range_per_rank_and_keeps_projectile_speed() {
        use crate::abilities::catalog::builtin;
        use crate::heroes::HeroId;
        use crate::items::apply_passives;

        let def = builtin(AbilityId::TakeAim);
        for (rank, total) in [(1, 785.0), (4, 1250.0), (7, 1715.0)] {
            let (mut health, mut mana, mut stats, mut attrs) = HeroId::Sniper.def().vitals();
            let before = crate::basic_attack::projectile_speed_for(stats.base_attack_range())
                + stats.attack_projectile_speed_bonus;
            let passives = passive::passives_at_rank(&def, rank);
            assert_eq!(passives.attack_range, total - 550.0, "rank {rank}");
            apply_passives(passives, &mut health, &mut mana, &mut stats, &mut attrs);
            assert_eq!(stats.attack_range, total, "rank {rank}");
            assert_eq!(stats.base_attack_range(), 550.0);
            assert!(!stats.is_melee());
            let after = crate::basic_attack::projectile_speed_for(stats.base_attack_range())
                + stats.attack_projectile_speed_bonus;
            assert!((before - 3000.0).abs() < 1e-3 && (after - before).abs() < 1e-3);
        }
        let none = passive::passives_at_rank(&def, 0);
        assert_eq!(none.attack_range, 0.0, "unlearned adds nothing");
    }

    /// Requests Assassinate on `target` and steps until it has fired (or `max_frames`),
    /// returning the number of frames it took and the projectiles spawned that frame.
    fn cast_assassinate(
        app: &mut App,
        caster: Entity,
        target: Entity,
        max_frames: usize,
    ) -> (Option<usize>, Vec<SpawnProjectileEvent>) {
        request(app, caster, 3, CastTarget::Unit(target));
        let mut fired_at = None;
        let mut spawned = Vec::new();
        for frame in 1..=max_frames {
            step(app);
            if fired_at.is_none() && !fired_events(app).is_empty() {
                fired_at = Some(frame);
            }
            spawned.extend(
                app.world()
                    .resource::<Messages<SpawnProjectileEvent>>()
                    .iter_current_update_messages()
                    .copied(),
            );
            if fired_at.is_some() {
                break;
            }
        }
        (fired_at, spawned)
    }

    #[test]
    fn assassinate_fires_after_two_second_aim_and_mini_stuns() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(2000.0, 0.0, 0.0));
        // Someone else's long stun is already on the target (it outlasts the 2 s aim).
        app.world_mut()
            .get_mut::<StatusEffects>(target)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::stunned(10.0));

        let (fired_at, spawned) = cast_assassinate(&mut app, caster, target, 60);
        let fired_at = fired_at.expect("the shot fires");
        let seconds = fired_at as f32 * 0.05;
        assert!(
            seconds >= 1.95,
            "not before the 2 s aim: fired at {seconds}"
        );
        assert!(seconds <= 2.15, "and not long after: fired at {seconds}");
        assert_eq!(
            app.world().get::<Mana>(caster).unwrap().current,
            1000.0 - 175.0,
            "mana is paid when it fires"
        );
        let slot = &app.world().get::<AbilityLoadout>(caster).unwrap().slots[3];
        assert!(slot.cooldown.remaining > 0.0);
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].speed, Some(1500.0));
        assert_eq!(spawned[0].target, Some(target), "a homing shot");

        // The shot lands: magical damage and the 0.1 s mini stun under its own id.
        app.world_mut().write_message(hit(
            spawned[0].payload,
            Vec3::new(2000.0, 0.0, 0.0),
            Some(target),
            vec![],
        ));
        step(&mut app);
        assert_eq!(
            health(&app, target),
            1000.0 - 300.0,
            "rank-1 magical damage"
        );
        let mini = status_of(&app, target, "assassinate_stun").expect("mini stun");
        assert!(mini.stunned && mini.remaining <= 0.1);
        let foreign = status_of(&app, target, "stun").expect("the foreign stun survives");
        assert!(
            foreign.remaining > 7.5,
            "10 s minus the 2.1 s that passed, never replaced by the 0.1 s stun: {}",
            foreign.remaining
        );
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(status_of(&app, target, "assassinate_stun").is_none());
        assert!(
            status_of(&app, target, "stun").is_some(),
            "the long stun was never shortened"
        );
    }

    #[test]
    fn assassinate_does_not_fire_before_the_aim_completes() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(2000.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Unit(target));
        for _ in 0..38 {
            step(&mut app);
            assert!(fired_events(&app).is_empty());
        }
        assert!(
            app.world().get::<Activation>(caster).is_some(),
            "still aiming at 1.9 s"
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 1000.0);
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(
            app.world().get::<Activation>(caster).is_some(),
            "in backswing after the shot"
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 825.0);
    }

    #[test]
    fn assassinate_cancels_free_when_target_leaves_buffer() {
        // 3000 cast range + 600 buffer: 3700 breaks the lock, 3300 does not.
        for (distance, fires) in [(3700.0, false), (3300.0, true)] {
            let mut app = sniper_app();
            let caster = spawn_caster(&mut app, SNIPER);
            let target = spawn_unit(&mut app, Team::Dire, Vec3::new(2800.0, 0.0, 0.0));
            request(&mut app, caster, 3, CastTarget::Unit(target));
            for _ in 0..10 {
                step(&mut app);
            }
            assert!(app.world().get::<Activation>(caster).is_some());
            app.world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(distance, 0.0, 0.0);
            let mut fired = false;
            for _ in 0..60 {
                step(&mut app);
                fired |= !fired_events(&app).is_empty();
            }
            assert_eq!(fired, fires, "target at {distance}");
            let mana = app.world().get::<Mana>(caster).unwrap().current;
            let slot = &app.world().get::<AbilityLoadout>(caster).unwrap().slots[3];
            if fires {
                assert_eq!(mana, 825.0);
                assert!(slot.cooldown.remaining > 0.0);
            } else {
                assert_eq!(mana, 1000.0, "a broken lock is free");
                assert_eq!(slot.cooldown.remaining, 0.0);
            }
        }
    }
    /// Every damage event written in the last update.
    fn damage_events(app: &App) -> Vec<DamageEvent> {
        app.world()
            .resource::<Messages<DamageEvent>>()
            .iter_current_update_messages()
            .cloned()
            .collect()
    }

    #[test]
    fn headshot_damage_is_attack_bonus_damage_and_shrapnel_ticks_are_not() {
        use crate::combat::DamageKind;

        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        let mut headshot = None;
        for _ in 0..40 {
            write_attack_hit(&mut app, caster, enemy);
            step(&mut app);
            headshot = damage_events(&app)
                .into_iter()
                .find(|event| event.target == enemy);
            if headshot.is_some() {
                break;
            }
        }
        let headshot = headshot.expect("Headshot proc'd");
        assert_eq!(headshot.kind, DamageKind::AttackBonus);
        assert!(!headshot.is_spell, "Headshot is not spell damage");
        assert_eq!(headshot.damage_type, DamageType::Physical);
        assert_eq!(headshot.source, Some(caster));

        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 0.0));
        request(
            &mut app,
            caster,
            0,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        let mut tick = None;
        for _ in 0..60 {
            step(&mut app);
            tick = damage_events(&app)
                .into_iter()
                .find(|event| event.target == enemy);
            if tick.is_some() {
                break;
            }
        }
        let tick = tick.expect("Shrapnel ticked");
        assert_ne!(tick.kind, DamageKind::AttackBonus);
        assert!(tick.is_spell, "Shrapnel ticks are spell damage");
        assert_eq!(tick.damage_type, DamageType::Magical);
    }

    #[test]
    fn headshot_knockback_points_away_from_the_attacker_on_every_side() {
        for (attacker, target, expected) in [
            (
                Vec3::new(500.0, 0.0, 0.0),
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(50.0, 0.0, 0.0),
            ),
            (
                Vec3::new(0.0, 0.0, 500.0),
                Vec3::new(0.0, 0.0, 100.0),
                Vec3::new(0.0, 0.0, 50.0),
            ),
            (
                Vec3::new(0.0, 0.0, -300.0),
                Vec3::new(0.0, 0.0, 100.0),
                Vec3::new(0.0, 0.0, 150.0),
            ),
        ] {
            let mut app = sniper_app();
            let caster = spawn_caster(&mut app, SNIPER);
            app.world_mut()
                .get_mut::<Transform>(caster)
                .unwrap()
                .translation = attacker;
            let enemy = spawn_unit(&mut app, Team::Dire, target);
            hit_until_headshot(&mut app, caster, enemy);
            for _ in 0..4 {
                step(&mut app);
            }
            let at = position(&app, enemy);
            assert!(
                at.distance(expected) < 1.0,
                "attacker {attacker:?} target {target:?}: expected {expected:?}, got {at:?}"
            );
        }
    }

    #[test]
    fn shrapnel_fires_exactly_ten_ticks_at_uneven_frame_times() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 0.0));
        request(
            &mut app,
            caster,
            0,
            CastTarget::Point(Vec3::new(1000.0, 0.0, 0.0)),
        );
        let frames = [0.016, 0.033, 0.2, 0.007, 0.5, 0.041];
        let mut total = 0.0;
        let mut ticks = 0;
        let mut index = 0;
        while total < 16.0 {
            let dt = frames[index % frames.len()];
            index += 1;
            total += dt;
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_secs_f32(dt));
            app.update();
            let in_frame = shrapnel_ticks(&app).len();
            // A 0.5 s frame can owe at most one tick; ticks never bunch beyond what is overdue.
            assert!(in_frame <= 1, "{in_frame} ticks in one {dt}s frame");
            ticks += in_frame;
        }
        assert_eq!(ticks, 10, "10 ticks in total at any frame rate");
        assert_eq!(health(&app, enemy), 1000.0 - 10.0 * 30.0);
        assert_eq!(zone_count(&mut app), 0, "the zone is gone after 10 s");
        assert!(status_of(&app, enemy, "shrapnel_slow").is_none());
        assert_eq!(
            app.world().get::<CombatStats>(enemy).unwrap().move_speed,
            300.0
        );
    }

    #[test]
    fn a_broken_assassinate_lock_leaves_no_state_behind_and_can_be_recast() {
        let mut app = sniper_app();
        let caster = spawn_caster(&mut app, SNIPER);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(2800.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Unit(target));
        for _ in 0..10 {
            step(&mut app);
        }
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(3700.0, 0.0, 0.0);
        step(&mut app);
        assert!(
            app.world().get::<Activation>(caster).is_none(),
            "lock broken"
        );
        assert!(app.world().get::<QueuedActivation>(caster).is_none());
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 1000.0);

        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(2800.0, 0.0, 0.0);
        request(&mut app, caster, 3, CastTarget::Unit(target));
        let mut fired = false;
        for _ in 0..60 {
            step(&mut app);
            fired |= !fired_events(&app).is_empty();
        }
        assert!(fired, "a fresh cast after a broken lock still fires");
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 825.0);
    }

    // ---- Ursa ----

    const URSA: [AbilityId; 4] = [
        AbilityId::Earthshock,
        AbilityId::Overpower,
        AbilityId::FurySwipes,
        AbilityId::Enrage,
    ];

    fn ursa_app() -> App {
        qop_app()
    }

    fn set_rank(app: &mut App, caster: Entity, slot: usize, rank: u32) {
        app.world_mut()
            .get_mut::<AbilityLoadout>(caster)
            .unwrap()
            .slots[slot]
            .rank = rank;
    }

    fn attack_speed_bonus(app: &App, unit: Entity) -> f32 {
        app.world()
            .get::<CombatStats>(unit)
            .unwrap()
            .attack_speed_flat
    }

    /// Casts Earthshock and runs long enough for the leap and the shock to finish.
    fn cast_earthshock(app: &mut App, caster: Entity) {
        request(app, caster, 0, CastTarget::None);
        for _ in 0..16 {
            step(app);
        }
    }

    #[test]
    fn earthshock_leaps_forward_then_shocks_at_landing() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        // The caster faces -Z, so it lands at z = -250. 385 reaches 300 beyond the landing
        // point, but not 300 behind the take-off point (550 from the landing point).
        let ahead = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -550.0));
        let behind = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 300.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(0.0, 0.0, -300.0));
        cast_earthshock(&mut app, caster);

        let landed = position(&app, caster);
        assert!(
            landed.distance(Vec3::new(0.0, 0.0, -250.0)) < 1.0,
            "leapt 250 forward: {landed:?}"
        );
        assert_eq!(health(&app, ahead), 1000.0 - 75.0, "75 magical, no resist");
        let slow = status_of(&app, ahead, "earthshock_slow").expect("slowed");
        assert!((slow.move_speed + 66.0).abs() < 1e-3, "22% of 300");
        assert!(slow.remaining > 3.0 && slow.remaining <= 4.0);
        assert_eq!(health(&app, behind), 1000.0, "out of reach of the landing");
        assert!(status_of(&app, behind, "earthshock_slow").is_none());
        assert_eq!(health(&app, ally), 1000.0);
        assert_eq!(health(&app, caster), 1000.0, "never hits itself");
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 905.0);
    }

    #[test]
    fn earthshock_ranks_scale_damage_slow_and_cooldown() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        set_rank(&mut app, caster, 0, 7);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -300.0));
        cast_earthshock(&mut app, caster);
        assert_eq!(health(&app, enemy), 1000.0 - 375.0);
        let slow = status_of(&app, enemy, "earthshock_slow").unwrap();
        assert!((slow.move_speed + 174.0).abs() < 1e-3, "58% of 300");
        let cooldown = app.world().get::<AbilityLoadout>(caster).unwrap().slots[0]
            .cooldown
            .remaining;
        assert!(
            cooldown > 2.0 && cooldown <= 3.0,
            "rank 7 cooldown 3: {cooldown}"
        );
    }

    #[test]
    fn earthshock_stops_at_terrain_and_shocks_there() {
        let mut app = ursa_app();
        app.world_mut()
            .resource_mut::<NavGrid>()
            .rebuild(vec![NavShape::Circle {
                center: Vec2::new(0.0, -160.0),
                radius: 40.0,
            }]);
        let caster = spawn_caster(&mut app, URSA);
        app.world_mut()
            .entity_mut(caster)
            .insert(CollisionRadius(30.0));
        // 440 from the take-off point, so only a shock centred near the stop point reaches it.
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -440.0));
        cast_earthshock(&mut app, caster);
        let landed = position(&app, caster);
        assert!(
            landed.z < -10.0 && landed.z > -150.0,
            "stopped at the tree, short of the 250 leap: {landed:?}"
        );
        assert_eq!(
            health(&app, enemy),
            1000.0 - 75.0,
            "shock at the stop point"
        );
    }

    #[test]
    fn rooted_earthshock_shocks_in_place() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 300.0));
        // 600 from the take-off point: out of reach.
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -450.0));
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::rooted(5.0));
        cast_earthshock(&mut app, caster);
        assert!(position(&app, caster).length() < 1.0, "did not move");
        assert_eq!(health(&app, near), 1000.0 - 75.0);
        assert_eq!(health(&app, far), 1000.0);
    }

    #[test]
    fn overpower_lasts_exactly_rank_attacks() {
        for (rank, attacks) in [(1, 3), (4, 6), (7, 9)] {
            let mut app = ursa_app();
            let caster = spawn_caster(&mut app, URSA);
            set_rank(&mut app, caster, 1, rank);
            let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
            request(&mut app, caster, 1, CastTarget::None);
            for _ in 0..8 {
                step(&mut app);
            }
            assert_eq!(attack_speed_bonus(&app, caster), 400.0, "rank {rank}");
            let status = status_of(&app, caster, "overpower_attack_speed_flat").unwrap();
            assert_eq!(status.attack_charges, attacks);
            for used in 1..=attacks {
                write_attack_release(&mut app, caster, enemy);
                step(&mut app);
                let live = status_of(&app, caster, "overpower_attack_speed_flat").is_some();
                assert_eq!(live, used < attacks, "rank {rank} after attack {used}");
            }
            assert_eq!(attack_speed_bonus(&app, caster), 0.0, "reverted");
            // The next attack is not boosted and nothing breaks.
            write_attack_release(&mut app, caster, enemy);
            step(&mut app);
            assert_eq!(attack_speed_bonus(&app, caster), 0.0);
        }
    }

    #[test]
    fn overpower_expires_after_twenty_seconds_unused() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        request(&mut app, caster, 1, CastTarget::None);
        for _ in 0..8 {
            step(&mut app);
        }
        assert_eq!(attack_speed_bonus(&app, caster), 400.0);
        for _ in 0..410 {
            step(&mut app);
        }
        assert!(status_of(&app, caster, "overpower_attack_speed_flat").is_none());
        assert_eq!(attack_speed_bonus(&app, caster), 0.0);
    }

    #[test]
    fn overpower_recast_resets_the_charges() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        request(&mut app, caster, 1, CastTarget::None);
        for _ in 0..8 {
            step(&mut app);
        }
        for _ in 0..2 {
            write_attack_release(&mut app, caster, enemy);
            step(&mut app);
        }
        assert_eq!(
            status_of(&app, caster, "overpower_attack_speed_flat")
                .unwrap()
                .attack_charges,
            1
        );
        // Clear the cooldown and recast.
        app.world_mut()
            .get_mut::<AbilityLoadout>(caster)
            .unwrap()
            .slots[1]
            .cooldown
            .remaining = 0.0;
        request(&mut app, caster, 1, CastTarget::None);
        for _ in 0..8 {
            step(&mut app);
        }
        let status = status_of(&app, caster, "overpower_attack_speed_flat").unwrap();
        assert_eq!(status.attack_charges, 3);
        assert_eq!(
            attack_speed_bonus(&app, caster),
            400.0,
            "refreshed, not doubled"
        );
    }

    #[test]
    fn fury_swipes_bonus_grows_per_hit_and_resets_after_expiry() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let mut dealt = Vec::new();
        for _ in 0..3 {
            let before = health(&app, enemy);
            write_attack_hit(&mut app, caster, enemy);
            step(&mut app);
            step(&mut app);
            dealt.push(before - health(&app, enemy));
        }
        assert_eq!(dealt, [12.0, 24.0, 36.0], "stack included, rank 1: 12 each");
        let stacks = app
            .world()
            .get::<StatusEffects>(enemy)
            .unwrap()
            .stacks("fury_swipes_stacks");
        assert_eq!(stacks, 3);
        // Dispels cannot remove the stacks.
        app.world_mut().write_message(StatusEffectEvent::new(
            Some(enemy),
            enemy,
            crate::status::StatusChange::Dispel {
                kind: crate::status::StatusKind::Debuff,
                strength: crate::status::DispelStrength::Strong,
            },
        ));
        step(&mut app);
        assert_eq!(
            app.world()
                .get::<StatusEffects>(enemy)
                .unwrap()
                .stacks("fury_swipes_stacks"),
            3
        );
        // 8 s (rank 1) without another hit: the stacks are gone and the count restarts.
        for _ in 0..170 {
            step(&mut app);
        }
        assert_eq!(
            app.world()
                .get::<StatusEffects>(enemy)
                .unwrap()
                .stacks("fury_swipes_stacks"),
            0
        );
        let before = health(&app, enemy);
        write_attack_hit(&mut app, caster, enemy);
        step(&mut app);
        step(&mut app);
        assert_eq!(before - health(&app, enemy), 12.0);
    }

    #[test]
    fn fury_swipes_stacks_are_per_target() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        let a = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let b = spawn_unit(&mut app, Team::Dire, Vec3::new(-100.0, 0.0, 0.0));
        for target in [a, a, b, a] {
            write_attack_hit(&mut app, caster, target);
            step(&mut app);
            step(&mut app);
        }
        assert_eq!(health(&app, a), 1000.0 - (12.0 + 24.0 + 36.0));
        assert_eq!(health(&app, b), 1000.0 - 12.0);
    }

    #[test]
    fn fury_swipes_refreshes_the_stack_duration_and_ranks_scale() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        set_rank(&mut app, caster, 2, 7);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        for _ in 0..2 {
            write_attack_hit(&mut app, caster, enemy);
            step(&mut app);
            step(&mut app);
        }
        assert_eq!(
            health(&app, enemy),
            1000.0 - (60.0 + 120.0),
            "rank 7: 60 a stack"
        );
        let stack = status_of(&app, enemy, "fury_swipes_stacks").unwrap();
        assert!(
            stack.remaining > 31.0,
            "32 s at rank 7: {}",
            stack.remaining
        );
    }

    #[test]
    fn enrage_strong_dispels_then_reduces_damage_and_debuff_durations() {
        use crate::status::{StatusChange, StatusEffect};

        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(2000.0, 0.0, 0.0));
        // A basic and a strong-only debuff, and one nothing dispels.
        for effect in [
            StatusEffect {
                dispel_type: crate::status::DispelType::Basic,
                ..StatusEffect::debuff("weak", 30.0)
            },
            StatusEffect {
                dispel_type: crate::status::DispelType::Strong,
                ..StatusEffect::debuff("strong", 30.0)
            },
            StatusEffect {
                dispel_type: crate::status::DispelType::None,
                ..StatusEffect::debuff("sticky", 30.0)
            },
        ] {
            app.world_mut().write_message(StatusEffectEvent::new(
                Some(enemy),
                caster,
                StatusChange::Apply(effect),
            ));
        }
        step(&mut app);
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(status_of(&app, caster, "weak").is_none());
        assert!(status_of(&app, caster, "strong").is_none(), "strong dispel");
        assert!(status_of(&app, caster, "stun").is_none());
        assert!(
            status_of(&app, caster, "sticky").is_some(),
            "undispellable stays"
        );
        let reduction =
            status_of(&app, caster, "enrage_damage_reduction").expect("damage reduction");
        assert!(reduction.remaining > 3.5 && reduction.remaining <= 4.0);
        assert!(status_of(&app, caster, "enrage_status_resistance").is_some());

        // 80% less damage, after armor and resist (none here): 100 pure -> 20.
        app.world_mut()
            .write_message(DamageEvent::new(caster, 100.0, DamageType::Pure));
        step(&mut app);
        assert!(
            (health(&app, caster) - 980.0).abs() < 1e-3,
            "{}",
            health(&app, caster)
        );

        // A hostile debuff lands at half its length; a self-applied one does not.
        app.world_mut().write_message(StatusEffectEvent::new(
            Some(enemy),
            caster,
            StatusChange::Apply(StatusEffect::stunned(2.0)),
        ));
        app.world_mut().write_message(StatusEffectEvent::new(
            Some(caster),
            caster,
            StatusChange::Apply(StatusEffect::debuff("own", 2.0)),
        ));
        step(&mut app);
        let stun = status_of(&app, caster, "stun").unwrap();
        assert!(
            stun.remaining <= 1.0 && stun.remaining > 0.9,
            "{}",
            stun.remaining
        );
        assert!(status_of(&app, caster, "own").unwrap().remaining > 1.9);

        // After the 4 s both effects are gone: full damage, full-length debuffs.
        for _ in 0..90 {
            step(&mut app);
        }
        assert!(status_of(&app, caster, "enrage_damage_reduction").is_none());
        assert!(status_of(&app, caster, "enrage_status_resistance").is_none());
        let before = health(&app, caster);
        app.world_mut()
            .write_message(DamageEvent::new(caster, 100.0, DamageType::Pure));
        step(&mut app);
        assert!((before - health(&app, caster) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn enrage_ranks_scale_duration_and_cooldown() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        set_rank(&mut app, caster, 3, 4);
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        let reduction = status_of(&app, caster, "enrage_damage_reduction").unwrap();
        assert!(reduction.remaining > 5.0 && reduction.remaining <= 5.5);
        let cooldown = app.world().get::<AbilityLoadout>(caster).unwrap().slots[3]
            .cooldown
            .remaining;
        assert!(cooldown > 14.0 && cooldown <= 15.0, "{cooldown}");
    }

    // ---- Ursa verification: G1/G7 regressions on Skewer and Vanguard Dash, Enrage edges ----

    #[test]
    fn rooted_skewer_changes_nothing_even_though_it_reports_its_end() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, MAGNUS);
        let victim = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::rooted(5.0));
        request(&mut app, caster, 2, CastTarget::Point(Vec3::X * 600.0));
        for _ in 0..40 {
            step(&mut app);
        }
        assert!(position(&app, caster).length() < 1e-3, "did not move");
        assert_eq!(position(&app, victim), Vec3::new(100.0, 0.0, 0.0));
        assert_eq!(health(&app, victim), 1000.0, "no end-of-dash damage");
        assert!(status_of(&app, victim, "skewer_slow").is_none());
        assert!(status_of(&app, victim, "skewer_stun").is_none());
    }

    #[test]
    fn skewer_clicked_on_the_caster_dashes_along_the_facing() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, MAGNUS);
        step(&mut app);
        let here = position(&app, caster);
        request(&mut app, caster, 2, CastTarget::Point(here));
        for _ in 0..80 {
            step(&mut app);
        }
        let moved = position(&app, caster) - here;
        assert!(moved.length() > 100.0, "G1: it dashes: {moved:?}");
        assert!(
            moved.z < 0.0 && moved.x.abs() < 1.0,
            "along -Z facing: {moved:?}"
        );
        assert!(app.world().get::<ForcedMovement>(caster).is_none());
    }

    #[test]
    fn vanguard_dash_toward_a_point_is_unchanged_and_self_click_goes_forward() {
        for self_click in [false, true] {
            let mut app = qop_app();
            let caster = spawn_caster(
                &mut app,
                [
                    AbilityId::Dash,
                    AbilityId::Bolt,
                    AbilityId::Execute,
                    AbilityId::Bulwark,
                ],
            );
            step(&mut app);
            let here = position(&app, caster);
            let aim = if self_click {
                here
            } else {
                here + Vec3::X * 60.0
            };
            request(&mut app, caster, 0, CastTarget::Point(aim));
            for _ in 0..60 {
                step(&mut app);
            }
            let moved = position(&app, caster) - here;
            if self_click {
                assert!(moved.z < -50.0 && moved.x.abs() < 1.0, "forward: {moved:?}");
            } else {
                assert!(
                    (moved - Vec3::X * 60.0).length() < 2.0,
                    "short aim wins: {moved:?}"
                );
            }
        }
    }

    #[test]
    fn enrage_recast_and_later_strong_dispels_keep_its_own_buffs() {
        use crate::status::{DispelStrength, StatusChange, StatusKind};

        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        assert!(status_of(&app, caster, "enrage_damage_reduction").is_some());
        // Recast: the strong dispel row must not strip the buffs it is about to refresh.
        app.world_mut()
            .get_mut::<AbilityLoadout>(caster)
            .unwrap()
            .slots[3]
            .cooldown
            .remaining = 0.0;
        request(&mut app, caster, 3, CastTarget::None);
        for _ in 0..4 {
            step(&mut app);
        }
        // A strong dispel of debuffs, from anyone, leaves buffs alone.
        for kind in [StatusKind::Debuff, StatusKind::Buff] {
            let before = [
                status_of(&app, caster, "enrage_damage_reduction").is_some(),
                status_of(&app, caster, "enrage_status_resistance").is_some(),
            ];
            assert_eq!(before, [true, true]);
            if kind == StatusKind::Buff {
                break;
            }
            app.world_mut().write_message(StatusEffectEvent::new(
                Some(caster),
                caster,
                StatusChange::Dispel {
                    kind,
                    strength: DispelStrength::Strong,
                },
            ));
            step(&mut app);
        }
        let stacks = app
            .world()
            .get::<StatusEffects>(caster)
            .unwrap()
            .effects
            .iter()
            .filter(|e| e.id == "enrage_damage_reduction")
            .count();
        assert_eq!(stacks, 1, "refreshed, not duplicated");
        // 100 pure still 20 (not 4: reductions do not stack on a refresh).
        app.world_mut()
            .write_message(DamageEvent::new(caster, 100.0, DamageType::Pure));
        step(&mut app);
        assert!((health(&app, caster) - 980.0).abs() < 1e-3);
    }

    #[test]
    fn overpower_charges_survive_hits_by_extra_attacks_and_fury_swipes_still_stacks() {
        let mut app = ursa_app();
        let caster = spawn_caster(&mut app, URSA);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        request(&mut app, caster, 1, CastTarget::None);
        for _ in 0..8 {
            step(&mut app);
        }
        // A release that misses (no hit event) still spends a charge; a hit event alone
        // (as an extra attack's landing) does not.
        write_attack_release(&mut app, caster, enemy);
        step(&mut app);
        let charges = |app: &App| {
            status_of(app, caster, "overpower_attack_speed_flat")
                .unwrap()
                .attack_charges
        };
        assert_eq!(charges(&app), 2);
        write_attack_hit(&mut app, caster, enemy);
        step(&mut app);
        assert_eq!(
            charges(&app),
            2,
            "a hit without a normal release spends nothing"
        );
    }

    // --- Aura trigger ---

    fn aura_app() -> App {
        let mut app = test_app();
        app.add_systems(
            Update,
            crate::status::tick_status_effects.before(reactive_triggers::status_triggers),
        );
        app
    }

    fn buckler_carrier(app: &mut App, team: Team, pos: Vec3) -> Entity {
        let unit = spawn_unit(app, team, pos);
        app.world_mut().entity_mut(unit).insert(Inventory::empty());
        give_item(app, unit, ItemId::Buckler);
        unit
    }

    const AURA_ID: &str = "buckler_armor";

    #[test]
    fn aura_applies_on_the_first_frame_to_the_owner_and_allies_in_radius() {
        let mut app = aura_app();
        let owner = buckler_carrier(&mut app, Team::Radiant, Vec3::ZERO);
        let near = spawn_unit(&mut app, Team::Radiant, Vec3::new(1100.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Radiant, Vec3::new(1300.0, 0.0, 0.0));
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        step(&mut app);
        assert!(has_status(&app, owner, AURA_ID), "the owner is an ally");
        assert!(has_status(&app, near, AURA_ID));
        assert!(!has_status(&app, far, AURA_ID), "outside the radius");
        assert!(!has_status(&app, enemy, AURA_ID), "enemies are not buffed");
        assert_eq!(armor(&app, near), 2.0);
    }

    #[test]
    fn buckler_carrier_gets_exactly_one_aura_bonus_on_top_of_the_passive() {
        let mut app = aura_app();
        let owner = spawn_unit(&mut app, Team::Radiant, Vec3::ZERO);
        let base = armor(&app, owner);
        app.world_mut().entity_mut(owner).insert(Inventory::empty());
        give_item(&mut app, owner, ItemId::Buckler);
        for _ in 0..10 {
            step(&mut app);
            // The +1 passive is applied by the stat pass (not in this app; see items.rs);
            // the aura adds exactly +2, once, so the carrier ends at +3.
            assert_eq!(armor(&app, owner), base + 2.0);
        }
    }

    #[test]
    fn an_enemy_carriers_aura_buffs_only_its_own_team() {
        let mut app = aura_app();
        let carrier = buckler_carrier(&mut app, Team::Dire, Vec3::ZERO);
        let enemy = spawn_unit(&mut app, Team::Radiant, Vec3::new(100.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        for _ in 0..6 {
            step(&mut app);
        }
        assert!(has_status(&app, carrier, AURA_ID));
        assert!(has_status(&app, ally, AURA_ID));
        assert!(!has_status(&app, enemy, AURA_ID));
        assert_eq!(armor(&app, enemy), 0.0);
    }

    #[test]
    fn aura_lingers_then_expires_after_the_unit_leaves() {
        let mut app = aura_app();
        let _owner = buckler_carrier(&mut app, Team::Radiant, Vec3::ZERO);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(500.0, 0.0, 0.0));
        step(&mut app);
        assert!(has_status(&app, ally, AURA_ID));
        app.world_mut()
            .get_mut::<Transform>(ally)
            .unwrap()
            .translation
            .x = 5000.0;
        step(&mut app);
        step(&mut app);
        assert!(has_status(&app, ally, AURA_ID), "still within the linger");
        for _ in 0..12 {
            step(&mut app);
        }
        assert!(!has_status(&app, ally, AURA_ID), "expired");
        assert_eq!(armor(&app, ally), 0.0, "and its bonus reverted");
    }

    #[test]
    fn aura_ends_when_the_item_is_removed_and_when_the_owner_dies() {
        let mut app = aura_app();
        let owner = buckler_carrier(&mut app, Team::Radiant, Vec3::ZERO);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(500.0, 0.0, 0.0));
        step(&mut app);
        assert!(has_status(&app, ally, AURA_ID));
        app.world_mut().get_mut::<Inventory>(owner).unwrap().slots = Default::default();
        for _ in 0..14 {
            step(&mut app);
        }
        assert!(
            !has_status(&app, ally, AURA_ID),
            "sold: gone after the linger"
        );

        give_item(&mut app, owner, ItemId::Buckler);
        step(&mut app);
        assert!(has_status(&app, ally, AURA_ID));
        set_health(&mut app, owner, 0.0);
        for _ in 0..14 {
            step(&mut app);
        }
        assert!(!has_status(&app, ally, AURA_ID), "dead owner: no pulses");
    }

    #[test]
    fn identical_auras_from_two_carriers_do_not_stack() {
        let mut app = aura_app();
        let _a = buckler_carrier(&mut app, Team::Radiant, Vec3::ZERO);
        let _b = buckler_carrier(&mut app, Team::Radiant, Vec3::new(100.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(200.0, 0.0, 0.0));
        for _ in 0..10 {
            step(&mut app);
            assert_eq!(armor(&app, ally), 2.0);
        }
        let count = app
            .world()
            .get::<StatusEffects>(ally)
            .unwrap()
            .effects
            .iter()
            .filter(|e| e.id == AURA_ID)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn two_copies_in_one_inventory_share_one_pulse() {
        let mut app = aura_app();
        let owner = buckler_carrier(&mut app, Team::Radiant, Vec3::ZERO);
        give_item(&mut app, owner, ItemId::Buckler);
        for _ in 0..6 {
            step(&mut app);
        }
        assert_eq!(armor(&app, owner), 2.0);
        let tracking = app
            .world()
            .get::<reactive_triggers::TriggerTracking>(owner)
            .unwrap();
        assert_eq!(tracking.intervals.len(), 1);
    }

    #[test]
    fn aura_pulses_fire_no_status_triggers() {
        let mut app = aura_app();
        let _owner = buckler_carrier(&mut app, Team::Radiant, Vec3::ZERO);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(500.0, 0.0, 0.0));
        let mut lifecycle = 0;
        for _ in 0..20 {
            step(&mut app);
            lifecycle += current_messages::<crate::status::StatusLifecycleEvent>(&app).len();
        }
        assert!(has_status(&app, ally, AURA_ID));
        assert_eq!(lifecycle, 0, "reactive statuses are silent");
        let status = status_of(&app, ally, AURA_ID).unwrap();
        assert!(status.reactive);
    }

    #[test]
    fn aura_never_lapses_between_pulses() {
        let mut app = aura_app();
        let _owner = buckler_carrier(&mut app, Team::Radiant, Vec3::ZERO);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(500.0, 0.0, 0.0));
        step(&mut app);
        // Sampled every 50 ms frame for 3 s.
        for frame in 0..60 {
            step(&mut app);
            assert!(has_status(&app, ally, AURA_ID), "lapsed at frame {frame}");
        }
    }

    #[test]
    fn every_aura_row_outlasts_the_pulse_period_with_margin() {
        let mut checked = 0;
        for &item in ItemId::all() {
            for entry in item.definition().effects {
                if entry.trigger != AbilityTrigger::Aura {
                    continue;
                }
                let AbilityEffect::ApplyStatus { duration, .. } = entry.effect else {
                    panic!("{item:?}: an aura row must apply a status");
                };
                assert!(
                    duration.at(1) > 1.5 * reactive_triggers::AURA_PULSE_SECS,
                    "{item:?}: {}",
                    duration.at(1)
                );
                checked += 1;
            }
        }
        assert!(checked >= 3, "the aura items are in the catalog");
    }

    #[test]
    fn ability_aura_is_suppressed_by_break_but_item_aura_is_not() {
        let mut app = aura_app();
        let aura_entries: Vec<AbilityEffectEntry> = ItemId::Buckler
            .definition()
            .effects
            .iter()
            .copied()
            .filter(|e| e.trigger == AbilityTrigger::Aura)
            .collect();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(passive_def(aura_entries));
        let caster = spawn_caster(&mut app, PASSIVE_KIT);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(300.0, 0.0, 0.0));
        // `spawn_caster` already ran the first pulse, before the ally existed: wait for the next.
        for _ in 0..6 {
            step(&mut app);
        }
        assert!(has_status(&app, ally, AURA_ID), "the passive aura works");

        set_broken(&mut app, caster, true);
        for _ in 0..14 {
            step(&mut app);
        }
        assert!(!has_status(&app, ally, AURA_ID), "broken: it lapses");

        set_broken(&mut app, caster, false);
        for _ in 0..6 {
            step(&mut app);
        }
        assert!(has_status(&app, ally, AURA_ID), "back after the break");

        // An item aura ignores Break.
        let mut app = aura_app();
        let owner = buckler_carrier(&mut app, Team::Radiant, Vec3::ZERO);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(300.0, 0.0, 0.0));
        set_broken(&mut app, owner, true);
        for _ in 0..14 {
            step(&mut app);
        }
        assert!(has_status(&app, ally, AURA_ID));
    }

    // --- Break: ability passives off, items untouched ---

    const PASSIVE_KIT: [AbilityId; 4] = [
        AbilityId::StoneSkin,
        AbilityId::Dash,
        AbilityId::Bolt,
        AbilityId::Nova,
    ];

    fn passive_def(entries: Vec<AbilityEffectEntry>) -> AbilityDefinition {
        let mut def = AbilityDefinition::new(AbilityId::StoneSkin)
            .behavior(definition::AbilityBehavior::Passive);
        def.effects = entries;
        def
    }

    fn set_broken(app: &mut App, unit: Entity, broken: bool) {
        let mut statuses = app.world_mut().get_mut::<StatusEffects>(unit).unwrap();
        statuses.effects.retain(|e| e.id != "break");
        if broken {
            statuses
                .effects
                .push(crate::status::StatusEffect::broken(60.0));
        }
    }

    #[test]
    fn break_suppresses_on_attack_hit_passive() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(passive_def(vec![entry(
                AbilityTrigger::OnAttackHit,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(100.0),
            )]));
        let caster = spawn_caster(&mut app, PASSIVE_KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));

        write_attack_hit(&mut app, caster, target);
        step(&mut app);
        assert_eq!(health(&app, target), 900.0, "the passive procs normally");

        set_broken(&mut app, caster, true);
        write_attack_hit(&mut app, caster, target);
        step(&mut app);
        assert_eq!(health(&app, target), 900.0, "broken: no proc");

        set_broken(&mut app, caster, false);
        write_attack_hit(&mut app, caster, target);
        step(&mut app);
        assert_eq!(
            health(&app, target),
            800.0,
            "procs again once the break ends"
        );
    }

    #[test]
    fn break_does_not_suppress_active_abilities_or_cast_phase_effects() {
        let mut app = test_app();
        // An active ability that also reacts to attack hits is not a passive.
        app.world_mut().resource_mut::<AbilityDefinitions>().insert(
            AbilityDefinition::new(AbilityId::StoneSkin)
                .behavior(definition::AbilityBehavior::Active)
                .on(
                    AbilityTrigger::OnAttackHit,
                    EffectTarget::TriggerUnit,
                    fixed_magical_damage(100.0),
                ),
        );
        let caster = spawn_caster(&mut app, PASSIVE_KIT);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        set_broken(&mut app, caster, true);
        write_attack_hit(&mut app, caster, target);
        step(&mut app);
        assert_eq!(health(&app, target), 900.0);
    }

    #[test]
    fn break_drops_while_condition_status() {
        use definition::Condition;
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<AbilityDefinitions>()
            .insert(passive_def(vec![while_entry(
                Condition::HealthBelow { percent: 50 },
                "berserk",
            )]));
        let caster = spawn_caster(&mut app, PASSIVE_KIT);
        let base = armor(&app, caster);
        set_health(&mut app, caster, 100.0);
        step(&mut app);
        assert!(has_status(&app, caster, "berserk"));
        assert_eq!(armor(&app, caster), base + 10.0);

        set_broken(&mut app, caster, true);
        step(&mut app);
        assert!(!has_status(&app, caster, "berserk"), "held status dropped");
        assert_eq!(armor(&app, caster), base, "and its stat reverted");

        set_broken(&mut app, caster, false);
        step(&mut app);
        assert!(has_status(&app, caster, "berserk"), "restored after break");
        assert_eq!(armor(&app, caster), base + 10.0);
    }

    #[test]
    fn break_does_not_affect_item_passives() {
        use definition::Condition;
        let mut app = test_app();
        let _defs = install_test_item_definitions([item_with_effects(vec![
            while_entry(Condition::HealthBelow { percent: 50 }, "berserk"),
            entry(
                AbilityTrigger::OnAttackHit,
                EffectTarget::TriggerUnit,
                fixed_magical_damage(100.0),
            ),
        ])]);
        let unit = spawn_item_holder(&mut app, Team::Radiant, Vec3::ZERO);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(80.0, 0.0, 0.0));
        set_broken(&mut app, unit, true);
        set_health(&mut app, unit, 100.0);
        write_attack_hit(&mut app, unit, target);
        step(&mut app);
        step(&mut app);
        assert!(has_status(&app, unit, "berserk"), "item bonus still held");
        assert_eq!(health(&app, target), 900.0, "item proc still fires");
    }

    // ---- Slark ----

    const SLARK: [AbilityId; 4] = [
        AbilityId::DarkPact,
        AbilityId::Pounce,
        AbilityId::EssenceShift,
        AbilityId::ShadowDance,
    ];

    /// The Ursa test app plus the leash clamp, which the pounce tests need.
    fn slark_app() -> App {
        let mut app = qop_app();
        app.add_systems(Update, crate::movement::enforce_leashes);
        app
    }

    /// A Slark with hero attributes, so timed attribute modifiers have something to fold into.
    fn spawn_slark(app: &mut App) -> Entity {
        let caster = spawn_caster(app, SLARK);
        app.world_mut()
            .entity_mut(caster)
            .insert(crate::components::HeroAttributes::starter());
        caster
    }

    fn run_seconds(app: &mut App, seconds: f32) {
        for _ in 0..(seconds / 0.05).round() as usize {
            step(app);
        }
    }

    /// Damage events dealt to `unit` while running `seconds`.
    fn damage_to(app: &mut App, unit: Entity, seconds: f32) -> Vec<DamageEvent> {
        let mut hits = Vec::new();
        for _ in 0..(seconds / 0.05).round() as usize {
            step(app);
            hits.extend(
                current_messages::<DamageEvent>(app)
                    .into_iter()
                    .filter(|d| d.target == unit),
            );
        }
        hits
    }

    fn instances(app: &App, unit: Entity, id: &str) -> usize {
        app.world()
            .get::<StatusEffects>(unit)
            .unwrap()
            .effects
            .iter()
            .filter(|e| e.id == id)
            .count()
    }

    #[test]
    fn dark_pact_waits_then_pulses_ten_times_around_slark() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        // Nothing lands during the 1.5 s delay.
        let early = damage_to(&mut app, near, 1.4);
        assert!(early.is_empty(), "no pulse before the delay: {early:?}");
        assert_eq!(health(&app, near), 1000.0);
        let pulses = damage_to(&mut app, near, 1.6);
        assert_eq!(pulses.len(), 10, "ten pulses");
        assert!(
            pulses.iter().all(|d| d.amount == 7.5),
            "7.5 each: {pulses:?}"
        );
        assert_eq!(health(&app, near), 1000.0 - 75.0);
        assert_eq!(health(&app, far), 1000.0, "400 is outside the 325 radius");
        assert_eq!(zone_count(&mut app), 0, "the zone is gone after 2.5 s");
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 935.0);
    }

    #[test]
    fn dark_pact_dispels_debuffs_each_pulse() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        run_seconds(&mut app, 0.0);
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 1.55);
        {
            let mut statuses = app.world_mut().get_mut::<StatusEffects>(caster).unwrap();
            statuses
                .effects
                .push(crate::status::StatusEffect::debuff("test_slow", 30.0));
            statuses
                .effects
                .push(crate::status::StatusEffect::buff("test_buff", 30.0));
        }
        assert!(status_of(&app, caster, "test_slow").is_some());
        run_seconds(&mut app, 0.2);
        assert!(
            status_of(&app, caster, "test_slow").is_none(),
            "the next pulse sheds it"
        );
        assert!(status_of(&app, caster, "test_buff").is_some());

        // A debuff gained later in the window is shed by a later pulse too.
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::debuff("late_slow", 30.0));
        run_seconds(&mut app, 0.2);
        assert!(status_of(&app, caster, "late_slow").is_none());

        // After the zone ends nothing sheds debuffs any more.
        run_seconds(&mut app, 1.0);
        app.world_mut()
            .get_mut::<StatusEffects>(caster)
            .unwrap()
            .effects
            .push(crate::status::StatusEffect::debuff("after", 30.0));
        run_seconds(&mut app, 0.5);
        assert!(status_of(&app, caster, "after").is_some());
    }

    #[test]
    fn dark_pact_heals_fifteen_percent_of_damage_dealt() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 3.0);
        assert_eq!(health(&app, enemy), 925.0);
        let healed = health(&app, caster) - 500.0;
        assert!(
            (healed - 0.15 * 75.0).abs() < 1e-3,
            "15% of the 75 dealt: {healed}"
        );

        // No overkill: a nearly dead enemy only pays out what it lost.
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let weak = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        app.world_mut().get_mut::<Health>(weak).unwrap().current = 5.0;
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 500.0;
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 3.0);
        let healed = health(&app, caster) - 500.0;
        assert!(
            (healed - 0.15 * 5.0).abs() < 1e-3,
            "only the 5 health actually removed: {healed}"
        );
    }

    #[test]
    fn dark_pact_follows_slark_after_pounce() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        // Slark faces -Z and lands 700 away. This enemy is 900 from the cast point, so only
        // a zone that moved with him reaches it; the dash never touches it.
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -900.0));
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 0.1);
        request(&mut app, caster, 1, CastTarget::None);
        run_seconds(&mut app, 3.5);
        let landed = position(&app, caster);
        assert!(
            landed.distance(Vec3::new(0.0, 0.0, -700.0)) < 1.0,
            "leapt 700: {landed:?}"
        );
        assert_eq!(
            health(&app, enemy),
            1000.0 - 75.0,
            "all ten pulses were centred on the landing point"
        );
    }

    /// A Dire Slark standing at `pos`, which has cast Shadow Dance and is now untargetable.
    fn dancing_dire_slark(app: &mut App, pos: Vec3) -> Entity {
        let dancer = spawn_caster(app, SLARK);
        app.world_mut()
            .entity_mut(dancer)
            .insert((Team::Dire, Transform::from_translation(pos)));
        request(app, dancer, 3, CastTarget::None);
        run_seconds(app, 0.3);
        assert!(
            status_of(app, dancer, "untargetable").is_some(),
            "the dancer is untargetable"
        );
        dancer
    }

    #[test]
    fn pounce_contact_still_hits_a_shadow_dancing_enemy() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        // On the leap's path (he faces -Z); the dancer is not the full untargetable kind.
        let dancer = dancing_dire_slark(&mut app, Vec3::new(0.0, 0.0, -300.0));
        let plain = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -400.0));
        let before = health(&app, dancer);
        request(&mut app, caster, 1, CastTarget::None);
        run_seconds(&mut app, 1.3);
        assert_eq!(
            health(&app, dancer),
            before - 100.0,
            "dash contact counts a targeting-only untargetable unit"
        );
        assert_eq!(health(&app, plain), 900.0, "and a plain unit as before");
    }

    #[test]
    fn pounce_leash_still_grabs_a_shadow_dancing_enemy_at_the_landing_point() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let dancer = dancing_dire_slark(&mut app, Vec3::new(0.0, 0.0, -650.0));
        request(&mut app, caster, 1, CastTarget::None);
        run_seconds(&mut app, 1.3);
        assert!(
            status_of(&app, dancer, "leash").is_some(),
            "the landing area reaches the dancer"
        );
    }

    #[test]
    fn dark_pact_pulses_still_drain_a_shadow_dancing_enemy() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let dancer = dancing_dire_slark(&mut app, Vec3::new(200.0, 0.0, 0.0));
        let before = health(&app, dancer);
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 3.0);
        assert!(
            health(&app, dancer) < before,
            "areas still hit a targeting-only untargetable unit"
        );
    }

    #[test]
    fn dark_pact_dispel_leaves_shadow_dance_and_agility_buffs() {
        let mut app = slark_app();
        let caster = spawn_slark(&mut app);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::None);
        run_seconds(&mut app, 0.2);
        write_attack_hit(&mut app, caster, enemy);
        run_seconds(&mut app, 0.1);
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 2.0);
        assert!(status_of(&app, caster, "untargetable").is_some());
        assert!(status_of(&app, caster, "shadow_dance_move_speed_bonus").is_some());
        assert_eq!(instances(&app, caster, "essence_shift_agility"), 1);
    }

    #[test]
    fn dark_pact_never_damages_slark() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(100.0, 0.0, 0.0));
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        let own = damage_to(&mut app, caster, 3.0);
        assert!(own.is_empty(), "no self-damage: {own:?}");
        assert_eq!(health(&app, caster), 1000.0);
        assert_eq!(health(&app, ally), 1000.0, "allies are not hit");
        assert!(health(&app, enemy) < 1000.0);
    }

    #[test]
    fn dark_pact_stops_when_slark_dies() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 1.8);
        let hurt = health(&app, enemy);
        assert!(hurt < 1000.0 && hurt > 1000.0 - 75.0, "mid-window: {hurt}");
        app.world_mut().get_mut::<Health>(caster).unwrap().current = 0.0;
        run_seconds(&mut app, 2.0);
        assert_eq!(health(&app, enemy), hurt, "no pulse after he died");
        assert_eq!(zone_count(&mut app), 0);
    }

    #[test]
    fn pounce_leaps_forward_damages_passed_enemies_and_leashes_those_at_landing() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        // He faces -Z and lands at z = -700.
        let passed = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -300.0));
        let at_landing = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -650.0));
        let beside = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, -300.0));
        let behind = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 200.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(0.0, 0.0, -400.0));
        request(&mut app, caster, 1, CastTarget::None);
        run_seconds(&mut app, 1.3);
        let landed = position(&app, caster);
        assert!(
            landed.distance(Vec3::new(0.0, 0.0, -700.0)) < 1.0,
            "leapt 700 forward: {landed:?}"
        );
        assert_eq!(health(&app, passed), 900.0, "damaged once on the way");
        assert!(
            status_of(&app, passed, "leash").is_none(),
            "passed early: damaged but not leashed"
        );
        assert_eq!(health(&app, at_landing), 900.0, "damaged and leashed");
        let leash = status_of(&app, at_landing, "leash").expect("leashed");
        assert!(leash.remaining > 1.0 && leash.remaining <= 2.5);
        for untouched in [beside, behind, ally] {
            assert_eq!(health(&app, untouched), 1000.0);
            assert!(status_of(&app, untouched, "leash").is_none());
        }
        assert_eq!(health(&app, caster), 1000.0);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 925.0);
    }

    #[test]
    fn pounce_leash_holds_target_within_radius_of_landing_point() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, -650.0));
        request(&mut app, caster, 1, CastTarget::None);
        run_seconds(&mut app, 1.3);
        let statuses = app.world().get::<StatusEffects>(target).unwrap();
        let leash = statuses.leashes().next().expect("leashed");
        assert_eq!(leash.radius, 325.0);
        match leash.anchor {
            crate::status::LeashAnchor::Point(p) => {
                assert!(
                    p.distance(Vec2::new(0.0, -700.0)) < 1.0,
                    "landing point {p}"
                );
            }
            other => panic!("anchored at the landing point, not {other:?}"),
        }
        // Pushed far out, it is clamped back to the circle around the landing point.
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(1500.0, 0.0, -700.0);
        step(&mut app);
        let p = position(&app, target);
        let from_landing = Vec2::new(p.x, p.z).distance(Vec2::new(0.0, -700.0));
        assert!(
            (from_landing - 325.0).abs() < 1.0,
            "held at the edge: {from_landing}"
        );
        // Once the leash ends it is free again.
        run_seconds(&mut app, 3.0);
        assert!(status_of(&app, target, "leash").is_none());
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(1500.0, 0.0, -700.0);
        step(&mut app);
        assert_eq!(position(&app, target).x, 1500.0);
    }

    #[test]
    fn essence_shift_stacks_expire_independently() {
        let mut app = slark_app();
        let caster = spawn_slark(&mut app);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<CombatStats>(enemy)
            .unwrap()
            .attack_damage = 50.0;
        let base_agility = app
            .world()
            .get::<crate::components::HeroAttributes>(caster)
            .unwrap()
            .agility;
        let agility = |app: &App| {
            app.world()
                .get::<crate::components::HeroAttributes>(caster)
                .unwrap()
                .agility
        };
        let damage = |app: &App| app.world().get::<CombatStats>(enemy).unwrap().attack_damage;

        write_attack_hit(&mut app, caster, enemy);
        run_seconds(&mut app, 5.0);
        write_attack_hit(&mut app, caster, enemy);
        run_seconds(&mut app, 0.2);
        assert_eq!(instances(&app, enemy, "essence_shift_attack_damage"), 2);
        assert_eq!(instances(&app, caster, "essence_shift_agility"), 2);
        assert_eq!(agility(&app), base_agility + 6.0);
        assert_eq!(damage(&app), 48.0);

        // Rank 1 lasts 30s (10 + 20 per level): the first stack ends at about t = 30; the
        // second keeps going until about t = 35.
        run_seconds(&mut app, 25.1);
        assert_eq!(instances(&app, enemy, "essence_shift_attack_damage"), 1);
        assert_eq!(instances(&app, caster, "essence_shift_agility"), 1);
        assert_eq!(agility(&app), base_agility + 3.0);
        assert_eq!(damage(&app), 49.0);

        run_seconds(&mut app, 5.0);
        assert_eq!(instances(&app, enemy, "essence_shift_attack_damage"), 0);
        assert_eq!(instances(&app, caster, "essence_shift_agility"), 0);
        assert_eq!(agility(&app), base_agility);
        assert_eq!(damage(&app), 50.0);
    }

    #[test]
    fn essence_shift_agility_raises_attack_speed_and_armor() {
        let mut app = slark_app();
        let caster = spawn_slark(&mut app);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let stats = |app: &App| *app.world().get::<CombatStats>(caster).unwrap();
        let before = stats(&app);
        for _ in 0..7 {
            write_attack_hit(&mut app, caster, enemy);
            run_seconds(&mut app, 0.1);
        }
        assert_eq!(instances(&app, caster, "essence_shift_agility"), 7);
        let during = stats(&app);
        assert!(
            (during.armor - before.armor - 7.0 * 3.0 * 0.14).abs() < 1e-3,
            "armor {} -> {}",
            before.armor,
            during.armor
        );
        assert!(
            during.attack_speed > before.attack_speed,
            "attack speed {} -> {}",
            before.attack_speed,
            during.attack_speed
        );
        // Rank 1 stacks last 30s.
        run_seconds(&mut app, 31.0);
        let after = stats(&app);
        assert!((after.armor - before.armor).abs() < 1e-3);
        assert_eq!(instances(&app, caster, "essence_shift_agility"), 0);
    }

    #[test]
    fn essence_shift_does_nothing_on_miss() {
        let mut app = slark_app();
        let caster = spawn_slark(&mut app);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        // A swing that was released but never landed.
        write_attack_release(&mut app, caster, enemy);
        run_seconds(&mut app, 0.5);
        assert_eq!(instances(&app, enemy, "essence_shift_attack_damage"), 0);
        assert_eq!(instances(&app, caster, "essence_shift_agility"), 0);
    }

    #[test]
    fn shadow_dance_makes_slark_untargetable_hasted_and_regenerating() {
        use crate::status::TargetIntent;
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let base_speed = app.world().get::<CombatStats>(caster).unwrap().move_speed;
        request(&mut app, caster, 3, CastTarget::None);
        run_seconds(&mut app, 0.3);
        let statuses = app.world().get::<StatusEffects>(caster).unwrap();
        assert!(statuses.rejects(TargetIntent::Attack));
        assert!(statuses.rejects(TargetIntent::HostileSpell));
        assert!(!statuses.rejects(TargetIntent::Any), "areas still hit");
        assert!(!statuses.is_untargetable(), "not cyclone's full flag");
        assert_eq!(
            statuses.resource_sum(crate::vitals::ResourceStat::HealthRegen),
            70.0
        );
        let hasted = app.world().get::<CombatStats>(caster).unwrap().move_speed;
        assert!(
            (hasted - base_speed * 1.2).abs() < 1e-3,
            "+20% of base: {hasted}"
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 900.0);

        // Back to base once it ends (4 s at rank 1).
        run_seconds(&mut app, 4.0);
        let statuses = app.world().get::<StatusEffects>(caster).unwrap();
        assert!(statuses.effects.is_empty(), "{:?}", statuses.effects);
        assert!(!statuses.rejects(TargetIntent::Attack));
        assert_eq!(
            statuses.resource_sum(crate::vitals::ResourceStat::HealthRegen),
            0.0
        );
        let after = app.world().get::<CombatStats>(caster).unwrap().move_speed;
        assert!((after - base_speed).abs() < 1e-3, "{after}");
    }

    #[test]
    fn shadow_dance_ranks_scale() {
        for (rank, duration, speed, regen, cooldown) in [
            (1, 4.0, 360.0, 70.0, 50.0),
            (2, 4.25, 390.0, 100.0, 40.0),
            (3, 4.5, 420.0, 130.0, 30.0),
            (4, 4.75, 450.0, 160.0, 20.0),
        ] {
            let mut app = slark_app();
            let caster = spawn_caster(&mut app, SLARK);
            set_rank(&mut app, caster, 3, rank);
            request(&mut app, caster, 3, CastTarget::None);
            run_seconds(&mut app, 0.2);
            let dance = status_of(&app, caster, "untargetable").expect("untargetable");
            assert!(
                (duration - dance.remaining).abs() < 0.3,
                "rank {rank}: {}",
                dance.remaining
            );
            let speed_status = status_of(&app, caster, "shadow_dance_move_speed_bonus").unwrap();
            assert!((speed_status.move_speed - (speed - 300.0)).abs() < 1e-3);
            let regen_now = app
                .world()
                .get::<StatusEffects>(caster)
                .unwrap()
                .resource_sum(crate::vitals::ResourceStat::HealthRegen);
            assert_eq!(regen_now, regen, "rank {rank}");
            let remaining = app.world().get::<AbilityLoadout>(caster).unwrap().slots[3]
                .cooldown
                .remaining;
            assert!(
                remaining > cooldown - 1.0 && remaining <= cooldown,
                "rank {rank} cooldown {remaining}"
            );
        }
    }

    #[test]
    fn shadow_dance_with_dark_pact_still_pulses() {
        let mut app = slark_app();
        let caster = spawn_caster(&mut app, SLARK);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 0.1);
        request(&mut app, caster, 3, CastTarget::None);
        run_seconds(&mut app, 0.5);
        assert!(status_of(&app, caster, "untargetable").is_some());
        run_seconds(&mut app, 2.5);
        assert_eq!(
            health(&app, enemy),
            1000.0 - 75.0,
            "all ten pulses landed while untargetable"
        );
    }

    // ---- Winter Wyvern ----

    const WYVERN: [AbilityId; 4] = [
        AbilityId::ArcticBurn,
        AbilityId::SplinterBlast,
        AbilityId::ColdEmbrace,
        AbilityId::WintersCurse,
    ];

    /// The status-ticking test app plus the compulsion enforcement (taunts end with their target).
    fn wyvern_app() -> App {
        let mut app = qop_app();
        app.add_systems(Update, crate::ai::enforce_compulsions);
        app
    }

    fn stats_of(app: &App, unit: Entity) -> CombatStats {
        *app.world().get::<CombatStats>(unit).unwrap()
    }

    #[test]
    fn arctic_burn_grants_range_projectile_speed_attack_point_and_phase_then_reverts() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let before = stats_of(&app, caster);
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 0.3);

        let during = stats_of(&app, caster);
        assert_eq!(during.attack_range, before.attack_range + 250.0);
        assert_eq!(during.attack_range_bonus, before.attack_range_bonus + 250.0);
        assert_eq!(
            during.attack_projectile_speed_bonus,
            before.attack_projectile_speed_bonus + 500.0
        );
        let statuses = app.world().get::<StatusEffects>(caster).unwrap();
        assert_eq!(statuses.attack_point_override(), Some(0.1));
        assert!(statuses.is_phased());
        assert!(has_status(&app, caster, "arctic_burn_attack_range"));
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 900.0);

        // Rank 1 lasts 7 s; every part ends with it and the stats return exactly.
        run_seconds(&mut app, 7.2);
        let after = stats_of(&app, caster);
        assert_eq!(after.attack_range, before.attack_range);
        assert_eq!(after.attack_range_bonus, before.attack_range_bonus);
        assert_eq!(
            after.attack_projectile_speed_bonus,
            before.attack_projectile_speed_bonus
        );
        let statuses = app.world().get::<StatusEffects>(caster).unwrap();
        assert_eq!(statuses.attack_point_override(), None);
        assert!(!statuses.is_phased());
        assert!(statuses.effects.is_empty());
    }

    #[test]
    fn arctic_burn_attacks_burn_and_slow_only_while_active() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));

        // Not active: a hit does nothing extra (the passive-style rows are gated).
        write_attack_hit(&mut app, caster, target);
        run_seconds(&mut app, 0.2);
        assert!(!has_status(&app, target, "arctic_burn_burn"));
        assert!(!has_status(&app, target, "arctic_burn_slow"));

        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 0.3);
        write_attack_hit(&mut app, caster, target);
        run_seconds(&mut app, 0.2);
        let burn = status_of(&app, target, "arctic_burn_burn").expect("burning");
        assert_eq!(burn.damage_per_sec, 15.0);
        assert_eq!(burn.dot_source, Some(caster));
        let slow = status_of(&app, target, "arctic_burn_slow").expect("slowed");
        assert!((slow.move_slow_fraction - 0.15).abs() < 1e-6);
        let hp = health(&app, target);
        assert!(hp < 1000.0, "the burn ticks");
        // The burn and slow outlast nothing: 5 s later they are gone.
        run_seconds(&mut app, 5.5);
        assert!(!has_status(&app, target, "arctic_burn_burn"));
        assert!(!has_status(&app, target, "arctic_burn_slow"));

        // After the ability itself ran out (7 s) a hit again applies nothing.
        run_seconds(&mut app, 2.0);
        assert!(!has_status(&app, caster, "arctic_burn_attack_range"));
        write_attack_hit(&mut app, caster, target);
        run_seconds(&mut app, 0.2);
        assert!(!has_status(&app, target, "arctic_burn_burn"));
        assert!(!has_status(&app, target, "arctic_burn_slow"));
    }

    #[test]
    fn arctic_burn_burn_does_not_stack_on_repeat_hits() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 0.3);
        write_attack_hit(&mut app, caster, target);
        run_seconds(&mut app, 2.0);
        let aged = status_of(&app, target, "arctic_burn_burn")
            .unwrap()
            .remaining;
        for _ in 0..3 {
            write_attack_hit(&mut app, caster, target);
            run_seconds(&mut app, 0.1);
        }
        assert_eq!(instances(&app, target, "arctic_burn_burn"), 1);
        assert_eq!(instances(&app, target, "arctic_burn_slow"), 1);
        let refreshed = status_of(&app, target, "arctic_burn_burn")
            .unwrap()
            .remaining;
        assert!(refreshed > aged, "refreshed {refreshed} vs aged {aged}");
        assert_eq!(
            status_of(&app, target, "arctic_burn_burn")
                .unwrap()
                .damage_per_sec,
            15.0,
            "flat dps, not summed"
        );
    }

    /// Casts Splinter Blast at `target` and lands its projectile through the real combat
    /// splash rules: the primary is struck, the rest of the radius is `splashed`.
    fn land_splinter_blast(app: &mut App, caster: Entity, target: Entity) {
        use bevy::ecs::system::RunSystemOnce;
        let spawned = cast_for_projectiles(app, caster, 1, CastTarget::Unit(target));
        assert_eq!(spawned.len(), 1);
        let shot = spawned[0];
        assert_eq!(shot.splash_radius, 500.0);
        assert_eq!(shot.speed, Some(650.0));
        let at = app.world().get::<Transform>(target).unwrap().translation;
        app.world_mut().spawn((
            Transform::from_xyz(at.x, crate::scale::body(1.0), at.z),
            crate::components::Projectile {
                speed: 1.0,
                team: Team::Radiant,
                radius: crate::scale::body(0.85),
                lifetime: 1.0,
                splash_radius: shot.splash_radius,
            },
            shot.payload,
            crate::components::ProjectileHome {
                target,
                last_pos: at,
            },
        ));
        app.world_mut()
            .run_system_once(crate::combat::apply_projectile_hits)
            .unwrap();
        for _ in 0..6 {
            step(app);
        }
    }

    #[test]
    fn splinter_blast_hits_enemies_around_target_but_not_target() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(800.0, 0.0, 0.0));
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(800.0, 0.0, 300.0));
        let edge = spawn_unit(&mut app, Team::Dire, Vec3::new(1250.0, 0.0, 0.0));
        land_splinter_blast(&mut app, caster, target);
        // Rank 1: 80 magical damage (no resistance) and a 27% slow on every other enemy.
        for splashed in [near, edge] {
            assert_eq!(health(&app, splashed), 920.0);
            let slow = status_of(&app, splashed, "splinter_blast_slow").expect("slowed");
            assert!((slow.move_slow_fraction - 0.27).abs() < 1e-6);
        }
        assert_eq!(health(&app, target), 1000.0, "the primary is not splashed");
        assert!(!has_status(&app, target, "splinter_blast_slow"));
    }

    #[test]
    fn splinter_blast_ignores_allies_and_far_enemies() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(800.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(800.0, 0.0, 100.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(800.0, 0.0, 700.0));
        land_splinter_blast(&mut app, caster, target);
        for untouched in [target, ally, far] {
            assert_eq!(health(&app, untouched), 1000.0);
            assert!(!has_status(&app, untouched, "splinter_blast_slow"));
        }
    }

    #[test]
    fn cold_embrace_freezes_heals_flat_plus_max_hp_and_blocks_physical() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(300.0, 0.0, 0.0));
        set_health(&mut app, ally, 200.0);
        request(&mut app, caster, 2, CastTarget::Unit(ally));
        run_seconds(&mut app, 0.5);
        assert!(has_status(&app, ally, "frozen"));
        assert!(has_status(&app, ally, "heal_over_time"));
        let statuses = app.world().get::<StatusEffects>(ally).unwrap();
        assert!(statuses.is_stunned());
        assert!(
            statuses
                .blocks()
                .contains(crate::status::ActionBlocks::CAST)
        );

        // Rank 1: 40 + 1.5% of 1000 max health = 55 per second.
        let start = health(&app, ally);
        run_seconds(&mut app, 2.0);
        let healed = health(&app, ally) - start;
        assert!((healed - 110.0).abs() < 4.0, "healed {healed} in 2 s");

        // Physical damage is blocked (the heal keeps ticking), magical damage is not.
        let hp = health(&app, ally);
        app.world_mut()
            .write_message(DamageEvent::new(ally, 100.0, DamageType::Physical));
        step(&mut app);
        assert!(health(&app, ally) >= hp, "no physical damage taken");
        let hp = health(&app, ally);
        app.world_mut()
            .write_message(DamageEvent::new(ally, 100.0, DamageType::Magical));
        step(&mut app);
        assert!(health(&app, ally) < hp - 90.0, "magical damage lands");

        // It ends after 4 s and the status leaves no trace.
        run_seconds(&mut app, 2.5);
        assert!(!has_status(&app, ally, "frozen"));
        assert!(!has_status(&app, ally, "heal_over_time"));
    }

    #[test]
    fn cold_embrace_self_cast_allowed() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        request(&mut app, caster, 2, CastTarget::Unit(caster));
        run_seconds(&mut app, 0.5);
        assert!(has_status(&app, caster, "frozen"), "self cast lands");
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 950.0);
    }

    #[test]
    fn winters_curse_dispels_and_stuns_target() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let target = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        hold_status(&mut app, target, "some_buff", 30.0);
        request(&mut app, caster, 3, CastTarget::Unit(target));
        run_seconds(&mut app, 0.4);
        assert!(!has_status(&app, target, "some_buff"), "buffs dispelled");
        let stun = status_of(&app, target, "winters_curse_stun").expect("stunned");
        assert!(stun.stunned);
        assert!(
            (stun.remaining - 4.0).abs() <= 0.5,
            "4 s stun: {}",
            stun.remaining
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 850.0);
    }

    #[test]
    fn winters_curse_taunts_nearby_enemies_to_attack_target_with_bonus_attack_speed() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let cursed = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(700.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(1200.0, 0.0, 0.0));
        let speed = |app: &App, unit| stats_of(app, unit).attack_speed_flat;
        let before = speed(&app, near);
        request(&mut app, caster, 3, CastTarget::Unit(cursed));
        run_seconds(&mut app, 0.4);
        let taunt = taunt_of(&app, near).expect("nearby enemy is taunted");
        assert_eq!(
            taunt.compulsion,
            Some(crate::status::Compulsion::Attack { target: cursed })
        );
        assert_eq!(
            speed(&app, near),
            before + 50.0,
            "rank 1 bonus attack speed"
        );
        assert!(taunt_of(&app, far).is_none(), "outside the 525 radius");
        assert!(taunt_of(&app, caster).is_none());
    }

    #[test]
    fn winters_curse_cursed_target_not_taunted() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let cursed = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        let _near = spawn_unit(&mut app, Team::Dire, Vec3::new(500.0, 0.0, 0.0));
        let before = stats_of(&app, cursed).attack_speed_flat;
        request(&mut app, caster, 3, CastTarget::Unit(cursed));
        run_seconds(&mut app, 0.4);
        assert!(taunt_of(&app, cursed).is_none());
        assert!(has_status(&app, cursed, "winters_curse_stun"));
        assert_eq!(stats_of(&app, cursed).attack_speed_flat, before);
    }

    #[test]
    fn winters_curse_taunt_ends_when_target_dies() {
        let mut app = wyvern_app();
        let caster = spawn_caster(&mut app, WYVERN);
        let cursed = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(500.0, 0.0, 0.0));
        let before = stats_of(&app, near).attack_speed_flat;
        request(&mut app, caster, 3, CastTarget::Unit(cursed));
        run_seconds(&mut app, 0.4);
        assert!(taunt_of(&app, near).is_some());
        set_health(&mut app, cursed, 0.0);
        run_seconds(&mut app, 0.3);
        assert!(
            taunt_of(&app, near).is_none(),
            "the taunt ended with its target"
        );
        assert_eq!(
            stats_of(&app, near).attack_speed_flat,
            before,
            "and its attack speed with it"
        );
    }

    // ---- Shadowheart ----

    const SHADOWHEART: [AbilityId; 4] = [
        AbilityId::Purification,
        AbilityId::WardingBond,
        AbilityId::SpiritGuardians,
        AbilityId::SharranConversion,
    ];

    const HERO_MAGIC_RESIST: f32 = 6.321839; // 27.5%

    fn damaged_health(app: &App, unit: Entity) -> f32 {
        1000.0 - health(app, unit)
    }

    #[test]
    fn purification_heals_ally_and_pure_damages_enemies_around_it() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, SHADOWHEART);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(500.0, 0.0, 0.0));
        let bystander = spawn_unit(&mut app, Team::Radiant, Vec3::new(500.0, 0.0, 200.0));
        let near_ally = spawn_unit(&mut app, Team::Dire, Vec3::new(750.0, 0.0, 0.0));
        let near_caster = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 150.0));
        // Armor and magic resistance must not matter: the damage is pure.
        {
            let mut stats = app.world_mut().get_mut::<CombatStats>(near_ally).unwrap();
            stats.armor = 40.0;
            stats.magic_resist = 40.0;
        }
        set_health(&mut app, ally, 500.0);
        request(&mut app, caster, 0, CastTarget::Unit(ally));
        run_seconds(&mut app, 0.6);

        assert_eq!(health(&app, ally), 590.0, "healed 90, never damaged");
        assert_eq!(damaged_health(&app, near_ally), 90.0, "250 from the ally");
        assert_eq!(health(&app, near_caster), 1000.0, "400 from the ally");
        assert_eq!(health(&app, bystander), 1000.0, "allies are not damaged");
        assert_eq!(health(&app, caster), 1000.0);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 910.0);
    }

    #[test]
    fn purification_self_cast_heals_and_damages_around_caster() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, SHADOWHEART);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(400.0, 0.0, 0.0));
        set_health(&mut app, caster, 500.0);
        request(&mut app, caster, 0, CastTarget::Unit(caster));
        run_seconds(&mut app, 0.6);
        assert_eq!(health(&app, caster), 590.0);
        assert_eq!(damaged_health(&app, enemy), 90.0);
        assert_eq!(health(&app, far), 1000.0);
    }

    #[test]
    fn warding_bond_buffs_both_and_reverts_after_10s() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, SHADOWHEART);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(300.0, 0.0, 0.0));
        let armor_before = (stats_of(&app, caster).armor, stats_of(&app, ally).armor);
        request(&mut app, caster, 1, CastTarget::Unit(ally));
        run_seconds(&mut app, 0.6);

        for unit in [caster, ally] {
            assert_eq!(stats_of(&app, unit).armor, 1.0, "rank 1: +1 armor");
            assert!(has_status(&app, unit, "warding_bond_armor"));
            let statuses = app.world().get::<StatusEffects>(unit).unwrap();
            assert!(
                (statuses.magic_damage_factor() - 0.95).abs() < 1e-6,
                "+5% MR"
            );
        }
        assert!(has_status(&app, ally, "damage_share"));
        assert!(
            !has_status(&app, caster, "damage_share"),
            "only the ally is bonded"
        );
        let share = status_of(&app, ally, "damage_share").unwrap();
        assert!((share.remaining - 10.0).abs() <= 0.7, "{}", share.remaining);
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 910.0);

        run_seconds(&mut app, 10.0);
        for unit in [caster, ally] {
            assert!(
                app.world()
                    .get::<StatusEffects>(unit)
                    .unwrap()
                    .effects
                    .is_empty()
            );
        }
        assert_eq!(stats_of(&app, caster).armor, armor_before.0);
        assert_eq!(stats_of(&app, ally).armor, armor_before.1);
    }

    #[test]
    fn warding_bond_duplicates_target_damage_to_caster() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, SHADOWHEART);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(300.0, 0.0, 0.0));
        app.world_mut()
            .get_mut::<CombatStats>(caster)
            .unwrap()
            .magic_resist = HERO_MAGIC_RESIST;
        request(&mut app, caster, 1, CastTarget::Unit(ally));
        run_seconds(&mut app, 0.6);

        app.world_mut()
            .write_message(DamageEvent::new(ally, 100.0, DamageType::Magical));
        step(&mut app);
        // The ally has 0 MR + the bond's 5%: 100 x 0.95. The caster's copy has not landed.
        assert!((damaged_health(&app, ally) - 95.0).abs() < 0.01);
        assert_eq!(health(&app, caster), 1000.0);
        step(&mut app);
        // One frame later the caster took the same 100 through its own 27.5% and 5%.
        assert!((damaged_health(&app, caster) - 100.0 * 0.725 * 0.95).abs() < 0.01);
        assert!(
            (damaged_health(&app, ally) - 95.0).abs() < 0.01,
            "not reduced"
        );
        run_seconds(&mut app, 0.5);
        assert!(
            (damaged_health(&app, caster) - 68.875).abs() < 0.01,
            "only once"
        );
    }

    #[test]
    fn warding_bond_self_cast_buffs_once_without_share() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, SHADOWHEART);
        request(&mut app, caster, 1, CastTarget::Unit(caster));
        run_seconds(&mut app, 0.6);
        let statuses = app.world().get::<StatusEffects>(caster).unwrap();
        assert_eq!(
            statuses
                .effects
                .iter()
                .filter(|e| e.id == "warding_bond_armor")
                .count(),
            1
        );
        assert_eq!(stats_of(&app, caster).armor, 1.0, "applied once, not twice");
        assert!((statuses.magic_damage_factor() - 0.95).abs() < 1e-6);

        app.world_mut()
            .write_message(DamageEvent::new(caster, 100.0, DamageType::Magical));
        run_seconds(&mut app, 0.5);
        assert!(
            (damaged_health(&app, caster) - 95.0).abs() < 0.01,
            "no copy of its own damage"
        );
    }

    /// Spirit Guardians with a stationary enemy on the orbit path (180 east of the caster,
    /// which faces -Z); the enemy's armor and magic resistance are high to show the damage
    /// is pure.
    fn guardians_app() -> (App, Entity, Entity) {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, SHADOWHEART);
        let enemy = spawn_unit(&mut app, Team::Dire, Vec3::new(180.0, 0.0, 0.0));
        {
            let mut stats = app.world_mut().get_mut::<CombatStats>(enemy).unwrap();
            stats.armor = 50.0;
            stats.magic_resist = 50.0;
        }
        (app, caster, enemy)
    }

    fn live_orbits(app: &mut App) -> Vec<(f32, Vec3)> {
        app.world_mut()
            .query::<(&orbits::AbilityOrbit, &Transform)>()
            .iter(app.world())
            .map(|(orbit, tf)| (orbit.elapsed, tf.translation))
            .collect()
    }

    #[test]
    fn spirit_guardians_three_spirits_hit_enemy_on_orbit_path() {
        let (mut app, caster, enemy) = guardians_app();
        request(&mut app, caster, 2, CastTarget::None);
        // Until the orbit exists and has turned for (just under) one period.
        for _ in 0..200 {
            step(&mut app);
            if live_orbits(&mut app)
                .first()
                .is_some_and(|(elapsed, _)| *elapsed >= 1.4)
            {
                break;
            }
        }
        assert_eq!(live_orbits(&mut app).len(), 1);
        assert_eq!(
            damaged_health(&app, enemy),
            270.0,
            "3 spirits x 90 pure damage over one revolution"
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 910.0);
    }

    #[test]
    fn spirit_guardians_ends_after_duration() {
        let (mut app, caster, enemy) = guardians_app();
        request(&mut app, caster, 2, CastTarget::None);
        run_seconds(&mut app, 0.6);
        assert_eq!(live_orbits(&mut app).len(), 1);
        // Rank 1 lasts 3 s: 2 hits per spirit on this path, then nothing more.
        run_seconds(&mut app, 3.2);
        assert!(live_orbits(&mut app).is_empty());
        let total = damaged_health(&app, enemy);
        assert_eq!(total, 540.0, "3 spirits x 2 passes x 90");
        run_seconds(&mut app, 3.0);
        assert_eq!(damaged_health(&app, enemy), total, "no hits after it ended");
    }

    #[test]
    fn spirit_guardians_follow_shadowheart() {
        let (mut app, caster, old_spot) = guardians_app();
        request(&mut app, caster, 2, CastTarget::None);
        run_seconds(&mut app, 0.6);
        let new_home = Vec3::new(2000.0, 0.0, 0.0);
        move_unit(&mut app, caster, new_home);
        let new_spot = spawn_unit(&mut app, Team::Dire, new_home + Vec3::new(0.0, 0.0, 180.0));
        let before = health(&app, old_spot);
        run_seconds(&mut app, 1.5);
        assert!(
            damaged_health(&app, new_spot) >= 90.0,
            "the spirits went along"
        );
        assert_eq!(
            health(&app, old_spot),
            before,
            "and left the old spot alone"
        );
        let (_, at) = live_orbits(&mut app)[0];
        assert_eq!(at, new_home);
    }

    #[test]
    fn sharran_conversion_status_lasts_duration() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, SHADOWHEART);
        request(&mut app, caster, 3, CastTarget::None);
        run_seconds(&mut app, 0.3);
        let status = status_of(&app, caster, "convert_on_death").expect("aura");
        assert_eq!(status.convert_radius, 300.0);
        assert!(
            (status.remaining - 5.0).abs() <= 0.4,
            "{}",
            status.remaining
        );
        assert_eq!(app.world().get::<Mana>(caster).unwrap().current, 875.0);
        assert_eq!(
            app.world()
                .get::<StatusEffects>(caster)
                .unwrap()
                .convert_radius(),
            300.0
        );
        run_seconds(&mut app, 5.2);
        assert!(!has_status(&app, caster, "convert_on_death"));
        assert_eq!(
            app.world()
                .get::<StatusEffects>(caster)
                .unwrap()
                .convert_radius(),
            0.0
        );
    }

    // ---- Gyrocopter ----

    const GYRO: [AbilityId; 4] = [
        AbilityId::RocketBarrage,
        AbilityId::HomingMissile,
        AbilityId::FlakCannon,
        AbilityId::CallDown,
    ];

    /// The Ursa test app plus the real projectile pipeline, which Homing Missile needs.
    fn gyro_app() -> App {
        let mut app = qop_app();
        app.add_systems(
            Update,
            (
                spawn_requested_projectiles,
                crate::combat::fly_projectiles,
                crate::combat::apply_projectile_hits,
                crate::combat::tick_lifetimes,
            )
                .chain()
                .after(execute_ability_casts)
                .before(projectile_hit_triggers),
        );
        app
    }

    /// A unit the projectile pipeline can see (it reads global transforms).
    fn gyro_unit(app: &mut App, team: Team, pos: Vec3) -> Entity {
        let unit = spawn_unit(app, team, pos);
        with_world_transform(app, unit);
        unit
    }

    fn hits_on(app: &App, unit: Entity) -> Vec<DamageEvent> {
        current_messages::<DamageEvent>(app)
            .into_iter()
            .filter(|d| d.target == unit)
            .collect()
    }

    #[test]
    fn rocket_barrage_fires_thirty_rockets_over_three_seconds_on_random_enemies() {
        let mut app = qop_app();
        app.world_mut().resource_mut::<CombatRng>().seed(7);
        let caster = spawn_caster(&mut app, GYRO);
        let a = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let b = spawn_unit(&mut app, Team::Dire, Vec3::new(-250.0, 0.0, 100.0));
        let outside = spawn_unit(&mut app, Team::Dire, Vec3::new(500.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(100.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        let (mut on_a, mut on_b) = (0, 0);
        for _ in 0..(4.0_f32 / 0.05) as usize {
            step(&mut app);
            for hit in current_messages::<DamageEvent>(&app) {
                assert_eq!(hit.amount, 8.0, "rank 1 rocket");
                assert_eq!(hit.damage_type, DamageType::Magical);
                assert!(hit.is_spell);
                assert_eq!(hit.source, Some(caster));
                if hit.target == a {
                    on_a += 1;
                } else if hit.target == b {
                    on_b += 1;
                } else {
                    panic!("a rocket struck {:?}", hit.target);
                }
            }
        }
        assert_eq!(on_a + on_b, 30, "ten rockets a second for three seconds");
        assert!(on_a > 0 && on_b > 0, "both were hit: {on_a} / {on_b}");
        assert_eq!(health(&app, outside), 1000.0, "500 away is outside the 400");
        assert_eq!(health(&app, ally), 1000.0);
        assert_eq!(health(&app, a), 1000.0 - 8.0 * on_a as f32);
    }

    #[test]
    fn rocket_barrage_follows_gyrocopter() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, GYRO);
        let left_behind = spawn_unit(&mut app, Team::Dire, Vec3::new(100.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(3100.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        run_seconds(&mut app, 0.5);
        assert!(health(&app, left_behind) < 1000.0, "struck while near");
        assert_eq!(health(&app, far), 1000.0);

        app.world_mut()
            .get_mut::<Transform>(caster)
            .unwrap()
            .translation = Vec3::new(3000.0, 0.0, 0.0);
        let before = health(&app, left_behind);
        let mut on_far = 0;
        for _ in 0..(2.4_f32 / 0.05) as usize {
            step(&mut app);
            on_far += hits_on(&app, far).len();
        }
        assert!(on_far > 0, "the barrage moved with the caster");
        assert_eq!(health(&app, left_behind), before, "and left the old spot");
    }

    #[test]
    fn rocket_barrage_with_no_enemies_does_nothing() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, GYRO);
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(100.0, 0.0, 0.0));
        let far = spawn_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0));
        request(&mut app, caster, 0, CastTarget::None);
        let mut damage = 0;
        for _ in 0..(4.0_f32 / 0.05) as usize {
            step(&mut app);
            damage += current_messages::<DamageEvent>(&app).len();
        }
        assert_eq!(damage, 0);
        assert_eq!(health(&app, ally), 1000.0);
        assert_eq!(health(&app, far), 1000.0);
        assert_eq!(zone_count(&mut app), 0, "the zone ran out");
    }

    /// Frame index (0.05 s each) at which `unit` has taken `expected` damage events in a row.
    fn missile_position(app: &mut App) -> Option<Vec3> {
        app.world_mut()
            .query::<(&Transform, &crate::components::Projectile)>()
            .iter(app.world())
            .next()
            .map(|(tf, _)| tf.translation)
    }

    #[test]
    fn homing_missile_waits_then_hits_damages_and_stuns() {
        let mut app = gyro_app();
        let caster = spawn_caster(&mut app, GYRO);
        let target = gyro_unit(&mut app, Team::Dire, Vec3::new(600.0, 0.0, 0.0));
        request(&mut app, caster, 1, CastTarget::Unit(target));

        let mut first_seen = None;
        let mut launch = Vec3::ZERO;
        let mut moved_at = None;
        let mut damage = Vec::new();
        let mut stunned = false;
        for frame in 0..80 {
            step(&mut app);
            damage.extend(hits_on(&app, target));
            stunned |= has_status(&app, target, "homing_missile_stun");
            if let Some(position) = missile_position(&mut app) {
                if first_seen.is_none() {
                    first_seen = Some(frame);
                    launch = position;
                } else if moved_at.is_none() && position != launch {
                    moved_at = Some(frame);
                }
            }
        }
        let (seen, moved) = (first_seen.expect("launched"), moved_at.expect("flew"));
        let waited = (moved - seen) as f32 * 0.05;
        assert!(
            (0.45..=0.65).contains(&waited),
            "no movement before 0.5 s, waited {waited}"
        );
        assert_eq!(damage.len(), 1, "one strike");
        assert_eq!(damage[0].amount, 90.0, "rank 1");
        assert_eq!(damage[0].damage_type, DamageType::Magical);
        assert!(stunned, "stunned by homing_missile_stun");
        assert_eq!(missile_position(&mut app), None, "the missile is spent");
    }

    #[test]
    fn homing_missile_retargets_when_target_dies() {
        let mut app = gyro_app();
        let caster = spawn_caster(&mut app, GYRO);
        let target = gyro_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0));
        let heir = gyro_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 300.0));
        let too_far = gyro_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 1500.0));
        request(&mut app, caster, 1, CastTarget::Unit(target));
        run_seconds(&mut app, 0.2);
        set_health(&mut app, target, 0.0);
        let mut stunned = false;
        for _ in 0..80 {
            step(&mut app);
            stunned |= has_status(&app, heir, "homing_missile_stun");
        }
        assert_eq!(health(&app, heir), 910.0, "the new target takes the damage");
        assert!(stunned, "and the stun");
        assert!(!has_status(&app, target, "homing_missile_stun"));
        assert_eq!(health(&app, too_far), 1000.0);
        assert_eq!(missile_position(&mut app), None);
    }

    #[test]
    fn homing_missile_retarget_obeys_spell_landing_rules() {
        let mut app = gyro_app();
        let caster = spawn_caster(&mut app, GYRO);
        let target = gyro_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0));
        let heir = gyro_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 300.0));
        give_status(
            &mut app,
            heir,
            crate::status::StatusEffect::spell_block(30.0),
        );
        request(&mut app, caster, 1, CastTarget::Unit(target));
        run_seconds(&mut app, 0.2);
        set_health(&mut app, target, 0.0);
        for _ in 0..80 {
            step(&mut app);
        }
        assert_eq!(health(&app, heir), 1000.0, "the new target blocked it");
        assert!(status_of(&app, heir, "spell_block").is_none(), "spent");
        assert!(!has_status(&app, heir, "homing_missile_stun"));
    }

    /// Casts Flak Cannon (the rank given) and lets the status land.
    fn cast_flak(app: &mut App, caster: Entity, rank: u32) {
        set_rank(app, caster, 2, rank);
        request(app, caster, 2, CastTarget::None);
        run_seconds(app, 0.3);
    }

    fn flak_charges(app: &App, caster: Entity) -> Option<u32> {
        status_of(app, caster, "flak_cannon_attack_charges").map(|s| s.attack_charges)
    }

    fn release_attack(app: &mut App, attacker: Entity, target: Entity, extra: bool) {
        app.world_mut()
            .write_message(crate::basic_attack::BasicAttackReleaseEvent {
                attacker,
                target,
                damage: 10.0,
                extra,
            });
    }

    fn extra_targets(app: &App) -> Vec<Entity> {
        extra_attack_events(app)
            .iter()
            .map(|event| event.target)
            .collect()
    }

    #[test]
    fn flak_cannon_splits_each_attack_to_n_nearest_enemies_around_gyro() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, GYRO);
        let primary = spawn_unit(&mut app, Team::Dire, Vec3::new(-300.0, 0.0, 0.0));
        let near = spawn_unit(&mut app, Team::Dire, Vec3::new(200.0, 0.0, 0.0));
        let middle = spawn_unit(&mut app, Team::Dire, Vec3::new(0.0, 0.0, 600.0));
        // 1200 from Gyrocopter but 1500 from the primary: inside the search radius.
        let edge = spawn_unit(&mut app, Team::Dire, Vec3::new(1200.0, 0.0, 0.0));
        let _outside = spawn_unit(&mut app, Team::Dire, Vec3::new(1300.0, 0.0, 0.0));
        let _ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(50.0, 0.0, 0.0));
        cast_flak(&mut app, caster, 1);
        assert_eq!(flak_charges(&app, caster), Some(4));

        release_attack(&mut app, caster, primary, false);
        step(&mut app);
        let events = extra_attack_events(&app);
        assert_eq!(
            events.iter().map(|e| e.target).collect::<Vec<_>>(),
            vec![near, middle, edge],
            "nearest first, never the primary"
        );
        assert!(
            events
                .iter()
                .all(|e| e.attacker == caster && e.damage_ratio == 1.0)
        );
        assert_eq!(flak_charges(&app, caster), Some(3), "one charge spent");
    }

    #[test]
    fn flak_cannon_caps_extra_targets_by_rank() {
        for (rank, cap) in [(1, 4), (5, 8), (7, 9)] {
            let mut app = qop_app();
            let caster = spawn_caster(&mut app, GYRO);
            let primary = spawn_unit(&mut app, Team::Dire, Vec3::new(-100.0, 0.0, 0.0));
            for i in 0..12 {
                spawn_unit(
                    &mut app,
                    Team::Dire,
                    Vec3::new(100.0 + 50.0 * i as f32, 0.0, 0.0),
                );
            }
            cast_flak(&mut app, caster, rank);
            release_attack(&mut app, caster, primary, false);
            step(&mut app);
            assert_eq!(extra_targets(&app).len(), cap, "rank {rank}");
        }
    }

    #[test]
    fn flak_cannon_ends_after_n_attacks() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, GYRO);
        let primary = spawn_unit(&mut app, Team::Dire, Vec3::new(-100.0, 0.0, 0.0));
        let other = spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));
        cast_flak(&mut app, caster, 1);
        for attack in 1..=4 {
            release_attack(&mut app, caster, primary, false);
            step(&mut app);
            assert_eq!(extra_targets(&app), vec![other], "attack {attack} splits");
        }
        assert_eq!(flak_charges(&app, caster), None, "the fourth ended it");
        release_attack(&mut app, caster, primary, false);
        step(&mut app);
        assert!(extra_targets(&app).is_empty(), "the fifth does not split");
    }

    #[test]
    fn flak_cannon_ends_after_duration_with_charges_left() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, GYRO);
        let primary = spawn_unit(&mut app, Team::Dire, Vec3::new(-100.0, 0.0, 0.0));
        spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));
        cast_flak(&mut app, caster, 1);
        release_attack(&mut app, caster, primary, false);
        step(&mut app);
        assert_eq!(flak_charges(&app, caster), Some(3));
        run_seconds(&mut app, 9.0);
        assert_eq!(flak_charges(&app, caster), Some(3), "10 s have not passed");
        run_seconds(&mut app, 1.5);
        assert_eq!(
            flak_charges(&app, caster),
            None,
            "expired with charges left"
        );
        release_attack(&mut app, caster, primary, false);
        step(&mut app);
        assert!(extra_targets(&app).is_empty());
    }

    #[test]
    fn flak_cannon_extra_attacks_do_not_split_again_or_spend_charges() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, GYRO);
        let primary = spawn_unit(&mut app, Team::Dire, Vec3::new(-100.0, 0.0, 0.0));
        spawn_unit(&mut app, Team::Dire, Vec3::new(300.0, 0.0, 0.0));
        cast_flak(&mut app, caster, 1);
        release_attack(&mut app, caster, primary, true);
        step(&mut app);
        assert!(extra_targets(&app).is_empty(), "no chaining");
        assert_eq!(flak_charges(&app, caster), Some(4), "no charge spent");
    }

    #[test]
    fn call_down_hits_and_slows_after_two_seconds_only() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, GYRO);
        let aim = Vec3::new(800.0, 0.0, 0.0);
        let inside = spawn_unit(&mut app, Team::Dire, Vec3::new(1000.0, 0.0, 0.0));
        let outside = spawn_unit(&mut app, Team::Dire, Vec3::new(1500.0, 0.0, 0.0));
        let ally = spawn_unit(&mut app, Team::Radiant, Vec3::new(850.0, 0.0, 0.0));
        request(&mut app, caster, 3, CastTarget::Point(aim));

        let mut struck_at = None;
        let mut hits = 0;
        for frame in 1..=(6.0_f32 / 0.05) as usize {
            step(&mut app);
            let landed = hits_on(&app, inside);
            hits += landed.len();
            if let Some(hit) = landed.first() {
                assert_eq!(hit.amount, 200.0, "rank 1, before mitigation");
                assert_eq!(hit.damage_type, DamageType::Magical);
                struck_at.get_or_insert(frame as f32 * 0.05);
            }
            if struck_at.is_none() {
                assert_eq!(health(&app, inside), 1000.0, "nothing before the impact");
            }
        }
        // 0.3 s cast point, then the 2 s delay.
        let at = struck_at.expect("the impact landed");
        assert!((2.25..=2.5).contains(&at), "impact at {at} s");
        assert_eq!(hits, 1, "a single impact");
        assert_eq!(health(&app, inside), 800.0);
        assert_eq!(health(&app, outside), 1000.0);
        assert_eq!(health(&app, ally), 1000.0);
        let slow = status_of(&app, inside, "call_down_slow").expect("slowed");
        assert!((slow.move_slow_fraction - 0.6).abs() < 1e-6);
        assert!(!has_status(&app, outside, "call_down_slow"));
    }

    #[test]
    fn call_down_slow_lasts_four_seconds() {
        let mut app = qop_app();
        let caster = spawn_caster(&mut app, GYRO);
        let inside = spawn_unit(&mut app, Team::Dire, Vec3::new(900.0, 0.0, 0.0));
        request(
            &mut app,
            caster,
            3,
            CastTarget::Point(Vec3::new(800.0, 0.0, 0.0)),
        );
        run_seconds(&mut app, 2.6);
        assert!(has_status(&app, inside, "call_down_slow"));
        run_seconds(&mut app, 3.0);
        assert!(has_status(&app, inside, "call_down_slow"), "still 4 s long");
        run_seconds(&mut app, 1.0);
        assert!(!has_status(&app, inside, "call_down_slow"), "worn off");
    }
}
