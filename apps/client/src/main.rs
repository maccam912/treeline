use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
mod browser_terrain;

#[cfg(any(test, target_arch = "wasm32"))]
use std::cell::Cell;
#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

#[cfg(test)]
use glam::DVec2;
use glam::{DVec3, Mat4, Vec2, Vec3};
use treeline_coordinates::stable_hash;
use treeline_coordinates::{CellIndex, WorldIdentity, WorldPosition};
use treeline_ecology::{
    GroundVegetation, GroundVegetationBounds, ProceduralTree, ProceduralTrees, RockBounds, Soil,
    SurfaceRocks, TreeBounds,
};
use treeline_geography::Climate;
use treeline_mesher::Mesh;
use treeline_renderer::{
    AtmosphereSettings, LightingSettings, TerrainMesh, TerrainRenderer, TimeOfDay, TreeMeshDetail,
};
use treeline_simulation::{ActiveRegionId, ActiveWaterSimulation};
use treeline_terrain::DEFAULT_SURVEYED_TILE_EDGE_METERS;
#[cfg(test)]
use treeline_terrain::WildernessTerrain;
#[cfg(not(test))]
use treeline_terrain::{DEFAULT_SURVEYED_START_X, DEFAULT_SURVEYED_START_Z};
use treeline_terrain::{DensityField, SurfaceField};
use treeline_voxel::ChunkIndex;
#[cfg(test)]
use treeline_world::CURRENT_GENERATOR_VERSION;
#[cfg(not(test))]
use treeline_world::DEFAULT_WORLD_IDENTITY;
#[cfg(not(target_arch = "wasm32"))]
use treeline_world::TerrainMeshQueue;
use treeline_world::{
    ActiveWaterRegionSpec, ChunkMeshSpec, ChunkStreamer, ChunkStreamingConfig, FarTerrainMeshSpec,
    FarTerrainStreamer, FarTerrainStreamingConfig, FarTileIndex, GeneratedWorldTerrain,
    GenerationPriority, Lake, NearTerrainCutout, Season, TerrainMeshSpec,
};
use web_time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, Touch, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
#[cfg(not(target_arch = "wasm32"))]
use winit::window::CursorGrabMode;
use winit::window::{Window, WindowId};

#[cfg(not(test))]
const WORLD: WorldIdentity = DEFAULT_WORLD_IDENTITY;
#[cfg(test)]
const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, CURRENT_GENERATOR_VERSION, 0);
const WINDOW_TITLE: &str = "Treeline — Surveyed Wilderness";
const EYE_HEIGHT: f64 = 1.72;
const WALK_SPEED: f64 = 1.4;
const SPRINT_SPEED: f64 = 4.5;
const AERIAL_HEIGHT_METERS: f64 = 200.0;
const AERIAL_SPEED_MULTIPLIER: f64 = 10.0;
// 46.16084629042455, -88.3374704874157 in the tile's right-handed local frame.
#[cfg(not(test))]
const START_X: f64 = DEFAULT_SURVEYED_START_X;
#[cfg(not(test))]
const START_Z: f64 = DEFAULT_SURVEYED_START_Z;
#[cfg(test)]
const START_X: f64 = 26_176_064.0;
#[cfg(test)]
const START_Z: f64 = 39_040_066.0;
const START_YAW: f64 = -1.924_842_228_418_599_5;
const START_PITCH: f64 = -0.08;
const RANDOM_WARP_MIN_DISTANCE_METERS: f64 = 1_000_000.0;
const RANDOM_WARP_MAX_DISTANCE_METERS: f64 = 5_000_000.0;
const RANDOM_WARP_COORDINATE_LIMIT_METERS: f64 = 5_000_000.0;
const RANDOM_WARP_SITE_ATTEMPTS: usize = 64;
const SURVEYED_WARP_BORDER_CLEARANCE_METERS: f64 = 64.0;
const WATER_WARP_REGION_ATTEMPTS: usize = 16;
const WATER_WARP_DIRECTIONS: u32 = 16;
const WATER_WARP_MAX_SHORE_DISTANCE_METERS: f64 = 128_000.0;
const WATER_WARP_MIN_DEPTH_METERS: f64 = 0.5;
const WATER_WARP_SHORE_CLEARANCE_METERS: f64 = 8.0;
const ACTIVE_WATER_EDGE_METERS: f64 = 512.0;
const ACTIVE_WATER_CELL_COUNT: usize = 16;
const ACTIVE_WATER_STEP_SECONDS: f64 = 1.0;
const MAX_TERRAIN_INTEGRATIONS_PER_FRAME: usize = 2;
const TERRAIN_INTEGRATION_BUDGET: Duration = Duration::from_millis(3);
const DISTANT_TREE_DISTANCE_MULTIPLIER: u64 = 20;
const DISTANT_TREE_HIGH_QUALITY_DISTANCE_MULTIPLIER: u64 = 5;
const DISTANT_TREE_SIMPLIFIED_DISTANCE_MULTIPLIER: u64 = 10;
const DISTANT_TREE_TILE_CHUNKS_PER_EDGE: u64 = 4;
// Four 128 m tree tiles cover the renderer's 480 m maximum shadow-caster reach.
const SHADOW_TREE_TILE_RADIUS: u64 = 4;
const ATMOSPHERE_CELL_EDGE_METERS: f64 = 8_000.0;
#[cfg(not(target_arch = "wasm32"))]
const TERRAIN_PREFETCH_CENTERS_AHEAD: u64 = 2;
#[cfg(target_arch = "wasm32")]
const TERRAIN_PREFETCH_CENTERS_AHEAD: u64 = 2;

#[cfg(not(target_arch = "wasm32"))]
type ClientTerrainMeshQueue = TerrainMeshQueue<GeneratedWorldTerrain>;
#[cfg(target_arch = "wasm32")]
type ClientTerrainMeshQueue = browser_terrain::BrowserTerrainMeshQueue;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = TreelineApp::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() -> Result<(), Box<dyn Error>> {
    use winit::platform::web::EventLoopExtWebSys;

    console_error_panic_hook::set_once();
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.spawn_app(TreelineApp::default());
    Ok(())
}

#[derive(Default)]
struct TreelineApp {
    game: Option<Game>,
    initialization_started: bool,
    #[cfg(target_arch = "wasm32")]
    pending_game: Rc<RefCell<Option<Result<Game, String>>>>,
}

