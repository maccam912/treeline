//! Generator Lab — a Bevy-native top-down inspector for the surveyed world.

#![allow(clippy::needless_pass_by_value)]

mod inspect;
mod map;
mod view;

use bevy::asset::RenderAssetUsages;
use bevy::camera::ScalingMode;
use bevy::input::mouse::MouseWheel;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use bevy::window::{CursorMoved, PrimaryWindow, WindowResolution};
use inspect::Inspection;
use map::MapView;
use treeline_climate::Season;
use treeline_renderer::prepare_terrain_mesh;
use treeline_terrain::{SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z, SURVEYED_TILE_EDGE_METERS};
use treeline_world::{DEFAULT_WORLD_IDENTITY, WorldTerrain};
use view::ViewMode;

const MIN_SPAN_METERS: f64 = 60.0;
const MAX_SPAN_METERS: f64 = SURVEYED_TILE_EDGE_METERS * 1.2;
const ZOOM_STEP: f64 = 1.4;
const PAN_STEP_FRACTION: f64 = 0.15;

#[derive(Resource)]
struct LabState {
    terrain: WorldTerrain,
    view: MapView,
    cursor: [f64; 2],
    inspection: Option<Inspection>,
    map_dirty: bool,
    hud_dirty: bool,
}

#[derive(Component)]
struct MapSurface;

#[derive(Component)]
struct LabCamera;

#[derive(Component)]
struct LabHud;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Treeline Generator Lab".into(),
                resolution: WindowResolution::new(1200, 800),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(LabState {
            terrain: WorldTerrain::new(DEFAULT_WORLD_IDENTITY),
            view: MapView {
                center: [SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z],
                span_meters: 2_000.0,
                mode: ViewMode::default(),
                season: Season::Winter,
            },
            cursor: [600.0, 400.0],
            inspection: None,
            map_dirty: true,
            hud_dirty: true,
        })
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (handle_input, rebuild_map, update_hud, update_projection).chain(),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Name::new("map camera"),
        LabCamera,
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: 2_000.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 1_000.0, 0.0).looking_at(Vec3::ZERO, Vec3::Z),
    ));
    commands.spawn((
        Name::new("surveyed map"),
        MapSurface,
        Mesh3d(meshes.add(Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        ))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            cull_mode: None,
            ..default()
        })),
        Transform::default(),
    ));
    commands.spawn((
        LabHud,
        Text::new("Generator Lab"),
        TextFont::from_font_size(15.0),
        TextColor(Color::srgb(0.94, 0.94, 0.88)),
        Node {
            position_type: PositionType::Absolute,
            left: px(14),
            top: px(14),
            max_width: px(470),
            padding: UiRect::all(px(12)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.025, 0.04, 0.03, 0.88)),
    ));
}

