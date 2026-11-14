//! Story Mode: a room-based dungeon crawl built on the shared match lifecycle.
//!
//! One room exists at a time. Clearing it (or, in the safe camp, just arriving) arms two exit
//! pads; stepping on one swaps the room in place: the old room's entities are despawned in
//! `Last` (like `map::reset_match`, minus the heroes) and the next room is built the
//! following frame. Heroes persist, are moved to the new entry and keep their gold, level,
//! items and lives.
//!
//! Story rules sit outside the six gameplay layers (see `docs/architecture/composition.md`):
//! they are match-level rules that only emit through existing APIs. `map`, `items`,
//! `units` and `ai` never import this module.
//!
//! State machine of [`StoryRun::phase`]:
//!
//! ```text
//! Idle -> Loading(safe) -> Cleared (safe room: exits armed, no completion event)
//! Loading(room) -> Active (KillAll) -> Cleared (RoomCompletedEvent, once) -> exit pad dwell
//!   -> Transition(next) -> (teardown in Last) -> Loading(next)
//! Cleared at FINAL_DEPTH -> Victory;  last life lost -> GameOver
//! ```

pub mod layouts;
pub mod lives;
pub mod pickups;
mod room_catalog;
pub mod rooms;

use bevy::prelude::*;

use crate::components::{
    CollisionRadius, CombatStats, Downed, Health, HeroAttributes, Mana, PlayerWallet, Team,
    WorldTag,
};
use crate::dimensions::{collision_of, rect_touches_circle};
use crate::fog::FogFreeZones;
use crate::items::{
    BOTTLE_SLOT, Inventory, ItemId, ItemSlot, SHOP_SIZE, refill_charges, shop_material,
    shop_world_position, spawn_item_shop,
};
use crate::map::{self, MapGenSet};
use crate::movement::SimSet;
use crate::net::{NetMode, NetworkedHero};
use crate::resources::SharedAssets;
use crate::scale;
use crate::unit_commands::CommandQueue;
use crate::units::{CreepSpawn, spawn_creep_with};
use layouts::{LayoutMaterials, spawn_layout};
use lives::{HeroLives, clear_orders};
use pickups::spawn_item_pickup;
use rooms::{FINAL_DEPTH, RoomGoal, RoomId, RoomReward, choose_exits, safe_room, spawn_points};

/// Where the local hero stands in the safe starting room. Used by hero spawn so the hero
/// never appears at the classic base when a Story match begins.
pub const SAFE_ENTRY: Vec3 = Vec3::new(
    layouts::CAMP_ENTRY.x,
    scale::body(0.9),
    layouts::CAMP_ENTRY.y,
);

/// Lives every hero starts a run with. Each death uses one; the death that uses the last
/// ends the run (so three respawns).
pub const STORY_STARTING_LIVES: u32 = 4;
/// Seconds a dead hero waits before respawning at the room entry.
pub const STORY_RESPAWN_SECS: f32 = 5.0;
/// Seconds a hero must stand on an armed exit pad to take it.
pub const EXIT_DWELL_SECS: f32 = 0.5;
/// Half extents of an exit pad.
const EXIT_PAD_HALF: Vec2 = Vec2::splat(160.0);

/// Systems of a Story match, ordered around the shared simulation.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum StorySet {
    /// Gear, room construction and hero placement. Before unit commands and AI.
    Spawn,
    /// Deaths, respawns, completion, rewards, exits and pickups. After damage resolves and
    /// before dead units are despawned.
    Rules,
}

pub struct StoryPlugin;

