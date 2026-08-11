//! Bevy resources and systems that make the streamed world playable.

use std::error::Error;

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu"))]
use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::input::mouse::MouseMotion;
use bevy::input::touch::{TouchInput, TouchPhase};
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu"))]
use bevy::light::ShadowFilteringMethod;
use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap};
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu"))]
use bevy::pbr::ContactShadows;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use glam::DVec3;
use treeline_renderer::{LightingSettings, TimeOfDay, WorldMaterials, WorldMeshOrigin};
use treeline_terrain::{SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z};
use treeline_world::{DEFAULT_WORLD_IDENTITY, WorldTerrain};
use web_time::Instant;

use crate::camera::{Camera as PlayerCamera, EYE_HEIGHT, surface_height};
use crate::input::InputState;
use crate::progress::LoadProgress;
use crate::streaming::{PlayerMotion, ResidentTerrain, Streamers};
use crate::trees::ResidentTrees;
use crate::{TerrainMeshQueue, atmosphere, random, start_terrain_queue, streaming, warp};

const SPAWN_YAW: f64 = -1.924_842_228_418_599_5;
const SPAWN_PITCH: f64 = -0.08;
const FLOATING_ORIGIN_GRID_METERS: f64 = 256.0;

#[derive(Resource)]
pub struct GameTerrain(pub WorldTerrain);

pub struct TerrainJobs(pub TerrainMeshQueue);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Resource)]
enum WarpRequest {
    #[default]
    None,
    Random,
    LakeShore,
}

#[derive(Resource)]
struct Daylight(TimeOfDay);

#[derive(Resource, Default)]
struct FloatingOrigin([f64; 3]);

#[derive(Component)]
struct PlayerView;

#[derive(Component)]
struct Sun;

#[derive(Debug, Default)]
pub struct TreelineGamePlugin;

impl Plugin for TreelineGamePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DirectionalLightShadowMap { size: 2048 })
            .init_resource::<InputState>()
            .init_resource::<ResidentTerrain>()
            .init_resource::<ResidentTrees>()
            .init_resource::<WarpRequest>()
            .init_resource::<FloatingOrigin>()
            .insert_resource(Daylight(TimeOfDay::default()))
            .insert_resource(Streamers::default())
            .add_systems(Startup, setup_world)
            .add_systems(
                Update,
                (
                    collect_keyboard_input,
                    collect_mouse_look,
                    collect_touch_input,
                    collect_browser_actions,
                    apply_warp,
                    move_player,
                    stream_world,
                    sync_floating_origin,
                    sync_daylight,
                    publish_progress,
                )
                    .chain(),
            );
    }
}

pub fn initial_world() -> Result<(WorldTerrain, TerrainMeshQueue), Box<dyn Error>> {
    let terrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);
    let jobs = start_terrain_queue(terrain)?;
    Ok((terrain, jobs))
}

fn setup_world(
    mut commands: Commands,
    terrain: Res<GameTerrain>,
    streamers: Res<Streamers>,
    mut resident: ResMut<ResidentTerrain>,
    mut jobs: NonSendMut<TerrainJobs>,
) {
    let started = Instant::now();
    let spawn_y = surface_height(&terrain.0, SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z) + EYE_HEIGHT;
    let camera = PlayerCamera::new(
        DVec3::new(SURVEYED_SPAWN_X, spawn_y, SURVEYED_SPAWN_Z),
        SPAWN_YAW,
        SPAWN_PITCH,
    );
    streaming::schedule(
        &mut commands,
        *streamers,
        PlayerMotion::arrived(camera.world_position()),
        &mut resident,
        &mut jobs.0,
    )
    .expect("the surveyed spawn is streamable");
    let progress = progress_for(started, *streamers, &resident, camera)
        .expect("the surveyed spawn is inside the far terrain lattice");

    let fog = atmosphere::settings_for(terrain.0.climate()).unwrap_or_default();
    let camera_entity = commands
        .spawn((
            Name::new("player camera"),
            PlayerView,
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                near: 0.05,
                far: 25_000.0,
                ..default()
            }),
            Transform::default(),
            DistanceFog {
                color: Color::srgb(fog.fog_color[0], fog.fog_color[1], fog.fog_color[2]),
                directional_light_color: Color::srgba(1.0, 0.86, 0.7, 0.35),
                directional_light_exponent: 24.0,
                falloff: FogFalloff::from_visibility(18_000.0 / fog.fog_density.max(0.1)),
            },
            Msaa::Off,
        ))
        .id();
    add_camera_effects(&mut commands, camera_entity);

    let cascades = CascadeShadowConfigBuilder {
        num_cascades: if cfg!(target_arch = "wasm32") { 1 } else { 4 },
        first_cascade_far_bound: 32.0,
        maximum_distance: 480.0,
        overlap_proportion: 0.18,
        ..default()
    }
    .build();
    commands.spawn((
        Name::new("sun"),
        Sun,
        DirectionalLight {
            shadow_maps_enabled: true,
            contact_shadows_enabled: true,
            ..default()
        },
        Transform::default(),
        cascades,
    ));
    commands.insert_resource(camera);
    commands.insert_resource(progress);
}

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu"))]
fn add_camera_effects(commands: &mut Commands, camera: Entity) {
    commands.entity(camera).insert((
        ContactShadows::default(),
        TemporalAntiAliasing::default(),
        ShadowFilteringMethod::Temporal,
    ));
}

