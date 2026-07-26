use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use treeline_coordinates::{CellIndex, WorldIdentity};
use treeline_geography::{DrainageCell, RegionalProfile, WatershedRegion, WatershedRegionIndex};
use treeline_hydrology::{RiverNetwork, RiverSegment};
use treeline_mesher::{Mesh, SurfaceGridSpec, surface_grid};
use treeline_renderer::{TerrainMesh, TerrainRenderer};
use treeline_terrain::{SurfaceField, WildernessTerrain};
use treeline_world::GeneratedWorldTerrain;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

const GENERATOR_VERSION: u32 = 3;
const GRID_ROWS: usize = 128;
const MIN_SPAN_METERS: f64 = 1_000.0;
const MAX_SPAN_METERS: f64 = 1_000_000.0;
const DOMAIN_WATERSHED_COLOR: u64 = 0x5741_5445_5253_4844;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ViewMode {
    #[default]
    Terrain,
    Watersheds,
    FlowAccumulation,
    Rivers,
}

impl ViewMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Terrain => "terrain",
            Self::Watersheds => "watersheds",
            Self::FlowAccumulation => "flow accumulation",
            Self::Rivers => "rivers",
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = GeneratorLabApp::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[derive(Default)]
struct GeneratorLabApp {
    lab: Option<GeneratorLab>,
}

impl ApplicationHandler for GeneratorLabApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.lab.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Treeline Generator Lab")
            .with_inner_size(LogicalSize::new(1_200, 800));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create Generator Lab window: {error}");
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(GeneratorLab::new(window)) {
            Ok(lab) => self.lab = Some(lab),
            Err(error) => {
                eprintln!("failed to start Generator Lab: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(lab) = self.lab.as_mut() else {
            return;
        };
        if window_id != lab.window.id() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Err(error) = lab.resize(size.width, size.height) {
                    eprintln!("failed to resize Generator Lab: {error}");
                }
            }
            WindowEvent::RedrawRequested => match lab.render() {
                Ok(()) | Err(wgpu::SurfaceError::Timeout) => {}
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    lab.reconfigure_surface();
                }
                Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                Err(error) => eprintln!("Generator Lab render failed: {error}"),
            },
            WindowEvent::CursorMoved { position, .. } => lab.cursor = position,
            WindowEvent::MouseWheel { delta, .. } => {
                let direction = match delta {
                    MouseScrollDelta::LineDelta(_, vertical) => f64::from(vertical),
                    MouseScrollDelta::PixelDelta(position) => position.y.signum(),
                };
                if direction != 0.0 {
                    lab.zoom(if direction > 0.0 { 0.7 } else { 1.4 });
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => match button {
                MouseButton::Left => lab.inspect_cursor(),
                MouseButton::Right => {
                    lab.center = lab.cursor_world_position();
                    if let Err(error) = lab.regenerate() {
                        eprintln!("failed to teleport Generator Lab: {error}");
                    }
                }
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape {
                        event_loop.exit();
                    } else {
                        lab.handle_key(code);
                    }
                }
            }
            _ => {}
        }
    }
}

struct GeneratorLab {
    window: Arc<Window>,
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    renderer: TerrainRenderer,
    mesh: TerrainMesh,
    seed: u64,
    center: [f64; 2],
    span_meters: f64,
    cursor: PhysicalPosition<f64>,
    mode: ViewMode,
}

