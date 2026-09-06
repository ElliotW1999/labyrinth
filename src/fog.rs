//! Fog of war: team vision, tree line-of-sight, fog-free zones, and a grey overlay.

use bevy::prelude::*;

use crate::components::{Creep, Health, Obstacle, PlayerHero, Team, Tower, UnitRadius};
use crate::net::NetworkedHero;
use crate::resources::MatchConfig;

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FogFreeZones>()
            .init_resource::<FogOverlayAssets>()
            .add_systems(
                Startup,
                spawn_fog_overlay.after(crate::resources::load_shared_assets),
            )
            .add_systems(
                Update,
                (
                    update_unit_fog_visibility,
                    sync_healthbar_fog_visibility,
                    update_fog_overlay_tiles,
                )
                    .chain(),
            );
    }
}

/// World-space AABB (XZ) where fog of war does not apply.
#[derive(Resource, Debug, Default, Clone)]
pub struct FogFreeZones {
    pub rects: Vec<FogRect>,
}

#[derive(Debug, Clone, Copy)]
pub struct FogRect {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
}

impl FogRect {
    pub fn contains(self, pos: Vec3) -> bool {
        pos.x >= self.min_x && pos.x <= self.max_x && pos.z >= self.min_z && pos.z <= self.max_z
    }
}

/// Generous hero / tower day vision.
pub const HERO_VISION_RANGE: f32 = 26.0;
pub const TOWER_VISION_RANGE: f32 = 26.0;
/// Creep vision is half of hero vision.
pub const CREEP_VISION_RANGE: f32 = HERO_VISION_RANGE * 0.5;

const FOG_TILE_SIZE: f32 = 8.0;
/// Slightly grey, mostly transparent fog veil.
const FOG_ALPHA: f32 = 0.38;

#[derive(Resource, Default)]
struct FogOverlayAssets {
    fog_mat: Handle<StandardMaterial>,
    clear_mat: Handle<StandardMaterial>,
    mesh: Handle<Mesh>,
}

#[derive(Component, Debug, Clone, Copy)]
struct FogTile {
    center: Vec3,
}

#[derive(Clone, Copy)]
struct VisionSource {
    pos: Vec3,
    range: f32,
}

fn spawn_fog_overlay(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut fog_assets: ResMut<FogOverlayAssets>,
    config: Res<MatchConfig>,
) {
    fog_assets.mesh = meshes.add(Plane3d::default().mesh().size(FOG_TILE_SIZE, FOG_TILE_SIZE));
    fog_assets.fog_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.55, 0.55, 0.58, FOG_ALPHA),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        perceptual_roughness: 1.0,
        reflectance: 0.0,
        ..default()
    });
    fog_assets.clear_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.55, 0.55, 0.58, 0.0),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        perceptual_roughness: 1.0,
        reflectance: 0.0,
        ..default()
    });

    let half = config.map_half_extent;
    let mut x = -half;
    while x < half - 0.01 {
        let mut z = -half;
        while z < half - 0.01 {
            let cx = x + FOG_TILE_SIZE * 0.5;
            let cz = z + FOG_TILE_SIZE * 0.5;
            if cx.abs() <= half && cz.abs() <= half {
                commands.spawn((
                    Name::new("Fog Tile"),
                    FogTile {
                        center: Vec3::new(cx, 0.0, cz),
                    },
                    Mesh3d(fog_assets.mesh.clone()),
                    MeshMaterial3d(fog_assets.fog_mat.clone()),
                    Transform::from_xyz(cx, 0.55, cz),
                ));
            }
            z += FOG_TILE_SIZE;
        }
        x += FOG_TILE_SIZE;
    }
}

fn collect_vision(
    local_team: Team,
    sources: &Query<
        (
            &Transform,
            &Team,
            Option<&UnitRadius>,
            Has<Creep>,
            Has<Tower>,
            Has<NetworkedHero>,
            Has<PlayerHero>,
        ),
        Or<(With<PlayerHero>, With<NetworkedHero>, With<Creep>, With<Tower>)>,
    >,
) -> Vec<VisionSource> {
    let mut vision = Vec::new();
    for (tf, team, radius, is_creep, is_tower, is_hero, is_player) in sources.iter() {
        if *team != local_team {
            continue;
        }
        let range = if is_tower {
            TOWER_VISION_RANGE
        } else if is_creep {
            CREEP_VISION_RANGE
        } else if is_hero || is_player {
            HERO_VISION_RANGE
        } else {
            HERO_VISION_RANGE
        };
        let pad = radius.map(|r| r.0).unwrap_or(0.0);
        vision.push(VisionSource {
            pos: tf.translation,
            range: range + pad,
        });
    }
    vision
}

fn point_revealed(
    pos: Vec3,
    vision: &[VisionSource],
    fog_free: &FogFreeZones,
    trees: &[(Vec3, f32)],
) -> bool {
    if fog_free.rects.iter().any(|r| r.contains(pos)) {
        return true;
    }
    vision.iter().any(|src| {
        let dist = flat_distance(src.pos, pos);
        if dist > src.range {
            return false;
        }
        !line_blocked_by_trees(src.pos, pos, trees)
    })
}