impl ApplicationHandler for TreelineApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.game.is_some() || self.initialization_started {
            return;
        }
        self.initialization_started = true;

        let attributes = Window::default_attributes()
            .with_title(WINDOW_TITLE)
            .with_inner_size(LogicalSize::new(1280, 720));
        #[cfg(target_arch = "wasm32")]
        let attributes = {
            use winit::platform::web::WindowAttributesExtWebSys;

            attributes.with_append(true)
        };
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create the Treeline window: {error}");
                event_loop.exit();
                return;
            }
        };

        #[cfg(not(target_arch = "wasm32"))]
        match pollster::block_on(Game::new(window)) {
            Ok(game) => self.game = Some(game),
            Err(error) => {
                eprintln!("failed to start Treeline: {error}");
                event_loop.exit();
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            let pending_game = Rc::clone(&self.pending_game);
            wasm_bindgen_futures::spawn_local(async move {
                let result = Game::new(window).await.map_err(|error| error.to_string());
                *pending_game.borrow_mut() = Some(result);
            });
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        if window_id != game.window.id() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => game.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                game.update();
                match game.render() {
                    Ok(()) | Err(wgpu::SurfaceError::Timeout) => {}
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        game.reconfigure_surface();
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                    Err(error) => eprintln!("terrain render failed: {error}"),
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape && event.state == ElementState::Pressed {
                        game.set_cursor_captured(false);
                    } else if code == KeyCode::KeyR {
                        if event.state == ElementState::Pressed && !event.repeat {
                            game.request_random_warp();
                        }
                    } else if code == KeyCode::KeyC {
                        if event.state == ElementState::Pressed && !event.repeat {
                            game.request_cave_warp();
                        }
                    } else if code == KeyCode::KeyB {
                        if event.state == ElementState::Pressed && !event.repeat {
                            game.request_water_warp();
                        }
                    } else if code == KeyCode::KeyF {
                        if event.state == ElementState::Pressed && !event.repeat {
                            game.toggle_aerial_mode();
                        }
                    } else if code == KeyCode::KeyT {
                        if event.state == ElementState::Pressed && !event.repeat {
                            game.cycle_time_of_day();
                        }
                    } else {
                        game.input
                            .set_key(code, event.state == ElementState::Pressed);
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => game.set_cursor_captured(true),
            WindowEvent::Touch(touch) => game.handle_touch(touch),
            WindowEvent::Focused(false) => {
                game.set_cursor_captured(false);
                game.input.clear();
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        if game.cursor_captured
            && let DeviceEvent::MouseMotion { delta } = event
        {
            game.camera.look(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = event_loop;

        #[cfg(target_arch = "wasm32")]
        if self.game.is_none()
            && let Some(result) = self.pending_game.borrow_mut().take()
        {
            match result {
                Ok(game) => self.game = Some(game),
                Err(error) => {
                    eprintln!("failed to start Treeline: {error}");
                    event_loop.exit();
                }
            }
        }

        if let Some(game) = &self.game {
            game.window.request_redraw();
        }
    }
}

#[derive(Default)]
struct WarpRequests {
    random: bool,
    cave: bool,
    water: bool,
}

struct Game {
    window: Arc<Window>,
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    renderer: TerrainRenderer,
    terrain_chunks: BTreeMap<ChunkIndex, ResidentTerrainChunk>,
    far_terrain_tiles: BTreeMap<FarTileIndex, ResidentFarTerrainTile>,
    distant_tree_tiles: BTreeMap<DistantTreeTileIndex, ResidentDistantTreeTile>,
    pending_distant_tree_tiles: VecDeque<DistantTreeMeshSpec>,
    requested_chunks: BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far_tiles: BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    terrain_jobs: ClientTerrainMeshQueue,
    chunk_streamer: ChunkStreamer,
    far_terrain_streamer: FarTerrainStreamer,
    terrain: GeneratedWorldTerrain,
    camera: Camera,
    input: InputState,
    cursor_captured: bool,
    previous_frame: Instant,
    initial_generation: InitialGenerationProgress,
    warp_requests: WarpRequests,
    atmosphere_cell: Option<CellIndex>,
    water_simulation: ActiveWaterSimulation,
    active_water_region: ActiveRegionId,
    water_step_accumulator_seconds: f64,
    time_of_day: TimeOfDay,
    #[cfg(target_arch = "wasm32")]
    browser_actions: BrowserActions,
}

struct InitializedGpu {
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
}

async fn initialize_gpu(window: Arc<Window>) -> Result<InitializedGpu, Box<dyn Error>> {
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
                label: Some("Treeline device"),
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

    Ok(InitializedGpu {
        instance,
        surface,
        device,
        queue,
        surface_config,
    })
}

impl Game {
    async fn new(window: Arc<Window>) -> Result<Self, Box<dyn Error>> {
        let InitializedGpu {
            instance,
            surface,
            device,
            queue,
            surface_config,
        } = initialize_gpu(window.clone()).await?;

        let world_generation_started = Instant::now();
        window.set_title("Treeline — Preparing spawn geography…");
        let terrain = GeneratedWorldTerrain::new(WORLD);
        let renderer = create_terrain_renderer(&device, &queue, &surface_config);
        let chunk_streamer = ChunkStreamer::new(chunk_streaming_config());
        let far_terrain_streamer = FarTerrainStreamer::new(far_terrain_streaming_config());
        let (mut terrain_chunks, mut far_terrain_tiles) = (BTreeMap::new(), BTreeMap::new());
        let distant_tree_tiles = BTreeMap::new();
        let pending_distant_tree_tiles = VecDeque::new();
        let (mut requested_chunks, mut requested_far_tiles) = (BTreeMap::new(), BTreeMap::new());
        #[cfg(not(target_arch = "wasm32"))]
        let mut terrain_jobs = TerrainMeshQueue::for_generated_world(terrain.clone());
        #[cfg(target_arch = "wasm32")]
        let mut terrain_jobs = browser_terrain::BrowserTerrainMeshQueue::new(WORLD)?;

        let spawn_preparation_started = Instant::now();
        let start_y = surface_height(&terrain, START_X, START_Z) + EYE_HEIGHT;
        let spawn_preparation_time = spawn_preparation_started.elapsed();
        let camera = Camera::new(
            DVec3::new(START_X, start_y, START_Z),
            START_YAW,
            START_PITCH,
        );
        schedule_terrain(
            chunk_streamer,
            far_terrain_streamer,
            camera.world_position(),
            [0.0, 0.0],
            &mut terrain_chunks,
            &mut far_terrain_tiles,
            &mut requested_chunks,
            &mut requested_far_tiles,
            &mut terrain_jobs,
        )?;
        update_render_camera(
            &renderer,
            &queue,
            &camera,
            &surface_config,
            TimeOfDay::default(),
        );
        let atmosphere_cell =
            initialize_atmosphere(&renderer, &queue, &terrain, camera.world_position());
        let (water_simulation, active_water_region) =
            initialize_active_water(&terrain, camera.world_position())?;
        let initial_generation = start_initial_progress(
            &window,
            &renderer,
            &queue,
            world_generation_started,
            spawn_preparation_time,
            &requested_chunks,
            &requested_far_tiles,
            chunk_streamer,
            far_terrain_streamer,
            camera.world_position(),
        )?;
        #[cfg(target_arch = "wasm32")]
        let browser_actions = BrowserActions::new()?;

        let game = Self {
            window,
            _instance: instance,
            surface,
            device,
            queue,
            surface_config,
            renderer,
            terrain_chunks,
            far_terrain_tiles,
            distant_tree_tiles,
            pending_distant_tree_tiles,
            requested_chunks,
            requested_far_tiles,
            terrain_jobs,
            chunk_streamer,
            far_terrain_streamer,
            terrain,
            camera,
            input: InputState::default(),
            cursor_captured: false,
            previous_frame: Instant::now(),
            initial_generation,
            warp_requests: WarpRequests::default(),
            atmosphere_cell,
            water_simulation,
            active_water_region,
            water_step_accumulator_seconds: 0.0,
            time_of_day: TimeOfDay::default(),
            #[cfg(target_arch = "wasm32")]
            browser_actions,
        };
        Ok(game)
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.reconfigure_surface();
        self.renderer.resize(&self.device, width, height);
    }

    fn reconfigure_surface(&self) {
        self.surface.configure(&self.device, &self.surface_config);
    }

    fn set_cursor_captured(&mut self, captured: bool) {
        #[cfg(target_arch = "wasm32")]
        {
            self.cursor_captured = captured;
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let captured = if captured {
                self.window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined))
                    .is_ok()
            } else {
                let _ = self.window.set_cursor_grab(CursorGrabMode::None);
                false
            };
            self.window.set_cursor_visible(!captured);
            self.cursor_captured = captured;
        }
    }

    fn handle_touch(&mut self, touch: Touch) {
        let size = self.window.inner_size();
        let position = Vec2::new(f64_as_f32(touch.location.x), f64_as_f32(touch.location.y));
        let stick_radius = f64_as_f32(64.0 * self.window.scale_factor());

        match touch.phase {
            TouchPhase::Started => {
                self.input
                    .sticks
                    .begin(touch.id, position, u32_as_f32(size.width));
            }
            TouchPhase::Moved => self.input.sticks.update(touch.id, position),
            TouchPhase::Ended | TouchPhase::Cancelled => self.input.sticks.end(touch.id),
        }
        self.input.sticks.set_radius(stick_radius);
    }

    fn request_random_warp(&mut self) {
        self.warp_requests.random = true;
    }

    fn request_cave_warp(&mut self) {
        self.warp_requests.cave = true;
    }

    fn request_water_warp(&mut self) {
        self.warp_requests.water = true;
    }

    fn toggle_aerial_mode(&mut self) {
        let enabled = self.camera.toggle_aerial_mode(&self.terrain);
        #[cfg(target_arch = "wasm32")]
        BrowserActions::set_aerial_mode_enabled(enabled);
        eprintln!(
            "aerial mode {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    fn cycle_time_of_day(&mut self) {
        self.time_of_day = self.time_of_day.next();
        eprintln!("daylight: {}", self.time_of_day.label());
    }

    fn random_warp(&mut self) -> Result<(), Box<dyn Error>> {
        let started = Instant::now();
        let previous = self.camera.world_position();
        let [destination_x, destination_z] = random_warp_site(&self.terrain, previous)
            .ok_or_else(|| std::io::Error::other("could not find dry ground for a random warp"))?;
        let destination_y =
            self.camera
                .surface_relative_y(&self.terrain, destination_x, destination_z);
        let destination = DVec3::new(destination_x, destination_y, destination_z);
        let preparation_time = started.elapsed();
        self.relocate(destination, started, preparation_time)?;

        let current = self.camera.world_position();
        let distance_kilometers = (current.x - previous.x).hypot(current.z - previous.z) / 1_000.0;
        eprintln!(
            "random warp: ({:.0}, {:.0}) → ({destination_x:.0}, {destination_z:.0}), {distance_kilometers:.0} km",
            previous.x, previous.z
        );
        Ok(())
    }

    fn cave_warp(&mut self) -> Result<(), Box<dyn Error>> {
        let started = Instant::now();
        let previous = self.camera.world_position();
        let entrance = self
            .terrain
            .nearest_cave_entrance(previous, 12)
            .ok_or_else(|| std::io::Error::other("no cave entrance found in the search area"))?;
        let destination = DVec3::new(
            entrance.position.x,
            self.camera
                .surface_relative_y(&self.terrain, entrance.position.x, entrance.position.z),
            entrance.position.z,
        );
        let preparation_time = started.elapsed();
        self.relocate(destination, started, preparation_time)?;
        eprintln!(
            "cave warp: {} {} at ({:.0}, {:.0})",
            entrance.family.label(),
            match entrance.kind {
                treeline_world::CaveNodeKind::Entrance => "entrance",
                treeline_world::CaveNodeKind::Sinkhole => "sinkhole",
                _ => "surface connection",
            },
            entrance.position.x,
            entrance.position.z,
        );
        Ok(())
    }

    fn water_warp(&mut self) -> Result<(), Box<dyn Error>> {
        let started = Instant::now();
        let previous = self.camera.world_position();
        let site = water_warp_site(&self.terrain, previous)
            .ok_or_else(|| std::io::Error::other("no reachable water shore found"))?;
        let destination_y =
            self.camera
                .surface_relative_y(&self.terrain, site.destination[0], site.destination[1]);
        let destination = DVec3::new(site.destination[0], destination_y, site.destination[1]);
        let preparation_time = started.elapsed();
        self.relocate(destination, started, preparation_time)?;
        self.camera.face_horizontal(site.water);

        let current = self.camera.world_position();
        let distance_kilometers = (current.x - previous.x).hypot(current.z - previous.z) / 1_000.0;
        let shore_distance =
            (site.water[0] - site.destination[0]).hypot(site.water[1] - site.destination[1]);
        match site.body {
            WaterBody::Lake(lake) => eprintln!(
                "water warp: lake {:016x} shore at ({:.0}, {:.0}), water {shore_distance:.0} m away, {distance_kilometers:.0} km traveled",
                lake.id, site.destination[0], site.destination[1],
            ),
            WaterBody::Ocean => eprintln!(
                "water warp: ocean shore at ({:.0}, {:.0}), water {shore_distance:.0} m away, {distance_kilometers:.0} km traveled",
                site.destination[0], site.destination[1],
            ),
        }
        Ok(())
    }

    fn relocate(
        &mut self,
        destination: DVec3,
        started: Instant,
        preparation_time: Duration,
    ) -> Result<(), Box<dyn Error>> {
        for (_, spec) in std::mem::take(&mut self.requested_chunks) {
            self.terrain_jobs.cancel(TerrainMeshSpec::Near(spec));
        }
        for (_, spec) in std::mem::take(&mut self.requested_far_tiles) {
            self.terrain_jobs.cancel(TerrainMeshSpec::Far(spec));
        }
        self.terrain_jobs.retain_prewarm(&BTreeSet::new());
        self.terrain_chunks.clear();
        self.far_terrain_tiles.clear();
        self.distant_tree_tiles.clear();
        self.pending_distant_tree_tiles.clear();

        self.camera.position = destination;
        self.input.clear();
        self.previous_frame = Instant::now();
        schedule_terrain(
            self.chunk_streamer,
            self.far_terrain_streamer,
            self.camera.world_position(),
            [0.0, 0.0],
            &mut self.terrain_chunks,
            &mut self.far_terrain_tiles,
            &mut self.requested_chunks,
            &mut self.requested_far_tiles,
            &mut self.terrain_jobs,
        )?;
        self.initial_generation = start_initial_progress(
            &self.window,
            &self.renderer,
            &self.queue,
            started,
            preparation_time,
            &self.requested_chunks,
            &self.requested_far_tiles,
            self.chunk_streamer,
            self.far_terrain_streamer,
            self.camera.world_position(),
        )?;
        Ok(())
    }

    fn update(&mut self) {
        #[cfg(target_arch = "wasm32")]
        if self.browser_actions.take_random_warp_request() {
            self.request_random_warp();
        }
        #[cfg(target_arch = "wasm32")]
        if self.browser_actions.take_water_warp_request() {
            self.request_water_warp();
        }
        #[cfg(target_arch = "wasm32")]
        if self.browser_actions.take_aerial_mode_toggle_request() {
            self.toggle_aerial_mode();
        }
        if std::mem::take(&mut self.warp_requests.random)
            && let Err(error) = self.random_warp()
        {
            eprintln!("random warp failed: {error}");
        }
        if std::mem::take(&mut self.warp_requests.cave)
            && let Err(error) = self.cave_warp()
        {
            eprintln!("cave warp failed: {error}");
        }
        if std::mem::take(&mut self.warp_requests.water)
            && let Err(error) = self.water_warp()
        {
            eprintln!("water warp failed: {error}");
        }

        let now = Instant::now();
        let delta_seconds = (now - self.previous_frame).as_secs_f64().min(0.1);
        self.previous_frame = now;
        self.camera
            .look_with_stick(self.input.look_axis(), delta_seconds);
        let travel_direction = self.camera.travel_direction(&self.input);
        self.camera.walk(&self.input, &self.terrain, delta_seconds);
        if let Err(error) = self.update_living_water(delta_seconds) {
            eprintln!("active water simulation failed: {error}");
        }
        self.update_atmosphere();
        self.renderer.advance_water_time(&self.queue, delta_seconds);
        if let Err(error) = update_terrain(
            &self.device,
            &self.renderer,
            &self.terrain,
            self.chunk_streamer,
            self.far_terrain_streamer,
            self.camera.world_position(),
            travel_direction,
            &mut self.terrain_chunks,
            &mut self.far_terrain_tiles,
            &mut self.requested_chunks,
            &mut self.requested_far_tiles,
            &mut self.terrain_jobs,
            &mut self.initial_generation,
        ) {
            eprintln!("terrain chunk streaming failed: {error}");
        }
        if let Err(error) = update_distant_trees(
            &self.device,
            &self.renderer,
            &self.terrain,
            self.chunk_streamer.config(),
            self.camera.world_position(),
            &mut self.distant_tree_tiles,
            &mut self.pending_distant_tree_tiles,
        ) {
            eprintln!("distant tree streaming failed: {error}");
        }
        self.initial_generation.publish(&self.window);
        update_render_camera(
            &self.renderer,
            &self.queue,
            &self.camera,
            &self.surface_config,
            self.time_of_day,
        );
        if let Ok((cutout_min, cutout_max)) =
            far_cutout_bounds(self.chunk_streamer, self.camera.world_position())
        {
            self.renderer
                .update_far_cutout(&self.queue, cutout_min, cutout_max);
        }
    }

    fn update_atmosphere(&mut self) {
        let position = self.camera.world_position();
        let cell = CellIndex::containing(position.x, position.z, 0, ATMOSPHERE_CELL_EDGE_METERS);
        if cell == self.atmosphere_cell {
            return;
        }
        let Some(settings) = atmosphere_settings(&self.terrain, position.x, position.z) else {
            return;
        };
        self.renderer.update_atmosphere(&self.queue, settings);
        self.atmosphere_cell = cell;
    }

    fn update_living_water(&mut self, delta_seconds: f64) -> Result<(), Box<dyn Error>> {
        let (region, spec) = active_water_footprint(self.camera.world_position())
            .ok_or_else(|| std::io::Error::other("player is outside active-water range"))?;
        if region != self.active_water_region {
            let regenerated = self.terrain.active_water_region(spec).map_err(|error| {
                std::io::Error::other(format!("water reconstruction failed: {error:?}"))
            })?;
            let _ = self.water_simulation.freeze(self.active_water_region);
            self.water_simulation
                .activate(region, regenerated)
                .map_err(|error| {
                    std::io::Error::other(format!("water activation failed: {error:?}"))
                })?;
            self.active_water_region = region;
            self.water_step_accumulator_seconds = 0.0;
        }
        self.water_step_accumulator_seconds += delta_seconds;
        while self.water_step_accumulator_seconds >= ACTIVE_WATER_STEP_SECONDS {
            self.water_simulation
                .step(ACTIVE_WATER_STEP_SECONDS)
                .map_err(|error| std::io::Error::other(format!("water step failed: {error:?}")))?;
            self.water_step_accumulator_seconds -= ACTIVE_WATER_STEP_SECONDS;
        }
        Ok(())
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Treeline frame encoder"),
            });
        let shadow_tree_center = DistantTreeTileIndex::containing(self.camera.world_position());
        let shadow_meshes = self
            .far_terrain_tiles
            .values()
            .map(|resident| &resident.mesh)
            .chain(self.terrain_chunks.values().map(|resident| &resident.mesh))
            .chain(
                self.distant_tree_tiles
                    .iter()
                    .filter_map(|(tile, resident)| {
                        shadow_tree_center
                            .is_some_and(|center| {
                                tile.chebyshev_distance(center) <= SHADOW_TREE_TILE_RADIUS
                            })
                            .then_some(resident.mesh.as_ref())
                            .flatten()
                    }),
            )
            .chain(
                self.terrain_chunks
                    .values()
                    .filter_map(|resident| resident.rock_mesh.as_ref()),
            )
            .chain(
                self.terrain_chunks
                    .values()
                    .filter_map(|resident| resident.ground_vegetation_mesh.as_ref()),
            )
            .collect::<Vec<_>>();
        self.renderer.render(
            &mut encoder,
            &view,
            self.far_terrain_tiles
                .values()
                .map(|resident| &resident.mesh)
                .chain(
                    self.far_terrain_tiles
                        .values()
                        .filter_map(|resident| resident.lake_mesh.as_ref()),
                ),
            self.terrain_chunks
                .values()
                .map(|resident| &resident.mesh)
                .chain(
                    self.terrain_chunks
                        .values()
                        .filter_map(|resident| resident.lake_mesh.as_ref()),
                )
                .chain(
                    self.distant_tree_tiles
                        .values()
                        .filter_map(|resident| resident.mesh.as_ref()),
                )
                .chain(
                    self.terrain_chunks
                        .values()
                        .filter_map(|resident| resident.rock_mesh.as_ref()),
                )
                .chain(
                    self.terrain_chunks
                        .values()
                        .filter_map(|resident| resident.ground_vegetation_mesh.as_ref()),
                ),
            &shadow_meshes,
        );
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

struct ResidentTerrainChunk {
    spec: ChunkMeshSpec,
    mesh: TerrainMesh,
    lake_mesh: Option<TerrainMesh>,
    rock_mesh: Option<TerrainMesh>,
    ground_vegetation_mesh: Option<TerrainMesh>,
}

struct ResidentFarTerrainTile {
    spec: FarTerrainMeshSpec,
    mesh: TerrainMesh,
    lake_mesh: Option<TerrainMesh>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct DistantTreeTileIndex {
    x: i64,
    z: i64,
}

impl DistantTreeTileIndex {
    fn containing(position: WorldPosition) -> Option<Self> {
        let chunk = ChunkIndex::containing(position)?;
        let chunks_per_edge = i64::try_from(DISTANT_TREE_TILE_CHUNKS_PER_EDGE).ok()?;
        Some(Self {
            x: chunk.x.div_euclid(chunks_per_edge),
            z: chunk.z.div_euclid(chunks_per_edge),
        })
    }

    fn bounds(self) -> Option<TreeBounds> {
        let chunks_per_edge = i64::try_from(DISTANT_TREE_TILE_CHUNKS_PER_EDGE).ok()?;
        let chunk = ChunkIndex::new(
            self.x.checked_mul(chunks_per_edge)?,
            self.z.checked_mul(chunks_per_edge)?,
        );
        let origin = chunk.sample_origin();
        let edge = ChunkIndex::edge_meters() * 4.0;
        TreeBounds::new(origin.x, origin.z, origin.x + edge, origin.z + edge)
    }

    fn chebyshev_distance(self, other: Self) -> u64 {
        self.x.abs_diff(other.x).max(self.z.abs_diff(other.z))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DistantTreeMeshSpec {
    tile: DistantTreeTileIndex,
    detail: TreeMeshDetail,
}

struct ResidentDistantTreeTile {
    spec: DistantTreeMeshSpec,
    mesh: Option<TerrainMesh>,
}

#[derive(Debug)]
struct InitialGenerationProgress {
    started: Instant,
    spawn_preparation_time: Duration,
    horizon_tiles: BTreeSet<FarTileIndex>,
    far_tiles: BTreeSet<FarTileIndex>,
    near_chunks: BTreeSet<ChunkIndex>,
    completed_horizon_tiles: BTreeSet<FarTileIndex>,
    completed_far_tiles: BTreeSet<FarTileIndex>,
    completed_near_chunks: BTreeSet<ChunkIndex>,
    terrain_generation_time: Duration,
    lake_generation_time: Duration,
    integration_time: Duration,
    stale_jobs: usize,
    dirty: bool,
    finished_at: Option<Duration>,
    reported: bool,
}

impl InitialGenerationProgress {
    fn new(
        started: Instant,
        spawn_preparation_time: Duration,
        requested: &BTreeMap<ChunkIndex, ChunkMeshSpec>,
        requested_far: &BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
        far_streamer: FarTerrainStreamer,
        player_position: WorldPosition,
    ) -> Result<Self, Box<dyn Error>> {
        let far_center = FarTileIndex::containing(player_position)
            .ok_or_else(|| std::io::Error::other("player position is outside far tile range"))?;
        let horizon_radius = far_streamer.config().load_radius();
        let (horizon_tiles, far_tiles) = requested_far
            .keys()
            .copied()
            .partition(|tile| tile.chebyshev_distance(far_center) == horizon_radius);

        Ok(Self {
            started,
            spawn_preparation_time,
            horizon_tiles,
            far_tiles,
            near_chunks: requested.keys().copied().collect(),
            completed_horizon_tiles: BTreeSet::new(),
            completed_far_tiles: BTreeSet::new(),
            completed_near_chunks: BTreeSet::new(),
            terrain_generation_time: Duration::ZERO,
            lake_generation_time: Duration::ZERO,
            integration_time: Duration::ZERO,
            stale_jobs: 0,
            dirty: true,
            finished_at: None,
            reported: false,
        })
    }

    fn record_stale(&mut self, terrain_generation_time: Duration, lake_generation_time: Duration) {
        if self.finished_at.is_some() {
            return;
        }
        self.terrain_generation_time += terrain_generation_time;
        self.lake_generation_time += lake_generation_time;
        self.stale_jobs += 1;
    }

    fn record_completion(
        &mut self,
        spec: TerrainMeshSpec,
        terrain_generation_time: Duration,
        lake_generation_time: Duration,
        integration_time: Duration,
    ) {
        if self.finished_at.is_some() {
            return;
        }
        self.terrain_generation_time += terrain_generation_time;
        self.lake_generation_time += lake_generation_time;
        self.integration_time += integration_time;
        let changed = match spec {
            TerrainMeshSpec::Far(spec) if self.horizon_tiles.contains(&spec.tile) => {
                self.completed_horizon_tiles.insert(spec.tile)
            }
            TerrainMeshSpec::Far(spec) if self.far_tiles.contains(&spec.tile) => {
                self.completed_far_tiles.insert(spec.tile)
            }
            TerrainMeshSpec::Near(spec) if self.near_chunks.contains(&spec.chunk) => {
                self.completed_near_chunks.insert(spec.chunk)
            }
            _ => false,
        };
        self.dirty |= changed;
        if self.completed_horizon_tiles.len() == self.horizon_tiles.len()
            && self.completed_far_tiles.len() == self.far_tiles.len()
            && self.completed_near_chunks.len() == self.near_chunks.len()
        {
            self.finished_at = Some(self.started.elapsed());
            self.dirty = true;
        }
    }

    fn title(&self) -> String {
        format!(
            "Treeline — Building world: horizon {}/{} · far {}/{} · nearby {}/{}",
            self.completed_horizon_tiles.len(),
            self.horizon_tiles.len(),
            self.completed_far_tiles.len(),
            self.far_tiles.len(),
            self.completed_near_chunks.len(),
            self.near_chunks.len()
        )
    }

    fn publish(&mut self, window: &Window) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(wall_time) = self.finished_at else {
            window.set_title(&self.title());
            return;
        };
        window.set_title(WINDOW_TITLE);
        if self.reported {
            return;
        }
        self.reported = true;
        eprintln!(
            "initial world ready in {:.2}s: horizon {}/{}, far {}/{}, nearby {}/{}; spawn geography {:.2}s, worker terrain CPU {:.2}s, worker lake CPU {:.2}s, main-thread integration {:.2}s, stale jobs {}",
            wall_time.as_secs_f64(),
            self.completed_horizon_tiles.len(),
            self.horizon_tiles.len(),
            self.completed_far_tiles.len(),
            self.far_tiles.len(),
            self.completed_near_chunks.len(),
            self.near_chunks.len(),
            self.spawn_preparation_time.as_secs_f64(),
            self.terrain_generation_time.as_secs_f64(),
            self.lake_generation_time.as_secs_f64(),
            self.integration_time.as_secs_f64(),
            self.stale_jobs
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CameraMode {
    Ground,
    Aerial,
}

impl CameraMode {
    const fn height_above_ground(self) -> f64 {
        match self {
            Self::Ground => EYE_HEIGHT,
            Self::Aerial => AERIAL_HEIGHT_METERS,
        }
    }

    const fn speed_multiplier(self) -> f64 {
        match self {
            Self::Ground => 1.0,
            Self::Aerial => AERIAL_SPEED_MULTIPLIER,
        }
    }
}

struct Camera {
    position: DVec3,
    yaw: f64,
    pitch: f64,
    mode: CameraMode,
}

impl Camera {
    const fn new(position: DVec3, yaw: f64, pitch: f64) -> Self {
        Self {
            position,
            yaw,
            pitch,
            mode: CameraMode::Ground,
        }
    }

    fn direction(&self) -> DVec3 {
        let pitch_cosine = libm::cos(self.pitch);
        DVec3::new(
            libm::cos(self.yaw) * pitch_cosine,
            libm::sin(self.pitch),
            libm::sin(self.yaw) * pitch_cosine,
        )
        .normalize()
    }

    fn look(&mut self, delta_x: f64, delta_y: f64) {
        const SENSITIVITY: f64 = 0.002;
        self.yaw += delta_x * SENSITIVITY;
        self.pitch = (self.pitch - (delta_y * SENSITIVITY)).clamp(-1.5, 1.5);
    }

    fn look_with_stick(&mut self, axis: Vec2, delta_seconds: f64) {
        const HORIZONTAL_SPEED: f64 = 2.4;
        const VERTICAL_SPEED: f64 = 1.8;
        self.yaw += f64::from(axis.x) * HORIZONTAL_SPEED * delta_seconds;
        self.pitch =
            (self.pitch + (f64::from(axis.y) * VERTICAL_SPEED * delta_seconds)).clamp(-1.5, 1.5);
    }

    fn face_horizontal(&mut self, target: [f64; 2]) {
        let delta_x = target[0] - self.position.x;
        let delta_z = target[1] - self.position.z;
        if delta_x != 0.0 || delta_z != 0.0 {
            self.yaw = libm::atan2(delta_z, delta_x);
            self.pitch = -0.08;
        }
    }

    fn height_above_ground(&self) -> f64 {
        self.mode.height_above_ground()
    }

    fn surface_relative_y(&self, terrain: &impl SurfaceField, x: f64, z: f64) -> f64 {
        surface_height(terrain, x, z) + self.height_above_ground()
    }

    fn toggle_aerial_mode(&mut self, terrain: &impl SurfaceField) -> bool {
        self.mode = match self.mode {
            CameraMode::Ground => CameraMode::Aerial,
            CameraMode::Aerial => CameraMode::Ground,
        };
        self.snap_to_mode_height(terrain);
        self.mode == CameraMode::Aerial
    }

    fn snap_to_mode_height(&mut self, terrain: &impl SurfaceField) {
        self.position.y = self.surface_relative_y(terrain, self.position.x, self.position.z);
    }

    fn walk<T>(&mut self, input: &InputState, terrain: &T, delta_seconds: f64)
    where
        T: DensityField + SurfaceField,
    {
        let previous_position = self.position;
        let movement = self.movement(input);
        if movement.length_squared() > 0.0 {
            let base_speed = if input.sprint() {
                SPRINT_SPEED
            } else {
                WALK_SPEED
            };
            let speed = base_speed * self.mode.speed_multiplier();
            let intensity = movement.length().min(1.0);
            self.position += movement.normalize() * intensity * speed * delta_seconds;
        }
        if self.mode == CameraMode::Aerial {
            self.snap_to_mode_height(terrain);
            return;
        }
        let current_floor = self.position.y - EYE_HEIGHT;
        let floor = if let Some(floor) =
            walkable_floor_height(terrain, self.position.x, self.position.z, current_floor)
        {
            floor
        } else {
            self.position.x = previous_position.x;
            self.position.z = previous_position.z;
            walkable_floor_height(
                terrain,
                self.position.x,
                self.position.z,
                previous_position.y - EYE_HEIGHT,
            )
            .unwrap_or_else(|| surface_height(terrain, self.position.x, self.position.z))
        };
        self.position.y = floor + EYE_HEIGHT;
    }

    fn movement(&self, input: &InputState) -> DVec3 {
        let forward = DVec3::new(libm::cos(self.yaw), 0.0, libm::sin(self.yaw));
        let right = forward.cross(DVec3::Y);
        (forward * f64::from(input.forward_axis())) + (right * f64::from(input.right_axis()))
    }

    fn travel_direction(&self, input: &InputState) -> [f64; 2] {
        let movement = self.movement(input);
        if movement.length_squared() <= f64::EPSILON {
            [0.0, 0.0]
        } else {
            let direction = movement.normalize();
            [direction.x, direction.z]
        }
    }

    fn world_position(&self) -> WorldPosition {
        WorldPosition::new(self.position.x, self.position.y, self.position.z)
    }

    fn view_projection(&self, width: u32, height: u32) -> [[f32; 4]; 4] {
        let aspect = u32_as_f32(width) / u32_as_f32(height.max(1));
        // An infinite reverse-Z projection keeps the 32-bit depth buffer useful
        // for both nearby vegetation and the long-distance terrain horizon.
        let projection = Mat4::perspective_infinite_reverse_rh(60.0_f32.to_radians(), aspect, 0.1);
        let view = Mat4::look_to_rh(Vec3::ZERO, self.direction().as_vec3(), Vec3::Y);
        (projection * view).to_cols_array_2d()
    }
}

#[derive(Default)]
struct InputState {
    pressed: HashSet<KeyCode>,
    sticks: VirtualSticks,
}

impl InputState {
    fn set_key(&mut self, code: KeyCode, pressed: bool) {
        if pressed {
            self.pressed.insert(code);
        } else {
            self.pressed.remove(&code);
        }
    }

    fn forward_axis(&self) -> f32 {
        (f32::from(u8::from(
            self.is_down(KeyCode::KeyW) || self.is_down(KeyCode::ArrowUp),
        )) - f32::from(u8::from(
            self.is_down(KeyCode::KeyS) || self.is_down(KeyCode::ArrowDown),
        )) + self.sticks.movement_axis().y)
            .clamp(-1.0, 1.0)
    }

    fn right_axis(&self) -> f32 {
        (f32::from(u8::from(
            self.is_down(KeyCode::KeyD) || self.is_down(KeyCode::ArrowRight),
        )) - f32::from(u8::from(
            self.is_down(KeyCode::KeyA) || self.is_down(KeyCode::ArrowLeft),
        )) + self.sticks.movement_axis().x)
            .clamp(-1.0, 1.0)
    }

    fn look_axis(&self) -> Vec2 {
        self.sticks.look_axis()
    }

    fn sprint(&self) -> bool {
        self.is_down(KeyCode::ShiftLeft)
            || self.is_down(KeyCode::ShiftRight)
            || self.sticks.movement_axis().length() > 0.85
    }

    fn is_down(&self, code: KeyCode) -> bool {
        self.pressed.contains(&code)
    }

    fn clear(&mut self) {
        self.pressed.clear();
        self.sticks.clear();
    }
}

#[derive(Clone, Copy, Debug)]
struct StickTouch {
    id: u64,
    origin: Vec2,
    current: Vec2,
}

impl StickTouch {
    const fn new(id: u64, position: Vec2) -> Self {
        Self {
            id,
            origin: position,
            current: position,
        }
    }

    fn axis(self, radius: f32) -> Vec2 {
        let offset = Vec2::new(
            self.current.x - self.origin.x,
            self.origin.y - self.current.y,
        ) / radius.max(1.0);
        offset.clamp_length_max(1.0)
    }
}

#[derive(Debug)]
struct VirtualSticks {
    movement: Option<StickTouch>,
    look: Option<StickTouch>,
    radius: f32,
}

impl Default for VirtualSticks {
    fn default() -> Self {
        Self {
            movement: None,
            look: None,
            radius: 64.0,
        }
    }
}

impl VirtualSticks {
    fn begin(&mut self, id: u64, position: Vec2, viewport_width: f32) {
        let target = if position.x < viewport_width * 0.5 {
            &mut self.movement
        } else {
            &mut self.look
        };
        if target.is_none() {
            *target = Some(StickTouch::new(id, position));
        }
    }

    fn update(&mut self, id: u64, position: Vec2) {
        for stick in [&mut self.movement, &mut self.look].into_iter().flatten() {
            if stick.id == id {
                stick.current = position;
                break;
            }
        }
    }

    fn end(&mut self, id: u64) {
        if self.movement.is_some_and(|stick| stick.id == id) {
            self.movement = None;
        }
        if self.look.is_some_and(|stick| stick.id == id) {
            self.look = None;
        }
    }

    fn set_radius(&mut self, radius: f32) {
        self.radius = radius.max(1.0);
    }

    fn movement_axis(&self) -> Vec2 {
        self.movement
            .map_or(Vec2::ZERO, |stick| stick.axis(self.radius))
    }

    fn look_axis(&self) -> Vec2 {
        self.look
            .map_or(Vec2::ZERO, |stick| stick.axis(self.radius))
    }

    fn clear(&mut self) {
        self.movement = None;
        self.look = None;
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Default)]
struct ToggleRequestCounter {
    count: Cell<u32>,
}

#[cfg(any(test, target_arch = "wasm32"))]
impl ToggleRequestCounter {
    fn request(&self) {
        self.count.set(self.count.get().wrapping_add(1));
    }

    fn take(&self) -> bool {
        self.count.replace(0) % 2 == 1
    }
}

#[cfg(target_arch = "wasm32")]
struct BrowserActions {
    random_warp_requested: Rc<Cell<bool>>,
    random_warp_listener: Closure<dyn FnMut(web_sys::Event)>,
    water_warp_requested: Rc<Cell<bool>>,
    water_warp_listener: Closure<dyn FnMut(web_sys::Event)>,
    aerial_mode_toggle_requests: Rc<ToggleRequestCounter>,
    aerial_mode_toggle_listener: Closure<dyn FnMut(web_sys::Event)>,
}

#[cfg(target_arch = "wasm32")]
impl BrowserActions {
    fn new() -> Result<Self, Box<dyn Error>> {
        let window =
            web_sys::window().ok_or_else(|| std::io::Error::other("browser window unavailable"))?;
        let random_warp_requested = Rc::new(Cell::new(false));
        let requested = Rc::clone(&random_warp_requested);
        let random_warp_listener = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            requested.set(true);
        }) as Box<dyn FnMut(web_sys::Event)>);
        window
            .add_event_listener_with_callback(
                "treeline-random-warp",
                random_warp_listener.as_ref().unchecked_ref(),
            )
            .map_err(|error| {
                std::io::Error::other(format!("could not register random warp button: {error:?}"))
            })?;
        let water_warp_requested = Rc::new(Cell::new(false));
        let requested = Rc::clone(&water_warp_requested);
        let water_warp_listener = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            requested.set(true);
        }) as Box<dyn FnMut(web_sys::Event)>);
        window
            .add_event_listener_with_callback(
                "treeline-water-warp",
                water_warp_listener.as_ref().unchecked_ref(),
            )
            .map_err(|error| {
                std::io::Error::other(format!("could not register water warp button: {error:?}"))
            })?;
        let aerial_mode_toggle_requests = Rc::new(ToggleRequestCounter::default());
        let requested = Rc::clone(&aerial_mode_toggle_requests);
        let aerial_mode_toggle_listener = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            requested.request();
        })
            as Box<dyn FnMut(web_sys::Event)>);
        window
            .add_event_listener_with_callback(
                "treeline-toggle-aerial",
                aerial_mode_toggle_listener.as_ref().unchecked_ref(),
            )
            .map_err(|error| {
                std::io::Error::other(format!("could not register aerial-mode button: {error:?}"))
            })?;
        Ok(Self {
            random_warp_requested,
            random_warp_listener,
            water_warp_requested,
            water_warp_listener,
            aerial_mode_toggle_requests,
            aerial_mode_toggle_listener,
        })
    }

    fn take_random_warp_request(&self) -> bool {
        self.random_warp_requested.replace(false)
    }

    fn take_water_warp_request(&self) -> bool {
        self.water_warp_requested.replace(false)
    }

    fn take_aerial_mode_toggle_request(&self) -> bool {
        self.aerial_mode_toggle_requests.take()
    }

    fn set_aerial_mode_enabled(enabled: bool) {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(button) = document.get_element_by_id("aerial-mode-button") else {
            return;
        };
        let pressed = if enabled { "true" } else { "false" };
        let _ = button.set_attribute("aria-pressed", pressed);
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for BrowserActions {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback(
                "treeline-random-warp",
                self.random_warp_listener.as_ref().unchecked_ref(),
            );
            let _ = window.remove_event_listener_with_callback(
                "treeline-water-warp",
                self.water_warp_listener.as_ref().unchecked_ref(),
            );
            let _ = window.remove_event_listener_with_callback(
                "treeline-toggle-aerial",
                self.aerial_mode_toggle_listener.as_ref().unchecked_ref(),
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum WaterBody {
    Lake(Lake),
    Ocean,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WaterWarpSite {
    destination: [f64; 2],
    water: [f64; 2],
    body: WaterBody,
}

fn random_warp_site(terrain: &GeneratedWorldTerrain, current: WorldPosition) -> Option<[f64; 2]> {
    if terrain.is_surveyed_tile() {
        for _ in 0..RANDOM_WARP_SITE_ATTEMPTS {
            let candidate =
                surveyed_warp_destination(random_unit_interval(), random_unit_interval());
            if surface_feature_has_dry_ground(terrain, candidate[0], candidate[1]) {
                return Some(candidate);
            }
        }
        return None;
    }
    for _ in 0..RANDOM_WARP_SITE_ATTEMPTS {
        let candidate = random_warp_destination(random_unit_interval(), random_unit_interval());
        if random_warp_distance_is_eligible(current, candidate)
            && surface_feature_has_dry_ground(terrain, candidate[0], candidate[1])
        {
            return Some(candidate);
        }
    }
    None
}

fn surveyed_warp_destination(x_fraction: f64, z_fraction: f64) -> [f64; 2] {
    let usable_edge =
        DEFAULT_SURVEYED_TILE_EDGE_METERS - (SURVEYED_WARP_BORDER_CLEARANCE_METERS * 2.0);
    [x_fraction, z_fraction].map(|fraction| {
        SURVEYED_WARP_BORDER_CLEARANCE_METERS + (fraction.clamp(0.0, 1.0) * usable_edge)
    })
}

fn water_warp_site(
    terrain: &GeneratedWorldTerrain,
    current: WorldPosition,
) -> Option<WaterWarpSite> {
    if terrain.is_surveyed_tile() {
        const UPPER_HOLMES_LAKE_INTERIOR: [f64; 2] = [7_364.0, 6_894.0];
        let water = terrain
            .lake_surface_at(UPPER_HOLMES_LAKE_INTERIOR[0], UPPER_HOLMES_LAKE_INTERIOR[1])?;
        let body = WaterBody::Lake(water.lake);
        let (destination, shore_water) =
            dry_water_shore(terrain, body, UPPER_HOLMES_LAKE_INTERIOR, 0.25)?;
        return Some(WaterWarpSite {
            destination,
            water: shore_water,
            body,
        });
    }
    for _ in 0..WATER_WARP_REGION_ATTEMPTS {
        let anchor = random_warp_destination(random_unit_interval(), random_unit_interval());
        if !random_warp_distance_is_eligible(current, anchor) {
            continue;
        }
        if terrain
            .ocean_surface_at(anchor[0], anchor[1])
            .is_some_and(|sample| sample.water_depth_meters >= WATER_WARP_MIN_DEPTH_METERS)
            && let Some((destination, water)) =
                dry_water_shore(terrain, WaterBody::Ocean, anchor, random_unit_interval())
            && random_warp_distance_is_eligible(current, destination)
            && coordinate_is_within_warp_budget(destination)
        {
            return Some(WaterWarpSite {
                destination,
                water,
                body: WaterBody::Ocean,
            });
        }
        let Some(lakes) = terrain.regional_lakes_at(anchor[0], anchor[1]) else {
            continue;
        };
        if lakes.is_empty() {
            continue;
        }
        let first = fraction_index(random_unit_interval(), lakes.len());
        let direction_fraction = random_unit_interval();
        for offset in 0..lakes.len() {
            let lake = lakes[(first + offset) % lakes.len()];
            let Some(deep_water) = visible_lake_water_point(terrain, lake) else {
                continue;
            };
            if random_warp_distance_is_eligible(current, deep_water)
                && let Some((destination, water)) = dry_water_shore(
                    terrain,
                    WaterBody::Lake(lake),
                    deep_water,
                    direction_fraction,
                )
                && random_warp_distance_is_eligible(current, destination)
                && coordinate_is_within_warp_budget(destination)
            {
                return Some(WaterWarpSite {
                    destination,
                    water,
                    body: WaterBody::Lake(lake),
                });
            }
        }
    }
    None
}

fn fraction_index(fraction: f64, length: usize) -> usize {
    if length == 0 {
        return 0;
    }
    let length_u32 = u32::try_from(length).unwrap_or(u32::MAX);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = (fraction.clamp(0.0, 1.0) * f64::from(length_u32)) as usize;
    index.min(length - 1)
}

fn visible_lake_water_point(terrain: &GeneratedWorldTerrain, lake: Lake) -> Option<[f64; 2]> {
    const SAMPLE_OFFSETS_METERS: [f64; 5] = [0.0, -400.0, 400.0, -800.0, 800.0];

    let [center_x, center_z] = lake.bottom.center();
    for z_offset in SAMPLE_OFFSETS_METERS {
        for x_offset in SAMPLE_OFFSETS_METERS {
            let point = [center_x + x_offset, center_z + z_offset];
            if terrain.ocean_surface_at(point[0], point[1]).is_none()
                && terrain
                    .lake_surface_at(point[0], point[1])
                    .is_some_and(|sample| {
                        sample.lake.id == lake.id
                            && sample.water_depth_meters >= WATER_WARP_MIN_DEPTH_METERS
                    })
            {
                return Some(point);
            }
        }
    }
    None
}

fn dry_water_shore(
    terrain: &GeneratedWorldTerrain,
    body: WaterBody,
    water: [f64; 2],
    direction_fraction: f64,
) -> Option<([f64; 2], [f64; 2])> {
    for direction_index in 0..WATER_WARP_DIRECTIONS {
        let direction_fraction = (direction_fraction
            + (f64::from(direction_index) / f64::from(WATER_WARP_DIRECTIONS)))
        .fract();
        let angle = direction_fraction * std::f64::consts::TAU;
        let direction = [libm::cos(angle), libm::sin(angle)];
        let mut water_side = water;
        let mut distance = 64.0;
        while distance <= WATER_WARP_MAX_SHORE_DISTANCE_METERS {
            let candidate = [
                water[0] + (direction[0] * distance),
                water[1] + (direction[1] * distance),
            ];
            if same_body_water(terrain, body, candidate) {
                water_side = candidate;
                distance *= 2.0;
                continue;
            }
            if surface_feature_has_dry_ground(terrain, candidate[0], candidate[1]) {
                return refine_dry_shore(terrain, body, water_side, candidate, direction);
            }
            break;
        }
    }
    None
}

fn refine_dry_shore(
    terrain: &GeneratedWorldTerrain,
    body: WaterBody,
    mut water_side: [f64; 2],
    mut dry_side: [f64; 2],
    direction: [f64; 2],
) -> Option<([f64; 2], [f64; 2])> {
    for _ in 0..16 {
        let midpoint = [
            (water_side[0] + dry_side[0]) * 0.5,
            (water_side[1] + dry_side[1]) * 0.5,
        ];
        if same_body_water(terrain, body, midpoint) {
            water_side = midpoint;
        } else if surface_feature_has_dry_ground(terrain, midpoint[0], midpoint[1]) {
            dry_side = midpoint;
        } else {
            break;
        }
    }
    if (dry_side[0] - water_side[0]).hypot(dry_side[1] - water_side[1]) > 32.0 {
        return None;
    }
    let destination = [
        dry_side[0] + (direction[0] * WATER_WARP_SHORE_CLEARANCE_METERS),
        dry_side[1] + (direction[1] * WATER_WARP_SHORE_CLEARANCE_METERS),
    ];
    surface_feature_has_dry_ground(terrain, destination[0], destination[1])
        .then_some((destination, water_side))
        .or_else(|| {
            surface_feature_has_dry_ground(terrain, dry_side[0], dry_side[1])
                .then_some((dry_side, water_side))
        })
}

fn same_body_water(terrain: &GeneratedWorldTerrain, body: WaterBody, point: [f64; 2]) -> bool {
    match body {
        WaterBody::Lake(lake) => {
            terrain.ocean_surface_at(point[0], point[1]).is_none()
                && terrain
                    .lake_surface_at(point[0], point[1])
                    .is_some_and(|sample| {
                        sample.lake.id == lake.id && sample.water_depth_meters > 0.0
                    })
        }
        WaterBody::Ocean => terrain.ocean_surface_at(point[0], point[1]).is_some(),
    }
}

fn coordinate_is_within_warp_budget(point: [f64; 2]) -> bool {
    point
        .into_iter()
        .all(|coordinate| coordinate.abs() <= RANDOM_WARP_COORDINATE_LIMIT_METERS)
}

fn random_warp_destination(x_fraction: f64, z_fraction: f64) -> [f64; 2] {
    let coordinate = |fraction: f64| {
        ((fraction.clamp(0.0, 1.0) * 2.0) - 1.0) * RANDOM_WARP_COORDINATE_LIMIT_METERS
    };
    [coordinate(x_fraction), coordinate(z_fraction)]
}

fn random_warp_distance_is_eligible(current: WorldPosition, candidate: [f64; 2]) -> bool {
    let distance = (candidate[0] - current.x).hypot(candidate[1] - current.z);
    (RANDOM_WARP_MIN_DISTANCE_METERS..=RANDOM_WARP_MAX_DISTANCE_METERS).contains(&distance)
}

#[cfg(target_arch = "wasm32")]
fn random_unit_interval() -> f64 {
    js_sys::Math::random()
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::cast_precision_loss)]
fn random_unit_interval() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let entropy = elapsed.as_secs() ^ u64::from(elapsed.subsec_nanos()).rotate_left(32);
    let mixed = stable_hash(&[WORLD.seed, entropy, NONCE.fetch_add(1, Ordering::Relaxed)]);
    ((mixed >> 11) as f64) / 9_007_199_254_740_991.0
}

fn surface_height(terrain: &impl SurfaceField, x: f64, z: f64) -> f64 {
    terrain
        .surface_height(x, z)
        .expect("finite player positions must have terrain")
}

fn walkable_floor_height(
    terrain: &impl DensityField,
    x: f64,
    z: f64,
    current_floor: f64,
) -> Option<f64> {
    const MAX_STEP_UP_METERS: f64 = 1.5;
    const MAX_DESCENT_METERS: f64 = 160.0;
    const SCAN_STEP_METERS: f64 = 0.5;
    const REFINEMENT_STEPS: usize = 9;

    if !x.is_finite() || !z.is_finite() || !current_floor.is_finite() {
        return None;
    }
    let mut air_y = current_floor + MAX_STEP_UP_METERS;
    let mut air_density = terrain.sample(WorldPosition::new(x, air_y, z)).density;
    let minimum_y = current_floor - MAX_DESCENT_METERS;
    let mut sample_y = air_y - SCAN_STEP_METERS;
    while sample_y >= minimum_y {
        let density = terrain.sample(WorldPosition::new(x, sample_y, z)).density;
        if air_density > 0.0 && density <= 0.0 {
            let mut solid_y = sample_y;
            for _ in 0..REFINEMENT_STEPS {
                let midpoint = (solid_y + air_y) * 0.5;
                if terrain.sample(WorldPosition::new(x, midpoint, z)).density <= 0.0 {
                    solid_y = midpoint;
                } else {
                    air_y = midpoint;
                }
            }
            let floor = (solid_y + air_y) * 0.5;
            let eye_clearance = terrain
                .sample(WorldPosition::new(x, floor + EYE_HEIGHT, z))
                .density;
            if eye_clearance > 0.0 {
                return Some(floor);
            }
        }
        air_y = sample_y;
        air_density = density;
        sample_y -= SCAN_STEP_METERS;
    }
    None
}

fn chunk_streaming_config() -> ChunkStreamingConfig {
    if cfg!(target_arch = "wasm32") {
        ChunkStreamingConfig::new(2, 3).expect("the browser streaming radii are valid")
    } else {
        ChunkStreamingConfig::default()
    }
}

fn active_water_footprint(
    position: WorldPosition,
) -> Option<(ActiveRegionId, ActiveWaterRegionSpec)> {
    let cell = CellIndex::containing(position.x, position.z, 0, ACTIVE_WATER_EDGE_METERS)?;
    let id = ActiveRegionId::new(cell.x, cell.z);
    let origin_x = index_as_f64(cell.x) * ACTIVE_WATER_EDGE_METERS;
    let origin_z = index_as_f64(cell.z) * ACTIVE_WATER_EDGE_METERS;
    let spacing = ACTIVE_WATER_EDGE_METERS / usize_as_f64(ACTIVE_WATER_CELL_COUNT);
    let spec =
        ActiveWaterRegionSpec::new(origin_x, origin_z, [ACTIVE_WATER_CELL_COUNT; 2], spacing)?;
    Some((id, spec))
}

fn create_terrain_renderer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface_config: &wgpu::SurfaceConfiguration,
) -> TerrainRenderer {
    TerrainRenderer::new(
        device,
        queue,
        surface_config.format,
        surface_config.width,
        surface_config.height,
    )
}

fn update_render_camera(
    renderer: &TerrainRenderer,
    queue: &wgpu::Queue,
    camera: &Camera,
    surface_config: &wgpu::SurfaceConfiguration,
    time_of_day: TimeOfDay,
) {
    renderer.update_camera(
        queue,
        camera.view_projection(surface_config.width, surface_config.height),
        camera.position.to_array(),
        camera.direction().as_vec3().to_array(),
        LightingSettings::for_time_of_day(time_of_day),
    );
}

fn initialize_active_water(
    terrain: &GeneratedWorldTerrain,
    position: WorldPosition,
) -> Result<(ActiveWaterSimulation, ActiveRegionId), Box<dyn Error>> {
    let (id, spec) = active_water_footprint(position)
        .ok_or_else(|| std::io::Error::other("spawn is outside active-water range"))?;
    let active = terrain.active_water_region(spec).map_err(|error| {
        std::io::Error::other(format!("failed to generate spawn water: {error:?}"))
    })?;
    let mut simulation = ActiveWaterSimulation::default();
    simulation.activate(id, active).map_err(|error| {
        std::io::Error::other(format!("failed to activate spawn water: {error:?}"))
    })?;
    Ok((simulation, id))
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: i64) -> f64 {
    index as f64
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

fn far_terrain_streaming_config() -> FarTerrainStreamingConfig {
    FarTerrainStreamingConfig::new(1, 1).expect("the surveyed-world radius is valid")
}

fn far_cutout_bounds(
    chunk_streamer: ChunkStreamer,
    player_position: WorldPosition,
) -> Result<([f64; 2], [f64; 2]), Box<dyn Error>> {
    let center = ChunkIndex::containing(player_position)
        .ok_or_else(|| std::io::Error::other("player position is outside chunk index range"))?;
    let cutout = NearTerrainCutout::around(center, chunk_streamer.config().load_radius())
        .ok_or_else(|| std::io::Error::other("near terrain cutout is outside chunk index range"))?;
    let min = cutout.min.sample_origin();
    let max = cutout.max_exclusive.sample_origin();
    Ok(([min.x, min.z], [max.x, max.z]))
}

#[allow(clippy::too_many_arguments)]
fn start_initial_progress(
    window: &Window,
    renderer: &TerrainRenderer,
    queue: &wgpu::Queue,
    started: Instant,
    spawn_preparation_time: Duration,
    requested: &BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far: &BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    chunk_streamer: ChunkStreamer,
    far_streamer: FarTerrainStreamer,
    player_position: WorldPosition,
) -> Result<InitialGenerationProgress, Box<dyn Error>> {
    let (cutout_min, cutout_max) = far_cutout_bounds(chunk_streamer, player_position)?;
    renderer.update_far_cutout(queue, cutout_min, cutout_max);
    let progress = InitialGenerationProgress::new(
        started,
        spawn_preparation_time,
        requested,
        requested_far,
        far_streamer,
        player_position,
    )?;
    window.set_title(&progress.title());
    Ok(progress)
}

#[allow(clippy::too_many_arguments)]
fn update_terrain(
    device: &wgpu::Device,
    renderer: &TerrainRenderer,
    terrain: &GeneratedWorldTerrain,
    chunk_streamer: ChunkStreamer,
    far_streamer: FarTerrainStreamer,
    player_position: WorldPosition,
    travel_direction: [f64; 2],
    chunks: &mut BTreeMap<ChunkIndex, ResidentTerrainChunk>,
    far_tiles: &mut BTreeMap<FarTileIndex, ResidentFarTerrainTile>,
    requested: &mut BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far: &mut BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    jobs: &mut ClientTerrainMeshQueue,
    initial_generation: &mut InitialGenerationProgress,
) -> Result<(), Box<dyn Error>> {
    let frame_integration_started = Instant::now();
    let mut integrated = 0;
    while integrated < MAX_TERRAIN_INTEGRATIONS_PER_FRAME
        && (integrated == 0 || frame_integration_started.elapsed() < TERRAIN_INTEGRATION_BUDGET)
    {
        let Some(generated) = jobs.try_next() else {
            break;
        };
        integrated += 1;
        let integration_started = Instant::now();
        let spec = generated.spec;
        let terrain_generation_time = generated.terrain_generation_time;
        let lake_generation_time = generated.lake_generation_time;
        let priority = generated.priority;
        let expected = match spec {
            TerrainMeshSpec::Near(spec) => requested.get(&spec.chunk) == Some(&spec),
            TerrainMeshSpec::Far(spec) => requested_far.get(&spec.tile) == Some(&spec),
        };
        if !expected {
            if priority != GenerationPriority::PrefetchTerrain {
                initial_generation.record_stale(terrain_generation_time, lake_generation_time);
            }
            continue;
        }
        // The job has already left the queue, so the outstanding request must
        // be cleared before any fallible step. Leaving it in place would make
        // the streamer treat this chunk as still pending and never ask for it
        // again, leaving a permanent hole in the world.
        clear_terrain_request(spec, requested, requested_far);
        // A meshing failure must not abort the whole streaming update either;
        // the cleared request lets a later frame retry this chunk.
        let (mesh, lake_mesh) = match (generated.mesh, generated.lake_mesh.transpose()) {
            (Ok(mesh), Ok(lake_mesh)) => (mesh, lake_mesh),
            (Err(error), _) | (_, Err(error)) => {
                eprintln!("terrain mesh generation failed, retrying later: {error}");
                continue;
            }
        };
        let (surface, water) =
            upload_terrain_surface(device, renderer, terrain, &mesh, lake_mesh.as_ref())?;
        match spec {
            TerrainMeshSpec::Near(spec) => {
                chunks.insert(
                    spec.chunk,
                    ResidentTerrainChunk {
                        spec,
                        mesh: surface,
                        lake_mesh: water,
                        rock_mesh: rock_mesh_for_chunk(device, renderer, terrain, spec.chunk)?,
                        ground_vegetation_mesh: ground_vegetation_mesh_for_chunk(
                            device, renderer, terrain, spec.chunk,
                        )?,
                    },
                );
            }
            TerrainMeshSpec::Far(spec) => {
                far_tiles.insert(
                    spec.tile,
                    ResidentFarTerrainTile {
                        spec,
                        mesh: surface,
                        lake_mesh: water,
                    },
                );
            }
        }
        initial_generation.record_completion(
            spec,
            terrain_generation_time,
            lake_generation_time,
            integration_started.elapsed(),
        );
    }

    schedule_terrain(
        chunk_streamer,
        far_streamer,
        player_position,
        travel_direction,
        chunks,
        far_tiles,
        requested,
        requested_far,
        jobs,
    )
}

/// Drops the outstanding request for a completed mesh, whichever tier it
/// belongs to, so the streamer is free to schedule that footprint again.
fn clear_terrain_request(
    spec: TerrainMeshSpec,
    requested: &mut BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far: &mut BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
) {
    match spec {
        TerrainMeshSpec::Near(spec) => {
            requested.remove(&spec.chunk);
        }
        TerrainMeshSpec::Far(spec) => {
            requested_far.remove(&spec.tile);
        }
    }
}

/// Uploads a terrain surface and its optional water sheet with the shared snow
/// treatment, so near chunks and far tiles cannot drift apart in appearance.
fn upload_terrain_surface(
    device: &wgpu::Device,
    renderer: &TerrainRenderer,
    terrain: &GeneratedWorldTerrain,
    mesh: &Mesh,
    lake_mesh: Option<&Mesh>,
) -> Result<(TerrainMesh, Option<TerrainMesh>), Box<dyn Error>> {
    let surface = renderer.upload_snowy_mesh(device, mesh, |x, z| {
        terrain
            .snow_coverage_for_slope(x, z, Season::Winter, 0.0)
            .map(|snow| snow.coverage_fraction)
    })?;
    let water = lake_mesh
        .filter(|lake_mesh| !lake_mesh.indices.is_empty())
        .map(|lake_mesh| renderer.upload_water_mesh(device, lake_mesh))
        .transpose()?;
    Ok((surface, water))
}

fn update_distant_trees(
    device: &wgpu::Device,
    renderer: &TerrainRenderer,
    terrain: &GeneratedWorldTerrain,
    config: ChunkStreamingConfig,
    player_position: WorldPosition,
    tiles: &mut BTreeMap<DistantTreeTileIndex, ResidentDistantTreeTile>,
    pending: &mut VecDeque<DistantTreeMeshSpec>,
) -> Result<(), Box<dyn Error>> {
    let center = DistantTreeTileIndex::containing(player_position)
        .ok_or_else(|| std::io::Error::other("player position is outside tree tile range"))?;
    let load_radius = distant_tree_load_radius(config);
    let retain_radius = load_radius.saturating_add(1);
    let desired = desired_distant_tree_tiles(center, config)?;

    tiles.retain(|tile, _| tile.chebyshev_distance(center) <= retain_radius);
    pending.retain(|spec| desired.get(&spec.tile) == Some(&spec.detail));

    enqueue_distant_tree_replacements(center, &desired, tiles, pending, |resident| resident.spec);

    if let Some(spec) = pending.pop_front() {
        let mesh = distant_tree_mesh_for_tile(device, renderer, terrain, spec)?;
        tiles.insert(spec.tile, ResidentDistantTreeTile { spec, mesh });
    }
    Ok(())
}

fn enqueue_distant_tree_replacements<V>(
    center: DistantTreeTileIndex,
    desired: &BTreeMap<DistantTreeTileIndex, TreeMeshDetail>,
    tiles: &BTreeMap<DistantTreeTileIndex, V>,
    pending: &mut VecDeque<DistantTreeMeshSpec>,
    mut resident_spec: impl FnMut(&V) -> DistantTreeMeshSpec,
) {
    let mut missing = desired
        .iter()
        .filter_map(|(&tile, &detail)| {
            let spec = DistantTreeMeshSpec { tile, detail };
            if tiles
                .get(&tile)
                .is_some_and(|resident| resident_spec(resident) == spec)
                || pending.contains(&spec)
            {
                None
            } else {
                Some(spec)
            }
        })
        .collect::<Vec<_>>();
    missing.sort_by_key(|spec| {
        (
            spec.tile.chebyshev_distance(center),
            spec.tile.z,
            spec.tile.x,
        )
    });
    pending.extend(missing);
}

fn desired_distant_tree_tiles(
    center: DistantTreeTileIndex,
    config: ChunkStreamingConfig,
) -> Result<BTreeMap<DistantTreeTileIndex, TreeMeshDetail>, Box<dyn Error>> {
    let load_radius = distant_tree_load_radius(config);
    let load_radius_i64 = i64::try_from(load_radius)?;
    let high_quality_radius = config
        .load_radius()
        .saturating_mul(DISTANT_TREE_HIGH_QUALITY_DISTANCE_MULTIPLIER)
        .div_ceil(DISTANT_TREE_TILE_CHUNKS_PER_EDGE);
    let simplified_radius = config
        .load_radius()
        .saturating_mul(DISTANT_TREE_SIMPLIFIED_DISTANCE_MULTIPLIER)
        .div_ceil(DISTANT_TREE_TILE_CHUNKS_PER_EDGE);
    let mut desired = BTreeMap::new();
    for z_offset in -load_radius_i64..=load_radius_i64 {
        for x_offset in -load_radius_i64..=load_radius_i64 {
            let tile = DistantTreeTileIndex {
                x: center
                    .x
                    .checked_add(x_offset)
                    .ok_or_else(|| std::io::Error::other("tree tile x index overflow"))?,
                z: center
                    .z
                    .checked_add(z_offset)
                    .ok_or_else(|| std::io::Error::other("tree tile z index overflow"))?,
            };
            let detail = if tile.chebyshev_distance(center) <= high_quality_radius {
                TreeMeshDetail::Full
            } else if tile.chebyshev_distance(center) <= simplified_radius {
                TreeMeshDetail::Simplified
            } else {
                TreeMeshDetail::Silhouette
            };
            desired.insert(tile, detail);
        }
    }
    Ok(desired)
}

const fn distant_tree_load_radius(config: ChunkStreamingConfig) -> u64 {
    config
        .load_radius()
        .saturating_mul(DISTANT_TREE_DISTANCE_MULTIPLIER)
        .div_ceil(DISTANT_TREE_TILE_CHUNKS_PER_EDGE)
}

fn distant_tree_mesh_for_tile(
    device: &wgpu::Device,
    renderer: &TerrainRenderer,
    terrain: &GeneratedWorldTerrain,
    spec: DistantTreeMeshSpec,
) -> Result<Option<TerrainMesh>, Box<dyn Error>> {
    let bounds = spec
        .tile
        .bounds()
        .ok_or_else(|| std::io::Error::other("distant tree tile bounds are invalid"))?;
    let mut trees = trees_for_bounds(terrain, bounds)
        .ok_or_else(|| std::io::Error::other("distant tree generation is unavailable"))?;
    calibrate_surveyed_tree_sizes(terrain, &mut trees);
    trees.retain(|tree| surface_feature_has_dry_ground(terrain, tree.x, tree.z));
    if trees.is_empty() {
        return Ok(None);
    }
    Ok(Some(renderer.upload_trees(
        device,
        &trees,
        spec.detail,
        |x, z| terrain.surface_height(x, z),
    )?))
}

fn calibrate_surveyed_tree_sizes(terrain: &GeneratedWorldTerrain, trees: &mut [ProceduralTree]) {
    if !terrain.is_surveyed_tile() {
        return;
    }
    for tree in trees {
        let Some(canopy) = terrain.surveyed_canopy_at(tree.x, tree.z) else {
            continue;
        };
        let stature = (tree.height_meters / tree.genotype.mature_height_meters).clamp(0.18, 1.0);
        let height_rank = stable_unit_fraction(stable_hash(&[tree.id, 0x4c49_4441_525f_4854]));
        let target_height = canopy.top_height_meters * stature * (0.78 + (height_rank * 0.22));
        let scale = target_height / tree.height_meters;
        tree.height_meters = target_height;
        tree.trunk_base_radius_meters *= scale;
        tree.crown_radius_meters *= scale;
        tree.genotype.mature_height_meters *= scale;
    }
}

fn trees_for_bounds(
    terrain: &GeneratedWorldTerrain,
    bounds: TreeBounds,
) -> Option<Vec<ProceduralTree>> {
    let trees = ProceduralTrees::new(terrain.world());
    if !terrain.is_surveyed_tile() {
        return trees.trees_in(bounds);
    }
    trees.trees_in_with_stand_adjustment(bounds, |x, z, mut stand| {
        let Some(canopy) = terrain.surveyed_canopy_at(x, z) else {
            stand.canopy_cover_fraction = 0.0;
            stand.tree_density_per_hectare = 0.0;
            stand.aboveground_biomass_kg_per_square_meter = 0.0;
            stand.mean_canopy_height_meters = 0.0;
            return stand;
        };
        stand.canopy_cover_fraction = canopy.cover_fraction;
        stand.tree_density_per_hectare =
            lidar_tree_density_per_hectare(canopy.cover_fraction, canopy.top_height_meters);
        stand.mean_canopy_height_meters = canopy.top_height_meters;
        stand.aboveground_biomass_kg_per_square_meter =
            canopy.cover_fraction * canopy.top_height_meters * 0.56;
        stand
    })
}

fn lidar_tree_density_per_hectare(cover_fraction: f64, canopy_height_meters: f64) -> f64 {
    let normalized_height = (canopy_height_meters / 35.0).clamp(0.0, 1.0);
    (cover_fraction * (400.0 + (1_000.0 * (1.0 - normalized_height)))).clamp(0.0, 1_300.0)
}

fn stable_unit_fraction(hash: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let numerator = (hash >> 11) as f64;
    numerator / 9_007_199_254_740_992.0
}

fn rock_mesh_for_chunk(
    device: &wgpu::Device,
    renderer: &TerrainRenderer,
    terrain: &GeneratedWorldTerrain,
    chunk: ChunkIndex,
) -> Result<Option<TerrainMesh>, Box<dyn Error>> {
    let origin = chunk.sample_origin();
    let edge = ChunkIndex::edge_meters();
    let bounds = RockBounds::new(origin.x, origin.z, origin.x + edge, origin.z + edge)
        .ok_or_else(|| std::io::Error::other("rock chunk bounds are invalid"))?;
    let mut rocks = SurfaceRocks::new(terrain.world())
        .rocks_in(bounds)
        .ok_or_else(|| std::io::Error::other("surface-rock generation is unavailable"))?;
    rocks.retain(|rock| surface_feature_has_dry_ground(terrain, rock.x, rock.z));
    if rocks.is_empty() {
        return Ok(None);
    }
    let mesh = renderer.upload_rocks(device, &rocks, |x, z| terrain.surface_height(x, z))?;
    Ok(Some(mesh))
}

fn ground_vegetation_mesh_for_chunk(
    device: &wgpu::Device,
    renderer: &TerrainRenderer,
    terrain: &GeneratedWorldTerrain,
    chunk: ChunkIndex,
) -> Result<Option<TerrainMesh>, Box<dyn Error>> {
    let origin = chunk.sample_origin();
    let edge = ChunkIndex::edge_meters();
    let bounds = GroundVegetationBounds::new(origin.x, origin.z, origin.x + edge, origin.z + edge)
        .ok_or_else(|| std::io::Error::other("ground-vegetation chunk bounds are invalid"))?;
    let mut plants = GroundVegetation::new(terrain.world())
        .plants_in(bounds)
        .ok_or_else(|| std::io::Error::other("ground-vegetation generation is unavailable"))?;
    plants.retain(|plant| surface_feature_has_dry_ground(terrain, plant.x, plant.z));
    if plants.is_empty() {
        return Ok(None);
    }
    let mesh =
        renderer.upload_ground_vegetation(device, &plants, |x, z| terrain.surface_height(x, z))?;
    Ok(Some(mesh))
}

fn surface_feature_has_dry_ground(terrain: &GeneratedWorldTerrain, x: f64, z: f64) -> bool {
    let has_support = terrain.surface_height(x, z).is_some_and(|surface| {
        terrain
            .sample(WorldPosition::new(x, surface - 0.35, z))
            .is_solid()
    });
    has_support
        && terrain.lake_surface_at(x, z).is_none()
        && terrain.ocean_surface_at(x, z).is_none()
        && !terrain
            .river_influence_at(x, z)
            .is_some_and(|river| river.distance_meters <= river.channel_half_width_meters)
}

fn atmosphere_settings(
    terrain: &GeneratedWorldTerrain,
    x: f64,
    z: f64,
) -> Option<AtmosphereSettings> {
    let climate = Climate::new(terrain.world()).sample(x, z)?;
    let soil = Soil::new(terrain.world()).sample(x, z)?;
    let moisture = (soil.surface_moisture * 0.58
        + climate.precipitation_fraction() * 0.32
        + climate.ocean_proximity_fraction * 0.10)
        .clamp(0.0, 1.0);
    let warmth = climate.warmth_fraction();
    Some(AtmosphereSettings {
        fog_color: [
            f64_as_f32(0.36 + (warmth * 0.07) + ((1.0 - moisture) * 0.03)),
            f64_as_f32(0.52 + (warmth * 0.04) + (moisture * 0.05)),
            f64_as_f32(0.66 + ((1.0 - warmth) * 0.07) + (moisture * 0.03)),
        ],
        fog_density: f64_as_f32(0.58 + (moisture * 0.92)),
        moisture: f64_as_f32(moisture),
        prevailing_wind: climate.prevailing_wind.map(f64_as_f32),
    })
}

fn initialize_atmosphere(
    renderer: &TerrainRenderer,
    queue: &wgpu::Queue,
    terrain: &GeneratedWorldTerrain,
    position: WorldPosition,
) -> Option<CellIndex> {
    let cell = CellIndex::containing(position.x, position.z, 0, ATMOSPHERE_CELL_EDGE_METERS);
    if let Some(settings) = atmosphere_settings(terrain, position.x, position.z) {
        renderer.update_atmosphere(queue, settings);
    }
    cell
}

#[allow(clippy::too_many_arguments)]
fn schedule_terrain(
    chunk_streamer: ChunkStreamer,
    far_streamer: FarTerrainStreamer,
    player_position: WorldPosition,
    travel_direction: [f64; 2],
    chunks: &mut BTreeMap<ChunkIndex, ResidentTerrainChunk>,
    far_tiles: &mut BTreeMap<FarTileIndex, ResidentFarTerrainTile>,
    requested: &mut BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far: &mut BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    jobs: &mut ClientTerrainMeshQueue,
) -> Result<(), Box<dyn Error>> {
    let mut tracked_chunks = chunks
        .iter()
        .map(|(&chunk, resident)| (chunk, resident.spec))
        .collect::<BTreeMap<_, _>>();
    tracked_chunks.extend(requested.iter().map(|(&chunk, &spec)| (chunk, spec)));
    let chunk_plan = chunk_streamer
        .plan(player_position, &tracked_chunks)
        .ok_or_else(|| std::io::Error::other("player position is outside chunk index range"))?;

    for chunk in &chunk_plan.unload {
        chunks.remove(chunk);
        if let Some(spec) = requested.remove(chunk) {
            jobs.cancel(TerrainMeshSpec::Near(spec));
        }
    }

    let lod_counts = streamed_lod_counts(&chunk_plan.load);
    for spec in &chunk_plan.load {
        if let Some(previous) = requested.insert(spec.chunk, *spec) {
            jobs.cancel(TerrainMeshSpec::Near(previous));
        }
    }

    let mut tracked_far = far_tiles
        .iter()
        .map(|(&tile, resident)| (tile, resident.spec))
        .collect::<BTreeMap<_, _>>();
    tracked_far.extend(requested_far.iter().map(|(&tile, &spec)| (tile, spec)));
    let far_plan = far_streamer
        .plan(player_position, &tracked_far)
        .ok_or_else(|| std::io::Error::other("player position is outside far tile index range"))?;
    for tile in &far_plan.unload {
        far_tiles.remove(tile);
        if let Some(spec) = requested_far.remove(tile) {
            jobs.cancel(TerrainMeshSpec::Far(spec));
        }
    }
    if let Some(spec) = chunk_plan
        .load
        .iter()
        .find(|spec| spec.chunk == chunk_plan.center)
    {
        jobs.enqueue(
            GenerationPriority::PlayerTerrain,
            TerrainMeshSpec::Near(*spec),
        );
    }
    for spec in &far_plan.load {
        if let Some(previous) = requested_far.insert(spec.tile, *spec) {
            jobs.cancel(TerrainMeshSpec::Far(previous));
        }
        let priority = if spec.tile.chebyshev_distance(far_plan.center)
            == far_streamer.config().load_radius()
        {
            GenerationPriority::Horizon
        } else {
            GenerationPriority::FarTerrain
        };
        jobs.enqueue(priority, TerrainMeshSpec::Far(*spec));
    }
    for spec in chunk_plan
        .load
        .iter()
        .filter(|spec| spec.chunk != chunk_plan.center)
    {
        jobs.enqueue(
            GenerationPriority::NearTerrain,
            TerrainMeshSpec::Near(*spec),
        );
    }

    let prefetched = schedule_prefetch(
        chunk_streamer,
        player_position,
        travel_direction,
        &tracked_chunks,
        jobs,
    )?;

    if chunk_plan.load.is_empty()
        && chunk_plan.unload.is_empty()
        && far_plan.load.is_empty()
        && far_plan.unload.is_empty()
        && prefetched == 0
    {
        return Ok(());
    }
    eprintln!(
        "streaming center ({}, {}): queued {} far tiles, {} chunks [LOD2 {}, LOD3 {}, LOD4 {}], and {} predictive meshes; unloaded {} far / {} near, resident {} far / {} near",
        chunk_plan.center.x,
        chunk_plan.center.z,
        far_plan.load.len(),
        chunk_plan.load.len(),
        lod_counts[0],
        lod_counts[1],
        lod_counts[2],
        prefetched,
        far_plan.unload.len(),
        chunk_plan.unload.len(),
        far_tiles.len(),
        chunks.len()
    );
    Ok(())
}

/// Counts planned chunks per streamed LOD for the streaming report.
///
/// An LOD outside the streamed range is skipped rather than indexed, so
/// widening the range can never panic the streamer.
fn streamed_lod_counts(load: &[ChunkMeshSpec]) -> [usize; 3] {
    let mut counts = [0_usize; 3];
    for spec in load {
        if let Some(count) = spec
            .lod
            .get()
            .checked_sub(ChunkIndex::NEAR_LOD.get())
            .and_then(|offset| counts.get_mut(usize::from(offset)))
        {
            *count += 1;
        }
    }
    counts
}

fn schedule_prefetch(
    chunk_streamer: ChunkStreamer,
    player_position: WorldPosition,
    travel_direction: [f64; 2],
    tracked_chunks: &BTreeMap<ChunkIndex, ChunkMeshSpec>,
    jobs: &mut ClientTerrainMeshQueue,
) -> Result<usize, Box<dyn Error>> {
    let specs = chunk_streamer
        .prefetch_specs(
            player_position,
            travel_direction,
            TERRAIN_PREFETCH_CENTERS_AHEAD,
        )
        .ok_or_else(|| std::io::Error::other("terrain prefetch exceeds chunk index range"))?;
    let desired = specs
        .iter()
        .copied()
        .map(TerrainMeshSpec::Near)
        .collect::<BTreeSet<_>>();
    jobs.retain_prewarm(&desired);
    Ok(specs
        .into_iter()
        .filter(|spec| tracked_chunks.get(&spec.chunk) != Some(spec))
        .filter(|spec| jobs.prewarm(TerrainMeshSpec::Near(*spec)))
        .count())
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[allow(clippy::cast_precision_loss)]
fn u32_as_f32(value: u32) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    struct OpenSphericalCave;

    impl DensityField for OpenSphericalCave {
        fn sample(&self, position: WorldPosition) -> treeline_terrain::TerrainSample {
            let cave_void =
                5.5 - libm::hypot(libm::hypot(position.x, position.y + 5.0), position.z);
            let density = position.y.max(cave_void);
            treeline_terrain::TerrainSample::new(
                density,
                if density > 0.0 {
                    treeline_terrain::Material::Air
                } else {
                    treeline_terrain::Material::Rock
                },
            )
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct SlopedGround;

    impl SlopedGround {
        fn surface_height(x: f64, z: f64) -> f64 {
            (x * 0.25) - (z * 0.125)
        }
    }

    impl DensityField for SlopedGround {
        fn sample(&self, position: WorldPosition) -> treeline_terrain::TerrainSample {
            let density = position.y - Self::surface_height(position.x, position.z);
            treeline_terrain::TerrainSample::new(
                density,
                if density > 0.0 {
                    treeline_terrain::Material::Air
                } else {
                    treeline_terrain::Material::Rock
                },
            )
        }
    }

    impl SurfaceField for SlopedGround {
        fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
            (x.is_finite() && z.is_finite()).then(|| Self::surface_height(x, z))
        }
    }

    #[test]
    fn walkable_floor_descends_through_an_open_cave_mouth() {
        let floor = walkable_floor_height(&OpenSphericalCave, 0.0, 0.0, 0.0).expect("cave floor");
        assert!((floor + 10.5).abs() < 0.01);
    }

    #[test]
    fn solid_cave_wall_has_no_walkable_floor() {
        assert!(walkable_floor_height(&OpenSphericalCave, 8.0, 0.0, -10.5).is_none());
    }

    #[test]
    fn walking_keeps_submeter_steps_at_the_random_warp_limit() {
        let terrain = GeneratedWorldTerrain::new(WORLD);
        let mut input = InputState::default();
        input.set_key(KeyCode::KeyW, true);
        let y = surface_height(&terrain, 5_000_000.0, -5_000_000.0) + EYE_HEIGHT;
        let mut camera = Camera::new(DVec3::new(5_000_000.0, y, -5_000_000.0), 0.0, 0.0);
        let previous_x = camera.position.x;

        camera.walk(&input, &terrain, 1.0 / 60.0);

        let expected_step = WALK_SPEED / 60.0;
        assert!((camera.position.x - previous_x - expected_step).abs() < 1.0e-9);
    }

    #[test]
    fn aerial_toggle_uses_two_hundred_meter_ground_clearance() {
        let terrain = SlopedGround;
        let x = 24.0;
        let z = -12.0;
        let yaw = 0.75;
        let pitch = -0.2;
        let surface = SlopedGround::surface_height(x, z);
        let mut camera = Camera::new(DVec3::new(x, surface + EYE_HEIGHT, z), yaw, pitch);

        assert!(camera.toggle_aerial_mode(&terrain));
        assert_eq!(camera.mode, CameraMode::Aerial);
        assert_eq!(camera.position, DVec3::new(x, surface + 200.0, z));
        assert!((camera.yaw - yaw).abs() < f64::EPSILON);
        assert!((camera.pitch - pitch).abs() < f64::EPSILON);

        assert!(!camera.toggle_aerial_mode(&terrain));
        assert_eq!(camera.mode, CameraMode::Ground);
        assert_eq!(camera.position, DVec3::new(x, surface + EYE_HEIGHT, z));
        assert!((camera.yaw - yaw).abs() < f64::EPSILON);
        assert!((camera.pitch - pitch).abs() < f64::EPSILON);
    }

    #[test]
    fn surface_relative_destination_height_uses_the_current_camera_mode() {
        let terrain = SlopedGround;
        let target_x = 80.0;
        let target_z = -40.0;
        let surface = SlopedGround::surface_height(target_x, target_z);
        let mut camera = Camera::new(DVec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);

        assert!(
            (camera.surface_relative_y(&terrain, target_x, target_z) - (surface + EYE_HEIGHT))
                .abs()
                < f64::EPSILON
        );
        camera.toggle_aerial_mode(&terrain);
        assert!(
            (camera.surface_relative_y(&terrain, target_x, target_z)
                - (surface + AERIAL_HEIGHT_METERS))
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn aerial_movement_is_ten_times_faster_and_tracks_the_ground() {
        let terrain = SlopedGround;
        let mut input = InputState::default();
        input.set_key(KeyCode::KeyW, true);
        let mut ground_camera = Camera::new(DVec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);
        let mut aerial_camera = Camera::new(DVec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);
        aerial_camera.toggle_aerial_mode(&terrain);

        ground_camera.walk(&input, &terrain, 0.25);
        aerial_camera.walk(&input, &terrain, 0.25);

        assert!((aerial_camera.position.x - (ground_camera.position.x * 10.0)).abs() < 1.0e-9);
        assert!(aerial_camera.position.z.abs() < f64::EPSILON);
        assert!(
            (aerial_camera.position.y
                - (SlopedGround::surface_height(aerial_camera.position.x, 0.0)
                    + AERIAL_HEIGHT_METERS))
                .abs()
                < 1.0e-9
        );
    }

    #[test]
    fn aerial_sprinting_is_ten_times_faster_than_ground_sprinting() {
        let terrain = SlopedGround;
        let mut input = InputState::default();
        input.set_key(KeyCode::KeyW, true);
        input.set_key(KeyCode::ShiftLeft, true);
        let mut ground_camera = Camera::new(DVec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);
        let mut aerial_camera = Camera::new(DVec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);
        aerial_camera.toggle_aerial_mode(&terrain);

        ground_camera.walk(&input, &terrain, 0.25);
        aerial_camera.walk(&input, &terrain, 0.25);

        assert!((aerial_camera.position.x - (ground_camera.position.x * 10.0)).abs() < 1.0e-9);
    }

    #[test]
    fn toggle_requests_preserve_click_parity_between_frames() {
        let requests = ToggleRequestCounter::default();

        requests.request();
        requests.request();
        assert!(!requests.take());

        requests.request();
        requests.request();
        requests.request();
        assert!(requests.take());
        assert!(!requests.take());
    }

    #[test]
    fn initial_generation_reports_horizon_far_and_near_separately() {
        let near_spec = ChunkMeshSpec {
            chunk: ChunkIndex::new(0, 0),
            lod: ChunkIndex::NEAR_LOD,
            transition_faces: treeline_voxel::TransitionFaces::none(),
        };
        let requested = BTreeMap::from([(near_spec.chunk, near_spec)]);
        let horizon_spec = FarTerrainMeshSpec {
            tile: FarTileIndex::new(1, 0),
        };
        let far_spec = FarTerrainMeshSpec {
            tile: FarTileIndex::new(0, 0),
        };
        let requested_far =
            BTreeMap::from([(horizon_spec.tile, horizon_spec), (far_spec.tile, far_spec)]);
        let far_streamer =
            FarTerrainStreamer::new(FarTerrainStreamingConfig::new(1, 1).expect("valid radii"));
        let mut progress = InitialGenerationProgress::new(
            Instant::now(),
            Duration::ZERO,
            &requested,
            &requested_far,
            far_streamer,
            WorldPosition::new(0.0, 0.0, 0.0),
        )
        .expect("valid progress");

        assert!(
            progress
                .title()
                .contains("horizon 0/1 · far 0/1 · nearby 0/1")
        );
        for spec in [
            TerrainMeshSpec::Far(horizon_spec),
            TerrainMeshSpec::Far(far_spec),
            TerrainMeshSpec::Near(near_spec),
        ] {
            progress.record_completion(spec, Duration::ZERO, Duration::ZERO, Duration::ZERO);
        }

        assert!(progress.finished_at.is_some());
    }

    #[test]
    fn far_cutout_tracks_the_complete_near_residency_square() {
        let streamer = ChunkStreamer::new(ChunkStreamingConfig::new(4, 5).expect("valid radii"));
        let (min, max) =
            far_cutout_bounds(streamer, WorldPosition::new(0.0, 0.0, 0.0)).expect("valid cutout");

        assert!(
            min.into_iter()
                .zip([-128.0, -128.0])
                .all(|(actual, expected)| (actual - expected).abs() < f64::EPSILON)
        );
        assert!(
            max.into_iter()
                .zip([160.0, 160.0])
                .all(|(actual, expected)| (actual - expected).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn random_warp_distance_accepts_one_to_five_thousand_kilometers() {
        let current = WorldPosition::new(0.0, 0.0, 0.0);

        assert!(!random_warp_distance_is_eligible(
            current,
            [RANDOM_WARP_MIN_DISTANCE_METERS - 1.0, 0.0]
        ));
        assert!(random_warp_distance_is_eligible(
            current,
            [RANDOM_WARP_MIN_DISTANCE_METERS, 0.0]
        ));
        assert!(random_warp_distance_is_eligible(
            current,
            [RANDOM_WARP_MAX_DISTANCE_METERS, 0.0]
        ));
        assert!(!random_warp_distance_is_eligible(
            current,
            [RANDOM_WARP_MAX_DISTANCE_METERS + 1.0, 0.0]
        ));
    }

    #[test]
    fn random_warp_destination_clamps_to_the_precise_coordinate_budget() {
        let minimum = random_warp_destination(-1.0, -1.0);
        let center = random_warp_destination(0.5, 0.5);
        let maximum = random_warp_destination(2.0, 2.0);

        for (actual, expected) in [
            (minimum, [-RANDOM_WARP_COORDINATE_LIMIT_METERS; 2]),
            (center, [0.0; 2]),
            (maximum, [RANDOM_WARP_COORDINATE_LIMIT_METERS; 2]),
        ] {
            assert!(
                actual
                    .into_iter()
                    .zip(expected)
                    .all(|(actual, expected)| (actual - expected).abs() < f64::EPSILON)
            );
        }
    }

    #[test]
    fn surveyed_warp_destination_stays_inside_the_measured_tile() {
        for (actual, expected) in [
            (
                surveyed_warp_destination(-1.0, -1.0),
                [SURVEYED_WARP_BORDER_CLEARANCE_METERS; 2],
            ),
            (
                surveyed_warp_destination(2.0, 2.0),
                [DEFAULT_SURVEYED_TILE_EDGE_METERS - SURVEYED_WARP_BORDER_CLEARANCE_METERS; 2],
            ),
        ] {
            assert!(
                actual
                    .into_iter()
                    .zip(expected)
                    .all(|(actual, expected)| (actual - expected).abs() < f64::EPSILON)
            );
        }
    }

    #[test]
    fn distant_tree_replacement_keeps_the_resident_mesh_until_successor_is_ready() {
        let tile = DistantTreeTileIndex { x: 0, z: 0 };
        let old_spec = DistantTreeMeshSpec {
            tile,
            detail: TreeMeshDetail::Simplified,
        };
        let new_spec = DistantTreeMeshSpec {
            tile,
            detail: TreeMeshDetail::Full,
        };
        let desired = BTreeMap::from([(tile, new_spec.detail)]);
        let resident = BTreeMap::from([(tile, old_spec)]);
        let mut pending = VecDeque::new();

        enqueue_distant_tree_replacements(tile, &desired, &resident, &mut pending, |spec| *spec);

        assert_eq!(resident.get(&tile), Some(&old_spec));
        assert_eq!(pending, VecDeque::from([new_spec]));
    }

    #[test]
    fn distant_tree_plan_uses_full_simplified_and_silhouette_twenty_times_out() {
        let config = ChunkStreamingConfig::new(4, 5).expect("valid terrain radii");
        let center = DistantTreeTileIndex { x: 0, z: 0 };
        let desired = desired_distant_tree_tiles(center, config).expect("distant tree plan");
        let load_radius = distant_tree_load_radius(config);

        assert_eq!(load_radius, 20);
        assert_eq!(desired.len(), 1_681);
        assert_eq!(
            desired.get(&DistantTreeTileIndex { x: 5, z: 0 }),
            Some(&TreeMeshDetail::Full)
        );
        assert_eq!(
            desired.get(&DistantTreeTileIndex { x: 6, z: 0 }),
            Some(&TreeMeshDetail::Simplified)
        );
        assert_eq!(
            desired.get(&DistantTreeTileIndex { x: 10, z: 0 }),
            Some(&TreeMeshDetail::Simplified)
        );
        assert_eq!(
            desired.get(&DistantTreeTileIndex { x: 11, z: 0 }),
            Some(&TreeMeshDetail::Silhouette)
        );
        assert_eq!(
            load_radius * DISTANT_TREE_TILE_CHUNKS_PER_EDGE,
            config.load_radius() * DISTANT_TREE_DISTANCE_MULTIPLIER
        );
    }

    #[test]
    fn distant_tree_tiles_are_half_open_across_negative_boundaries() {
        assert_eq!(
            DistantTreeTileIndex::containing(WorldPosition::new(-0.001, 0.0, -128.0)),
            Some(DistantTreeTileIndex { x: -1, z: -1 })
        );
        assert_eq!(
            DistantTreeTileIndex::containing(WorldPosition::new(0.0, 0.0, 127.999)),
            Some(DistantTreeTileIndex { x: 0, z: 0 })
        );
        assert_eq!(
            DistantTreeTileIndex::containing(WorldPosition::new(128.0, 0.0, 128.0)),
            Some(DistantTreeTileIndex { x: 1, z: 1 })
        );
    }

    #[test]
    fn prototype_region_exposes_real_lake() {
        const RESET_LAKE_REGION: [f64; 2] = [-36_032_000.0, -15_744_000.0];
        let terrain = GeneratedWorldTerrain::new(WorldIdentity::new(0x5eed, 18, 0));
        let lake_point = terrain
            .regional_lakes_at(RESET_LAKE_REGION[0], RESET_LAKE_REGION[1])
            .and_then(|lakes| {
                lakes
                    .into_iter()
                    .find_map(|lake| visible_lake_water_point(&terrain, lake))
            })
            .expect("the version 18 prototype region should contain visible equilibrium water");
        for [x_offset, z_offset] in [[0.0, 0.0], [128.0, 0.0], [-128.0, 0.0], [0.0, 128.0]] {
            assert!(
                terrain
                    .lake_surface_at(lake_point[0] + x_offset, lake_point[1] + z_offset)
                    .is_some_and(|water| water.water_depth_meters >= 0.5),
                "the prototype region should retain a broad, visible lake"
            );
        }
    }

    #[test]
    fn water_warp_places_the_player_on_dry_ground_facing_visible_water() {
        let terrain = GeneratedWorldTerrain::new(WORLD);
        let lake_site = [
            [-36_032_000.0, -15_744_000.0],
            [-192_000.0, -192_000.0],
            [-64_000.0, -64_000.0],
            [64_000.0, 64_000.0],
            [192_000.0, 192_000.0],
        ]
        .into_iter()
        .find_map(|anchor| {
            terrain
                .regional_lakes_at(anchor[0], anchor[1])?
                .iter()
                .find_map(|&lake| {
                    let deep_water = visible_lake_water_point(&terrain, lake)?;
                    let body = WaterBody::Lake(lake);
                    let (destination, shore_water) =
                        dry_water_shore(&terrain, body, deep_water, 0.0)?;
                    Some((body, destination, shore_water))
                })
        });
        let ocean_site = || {
            let landform = WildernessTerrain::new(WORLD);
            for z_index in -64_i32..=64 {
                let z = f64::from(z_index) * 64_000.0;
                for x_index in -64_i32..64 {
                    let left = [f64::from(x_index) * 64_000.0, z];
                    let right = [left[0] + 64_000.0, z];
                    let left_height = landform.height_at(left[0], left[1])?;
                    let right_height = landform.height_at(right[0], right[1])?;
                    let (water, direction_fraction) = if left_height < 0.0 && right_height >= 0.0 {
                        (left, 0.0)
                    } else if right_height < 0.0 && left_height >= 0.0 {
                        (right, 0.5)
                    } else {
                        continue;
                    };
                    if terrain
                        .ocean_surface_at(water[0], water[1])
                        .is_some_and(|sample| {
                            sample.water_depth_meters >= WATER_WARP_MIN_DEPTH_METERS
                        })
                        && let Some((destination, shore_water)) =
                            dry_water_shore(&terrain, WaterBody::Ocean, water, direction_fraction)
                    {
                        return Some((WaterBody::Ocean, destination, shore_water));
                    }
                }
            }
            None
        };
        let (body, destination, shore_water) = lake_site
            .or_else(ocean_site)
            .expect("the representative regions should have a reachable water shore");

        assert!(surface_feature_has_dry_ground(
            &terrain,
            destination[0],
            destination[1]
        ));
        assert!(same_body_water(&terrain, body, shore_water));
        assert!(
            (destination[0] - shore_water[0]).hypot(destination[1] - shore_water[1]) <= 40.0,
            "the player should arrive close enough to see the water"
        );
    }

    #[test]
    fn atmosphere_controls_are_deterministic_and_bounded_by_local_geography() {
        let terrain = GeneratedWorldTerrain::new(WORLD);
        let first = atmosphere_settings(&terrain, START_X, START_Z).expect("atmosphere");
        let repeated = atmosphere_settings(&terrain, START_X, START_Z).expect("atmosphere");

        assert_eq!(
            first.fog_color.map(f32::to_bits),
            repeated.fog_color.map(f32::to_bits)
        );
        assert_eq!(first.fog_density.to_bits(), repeated.fog_density.to_bits());
        assert_eq!(first.moisture.to_bits(), repeated.moisture.to_bits());
        assert_eq!(
            first.prevailing_wind.map(f32::to_bits),
            repeated.prevailing_wind.map(f32::to_bits)
        );
        assert!((0.0..=1.0).contains(&first.moisture));
        assert!((0.58..=1.5).contains(&first.fog_density));
        assert!(
            first
                .fog_color
                .into_iter()
                .all(|channel| (0.0..=1.0).contains(&channel))
        );
        assert!(first.prevailing_wind.into_iter().all(f32::is_finite));
    }

    #[test]
    fn showcase_spawn_is_clear_and_retains_trees_and_ground_cover() {
        let terrain = GeneratedWorldTerrain::new(WORLD);
        assert!(
            terrain.lake_surface_at(START_X, START_Z).is_none(),
            "spawn should begin on dry ground"
        );
        let center =
            ChunkIndex::containing(WorldPosition::new(START_X, 0.0, START_Z)).expect("spawn chunk");
        let cutout = NearTerrainCutout::around(center, chunk_streaming_config().load_radius())
            .expect("spawn residency");
        let min = cutout.min.sample_origin();
        let max = cutout.max_exclusive.sample_origin();
        let bounds = TreeBounds::new(min.x, min.z, max.x, max.z).expect("spawn tree bounds");
        let retained = ProceduralTrees::new(WORLD)
            .trees_in(bounds)
            .expect("tree generation")
            .into_iter()
            .filter(|tree| surface_feature_has_dry_ground(&terrain, tree.x, tree.z))
            .collect::<Vec<_>>();
        let nearest_tree = retained
            .iter()
            .min_by(|left, right| {
                let left_clearance =
                    (left.x - START_X).hypot(left.z - START_Z) - left.crown_radius_meters;
                let right_clearance =
                    (right.x - START_X).hypot(right.z - START_Z) - right.crown_radius_meters;
                left_clearance.total_cmp(&right_clearance)
            })
            .expect("visible stand has a nearest tree");
        let crown_clearance = (nearest_tree.x - START_X).hypot(nearest_tree.z - START_Z)
            - nearest_tree.crown_radius_meters;

        assert!(retained.len() >= 20, "spawn should retain a visible stand");
        assert!(
            crown_clearance >= 2.0,
            "spawn should not begin inside a tree crown; clearance is {crown_clearance:.2} m near ({:.2}, {:.2})",
            nearest_tree.x,
            nearest_tree.z
        );

        let rock_bounds =
            RockBounds::new(min.x, min.z, max.x, max.z).expect("spawn surface-rock bounds");
        let rocks = SurfaceRocks::new(WORLD)
            .rocks_in(rock_bounds)
            .expect("surface-rock generation")
            .into_iter()
            .filter(|rock| surface_feature_has_dry_ground(&terrain, rock.x, rock.z))
            .collect::<Vec<_>>();
        let nearest_rock = rocks
            .iter()
            .min_by(|left, right| {
                let clearance = |rock: &&treeline_ecology::SurfaceRock| {
                    (rock.x - START_X).hypot(rock.z - START_Z)
                        - rock.radii_meters[0].max(rock.radii_meters[2])
                };
                clearance(left).total_cmp(&clearance(right))
            })
            .expect("spawn region has a nearest surface rock");
        let rock_clearance = (nearest_rock.x - START_X).hypot(nearest_rock.z - START_Z)
            - nearest_rock.radii_meters[0].max(nearest_rock.radii_meters[2]);
        assert!(
            rock_clearance >= 1.0,
            "spawn should not begin inside a surface rock; clearance is {rock_clearance:.2} m near ({:.2}, {:.2})",
            nearest_rock.x,
            nearest_rock.z
        );

        let vegetation_bounds = GroundVegetationBounds::new(min.x, min.z, max.x, max.z)
            .expect("spawn ground-vegetation bounds");
        let plants = GroundVegetation::new(WORLD)
            .plants_in(vegetation_bounds)
            .expect("ground-vegetation generation")
            .into_iter()
            .filter(|plant| surface_feature_has_dry_ground(&terrain, plant.x, plant.z))
            .collect::<Vec<_>>();
        assert!(
            plants.len() >= 100,
            "spawn should retain a visibly populated ground layer; found {} plants",
            plants.len()
        );
    }

    #[test]
    fn surveyed_canopy_creates_openings_and_height_bounded_dense_stands() {
        let surveyed_world = WorldIdentity::new(
            0x5eed,
            CURRENT_GENERATOR_VERSION,
            treeline_terrain::DEFAULT_SURVEYED_SETTINGS_HASH,
        );
        let terrain = GeneratedWorldTerrain::new(surveyed_world);
        let open_bounds = TreeBounds::new(8_928.0, 1_248.0, 9_024.0, 1_344.0).unwrap();
        let dense_bounds = TreeBounds::new(2_688.0, 5_952.0, 2_784.0, 6_048.0).unwrap();
        let open = trees_for_bounds(&terrain, open_bounds).unwrap();
        let mut dense = trees_for_bounds(&terrain, dense_bounds).unwrap();

        assert!(open.is_empty(), "lidar-open cells should remain open");
        assert!(
            dense.len() >= 50,
            "closed canopy should produce a dense stand"
        );
        calibrate_surveyed_tree_sizes(&terrain, &mut dense);
        assert!(dense.iter().all(|tree| {
            let canopy = terrain.surveyed_canopy_at(tree.x, tree.z).unwrap();
            tree.height_meters >= 0.0 && tree.height_meters <= canopy.top_height_meters
        }));
    }

    #[test]
    fn lidar_density_mapping_is_higher_for_closed_and_shorter_canopies() {
        let open = lidar_tree_density_per_hectare(0.15, 18.0);
        let closed = lidar_tree_density_per_hectare(0.90, 18.0);
        let short_closed = lidar_tree_density_per_hectare(0.90, 8.0);

        assert!(closed > open);
        assert!(short_closed > closed);
        assert!(closed > 750.0);
    }

    #[test]
    fn surface_feature_filter_excludes_channels_without_clearing_river_valleys() {
        let terrain = GeneratedWorldTerrain::new(WORLD);
        let mut channel = None;
        let mut valley = None;
        for z_index in -16_i32..=16 {
            for x_index in -16_i32..=16 {
                let x = START_X + (f64::from(x_index) * 2_000.0) + 1_000.0;
                let z = START_Z + (f64::from(z_index) * 2_000.0) + 1_000.0;
                let Some(river) = terrain.river_influence_at(x, z) else {
                    continue;
                };
                if river.distance_meters <= river.channel_half_width_meters {
                    channel.get_or_insert([x, z]);
                    let segment_x = river.segment.mouth.x - river.segment.source.x;
                    let segment_z = river.segment.mouth.z - river.segment.source.z;
                    let length = segment_x.hypot(segment_z);
                    let perpendicular = DVec2::new(-segment_z / length, segment_x / length);
                    for side in [-1.0, 1.0] {
                        let distance = river.channel_half_width_meters
                            + ((river.valley_half_width_meters - river.channel_half_width_meters)
                                * 0.33);
                        let candidate_x = x + (perpendicular.x * distance * side);
                        let candidate_z = z + (perpendicular.y * distance * side);
                        if terrain
                            .river_influence_at(candidate_x, candidate_z)
                            .is_some_and(|candidate| candidate.blend > 0.24)
                            && terrain.lake_surface_at(candidate_x, candidate_z).is_none()
                            && terrain.ocean_surface_at(candidate_x, candidate_z).is_none()
                        {
                            valley.get_or_insert([candidate_x, candidate_z]);
                        }
                    }
                }
            }
        }

        let [channel_x, channel_z] = channel.expect("search area should contain a river channel");
        let [valley_x, valley_z] = valley.expect("search area should contain a river valley");
        assert!(!surface_feature_has_dry_ground(
            &terrain, channel_x, channel_z
        ));
        assert!(surface_feature_has_dry_ground(&terrain, valley_x, valley_z));
    }

    #[test]
    fn twin_sticks_assign_left_to_movement_and_right_to_look() {
        let mut sticks = VirtualSticks::default();
        sticks.set_radius(100.0);
        sticks.begin(1, Vec2::new(100.0, 500.0), 1_000.0);
        sticks.begin(2, Vec2::new(900.0, 500.0), 1_000.0);
        sticks.update(1, Vec2::new(150.0, 400.0));
        sticks.update(2, Vec2::new(850.0, 550.0));

        assert_eq!(sticks.movement_axis(), Vec2::new(0.5, 1.0).normalize());
        assert_eq!(sticks.look_axis(), Vec2::new(-0.5, -0.5));
    }

    #[test]
    fn releasing_a_touch_only_resets_its_stick() {
        let mut sticks = VirtualSticks::default();
        sticks.begin(1, Vec2::new(100.0, 500.0), 1_000.0);
        sticks.begin(2, Vec2::new(900.0, 500.0), 1_000.0);
        sticks.update(1, Vec2::new(130.0, 500.0));
        sticks.update(2, Vec2::new(870.0, 500.0));
        sticks.end(1);

        assert_eq!(sticks.movement_axis(), Vec2::ZERO);
        assert_ne!(sticks.look_axis(), Vec2::ZERO);
    }
}
