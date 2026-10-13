//! Shared activation pipeline for anything a unit can trigger: abilities today,
//! active items next.
//!
//! ```text
//! ActivationRequest { caster, source, target }
//!   → ActionSources::spec   (source layer: ability learned / castable, … → ActionSpec)
//!   → check_ready           (cooldown + charges, mana, caster can act)
//!   → resolve_target        (target type, team rule, living unit)
//!   ─┬─ out of range → QueuedActivation + MoveTarget (walk, re-request)
//!    └─ ok → Activation: turn → cast point
//!               → re-check, pay cost, ActionSources::commit (cooldown, charges, source rules)
//!               → ActionFiredEvent → channel → ActionChannelEndEvent → backswing
//! ```
//!
//! Source layers own what is specific to them: the ability layer
//! (`abilities::casting`) decides whether an ability is learned / castable, builds
//! its [`ActionSpec`] from the definition and rank, and turns
//! [`ActionFiredEvent`]s into ability casts. An item layer will plug in the same way
//! (inventory ownership, item availability) through [`ActionSource::Item`].

pub mod pipeline;
pub mod resources;
pub mod targeting;

use bevy::prelude::*;

use crate::components::{AbilityId, CastTarget};
use crate::items::ItemId;

pub use pipeline::{resolve_queued_activations, tick_activations, validate_activation_requests};
pub use resources::{ActionCooldown, ActionCost, check_ready};
pub use targeting::{
    CasterInfo, ResolvedTarget, TargetTeam, TargetType, UnitInfo, in_range, resolve_target,
    team_allows,
};

/// Which slot of the caster an activation comes from.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionSource {
    /// Index into the caster's `AbilityLoadout`.
    Ability(usize),
    /// Index into the caster's `Inventory` (not wired yet; rejected as unavailable).
    Item(usize),
}

/// What occupies the source slot. Stored on in-flight activations so a slot that
/// changed contents (re-skilled, item sold) cancels instead of firing something else.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionId {
    Ability(AbilityId),
    Item(ItemId),
}

/// Attempt to activate `source` on `caster`, aimed at `target`.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ActivationRequest {
    pub caster: Entity,
    pub source: ActionSource,
    pub target: CastTarget,
}

impl ActivationRequest {
    pub fn ability(caster: Entity, slot: usize, target: CastTarget) -> Self {
        Self {
            caster,
            source: ActionSource::Ability(slot),
            target,
        }
    }
}

/// Source-independent rules of one activation, resolved by the source layer at the
/// source's current level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActionSpec {
    pub action: ActionId,
    /// Ability rank; items use 1.
    pub level: u32,
    pub target_type: TargetType,
    pub target_team: TargetTeam,
    pub cast_range: f32,
    /// Seconds before the action fires (turning toward the aim comes first).
    pub cast_time: f32,
    /// Seconds the caster channels after firing (`0` = not channelled).
    pub channel_time: f32,
    pub backswing: f32,
    pub cost: ActionCost,
    /// Cooldown started on commit.
    pub cooldown: f32,
    /// Current cooldown / charge state of the source slot.
    pub cooldown_state: ActionCooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionError {
    /// The source layer refused (unlearned / passive ability, missing slot, …).
    Unavailable,
    OnCooldown,
    NotEnoughMana,
    CasterDisabled,
    InvalidTarget,
}

/// In-progress activation: cast point, then channel, then backswing.
#[derive(Component, Debug, Clone, Copy)]
pub struct Activation {
    pub source: ActionSource,
    pub action: ActionId,
    pub level: u32,
    pub target: CastTarget,
    pub aim: Vec3,
    /// Time left in the cast point (the action fires when this hits 0).
    pub point_remaining: f32,
    /// After firing, remaining channel time (interrupted by silence / stun).
    pub channel_remaining: f32,
    /// After the channel, remaining backswing (cancellable).
    pub backswing_remaining: f32,
    pub fired: bool,
}

impl Activation {
    pub fn is_ability_slot(&self, slot: usize) -> bool {
        self.source == ActionSource::Ability(slot)
    }

    /// Fired and no longer channelling: the order it came from is done.
    pub fn released(&self) -> bool {
        self.fired && self.channel_remaining <= 0.0
    }
}

/// Validated activation waiting for the caster to walk into range; re-requested once in range.
#[derive(Component, Debug, Clone, Copy)]
pub struct QueuedActivation {
    pub source: ActionSource,
    pub action: ActionId,
    pub target: CastTarget,
    /// Last known aim (tracks a unit target while it lives).
    pub aim: Vec3,
}

/// An activation finished its cast point and paid its costs.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ActionFiredEvent {
    pub caster: Entity,
    pub source: ActionSource,
    pub action: ActionId,
    pub level: u32,
    pub target: CastTarget,
    pub aim: Vec3,
}

/// A channelled activation stopped: ran its full `channel_time` (`completed`) or the
/// caster was silenced / stunned. Channels cancelled by a new order end silently.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ActionChannelEndEvent {
    pub caster: Entity,
    pub source: ActionSource,
    pub action: ActionId,
    pub level: u32,
    pub target: CastTarget,
    pub aim: Vec3,
    pub completed: bool,
}

/// Registers the shared messages. The pipeline systems are scheduled by the source
/// layers that need them (see `AbilitiesPlugin`), so they run in the right place
/// relative to the source's mechanics.
pub struct ActionsPlugin;

impl Plugin for ActionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ActivationRequest>()
            .add_message::<ActionFiredEvent>()
            .add_message::<ActionChannelEndEvent>();
    }
}
