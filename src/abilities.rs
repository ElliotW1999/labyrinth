//! Hero abilities: instant vs targeted casting, indicators, and spell VFX.

use bevy::prelude::*;

use crate::combat::{apply_damage, flat_distance, cursor_ground_hit, spawn_spell_bolt};
use crate::components::{
    AbilityCastKind, AbilityId, AbilityLoadout, AttackTarget, CombatStats, DamageType, Ground,
    Health, Lifetime, Mana, MoveTarget, PlayerHero, SpellFx, Team, UnitRadius,
};
use crate::resources::SharedAssets;

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
                    despawn_indicators,
                    animate_spell_fx,
                    apply_pending_damage,
                )
                    .chain(),
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
    pub aoe_radius: f32,
}

#[derive(Component)]
struct RangeIndicator;

#[derive(Component)]
struct AoeIndicator;

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
    mut hero: Query<
        (
            Entity,
            &Transform,
            &Team,
            &mut AbilityLoadout,
            &mut Mana,
        ),
        With<PlayerHero>,
    >,
    enemies: Query<(Entity, &Transform, &Team, &Health, &CombatStats), Without<PlayerHero>>,
) {
    let Ok((hero_entity, transform, team, mut loadout, mut mana)) = hero.single_mut()
    else {
        return;
    };

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

        // Switching hotkeys cancels an in-progress targeted cast first.
        if targeting.active.is_some() {
            clear_targeting(&mut commands, &mut targeting);
        }

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
                let Some(slot_mut) = loadout.slot_mut(index) else {
                    continue;
                };
                if !mana.try_spend(slot_mut.mana_cost) {
                    continue;
                }
                slot_mut.cooldown_remaining = slot_mut.cooldown;
                cast_instant(
                    &mut commands,
                    &assets,
                    hero_entity,
                    transform,
                    *team,
                    slot_mut,
                    &enemies,
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
                });
                spawn_indicators(
                    &mut commands,
                    &assets,
                    transform.translation,
                    cast_range,
                    aoe_radius,
                );
            }
        }
    }
}

fn cast_instant(
    commands: &mut Commands,
    assets: &SharedAssets,
    hero_entity: Entity,
    transform: &Transform,
    team: Team,
    slot: &crate::components::AbilitySlot,
    enemies: &Query<(Entity, &Transform, &Team, &Health, &CombatStats), Without<PlayerHero>>,
) {
    match slot.id {
        AbilityId::Dash => {
            let forward = transform.forward();
            let dest = transform.translation + *forward * slot.dash_distance();
            spawn_dash_ghosts(commands, assets, transform.translation, dest);
            commands.entity(hero_entity).insert(MoveTarget {
                position: Vec3::new(dest.x, 0.0, dest.z),
            });
        }
        AbilityId::Shockwave => {
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
            for (enemy_entity, enemy_tf, enemy_team, enemy_hp, stats) in enemies.iter() {
                if *enemy_team == team || !enemy_hp.is_alive() {
                    continue;
                }
                if flat_distance(origin, enemy_tf.translation) <= radius {
                    let amount = apply_damage(damage, DamageType::Magical, stats);
                    commands.entity(enemy_entity).insert(PendingDamage { amount });
                }
            }
        }
        AbilityId::Bolt | AbilityId::Nova => {}
    }
}

