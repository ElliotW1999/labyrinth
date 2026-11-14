//! Room definitions (data, generated from `data/rooms.csv` and `data/room_spawns.csv`) and
//! the pure rules built on them: which rooms are eligible at a depth, how the two exits are
//! drawn, and where a group of creeps stands.

use bevy::prelude::*;

use super::StoryRng;
use super::layouts::RoomLayoutId;
pub use super::room_catalog::{FINAL_DEPTH, ROOMS, RoomId};
use crate::components::CreepClass;

/// How a room plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomKind {
    /// No enemies; a shop and the exits.
    Safe,
    Combat,
    Elite,
    Boss,
}

/// What clears a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomGoal {
    /// Cleared on entry (the safe camp).
    None,
    /// Cleared when no living hostile remains.
    KillAll,
}

/// What clearing a room pays out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomReward {
    None,
    /// Gold added to every hero's wallet.
    Gold(u32),
    /// A neutral item pickup at the room centre.
    NeutralItem,
}

/// A group of identical creeps: `count` of them spread on a small ring around `offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomSpawn {
    pub ranged: bool,
    pub class: CreepClass,
    pub count: u32,
    /// World XZ offset from the room centre.
    pub offset: Vec2,
}

#[derive(Debug, Clone, Copy)]
pub struct RoomDefinition {
    pub id: RoomId,
    pub name: &'static str,
    pub kind: RoomKind,
    pub layout: RoomLayoutId,
    /// Depths (rooms cleared so far, counting the safe camp as 0) the room can appear at.
    pub min_depth: u32,
    pub max_depth: u32,
    /// Relative odds in the exit draw (at least 1).
    pub weight: u32,
    pub goal: RoomGoal,
    pub reward: RoomReward,
    /// Spawns an item shop.
    pub shop: bool,
    pub description: &'static str,
    pub spawns: &'static [RoomSpawn],
}

impl RoomId {
    pub fn definition(self) -> &'static RoomDefinition {
        ROOMS
            .iter()
            .find(|room| room.id == self)
            .expect("every RoomId has a definition")
    }

    pub fn name(self) -> &'static str {
        self.definition().name
    }
}

impl RoomDefinition {
    /// Total creeps the room spawns.
    pub fn hostile_count(&self) -> u32 {
        self.spawns.iter().map(|spawn| spawn.count).sum()
    }

    /// Whether the room can be offered as an exit leading to `depth`.
    pub fn eligible_at(&self, depth: u32) -> bool {
        self.kind != RoomKind::Safe && (self.min_depth..=self.max_depth).contains(&depth)
    }

    /// Short text for an armed exit pad: the room name and what it pays.
    pub fn exit_label(&self) -> String {
        match self.reward {
            RoomReward::None => self.name.to_string(),
            RoomReward::Gold(amount) => format!("{} - {amount} gold", self.name),
            RoomReward::NeutralItem => format!("{} - neutral item", self.name),
        }
    }
}

/// The starting room: the one `Safe` room of the catalog.
pub fn safe_room() -> RoomId {
    ROOMS
        .iter()
        .find(|room| room.kind == RoomKind::Safe)
        .map(|room| room.id)
        .expect("the catalog has a safe room")
}

/// Two distinct rooms eligible at `depth`, drawn by weight. With only one candidate the
/// second exit stays `None`; with none both are.
pub fn choose_exits(depth: u32, rng: &mut StoryRng) -> [Option<RoomId>; 2] {
    let mut pool: Vec<&RoomDefinition> = ROOMS.iter().filter(|r| r.eligible_at(depth)).collect();
    let mut exits = [None, None];
    for exit in &mut exits {
        if pool.is_empty() {
            break;
        }
        let total: u32 = pool.iter().map(|room| room.weight.max(1)).sum();
        let mut roll = rng.below(total);
        let index = pool
            .iter()
            .position(|room| {
                let weight = room.weight.max(1);
                if roll < weight {
                    true
                } else {
                    roll -= weight;
                    false
                }
            })
            .unwrap_or(0);
        *exit = Some(pool.remove(index).id);
    }
    exits
}

/// Ring radius per creep of a group.
const SPAWN_RING_STEP: f32 = 60.0;

