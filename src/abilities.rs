//! Hero abilities: instant vs targeted casting, indicators, and spell VFX.

use bevy::prelude::*;

use crate::combat::{apply_damage, flat_distance, cursor_ground_hit, spawn_spell_bolt};
use crate::components::{
    AbilityCastKind, AbilityCasting, AbilityId, AbilityLoadout, AbilitySlot, AttackMoveOrder,
    AttackSwing, AttackTarget, BoundRadius, CombatStats, DamageType, Ground, Health, Lifetime,
    Mana, MoveTarget, PlayerHero, QueuedAbilityCast, SelectionBox, SpellFx, Team,
};
use crate::items::{
    apply_debuff_immunity, apply_disarm, apply_forceful, apply_phased, apply_root, apply_silence,
    apply_stun,
    apply_status, StatusEffect, StatusEffects,
};
use crate::resources::SharedAssets;
use crate::scale;

pub struct AbilitiesPlugin;

impl Plugin for AbilitiesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AbilityTargeting>()
            .add_systems(
                Update,
                (
                    tick_ability_cooldowns,
                    regen_mana,
                    begin_or_cast_from_hotkeys,
                    update_targeting_indicators,
                    confirm_or_cancel_targeted_cast,
                    resolve_queued_ability_casts,
                    tick_ability_casting,
                    despawn_indicators,
                    animate_spell_fx,
                    apply_pending_damage,
                )
                    .chain()
                    .run_if(crate::net::is_sim_authority),
            );
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
    pub ability: AbilityId,
    pub cast_range: f32,
    /// AoE radius (area) or projectile width (point).
    pub aoe_radius: f32,
    /// When true, confirm requires a creep/hero under the cursor.
    pub unit_only: bool,
    /// When true, show trajectory corridor instead of an AoE disc.
    pub point_target: bool,
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

fn begin_or_cast_from_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mut targeting: ResMut<AbilityTargeting>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    hero: Query<
        (
            Entity,
            &Transform,
            &AbilityLoadout,
            &Mana,
            &StatusEffects,
        ),
        With<PlayerHero>,
    >,
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

        // Switching hotkeys cancels an in-progress targeted cast / queued cast first.
        if targeting.active.is_some() {
            clear_targeting(&mut commands, &mut targeting);
        }
        commands
            .entity(hero_entity)
            .remove::<QueuedAbilityCast>()
            .remove::<AbilityCasting>();

        let Some(slot) = loadout.slots.get(index).cloned() else {
            continue;
        };
        if slot.rank == 0 {
            continue;
        }
        if slot.cooldown_remaining > 0.0 || mana.current < slot.mana_cost {
            continue;
        }

        match slot.cast_kind() {
            AbilityCastKind::Instant => {
                // Mana + cooldown apply when the cast point completes, not at press.
                start_ability_cast(
                    &mut commands,
                    hero_entity,
                    index,
                    &slot,
                    transform.translation,
                    0.0,
                    None,
                );
            }
            AbilityCastKind::Targeted {
                cast_range,
                aoe_radius,
            } => {
                targeting.active = Some(PendingTargetedCast {
                    slot: index,
                    ability: slot.id,
                    cast_range,
                    aoe_radius,
                    unit_only: false,
                    point_target: false,
                });
                spawn_indicators(
                    &mut commands,
                    &assets,
                    transform.translation,
                    cast_range,
                    aoe_radius,
                    false,
                );
            }
            AbilityCastKind::PointTargeted {
                cast_range,
                projectile_width,
            } => {
                targeting.active = Some(PendingTargetedCast {
                    slot: index,
                    ability: slot.id,
                    cast_range,
                    aoe_radius: projectile_width,
                    unit_only: false,
                    point_target: true,
                });
                spawn_indicators(
                    &mut commands,
                    &assets,
                    transform.translation,
                    cast_range,
                    projectile_width,
                    true,
                );
            }
            AbilityCastKind::UnitTargeted { cast_range } => {
                targeting.active = Some(PendingTargetedCast {
                    slot: index,
                    ability: slot.id,
                    cast_range,
                    aoe_radius: 20.0,
                    unit_only: true,
                    point_target: false,
                });
                spawn_indicators(
                    &mut commands,
                    &assets,
                    transform.translation,
                    cast_range,
                    20.0,
                    false,
                );
            }
        }
    }
}

