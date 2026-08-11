//! The streaming world.
//!
//! This crate composes the measured terrain bundle, the site climate, and the
//! forest generator into one [`WorldTerrain`], then decides what part of it is
//! resident and builds the meshes for it.
//!
//! The three layers are deliberately separate:
//!
//! - [`WorldTerrain`] answers questions about the world at a position. Pure,
//!   no state, safe to call from any thread.
//! - [`ChunkStreamer`] and [`FarTerrainStreamer`] decide which meshes should
//!   exist. Pure planning; they start no work and own no GPU resources.
//! - [`TerrainMeshQueue`] runs the work in priority order and caches results.
//!
//! Because a mesh is a pure function of its [`TerrainMeshSpec`], the order jobs
//! complete in is not observable by the world itself. That is what lets the
//! queue reorder, drop, cache, and parallelize freely.

mod mesh;
mod streaming;
mod terrain;
mod water;

pub use mesh::{
    GeneratedTerrainMesh, GenerationPriority, TerrainMeshQueue, TerrainMeshSpec,
    generate_world_terrain_mesh,
};
pub use streaming::{
    ChunkMeshSpec, ChunkStreamer, ChunkStreamingConfig, ChunkStreamingPlan, FarTerrainMeshSpec,
    FarTerrainStreamer, FarTerrainStreamingConfig, FarTerrainStreamingPlan, FarTileIndex,
};
pub use terrain::{LakeSurface, SnowCover, WorldTerrain};

pub use treeline_climate::Season;

use treeline_coordinates::WorldIdentity;
use treeline_terrain::SURVEYED_SETTINGS_HASH;

/// Generation contract for newly created worlds.
///
/// Increment when a change makes the same identity produce a different world.
/// The measured layers are versioned separately by the settings hash, which
/// selects the bundle; this versions everything derived from them.
pub const CURRENT_GENERATOR_VERSION: u32 = 22;

/// The world the client loads.
///
/// The seed supplies stable identities for tree individuals. The settings hash
/// selects the surveyed bundle, so changing the measurements requires a new
/// identity and cannot silently alter a saved world.
pub const DEFAULT_WORLD_IDENTITY: WorldIdentity =
    WorldIdentity::new(0x5eed, CURRENT_GENERATOR_VERSION, SURVEYED_SETTINGS_HASH);

/// Lifecycle of one region in a streaming world.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RegionState {
    #[default]
    Ungenerated,
    Generated,
    Active,
    Frozen,
}

impl RegionState {
    /// Whether a transition is legal.
    ///
    /// Terrain is regenerated rather than persisted, so a region can cycle
    /// between active and frozen indefinitely, but never returns to
    /// ungenerated.
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Ungenerated, Self::Generated)
                | (Self::Generated, Self::Active | Self::Frozen)
                | (Self::Active, Self::Frozen)
                | (Self::Frozen, Self::Active)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_world_selects_the_surveyed_bundle() {
        assert_eq!(DEFAULT_WORLD_IDENTITY.settings_hash, SURVEYED_SETTINGS_HASH);
        assert_eq!(
            DEFAULT_WORLD_IDENTITY.generator_version,
            CURRENT_GENERATOR_VERSION
        );
    }

    #[test]
    fn regions_reach_active_only_through_generation() {
        assert!(RegionState::Ungenerated.can_transition_to(RegionState::Generated));
        assert!(!RegionState::Ungenerated.can_transition_to(RegionState::Active));
        assert!(RegionState::Generated.can_transition_to(RegionState::Active));
    }

    #[test]
    fn regions_cycle_between_active_and_frozen_but_never_regress() {
        assert!(RegionState::Active.can_transition_to(RegionState::Frozen));
        assert!(RegionState::Frozen.can_transition_to(RegionState::Active));
        assert!(!RegionState::Frozen.can_transition_to(RegionState::Ungenerated));
        assert!(!RegionState::Active.can_transition_to(RegionState::Generated));
    }
}
