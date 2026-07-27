use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use treeline_coordinates::{CellIndex, WorldIdentity};
use treeline_ecology::{
    ForestDistribution, ForestSample, ProceduralTree, ProceduralTrees, RockBounds, Soil,
    SoilSample, SurfaceRock, SurfaceRockDistribution, SurfaceRockSample, SurfaceRocks, TreeBounds,
};
use treeline_geography::{
    Climate, ClimateSample, DrainageCell, RegionalProfile, Season, SeasonalClimateSample,
    WatershedRegion, WatershedRegionIndex,
};
use treeline_hydrology::{Lake, LakeNetwork, RiverNetwork, RiverSegment};
use treeline_mesher::{Mesh, SurfaceGridSpec, surface_grid};
use treeline_renderer::{TerrainMesh, TerrainRenderer};
use treeline_terrain::{SurfaceField, WildernessTerrain};
use treeline_world::{CURRENT_GENERATOR_VERSION, GeneratedWorldTerrain, WorldErosionSample};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

const GENERATOR_VERSION: u32 = CURRENT_GENERATOR_VERSION;
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
    Lakes,
    Erosion,
    Temperature,
    Precipitation,
    Snowpack,
    Soil,
    Forest,
    SurfaceRocks,
}

impl ViewMode {
    const ALL: [Self; 12] = [
        Self::Terrain,
        Self::Watersheds,
        Self::FlowAccumulation,
        Self::Rivers,
        Self::Lakes,
        Self::Erosion,
        Self::Temperature,
        Self::Precipitation,
        Self::Snowpack,
        Self::Soil,
        Self::Forest,
        Self::SurfaceRocks,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Terrain => "terrain",
            Self::Watersheds => "watersheds",
            Self::FlowAccumulation => "flow accumulation",
            Self::Rivers => "rivers",
            Self::Lakes => "lakes",
            Self::Erosion => "erosion",
            Self::Temperature => "temperature",
            Self::Precipitation => "precipitation",
            Self::Snowpack => "snowpack",
            Self::Soil => "soil",
            Self::Forest => "forest distribution",
            Self::SurfaceRocks => "surface rocks",
        }
    }

    const fn shortcut(self) -> &'static str {
        match self {
            Self::Terrain => "1",
            Self::Watersheds => "2",
            Self::FlowAccumulation => "3",
            Self::Rivers => "4",
            Self::Lakes => "5",
            Self::Erosion => "6",
            Self::Temperature => "7",
            Self::Precipitation => "8",
            Self::Snowpack => "9",
            Self::Soil => "0",
            Self::Forest => "F",
            Self::SurfaceRocks => "G",
        }
    }

    const fn is_environment_layer(self) -> bool {
        matches!(
            self,
            Self::Temperature
                | Self::Precipitation
                | Self::Snowpack
                | Self::Soil
                | Self::Forest
                | Self::SurfaceRocks
        )
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
        let egui_consumed = lab.handle_window_event(&event);
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
            WindowEvent::MouseWheel { delta, .. } if !egui_consumed => {
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
            } if !egui_consumed => match button {
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
                    } else if !egui_consumed {
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
    egui_context: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    seed: u64,
    center: [f64; 2],
    span_meters: f64,
    cursor: PhysicalPosition<f64>,
    mode: ViewMode,
    season: Season,
    inspection: Option<String>,
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
        let egui_context = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(f64_as_f32(window.scale_factor())),
            window.theme(),
            usize::try_from(device.limits().max_texture_dimension_2d).ok(),
        );
        let egui_renderer = egui_wgpu::Renderer::new(&device, surface_format, None, 1, false);
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
            Season::default(),
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
            egui_context,
            egui_state,
            egui_renderer,
            seed,
            center,
            span_meters,
            cursor,
            mode,
            season: Season::default(),
            inspection: None,
        };
        lab.update_camera();
        lab.update_title();
        lab.window.request_redraw();
        Ok(lab)
    }

    fn handle_window_event(&mut self, event: &WindowEvent) -> bool {
        let response = self.egui_state.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
        response.consumed
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
            KeyCode::KeyC => {
                self.season = self.season.next();
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
            KeyCode::Digit5 => {
                self.mode = ViewMode::Lakes;
                true
            }
            KeyCode::Digit6 => {
                self.mode = ViewMode::Erosion;
                true
            }
            KeyCode::Digit7 => {
                self.mode = ViewMode::Temperature;
                true
            }
            KeyCode::Digit8 => {
                self.mode = ViewMode::Precipitation;
                true
            }
            KeyCode::Digit9 => {
                self.mode = ViewMode::Snowpack;
                true
            }
            KeyCode::Digit0 => {
                self.mode = ViewMode::Soil;
                true
            }
            KeyCode::KeyF => {
                self.mode = ViewMode::Forest;
                true
            }
            KeyCode::KeyG => {
                self.mode = ViewMode::SurfaceRocks;
                true
            }
            _ => false,
        };
        self.span_meters = self.span_meters.clamp(MIN_SPAN_METERS, MAX_SPAN_METERS);
        if changed && let Err(error) = self.regenerate() {
            eprintln!("failed to update Generator Lab view: {error}");
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
            self.season,
        )?;
        self.mesh = self.renderer.upload_mesh(&self.device, &mesh_data)?;
        self.update_camera();
        self.update_title();
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

    fn inspect_cursor(&mut self) {
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
        let erosion = generated_terrain.erosion_at(x, z);
        let Some(profile) = RegionalProfile::sample(world, x, z) else {
            return;
        };
        let Some(climate) = Climate::new(world).sample(x, z) else {
            return;
        };
        let Some(seasonal_climate) = Climate::new(world).sample_season(x, z, self.season) else {
            return;
        };
        let Some(soil) = Soil::new(world).sample(x, z) else {
            return;
        };
        let Some(forest) = ForestDistribution::new(world).sample(x, z) else {
            return;
        };
        let Some(rock_distribution) = SurfaceRockDistribution::new(world).sample(x, z) else {
            return;
        };
        let Some(tree_inspection) = inspect_nearby_trees(world, x, z) else {
            return;
        };
        let Some(rock_inspection) = inspect_nearby_rocks(world, x, z) else {
            return;
        };
        let watershed = WatershedRegionIndex::containing(x, z)
            .and_then(|index| WatershedRegion::generate(world, index));
        let drainage = watershed
            .as_ref()
            .and_then(|region| region.cell_at(x, z).copied());
        let river_network = watershed.as_ref().and_then(RiverNetwork::from_watershed);
        let lake_network = watershed.as_ref().and_then(LakeNetwork::from_watershed);
        let river = drainage.and_then(|cell| {
            river_network
                .as_ref()
                .and_then(|network| network.segment_from(cell.index))
                .copied()
        });
        let lake = drainage.and_then(|cell| {
            lake_network
                .as_ref()
                .and_then(|network| network.lake_for_cell(cell.index))
        });
        let lake_surface = generated_terrain.lake_surface_at(x, z);
        let drainage_summary = describe_drainage(drainage, river, lake);
        let tree_summary = tree_inspection.summary();
        let rock_summary = rock_inspection.summary();
        let summary = format!(
            "x {x:.0} m, z {z:.0} m | height {carved_surface_height:.0} m | {} {:.1} °C | snow {:.0} mm | {:.0} mm/yr | {} {:.1} pH, {:.0}% moist | forest {:.0}% {}, {:.0} yr | {} stems/1,024 m², nearest {tree_summary} | rocks {:.0}/ha, {} nearby, nearest {rock_summary} | ridge +{:.0} m | {drainage_summary}",
            self.season.label(),
            seasonal_climate.mean_temperature_celsius,
            seasonal_climate.snowpack_water_equivalent_millimeters,
            climate.annual_precipitation_millimeters,
            soil.texture.label(),
            soil.acidity_ph,
            soil.surface_moisture * 100.0,
            forest.canopy_cover_fraction * 100.0,
            forest.dominant_group().label(),
            forest.stand_age_years,
            tree_inspection.count,
            rock_distribution.density_per_hectare,
            rock_inspection.count,
            macro_sample.mountain_uplift_meters,
        );
        eprintln!(
            "Generator Lab inspection\ncoordinate: ({x:.2}, {z:.2})\nbase surface height: {surface_height:.2} m\nshaped surface height: {carved_surface_height:.2} m\nmacro terrain: {macro_sample:#?}\nregional profile: {profile:#?}\nannual climate: {climate:#?}\nseasonal climate: {seasonal_climate:#?}\nsoil: {soil:#?}\nforest: {forest:#?}\nsurface-rock distribution: {rock_distribution:#?}\nnearby procedural tree count: {}\nnearest procedural tree: {nearest_tree:#?}\nnearby surface-rock count: {}\nnearest surface rock: {nearest_rock:#?}\ndrainage cell: {drainage:#?}\nriver segment: {river:#?}\nriver terrain influence: {river_influence:#?}\nerosion: {erosion:#?}\nlake: {lake:#?}\nlake surface: {lake_surface:#?}",
            tree_inspection.count,
            rock_inspection.count,
            nearest_tree = tree_inspection.nearest,
            nearest_rock = rock_inspection.nearest,
        );
        self.inspection = Some(summary);
        self.window.request_redraw();
    }

    fn update_title(&self) {
        self.window.set_title(&format!(
            "Treeline Generator Lab — {} ({}) — seed {:x}",
            self.mode.label(),
            self.season.label(),
            self.seed,
        ));
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let context = self.egui_context.clone();
        let snapshot = LabUiSnapshot {
            mode: self.mode,
            season: self.season,
            seed: self.seed,
            center: self.center,
            span_meters: self.span_meters,
            cursor_world: self.cursor_world_position(),
            inspection: self.inspection.as_deref(),
        };
        let mut ui_action = LabUiAction::default();
        let full_output = context.run(raw_input, |context| {
            draw_generator_lab_ui(context, snapshot, &mut ui_action);
        });
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        self.apply_ui_action(ui_action);

        let paint_jobs = context.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        for (texture_id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *texture_id, image_delta);
        }

        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Generator Lab frame encoder"),
            });
        let mut command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        self.renderer.render(
            &mut encoder,
            &view,
            std::iter::empty(),
            std::iter::once(&self.mesh),
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Generator Lab UI render pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            self.egui_renderer
                .render(&mut pass, &paint_jobs, &screen_descriptor);
        }
        command_buffers.push(encoder.finish());
        self.queue.submit(command_buffers);
        for texture_id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(texture_id);
        }
        frame.present();
        Ok(())
    }

    fn apply_ui_action(&mut self, action: LabUiAction) {
        let mut changed = false;
        if let Some(mode) = action.mode
            && mode != self.mode
        {
            self.mode = mode;
            changed = true;
        }
        if action.next_season {
            self.season = self.season.next();
            changed = true;
        }
        if action.seed_delta < 0 {
            self.seed = self.seed.wrapping_sub(1);
            changed = true;
        } else if action.seed_delta > 0 {
            self.seed = self.seed.wrapping_add(1);
            changed = true;
        }
        if action.reset_center {
            self.center = [0.0, 0.0];
            changed = true;
        }
        if action.pan != [0, 0] {
            let step = self.span_meters * 0.15;
            self.center[0] += f64::from(action.pan[0]) * step;
            self.center[1] += f64::from(action.pan[1]) * step;
            changed = true;
        }
        if let Some(factor) = action.zoom_factor {
            self.span_meters *= factor;
            changed = true;
        }
        self.span_meters = self.span_meters.clamp(MIN_SPAN_METERS, MAX_SPAN_METERS);
        if changed && let Err(error) = self.regenerate() {
            eprintln!("failed to update Generator Lab from UI: {error}");
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct NearbyTreeInspection {
    count: usize,
    nearest: Option<ProceduralTree>,
}

impl NearbyTreeInspection {
    fn summary(self) -> String {
        self.nearest.map_or_else(
            || "no nearby tree".to_owned(),
            |tree| {
                format!(
                    "{} {:.1} m {}",
                    tree.condition.label(),
                    tree.height_meters,
                    tree.genotype.functional_group.label()
                )
            },
        )
    }
}

fn inspect_nearby_trees(world: WorldIdentity, x: f64, z: f64) -> Option<NearbyTreeInspection> {
    let bounds = TreeBounds::new(x - 16.0, z - 16.0, x + 16.0, z + 16.0)?;
    let trees = ProceduralTrees::new(world).trees_in(bounds)?;
    let nearest = trees.iter().min_by(|left, right| {
        let left_distance = libm::fma(left.x - x, left.x - x, (left.z - z) * (left.z - z));
        let right_distance = libm::fma(right.x - x, right.x - x, (right.z - z) * (right.z - z));
        left_distance.total_cmp(&right_distance)
    });
    Some(NearbyTreeInspection {
        count: trees.len(),
        nearest: nearest.copied(),
    })
}

#[derive(Clone, Copy, Debug)]
struct NearbyRockInspection {
    count: usize,
    nearest: Option<SurfaceRock>,
}

impl NearbyRockInspection {
    fn summary(self) -> String {
        self.nearest.map_or_else(
            || "no nearby rock".to_owned(),
            |rock| {
                format!(
                    "{} {:.1}×{:.1}×{:.1} m",
                    rock.genotype.form.label(),
                    rock.radii_meters[0] * 2.0,
                    rock.radii_meters[1] * 2.0,
                    rock.radii_meters[2] * 2.0,
                )
            },
        )
    }
}

fn inspect_nearby_rocks(world: WorldIdentity, x: f64, z: f64) -> Option<NearbyRockInspection> {
    let bounds = RockBounds::new(x - 16.0, z - 16.0, x + 16.0, z + 16.0)?;
    let rocks = SurfaceRocks::new(world).rocks_in(bounds)?;
    let nearest = rocks.iter().min_by(|left, right| {
        let left_distance = libm::fma(left.x - x, left.x - x, (left.z - z) * (left.z - z));
        let right_distance = libm::fma(right.x - x, right.x - x, (right.z - z) * (right.z - z));
        left_distance.total_cmp(&right_distance)
    });
    Some(NearbyRockInspection {
        count: rocks.len(),
        nearest: nearest.copied(),
    })
}

fn describe_drainage(
    drainage: Option<DrainageCell>,
    river: Option<RiverSegment>,
    lake: Option<Lake>,
) -> String {
    drainage.map_or_else(
        || "drainage unavailable".to_owned(),
        |cell| {
            let river_summary = river.map_or_else(String::new, |segment| {
                format!(
                    " | river {:.2} m³/s | {:.0} km²",
                    segment.discharge_cubic_meters_per_second,
                    segment.drainage_area_square_kilometers
                )
            });
            let lake_summary = lake.map_or_else(String::new, |lake| {
                format!(
                    " | lake {:x} at {:.1} m",
                    lake.id, lake.surface_elevation_meters
                )
            });
            format!(
                "flow {} cells | outlet ({}, {}){}{}{}",
                cell.flow_accumulation_cells,
                cell.watershed_outlet.x,
                cell.watershed_outlet.z,
                if cell.basin.is_some() { " | basin" } else { "" },
                river_summary,
                lake_summary
            )
        },
    )
}

#[derive(Clone, Copy)]
struct LabUiSnapshot<'inspection> {
    mode: ViewMode,
    season: Season,
    seed: u64,
    center: [f64; 2],
    span_meters: f64,
    cursor_world: [f64; 2],
    inspection: Option<&'inspection str>,
}

#[derive(Clone, Copy, Debug, Default)]
struct LabUiAction {
    mode: Option<ViewMode>,
    next_season: bool,
    seed_delta: i8,
    pan: [i8; 2],
    zoom_factor: Option<f64>,
    reset_center: bool,
}

fn draw_generator_lab_ui(
    context: &egui::Context,
    snapshot: LabUiSnapshot<'_>,
    action: &mut LabUiAction,
) {
    egui::TopBottomPanel::top("generator_lab_status").show(context, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Treeline Generator Lab");
            ui.separator();
            ui.label(format!("View: {}", snapshot.mode.label()));
            ui.separator();
            ui.label(format!("Season: {}", snapshot.season.label()));
            ui.separator();
            ui.monospace(format!("Seed: {:016x}", snapshot.seed));
            ui.separator();
            ui.label(format!(
                "Center: ({:.1}, {:.1}) km",
                snapshot.center[0] / 1_000.0,
                snapshot.center[1] / 1_000.0
            ));
            ui.separator();
            ui.label(format!("Span: {:.0} km", snapshot.span_meters / 1_000.0));
        });
    });

    egui::SidePanel::left("generator_lab_controls")
        .resizable(false)
        .default_width(240.0)
        .show(context, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("View layers");
                ui.add_space(4.0);
                for mode in ViewMode::ALL {
                    let label = format!("{}   {}", mode.shortcut(), mode.label());
                    if ui.selectable_label(snapshot.mode == mode, label).clicked() {
                        action.mode = Some(mode);
                    }
                }

                draw_climate_season_ui(ui, snapshot.season, action);
                ui.separator();
                ui.heading("Navigate");
                ui.add_space(4.0);
                egui::Grid::new("generator_lab_navigation")
                    .spacing([6.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("");
                        if ui.add_sized([58.0, 28.0], egui::Button::new("↑")).clicked() {
                            action.pan[1] = -1;
                        }
                        ui.end_row();

                        if ui.add_sized([58.0, 28.0], egui::Button::new("←")).clicked() {
                            action.pan[0] = -1;
                        }
                        if ui
                            .add_sized([58.0, 28.0], egui::Button::new("Home"))
                            .clicked()
                        {
                            action.reset_center = true;
                        }
                        if ui.add_sized([58.0, 28.0], egui::Button::new("→")).clicked() {
                            action.pan[0] = 1;
                        }
                        ui.end_row();

                        ui.label("");
                        if ui.add_sized([58.0, 28.0], egui::Button::new("↓")).clicked() {
                            action.pan[1] = 1;
                        }
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    if ui.button("−  Zoom out").clicked() {
                        action.zoom_factor = Some(1.4);
                    }
                    if ui.button("+  Zoom in").clicked() {
                        action.zoom_factor = Some(0.7);
                    }
                });
                ui.label(format!(
                    "Cursor: ({:.1}, {:.1}) km",
                    snapshot.cursor_world[0] / 1_000.0,
                    snapshot.cursor_world[1] / 1_000.0
                ));

                ui.separator();
                ui.heading("World seed");
                ui.monospace(format!("{:016x}", snapshot.seed));
                ui.horizontal(|ui| {
                    if ui.button("← Previous").clicked() {
                        action.seed_delta = -1;
                    }
                    if ui.button("Next →").clicked() {
                        action.seed_delta = 1;
                    }
                });

                draw_keyboard_help(ui);

                if let Some(inspection) = snapshot.inspection {
                    ui.separator();
                    ui.heading("Inspection");
                    ui.label(inspection);
                }
            });
        });
}

