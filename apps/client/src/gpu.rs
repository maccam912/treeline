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

/// Picks the backend set to bring the instance up on.
///
/// Browsers need the choice made before the canvas is touched. A canvas keeps
/// the first drawing context it is given, so an instance that claims it for
/// WebGPU leaves no way back to WebGL2 — and phones routinely expose
/// `navigator.gpu` while refusing to hand out an adapter. Probing WebGPU first,
/// away from the canvas, keeps those devices on the WebGL2 path.
async fn usable_backends() -> wgpu::Backends {
    #[cfg(not(target_arch = "wasm32"))]
    {
        wgpu::Backends::all()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let probe = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..wgpu::InstanceDescriptor::default()
        });
        let webgpu_works = probe
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .is_some();
        if webgpu_works {
            wgpu::Backends::BROWSER_WEBGPU
        } else {
            wgpu::Backends::GL
        }
    }
}

impl Gpu {
    /// Opens a device and configures the window's surface.
    ///
    /// # Errors
    ///
    /// Returns an error when no compatible adapter exists or the device cannot
    /// be created.
    pub async fn new(window: Arc<Window>) -> Result<Self, Box<dyn Error>> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: usable_backends().await,
            ..wgpu::InstanceDescriptor::default()
        });
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .ok_or_else(|| std::io::Error::other("no compatible graphics adapter found"))?;
        // Asking for the desktop defaults fails outright on WebGL2 and on
        // phone-class adapters, which report far smaller ceilings. The renderer
        // uses no storage buffers or compute, so whatever the adapter offers is
        // enough.
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Treeline device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: adapter.limits(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await?;

        let size = window.inner_size();
        let capabilities = surface.get_capabilities(&adapter);
        // Shadows need depth textures sampled through a comparison sampler.
        // WebGL2 has both, so this asks the adapter rather than the backend.
        let shadows_enabled = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPARISON_SAMPLERS);
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