impl Plugin for StoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StoryRun>()
            .init_resource::<StorySeed>()
            .init_resource::<StoryVisuals>()
            .add_message::<RoomEnteredEvent>()
            .add_message::<RoomCompletedEvent>()
            .add_message::<StoryRunEndedEvent>()
            .configure_sets(
                Update,
                (
                    StorySet::Spawn.after(MapGenSet).before(SimSet::Commands),
                    StorySet::Rules
                        .after(crate::combat::apply_damage_events)
                        .before(crate::combat::despawn_dead),
                ),
            )
            .add_systems(
                Update,
                (
                    start_story_run
                        .in_set(MapGenSet)
                        .run_if(map::map_requested)
                        .run_if(map::story_mode)
                        .run_if(crate::net::is_sim_authority),
                    (grant_story_gear, spawn_pending_room, place_heroes_at_entry)
                        .chain()
                        .in_set(StorySet::Spawn)
                        .run_if(map::story_mode)
                        .run_if(crate::net::is_sim_authority),
                    (
                        lives::story_hero_deaths,
                        lives::tick_downed_heroes,
                        check_room_completion,
                        on_room_completed,
                        tick_exit_pads,
                        pickups::collect_item_pickups,
                    )
                        .chain()
                        .in_set(StorySet::Rules)
                        .run_if(map::story_mode)
                        .run_if(crate::net::is_sim_authority),
                ),
            )
            // `Last`, like `reset_match`: queued commands of every Update system have been
            // applied, so despawning the room cannot race an insert onto a removed unit.
            .add_systems(
                Last,
                teardown_room
                    .before(map::reset_match)
                    .run_if(map::story_mode)
                    .run_if(crate::net::is_sim_authority),
            );
    }
}

/// Story Mode is offline only in v1: rooms, lives and pickups are authority-owned state
/// that the UDP prototype does not replicate.
pub fn story_available(mode: NetMode) -> bool {
    mode == NetMode::Offline
}

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoryPhase {
    /// No run (classic match, or hero select).
    #[default]
    Idle,
    /// The room is about to be built by `spawn_pending_room`.
    Loading(RoomId),
    /// Hostiles alive.
    Active,
    /// Cleared (or the safe camp): the exit pads are armed.
    Cleared,
    /// A hero took an exit: the room is torn down in `Last`, then the next one loads.
    Transition(RoomId),
    GameOver,
    Victory,
}

/// The state of the current run.
#[derive(Resource, Debug, Clone)]
pub struct StoryRun {
    pub phase: StoryPhase,
    /// Rooms cleared so far counting the safe camp as 0 (the room being played has this depth).
    pub depth: u32,
    pub room: Option<RoomId>,
    /// Where heroes appear in the current room (world position, ground level).
    pub entry: Vec3,
    /// The rooms the two exit pads lead to (`None`: that pad is unused).
    pub exits: [Option<RoomId>; 2],
    pub hostiles_spawned: u32,
    /// Heroes still have to be moved to `entry`.
    pub needs_placement: bool,
    pub rng: StoryRng,
}

impl StoryRun {
    fn new(seed: u64) -> Self {
        Self {
            phase: StoryPhase::Idle,
            depth: 0,
            room: None,
            entry: SAFE_ENTRY,
            exits: [None, None],
            hostiles_spawned: 0,
            needs_placement: false,
            rng: StoryRng::new(seed),
        }
    }
}

impl Default for StoryRun {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Seed of the room draw. `None` is time-based; tests pin a value.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StorySeed(pub Option<u64>);

impl StorySeed {
    fn resolve(self) -> u64 {
        self.0.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as u64)
        })
    }
}

/// A small xorshift generator, separate from `CombatRng` so combat determinism tests are
/// not disturbed by room draws.
#[derive(Debug, Clone)]
pub struct StoryRng(u64);

impl StoryRng {
    pub fn new(seed: u64) -> Self {
        // SplitMix64 finaliser so nearby seeds diverge; xorshift needs a non-zero state.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self(z.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..n` (`n` must be above 0).
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u64() >> 32) % u64::from(n.max(1))) as u32
    }
}

/// An enemy placed by the room; the room is cleared when none lives.
#[derive(Component, Debug, Clone, Copy)]
pub struct RoomHostile;

/// A pad that takes a hero to the next room once it has stood on it for [`EXIT_DWELL_SECS`].
#[derive(Component, Debug, Clone, Copy)]
pub struct ExitPad {
    pub slot: usize,
    pub half: Vec2,
    pub dwell: f32,
    pub armed: bool,
}

/// Facts other systems (HUD, tests) can read.
#[derive(Message, Debug, Clone, Copy)]
pub struct RoomEnteredEvent {
    pub room: RoomId,
    pub depth: u32,
}

