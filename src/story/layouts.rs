//! Room geometry: floor, walls and pillars, plus the authored entry, exit pad and shop
//! positions of each layout. Rooms are built centred on the world origin, well inside the
//! map extent, so `NavGrid` needs no change: it rebuilds from the room's `Obstacle`s.

use bevy::prelude::*;

use crate::components::{Ground, Obstacle};
use crate::fog::FogRect;
use crate::resources::SharedAssets;

/// Which geometry a room uses. `scripts/RoomGenerator.py` reads the variants between the
/// `room_layouts` markers to validate the `layout` column of `data/rooms.csv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoomLayoutId {
    // <room_layouts>
    Camp,
    Arena,
    Pillars,
    Large,
    // </room_layouts>
}

/// Half thickness of the four boundary walls (they sit outside the playable rectangle).
pub const WALL_HALF_THICKNESS: f32 = 100.0;
/// Visual height of walls and pillars.
pub const WALL_HEIGHT: f32 = 320.0;
/// How far past the walls the fog-free rectangle reaches.
const FOG_MARGIN: f32 = 400.0;

/// Where the hero stands in the safe camp.
pub const CAMP_ENTRY: Vec2 = Vec2::new(0.0, -700.0);

/// Authored geometry of one layout. All positions are world XZ (the room is centred on the
/// origin) and are checked walkable by a test.
#[derive(Debug, Clone, Copy)]
pub struct RoomLayout {
    /// Half extents of the walkable rectangle.
    pub half: Vec2,
    /// Where heroes appear on entering (and respawn).
    pub entry: Vec2,
    /// Centres of the two exit pads.
    pub exits: [Vec2; 2],
    /// Where the item shop stands (rooms with `shop`).
    pub shop: Vec2,
    /// Pillars: (centre, half extents).
    pub pillars: &'static [(Vec2, Vec2)],
}

const CAMP: RoomLayout = RoomLayout {
    half: Vec2::new(1500.0, 1000.0),
    entry: CAMP_ENTRY,
    exits: [Vec2::new(-700.0, 700.0), Vec2::new(700.0, 700.0)],
    shop: Vec2::new(0.0, -200.0),
    pillars: &[],
};

const ARENA: RoomLayout = RoomLayout {
    half: Vec2::new(1800.0, 1300.0),
    entry: Vec2::new(0.0, 1000.0),
    exits: [Vec2::new(-900.0, 1000.0), Vec2::new(900.0, 1000.0)],
    shop: Vec2::new(0.0, 0.0),
    pillars: &[],
};

const PILLARS: RoomLayout = RoomLayout {
    half: Vec2::new(1800.0, 1300.0),
    entry: Vec2::new(0.0, 1000.0),
    exits: [Vec2::new(-900.0, 1000.0), Vec2::new(900.0, 1000.0)],
    shop: Vec2::new(0.0, 0.0),
    pillars: &[
        (Vec2::new(-700.0, 100.0), Vec2::new(110.0, 110.0)),
        (Vec2::new(700.0, 100.0), Vec2::new(110.0, 110.0)),
        (Vec2::new(0.0, -250.0), Vec2::new(110.0, 110.0)),
    ],
};

const LARGE: RoomLayout = RoomLayout {
    half: Vec2::new(2200.0, 1600.0),
    entry: Vec2::new(0.0, 1300.0),
    exits: [Vec2::new(-1000.0, 1300.0), Vec2::new(1000.0, 1300.0)],
    shop: Vec2::new(0.0, 0.0),
    pillars: &[
        (Vec2::new(-900.0, -100.0), Vec2::new(150.0, 150.0)),
        (Vec2::new(900.0, -100.0), Vec2::new(150.0, 150.0)),
    ],
};

impl RoomLayoutId {
    pub fn layout(self) -> &'static RoomLayout {
        match self {
            RoomLayoutId::Camp => &CAMP,
            RoomLayoutId::Arena => &ARENA,
            RoomLayoutId::Pillars => &PILLARS,
            RoomLayoutId::Large => &LARGE,
        }
    }

    #[cfg(test)]
    pub fn all() -> [RoomLayoutId; 4] {
        [
            RoomLayoutId::Camp,
            RoomLayoutId::Arena,
            RoomLayoutId::Pillars,
            RoomLayoutId::Large,
        ]
    }
}

