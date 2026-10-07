//! Abilities whose *mechanics* don't fit [`super::definition::AbilityMechanic`]
//! (e.g. a Skewer-style drag) register a one-shot system here with
//! `app.register_ability_behavior(id, system)`. They still go through the shared
//! cast pipeline and report what happened as `AbilityTriggerEvent`s, such as
//! `AbilityTrigger::Custom("on_skewer_end")` with the dragged units as affected units.
//! Consequences stay in the ability's effect entries.
//!
//! Every built-in ability is currently expressed with the generic mechanics.

use bevy::prelude::*;

pub fn register(_app: &mut App) {}