/// Written exactly once per cleared room (the safe camp never writes it).
#[derive(Message, Debug, Clone, Copy)]
pub struct RoomCompletedEvent {
    pub room: RoomId,
    pub depth: u32,
}

#[derive(Message, Debug, Clone, Copy)]
pub struct StoryRunEndedEvent {
    pub victory: bool,
}

/// Materials and meshes shared by every room, built the first time one is needed.
#[derive(Clone)]
pub struct StoryHandles {
    pub wall: Handle<StandardMaterial>,
    pub pad_idle: Handle<StandardMaterial>,
    pub pad_armed: Handle<StandardMaterial>,
    pub pickup: Handle<StandardMaterial>,
    pub pickup_mesh: Handle<Mesh>,
    pub shop: Handle<StandardMaterial>,
    pub shop_mesh: Handle<Mesh>,
}

#[derive(Resource, Default)]
pub struct StoryVisuals {
    handles: Option<StoryHandles>,
}

impl StoryVisuals {
    pub fn ensure(
        &mut self,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) -> &StoryHandles {
        self.handles.get_or_insert_with(|| StoryHandles {
            wall: materials.add(StandardMaterial {
                base_color: Color::srgb(0.28, 0.27, 0.32),
                perceptual_roughness: 0.9,
                ..default()
            }),
            pad_idle: materials.add(StandardMaterial {
                base_color: Color::srgba(0.35, 0.35, 0.4, 0.5),
                perceptual_roughness: 0.8,
                ..default()
            }),
            pad_armed: materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.85, 0.45),
                emissive: LinearRgba::rgb(0.2, 1.4, 0.5),
                perceptual_roughness: 0.5,
                ..default()
            }),
            pickup: materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.8, 0.3),
                emissive: LinearRgba::rgb(1.6, 1.1, 0.2),
                perceptual_roughness: 0.4,
                metallic: 0.4,
                ..default()
            }),
            pickup_mesh: meshes.add(Cuboid::new(90.0, 90.0, 90.0)),
            shop: materials.add(shop_material(Team::Radiant)),
            shop_mesh: meshes.add(Cuboid::new(SHOP_SIZE, SHOP_SIZE, SHOP_SIZE)),
        })
    }
}

/// Reset the run for a fresh Story match: the safe camp loads this frame.
fn start_story_run(mut run: ResMut<StoryRun>, seed: Res<StorySeed>) {
    *run = StoryRun::new(seed.resolve());
    run.phase = StoryPhase::Loading(safe_room());
}

/// Every hero starts the run with its lives and the Bottle in the bottle slot. Keyed on
/// `Without<HeroLives>` so it also covers a hero respawned by Change Hero. Classic matches
/// never run this, so slots 6 and 7 stay empty there.
#[allow(clippy::type_complexity)]
fn grant_story_gear(
    mut commands: Commands,
    mut heroes: Query<
        (
            Entity,
            &mut Inventory,
            &mut Health,
            &mut Mana,
            &mut CombatStats,
            &mut HeroAttributes,
        ),
        (With<NetworkedHero>, Without<HeroLives>),
    >,
) {
    for (entity, mut inventory, mut health, mut mana, mut stats, mut attrs) in &mut heroes {
        crate::items::place_in_slot(
            &mut inventory,
            &mut health,
            &mut mana,
            &mut stats,
            &mut attrs,
            BOTTLE_SLOT,
            bottle_item(),
        );
        commands.entity(entity).insert(HeroLives {
            remaining: STORY_STARTING_LIVES,
        });
    }
}

/// The one item with `slot=bottle`.
fn bottle_item() -> ItemId {
    ItemId::all()
        .iter()
        .copied()
        .find(|id| id.definition().slot == ItemSlot::Bottle)
        .expect("the item catalog has a bottle")
}

fn ground_point(xz: Vec2) -> Vec3 {
    Vec3::new(xz.x, 0.0, xz.y)
}

