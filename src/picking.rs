//! Cursor picking against unit [`SelectionBounds`]: hover, left-click selection,
//! and the shared cursor ray used by right-click and spell targeting.

use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;

use crate::abilities::AbilityTargeting;
use crate::components::{Ground, Health, PlayerHero, SelectionBounds, Team};
use crate::dimensions::pick_unit;
use crate::items::ShopUiState;
use crate::menu::MainMenuState;
use crate::ui::UiPointerState;

pub struct PickingPlugin;

impl Plugin for PickingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HoveredUnit>()
            .init_resource::<SelectedUnit>()
            .add_systems(
                Update,
                (update_hovered_unit, select_on_left_click, draw_pick_highlights)
                    .chain()
                    .before(crate::abilities::confirm_or_cancel_targeted_cast),
            );
    }
}

/// Unit whose selection volume is under the cursor this frame.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct HoveredUnit(pub Option<Entity>);

/// Unit last left-clicked by the local player.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct SelectedUnit(pub Option<Entity>);

/// Units that can be picked by the cursor.
pub type PickableUnits<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        &'static SelectionBounds,
        &'static Team,
        &'static Health,
        &'static Visibility,
    ),
>;

/// World-space ray from the camera through the cursor.
pub fn cursor_ray(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform)>,
) -> Option<Ray3d> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, cam_transform) = camera.single().ok()?;
    camera.viewport_to_world(cam_transform, cursor).ok()
}

/// Where a ray meets the ground plane.
pub fn ground_hit(ray: Ray3d, ground: &Query<&GlobalTransform, With<Ground>>) -> Option<Vec3> {
    let ground_tf = ground.single().ok()?;
    let distance = ray.intersect_plane(ground_tf.translation(), InfinitePlane3d::new(Dir3::Y))?;
    Some(ray.get_point(distance))
}

pub fn cursor_ground_hit(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform)>,
    ground: &Query<&GlobalTransform, With<Ground>>,
) -> Option<Vec3> {
    ground_hit(cursor_ray(windows, camera)?, ground)
}

/// Best living, visible unit under `ray` whose team passes `team_filter`.
pub fn pick_under_ray(
    ray: Ray3d,
    units: &PickableUnits,
    team_filter: impl Fn(Team) -> bool,
) -> Option<Entity> {
    pick_unit(
        ray,
        units
            .iter()
            .filter(|(_, _, _, team, hp, vis)| {
                hp.is_alive() && !matches!(**vis, Visibility::Hidden) && team_filter(**team)
            })
            .map(|(entity, gt, selection, ..)| (entity, gt.translation(), *selection)),
    )
}

fn update_hovered_unit(
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    pointer: Res<UiPointerState>,
    units: PickableUnits,
    mut hovered: ResMut<HoveredUnit>,
) {
    hovered.0 = if pointer.over_blocking_ui || pointer.over_minimap {
        None
    } else {
        cursor_ray(&windows, &camera).and_then(|ray| pick_under_ray(ray, &units, |_| true))
    };
}

fn select_on_left_click(
    mouse: Res<ButtonInput<MouseButton>>,
    menu: Res<MainMenuState>,
    shop: Res<ShopUiState>,
    pointer: Res<UiPointerState>,
    targeting: Res<AbilityTargeting>,
    hovered: Res<HoveredUnit>,
    alive: Query<&Health>,
    mut selected: ResMut<SelectedUnit>,
) {
    if selected.0.is_some_and(|e| alive.get(e).map_or(true, |hp| !hp.is_alive())) {
        selected.0 = None;
    }
    if !mouse.just_pressed(MouseButton::Left)
        || menu.open
        || shop.open
        || pointer.over_blocking_ui
        || pointer.over_minimap
        || targeting.active.is_some()
    {
        return;
    }
    // Clicking empty ground keeps the current selection, as in most MOBAs.
    if let Some(unit) = hovered.0 {
        selected.0 = Some(unit);
    }
}

fn draw_pick_highlights(
    hovered: Res<HoveredUnit>,
    selected: Res<SelectedUnit>,
    local: Query<&Team, With<PlayerHero>>,
    units: Query<(&GlobalTransform, &SelectionBounds, &Team)>,
    mut gizmos: Gizmos,
) {
    let local_team = local.single().ok().copied();
    let ring = |gizmos: &mut Gizmos, entity: Entity, scale: f32, color: Color| {
        let Ok((gt, selection, _)) = units.get(entity) else {
            return;
        };
        let pos = gt.translation();
        let iso = Isometry3d::new(Vec3::new(pos.x, 4.0, pos.z), Quat::from_rotation_x(FRAC_PI_2));
        gizmos.circle(iso, selection.half_extents.x * scale, color);
    };

    if let Some(entity) = selected.0 {
        let friendly = units
            .get(entity)
            .is_ok_and(|(_, _, team)| Some(*team) == local_team);
        let color = if friendly {
            Color::srgb(0.3, 1.0, 0.4)
        } else {
            Color::srgb(1.0, 0.3, 0.25)
        };
        ring(&mut gizmos, entity, 1.0, color);
        ring(&mut gizmos, entity, 0.96, color);
    }
    if let Some(entity) = hovered.0.filter(|e| Some(*e) != selected.0) {
        ring(&mut gizmos, entity, 1.0, Color::srgba(1.0, 1.0, 1.0, 0.8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::IntoSystem;

    #[test]
    fn picking_systems_initialize() {
        let mut world = World::new();
        let mut hover = IntoSystem::into_system(update_hovered_unit);
        hover.initialize(&mut world);
        let mut select = IntoSystem::into_system(select_on_left_click);
        select.initialize(&mut world);
    }
}
