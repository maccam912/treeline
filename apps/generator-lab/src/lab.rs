//! The lab window: a top-down map of one layer, and what is under the cursor.

use std::error::Error;
use std::sync::Arc;

use treeline_climate::Season;
use treeline_renderer::{LightingSettings, TerrainMesh, TerrainRenderer};
use treeline_terrain::{SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z, SURVEYED_TILE_EDGE_METERS};
use treeline_world::{DEFAULT_WORLD_IDENTITY, WorldTerrain};
use winit::dpi::PhysicalPosition;
use winit::event::WindowEvent;
use winit::keyboard::KeyCode;
use winit::window::Window;

use crate::inspect::{self, Inspection};
use crate::map::{self, MapView};
use crate::ui;
use crate::view::ViewMode;

/// Zoom limits: the whole tile, down to individual canopy cells.
const MIN_SPAN_METERS: f64 = 60.0;
const MAX_SPAN_METERS: f64 = SURVEYED_TILE_EDGE_METERS * 1.2;
/// How far one zoom step moves, and how far one pan step moves.
const ZOOM_STEP: f64 = 1.4;
const PAN_STEP_FRACTION: f64 = 0.15;

/// Height the map camera sits at above the map plane.
const CAMERA_HEIGHT_METERS: f64 = 1_000.0;

pub struct Lab {
    window: Arc<Window>,
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    renderer: TerrainRenderer,
    egui: ui::Egui,
    terrain: WorldTerrain,
    view: MapView,
    mesh: TerrainMesh,
    cursor: PhysicalPosition<f64>,
    inspection: Option<Inspection>,
}