fn handle_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut cursor_events: MessageReader<CursorMoved>,
    mut wheel_events: MessageReader<MouseWheel>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut state: ResMut<LabState>,
) {
    for event in cursor_events.read() {
        state.cursor = [f64::from(event.position.x), f64::from(event.position.y)];
    }
    let pan = state.view.span_meters * PAN_STEP_FRACTION;
    let mut changed = false;
    for (key, mode) in [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
    ]
    .into_iter()
    .zip(ViewMode::ALL)
    {
        if keys.just_pressed(key) && state.view.mode != mode {
            state.view.mode = mode;
            changed = true;
        }
    }
    let mut pan_by = |offset: [f64; 2]| {
        state.view.center[0] += offset[0];
        state.view.center[1] += offset[1];
        changed = true;
    };
    if keys.just_pressed(KeyCode::KeyW) || keys.just_pressed(KeyCode::ArrowUp) {
        pan_by([0.0, -pan]);
    }
    if keys.just_pressed(KeyCode::KeyS) || keys.just_pressed(KeyCode::ArrowDown) {
        pan_by([0.0, pan]);
    }
    if keys.just_pressed(KeyCode::KeyA) || keys.just_pressed(KeyCode::ArrowLeft) {
        pan_by([-pan, 0.0]);
    }
    if keys.just_pressed(KeyCode::KeyD) || keys.just_pressed(KeyCode::ArrowRight) {
        pan_by([pan, 0.0]);
    }
    let mut zoom_factor = 1.0;
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) {
        zoom_factor /= ZOOM_STEP;
    }
    if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        zoom_factor *= ZOOM_STEP;
    }
    for wheel in wheel_events.read() {
        if wheel.y > 0.0 {
            zoom_factor /= ZOOM_STEP;
        } else if wheel.y < 0.0 {
            zoom_factor *= ZOOM_STEP;
        }
    }
    if (zoom_factor - 1.0_f64).abs() > f64::EPSILON {
        state.view.span_meters =
            (state.view.span_meters * zoom_factor).clamp(MIN_SPAN_METERS, MAX_SPAN_METERS);
        changed = true;
    }
    if keys.just_pressed(KeyCode::KeyC) {
        state.view.season = state.view.season.next();
        changed = true;
    }
    let width = logical_extent(window.width());
    let height = logical_extent(window.height());
    if mouse.just_pressed(MouseButton::Right) {
        state.view.center = state.view.world_position_at(state.cursor, width, height);
        changed = true;
    }
    if mouse.just_pressed(MouseButton::Left) {
        let [x, z] = state.view.world_position_at(state.cursor, width, height);
        state.inspection = Some(inspect::at(state.terrain, x, z, state.view.season));
        state.hud_dirty = true;
    }
    state.map_dirty |= changed;
    state.hud_dirty |= changed;
}

fn rebuild_map(
    mut state: ResMut<LabState>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut meshes: ResMut<Assets<Mesh>>,
    map_mesh: Single<&Mesh3d, With<MapSurface>>,
) {
    if !state.map_dirty {
        return;
    }
    state.map_dirty = false;
    let width = window.resolution.physical_width().max(1);
    let height = window.resolution.physical_height().max(1);
    let source = map::build(state.terrain, state.view, width, height);
    match prepare_terrain_mesh(&source, |_, _| None) {
        Ok(prepared) => {
            let Some(mut mesh) = meshes.get_mut(&map_mesh.0) else {
                error!("the Generator Lab map mesh asset is missing");
                return;
            };
            *mesh = prepared.mesh;
        }
        Err(error) => error!("could not rebuild the inspected map: {error}"),
    }
}

fn update_projection(
    state: Res<LabState>,
    mut projection: Single<&mut Projection, With<LabCamera>>,
) {
    if !state.is_changed() {
        return;
    }
    if let Projection::Orthographic(orthographic) = &mut **projection {
        orthographic.scaling_mode = ScalingMode::FixedVertical {
            viewport_height: f64_as_f32(state.view.span_meters),
        };
    }
}

fn update_hud(mut state: ResMut<LabState>, mut text: Single<&mut Text, With<LabHud>>) {
    if !state.hud_dirty {
        return;
    }
    state.hud_dirty = false;
    let mut lines = vec![
        format!(
            "{} — {}",
            state.view.mode.label(),
            state.view.mode.description()
        ),
        format!(
            "center {:.1}, {:.1} · span {:.0} m · {}",
            state.view.center[0],
            state.view.center[1],
            state.view.span_meters,
            state.view.season.label()
        ),
        "1–7 layer · WASD pan · wheel/+/- zoom · C season".into(),
        "left click inspect · right click recenter".into(),
    ];
    if let Some(inspection) = &state.inspection {
        lines.push(String::new());
        lines.push(format!(
            "inspection {:.2}, {:.2}",
            inspection.position[0], inspection.position[1]
        ));
        lines.extend(inspection.lines.iter().cloned());
    }
    text.0 = lines.join("\n");
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn logical_extent(value: f32) -> u32 {
    value.max(1.0) as u32
}