fn start_ability_cast(
    commands: &mut Commands,
    hero: Entity,
    slot: usize,
    ability: &AbilitySlot,
    aim: Vec3,
    aoe_radius: f32,
    unit_target: Option<Entity>,
) {
    commands
        .entity(hero)
        .remove::<MoveTarget>()
        .remove::<AttackTarget>()
        .remove::<AttackMoveOrder>()
        .remove::<AttackSwing>()
        .insert(AbilityCasting {
            slot,
            ability: ability.id,
            aoe_radius,
            aim,
            unit_target,
            point_remaining: ability.cast_point.clamp(0.0, 0.5),
            backswing_remaining: ability.cast_backswing.clamp(0.0, 0.5),
            fired: false,
        });
}

fn cast_instant(
    commands: &mut Commands,
    assets: &SharedAssets,
    transform: &Transform,
    team: Team,
    slot: &crate::components::AbilitySlot,
    enemies: &Query<
        (Entity, &Transform, &Team, &Health, &CombatStats, Option<&BoundRadius>),
        Without<PlayerHero>,
    >,
    stats: &mut CombatStats,
    statuses: &mut StatusEffects,
) -> Vec<(Entity, AbilityId)> {
    let mut cc_hits = Vec::new();
    match slot.id {
        AbilityId::Shockwave | AbilityId::Flurry | AbilityId::FrostNova => {
            let origin = transform.translation;
            let radius = slot.shockwave_radius();
            let damage = slot.shockwave_damage();
            spawn_expanding_ring(
                commands,
                assets,
                origin,
                radius,
                assets.shockwave_mat.clone(),
                0.45,
            );
            for (enemy_entity, enemy_tf, enemy_team, enemy_hp, enemy_stats, bound) in enemies.iter()
            {
                if *enemy_team == team || !enemy_hp.is_alive() {
                    continue;
                }
                let b = bound.map(|r| r.0).unwrap_or(scale::HERO_BOUND);
                if flat_distance(origin, enemy_tf.translation) <= radius + b {
                    let amount = apply_damage(damage, DamageType::Magical, enemy_stats);
                    commands.entity(enemy_entity).insert(PendingDamage { amount });
                    if slot.id == AbilityId::FrostNova || slot.id == AbilityId::Flurry {
                        cc_hits.push((enemy_entity, slot.id));
                    }
                }
            }
            if slot.id == AbilityId::Shockwave {
                apply_forceful(statuses, 2.0);
            }
        }
        AbilityId::Barrier => {
            apply_debuff_immunity(statuses, stats, slot.barrier_duration());
            apply_status(
                statuses,
                stats,
                StatusEffect {
                    armor: slot.barrier_armor(),
                    ..StatusEffect::buff("barrier", slot.barrier_duration())
                },
            );
            spawn_expanding_ring(
                commands,
                assets,
                transform.translation,
                scale::u(2.5),
                assets.nova_mat.clone(),
                0.4,
            );
        }
        _ => {}
    }
    cc_hits
}

fn fire_dash(
    commands: &mut Commands,
    assets: &SharedAssets,
    hero_entity: Entity,
    transform: &Transform,
    stats: &CombatStats,
    statuses: &mut StatusEffects,
    aim: Vec3,
    dash_distance: f32,
) {
    let from = transform.translation;
    let mut dest = Vec3::new(aim.x, from.y, aim.z);
    let flat = Vec3::new(dest.x - from.x, 0.0, dest.z - from.z);
    let dist = flat.length();
    if dist > dash_distance && dist > 1e-4 {
        dest = from + flat.normalize() * dash_distance;
    }
    spawn_dash_ghosts(commands, assets, from, dest);
    let travel = flat_distance(from, dest) / stats.move_speed.max(1.0);
    apply_phased(statuses, travel + 0.15);
    commands
        .entity(hero_entity)
        .insert(MoveTarget {
            position: Vec3::new(dest.x, 0.0, dest.z),
        })
        .remove::<AttackTarget>()
        .remove::<AttackMoveOrder>();
}