impl Lab {
    /// Opens the lab centered on the player's spawn.
    ///
    /// # Errors
    ///
    /// Returns an error when the GPU cannot be initialized or the first map
    /// cannot be uploaded.
    pub async fn new(window: Arc<Window>) -> Result<Self, Box<dyn Error>> {
        let (instance, surface, device, queue, surface_config) = init_gpu(&window).await?;
        let renderer = TerrainRenderer::new(
            &device,
            &queue,
            surface_config.format,
            surface_config.width,
            surface_config.height,
        );
        let egui = ui::Egui::new(&window, &device, surface_config.format);
        let terrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);
        let view = MapView {
            center: [SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z],
            span_meters: 2_000.0,
            mode: ViewMode::default(),
            season: Season::Winter,
        };
        let mesh = renderer.upload_mesh(
            &device,
            &map::build(terrain, view, surface_config.width, surface_config.height),
        )?;
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
            egui,
            terrain,
            view,
            mesh,
            cursor,
            inspection: None,
        };
        lab.update_camera();
        lab.window.request_redraw();
        Ok(lab)
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    /// Offers an event to the UI first, reporting whether it was consumed.
    pub fn handle_ui_event(&mut self, event: &WindowEvent) -> bool {
        let (consumed, repaint) = self.egui.on_window_event(&self.window, event);
        if repaint {
            self.window.request_redraw();
        }
        consumed
    }

    pub fn set_cursor(&mut self, position: PhysicalPosition<f64>) {
        self.cursor = position;
    }

    /// # Errors
    ///
    /// Returns an error when the resized map cannot be uploaded.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), Box<dyn Error>> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.reconfigure_surface();
        self.renderer.resize(&self.device, width, height);
        self.rebuild()
    }

    pub fn reconfigure_surface(&self) {
        self.surface.configure(&self.device, &self.surface_config);
    }

    /// Applies a keyboard command, rebuilding the map when the view changes.
    ///
    /// # Errors
    ///
    /// Returns an error when the rebuilt map cannot be uploaded.
    pub fn handle_key(&mut self, code: KeyCode) -> Result<(), Box<dyn Error>> {
        let pan = self.view.span_meters * PAN_STEP_FRACTION;
        let changed = match code {
            KeyCode::Digit1 => self.set_mode(ViewMode::Elevation),
            KeyCode::Digit2 => self.set_mode(ViewMode::Imagery),
            KeyCode::Digit3 => self.set_mode(ViewMode::Water),
            KeyCode::Digit4 => self.set_mode(ViewMode::CanopyCover),
            KeyCode::Digit5 => self.set_mode(ViewMode::CanopyHeight),
            KeyCode::Digit6 => self.set_mode(ViewMode::Forest),
            KeyCode::Digit7 => self.set_mode(ViewMode::Snow),
            KeyCode::KeyW | KeyCode::ArrowUp => self.pan([0.0, -pan]),
            KeyCode::KeyS | KeyCode::ArrowDown => self.pan([0.0, pan]),
            KeyCode::KeyA | KeyCode::ArrowLeft => self.pan([-pan, 0.0]),
            KeyCode::KeyD | KeyCode::ArrowRight => self.pan([pan, 0.0]),
            KeyCode::Equal | KeyCode::NumpadAdd => self.zoom(1.0 / ZOOM_STEP),
            KeyCode::Minus | KeyCode::NumpadSubtract => self.zoom(ZOOM_STEP),
            KeyCode::KeyC => {
                self.view.season = self.view.season.next();
                true
            }
            _ => false,
        };
        if changed { self.rebuild() } else { Ok(()) }
    }

    /// # Errors
    ///
    /// Returns an error when the rebuilt map cannot be uploaded.
    pub fn zoom_by(&mut self, factor: f64) -> Result<(), Box<dyn Error>> {
        if self.zoom(factor) {
            return self.rebuild();
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error when the rebuilt map cannot be uploaded.
    pub fn recenter_on_cursor(&mut self) -> Result<(), Box<dyn Error>> {
        self.view.center = self.cursor_world_position();
        self.rebuild()
    }

    /// Samples every layer under the cursor.
    pub fn inspect_cursor(&mut self) {
        let [x, z] = self.cursor_world_position();
        self.inspection = Some(inspect::at(self.terrain, x, z, self.view.season));
        self.window.request_redraw();
    }

    fn set_mode(&mut self, mode: ViewMode) -> bool {
        let changed = self.view.mode != mode;
        self.view.mode = mode;
        changed
    }

    fn pan(&mut self, offset: [f64; 2]) -> bool {
        self.view.center[0] += offset[0];
        self.view.center[1] += offset[1];
        true
    }

    fn zoom(&mut self, factor: f64) -> bool {
        let span = (self.view.span_meters * factor).clamp(MIN_SPAN_METERS, MAX_SPAN_METERS);
        let changed = span.to_bits() != self.view.span_meters.to_bits();
        self.view.span_meters = span;
        changed
    }

    fn cursor_world_position(&self) -> [f64; 2] {
        self.view.world_position_at(
            [self.cursor.x, self.cursor.y],
            self.surface_config.width,
            self.surface_config.height,
        )
    }

    fn rebuild(&mut self) -> Result<(), Box<dyn Error>> {
        self.mesh = self.renderer.upload_mesh(
            &self.device,
            &map::build(
                self.terrain,
                self.view,
                self.surface_config.width,
                self.surface_config.height,
            ),
        )?;
        self.update_camera();
        self.window.request_redraw();
        Ok(())
    }

    /// Points the camera straight down at the map's center.
    fn update_camera(&self) {
        let center = [
            self.view.center[0],
            CAMERA_HEIGHT_METERS,
            self.view.center[1],
        ];
        self.renderer.update_camera(
            &self.queue,
            ui::top_down_view_projection(
                self.view.span_meters,
                self.surface_config.width,
                self.surface_config.height,
            ),
            center,
            [0.0, -1.0, 0.0],
            LightingSettings::default(),
        );
    }

    /// Draws the map and the surrounding UI.
    ///
    /// # Errors
    ///
    /// Returns the surface error when the swapchain image cannot be acquired.
    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Generator Lab encoder"),
            });

        self.renderer
            .render(&mut encoder, &view, [], [&self.mesh], &[]);
        self.egui.render(
            &self.window,
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            &self.surface_config,
            self.view,
            self.inspection.as_ref(),
        );

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

type InitializedGpu = (
    wgpu::Instance,
    wgpu::Surface<'static>,
    wgpu::Device,
    wgpu::Queue,
    wgpu::SurfaceConfiguration,
);

async fn init_gpu(window: &Arc<Window>) -> Result<InitializedGpu, Box<dyn Error>> {
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
    let format = capabilities
        .formats
        .iter()
        .copied()
        .find(wgpu::TextureFormat::is_srgb)
        .unwrap_or(capabilities.formats[0]);
    let surface_config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: capabilities.alpha_modes[0],
        view_formats: vec![],
    };
    surface.configure(&device, &surface_config);
    Ok((instance, surface, device, queue, surface_config))
}
