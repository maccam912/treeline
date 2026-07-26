use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

use glam::{Mat4, Vec2, Vec3};
use treeline_coordinates::{WorldIdentity, WorldPosition};
use treeline_renderer::{TerrainMesh, TerrainRenderer};
use treeline_terrain::SurfaceField;
use treeline_voxel::ChunkIndex;
use treeline_world::{
    ChunkMeshSpec, ChunkStreamer, ChunkStreamingConfig, FarTerrainMeshSpec, FarTerrainStreamer,
    FarTerrainStreamingConfig, FarTileIndex, GeneratedWorldTerrain, GenerationPriority,
    NearTerrainCutout, TerrainMeshQueue, TerrainMeshSpec,
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

const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, 5, 0);
const EYE_HEIGHT: f32 = 1.72;
const WALK_SPEED: f32 = 8.0;
const SPRINT_SPEED: f32 = 16.0;

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
            .with_title("Treeline — Infinite Landscape")
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
}

impl Game {
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
        let mut terrain_jobs = TerrainMeshQueue::new(terrain.clone());

        let start_x = 16.0;
        let start_z = 80.0;
        let start_y = surface_height(&terrain, start_x, start_z) + EYE_HEIGHT;
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
            self.chunk_streamer,
            self.far_terrain_streamer,
            self.camera.world_position(),
            &mut self.terrain_chunks,
            &mut self.far_terrain_tiles,
            &mut self.requested_chunks,
            &mut self.requested_far_tiles,
            &mut self.terrain_jobs,
            &self.terrain,
        ) {
            eprintln!("terrain chunk streaming failed: {error}");
        }
        self.renderer.update_camera(
            &self.queue,
            self.camera
                .view_projection(self.surface_config.width, self.surface_config.height),
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
                label: Some("Treeline frame encoder"),
            });
        self.renderer.render(
            &mut encoder,
            &view,
            self.far_terrain_tiles
                .values()
                .map(|resident| &resident.mesh)
                .chain(self.terrain_chunks.values().map(|resident| &resident.mesh))
                .chain(
                    self.far_terrain_tiles
                        .values()
                        .filter_map(|resident| resident.lake_mesh.as_ref()),
                )
                .chain(
                    self.terrain_chunks
                        .values()
                        .filter_map(|resident| resident.lake_mesh.as_ref()),
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
}

struct ResidentFarTerrainTile {
    spec: FarTerrainMeshSpec,
    mesh: TerrainMesh,
    lake_mesh: Option<TerrainMesh>,
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
        let pitch_cosine = self.pitch.cos();
        Vec3::new(
            self.yaw.cos() * pitch_cosine,
            self.pitch.sin(),
            self.yaw.sin() * pitch_cosine,
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
        let forward = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
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

#[allow(clippy::too_many_arguments)]
fn update_terrain(
    device: &wgpu::Device,
    renderer: &TerrainRenderer,
    chunk_streamer: ChunkStreamer,
    far_streamer: FarTerrainStreamer,
    player_position: WorldPosition,
    chunks: &mut BTreeMap<ChunkIndex, ResidentTerrainChunk>,
    far_tiles: &mut BTreeMap<FarTileIndex, ResidentFarTerrainTile>,
    requested: &mut BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far: &mut BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    jobs: &mut TerrainMeshQueue<GeneratedWorldTerrain>,
    terrain: &GeneratedWorldTerrain,
) -> Result<(), Box<dyn Error>> {
    while let Some(generated) = jobs.try_next() {
        match generated.spec {
            TerrainMeshSpec::Near(spec) => {
                if requested.get(&spec.chunk) != Some(&spec) {
                    continue;
                }
                requested.remove(&spec.chunk);
                let lake_mesh = terrain.lake_surface_mesh(TerrainMeshSpec::Near(spec))?;
                chunks.insert(
                    spec.chunk,
                    ResidentTerrainChunk {
                        spec,
                        mesh: renderer.upload_mesh(device, &generated.mesh?)?,
                        lake_mesh: (!lake_mesh.indices.is_empty())
                            .then(|| renderer.upload_mesh(device, &lake_mesh))
                            .transpose()?,
                    },
                );
            }
            TerrainMeshSpec::Far(spec) => {
                if requested_far.get(&spec.tile) != Some(&spec) {
                    continue;
                }
                requested_far.remove(&spec.tile);
                let lake_mesh = terrain.lake_surface_mesh(TerrainMeshSpec::Far(spec))?;
                far_tiles.insert(
                    spec.tile,
                    ResidentFarTerrainTile {
                        spec,
                        mesh: renderer.upload_mesh(device, &generated.mesh?)?,
                        lake_mesh: (!lake_mesh.indices.is_empty())
                            .then(|| renderer.upload_mesh(device, &lake_mesh))
                            .transpose()?,
                    },
                );
            }
        }
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
        requested.remove(chunk);
    }

    let mut lod_counts = [0_usize; 3];
    for spec in &chunk_plan.load {
        let lod_index = usize::from(spec.lod.get() - ChunkIndex::NEAR_LOD.get());
        lod_counts[lod_index] += 1;
        requested.insert(spec.chunk, *spec);
    }

    let near_cutout =
        NearTerrainCutout::around(chunk_plan.center, chunk_streamer.config().load_radius());
    let mut tracked_far = far_tiles
        .iter()
        .map(|(&tile, resident)| (tile, resident.spec))
        .collect::<BTreeMap<_, _>>();
    tracked_far.extend(requested_far.iter().map(|(&tile, &spec)| (tile, spec)));
    let far_plan = far_streamer
        .plan(player_position, &tracked_far, near_cutout)
        .ok_or_else(|| std::io::Error::other("player position is outside far tile index range"))?;
    for tile in &far_plan.unload {
        far_tiles.remove(tile);
        requested_far.remove(tile);
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
        requested_far.insert(spec.tile, *spec);
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