fn draw_climate_season_ui(ui: &mut egui::Ui, season: Season, action: &mut LabUiAction) {
    ui.separator();
    ui.heading("Climate season");
    ui.label(season.label());
    if ui.button("Next season (C)").clicked() {
        action.next_season = true;
    }
}

fn draw_keyboard_help(ui: &mut egui::Ui) {
    ui.separator();
    ui.heading("Keyboard & mouse");
    for help in [
        "1–9  Select view layer",
        "0 / F / G  Soil / forest / surface rocks",
        "C  Advance climate season",
        "WASD / arrows  Pan",
        "+ / − / wheel  Zoom",
        "R  Next seed",
        "T / right-click  Center on cursor",
        "Left-click terrain  Inspect",
        "Esc  Quit",
    ] {
        ui.label(help);
    }
}

fn generate_mesh(
    seed: u64,
    center: [f64; 2],
    span_meters: f64,
    width: u32,
    height: u32,
    mode: ViewMode,
    season: Season,
) -> Result<treeline_mesher::Mesh, Box<dyn Error>> {
    if mode != ViewMode::Terrain {
        return generate_drainage_mesh(seed, center, span_meters, width, height, mode, season);
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
    season: Season,
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
    let generated_terrain = (mode == ViewMode::Erosion).then(|| GeneratedWorldTerrain::new(world));
    let environment_layers = EnvironmentLayers {
        climate: Climate::new(world),
        soil: Soil::new(world),
        forest: ForestDistribution::new(world),
        surface_rocks: SurfaceRockDistribution::new(world),
    };
    let mut regions = BTreeMap::new();
    let mut river_networks = BTreeMap::new();
    let mut positions = Vec::with_capacity(count_x * count_z);
    let mut normals = Vec::with_capacity(count_x * count_z);
    let mut colors = Vec::with_capacity(count_x * count_z);
    for z in 0..count_z {
        let world_z = origin_z + (usize_as_f64(z) * spacing);
        for x in 0..count_x {
            let world_x = origin_x + (usize_as_f64(x) * spacing);
            positions.push([f64_as_f32(world_x), 0.0, f64_as_f32(world_z)]);
            normals.push([0.0, 1.0, 0.0]);
            if mode.is_environment_layer() {
                colors.push(
                    environment_layer_color(mode, season, world_x, world_z, environment_layers)
                        .ok_or_else(|| {
                            std::io::Error::other("failed to sample environment layer")
                        })?,
                );
                continue;
            }
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
            let erosion = generated_terrain
                .as_ref()
                .and_then(|terrain| terrain.erosion_at(world_x, world_z));
            colors.push(drainage_color(world, *cell, river, erosion.as_ref(), mode));
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

#[derive(Clone, Copy, Debug)]
struct EnvironmentLayers {
    climate: Climate,
    soil: Soil,
    forest: ForestDistribution,
    surface_rocks: SurfaceRockDistribution,
}

fn environment_layer_color(
    mode: ViewMode,
    season: Season,
    x: f64,
    z: f64,
    layers: EnvironmentLayers,
) -> Option<[f32; 4]> {
    match mode {
        ViewMode::Temperature | ViewMode::Precipitation | ViewMode::Snowpack => {
            Some(climate_color(
                layers.climate.sample(x, z)?,
                layers.climate.sample_season(x, z, season)?,
                mode,
            ))
        }
        ViewMode::Soil => Some(soil_color(layers.soil.sample(x, z)?)),
        ViewMode::Forest => Some(forest_color(layers.forest.sample(x, z)?)),
        ViewMode::SurfaceRocks => Some(surface_rock_color(layers.surface_rocks.sample(x, z)?)),
        ViewMode::Terrain
        | ViewMode::Watersheds
        | ViewMode::FlowAccumulation
        | ViewMode::Rivers
        | ViewMode::Lakes
        | ViewMode::Erosion => None,
    }
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
    erosion: Option<&WorldErosionSample>,
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
            let strength =
                (libm::log2f(u64_as_f32(cell.flow_accumulation_cells)) / 12.0).clamp(0.0, 1.0);
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
            let strength =
                ((libm::log2f(f64_as_f32(segment.discharge_cubic_meters_per_second)) + 4.0) / 10.0)
                    .clamp(0.0, 1.0);
            [
                lerp_f32(0.08, 0.02, strength),
                lerp_f32(0.42, 0.72, strength),
                lerp_f32(0.72, 1.0, strength),
                1.0,
            ]
        }),
        ViewMode::Lakes => {
            let depth = f64_as_f32((cell.filled_elevation_meters - cell.elevation_meters).max(0.0));
            if cell.basin.is_some() {
                let strength = (depth / 80.0).clamp(0.0, 1.0);
                [
                    lerp_f32(0.08, 0.01, strength),
                    lerp_f32(0.52, 0.20, strength),
                    lerp_f32(0.76, 0.48, strength),
                    1.0,
                ]
            } else {
                [0.24, 0.21, 0.15, 1.0]
            }
        }
        ViewMode::Erosion => erosion.map_or([0.08, 0.08, 0.08, 1.0], |erosion| {
            let weathering =
                f64_as_f32(erosion.surface.macro_weathering_meters / 120.0).clamp(0.0, 1.0);
            let deposition =
                f64_as_f32(erosion.surface.sediment_deposition_meters / 18.0).clamp(0.0, 1.0);
            let drainage = erosion
                .gully
                .map_or(0.0, |gully| {
                    f64_as_f32(gully.blend * gully.segment.incision_depth_meters / 14.0)
                })
                .max(erosion.river.map_or(0.0, |river| {
                    f64_as_f32(river.blend * river.incision_depth_meters / 48.0)
                }))
                .clamp(0.0, 1.0);
            [
                0.08 + (weathering * 0.85),
                0.08 + (deposition * 0.78),
                0.08 + (drainage * 0.92),
                1.0,
            ]
        }),
        ViewMode::Temperature
        | ViewMode::Precipitation
        | ViewMode::Snowpack
        | ViewMode::Soil
        | ViewMode::Forest
        | ViewMode::SurfaceRocks => [1.0, 0.0, 1.0, 1.0],
    }
}

fn climate_color(
    annual: ClimateSample,
    seasonal: SeasonalClimateSample,
    mode: ViewMode,
) -> [f32; 4] {
    match mode {
        ViewMode::Temperature => {
            let warmth =
                f64_as_f32((seasonal.mean_temperature_celsius + 35.0) / 70.0).clamp(0.0, 1.0);
            let temperate = 1.0 - ((warmth - 0.5).abs() * 2.0);
            [
                lerp_f32(0.08, 0.92, warmth),
                0.18 + (temperate * 0.62),
                lerp_f32(0.92, 0.08, warmth),
                1.0,
            ]
        }
        ViewMode::Precipitation => {
            let moisture = f64_as_f32(seasonal.precipitation_millimeters / 900.0).clamp(0.0, 1.0);
            [
                lerp_f32(0.62, 0.04, moisture),
                lerp_f32(0.34, 0.64, moisture),
                lerp_f32(0.10, 0.92, moisture),
                1.0,
            ]
        }
        ViewMode::Snowpack => {
            let snow = f64_as_f32(seasonal.snowpack_water_equivalent_millimeters / 1_200.0)
                .clamp(0.0, 1.0);
            let permanent =
                f64_as_f32(annual.permanent_snowpack_water_equivalent_millimeters / 1_200.0)
                    .clamp(0.0, 1.0);
            [
                lerp_f32(0.20, 0.94 - (permanent * 0.08), snow),
                lerp_f32(0.18, 0.98, snow),
                lerp_f32(0.14, 1.0, snow),
                1.0,
            ]
        }
        _ => [1.0, 0.0, 1.0, 1.0],
    }
}

fn soil_color(soil: SoilSample) -> [f32; 4] {
    let sand = f64_as_f32(soil.composition.sand_fraction);
    let silt = f64_as_f32(soil.composition.silt_fraction);
    let clay = f64_as_f32(soil.composition.clay_fraction);
    let moisture = f64_as_f32(soil.surface_moisture);
    let organic = f64_as_f32(soil.organic_matter_fraction / 0.17);
    let mineral = [
        (sand * 0.76) + (silt * 0.50) + (clay * 0.58),
        (sand * 0.62) + (silt * 0.49) + (clay * 0.30),
        (sand * 0.34) + (silt * 0.43) + (clay * 0.22),
    ];
    let darkening = 1.0 - (organic * 0.45);
    [
        mineral[0] * darkening * (1.0 - (moisture * 0.18)),
        mineral[1] * darkening,
        (mineral[2] * darkening) + (moisture * 0.16),
        1.0,
    ]
}

fn forest_color(forest: ForestSample) -> [f32; 4] {
    let composition = forest.composition;
    let evergreen = f64_as_f32(composition.evergreen_needleleaf_fraction);
    let cold_deciduous = f64_as_f32(composition.cold_deciduous_fraction);
    let temperate = f64_as_f32(composition.temperate_broadleaf_fraction);
    let dry = f64_as_f32(composition.dry_woodland_fraction);
    let canopy = f64_as_f32(forest.canopy_cover_fraction);
    let disturbance = f64_as_f32(forest.disturbance_severity);
    let forest_tint = [
        (evergreen * 0.04) + (cold_deciduous * 0.24) + (temperate * 0.10) + (dry * 0.38),
        (evergreen * 0.22) + (cold_deciduous * 0.43) + (temperate * 0.36) + (dry * 0.42),
        (evergreen * 0.12) + (cold_deciduous * 0.15) + (temperate * 0.09) + (dry * 0.17),
    ];
    let bare = [0.62, 0.53, 0.33];
    let visible_cover = (canopy * 1.15).clamp(0.0, 1.0);
    [
        lerp_f32(bare[0], forest_tint[0], visible_cover) + (disturbance * 0.10),
        lerp_f32(bare[1], forest_tint[1], visible_cover) - (disturbance * 0.08),
        lerp_f32(bare[2], forest_tint[2], visible_cover) - (disturbance * 0.04),
        1.0,
    ]
}

fn surface_rock_color(rocks: SurfaceRockSample) -> [f32; 4] {
    let density = f64_as_f32(rocks.density_per_hectare / 2_100.0).clamp(0.0, 1.0);
    let exposure = f64_as_f32(rocks.rock_exposure_fraction);
    let scree = f64_as_f32(rocks.scree_cover_fraction);
    let hardness = f64_as_f32(rocks.hardness_fraction);
    let carbonate = f64_as_f32(rocks.carbonate_fraction);
    [
        (0.12 + (density * 0.48) + (carbonate * 0.22)).clamp(0.0, 1.0),
        (0.10 + (scree * 0.52) + (carbonate * 0.16)).clamp(0.0, 1.0),
        (0.09 + (exposure * 0.34) + (hardness * 0.34)).clamp(0.0, 1.0),
        1.0,
    ]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forest_view_generates_varied_opaque_coverage_colors() {
        let mesh = generate_mesh(
            0x5eed,
            [0.0, 0.0],
            64_000.0,
            64,
            64,
            ViewMode::Forest,
            Season::Summer,
        )
        .expect("forest view mesh");
        let first = mesh.colors[0];

        assert_eq!(mesh.colors.len(), mesh.positions.len());
        assert!(
            mesh.colors
                .iter()
                .all(|color| (color[3] - 1.0).abs() < f32::EPSILON)
        );
        assert!(mesh.colors.iter().any(|color| {
            color
                .iter()
                .zip(first)
                .map(|(channel, first_channel)| (channel - first_channel).abs())
                .sum::<f32>()
                > 0.01
        }));
        assert!(mesh.colors.iter().all(|color| {
            color[..3]
                .iter()
                .all(|channel| (0.0..=1.0).contains(channel))
        }));
    }

    #[test]
    fn surface_rock_view_generates_varied_opaque_distribution_colors() {
        let mesh = generate_mesh(
            0x5eed,
            [0.0, 0.0],
            64_000.0,
            64,
            64,
            ViewMode::SurfaceRocks,
            Season::Summer,
        )
        .expect("surface-rock view mesh");
        let first = mesh.colors[0];

        assert_eq!(mesh.colors.len(), mesh.positions.len());
        assert!(
            mesh.colors
                .iter()
                .all(|color| (color[3] - 1.0).abs() < f32::EPSILON)
        );
        assert!(mesh.colors.iter().any(|color| {
            color
                .iter()
                .zip(first)
                .map(|(channel, first_channel)| (channel - first_channel).abs())
                .sum::<f32>()
                > 0.01
        }));
        assert!(mesh.colors.iter().all(|color| {
            color[..3]
                .iter()
                .all(|channel| (0.0..=1.0).contains(channel))
        }));
    }
}
