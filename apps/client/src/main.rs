use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

use glam::{Mat4, Vec2, Vec3};
use treeline_coordinates::{WorldIdentity, WorldPosition};
use treeline_ecology::{ProceduralTrees, TreeBounds};
use treeline_renderer::{TerrainMesh, TerrainRenderer};
use treeline_terrain::SurfaceField;
use treeline_voxel::ChunkIndex;
use treeline_world::{
    CURRENT_GENERATOR_VERSION, ChunkMeshSpec, ChunkStreamer, ChunkStreamingConfig,
    FarTerrainMeshSpec, FarTerrainStreamer, FarTerrainStreamingConfig, FarTileIndex,
    GeneratedWorldTerrain, GenerationPriority, NearTerrainCutout, TerrainMeshQueue,
    TerrainMeshSpec,
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

const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, CURRENT_GENERATOR_VERSION, 0);
const WINDOW_TITLE: &str = "Treeline — Infinite Landscape";
const EYE_HEIGHT: f32 = 1.72;
const WALK_SPEED: f32 = 8.0;
const SPRINT_SPEED: f32 = 16.0;
const MAX_TERRAIN_INTEGRATIONS_PER_FRAME: usize = 2;
const TERRAIN_INTEGRATION_BUDGET: Duration = Duration::from_millis(3);

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
        if self.game.is_none() {
            if let Some(result) = self.pending_game.borrow_mut().take() {
                match result {
                    Ok(game) => self.game = Some(game),
                    Err(error) => {
                        eprintln!("failed to start Treeline: {error}");
                        event_loop.exit();
                    }
                }
            }
        }

        if let Some(game) = &self.game {
            game.window.request_redraw();
        }
    }
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
    requested_chunks: BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far_tiles: BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    terrain_jobs: TerrainMeshQueue<GeneratedWorldTerrain>,
    chunk_streamer: ChunkStreamer,
    far_terrain_streamer: FarTerrainStreamer,
    terrain: GeneratedWorldTerrain,
    camera: Camera,
    input: InputState,
    cursor_captured: bool,
    previous_frame: Instant,
    initial_generation: InitialGenerationProgress,
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
        let renderer = TerrainRenderer::new(
            &device,
            surface_config.format,
            surface_config.width,
            surface_config.height,
        );
        let chunk_streamer = ChunkStreamer::new(chunk_streaming_config());
        let far_terrain_streamer = FarTerrainStreamer::new(far_terrain_streaming_config());
        let mut terrain_chunks = BTreeMap::new();
        let mut far_terrain_tiles = BTreeMap::new();
        let mut requested_chunks = BTreeMap::new();
        let mut requested_far_tiles = BTreeMap::new();
        let mut terrain_jobs = TerrainMeshQueue::with_lake_mesh(
            terrain.clone(),
            GeneratedWorldTerrain::lake_surface_mesh,
        );

        let start_x = 16.0;
        let start_z = 80.0;
        let spawn_preparation_started = Instant::now();
        let start_y = surface_height(&terrain, start_x, start_z) + EYE_HEIGHT;
        let spawn_preparation_time = spawn_preparation_started.elapsed();
        let camera = Camera::new(Vec3::new(start_x, start_y, start_z));
        schedule_terrain(
            chunk_streamer,
            far_terrain_streamer,
            camera.world_position(),
            &mut terrain_chunks,
            &mut far_terrain_tiles,
            &mut requested_chunks,
            &mut requested_far_tiles,
            &mut terrain_jobs,
        )?;
        renderer.update_camera(
            &queue,
            camera.view_projection(surface_config.width, surface_config.height),
        );
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

    fn update(&mut self) {
        let now = Instant::now();
        let delta_seconds = (now - self.previous_frame).as_secs_f32().min(0.1);
        self.previous_frame = now;
        self.camera
            .look_with_stick(self.input.look_axis(), delta_seconds);
        self.camera.walk(&self.input, &self.terrain, delta_seconds);
        if let Err(error) = update_terrain(
            &self.device,
            &self.renderer,
            &self.terrain,
            self.chunk_streamer,
            self.far_terrain_streamer,
            self.camera.world_position(),
            &mut self.terrain_chunks,
            &mut self.far_terrain_tiles,
            &mut self.requested_chunks,
            &mut self.requested_far_tiles,
            &mut self.terrain_jobs,
            &mut self.initial_generation,
        ) {
            eprintln!("terrain chunk streaming failed: {error}");
        }
        self.initial_generation.publish(&self.window);
        self.renderer.update_camera(
            &self.queue,
            self.camera
                .view_projection(self.surface_config.width, self.surface_config.height),
        );
        if let Ok((cutout_min, cutout_max)) =
            far_cutout_bounds(self.chunk_streamer, self.camera.world_position())
        {
            self.renderer
                .update_far_cutout(&self.queue, cutout_min, cutout_max);
        }
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
                    self.terrain_chunks
                        .values()
                        .filter_map(|resident| resident.tree_mesh.as_ref()),
                ),
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
    tree_mesh: Option<TerrainMesh>,
}