#[cfg(all(target_arch = "wasm32", not(feature = "webgpu")))]
fn add_camera_effects(_commands: &mut Commands, _camera: Entity) {}

fn collect_keyboard_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut input: ResMut<InputState>,
    mut warp_request: ResMut<WarpRequest>,
    mut daylight: ResMut<Daylight>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    terrain: Res<GameTerrain>,
    mut camera: ResMut<PlayerCamera>,
) {
    for key in [
        KeyCode::KeyW,
        KeyCode::KeyA,
        KeyCode::KeyS,
        KeyCode::KeyD,
        KeyCode::ArrowUp,
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
    ] {
        input.set_key(key, keys.pressed(key));
    }
    if mouse.just_pressed(MouseButton::Left) {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        input.clear();
    }
    if keys.just_pressed(KeyCode::KeyR) {
        *warp_request = WarpRequest::Random;
    }
    if keys.just_pressed(KeyCode::KeyB) {
        *warp_request = WarpRequest::LakeShore;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        let enabled = camera.toggle_aerial_mode(&terrain.0);
        #[cfg(target_arch = "wasm32")]
        crate::browser::BrowserActions::set_aerial_mode_enabled(enabled);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = enabled;
    }
    if keys.just_pressed(KeyCode::KeyT) {
        daylight.0 = daylight.0.next();
    }
}

fn collect_mouse_look(
    mut motions: MessageReader<MouseMotion>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    mut camera: ResMut<PlayerCamera>,
) {
    if cursor.grab_mode == CursorGrabMode::None {
        motions.clear();
        return;
    }
    for motion in motions.read() {
        camera.look(f64::from(motion.delta.x), f64::from(motion.delta.y));
    }
}

