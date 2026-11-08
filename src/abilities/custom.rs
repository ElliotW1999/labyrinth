//! Abilities whose *mechanics* don't fit [`super::definition::AbilityMechanic`]
//! (anything the shared dash / charge / drag / projectile mechanics cannot express) register a one-shot system here with
//! `app.register_ability_behavior(id, system)`. They still go through the shared
//! cast pipeline and report what happened as `AbilityTriggerEvent`s, such as
//! `AbilityTrigger::Custom("on_my_event")` with the units involved as affected units.
//! Consequences stay in the ability's effect entries.
//!
//! Every built-in ability is currently expressed with the generic mechanics.

use bevy::prelude::*;

use super::effects::{AbilityEffectAppExt, CustomEffectInput, EffectSource};
use crate::combat::{DamageEvent, emit_chain_damage};
use crate::components::{BoundRadius, Health, Team};
use crate::scale;

fn bolt_chain(
    In(input): In<CustomEffectInput>,
    units: Query<(Entity, &Transform, &Team, &Health, Option<&BoundRadius>)>,
    mut damage: MessageWriter<DamageEvent>,
) {
    let Some(primary) = input.targets.first().copied() else {
        return;
    };
    if !matches!(input.ctx.source, EffectSource::Ability(_)) {
        return;
    }
    emit_chain_damage(
        &mut damage,
        primary,
        input.ctx.caster,
        input.amount,
        crate::components::DamageType::Magical,
        input.ctx.team,
        input.value.round() as u32,
        scale::u(450.0),
        0.75,
        &units,
    );
}

pub fn register(app: &mut App) {
    app.register_custom_effect("bolt_chain", bolt_chain);
}