impl GeneratorLab {
    async fn new(window: Arc<Window>) -> Result<Self, Box<dyn Error>> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .ok_or_else(|| std::io::Error::other("no compatible graphics adapter found"))?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Generator Lab device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await?;
        let size = window.inner_size();
        let capabilities = surface.get_capabilities(&adapter);
        let surface_format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);
        let renderer = TerrainRenderer::new(
            &device,
            surface_format,
            surface_config.width,
            surface_config.height,
        );
        let seed = 0x5eed;
        let center = [0.0, 0.0];
        let span_meters = 128_000.0;
        let mode = ViewMode::Terrain;
        let mesh_data = generate_mesh(
            seed,
            center,
            span_meters,
            surface_config.width,
            surface_config.height,
            mode,
        )?;
        let mesh = renderer.upload_mesh(&device, &mesh_data)?;
        let cursor = PhysicalPosition::new(
            f64::from(surface_config.width) * 0.5,
            f64::from(surface_config.height) * 0.5,
        );
        let lab = Self {
            window,
            _instance: instance,
            surface,
            device,
            queue,
            surface_config,
            renderer,
            mesh,
            seed,
            center,
            span_meters,
            cursor,
            mode,
        };
        lab.update_camera();
        lab.update_title(None);
        Ok(lab)
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), Box<dyn Error>> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.reconfigure_surface();
        self.renderer.resize(&self.device, width, height);
        self.regenerate()
    }

    fn reconfigure_surface(&self) {
        self.surface.configure(&self.device, &self.surface_config);
    }

    fn handle_key(&mut self, code: KeyCode) {
        let step = self.span_meters * 0.15;
        let changed = match code {
            KeyCode::KeyW | KeyCode::ArrowUp => {
                self.center[1] -= step;
                true
            }
            KeyCode::KeyS | KeyCode::ArrowDown => {
                self.center[1] += step;
                true
            }
            KeyCode::KeyA | KeyCode::ArrowLeft => {
                self.center[0] -= step;
                true
            }
            KeyCode::KeyD | KeyCode::ArrowRight => {
                self.center[0] += step;
                true
            }
            KeyCode::Equal | KeyCode::NumpadAdd => {
                self.span_meters *= 0.7;
                true
            }
            KeyCode::Minus | KeyCode::NumpadSubtract => {
                self.span_meters *= 1.4;
                true
            }
            KeyCode::KeyR => {
                self.seed = self.seed.wrapping_add(1);
                true
            }
            KeyCode::KeyT => {
                self.center = self.cursor_world_position();
                true
            }
            KeyCode::Digit1 => {
                self.mode = ViewMode::Terrain;
                true
            }
            KeyCode::Digit2 => {
                self.mode = ViewMode::Watersheds;
                true
            }
            KeyCode::Digit3 => {
                self.mode = ViewMode::FlowAccumulation;
                true
            }
            KeyCode::Digit4 => {
                self.mode = ViewMode::Rivers;
                true
            }
            _ => false,
        };
        self.span_meters = self.span_meters.clamp(MIN_SPAN_METERS, MAX_SPAN_METERS);
        if changed {
            if let Err(error) = self.regenerate() {
                eprintln!("failed to update Generator Lab view: {error}");
            }
        }
    }

    fn zoom(&mut self, factor: f64) {
        self.span_meters = (self.span_meters * factor).clamp(MIN_SPAN_METERS, MAX_SPAN_METERS);
        if let Err(error) = self.regenerate() {
            eprintln!("failed to zoom Generator Lab: {error}");
        }
    }

    fn regenerate(&mut self) -> Result<(), Box<dyn Error>> {
        let mesh_data = generate_mesh(
            self.seed,
            self.center,
            self.span_meters,
            self.surface_config.width,
            self.surface_config.height,
            self.mode,
        )?;
        self.mesh = self.renderer.upload_mesh(&self.device, &mesh_data)?;
        self.update_camera();
        self.update_title(None);
        self.window.request_redraw();
        Ok(())
    }

    fn update_camera(&self) {
        let aspect =
            f64::from(self.surface_config.width) / f64::from(self.surface_config.height.max(1));
        let half_height = f64_as_f32(self.span_meters * 0.5);
        let half_width = f64_as_f32(self.span_meters * aspect * 0.5);
        let center_x = f64_as_f32(self.center[0]);
        let center_z = f64_as_f32(self.center[1]);
        let projection = Mat4::orthographic_rh(
            -half_width,
            half_width,
            -half_height,
            half_height,
            0.1,
            4_000.0,
        );
        let view = Mat4::look_to_rh(
            Vec3::new(center_x, 2_000.0, center_z),
            Vec3::NEG_Y,
            Vec3::NEG_Z,
        );
        self.renderer
            .update_camera(&self.queue, (projection * view).to_cols_array_2d());
    }

    fn cursor_world_position(&self) -> [f64; 2] {
        let width = f64::from(self.surface_config.width.max(1));
        let height = f64::from(self.surface_config.height.max(1));
        let aspect = width / height;
        [
            self.center[0] + ((self.cursor.x / width) - 0.5) * self.span_meters * aspect,
            self.center[1] + ((self.cursor.y / height) - 0.5) * self.span_meters,
        ]
    }

    fn inspect_cursor(&self) {
        let [x, z] = self.cursor_world_position();
        let world = WorldIdentity::new(self.seed, GENERATOR_VERSION, 0);
        let terrain = WildernessTerrain::new(world);
        let generated_terrain = GeneratedWorldTerrain::new(world);
        let Some((macro_sample, surface_height)) = terrain.inspect(x, z) else {
            return;
        };
        let carved_surface_height = generated_terrain
            .surface_height(x, z)
            .unwrap_or(surface_height);
        let river_influence = generated_terrain.river_influence_at(x, z);
        let Some(profile) = RegionalProfile::sample(world, x, z) else {
            return;
        };
        let watershed = WatershedRegionIndex::containing(x, z)
            .and_then(|index| WatershedRegion::generate(world, index));
        let drainage = watershed
            .as_ref()
            .and_then(|region| region.cell_at(x, z).copied());
        let river_network = watershed.as_ref().and_then(RiverNetwork::from_watershed);
        let river = drainage.and_then(|cell| {
            river_network
                .as_ref()
                .and_then(|network| network.segment_from(cell.index))
                .copied()
        });
        let drainage_summary = drainage.map_or_else(
            || "drainage unavailable".to_owned(),
            |cell| {
                let river_summary = river.map_or_else(String::new, |segment| {
                    format!(
                        " | river {:.2} m³/s | {:.0} km²",
                        segment.discharge_cubic_meters_per_second,
                        segment.drainage_area_square_kilometers
                    )
                });
                format!(
                    "flow {} cells | outlet ({}, {}){}{}",
                    cell.flow_accumulation_cells,
                    cell.watershed_outlet.x,
                    cell.watershed_outlet.z,
                    if cell.basin.is_some() { " | basin" } else { "" },
                    river_summary
                )
            },
        );
        let summary = format!(
            "x {x:.0} m, z {z:.0} m | height {carved_surface_height:.0} m | ridge +{:.0} m | {drainage_summary}",
            macro_sample.mountain_uplift_meters
        );
        eprintln!(
            "Generator Lab inspection\ncoordinate: ({x:.2}, {z:.2})\nbase surface height: {surface_height:.2} m\ncarved surface height: {carved_surface_height:.2} m\nmacro terrain: {macro_sample:#?}\nregional profile: {profile:#?}\ndrainage cell: {drainage:#?}\nriver segment: {river:#?}\nriver terrain influence: {river_influence:#?}"
        );
        self.update_title(Some(&summary));
    }

    fn update_title(&self, inspection: Option<&str>) {
        let base = format!(
            "Treeline Generator Lab — {} | seed {:x} | center ({:.1}, {:.1}) km | span {:.0} km | 1 terrain · 2 watersheds · 3 flow · 4 rivers · WASD pan · +/- zoom · R seed · click inspect",
            self.mode.label(),
            self.seed,
            self.center[0] / 1_000.0,
            self.center[1] / 1_000.0,
            self.span_meters / 1_000.0
        );
        self.window.set_title(
            inspection
                .map(|inspection| format!("{base} | {inspection}"))
                .as_deref()
                .unwrap_or(&base),
        );
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Generator Lab frame encoder"),
            });
        self.renderer
            .render(&mut encoder, &view, std::iter::once(&self.mesh));
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

