//! Health bars floating above living units, plus screen-space hero nameplates.

use bevy::prelude::*;

use crate::camera::GameCamera;
use crate::components::{
    HasHealthBar, HasWorldLabel, Health, HealthBar, HealthBarFill, SelectionBounds,
    WorldHeroNameLabel, WorldNameLayer, WorldTag,
};
use crate::heroes::HeroKind;
use crate::resources::SharedAssets;
use crate::scale;

pub struct HealthBarPlugin;

impl Plugin for HealthBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                attach_health_bars,
                attach_world_labels,
                sync_health_bars,
                sync_world_hero_names,
                cull_orphan_health_bars,
                cull_orphan_world_names,
            )
                .chain(),
        );
    }
}

fn attach_health_bars(
    mut commands: Commands,
    assets: Res<SharedAssets>,
    units: Query<(Entity, Option<&SelectionBounds>), (With<Health>, Without<HasHealthBar>)>,
) {
    for (entity, selection) in &units {
        let width = bar_width(selection);

        commands
            .spawn((
                Name::new("HealthBar"),
                HealthBar { owner: entity },
                Transform::default(),
                Visibility::default(),
            ))
            .with_children(|parent| {
                // Mesh is 1×1 unit; scale X to the desired world width.
                parent.spawn((
                    Mesh3d(assets.health_bar_bg_mesh.clone()),
                    MeshMaterial3d(assets.health_bar_bg_mat.clone()),
                    Transform::from_xyz(0.0, 0.0, -0.02).with_scale(Vec3::new(width, 1.0, 1.0)),
                ));
                parent.spawn((
                    HealthBarFill,
                    Mesh3d(assets.health_bar_fill_mesh.clone()),
                    MeshMaterial3d(assets.health_bar_fill_mat.clone()),
                    Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::new(width, 1.0, 1.0)),
                ));
            });

        commands.entity(entity).insert(HasHealthBar);
    }
}

/// Name labels: heroes show their name, anything with a [`WorldTag`] (Captain, Boss, Story
/// exits) shows the tag. Independent of health bars so health-less entities can be labelled.
#[allow(clippy::type_complexity)]
fn attach_world_labels(
    mut commands: Commands,
    owners: Query<
        (Entity, Option<&HeroKind>, Option<&WorldTag>),
        (Or<(With<HeroKind>, With<WorldTag>)>, Without<HasWorldLabel>),
    >,
    layer: Query<Entity, With<WorldNameLayer>>,
) {
    let Ok(layer_entity) = layer.single() else {
        return;
    };
    for (entity, hero_kind, tag) in &owners {
        let text = label_text(hero_kind, tag);
        commands.entity(layer_entity).with_children(|parent| {
            parent.spawn((
                Name::new(format!("WorldName:{text}")),
                WorldHeroNameLabel { owner: entity },
                Text::new(text),
                TextFont::from_font_size(14.0),
                TextColor(Color::srgb(0.95, 0.96, 1.0)),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(-100.0),
                    top: px(-100.0),
                    padding: UiRect::axes(px(6), px(2)),
                    border_radius: BorderRadius::all(px(3)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.05, 0.07, 0.1, 0.72)),
                Visibility::Hidden,
                ZIndex(5),
            ));
        });
        commands.entity(entity).insert(HasWorldLabel);
    }
}

/// A [`WorldTag`] wins over the hero name.
fn label_text(hero_kind: Option<&HeroKind>, tag: Option<&WorldTag>) -> String {
    match (tag, hero_kind) {
        (Some(tag), _) => tag.0.clone(),
        (None, Some(kind)) => kind.0.name().to_string(),
        (None, None) => String::new(),
    }
}

