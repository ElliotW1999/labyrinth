//! Story Mode: a room-based dungeon crawl built on the shared match lifecycle.
//!
//! Story rules sit outside the six gameplay layers (see `docs/architecture/composition.md`):
//! they are match-level rules that only emit through existing APIs. `map`, `items`,
//! `units` and `ai` never import this module.

use bevy::prelude::*;

use crate::net::NetMode;

/// Where the local hero stands in the safe starting room. Used by hero spawn so the hero
/// never appears at the classic base when a Story match begins.
pub const SAFE_ENTRY: Vec3 = Vec3::new(0.0, crate::scale::body(0.9), -900.0);

pub struct StoryPlugin;

impl Plugin for StoryPlugin {
    fn build(&self, _app: &mut App) {}
}

/// Story Mode is offline only in v1: rooms, lives and pickups are authority-owned state
/// that the UDP prototype does not replicate.
pub fn story_available(mode: NetMode) -> bool {
    mode == NetMode::Offline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_is_offline_only() {
        assert!(story_available(NetMode::Offline));
        assert!(!story_available(NetMode::Host));
        assert!(!story_available(NetMode::Client));
    }
}
