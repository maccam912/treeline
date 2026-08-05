//! Bringing up the graphics device and swapchain.

use std::error::Error;
use std::sync::Arc;

use winit::window::Window;

/// Everything the game needs from the graphics API after startup.
#[derive(Debug)]
pub struct Gpu {
    /// Held only to keep the surface's parent alive.
    pub _instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface_config: wgpu::SurfaceConfiguration,
    /// Whether the adapter can render the shadow maps.
    pub shadows_enabled: bool,
}

impl Gpu {
    /// Opens a device and configures the window's surface.
    ///
    /// # Errors
    ///
    /// Returns an error when no compatible adapter exists or the device cannot
    /// be created.
    pub async fn new(window: Arc<Window>) -> Result<Self, Box<dyn Error>> {
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
        // WebGL2 cannot represent the renderer's shadow depth textures, so the
        // browser build turns shadows off and lights everything with the sun.
        let shadows_enabled = adapter.get_info().backend != wgpu::Backend::Gl;
        // An sRGB surface lets the hardware do the final color conversion, so
        // shading stays linear all the way through.
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

        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            surface_config,
            shadows_enabled,
        })
    }
}
