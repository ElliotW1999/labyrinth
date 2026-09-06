//! Fog of war: team vision, tree line-of-sight, and fog-free zones.

use bevy::prelude::*;

use crate::components::{Creep, Health, Obstacle, PlayerHero, Team, Tower, UnitRadius};
use crate::net::NetworkedHero;

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FogFreeZones>().add_systems(
            Update,
            (update_unit_fog_visibility, sync_healthbar_fog_visibility).chain(),
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

#[derive(Clone, Copy)]
struct VisionSource {
    pos: Vec3,
    range: f32,
}

fn update_unit_fog_visibility(
    fog_free: Res<FogFreeZones>,
    local: Query<&Team, With<PlayerHero>>,
    sources: Query<
        (&Transform, &Team, Option<&UnitRadius>, Has<Creep>, Has<Tower>, Has<NetworkedHero>, Has<PlayerHero>),
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
        // No local hero yet (select screen) — show everyone.
        for (_, _, _, mut vis, _, _, _) in &mut units {
            *vis = Visibility::Visible;
        }
        return;
    };

    let tree_snaps: Vec<(Vec3, f32)> = trees
        .iter()
        .map(|(tf, obs)| (tf.translation, obs.radius))
        .collect();

    let mut vision: Vec<VisionSource> = Vec::new();
    for (tf, team, radius, is_creep, is_tower, is_hero, is_player) in &sources {
        if *team != *local_team {
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

    for (_entity, tf, team, mut vis, is_player, is_net, is_creep) in &mut units {
        if !is_player && !is_net && !is_creep {
            continue;
        }
        // Allies (and self) are always visible.
        if *team == *local_team {
            *vis = Visibility::Visible;
            continue;
        }
        let pos = tf.translation;
        if fog_free.rects.iter().any(|r| r.contains(pos)) {
            *vis = Visibility::Visible;
            continue;
        }
        let revealed = vision.iter().any(|src| {
            let dist = flat_distance(src.pos, pos);
            if dist > src.range {
                return false;
            }
            !line_blocked_by_trees(src.pos, pos, &tree_snaps)
        });
        *vis = if revealed {
            Visibility::Visible
        } else {
            Visibility::Hidden
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
        // Ignore trees very close to either endpoint (standing next to a tree).
        let r = (*radius + 0.15).max(0.2);
        let acx = tree.x - ax;
        let acz = tree.z - az;
        let t = ((acx * abx + acz * abz) / ab_len_sq).clamp(0.0, 1.0);
        // Skip blockers near the viewer or target so standing in brush still works.
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
