//! Unit command queue (Dota-style Shift queuing).
//!
//! Input, the minimap, and the network host send [`IssueCommand`] messages. A
//! normal command clears the unit's [`CommandQueue`] and starts immediately; a
//! queued (Shift) command is appended and starts when everything before it has
//! finished. Starting a command reuses the existing order components
//! (`MoveTarget`, `AttackTarget`, `AttackMoveOrder`, `AbilityCastRequest`), so
//! the AI, navigation, and casting pipelines are unchanged.
//!
//! Completion:
//! - `Move` — the unit arrived (or movement gave up next to a blocked goal): `MoveTarget` is gone.
//! - `AttackMove` — `AttackMoveOrder` is gone (destination reached).
//! - `Attack` — the target died / was removed, or the attack was cancelled by something else.
//! - `CastAbility` — the ability fired (the next command may cancel its backswing), or the cast
//!   was rejected / interrupted before firing. Unit, point, and no-target casts all follow this.
//! - `Stop` — immediately.
//!
//! A command whose target is already invalid when it would start (dead or missing
//! unit) is skipped, and a command that never takes effect (e.g. a cast rejected for
//! cooldown or mana) completes after a few frames, so neither can stall the queue.

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::abilities::casting::AbilityCastRequest;
use crate::components::{
    AbilityCasting, AttackMoveOrder, AttackTarget, CastTarget, CollisionRadius, Health, MoveTarget,
    QueuedAbilityCast,
};
use crate::dimensions::collision_of;
use crate::movement::{order_attack_move, order_attack_unit, order_hero_move, order_hero_stop, SimSet};
use crate::navigation::{flat, ground, NavGrid};

/// Frames a started command may go without any visible effect before it counts as failed.
const EFFECT_GRACE_FRAMES: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnitCommand {
    Move { destination: Vec3 },
    AttackMove { destination: Vec3 },
    Attack { target: Entity },
    CastAbility { slot: usize, target: CastTarget },
    Stop,
}

impl UnitCommand {
    /// Ground point this command walks to, if any (debug drawing).
    pub fn destination(&self) -> Option<Vec3> {
        match *self {
            UnitCommand::Move { destination } | UnitCommand::AttackMove { destination } => Some(destination),
            UnitCommand::CastAbility {
                target: CastTarget::Point(p),
                ..
            } => Some(p),
            _ => None,
        }
    }
}

/// Request that `unit` performs `command`. `queue: true` is Shift: append instead of replace.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct IssueCommand {
    pub unit: Entity,
    pub command: UnitCommand,
    pub queue: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveCommand {
    pub command: UnitCommand,
    frames: u32,
    /// The command's order component has been seen on the unit at least once.
    observed: bool,
}

impl ActiveCommand {
    fn new(command: UnitCommand) -> Self {
        Self {
            command,
            frames: 0,
            observed: false,
        }
    }
}

/// Current and pending commands of a controllable unit.
#[derive(Component, Debug, Clone, Default)]
pub struct CommandQueue {
    pub current: Option<ActiveCommand>,
    pub queued: VecDeque<UnitCommand>,
}

impl CommandQueue {
    /// Current command followed by the queued ones.
    pub fn iter(&self) -> impl Iterator<Item = &UnitCommand> {
        self.current.iter().map(|a| &a.command).chain(self.queued.iter())
    }
}

pub struct UnitCommandsPlugin;

impl Plugin for UnitCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<IssueCommand>().add_systems(
            Update,
            (process_issued_commands, advance_command_queues)
                .chain()
                .in_set(SimSet::Commands)
                .run_if(crate::net::is_sim_authority),
        );
    }
}

/// Everything `start_command` needs to validate and translate a command.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CommandContext<'w, 's> {
    commands: Commands<'w, 's>,
    casts: MessageWriter<'w, AbilityCastRequest>,
    grid: Res<'w, NavGrid>,
    radii: Query<'w, 's, Option<&'static CollisionRadius>>,
    health: Query<'w, 's, &'static Health>,
}

impl CommandContext<'_, '_> {
    fn target_alive(&self, target: Entity) -> bool {
        self.health.get(target).is_ok_and(|h| h.is_alive())
    }

    /// Non-navigable destinations resolve to the closest point the unit fits.
    fn resolve_destination(&self, unit: Entity, destination: Vec3) -> Vec3 {
        let r = self.radii.get(unit).map_or(0.0, collision_of);
        self.grid
            .nearest_walkable(flat(destination), r)
            .map_or(destination, ground)
    }

    /// Translate a command into order components. Returns `None` when it is invalid
    /// up front (dead / missing target) so the caller can skip to the next one.
    fn start(&mut self, unit: Entity, command: UnitCommand) -> Option<UnitCommand> {
        match command {
            UnitCommand::Move { destination } => {
                let destination = self.resolve_destination(unit, destination);
                order_hero_move(&mut self.commands, unit, destination);
                Some(UnitCommand::Move { destination })
            }
            UnitCommand::AttackMove { destination } => {
                let destination = self.resolve_destination(unit, destination);
                order_attack_move(&mut self.commands, unit, destination);
                Some(UnitCommand::AttackMove { destination })
            }
            UnitCommand::Attack { target } => {
                if target == unit || !self.target_alive(target) {
                    return None;
                }
                order_attack_unit(&mut self.commands, unit, target);
                Some(command)
            }
            UnitCommand::CastAbility { slot, target } => {
                if target.unit().is_some_and(|t| !self.target_alive(t)) {
                    return None;
                }
                self.commands
                    .entity(unit)
                    .remove::<(QueuedAbilityCast, AbilityCasting)>();
                self.casts.write(AbilityCastRequest {
                    caster: unit,
                    slot,
                    target,
                });
                Some(command)
            }
            UnitCommand::Stop => {
                order_hero_stop(&mut self.commands, unit);
                Some(command)
            }
        }
    }

