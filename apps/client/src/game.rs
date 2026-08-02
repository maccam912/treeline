//! The game: what exists while playing, and what one frame does.

use std::error::Error;
use std::sync::Arc;

use glam::{DVec3, Vec2};
use treeline_renderer::{LightingSettings, TerrainRenderer, TimeOfDay};
use treeline_terrain::{SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z};
use treeline_world::{DEFAULT_WORLD_IDENTITY, WorldTerrain};
use web_time::Instant;
use winit::event::{ElementState, MouseButton, Touch, TouchPhase, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
#[cfg(not(target_arch = "wasm32"))]
use winit::window::CursorGrabMode;
use winit::window::Window;

use crate::camera::{Camera, EYE_HEIGHT, surface_height};
use crate::gpu::Gpu;
use crate::input::InputState;
use crate::progress::LoadProgress;
use crate::streaming::{PlayerMotion, ResidentTerrain, Streamers, Uploader};
use crate::trees::{ResidentTrees, TreeTileIndex};
use crate::{
    TerrainMeshQueue, WINDOW_TITLE, atmosphere, random, start_terrain_queue, streaming, warp,
};

/// Facing roughly north-west at spawn, level with the horizon.
const SPAWN_YAW: f64 = -1.924_842_228_418_599_5;
const SPAWN_PITCH: f64 = -0.08;

/// A warp the player asked for, handled at the start of the next frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum WarpRequest {
    #[default]
    None,
    Random,
    LakeShore,
}

pub struct Game {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: TerrainRenderer,
    terrain: WorldTerrain,
    streamers: Streamers,
    resident: ResidentTerrain,
    trees: ResidentTrees,
    jobs: TerrainMeshQueue,
    camera: Camera,
    input: InputState,
    cursor_captured: bool,
    previous_frame: Instant,
    progress: LoadProgress,
    warp_request: WarpRequest,
    time_of_day: TimeOfDay,
    #[cfg(target_arch = "wasm32")]
    browser: crate::browser::BrowserActions,
}

impl Game {
    /// Brings up the GPU, the world, and the terrain around the spawn point.
    ///
    /// # Errors
    ///
    /// Returns an error when the GPU cannot be initialized or the spawn point
    /// cannot be streamed.
    pub async fn new(window: Arc<Window>) -> Result<Self, Box<dyn Error>> {
        let started = Instant::now();
        window.set_title("Treeline — Preparing spawn…");

        let gpu = Gpu::new(window.clone()).await?;
        let terrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);
        let renderer = TerrainRenderer::new(
            &gpu.device,
            &gpu.queue,
            gpu.surface_config.format,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );
        if let Some(settings) = atmosphere::settings_for(terrain.climate()) {
            renderer.update_atmosphere(&gpu.queue, settings);
        }

        let spawn_y = surface_height(&terrain, SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z) + EYE_HEIGHT;
        let camera = Camera::new(
            DVec3::new(SURVEYED_SPAWN_X, spawn_y, SURVEYED_SPAWN_Z),
            SPAWN_YAW,
            SPAWN_PITCH,
        );
        let streamers = Streamers::default();
        let mut resident = ResidentTerrain::default();
        let mut jobs = start_terrain_queue(terrain)?;
        streaming::schedule(
            streamers,
            PlayerMotion::arrived(camera.world_position()),
            &mut resident,
            &mut jobs,
        )?;
        let progress = start_progress(
            &window, &renderer, &gpu, streamers, &resident, camera, started,
        )?;

