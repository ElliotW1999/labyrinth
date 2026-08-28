//! Hero ability loadout: Q dash, W shockwave, E bolt, R nova.

use bevy::prelude::*;

use crate::combat::{flat_distance, spawn_projectile};
use crate::components::{
    AbilityId, AbilityLoadout, AttackTarget, Health, Mana, MoveTarget, PlayerHero, Team,
};
use crate::resources::SharedAssets;

pub struct AbilitiesPlugin;

impl Plugin for AbilitiesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                tick_ability_cooldowns,
                regen_mana,
                cast_abilities,
                apply_pending_damage,
            ),
        );
    }
}

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

fn cast_abilities(
    keys: Res<ButtonInput<KeyCode>>,
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
    enemies: Query<(Entity, &Transform, &Team, &Health), Without<PlayerHero>>,
) {
    let Ok((hero_entity, transform, team, mut loadout, mut mana, mut health)) = hero.single_mut()
    else {
        return;
    };

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
        let Some(slot) = loadout.slot_mut(index) else {
            continue;
        };
        if slot.cooldown_remaining > 0.0 || !mana.try_spend(slot.mana_cost) {
            continue;
        }

        let ability = slot.id;
        let cooldown = slot.cooldown;
        slot.cooldown_remaining = cooldown;

        match ability {
            AbilityId::Dash => {
                let forward = transform.forward();
                let dest = transform.translation + *forward * 10.0;
                commands.entity(hero_entity).insert(MoveTarget {
                    position: Vec3::new(dest.x, 0.0, dest.z),
                });
            }
            AbilityId::Shockwave => {
                let origin = transform.translation;
                for (enemy_entity, enemy_tf, enemy_team, enemy_hp) in &enemies {
                    if *enemy_team == *team || !enemy_hp.is_alive() {
                        continue;
                    }
                    if flat_distance(origin, enemy_tf.translation) <= 9.0 {
                        commands.entity(enemy_entity).insert(PendingDamage(120.0));
                    }
                }
            }
            AbilityId::Bolt => {
                let origin = transform.translation;
                let target = enemies
                    .iter()
                    .filter(|(_, _, enemy_team, hp)| **enemy_team == team.enemy() && hp.is_alive())
                    .min_by(|a, b| {
                        flat_distance(origin, a.1.translation)
                            .partial_cmp(&flat_distance(origin, b.1.translation))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                if let Some((enemy, enemy_tf, _, _)) = target {
                    spawn_projectile(
                        &mut commands,
                        &assets,
                        *team,
                        origin,
                        enemy,
                        enemy_tf.translation,
                        140.0,
                        32.0,
                    );
                    commands.entity(hero_entity).insert(AttackTarget(enemy));
                }
            }
            AbilityId::Nova => {
                let origin = transform.translation;
                health.current = (health.current + 150.0).min(health.max);
                for (enemy_entity, enemy_tf, enemy_team, enemy_hp) in &enemies {
                    if *enemy_team == *team || !enemy_hp.is_alive() {
                        continue;
                    }
                    if flat_distance(origin, enemy_tf.translation) <= 14.0 {
                        commands.entity(enemy_entity).insert(PendingDamage(220.0));
                    }
                }
            }
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct PendingDamage(pub f32);

fn apply_pending_damage(
    mut commands: Commands,
    mut victims: Query<(Entity, &mut Health, &PendingDamage)>,
) {
    for (entity, mut health, damage) in &mut victims {
        health.current -= damage.0;
        commands.entity(entity).remove::<PendingDamage>();
    }
}