    /// Start queued commands until one is valid (or the queue is empty).
    fn start_next(&mut self, unit: Entity, queue: &mut CommandQueue) {
        queue.current = None;
        while let Some(next) = queue.queued.pop_front() {
            if let Some(started) = self.start(unit, next) {
                queue.current = Some(ActiveCommand::new(started));
                return;
            }
        }
    }
}

pub fn process_issued_commands(
    mut issued: MessageReader<IssueCommand>,
    mut queues: Query<&mut CommandQueue>,
    mut ctx: CommandContext,
) {
    for issue in issued.read() {
        let Ok(mut queue) = queues.get_mut(issue.unit) else {
            // Units without a queue still obey, they just can't stack commands.
            ctx.start(issue.unit, issue.command);
            continue;
        };
        if issue.queue && queue.current.is_some() {
            queue.queued.push_back(issue.command);
            continue;
        }
        if !issue.queue {
            queue.queued.clear();
        }
        queue.queued.push_front(issue.command);
        ctx.start_next(issue.unit, &mut queue);
    }
}

type OrderState<'a> = (
    Has<MoveTarget>,
    Has<AttackMoveOrder>,
    Option<&'a AttackTarget>,
    Option<&'a AbilityCasting>,
    Has<QueuedAbilityCast>,
);

/// Outcome of checking the current command against the unit's order components.
fn command_finished(
    active: &mut ActiveCommand,
    (moving, attack_moving, attack, casting, queued_cast): (bool, bool, Option<&AttackTarget>, Option<&AbilityCasting>, bool),
    target_alive: impl Fn(Entity) -> bool,
) -> bool {
    active.frames += 1;
    let in_effect = match active.command {
        UnitCommand::Move { .. } => moving,
        UnitCommand::AttackMove { .. } => attack_moving,
        UnitCommand::Attack { target } => {
            if !target_alive(target) {
                return true;
            }
            attack.is_some_and(|a| a.0 == target)
        }
        UnitCommand::CastAbility { slot, target } => {
            let mine = casting.filter(|c| c.slot == slot);
            if mine.is_some_and(|c| c.fired) {
                return true;
            }
            if target.unit().is_some_and(|t| !target_alive(t)) && mine.is_none() {
                return true;
            }
            mine.is_some() || queued_cast
        }
        UnitCommand::Stop => return true,
    };
    if in_effect {
        active.observed = true;
        return false;
    }
    active.observed || active.frames > EFFECT_GRACE_FRAMES
}

pub fn advance_command_queues(
    mut units: Query<(Entity, &mut CommandQueue, OrderState)>,
    mut ctx: CommandContext,
) {
    for (unit, mut queue, state) in &mut units {
        let Some(active) = queue.current.as_mut() else {
            continue;
        };
        if command_finished(active, state, |t| ctx.target_alive(t)) {
            ctx.start_next(unit, &mut queue);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(i: u32) -> Entity {
        Entity::from_raw_u32(i).unwrap()
    }

    fn casting(slot: usize, fired: bool) -> AbilityCasting {
        AbilityCasting {
            slot,
            ability: crate::components::AbilityId::Bolt,
            target: CastTarget::None,
            aim: Vec3::ZERO,
            point_remaining: 0.0,
            backswing_remaining: 0.3,
            fired,
        }
    }

    #[test]
    fn move_completes_when_move_target_clears() {
        let mut a = ActiveCommand::new(UnitCommand::Move {
            destination: Vec3::X,
        });
        assert!(!command_finished(&mut a, (true, false, None, None, false), |_| true));
        assert!(command_finished(&mut a, (false, false, None, None, false), |_| true));
    }

    #[test]
    fn attack_completes_on_death_or_retarget() {
        let target = e(3);
        let mut a = ActiveCommand::new(UnitCommand::Attack { target });
        let attacking = AttackTarget(target);
        assert!(!command_finished(&mut a, (true, false, Some(&attacking), None, false), |_| true));
        assert!(command_finished(&mut a, (false, false, Some(&attacking), None, false), |_| false));
        let mut b = ActiveCommand::new(UnitCommand::Attack { target });
        assert!(!command_finished(&mut b, (false, false, Some(&attacking), None, false), |_| true));
        let other = AttackTarget(e(4));
        assert!(command_finished(&mut b, (false, false, Some(&other), None, false), |_| true));
    }

    #[test]
    fn cast_completes_when_fired_or_rejected() {
        let mut a = ActiveCommand::new(UnitCommand::CastAbility {
            slot: 1,
            target: CastTarget::None,
        });
        assert!(!command_finished(&mut a, (false, false, None, Some(&casting(1, false)), false), |_| true));
        assert!(command_finished(&mut a, (false, false, None, Some(&casting(1, true)), false), |_| true));

        // Never takes effect (cooldown / mana): fails after the grace period.
        let mut b = ActiveCommand::new(UnitCommand::CastAbility {
            slot: 2,
            target: CastTarget::None,
        });
        let mut finished = false;
        for _ in 0..=EFFECT_GRACE_FRAMES {
            finished = command_finished(&mut b, (false, false, None, None, false), |_| true);
        }
        assert!(finished);

        // Walking into range keeps it alive; the target dying ends it.
        let mut c = ActiveCommand::new(UnitCommand::CastAbility {
            slot: 1,
            target: CastTarget::Unit(e(9)),
        });
        assert!(!command_finished(&mut c, (true, false, None, None, true), |_| true));
        assert!(command_finished(&mut c, (true, false, None, None, true), |_| false));
    }
}