fn fire_bolt(
    commands: &mut Commands,
    assets: &SharedAssets,
    hero_entity: Entity,
    transform: &Transform,
    team: Team,
    aim: Vec3,
    aoe: f32,
    bolt_damage: f32,
    unit_target: Option<(Entity, Vec3)>,
) {
    if let Some((enemy, enemy_pos)) = unit_target {
        spawn_spell_bolt(
            commands,
            assets,
            team,
            transform.translation,
            Some(enemy),
            enemy_pos,
            bolt_damage,
            aoe,
        );
        commands.entity(hero_entity).insert(AttackTarget(enemy));
    } else {
        spawn_spell_bolt(
            commands,
            assets,
            team,
            transform.translation,
            None,
            aim,
            bolt_damage,
            aoe,
        );
    }
}

fn confirm_or_cancel_targeted_cast(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut targeting: ResMut<AbilityTargeting>,
    shop_ui: Res<crate::items::ShopUiState>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    mut commands: Commands,
    hero: Query<
        (
            Entity,
            &Transform,
            &Team,
            &AbilityLoadout,
            &Mana,
            &StatusEffects,
        ),
        With<PlayerHero>,
    >,
    enemies: Query<
        (
            Entity,
            &Transform,
            &Team,
            &Health,
            &CombatStats,
            Option<&SelectionBox>,
            &Visibility,
        ),
        Without<PlayerHero>,
    >,
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

    let Ok((hero_entity, transform, team, loadout, mana, statuses)) = hero.single()
    else {
        return;
    };

    if !statuses.can_cast() {
        clear_targeting(&mut commands, &mut targeting);
        return;
    }

    let Some(hit) = cursor_ground_hit(&windows, &camera, &ground) else {
        return;
    };

    let unit_target = enemies
        .iter()
        .filter(|(_, _, enemy_team, hp, _, _, vis)| {
            **enemy_team == team.enemy()
                && hp.is_alive()
                && !matches!(*vis, Visibility::Hidden)
        })
        .filter(|(_, tf, _, _, _, selection, _)| {
            let half = selection
                .map(|s| s.half_extent)
                .unwrap_or(scale::HERO_BOUND);
            (hit.x - tf.translation.x).abs() <= half && (hit.z - tf.translation.z).abs() <= half
        })
        .min_by(|a, b| {
            flat_distance(a.1.translation, hit)
                .partial_cmp(&flat_distance(b.1.translation, hit))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(e, tf, _, _, _, _, _)| (e, tf.translation));

    if pending.unit_only && unit_target.is_none() {
        // Unit-targeted spells require a creep/hero under the cursor.
        return;
    }

    let aim = unit_target.map(|(_, pos)| pos).unwrap_or(hit);
    let dist = flat_distance(transform.translation, aim);

    if dist > pending.cast_range {
        clear_targeting(&mut commands, &mut targeting);
        commands.entity(hero_entity).insert(QueuedAbilityCast {
            slot: pending.slot,
            ability: pending.ability,
            cast_range: pending.cast_range,
            aoe_radius: pending.aoe_radius,
            aim,
            unit_target: unit_target.map(|(e, _)| e),
        });
        commands
            .entity(hero_entity)
            .insert(MoveTarget { position: aim })
            .remove::<AttackTarget>()
            .remove::<AttackMoveOrder>();
        return;
    }

    let Some(slot) = loadout.slots.get(pending.slot).cloned() else {
        clear_targeting(&mut commands, &mut targeting);
        return;
    };
    if slot.rank == 0 || slot.cooldown_remaining > 0.0 || mana.current < slot.mana_cost {
        clear_targeting(&mut commands, &mut targeting);
        return;
    }
    // Mana + cooldown apply when the cast point completes (cancel during cast point is free).
    let aoe = pending.aoe_radius;
    clear_targeting(&mut commands, &mut targeting);

    start_ability_cast(
        &mut commands,
        hero_entity,
        pending.slot,
        &slot,
        aim,
        aoe,
        unit_target.map(|(e, _)| e),
    );
}

fn resolve_queued_ability_casts(
    mut commands: Commands,
    hero: Query<
        (
            Entity,
            &Transform,
            &QueuedAbilityCast,
            &AbilityLoadout,
            &Mana,
            &StatusEffects,
        ),
        With<PlayerHero>,
    >,
    enemies: Query<
        (Entity, &Transform, &Team, &Health, &CombatStats, Option<&BoundRadius>),
        Without<PlayerHero>,
    >,
) {
    let Ok((
        hero_entity,
        transform,
        queued,
        loadout,
        mana,
        statuses,
    )) = hero.single()
    else {
        return;
    };

    let mut aim = queued.aim;
    let mut unit_target = None;
    if let Some(target) = queued.unit_target {
        if let Ok((_, tf, _, hp, _, _)) = enemies.get(target) {
            if hp.is_alive() {
                aim = tf.translation;
                unit_target = Some((target, tf.translation));
            }
        }
    }

    let dist = flat_distance(transform.translation, aim);
    if dist > queued.cast_range {
        // Keep walking toward the aim.
        commands.entity(hero_entity).insert(MoveTarget { position: aim });
        return;
    }

    let queued = *queued;
    if !statuses.can_cast() {
        commands
            .entity(hero_entity)
            .remove::<QueuedAbilityCast>()
            .remove::<MoveTarget>();
        return;
    }
    // Unit-targeted queue requires a living unit target.
    if matches!(
        AbilitySlot::fresh(queued.ability).cast_kind(),
        AbilityCastKind::UnitTargeted { .. }
    ) && unit_target.is_none()
    {
        commands
            .entity(hero_entity)
            .remove::<QueuedAbilityCast>()
            .remove::<MoveTarget>();
        return;
    }
    let Some(slot) = loadout.slots.get(queued.slot).cloned() else {
        commands.entity(hero_entity).remove::<QueuedAbilityCast>();
        return;
    };
    if slot.rank == 0 || slot.cooldown_remaining > 0.0 || mana.current < slot.mana_cost {
        commands
            .entity(hero_entity)
            .remove::<QueuedAbilityCast>()
            .remove::<MoveTarget>();
        return;
    }
    // Mana + cooldown apply when the cast point completes.
    let aoe = queued.aoe_radius;

    commands
        .entity(hero_entity)
        .remove::<QueuedAbilityCast>()
        .remove::<MoveTarget>();

    start_ability_cast(
        &mut commands,
        hero_entity,
        queued.slot,
        &slot,
        aim,
        aoe,
        unit_target.map(|(e, _)| e),
    );
}

fn tick_ability_casting(
    time: Res<Time>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut hero: Query<
        (
            Entity,
            &mut Transform,
            &Team,
            &mut AbilityCasting,
            &mut AbilityLoadout,
            &mut Mana,
            &mut Health,
            &mut StatusEffects,
            &mut CombatStats,
        ),
        With<PlayerHero>,
    >,
    mut enemy_set: ParamSet<(
        Query<
            (Entity, &Transform, &Team, &Health, &CombatStats, Option<&BoundRadius>),
            Without<PlayerHero>,
        >,
        Query<(&mut StatusEffects, &mut CombatStats), Without<PlayerHero>>,
    )>,
) {
    let dt = time.delta_secs();
    let Ok((
        hero_entity,
        mut transform,
        team,
        mut casting,
        mut loadout,
        mut mana,
        mut health,
        mut statuses,
        mut stats,
    )) = hero.single_mut()
    else {
        return;
    };

    if !casting.fired && !statuses.can_cast() {
        commands.entity(hero_entity).remove::<AbilityCasting>();
        return;
    }

    if !casting.fired {
        // Hold cast point until the aim point is inside the facing cone.
        let aim = casting.aim;
        let dir = aim - transform.translation;
        if !crate::facing::turn_toward(&mut transform, dir, stats.turn_rate, dt) {
            return;
        }
        casting.point_remaining = (casting.point_remaining - dt).max(0.0);
        if casting.point_remaining > 0.0 {
            return;
        }

        // Commit mana + cooldown only once the cast point finishes without cancel.
        let Some(slot_mut) = loadout.slot_mut(casting.slot) else {
            commands.entity(hero_entity).remove::<AbilityCasting>();
            return;
        };
        if slot_mut.rank == 0
            || slot_mut.cooldown_remaining > 0.0
            || !mana.try_spend(slot_mut.mana_cost)
        {
            commands.entity(hero_entity).remove::<AbilityCasting>();
            return;
        }
        slot_mut.cooldown_remaining = slot_mut.cooldown;
        let slot = slot_mut.clone();

        casting.fired = true;
        let ability = casting.ability;
        let aoe = casting.aoe_radius;
        let unit_entity = casting.unit_target;

        match ability {
            AbilityId::Dash | AbilityId::Blink => {
                fire_dash(
                    &mut commands,
                    &assets,
                    hero_entity,
                    &transform,
                    &stats,
                    &mut statuses,
                    aim,
                    slot.dash_distance(),
                );
            }
            AbilityId::Bolt | AbilityId::ArcMissile | AbilityId::Execute => {
                let mut damage = slot.bolt_damage();
                let unit_target = unit_entity.and_then(|e| {
                    enemy_set
                        .p0()
                        .get(e)
                        .ok()
                        .map(|(_, tf, _, _, _, _)| (e, tf.translation))
                });
                if ability == AbilityId::Execute {
                    if let Some((enemy, _)) = unit_target {
                        if let Ok((_, _, _, hp, _, _)) = enemy_set.p0().get(enemy) {
                            if hp.current / hp.max.max(1.0) < 0.35 {
                                damage *= 1.55;
                            }
                        }
                    }
                }
                fire_bolt(
                    &mut commands,
                    &assets,
                    hero_entity,
                    &transform,
                    *team,
                    aim,
                    aoe.max(1.2),
                    damage,
                    unit_target,
                );
            }
            AbilityId::Nova | AbilityId::Meteor | AbilityId::Caltrops => {
                let nova_heal = slot.nova_heal();
                if nova_heal > 0.0 {
                    health.current = (health.current + nova_heal).min(health.max);
                }
                let ground_damage = if ability == AbilityId::Caltrops {
                    slot.bolt_damage()
                } else {
                    slot.nova_damage()
                };
                spawn_expanding_ring(
                    &mut commands,
                    &assets,
                    aim,
                    aoe,
                    assets.nova_mat.clone(),
                    0.55,
                );
                let mut meteor_hits = Vec::new();
                for (enemy_entity, enemy_tf, enemy_team, enemy_hp, enemy_stats, bound) in
                    enemy_set.p0().iter()
                {
                    if *enemy_team == *team || !enemy_hp.is_alive() {
                        continue;
                    }
                    let b = bound.map(|r| r.0).unwrap_or(scale::HERO_BOUND);
                    if flat_distance(aim, enemy_tf.translation) <= aoe + b {
                        let amount = apply_damage(ground_damage, DamageType::Magical, enemy_stats);
                        commands.entity(enemy_entity).insert(PendingDamage { amount });
                        if ability == AbilityId::Meteor {
                            meteor_hits.push(enemy_entity);
                        }
                    }
                }
                for enemy in meteor_hits {
                    if let Ok((mut st, mut st_stats)) = enemy_set.p1().get_mut(enemy) {
                        apply_stun(&mut st, &mut st_stats, 1.1);
                    }
                }
            }
            AbilityId::Shockwave
            | AbilityId::Flurry
            | AbilityId::FrostNova
            | AbilityId::Barrier => {
                let hits = cast_instant(
                    &mut commands,
                    &assets,
                    &transform,
                    *team,
                    &slot,
                    &enemy_set.p0(),
                    &mut stats,
                    &mut statuses,
                );
                for (enemy, id) in hits {
                    if let Ok((mut st, mut st_stats)) = enemy_set.p1().get_mut(enemy) {
                        if id == AbilityId::FrostNova {
                            apply_root(&mut st, &mut st_stats, 1.4);
                        }
                        if id == AbilityId::Flurry {
                            apply_silence(&mut st, &mut st_stats, 1.2);
                            apply_disarm(&mut st, &mut st_stats, 1.0);
                        }
                    }
                }
            }
            // Stub abilities from HeroGenerator — no gameplay effect yet.
            _ => {}
        }

        if casting.backswing_remaining <= 0.0 {
            commands.entity(hero_entity).remove::<AbilityCasting>();
        }
        return;
    }

    casting.backswing_remaining = (casting.backswing_remaining - dt).max(0.0);
    if casting.backswing_remaining <= 0.0 {
        commands.entity(hero_entity).remove::<AbilityCasting>();
    }
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

fn spawn_dash_ghosts(
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

fn spawn_expanding_ring(
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

#[derive(Component, Debug, Clone, Copy)]
pub struct PendingDamage {
    pub amount: f32,
}

fn apply_pending_damage(
    mut commands: Commands,
    mut victims: Query<(Entity, &mut Health, &PendingDamage)>,
) {
    for (entity, mut health, damage) in &mut victims {
        health.current -= damage.amount;
        commands.entity(entity).remove::<PendingDamage>();
    }
}
