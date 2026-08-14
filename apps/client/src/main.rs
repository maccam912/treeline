//! Treeline's Bevy application.
//!
//! Generation stays in the domain crates and remains deterministic. Bevy owns
//! the application lifecycle, input, assets, scene visibility, rendering, and
//! platform backends; the game systems stream finished measured-world meshes
//! into that engine-facing layer.

#![allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]

mod atmosphere;
#[cfg(target_arch = "wasm32")]
mod browser;
mod camera;
mod game;
mod input;
#[cfg(not(target_arch = "wasm32"))]
mod profiling;
mod progress;
mod random;
mod streaming;
mod trees;
mod warp;

use std::error::Error;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};
use treeline_renderer::TreelineRenderPlugin;

pub const WINDOW_TITLE: &str = "Treeline — Surveyed Wilderness";

#[cfg(not(target_arch = "wasm32"))]
type TerrainMeshQueue = treeline_world::TerrainMeshQueue<treeline_world::WorldTerrain>;
#[cfg(target_arch = "wasm32")]
type TerrainMeshQueue = browser::BrowserTerrainMeshQueue;

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

fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    let (terrain, jobs) = game::initial_world()?;
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: WINDOW_TITLE.into(),
            resolution: WindowResolution::new(1280, 720),
            present_mode: PresentMode::AutoVsync,
            fit_canvas_to_parent: true,
            prevent_default_event_handling: false,
            ..default()
        }),
        ..default()
    }))
    .add_plugins(TreelineRenderPlugin);

    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(profiling::FrameProfilerPlugin);

    app.insert_resource(game::GameTerrain(terrain))
        .insert_non_send(game::TerrainJobs(jobs))
        .add_plugins(game::TreelineGamePlugin);

    #[cfg(target_arch = "wasm32")]
    app.insert_non_send(browser::BrowserActions::new()?);

    app.run();
    Ok(())
}