fn sync_health_bars(
    camera: Query<&GlobalTransform, With<GameCamera>>,
    owners: Query<(&Health, &GlobalTransform, Option<&SelectionBounds>), Without<HealthBar>>,
    mut bars: Query<(&HealthBar, &mut Transform, Option<&Children>), With<HealthBar>>,
    mut fills: Query<&mut Transform, (With<HealthBarFill>, Without<HealthBar>)>,
    mut backgrounds: Query<
        &mut Transform,
        (Without<HealthBarFill>, Without<HealthBar>, With<Mesh3d>),
    >,
) {
    // Face the camera's view plane so the bar stays screen-aligned as the camera moves.
    let camera_rotation = camera
        .single()
        .map(|camera| camera.rotation())
        .unwrap_or(Quat::IDENTITY);
    let mut fill_updates: Vec<(Entity, f32, f32)> = Vec::new();
    let mut bg_updates: Vec<(Entity, f32)> = Vec::new();

    for (bar, mut bar_tf, children) in &mut bars {
        let Ok((health, owner_gt, selection)) = owners.get(bar.owner) else {
            continue;
        };

        let height = bar_height(selection);
        let width = bar_width(selection);
        let owner_pos = owner_gt.translation();
        bar_tf.translation = owner_pos + Vec3::Y * height;
        bar_tf.rotation = camera_rotation;

        let fraction = if health.max <= 0.0 {
            0.0
        } else {
            (health.current / health.max).clamp(0.0, 1.0)
        };

        let Some(children) = children else {
            continue;
        };
        for (i, child) in children.iter().enumerate() {
            if i == 0 {
                bg_updates.push((child, width));
            } else {
                fill_updates.push((child, width, fraction));
            }
        }
    }

    for (child, width) in bg_updates {
        if let Ok(mut bg_tf) = backgrounds.get_mut(child) {
            bg_tf.scale = Vec3::new(width, 1.0, 1.0);
            bg_tf.translation.x = 0.0;
        }
    }

    for (child, width, fraction) in fill_updates {
        if let Ok(mut fill_tf) = fills.get_mut(child) {
            fill_tf.scale = Vec3::new(width * fraction, 1.0, 1.0);
            fill_tf.translation.x = -0.5 * width * (1.0 - fraction);
        }
    }
}

#[allow(clippy::type_complexity)]
fn sync_world_hero_names(
    camera: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    owners: Query<
        (
            &GlobalTransform,
            Option<&HeroKind>,
            Option<&WorldTag>,
            Option<&SelectionBounds>,
            &Visibility,
        ),
        Without<WorldHeroNameLabel>,
    >,
    mut labels: Query<(&WorldHeroNameLabel, &mut Node, &mut Visibility, &mut Text)>,
) {
    let Ok((camera, cam_gt)) = camera.single() else {
        return;
    };

    for (label, mut node, mut vis, mut text) in &mut labels {
        let Ok((owner_gt, kind, tag, selection, owner_vis)) = owners.get(label.owner) else {
            *vis = Visibility::Hidden;
            continue;
        };
        if matches!(*owner_vis, Visibility::Hidden) {
            *vis = Visibility::Hidden;
            continue;
        }

        let world_pos = owner_gt.translation()
            + Vec3::Y * (bar_height(selection) + scale::HEALTH_BAR_BG_THICKNESS);
        let Ok(screen) = camera.world_to_viewport(cam_gt, world_pos) else {
            *vis = Visibility::Hidden;
            continue;
        };

        let wanted = label_text(kind, tag);
        // Approximate half-width so the label sits centered above the bar.
        let half_width = if tag.is_some() {
            wanted.chars().count() as f32 * 4.0 + 6.0
        } else {
            36.0
        };
        *text = Text::new(wanted);
        node.left = px(screen.x - half_width);
        node.top = px(screen.y - 16.0);
        *vis = Visibility::Visible;
    }
}

fn cull_orphan_health_bars(
    mut commands: Commands,
    bars: Query<(Entity, &HealthBar)>,
    owners: Query<Entity, With<Health>>,
) {
    for (entity, bar) in &bars {
        if owners.get(bar.owner).is_err() {
            commands.entity(entity).despawn();
        }
    }
}

fn cull_orphan_world_names(
    mut commands: Commands,
    labels: Query<(Entity, &WorldHeroNameLabel)>,
    owners: Query<Entity, Or<(With<HeroKind>, With<WorldTag>)>>,
) {
    for (entity, label) in &labels {
        if owners.get(label.owner).is_err() {
            commands.entity(entity).despawn();
        }
    }
}

