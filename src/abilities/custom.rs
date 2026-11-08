//! Abilities whose *mechanics* don't fit [`super::definition::AbilityMechanic`]
//! (anything the shared dash / charge / drag / projectile mechanics cannot express) register a one-shot system here with
//! `app.register_ability_behavior(id, system)`. They still go through the shared
//! cast pipeline and report what happened as `AbilityTriggerEvent`s, such as
//! `AbilityTrigger::Custom("on_my_event")` with the units involved as affected units.
//! Consequences stay in the ability's effect entries. Custom *effects*
//! (`app.register_custom_effect`) are the other escape hatch, for consequences the
//! shared effects cannot express.
//!
//! Every built-in ability is currently expressed with the generic mechanics and effects;
//! even Bolt's chain is the shared `AbilityEffect::Chain` gate plus an `on_chain_hit`
//! damage row. Nothing is registered here.

use bevy::prelude::*;

pub fn register(_app: &mut App) {}