fn update_unit_fog_visibility(
    fog_free: Res<FogFreeZones>,
    local: Query<&Team, With<PlayerHero>>,
    sources: Query<
        (
            &Transform,
            &Team,
            Option<&UnitRadius>,
            Has<Creep>,
            Has<Tower>,
            Has<NetworkedHero>,
            Has<PlayerHero>,
        ),
        Or<(With<PlayerHero>, With<NetworkedHero>, With<Creep>, With<Tower>)>,
    >,
    trees: Query<(&Transform, &Obstacle)>,
    mut units: Query<
        (
            Entity,
            &Transform,
            &Team,
            &mut Visibility,
            Has<PlayerHero>,
            Has<NetworkedHero>,
            Has<Creep>,
        ),
        Or<(With<PlayerHero>, With<NetworkedHero>, With<Creep>)>,
    >,
) {
    let Ok(local_team) = local.single() else {
        for (_, _, _, mut vis, _, _, _) in &mut units {
            *vis = Visibility::Visible;
        }
        return;
    };

    let tree_snaps: Vec<(Vec3, f32)> = trees
        .iter()
        .map(|(tf, obs)| (tf.translation, obs.radius))
        .collect();
    let vision = collect_vision(*local_team, &sources);

    for (_entity, tf, team, mut vis, is_player, is_net, is_creep) in &mut units {
        if !is_player && !is_net && !is_creep {
            continue;
        }
        if *team == *local_team {
            *vis = Visibility::Visible;
            continue;
        }
        let revealed = point_revealed(tf.translation, &vision, &fog_free, &tree_snaps);
        *vis = if revealed {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn update_fog_overlay_tiles(
    fog_free: Res<FogFreeZones>,
    fog_assets: Res<FogOverlayAssets>,
    local: Query<&Team, With<PlayerHero>>,
    sources: Query<
        (
            &Transform,
            &Team,
            Option<&UnitRadius>,
            Has<Creep>,
            Has<Tower>,
            Has<NetworkedHero>,
            Has<PlayerHero>,
        ),
        Or<(With<PlayerHero>, With<NetworkedHero>, With<Creep>, With<Tower>)>,
    >,
    trees: Query<(&Transform, &Obstacle)>,
    mut tiles: Query<(&FogTile, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let tree_snaps: Vec<(Vec3, f32)> = trees
        .iter()
        .map(|(tf, obs)| (tf.translation, obs.radius))
        .collect();

    let vision = if let Ok(local_team) = local.single() {
        collect_vision(*local_team, &sources)
    } else {
        Vec::new()
    };
    // Before hero spawn, clear the overlay so the select screen isn't veiled.
    let force_clear = vision.is_empty();

    for (tile, mut mat) in &mut tiles {
        let revealed =
            force_clear || point_revealed(tile.center, &vision, &fog_free, &tree_snaps);
        mat.0 = if revealed {
            fog_assets.clear_mat.clone()
        } else {
            fog_assets.fog_mat.clone()
        };
    }
}

fn sync_healthbar_fog_visibility(
    owners: Query<&Visibility, (With<Health>, Without<crate::components::HealthBar>)>,
    mut bars: Query<
        (&crate::components::HealthBar, &mut Visibility),
        With<crate::components::HealthBar>,
    >,
) {
    for (bar, mut vis) in &mut bars {
        let Ok(owner_vis) = owners.get(bar.owner) else {
            continue;
        };
        *vis = match *owner_vis {
            Visibility::Hidden => Visibility::Hidden,
            Visibility::Visible | Visibility::Inherited => Visibility::Visible,
        };
    }
}

fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

/// True if the XZ segment from `from` to `to` clips a tree circle.
pub fn line_blocked_by_trees(from: Vec3, to: Vec3, trees: &[(Vec3, f32)]) -> bool {
    let ax = from.x;
    let az = from.z;
    let bx = to.x;
    let bz = to.z;
    let abx = bx - ax;
    let abz = bz - az;
    let ab_len_sq = abx * abx + abz * abz;
    if ab_len_sq < 1e-8 {
        return false;
    }
    for (tree, radius) in trees {
        let r = (*radius + 0.15).max(0.2);
        let acx = tree.x - ax;
        let acz = tree.z - az;
        let t = ((acx * abx + acz * abz) / ab_len_sq).clamp(0.0, 1.0);
        if t < 0.05 || t > 0.95 {
            continue;
        }
        let px = ax + abx * t;
        let pz = az + abz * t;
        let dx = tree.x - px;
        let dz = tree.z - pz;
        if dx * dx + dz * dz <= r * r {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_blocks_mid_segment() {
        let from = Vec3::new(0.0, 0.0, 0.0);
        let to = Vec3::new(10.0, 0.0, 0.0);
        let trees = [(Vec3::new(5.0, 1.0, 0.0), 1.0)];
        assert!(line_blocked_by_trees(from, to, &trees));
    }

    #[test]
    fn open_line_is_clear() {
        let from = Vec3::new(0.0, 0.0, 0.0);
        let to = Vec3::new(10.0, 0.0, 0.0);
        let trees = [(Vec3::new(5.0, 1.0, 4.0), 1.0)];
        assert!(!line_blocked_by_trees(from, to, &trees));
    }
}