/// Bar width tracks the unit's selection footprint.
fn bar_width(selection: Option<&SelectionBounds>) -> f32 {
    let half = selection.map_or(scale::HERO_MODEL_WIDTH * 0.5, |s| s.half_extents.x);
    (half * 2.2).clamp(
        scale::CREEP_MODEL_WIDTH * 0.8,
        scale::TOWER_MODEL_WIDTH * 1.6,
    )
}

/// Offset from unit origin to just above the selection volume's top.
fn bar_height(selection: Option<&SelectionBounds>) -> f32 {
    let top = selection.map_or(scale::HERO_MODEL_HEIGHT * 0.5, SelectionBounds::top);
    top + scale::HEALTH_BAR_BG_THICKNESS * 0.75
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::IntoSystem;

    #[test]
    fn sync_health_bars_system_initializes() {
        let mut world = World::new();
        let mut system = IntoSystem::into_system(sync_health_bars);
        system.initialize(&mut world);
    }

    #[test]
    fn sync_world_hero_names_system_initializes() {
        let mut world = World::new();
        let mut system = IntoSystem::into_system(sync_world_hero_names);
        system.initialize(&mut world);
    }

    #[test]
    fn attach_world_labels_system_initializes() {
        let mut world = World::new();
        let mut system = IntoSystem::into_system(attach_world_labels);
        system.initialize(&mut world);
    }

    #[test]
    fn world_tags_label_non_hero_owners_and_win_over_hero_names() {
        let mut world = World::new();
        let layer = world.spawn(WorldNameLayer).id();
        let captain = world.spawn(WorldTag("Captain".to_string())).id();
        let hero = world.spawn(HeroKind(crate::heroes::HeroId::Vanguard)).id();
        let plain = world.spawn_empty().id();
        let mut system = IntoSystem::into_system(attach_world_labels);
        system.initialize(&mut world);
        let _ = system.run((), &mut world);
        system.apply_deferred(&mut world);

        let mut labels = world.query::<(&WorldHeroNameLabel, &Text, &ChildOf)>();
        let mut found: Vec<(Entity, String)> = labels
            .iter(&world)
            .map(|(label, text, parent)| {
                assert_eq!(parent.parent(), layer);
                (label.owner, text.0.clone())
            })
            .collect();
        found.sort();
        assert_eq!(found.len(), 2, "the unlabelled entity gets nothing");
        assert!(found.contains(&(captain, "Captain".to_string())));
        assert!(found.contains(&(hero, crate::heroes::HeroId::Vanguard.name().to_string())));
        assert!(world.get::<HasWorldLabel>(plain).is_none());
        assert!(world.get::<HasWorldLabel>(captain).is_some());
        assert_eq!(
            label_text(
                Some(&HeroKind(crate::heroes::HeroId::Vanguard)),
                Some(&WorldTag("Boss".to_string()))
            ),
            "Boss"
        );
    }

    #[test]
    fn attach_health_bars_system_initializes() {
        let mut world = World::new();
        world.init_resource::<SharedAssets>();
        let mut system = IntoSystem::into_system(attach_health_bars);
        system.initialize(&mut world);
    }

    #[test]
    fn health_bars_stay_orthogonal_to_the_camera() {
        let mut world = World::new();
        let rotation = Quat::from_rotation_x(-0.9);
        world.spawn((GameCamera, GlobalTransform::from_rotation(rotation)));
        let owner = world
            .spawn((
                Health::new(100.0),
                GlobalTransform::from_translation(Vec3::new(10.0, 0.0, -4.0)),
            ))
            .id();
        let bar = world
            .spawn((HealthBar { owner }, Transform::default()))
            .id();

        let mut system = IntoSystem::into_system(sync_health_bars);
        system.initialize(&mut world);
        let _ = system.run((), &mut world);

        let transform = world.get::<Transform>(bar).unwrap();
        let delta = transform.rotation.dot(rotation).abs();
        assert!(
            (delta - 1.0).abs() < 1e-4,
            "bar should match the camera orientation"
        );
        assert!(transform.translation.y > 0.0);
    }
}