/// Build the room a [`StoryPhase::Loading`] names: floor, walls, pad pair, shop, hostiles.
#[allow(clippy::too_many_arguments)]
fn spawn_pending_room(
    mut commands: Commands,
    mut run: ResMut<StoryRun>,
    assets: Res<SharedAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut visuals: ResMut<StoryVisuals>,
    fog_free: Option<ResMut<FogFreeZones>>,
    mut entered: MessageWriter<RoomEnteredEvent>,
) {
    let StoryPhase::Loading(room_id) = run.phase else {
        return;
    };
    let def = room_id.definition();
    let layout = def.layout.layout();
    let handles = visuals.ensure(&mut meshes, &mut materials).clone();

    spawn_layout(
        &mut commands,
        &mut meshes,
        &assets,
        &LayoutMaterials {
            wall: handles.wall.clone(),
        },
        layout,
    );
    if let Some(mut fog_free) = fog_free {
        fog_free.rects.push(layout.fog_rect());
    }

    // A room without a goal (the safe camp) arms its exits on arrival.
    let cleared_on_entry = def.goal == RoomGoal::None;
    let exits = if cleared_on_entry {
        choose_exits(run.depth + 1, &mut run.rng)
    } else {
        [None, None]
    };
    for (slot, center) in layout.exits.into_iter().enumerate() {
        let label = exits[slot].map(|id| id.definition().exit_label());
        spawn_exit_pad(
            &mut commands,
            &mut meshes,
            &handles,
            slot,
            center,
            label,
            cleared_on_entry && exits[slot].is_none(),
        );
    }

    if def.shop {
        let shop = ground_point(layout.shop);
        let mut position = shop_world_position(Team::Radiant);
        position.x = shop.x;
        position.z = shop.z;
        spawn_item_shop(
            &mut commands,
            "Story Item Shop",
            handles.shop_mesh.clone(),
            handles.shop.clone(),
            Team::Radiant,
            position,
        );
    }

    let mut hostiles = 0;
    for spawn in def.spawns {
        for point in spawn_points(spawn.count, spawn.offset) {
            let id = spawn_creep_with(
                &mut commands,
                &assets,
                CreepSpawn {
                    team: Team::Dire,
                    position: ground_point(point),
                    ranged: spawn.ranged,
                    class: spawn.class,
                    lane: None,
                },
            );
            commands.entity(id).insert(RoomHostile);
            hostiles += 1;
        }
    }

    run.room = Some(room_id);
    run.exits = exits;
    run.hostiles_spawned = hostiles;
    run.entry = ground_point(layout.entry);
    run.needs_placement = true;
    run.phase = if cleared_on_entry {
        StoryPhase::Cleared
    } else {
        StoryPhase::Active
    };
    entered.write(RoomEnteredEvent {
        room: room_id,
        depth: run.depth,
    });
}

/// One exit pad. `label` arms it (the pad leads somewhere); `hidden` marks a pad with no
/// destination at all.
fn spawn_exit_pad(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    handles: &StoryHandles,
    slot: usize,
    center: Vec2,
    label: Option<String>,
    hidden: bool,
) {
    let armed = label.is_some();
    let mut pad = commands.spawn((
        Name::new(format!("Exit Pad {slot}")),
        Mesh3d(meshes.add(Cuboid::new(
            EXIT_PAD_HALF.x * 2.0,
            8.0,
            EXIT_PAD_HALF.y * 2.0,
        ))),
        MeshMaterial3d(if armed {
            handles.pad_armed.clone()
        } else {
            handles.pad_idle.clone()
        }),
        Transform::from_xyz(center.x, 4.0, center.y),
        ExitPad {
            slot,
            half: EXIT_PAD_HALF,
            dwell: 0.0,
            armed,
        },
    ));
    if hidden {
        pad.insert(Visibility::Hidden);
    }
    if let Some(label) = label {
        pad.insert(WorldTag(label));
    }
}