fn generate_mesh(
    seed: u64,
    center: [f64; 2],
    span_meters: f64,
    width: u32,
    height: u32,
    mode: ViewMode,
) -> Result<treeline_mesher::Mesh, Box<dyn Error>> {
    if mode != ViewMode::Terrain {
        return generate_drainage_mesh(seed, center, span_meters, width, height, mode);
    }
    let (columns, spacing) = grid_dimensions(span_meters, width, height);
    let field = GeneratedWorldTerrain::new(WorldIdentity::new(seed, GENERATOR_VERSION, 0));
    Ok(surface_grid(
        &field,
        SurfaceGridSpec::new(
            center[0] - (usize_as_f64(columns) * spacing * 0.5),
            center[1] - (span_meters * 0.5),
            [columns, GRID_ROWS],
            spacing,
        ),
    )?)
}

fn generate_drainage_mesh(
    seed: u64,
    center: [f64; 2],
    span_meters: f64,
    width: u32,
    height: u32,
    mode: ViewMode,
) -> Result<Mesh, Box<dyn Error>> {
    let (columns, spacing) = grid_dimensions(span_meters, width, height);
    let count_x = columns
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("drainage grid is too large"))?;
    let count_z = GRID_ROWS
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("drainage grid is too large"))?;
    let origin_x = center[0] - (usize_as_f64(columns) * spacing * 0.5);
    let origin_z = center[1] - (span_meters * 0.5);
    let world = WorldIdentity::new(seed, GENERATOR_VERSION, 0);
    let mut regions = BTreeMap::new();
    let mut river_networks = BTreeMap::new();
    let mut positions = Vec::with_capacity(count_x * count_z);
    let mut normals = Vec::with_capacity(count_x * count_z);
    let mut colors = Vec::with_capacity(count_x * count_z);
    for z in 0..count_z {
        let world_z = origin_z + (usize_as_f64(z) * spacing);
        for x in 0..count_x {
            let world_x = origin_x + (usize_as_f64(x) * spacing);
            let region_index = WatershedRegionIndex::containing(world_x, world_z)
                .ok_or_else(|| std::io::Error::other("invalid drainage coordinate"))?;
            if let std::collections::btree_map::Entry::Vacant(entry) = regions.entry(region_index) {
                let region = WatershedRegion::generate(world, region_index)
                    .ok_or_else(|| std::io::Error::other("failed to generate watershed region"))?;
                let rivers = RiverNetwork::from_watershed(&region)
                    .ok_or_else(|| std::io::Error::other("failed to generate river network"))?;
                entry.insert(region);
                river_networks.insert(region_index, rivers);
            }
            let cell = regions[&region_index]
                .cell_at(world_x, world_z)
                .ok_or_else(|| std::io::Error::other("missing drainage cell"))?;
            let river = river_networks[&region_index]
                .segment_from(cell.index)
                .copied();
            positions.push([f64_as_f32(world_x), 0.0, f64_as_f32(world_z)]);
            normals.push([0.0, 1.0, 0.0]);
            colors.push(drainage_color(world, *cell, river, mode));
        }
    }

    let mut indices = Vec::with_capacity(columns * GRID_ROWS * 6);
    for z in 0..GRID_ROWS {
        for x in 0..columns {
            let top_left = z * count_x + x;
            let bottom_left = top_left + count_x;
            let top_right = top_left + 1;
            let bottom_right = bottom_left + 1;
            indices.extend([
                u32::try_from(top_left)?,
                u32::try_from(bottom_left)?,
                u32::try_from(top_right)?,
                u32::try_from(top_right)?,
                u32::try_from(bottom_left)?,
                u32::try_from(bottom_right)?,
            ]);
        }
    }

    Ok(Mesh {
        positions,
        normals,
        colors,
        indices,
    })
}