struct ResidentFarTerrainTile {
    spec: FarTerrainMeshSpec,
    mesh: TerrainMesh,
    lake_mesh: Option<TerrainMesh>,
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

struct Camera {
    position: Vec3,
    yaw: f32,
    pitch: f32,
}

impl Camera {
    const fn new(position: Vec3) -> Self {
        Self {
            position,
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
        }
    }

    fn direction(&self) -> Vec3 {
        let pitch_cosine = libm::cosf(self.pitch);
        Vec3::new(
            libm::cosf(self.yaw) * pitch_cosine,
            libm::sinf(self.pitch),
            libm::sinf(self.yaw) * pitch_cosine,
        )
        .normalize()
    }

    fn look(&mut self, delta_x: f64, delta_y: f64) {
        const SENSITIVITY: f32 = 0.002;
        self.yaw += f64_as_f32(delta_x) * SENSITIVITY;
        self.pitch = (self.pitch - (f64_as_f32(delta_y) * SENSITIVITY)).clamp(-1.5, 1.5);
    }

    fn look_with_stick(&mut self, axis: Vec2, delta_seconds: f32) {
        const HORIZONTAL_SPEED: f32 = 2.4;
        const VERTICAL_SPEED: f32 = 1.8;
        self.yaw += axis.x * HORIZONTAL_SPEED * delta_seconds;
        self.pitch = (self.pitch + (axis.y * VERTICAL_SPEED * delta_seconds)).clamp(-1.5, 1.5);
    }

    fn walk(&mut self, input: &InputState, terrain: &GeneratedWorldTerrain, delta_seconds: f32) {
        let forward = Vec3::new(libm::cosf(self.yaw), 0.0, libm::sinf(self.yaw));
        let right = forward.cross(Vec3::Y);
        let movement = (forward * input.forward_axis()) + (right * input.right_axis());
        if movement.length_squared() > 0.0 {
            let speed = if input.sprint() {
                SPRINT_SPEED
            } else {
                WALK_SPEED
            };
            let intensity = movement.length().min(1.0);
            self.position += movement.normalize() * intensity * speed * delta_seconds;
        }
        self.position.y = surface_height(terrain, self.position.x, self.position.z) + EYE_HEIGHT;
    }

    fn world_position(&self) -> WorldPosition {
        WorldPosition::new(
            f64::from(self.position.x),
            f64::from(self.position.y),
            f64::from(self.position.z),
        )
    }