/// Move every living hero to the room entry, dropping the orders aimed at the old room.
/// Stays pending until at least one hero was moved, so it is robust to the command flush
/// that happens in the frame a hero spawns.
#[allow(clippy::type_complexity)]
fn place_heroes_at_entry(
    mut commands: Commands,
    mut run: ResMut<StoryRun>,
    mut heroes: Query<
        (
            Entity,
            &mut Transform,
            Option<&CollisionRadius>,
            Option<&mut CommandQueue>,
        ),
        (With<NetworkedHero>, Without<Downed>),
    >,
    focus: Option<ResMut<crate::camera::CameraFocus>>,
) {
    if !run.needs_placement {
        return;
    }
    let mut moved = 0;
    let mut spread = 0.0;
    for (entity, mut transform, radius, mut queue) in &mut heroes {
        transform.translation.x = run.entry.x + spread;
        transform.translation.z = run.entry.z;
        spread += collision_of(radius) * 2.0 + 10.0;
        clear_orders(&mut commands, entity, queue.as_deref_mut());
        moved += 1;
    }
    if moved == 0 {
        return;
    }
    if let Some(mut focus) = focus {
        focus.position = Vec3::new(run.entry.x, 0.0, run.entry.z);
    }
    run.needs_placement = false;
}

/// An `Active` room is cleared when it had hostiles and none is alive any more. The phase
/// latch makes the completion event fire once.
fn check_room_completion(
    mut run: ResMut<StoryRun>,
    hostiles: Query<&Health, With<RoomHostile>>,
    mut completed: MessageWriter<RoomCompletedEvent>,
) {
    if run.phase != StoryPhase::Active || run.hostiles_spawned == 0 {
        return;
    }
    if hostiles.iter().any(|health| health.is_alive()) {
        return;
    }
    let Some(room) = run.room else {
        return;
    };
    run.phase = StoryPhase::Cleared;
    completed.write(RoomCompletedEvent {
        room,
        depth: run.depth,
    });
}

/// Reward and refill for a cleared room, then the exits (or victory at the final depth).
#[allow(clippy::too_many_arguments)]
fn on_room_completed(
    mut events: MessageReader<RoomCompletedEvent>,
    mut commands: Commands,
    mut run: ResMut<StoryRun>,
    mut visuals: ResMut<StoryVisuals>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut heroes: Query<(&mut Inventory, &mut PlayerWallet), With<NetworkedHero>>,
    mut pads: Query<(Entity, &mut ExitPad, &mut MeshMaterial3d<StandardMaterial>)>,
    mut ended: MessageWriter<StoryRunEndedEvent>,
) {
    for event in events.read() {
        let def = event.room.definition();
        let handles = visuals.ensure(&mut meshes, &mut materials).clone();

        // Rule order: refill the bottles, then pay the reward.
        for (mut inventory, _) in &mut heroes {
            refill_charges(&mut inventory, BOTTLE_SLOT);
        }
        match def.reward {
            RoomReward::None => {}
            RoomReward::Gold(amount) => {
                for (_, mut wallet) in &mut heroes {
                    wallet.gold = wallet.gold.saturating_add(amount);
                }
            }
            RoomReward::NeutralItem => {
                let equipped = heroes
                    .iter()
                    .next()
                    .and_then(|(inventory, _)| inventory.slots[crate::items::NEUTRAL_SLOT])
                    .map(|item| item.id);
                let mut pool: Vec<ItemId> = ItemId::all()
                    .iter()
                    .copied()
                    .filter(|id| id.definition().slot == ItemSlot::Neutral)
                    .collect();
                if pool.len() > 1 {
                    pool.retain(|id| Some(*id) != equipped);
                }
                if !pool.is_empty() {
                    let pick = pool[run.rng.below(pool.len() as u32) as usize];
                    spawn_item_pickup(
                        &mut commands,
                        &handles,
                        Vec3::ZERO,
                        crate::items::ItemInstance::new(pick),
                        true,
                    );
                }
            }
        }

        if event.depth >= FINAL_DEPTH {
            run.phase = StoryPhase::Victory;
            ended.write(StoryRunEndedEvent { victory: true });
            continue;
        }
        run.exits = choose_exits(event.depth + 1, &mut run.rng);
        for (entity, mut pad, mut material) in &mut pads {
            let Some(next) = run.exits.get(pad.slot).copied().flatten() else {
                continue;
            };
            pad.armed = true;
            *material = MeshMaterial3d(handles.pad_armed.clone());
            commands.entity(entity).insert((
                Visibility::Inherited,
                WorldTag(next.definition().exit_label()),
            ));
        }
    }
}