fn grid_dimensions(span_meters: f64, width: u32, height: u32) -> (usize, f64) {
    let spacing = span_meters / usize_as_f64(GRID_ROWS);
    let columns = usize::try_from(
        (u64::from(width) * u64::try_from(GRID_ROWS).expect("grid row count fits u64"))
            .div_ceil(u64::from(height.max(1))),
    )
    .unwrap_or(GRID_ROWS)
    .max(1);
    (columns, spacing)
}

fn drainage_color(
    world: WorldIdentity,
    cell: DrainageCell,
    river: Option<RiverSegment>,
    mode: ViewMode,
) -> [f32; 4] {
    match mode {
        ViewMode::Terrain => [1.0, 1.0, 1.0, 0.0],
        ViewMode::Watersheds => {
            let key = CellIndex::new(cell.watershed_outlet.x, cell.watershed_outlet.z, 0)
                .generation_key(world, DOMAIN_WATERSHED_COLOR);
            [
                hash_channel(key, 0),
                hash_channel(key, 8),
                hash_channel(key, 16),
                1.0,
            ]
        }
        ViewMode::FlowAccumulation => {
            let strength = (u64_as_f32(cell.flow_accumulation_cells).log2() / 12.0).clamp(0.0, 1.0);
            let dry = [0.28, 0.24, 0.16];
            let wet = if cell.basin.is_some() {
                [0.05, 0.72, 0.78]
            } else {
                [0.04, 0.38, 0.92]
            };
            [
                lerp_f32(dry[0], wet[0], strength),
                lerp_f32(dry[1], wet[1], strength),
                lerp_f32(dry[2], wet[2], strength),
                1.0,
            ]
        }
        ViewMode::Rivers => river.map_or([0.24, 0.21, 0.15, 1.0], |segment| {
            let strength = ((f64_as_f32(segment.discharge_cubic_meters_per_second).log2() + 4.0)
                / 10.0)
                .clamp(0.0, 1.0);
            [
                lerp_f32(0.08, 0.02, strength),
                lerp_f32(0.42, 0.72, strength),
                lerp_f32(0.72, 1.0, strength),
                1.0,
            ]
        }),
    }
}

fn hash_channel(key: u64, shift: u32) -> f32 {
    let byte = u8::try_from((key >> shift) & 0xff).expect("masked hash lane fits u8");
    0.25 + (f32::from(byte) / 255.0 * 0.65)
}

fn lerp_f32(start: f32, end: f32, amount: f32) -> f32 {
    start + ((end - start) * amount)
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[allow(clippy::cast_precision_loss)]
fn u64_as_f32(value: u64) -> f32 {
    value as f32
}
