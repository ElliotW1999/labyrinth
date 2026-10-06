//! Abilities whose logic doesn't fit the reusable effect list. They still go through
//! the shared cast pipeline (validation, cast point, costs) and emit shared events.

use bevy::prelude::*;

use super::definition::RankValue;
use super::effects::{AbilityAppExt, CastContext};
use crate::combat::{ProjectilePayload, SpawnProjectileEvent};
use crate::components::{AbilityId, DamageType, Health};

pub fn register(app: &mut App) {
    app.register_ability_behavior(AbilityId::Execute, execute);
}

const EXECUTE_DAMAGE: RankValue = RankValue::linear(110.0, 40.0);
const EXECUTE_THRESHOLD: f32 = 0.35;
const EXECUTE_BONUS: f32 = 1.55;

/// Bolt whose damage is boosted when the target is below 35% HP at the moment of casting.
fn execute(
    In(ctx): In<CastContext>,
    targets: Query<(&Transform, &Health)>,
    mut projectiles: MessageWriter<SpawnProjectileEvent>,
) {
    let target = ctx
        .unit_target
        .and_then(|e| targets.get(e).ok().map(|(tf, hp)| (e, tf.translation, *hp)));
    let mut damage = EXECUTE_DAMAGE.at(ctx.rank);
    if target.is_some_and(|(_, _, hp)| hp.current / hp.max.max(1.0) < EXECUTE_THRESHOLD) {
        damage *= EXECUTE_BONUS;
    }
    projectiles.write(SpawnProjectileEvent {
        team: ctx.team,
        origin: ctx.origin,
        target: target.map(|(e, _, _)| e),
        target_pos: target.map_or(ctx.aim, |(_, pos, _)| pos),
        damage,
        damage_type: DamageType::Magical,
        splash_radius: ctx.aoe_radius,
        payload: Some(ProjectilePayload {
            caster: ctx.caster,
            ability: ctx.ability,
            rank: ctx.rank,
            effect_index: None,
        }),
    });
}
