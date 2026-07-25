//! Renderer-facing representation tiers, independent of a concrete GPU backend.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerrainRenderTier {
    FullVoxel,
    VoxelLod,
    CoarseSurface,
    Horizon,
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
    fn distant_mountains_do_not_require_voxel_interiors() {
        assert_eq!(terrain_tier(30_000.0), TerrainRenderTier::Horizon);
    }
}
