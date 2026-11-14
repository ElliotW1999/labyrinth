//! Lives, downed heroes and respawn. A hero that dies loses one life and is `Downed`; after
//! [`super::STORY_RESPAWN_SECS`] it is revived at the current room's entry. The death that
//! uses the last life ends the run: the hero stays dead.

use bevy::prelude::*;

use super::{STORY_RESPAWN_SECS, StoryPhase, StoryRun, StoryRunEndedEvent};
use crate::components::{Downed, Health, Mana};
use crate::net::NetworkedHero;
use crate::status::{DispelStrength, StatusChange, StatusEffectEvent, StatusKind};
use crate::unit_commands::CommandQueue;

/// Lives a hero has left. Inserted on every hero when a story run starts.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeroLives {
    pub remaining: u32,
}

/// Strip everything a relocated or downed hero was doing: movement orders and steering,
/// attack target, activation, queued cast and the command queue (invariant S7).
pub fn clear_orders(commands: &mut Commands, entity: Entity, queue: Option<&mut CommandQueue>) {
    crate::teleport::drop_stale_orders(commands, entity);
    crate::movement::order_hero_stop(commands, entity);
    if let Some(queue) = queue {
        queue.current = None;
        queue.queued.clear();
    }
}

/// A strong dispel of debuffs through the normal status path (held and permanent statuses
/// are untouched), so damage over time cannot kill a hero that has just been revived.
fn dispel_debuffs(statuses: &mut MessageWriter<StatusEffectEvent>, hero: Entity) {
    statuses.write(StatusEffectEvent::new(
        None,
        hero,
        StatusChange::Dispel {
            kind: StatusKind::Debuff,
            strength: DispelStrength::Strong,
        },
    ));
}

/// A hero at zero health uses a life. Out of lives the run is over and the hero stays
/// dead; otherwise it goes down and waits to respawn. Runs after `apply_damage_events`.
#[allow(clippy::type_complexity)]
pub fn story_hero_deaths(
    mut commands: Commands,
    mut run: ResMut<StoryRun>,
    mut heroes: Query<
        (Entity, &mut HeroLives, &Health, Option<&mut CommandQueue>),
        (With<NetworkedHero>, Without<Downed>),
    >,
    mut statuses: MessageWriter<StatusEffectEvent>,
    mut ended: MessageWriter<StoryRunEndedEvent>,
) {
    for (entity, mut lives, health, mut queue) in &mut heroes {
        if health.is_alive() {
            continue;
        }
        lives.remaining = lives.remaining.saturating_sub(1);
        clear_orders(&mut commands, entity, queue.as_deref_mut());
        dispel_debuffs(&mut statuses, entity);
        if lives.remaining == 0 {
            // Never revived: the infinite timer keeps the hero down for good.
            commands.entity(entity).insert(Downed {
                respawn_in: f32::INFINITY,
            });
            if !matches!(run.phase, StoryPhase::GameOver | StoryPhase::Victory) {
                run.phase = StoryPhase::GameOver;
                ended.write(StoryRunEndedEvent { victory: false });
            }
        } else {
            commands.entity(entity).insert(Downed {
                respawn_in: STORY_RESPAWN_SECS,
            });
        }
    }
}

/// Count the respawn timers down and bring heroes back at the room entry with full health
/// and mana.
#[allow(clippy::type_complexity)]
pub fn tick_downed_heroes(
    time: Res<Time>,
    mut commands: Commands,
    run: Res<StoryRun>,
    mut heroes: Query<
        (
            Entity,
            &mut Downed,
            &mut Health,
            &mut Mana,
            &mut Transform,
            Option<&mut CommandQueue>,
        ),
        With<NetworkedHero>,
    >,
    mut statuses: MessageWriter<StatusEffectEvent>,
) {
    let dt = time.delta_secs();
    for (entity, mut downed, mut health, mut mana, mut transform, mut queue) in &mut heroes {
        downed.respawn_in -= dt;
        if downed.respawn_in > 0.0 {
            continue;
        }
        crate::vitals::revive(&mut health, &mut mana);
        transform.translation.x = run.entry.x;
        transform.translation.z = run.entry.z;
        clear_orders(&mut commands, entity, queue.as_deref_mut());
        dispel_debuffs(&mut statuses, entity);
        commands.entity(entity).remove::<Downed>();
    }
}
