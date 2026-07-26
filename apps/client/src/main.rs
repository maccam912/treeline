use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use treeline_coordinates::{WorldIdentity, WorldPosition};
use treeline_renderer::{TerrainMesh, TerrainRenderer};
use treeline_terrain::WildernessTerrain;
use treeline_voxel::ChunkIndex;
use treeline_world::{
    ChunkMeshSpec, ChunkStreamer, ChunkStreamingConfig, FarTerrainMeshSpec, FarTerrainStreamer,
    FarTerrainStreamingConfig, FarTileIndex, GenerationPriority, NearTerrainCutout,
    TerrainMeshQueue, TerrainMeshSpec,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, 2, 0);
const EYE_HEIGHT: f32 = 1.72;
const WALK_SPEED: f32 = 8.0;
const SPRINT_SPEED: f32 = 16.0;

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = TreelineApp::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[derive(Default)]
struct TreelineApp {
    game: Option<Game>,
}

impl ApplicationHandler for TreelineApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.game.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title("Treeline — Infinite Landscape")
            .with_inner_size(LogicalSize::new(1280, 720));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create the Treeline window: {error}");
                event_loop.exit();
                return;
            }
        };

        match pollster::block_on(Game::new(window)) {
            Ok(game) => self.game = Some(game),
            Err(error) => {
                eprintln!("failed to start Treeline: {error}");
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
        if game.cursor_captured {
            if let DeviceEvent::MouseMotion { delta } = event {
                game.camera.look(delta.0, delta.1);
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
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
    terrain_jobs: TerrainMeshQueue<WildernessTerrain>,
    chunk_streamer: ChunkStreamer,
    far_terrain_streamer: FarTerrainStreamer,
    terrain: WildernessTerrain,
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

        let terrain = WildernessTerrain::new(WORLD);
        let renderer = TerrainRenderer::new(
            &device,
            surface_config.format,
            surface_config.width,
            surface_config.height,
        );
        let chunk_streamer = ChunkStreamer::new(ChunkStreamingConfig::default());
        let far_terrain_streamer = FarTerrainStreamer::new(FarTerrainStreamingConfig::default());
        let mut terrain_chunks = BTreeMap::new();
        let mut far_terrain_tiles = BTreeMap::new();
        let mut requested_chunks = BTreeMap::new();
        let mut requested_far_tiles = BTreeMap::new();
        let mut terrain_jobs = TerrainMeshQueue::new(terrain);

        let start_x = 0.0;
        let start_z = 70.0;
        let start_y = surface_height(terrain, start_x, start_z) + EYE_HEIGHT;
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

        let mut game = Self {
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
        game.set_cursor_captured(true);
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

    fn update(&mut self) {
        let now = Instant::now();
        let delta_seconds = (now - self.previous_frame).as_secs_f32().min(0.1);
        self.previous_frame = now;
        self.camera.walk(&self.input, self.terrain, delta_seconds);
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
                .chain(self.terrain_chunks.values().map(|resident| &resident.mesh)),
        );
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

struct ResidentTerrainChunk {
    spec: ChunkMeshSpec,
    mesh: TerrainMesh,
}

struct ResidentFarTerrainTile {
    spec: FarTerrainMeshSpec,
    mesh: TerrainMesh,
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
            pitch: -0.08,
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

    fn walk(&mut self, input: &InputState, terrain: WildernessTerrain, delta_seconds: f32) {
        let forward = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let right = forward.cross(Vec3::Y);
        let movement = (forward * input.forward_axis()) + (right * input.right_axis());
        if movement.length_squared() > 0.0 {
            let speed = if input.sprint() {
                SPRINT_SPEED
            } else {
                WALK_SPEED
            };
            self.position += movement.normalize() * speed * delta_seconds;
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
        f32::from(u8::from(
            self.is_down(KeyCode::KeyW) || self.is_down(KeyCode::ArrowUp),
        )) - f32::from(u8::from(
            self.is_down(KeyCode::KeyS) || self.is_down(KeyCode::ArrowDown),
        ))
    }

    fn right_axis(&self) -> f32 {
        f32::from(u8::from(
            self.is_down(KeyCode::KeyD) || self.is_down(KeyCode::ArrowRight),
        )) - f32::from(u8::from(
            self.is_down(KeyCode::KeyA) || self.is_down(KeyCode::ArrowLeft),
        ))
    }

    fn sprint(&self) -> bool {
        self.is_down(KeyCode::ShiftLeft) || self.is_down(KeyCode::ShiftRight)
    }

    fn is_down(&self, code: KeyCode) -> bool {
        self.pressed.contains(&code)
    }

    fn clear(&mut self) {
        self.pressed.clear();
    }
}

fn surface_height(terrain: WildernessTerrain, x: f32, z: f32) -> f32 {
    let height = terrain
        .height_at(f64::from(x), f64::from(z))
        .expect("finite player positions must have terrain");
    f64_as_f32(height)
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
    jobs: &mut TerrainMeshQueue<WildernessTerrain>,
) -> Result<(), Box<dyn Error>> {
    while let Some(generated) = jobs.try_next() {
        match generated.spec {
            TerrainMeshSpec::Near(spec) => {
                if requested.get(&spec.chunk) != Some(&spec) {
                    continue;
                }
                requested.remove(&spec.chunk);
                chunks.insert(
                    spec.chunk,
                    ResidentTerrainChunk {
                        spec,
                        mesh: renderer.upload_mesh(device, &generated.mesh?)?,
                    },
                );
            }
            TerrainMeshSpec::Far(spec) => {
                if requested_far.get(&spec.tile) != Some(&spec) {
                    continue;
                }
                requested_far.remove(&spec.tile);
                far_tiles.insert(
                    spec.tile,
                    ResidentFarTerrainTile {
                        spec,
                        mesh: renderer.upload_mesh(device, &generated.mesh?)?,
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
    jobs: &mut TerrainMeshQueue<WildernessTerrain>,
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

    let near_cutout = if chunk_plan.load.is_empty() && requested.is_empty() {
        NearTerrainCutout::around(chunk_plan.center, chunk_streamer.config().load_radius())
    } else {
        None
    };
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
    for spec in &chunk_plan.load {
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
