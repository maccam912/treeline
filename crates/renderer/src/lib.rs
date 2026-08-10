//! Rendering for Treeline's surveyed world.
//!
//! The renderer draws four things — terrain, water, trees, and sky — through a
//! single pipeline and a single vertex format. What varies between them is a
//! `surface_kind` tag and which material layer the shader samples, not the
//! shader or the pipeline.
//!
//! Two properties shape the whole crate. World coordinates exceed `f32`
//! precision, so positions travel as split high/low pairs and are reconstructed
//! relative to the camera. And terrain is streamed as independently owned
//! meshes, so uploads never touch shared state and can happen in any order.
//!
//! This crate deliberately knows nothing about world generation: callers pass
//! in finished meshes, tree individuals, and lighting values.

mod gpu;
mod lighting;
mod material;
mod renderer;
mod snow;
mod tree_mesh;
mod uniform;
mod vertex;

pub use gpu::TerrainMesh;
pub use lighting::{
    AtmosphereSettings, LightingSettings, SHADOW_CASTER_DISTANCE_METERS, TimeOfDay,
};
pub use renderer::TerrainRenderer;

use std::error::Error;
use std::fmt::{Display, Formatter};

/// The only way rendering fails: a mesh too large to address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererError {
    TooManyIndices,
}

impl Display for RendererError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyIndices => formatter.write_str("the terrain mesh has too many indices"),
        }
    }
}

impl Error for RendererError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerrainRenderTier {
    FullVoxel,
    VoxelLod,
    CoarseSurface,
    Horizon,
}

/// Geometry detail for one deterministic set of procedural tree individuals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeMeshDetail {
    /// Trunks, branches, and damage.
    Full,
    /// Trunks alone, without individual branches.
    Simplified,
    /// A minimal trunk silhouette for the outer individual-tree ring.
    Silhouette,
}

/// Selects a representation from horizontal distance in meters.
pub fn terrain_tier(distance_meters: f64) -> TerrainRenderTier {
    if distance_meters < 200.0 {
        TerrainRenderTier::FullVoxel
    } else if distance_meters < 2_000.0 {
        TerrainRenderTier::VoxelLod
    } else if distance_meters < 20_000.0 {
        TerrainRenderTier::CoarseSurface
    } else {
        TerrainRenderTier::Horizon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representation_coarsens_with_distance() {
        assert_eq!(terrain_tier(10.0), TerrainRenderTier::FullVoxel);
        assert_eq!(terrain_tier(500.0), TerrainRenderTier::VoxelLod);
        assert_eq!(terrain_tier(5_000.0), TerrainRenderTier::CoarseSurface);
        assert_eq!(terrain_tier(30_000.0), TerrainRenderTier::Horizon);
    }
}