    fn view_projection(&self, width: u32, height: u32) -> [[f32; 4]; 4] {
        let aspect = u32_as_f32(width) / u32_as_f32(height.max(1));
        let projection = Mat4::perspective_rh(60.0_f32.to_radians(), aspect, 0.1, 50_000.0);
        let view = Mat4::look_to_rh(self.position, self.direction(), Vec3::Y);
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

fn surface_height(terrain: &impl SurfaceField, x: f32, z: f32) -> f32 {
    let height = terrain
        .surface_height(f64::from(x), f64::from(z))
        .expect("finite player positions must have terrain");
    f64_as_f32(height)
}

fn chunk_streaming_config() -> ChunkStreamingConfig {
    if cfg!(target_arch = "wasm32") {
        ChunkStreamingConfig::new(2, 3).expect("the browser streaming radii are valid")
    } else {
        ChunkStreamingConfig::default()
    }
}

fn far_terrain_streaming_config() -> FarTerrainStreamingConfig {
    if cfg!(target_arch = "wasm32") {
        FarTerrainStreamingConfig::new(4, 5).expect("the browser streaming radii are valid")
    } else {
        FarTerrainStreamingConfig::default()
    }
}

fn far_cutout_bounds(
    chunk_streamer: ChunkStreamer,
    player_position: WorldPosition,
) -> Result<([f32; 2], [f32; 2]), Box<dyn Error>> {
    let center = ChunkIndex::containing(player_position)
        .ok_or_else(|| std::io::Error::other("player position is outside chunk index range"))?;
    let cutout = NearTerrainCutout::around(center, chunk_streamer.config().load_radius())
        .ok_or_else(|| std::io::Error::other("near terrain cutout is outside chunk index range"))?;
    let min = cutout.min.sample_origin();
    let max = cutout.max_exclusive.sample_origin();
    Ok((
        [f64_as_f32(min.x), f64_as_f32(min.z)],
        [f64_as_f32(max.x), f64_as_f32(max.z)],
    ))
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
    chunks: &mut BTreeMap<ChunkIndex, ResidentTerrainChunk>,
    far_tiles: &mut BTreeMap<FarTileIndex, ResidentFarTerrainTile>,
    requested: &mut BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far: &mut BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    jobs: &mut TerrainMeshQueue<GeneratedWorldTerrain>,
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
        let expected = match spec {
            TerrainMeshSpec::Near(spec) => requested.get(&spec.chunk) == Some(&spec),
            TerrainMeshSpec::Far(spec) => requested_far.get(&spec.tile) == Some(&spec),
        };
        if !expected {
            initial_generation.record_stale(terrain_generation_time, lake_generation_time);
            continue;
        }
        let mesh = generated.mesh?;
        let lake_mesh = generated.lake_mesh.transpose()?;
        match spec {
            TerrainMeshSpec::Near(spec) => {
                requested.remove(&spec.chunk);
                let tree_mesh = tree_mesh_for_chunk(device, renderer, terrain, spec.chunk)?;
                chunks.insert(
                    spec.chunk,
                    ResidentTerrainChunk {
                        spec,
                        mesh: renderer.upload_mesh(device, &mesh)?,
                        lake_mesh: lake_mesh
                            .as_ref()
                            .filter(|lake_mesh| !lake_mesh.indices.is_empty())
                            .map(|lake_mesh| renderer.upload_mesh(device, lake_mesh))
                            .transpose()?,
                        tree_mesh,
                    },
                );
            }
            TerrainMeshSpec::Far(spec) => {
                requested_far.remove(&spec.tile);
                far_tiles.insert(
                    spec.tile,
                    ResidentFarTerrainTile {
                        spec,
                        mesh: renderer.upload_mesh(device, &mesh)?,
                        lake_mesh: lake_mesh
                            .as_ref()
                            .filter(|lake_mesh| !lake_mesh.indices.is_empty())
                            .map(|lake_mesh| renderer.upload_mesh(device, lake_mesh))
                            .transpose()?,
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
        chunks,
        far_tiles,
        requested,
        requested_far,
        jobs,
    )
}

fn tree_mesh_for_chunk(
    device: &wgpu::Device,
    renderer: &TerrainRenderer,
    terrain: &GeneratedWorldTerrain,
    chunk: ChunkIndex,
) -> Result<Option<TerrainMesh>, Box<dyn Error>> {
    let origin = chunk.sample_origin();
    let edge = ChunkIndex::edge_meters();
    let bounds = TreeBounds::new(origin.x, origin.z, origin.x + edge, origin.z + edge)
        .ok_or_else(|| std::io::Error::other("tree chunk bounds are invalid"))?;
    let mut trees = ProceduralTrees::new(terrain.world())
        .trees_in(bounds)
        .ok_or_else(|| std::io::Error::other("tree generation is unavailable"))?;
    trees.retain(|tree| {
        terrain.lake_surface_at(tree.x, tree.z).is_none()
            && !terrain
                .river_influence_at(tree.x, tree.z)
                .is_some_and(|river| river.blend > 0.24)
    });
    if trees.is_empty() {
        return Ok(None);
    }
    let mesh = renderer.upload_trees(device, &trees, |x, z| terrain.surface_height(x, z))?;
    Ok(Some(mesh))
}

#[allow(clippy::too_many_arguments)]
fn schedule_terrain(
    chunk_streamer: ChunkStreamer,
    far_streamer: FarTerrainStreamer,
    player_position: WorldPosition,
    chunks: &mut BTreeMap<ChunkIndex, ResidentTerrainChunk>,
    far_tiles: &mut BTreeMap<FarTileIndex, ResidentFarTerrainTile>,
    requested: &mut BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far: &mut BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    jobs: &mut TerrainMeshQueue<GeneratedWorldTerrain>,
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

    let mut lod_counts = [0_usize; 3];
    for spec in &chunk_plan.load {
        let lod_index = usize::from(spec.lod.get() - ChunkIndex::NEAR_LOD.get());
        lod_counts[lod_index] += 1;
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

    if chunk_plan.load.is_empty()
        && chunk_plan.unload.is_empty()
        && far_plan.load.is_empty()
        && far_plan.unload.is_empty()
    {
        return Ok(());
    }
    eprintln!(
        "streaming center ({}, {}): queued {} far tiles and {} chunks [LOD2 {}, LOD3 {}, LOD4 {}], unloaded {} far / {} near, resident {} far / {} near",
        chunk_plan.center.x,
        chunk_plan.center.z,
        far_plan.load.len(),
        chunk_plan.load.len(),
        lod_counts[0],
        lod_counts[1],
        lod_counts[2],
        far_plan.unload.len(),
        chunk_plan.unload.len(),
        far_tiles.len(),
        chunks.len()
    );
    Ok(())
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
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        );
        assert!(
            max.into_iter()
                .zip([160.0, 160.0])
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        );
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
