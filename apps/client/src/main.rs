//! The Treeline client.
//!
//! The client streams the surveyed world around the player and draws it. It
//! owns no generation: [`treeline_world`] answers questions about the terrain,
//! and [`treeline_renderer`] draws whatever meshes come back.
//!
//! One structure recurs throughout: nothing waits. Terrain is requested, and
//! frames keep running until it arrives. That is why residency, progress
//! reporting, and rendering are separate modules — each one has to cope with a
//! world that is only partly there.

mod atmosphere;
#[cfg(target_arch = "wasm32")]
mod browser;
mod camera;
mod game;
mod gpu;
mod input;
mod progress;
mod random;
mod streaming;
mod trees;
mod warp;

use std::error::Error;
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

use game::Game;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const WINDOW_TITLE: &str = "Treeline — Surveyed Wilderness";

/// The terrain queue this build uses.
///
/// Native builds run worker threads inside the world crate. Browsers cannot,
/// so they drive independent Web Workers that satisfy the same contract.
#[cfg(not(target_arch = "wasm32"))]
type TerrainMeshQueue = treeline_world::TerrainMeshQueue<treeline_world::WorldTerrain>;
#[cfg(target_arch = "wasm32")]
type TerrainMeshQueue = browser::BrowserTerrainMeshQueue;

/// Starts terrain generation for a world.
///
/// # Errors
///
/// Returns an error when the browser's Web Workers cannot be started. The
/// native queue cannot fail.
#[allow(clippy::unnecessary_wraps)]
fn start_terrain_queue(
    terrain: treeline_world::WorldTerrain,
) -> Result<TerrainMeshQueue, Box<dyn Error>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(TerrainMeshQueue::for_world(terrain))
    }
    #[cfg(target_arch = "wasm32")]
    {
        browser::BrowserTerrainMeshQueue::new(terrain.world())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut TreelineApp::default())?;
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

/// Owns the game across the window lifecycle.
///
/// Startup is asynchronous because opening a GPU device is. Native builds can
/// block on it; browsers cannot, so the game arrives later through a shared
/// slot the event loop polls.
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

        let Some(window) = create_window(event_loop) else {
            event_loop.exit();
            return;
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
        if window_id == game.window_id() && game.handle_window_event(event) {
            event_loop.exit();
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let (Some(game), DeviceEvent::MouseMotion { delta }) = (self.game.as_mut(), event) {
            game.handle_mouse_motion(delta);
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
            game.request_redraw();
        }
    }
}

fn create_window(event_loop: &ActiveEventLoop) -> Option<Arc<Window>> {
    let attributes = Window::default_attributes()
        .with_title(WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(1280, 720));
    #[cfg(target_arch = "wasm32")]
    let attributes = {
        use winit::platform::web::WindowAttributesExtWebSys;

        attributes.with_append(true)
    };

    match event_loop.create_window(attributes) {
        Ok(window) => Some(Arc::new(window)),
        Err(error) => {
            eprintln!("failed to create the Treeline window: {error}");
            None
        }
    }
}