        Ok(Self {
            window,
            gpu,
            renderer,
            terrain,
            streamers,
            resident,
            trees: ResidentTrees::default(),
            jobs,
            camera,
            input: InputState::default(),
            cursor_captured: false,
            previous_frame: Instant::now(),
            progress,
            warp_request: WarpRequest::None,
            time_of_day: TimeOfDay::default(),
            #[cfg(target_arch = "wasm32")]
            browser: crate::browser::BrowserActions::new()?,
        })
    }

    pub fn window_id(&self) -> winit::window::WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn handle_mouse_motion(&mut self, delta: (f64, f64)) {
        if self.cursor_captured {
            self.camera.look(delta.0, delta.1);
        }
    }

    /// Routes one window event, returning whether the window should close.
    pub fn handle_window_event(&mut self, event: WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => return true,
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::RedrawRequested => self.frame(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    let pressed = event.state == ElementState::Pressed;
                    if pressed && !event.repeat {
                        self.handle_action_key(code);
                    }
                    self.input.set_key(code, pressed);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.set_cursor_captured(true),
            WindowEvent::Touch(touch) => self.handle_touch(touch),
            WindowEvent::Focused(false) => {
                self.set_cursor_captured(false);
                self.input.clear();
            }
            _ => {}
        }
        false
    }

    fn handle_action_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Escape => self.set_cursor_captured(false),
            KeyCode::KeyR => self.warp_request = WarpRequest::Random,
            KeyCode::KeyB => self.warp_request = WarpRequest::LakeShore,
            KeyCode::KeyF => self.toggle_aerial_mode(),
            KeyCode::KeyT => {
                self.time_of_day = self.time_of_day.next();
                eprintln!("daylight: {}", self.time_of_day.label());
            }
            _ => {}
        }
    }

    fn handle_touch(&mut self, touch: Touch) {
        let size = self.window.inner_size();
        let position = Vec2::new(f64_as_f32(touch.location.x), f64_as_f32(touch.location.y));
        match touch.phase {
            TouchPhase::Started => self.input.begin_touch(
                touch.id,
                position,
                u32_as_f32(size.width),
                f64_as_f32(64.0 * self.window.scale_factor()),
            ),
            TouchPhase::Moved => self.input.move_touch(touch.id, position),
            TouchPhase::Ended | TouchPhase::Cancelled => self.input.end_touch(touch.id),
        }
    }

    fn toggle_aerial_mode(&mut self) {
        let enabled = self.camera.toggle_aerial_mode(&self.terrain);
        #[cfg(target_arch = "wasm32")]
        crate::browser::BrowserActions::set_aerial_mode_enabled(enabled);
        eprintln!(
            "aerial mode {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Turns page-button presses into the same requests the keys produce.
    #[cfg(target_arch = "wasm32")]
    fn apply_browser_buttons(&mut self) {
        if self.browser.take_random_warp() {
            self.warp_request = WarpRequest::Random;
        }
        if self.browser.take_water_warp() {
            self.warp_request = WarpRequest::LakeShore;
        }
        if self.browser.take_aerial_toggle() {
            self.toggle_aerial_mode();
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.gpu.surface_config.width = width;
        self.gpu.surface_config.height = height;
        self.reconfigure_surface();
        self.renderer.resize(&self.gpu.device, width, height);
    }

    fn reconfigure_surface(&self) {
        self.gpu
            .surface
            .configure(&self.gpu.device, &self.gpu.surface_config);
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

    /// Advances and draws one frame.
    fn frame(&mut self) {
        self.update();
        match self.render() {
            Ok(()) | Err(wgpu::SurfaceError::Timeout) => {}
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.reconfigure_surface();
            }
            Err(error) => eprintln!("frame render failed: {error}"),
        }
    }

    fn update(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.apply_browser_buttons();

        if let Err(error) = self.apply_warp_request() {
            eprintln!("warp failed: {error}");
        }

        let now = Instant::now();
        // Clamp the step so a long stall — a resize, a tab regaining focus —
        // cannot teleport the player.
        let delta_seconds = (now - self.previous_frame).as_secs_f64().min(0.1);
        self.previous_frame = now;

        self.camera
            .look_with_stick(self.input.look_axis(), delta_seconds);
        let travel_direction = self.camera.travel_direction(&self.input);
        self.camera.walk(&self.input, &self.terrain, delta_seconds);
        self.renderer
            .advance_water_time(&self.gpu.queue, delta_seconds);

        if let Err(error) = streaming::update(
            Uploader {
                device: &self.gpu.device,
                renderer: &self.renderer,
                terrain: &self.terrain,
            },
            self.streamers,
            PlayerMotion {
                position: self.camera.world_position(),
                travel_direction,
            },
            &mut self.resident,
            &mut self.jobs,
            &mut self.progress,
        ) {
            eprintln!("terrain streaming failed: {error}");
        }
        if let Err(error) = self.trees.update(
            &self.gpu.device,
            &self.renderer,
            &self.terrain,
            self.streamers.near.config(),
            self.camera.world_position(),
        ) {
            eprintln!("tree streaming failed: {error}");
        }

        self.progress.publish(&self.window);
        self.update_render_camera();
        if let Ok((min, max)) =
            streaming::far_cutout_bounds(self.streamers, self.camera.world_position())
        {
            self.renderer.update_far_cutout(&self.gpu.queue, min, max);
        }
    }

    fn update_render_camera(&self) {
        self.renderer.update_camera(
            &self.gpu.queue,
            self.camera.view_projection(
                self.gpu.surface_config.width,
                self.gpu.surface_config.height,
            ),
            self.camera.position.to_array(),
            self.camera.direction().as_vec3().to_array(),
            LightingSettings::for_time_of_day(self.time_of_day),
        );
    }

    /// Moves the player, then rebuilds the world around them.
    fn apply_warp_request(&mut self) -> Result<(), Box<dyn Error>> {
        let site = match std::mem::take(&mut self.warp_request) {
            WarpRequest::None => return Ok(()),
            WarpRequest::Random => warp::random_site(&self.terrain, random::unit_interval)
                .ok_or_else(|| std::io::Error::other("could not find dry ground to warp to"))?,
            WarpRequest::LakeShore => warp::lake_shore_site(&self.terrain, random::unit_interval())
                .ok_or_else(|| std::io::Error::other("could not find a reachable lake shore"))?,
        };

        let started = Instant::now();
        let [x, z] = site.destination;
        self.resident.clear(&mut self.jobs);
        self.trees.clear();
        self.camera.position = DVec3::new(x, self.camera.height_over(&self.terrain, x, z), z);
        if let Some(target) = site.face {
            self.camera.face(target);
        }
        self.input.clear();
        self.previous_frame = Instant::now();

        streaming::schedule(
            self.streamers,
            PlayerMotion::arrived(self.camera.world_position()),
            &mut self.resident,
            &mut self.jobs,
        )?;
        self.progress = start_progress(
            &self.window,
            &self.renderer,
            &self.gpu,
            self.streamers,
            &self.resident,
            self.camera,
            started,
        )?;
        eprintln!("warped to ({x:.0}, {z:.0})");
        Ok(())
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let frame = self.gpu.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Treeline frame encoder"),
            });

        let far_meshes = self
            .resident
            .far_tiles
            .values()
            .map(|tile| &tile.mesh)
            .chain(
                self.resident
                    .far_tiles
                    .values()
                    .filter_map(|tile| tile.lake_mesh.as_ref()),
            );
        let near_meshes = self
            .resident
            .chunks
            .values()
            .map(|chunk| &chunk.mesh)
            .chain(
                self.resident
                    .chunks
                    .values()
                    .filter_map(|chunk| chunk.lake_mesh.as_ref()),
            )
            .chain(self.trees.meshes());

        // Only nearby geometry casts shadows: distant trees would fill the
        // cascades with detail no shadow is visible at.
        let shadow_meshes = self
            .resident
            .far_tiles
            .values()
            .map(|tile| &tile.mesh)
            .chain(self.resident.chunks.values().map(|chunk| &chunk.mesh))
            .chain(
                TreeTileIndex::containing(self.camera.world_position())
                    .into_iter()
                    .flat_map(|center| self.trees.shadow_meshes(center)),
            )
            .collect::<Vec<_>>();

        self.renderer
            .render(&mut encoder, &view, far_meshes, near_meshes, &shadow_meshes);
        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

/// Starts a progress report and points far terrain at the new near-terrain hole.
fn start_progress(
    window: &Window,
    renderer: &TerrainRenderer,
    gpu: &Gpu,
    streamers: Streamers,
    resident: &ResidentTerrain,
    camera: Camera,
    started: Instant,
) -> Result<LoadProgress, Box<dyn Error>> {
    let (min, max) = streaming::far_cutout_bounds(streamers, camera.world_position())?;
    renderer.update_far_cutout(&gpu.queue, min, max);

    let (chunks, far_tiles) = resident.outstanding();
    let progress = LoadProgress::new(
        started,
        chunks,
        far_tiles,
        streamers.far,
        camera.world_position(),
    )
    .ok_or_else(|| std::io::Error::other("player position is outside far tile range"))?;
    window.set_title(WINDOW_TITLE);
    Ok(progress)
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[allow(clippy::cast_precision_loss)]
fn u32_as_f32(value: u32) -> f32 {
    value as f32
}
