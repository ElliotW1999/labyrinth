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
//!    (actions that don't occupy the caster fire right away and keep current orders)
//! ```
//!
//! Source layers own what is specific to them: the ability layer
//! (`abilities::casting`) decides whether an ability is learned / castable, builds
//! its [`ActionSpec`] from the definition and rank, and turns
//! [`ActionFiredEvent`]s into ability casts. The item layer (`items`) checks that the
//! caster's inventory slot holds an active item, builds the spec from the item's
//! activation data and instance cooldown / charges, and spends charges / stacks and
//! fires `OnUse` from [`ActionFiredEvent`]s.

pub mod pipeline;
pub mod resources;
pub mod targeting;

use bevy::prelude::*;

use crate::components::{AbilityId, CastTarget};
use crate::items::{ItemId, ItemInstanceId};

pub use pipeline::{
    ActionSources, resolve_queued_activations, tick_activations, validate_activation_requests,
};
pub use resources::{ActionCooldown, ActionCost, DisableRule, check_ready};
pub use targeting::{
    CasterInfo, ResolvedTarget, TargetTeam, TargetType, UnitInfo, in_range, resolve_target,
    team_allows,
};

/// Which slot of the caster an activation comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionSource {
    /// Index into the caster's `AbilityLoadout`.
    Ability(usize),
    /// Index into the caster's `Inventory`.
    Item(usize),
}

/// What occupies the source slot. Stored on in-flight activations so a slot that
/// changed contents (re-skilled, item sold) cancels instead of firing something else.
/// Items carry their instance so each copy keeps its own cooldown and charges and an
/// in-flight use follows that copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionId {
    Ability(AbilityId),
    Item {
        item: ItemId,
        instance: ItemInstanceId,
    },
}

/// Attempt to activate `source` on `caster`, aimed at `target`.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ActivationRequest {
    pub caster: Entity,
    pub source: ActionSource,
    pub target: CastTarget,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ActivationRequest {
    pub fn ability(caster: Entity, slot: usize, target: CastTarget) -> Self {
        Self {
            caster,
            source: ActionSource::Ability(slot),
            target,
        }
    }

    pub fn item(caster: Entity, slot: usize, target: CastTarget) -> Self {
        Self {
            caster,
            source: ActionSource::Item(slot),
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
    pub disable: DisableRule,
    /// Holds an [`Activation`] (stops the caster, turns, cast point, channel,
    /// backswing). When false (no cast point or channel) the action fires the moment
    /// it is validated in range, without touching the caster's current orders.
    pub occupies_caster: bool,
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
    pub disable: DisableRule,
    /// Time left in the cast point (the action fires when this hits 0).
    pub point_remaining: f32,
    /// After firing, remaining channel time (interrupted by silence / stun).
    pub channel_remaining: f32,
    /// After the channel, remaining backswing (cancellable).
    pub backswing_remaining: f32,
    pub fired: bool,
}

impl Activation {
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