fn confirm_or_cancel_targeted_cast(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut targeting: ResMut<AbilityTargeting>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    ground: Query<&GlobalTransform, With<Ground>>,
    mut commands: Commands,
    assets: Res<SharedAssets>,
    mut hero: Query<
        (
            Entity,
            &Transform,
            &Team,
            &mut AbilityLoadout,
            &mut Mana,
            &mut Health,
        ),
        With<PlayerHero>,
    >,
    enemies: Query<(Entity, &Transform, &Team, &Health, &CombatStats, Option<&UnitRadius>), Without<PlayerHero>>,
) {
    let Some(pending) = targeting.active else {
        return;
    };

    // Cancel: Escape, Space, RMB (commands issued before LMB).
    let cancel = keys.just_pressed(KeyCode::Escape)
        || keys.just_pressed(KeyCode::Space)
        || mouse.just_pressed(MouseButton::Right);
    if cancel {
        clear_targeting(&mut commands, &mut targeting);
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok((hero_entity, transform, team, mut loadout, mut mana, mut health)) = hero.single_mut()
    else {
        return;
    };

    let Some(hit) = cursor_ground_hit(&windows, &camera, &ground) else {
        return;
    };

    let dist = flat_distance(transform.translation, hit);
    if dist > pending.cast_range {
        // Out of range — keep targeting so the player can adjust.
        return;
    }

    let Some(slot) = loadout.slot_mut(pending.slot) else {
        clear_targeting(&mut commands, &mut targeting);
        return;
    };
    if slot.rank == 0 || slot.cooldown_remaining > 0.0 || !mana.try_spend(slot.mana_cost) {
        clear_targeting(&mut commands, &mut targeting);
        return;
    }
    slot.cooldown_remaining = slot.cooldown;
    let ability = pending.ability;
    let aoe = pending.aoe_radius;
    let bolt_damage = slot.bolt_damage();
    let nova_damage = slot.nova_damage();
    let nova_heal = slot.nova_heal();
    clear_targeting(&mut commands, &mut targeting);

    match ability {
        AbilityId::Bolt => {
            let unit_target = enemies
                .iter()
                .filter(|(_, _, enemy_team, hp, _, _)| {
                    **enemy_team == team.enemy() && hp.is_alive()
                })
                .filter(|(_, tf, _, _, _, radius)| {
                    let r = radius.map(|r| r.0).unwrap_or(0.5);
                    flat_distance(tf.translation, hit) < r + 1.4
                })
                .min_by(|a, b| {
                    flat_distance(a.1.translation, hit)
                        .partial_cmp(&flat_distance(b.1.translation, hit))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

            if let Some((enemy, enemy_tf, _, _, _stats, _)) = unit_target {
                spawn_spell_bolt(
                    &mut commands,
                    &assets,
                    *team,
                    transform.translation,
                    Some(enemy),
                    enemy_tf.translation,
                    bolt_damage,
                    aoe,
                );
                commands.entity(hero_entity).insert(AttackTarget(enemy));
            } else {
                spawn_spell_bolt(
                    &mut commands,
                    &assets,
                    *team,
                    transform.translation,
                    None,
                    hit,
                    bolt_damage,
                    aoe,
                );
            }
        }
        AbilityId::Nova => {
            health.current = (health.current + nova_heal).min(health.max);
            spawn_expanding_ring(
                &mut commands,
                &assets,
                hit,
                aoe,
                assets.nova_mat.clone(),
                0.55,
            );
            for (enemy_entity, enemy_tf, enemy_team, enemy_hp, stats, _) in &enemies {
                if *enemy_team == *team || !enemy_hp.is_alive() {
                    continue;
                }
                if flat_distance(hit, enemy_tf.translation) <= aoe {
                    let amount = apply_damage(nova_damage, DamageType::Magical, stats);
                    commands.entity(enemy_entity).insert(PendingDamage { amount });
                }
            }
        }
        _ => {}
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
            Without<PlayerHero>,
        ),
    >,
    mut aoe_q: Query<
        &mut Transform,
        (
            With<AoeIndicator>,
            Without<RangeIndicator>,
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
        tf.translation = Vec3::new(hero_pos.x, 0.08, hero_pos.z);
        tf.scale = Vec3::new(pending.cast_range, 1.0, pending.cast_range);
    }

    let in_range = flat_distance(hero_pos, cursor) <= pending.cast_range;
    let aoe_pos = if in_range {
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

    for mut tf in &mut aoe_q {
        tf.translation = Vec3::new(aoe_pos.x, 0.1, aoe_pos.z);
        tf.scale = Vec3::new(pending.aoe_radius, 1.0, pending.aoe_radius);
    }
}

fn spawn_indicators(
    commands: &mut Commands,
    assets: &SharedAssets,
    hero_pos: Vec3,
    cast_range: f32,
    aoe_radius: f32,
) {
    commands.spawn((
        Name::new("Range Indicator"),
        RangeIndicator,
        Mesh3d(assets.indicator_ring_mesh.clone()),
        MeshMaterial3d(assets.indicator_range_mat.clone()),
        Transform::from_translation(Vec3::new(hero_pos.x, 0.08, hero_pos.z))
            .with_scale(Vec3::new(cast_range, 1.0, cast_range)),
    ));
    commands.spawn((
        Name::new("AoE Indicator"),
        AoeIndicator,
        Mesh3d(assets.indicator_ring_mesh.clone()),
        MeshMaterial3d(assets.indicator_aoe_mat.clone()),
        Transform::from_translation(Vec3::new(hero_pos.x, 0.1, hero_pos.z))
            .with_scale(Vec3::new(aoe_radius, 1.0, aoe_radius)),
    ));
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
    indicators: Query<Entity, Or<(With<RangeIndicator>, With<AoeIndicator>)>>,
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
        let pos = from.lerp(to, t) + Vec3::Y * 0.9;
        commands.spawn((
            Name::new("Dash Ghost"),
            Mesh3d(assets.unit_mesh.clone()),
            MeshMaterial3d(assets.dash_ghost_mat.clone()),
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
        Transform::from_translation(Vec3::new(origin.x, 0.12, origin.z))
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
