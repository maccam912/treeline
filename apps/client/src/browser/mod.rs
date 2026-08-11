//! Browser-only plumbing.
//!
//! Two things differ in a browser: there are no worker threads inside the
//! process, so terrain generation goes out to Web Workers; and there is no
//! keyboard on a phone, so the page carries buttons.

mod actions;
mod terrain_queue;

pub use actions::BrowserActions;
pub use terrain_queue::BrowserTerrainMeshQueue;