/// Time on an armed pad accumulates while a living hero overlaps it and resets when none
/// does. At [`EXIT_DWELL_SECS`] the run moves to `Transition`. Creeps never trigger pads.
#[allow(clippy::type_complexity)]
fn tick_exit_pads(
    time: Res<Time>,
    mut run: ResMut<StoryRun>,
    mut pads: Query<(&mut ExitPad, &Transform)>,
    heroes: Query<
        (&Transform, Option<&CollisionRadius>, &Health),
        (With<NetworkedHero>, Without<Downed>),
    >,
    shop: Option<ResMut<crate::items::ShopUiState>>,
) {
    if run.phase != StoryPhase::Cleared {
        return;
    }
    let dt = time.delta_secs();
    let mut taken = None;
    for (mut pad, transform) in &mut pads {
        let overlapped = pad.armed
            && heroes.iter().any(|(hero_tf, radius, health)| {
                health.is_alive()
                    && rect_touches_circle(
                        transform.translation,
                        pad.half,
                        hero_tf.translation,
                        collision_of(radius),
                    )
            });
        if !overlapped {
            pad.dwell = 0.0;
            continue;
        }
        pad.dwell += dt;
        if pad.dwell >= EXIT_DWELL_SECS
            && taken.is_none()
            && let Some(next) = run.exits.get(pad.slot).copied().flatten()
        {
            taken = Some(next);
        }
    }
    if let Some(next) = taken {
        run.phase = StoryPhase::Transition(next);
        if let Some(mut shop) = shop {
            shop.open = false;
            shop.detail = None;
        }
    }
}

/// Despawn the finished room (invariant S6: only in `Last`) and queue the next one. Heroes
/// stay; their orders are dropped so nothing refers to the removed room.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn teardown_room(
    mut commands: Commands,
    mut run: ResMut<StoryRun>,
    room: Query<
        Entity,
        (
            map::MatchEntityFilter,
            Without<NetworkedHero>,
            Without<crate::components::HealthBar>,
        ),
    >,
    mut heroes: Query<(Entity, Option<&mut CommandQueue>), With<NetworkedHero>>,
    fog_free: Option<ResMut<FogFreeZones>>,
    targeting: Option<ResMut<crate::abilities::AbilityTargeting>>,
    hovered: Option<ResMut<crate::picking::HoveredUnit>>,
    selected: Option<ResMut<crate::picking::SelectedUnit>>,
) {
    let StoryPhase::Transition(next) = run.phase else {
        return;
    };
    for entity in &room {
        commands.entity(entity).despawn();
    }
    for (entity, mut queue) in &mut heroes {
        clear_orders(&mut commands, entity, queue.as_deref_mut());
    }
    if let Some(mut fog_free) = fog_free {
        fog_free.rects.clear();
    }
    if let Some(mut targeting) = targeting {
        targeting.active = None;
    }
    if let Some(mut hovered) = hovered {
        hovered.0 = None;
    }
    if let Some(mut selected) = selected {
        selected.0 = None;
    }
    run.depth += 1;
    run.room = None;
    run.exits = [None, None];
    run.hostiles_spawned = 0;
    run.phase = StoryPhase::Loading(next);
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

    #[test]
    fn rng_is_deterministic_and_spreads() {
        let mut a = StoryRng::new(7);
        let mut b = StoryRng::new(7);
        let draws: Vec<u32> = (0..64).map(|_| a.below(10)).collect();
        assert_eq!(draws, (0..64).map(|_| b.below(10)).collect::<Vec<_>>());
        for value in 0..10 {
            assert!(draws.contains(&value), "{value} never drawn");
        }
        assert_ne!(StoryRng::new(0).next_u64(), 0);
    }

    #[test]
    fn a_fresh_run_is_idle_at_the_safe_entry() {
        let run = StoryRun::default();
        assert_eq!(run.phase, StoryPhase::Idle);
        assert_eq!(run.entry, SAFE_ENTRY);
        assert_eq!(run.depth, 0);
    }
}
