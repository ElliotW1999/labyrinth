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
pub mod passive;
pub mod tooltip;

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
            .add_message::<HealEvent>()
            .add_message::<ManaRestoreEvent>()
            .add_message::<vitals::ResourceDrainEvent>()
            .add_message::<StatusEffectEvent>()
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
                    (crate::items::resolve_item_uses, execute_item_mechanics).chain(),
                    execute_ability_casts,
                    action_channel_end_triggers,
                    projectile_hit_triggers,
                    attack_triggers::attack_triggers,
                    // Before trigger resolution, so contact / end triggers fire this frame.
                    (tick_forced_movement, settle_grounded_units).chain(),
                    resolve_ability_triggers,
                    spawn_requested_projectiles,
                    (
                        apply_heal_events,
                        apply_mana_restore_events,
                        vitals::apply_resource_drain_events,
                        apply_displacement_events,
                        apply_status_effect_events,
                        crate::vitals::sync_status_resource_caps,
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

fn tick_ability_cooldowns(time: Res<Time>, mut query: Query<&mut AbilityLoadout>) {
    let dt = time.delta_secs();
    for mut loadout in &mut query {
        for slot in &mut loadout.slots {
            slot.cooldown.tick(dt);
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
        AbilityDefinition, AbilityEffect, AbilityEffectEntry, AbilityMechanic, AbilityTrigger,
        AreaRadius, DisplacementKind, EffectScaling, EffectTarget, RankValue, StatusSpec,
        TargetTeam,
    };
    use super::effects::{AbilityEffectAppExt, CustomEffectInput, EffectSource, TriggerContext};
    use super::mechanics::{AbilityAppExt, CastContext};
    use super::*;
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
            .add_message::<HealEvent>()
            .add_message::<ManaRestoreEvent>()
            .add_message::<vitals::ResourceDrainEvent>()
            .add_message::<StatusEffectEvent>()
            .add_message::<DisplacementEvent>()
            .add_message::<TeleportEvent>()
            .add_message::<TeleportCompletedEvent>()
            .add_message::<TeleportRejectedEvent>()
            .init_resource::<NavGrid>()
            .add_message::<DamageEvent>()
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
                    (crate::items::resolve_item_uses, execute_item_mechanics).chain(),
                    execute_ability_casts,
                    action_channel_end_triggers,
                    projectile_hit_triggers,
                    attack_triggers::attack_triggers,
                    (tick_forced_movement, settle_grounded_units).chain(),
                    resolve_ability_triggers,
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
            max_targets,
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
                    max_targets: 0,
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
            .cleave_damage_ratio = 0.5;

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
            dps: RankValue::fixed(12.0),
            damage_type: DamageType::Magical,
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
            a.undispellable = b.undispellable;
            a.dispel_type = b.dispel_type;
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
                AbilityEffect::Teleport,
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
                effect: AbilityEffect::Teleport,
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
}