fn collect_touch_input(
    mut touches: MessageReader<TouchInput>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut input: ResMut<InputState>,
) {
    let width = window.width().max(1.0);
    let radius = 64.0 * window.resolution.scale_factor();
    for touch in touches.read() {
        let position = glam::Vec2::new(touch.position.x, touch.position.y);
        match touch.phase {
            TouchPhase::Started => input.begin_touch(touch.id, position, width, radius),
            TouchPhase::Moved => input.move_touch(touch.id, position),
            TouchPhase::Ended | TouchPhase::Canceled => input.end_touch(touch.id),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn collect_browser_actions() {}

#[cfg(target_arch = "wasm32")]
fn collect_browser_actions(
    actions: NonSend<crate::browser::BrowserActions>,
    mut warp_request: ResMut<WarpRequest>,
    terrain: Res<GameTerrain>,
    mut camera: ResMut<PlayerCamera>,
) {
    if actions.take_random_warp() {
        *warp_request = WarpRequest::Random;
    }
    if actions.take_water_warp() {
        *warp_request = WarpRequest::LakeShore;
    }
    if actions.take_aerial_toggle() {
        let enabled = camera.toggle_aerial_mode(&terrain.0);
        crate::browser::BrowserActions::set_aerial_mode_enabled(enabled);
    }
}

fn move_player(
    time: Res<Time>,
    input: Res<InputState>,
    terrain: Res<GameTerrain>,
    mut camera: ResMut<PlayerCamera>,
) {
    let delta_seconds = time.delta_secs_f64().min(0.1);
    camera.look_with_stick(input.look_axis(), delta_seconds);
    camera.walk(&input, &terrain.0, delta_seconds);
}

#[allow(clippy::too_many_arguments)]
fn stream_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    materials: Res<WorldMaterials>,
    terrain: Res<GameTerrain>,
    streamers: Res<Streamers>,
    camera: Res<PlayerCamera>,
    input: Res<InputState>,
    mut resident: ResMut<ResidentTerrain>,
    mut trees: ResMut<ResidentTrees>,
    mut jobs: NonSendMut<TerrainJobs>,
    mut progress: ResMut<LoadProgress>,
) {
    let motion = PlayerMotion {
        position: camera.world_position(),
        travel_direction: camera.travel_direction(&input),
    };
    if let Err(error) = streaming::update(
        &mut commands,
        &mut meshes,
        &materials,
        &terrain.0,
        *streamers,
        motion,
        &mut resident,
        &mut jobs.0,
        &mut progress,
    ) {
        error!("terrain streaming failed: {error}");
    }
    if let Err(error) = trees.update(
        &mut commands,
        &mut meshes,
        &materials,
        &terrain.0,
        streamers.near.config(),
        camera.world_position(),
    ) {
        error!("tree streaming failed: {error}");
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_warp(
    mut commands: Commands,
    terrain: Res<GameTerrain>,
    streamers: Res<Streamers>,
    mut request: ResMut<WarpRequest>,
    mut camera: ResMut<PlayerCamera>,
    mut input: ResMut<InputState>,
    mut resident: ResMut<ResidentTerrain>,
    mut trees: ResMut<ResidentTrees>,
    mut jobs: NonSendMut<TerrainJobs>,
    mut progress: ResMut<LoadProgress>,
) {
    let site = match std::mem::take(&mut *request) {
        WarpRequest::None => return,
        WarpRequest::Random => warp::random_site(&terrain.0, random::unit_interval),
        WarpRequest::LakeShore => warp::lake_shore_site(&terrain.0, random::unit_interval()),
    };
    let Some(site) = site else {
        error!("could not find a valid warp destination");
        return;
    };
    let [x, z] = site.destination;
    resident.clear(&mut commands, &mut jobs.0);
    trees.clear(&mut commands);
    camera.position = DVec3::new(x, camera.height_over(&terrain.0, x, z), z);
    if let Some(target) = site.face {
        camera.face(target);
    }
    input.clear();
    if let Err(error) = streaming::schedule(
        &mut commands,
        *streamers,
        PlayerMotion::arrived(camera.world_position()),
        &mut resident,
        &mut jobs.0,
    ) {
        error!("warp streaming failed: {error}");
        return;
    }
    if let Some(next) = progress_for(Instant::now(), *streamers, &resident, *camera) {
        *progress = next;
    }
}

fn sync_floating_origin(
    camera: Res<PlayerCamera>,
    mut floating: ResMut<FloatingOrigin>,
    mut world_meshes: Query<(&WorldMeshOrigin, &mut Transform), Without<PlayerView>>,
    mut view: Single<&mut Transform, With<PlayerView>>,
) {
    floating.0 = [camera.position.x, 0.0, camera.position.z].map(|coordinate| {
        libm::floor(coordinate / FLOATING_ORIGIN_GRID_METERS) * FLOATING_ORIGIN_GRID_METERS
    });
    for (origin, mut transform) in &mut world_meshes {
        transform.translation = Vec3::new(
            f64_as_f32(origin.0[0] - floating.0[0]),
            f64_as_f32(origin.0[1] - floating.0[1]),
            f64_as_f32(origin.0[2] - floating.0[2]),
        );
    }
    view.translation = Vec3::new(
        f64_as_f32(camera.position.x - floating.0[0]),
        f64_as_f32(camera.position.y - floating.0[1]),
        f64_as_f32(camera.position.z - floating.0[2]),
    );
    view.look_to(
        Vec3::from_array(camera.direction().as_vec3().to_array()),
        Vec3::Y,
    );
}

fn sync_daylight(
    daylight: Res<Daylight>,
    mut sun: Single<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    mut clear: ResMut<ClearColor>,
) {
    if !daylight.is_changed() {
        return;
    }
    let settings = LightingSettings::for_time_of_day(daylight.0);
    sun.0.color = Color::srgb(
        settings.sun_color[0],
        settings.sun_color[1],
        settings.sun_color[2],
    );
    sun.0.illuminance = 18_000.0 * settings.sun_intensity;
    let toward_ground = -Vec3::from_array(settings.sun_direction);
    sun.1.look_to(toward_ground, Vec3::Y);
    clear.0 = Color::srgb(
        settings.sky_horizon[0],
        settings.sky_horizon[1],
        settings.sky_horizon[2],
    );
}

fn publish_progress(
    mut progress: ResMut<LoadProgress>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    progress.publish(&mut window);
}

fn progress_for(
    started: Instant,
    streamers: Streamers,
    resident: &ResidentTerrain,
    camera: PlayerCamera,
) -> Option<LoadProgress> {
    let (chunks, far_tiles) = resident.outstanding();
    LoadProgress::new(
        started,
        chunks,
        far_tiles,
        streamers.far,
        camera.world_position(),
    )
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}