/// Where the `count` creeps of a group at `offset` (relative to the room centre) stand: the
/// offset itself for one, else evenly on a ring of radius `60 x count` around it.
pub fn spawn_points(count: u32, offset: Vec2) -> Vec<Vec2> {
    if count <= 1 {
        return vec![offset];
    }
    let radius = SPAWN_RING_STEP * count as f32;
    (0..count)
        .map(|i| {
            let angle = std::f32::consts::TAU * i as f32 / count as f32;
            offset + Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::layouts::grid_of;

    #[test]
    fn catalog_has_one_safe_shop_room_at_depth_zero_and_a_boss_at_final_depth() {
        let safe: Vec<_> = ROOMS.iter().filter(|r| r.kind == RoomKind::Safe).collect();
        assert_eq!(safe.len(), 1);
        let camp = safe[0];
        assert_eq!((camp.min_depth, camp.max_depth), (0, 0));
        assert!(camp.shop);
        assert_eq!(camp.goal, RoomGoal::None);
        assert!(camp.spawns.is_empty());
        assert_eq!(camp.id, safe_room());

        let bosses: Vec<_> = ROOMS.iter().filter(|r| r.kind == RoomKind::Boss).collect();
        assert!(!bosses.is_empty());
        for boss in bosses {
            assert_eq!((boss.min_depth, boss.max_depth), (FINAL_DEPTH, FINAL_DEPTH));
            assert!(boss.spawns.iter().any(|s| s.class == CreepClass::Boss));
        }
        for room in ROOMS.iter().filter(|r| r.kind != RoomKind::Safe) {
            assert_eq!(room.goal, RoomGoal::KillAll, "{}", room.name);
            assert!(!room.spawns.is_empty(), "{}", room.name);
            assert!(!room.shop, "{}", room.name);
        }
    }

    #[test]
    fn every_depth_has_an_eligible_room() {
        for depth in 1..FINAL_DEPTH {
            assert!(
                ROOMS
                    .iter()
                    .any(|r| r.eligible_at(depth) && r.kind != RoomKind::Boss),
                "depth {depth}"
            );
        }
        assert!(
            ROOMS
                .iter()
                .any(|r| r.eligible_at(FINAL_DEPTH) && r.kind == RoomKind::Boss)
        );
        assert!(
            ROOMS.iter().all(|r| !r.eligible_at(0)),
            "the safe camp is never an exit"
        );
    }

    #[test]
    fn choose_exits_is_deterministic_and_distinct_for_a_seed() {
        for seed in 0..40 {
            let first = choose_exits(2, &mut StoryRng::new(seed));
            let again = choose_exits(2, &mut StoryRng::new(seed));
            assert_eq!(first, again, "seed {seed}");
            let [Some(a), Some(b)] = first else {
                panic!("depth 2 offers at least two rooms");
            };
            assert_ne!(a, b, "seed {seed}");
            assert!(a.definition().eligible_at(2) && b.definition().eligible_at(2));
        }
    }

    #[test]
    fn choose_exits_single_candidate_leaves_second_pad_empty() {
        let exits = choose_exits(FINAL_DEPTH, &mut StoryRng::new(3));
        assert!(exits[0].is_some_and(|id| id.definition().kind == RoomKind::Boss));
        assert_eq!(exits[1], None);
        // Nothing is eligible past the final depth.
        assert_eq!(
            choose_exits(FINAL_DEPTH + 1, &mut StoryRng::new(3)),
            [None, None]
        );
    }

    #[test]
    fn exits_cover_every_eligible_room_over_many_seeds() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..200 {
            for exit in choose_exits(2, &mut StoryRng::new(seed))
                .into_iter()
                .flatten()
            {
                seen.insert(exit);
            }
        }
        let eligible = ROOMS.iter().filter(|r| r.eligible_at(2)).count();
        assert_eq!(seen.len(), eligible);
    }

    #[test]
    fn spawn_points_ring_the_offset() {
        let offset = Vec2::new(10.0, -20.0);
        assert_eq!(spawn_points(1, offset), vec![offset]);
        let points = spawn_points(4, offset);
        assert_eq!(points.len(), 4);
        for point in points {
            assert!((point.distance(offset) - 240.0).abs() < 0.01);
        }
    }

    #[test]
    fn every_spawn_point_entry_exit_and_shop_is_walkable_in_its_layout() {
        // The largest scaled creep radius: a Boss's collision circle.
        let boss_radius = crate::dimensions::UnitDimensions::MELEE_CREEP.collision
            * CreepClass::Boss.profile().model_scale;
        for room in ROOMS {
            let layout = room.layout.layout();
            let grid = grid_of(layout);
            let mut points = vec![
                ("entry", layout.entry),
                ("exit 0", layout.exits[0]),
                ("exit 1", layout.exits[1]),
                ("reward pickup", Vec2::ZERO),
            ];
            if room.shop {
                points.push(("shop", layout.shop));
            }
            for spawn in room.spawns {
                for point in spawn_points(spawn.count, spawn.offset) {
                    points.push(("spawn", point));
                }
            }
            for (what, point) in points {
                assert!(
                    grid.point_clear(point, boss_radius),
                    "{}: {what} at {point} is not walkable",
                    room.name
                );
            }
        }
    }
}
