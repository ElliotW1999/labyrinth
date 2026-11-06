//! Shared gameplay effects: trigger context, effect entries, and the single executor.
#![allow(unused_imports)]
//!
//! Types and systems live under [`crate::abilities`] today; this module is a stable
//! re-export surface for callers that should not depend on the ability plugin layout.

pub use crate::abilities::definition::{
    AbilityEffect, AbilityEffectEntry, AbilityTrigger, EffectScaling, EffectTarget,
};
pub use crate::abilities::effects::{
    AbilityEffectAppExt, AbilityTriggerEvent, CustomAbilityEffects, CustomEffectInput,
    EffectRunner, EffectSource, HealEvent, ManaRestoreEvent, StatusChange, StatusEffectEvent,
    TriggerContext, apply_heal_events, apply_mana_restore_events, apply_status_effect_events,
    resolve_ability_triggers,
};