impl RoomLayout {
    /// Every static blocker of the room as (centre, half extents): the four boundary walls,
    /// then the pillars. The spawner and the walkability test both read this.
    pub fn obstacles(&self) -> Vec<(Vec2, Vec2)> {
        let t = WALL_HALF_THICKNESS;
        let mut shapes = vec![
            // North and south walls span the corners; east and west fit between them.
            (
                Vec2::new(0.0, self.half.y + t),
                Vec2::new(self.half.x + 2.0 * t, t),
            ),
            (
                Vec2::new(0.0, -self.half.y - t),
                Vec2::new(self.half.x + 2.0 * t, t),
            ),
            (Vec2::new(self.half.x + t, 0.0), Vec2::new(t, self.half.y)),
            (Vec2::new(-self.half.x - t, 0.0), Vec2::new(t, self.half.y)),
        ];
        shapes.extend_from_slice(self.pillars);
        shapes
    }

    /// The fog-free rectangle covering the room: rooms are always fully visible.
    pub fn fog_rect(&self) -> FogRect {
        FogRect {
            min_x: -self.half.x - FOG_MARGIN,
            max_x: self.half.x + FOG_MARGIN,
            min_z: -self.half.y - FOG_MARGIN,
            max_z: self.half.y + FOG_MARGIN,
        }
    }
}

/// Materials the room geometry needs, created once by the story plugin.
pub struct LayoutMaterials {
    pub wall: Handle<StandardMaterial>,
}

/// Spawn the floor (the picking [`Ground`]), the boundary walls, the pillars and the light
/// of a room. Every entity is a root `Transform` entity, so `teardown_room` removes it.
pub fn spawn_layout(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    assets: &SharedAssets,
    materials: &LayoutMaterials,
    layout: &RoomLayout,
) {
    commands.spawn((
        Name::new("Room Floor"),
        Mesh3d(
            meshes.add(
                Plane3d::default()
                    .mesh()
                    .size(layout.half.x * 2.0, layout.half.y * 2.0),
            ),
        ),
        MeshMaterial3d(assets.ground_mat.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
        Ground,
    ));

    for (index, (center, half)) in layout.obstacles().into_iter().enumerate() {
        let name = if index < 4 {
            format!("Room Wall {index}")
        } else {
            format!("Room Pillar {}", index - 4)
        };
        commands.spawn((
            Name::new(name),
            Mesh3d(meshes.add(Cuboid::new(half.x * 2.0, WALL_HEIGHT, half.y * 2.0))),
            MeshMaterial3d(materials.wall.clone()),
            Transform::from_xyz(center.x, WALL_HEIGHT * 0.5, center.y),
            Obstacle::aabb(half.x, half.y),
        ));
    }

    commands.spawn((
        Name::new("Room Light"),
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(1200.0, 3000.0, 800.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.7, 0.75, 0.85),
        brightness: 120.0,
        ..default()
    });
}

/// A navigation grid holding exactly the room's static shapes (tests).
#[cfg(test)]
pub(crate) fn grid_of(layout: &RoomLayout) -> crate::navigation::NavGrid {
    use crate::navigation::{NavGrid, NavShape};
    let mut shapes: Vec<NavShape> = layout
        .obstacles()
        .into_iter()
        .map(|(center, half)| NavShape::Aabb { center, half })
        .collect();
    crate::navigation::grid::sort_shapes(&mut shapes);
    let mut grid = NavGrid::default();
    grid.rebuild(shapes);
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camp_entry_matches_the_shared_constant() {
        assert_eq!(CAMP.entry, CAMP_ENTRY);
        assert_eq!(super::super::SAFE_ENTRY.x, CAMP_ENTRY.x);
        assert_eq!(super::super::SAFE_ENTRY.z, CAMP_ENTRY.y);
    }

    #[test]
    fn walls_enclose_the_walkable_rectangle() {
        for id in RoomLayoutId::all() {
            let layout = id.layout();
            let grid = grid_of(layout);
            let r = 68.4; // the Boss's collision radius
            let reach = r + 2.0; // just off the wall face
            for corner in [
                Vec2::new(layout.half.x - reach, layout.half.y - reach),
                Vec2::new(-layout.half.x + reach, layout.half.y - reach),
                Vec2::new(layout.half.x - reach, -layout.half.y + reach),
                Vec2::new(-layout.half.x + reach, -layout.half.y + reach),
            ] {
                if layout
                    .pillars
                    .iter()
                    .all(|(c, h)| (corner - *c).abs().cmpgt(*h + Vec2::splat(r)).any())
                {
                    assert!(grid.point_clear(corner, r), "{id:?} corner {corner}");
                }
            }
            assert!(
                !grid.point_clear(Vec2::new(layout.half.x + 50.0, 0.0), 10.0),
                "{id:?}: beyond the east wall is blocked"
            );
            assert!(
                !grid.point_clear(Vec2::new(0.0, -layout.half.y - 50.0), 10.0),
                "{id:?}: beyond the south wall is blocked"
            );
        }
    }
}
