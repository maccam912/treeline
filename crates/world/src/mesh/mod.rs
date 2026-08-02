//! Terrain mesh jobs: what to build, in what order, and what comes back.

mod cache;
mod queue;

pub use queue::TerrainMeshQueue;

use treeline_mesher::{Mesh, MeshingError, SurfaceGridSpec, transvoxel_chunk};
use treeline_terrain::{DensityField, SurfaceField};
use treeline_voxel::ChunkIndex;
use web_time::{Duration, Instant};

use crate::streaming::{ChunkMeshSpec, FarTerrainMeshSpec, far_terrain_mesh};
use crate::terrain::WorldTerrain;

/// Job tiers, ordered so distant terrain becomes visible before near detail.
///
/// The ordering is the type's contract: `PlayerTerrain` outranks everything, so
/// the ground under the player is never queued behind speculative work.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GenerationPriority {
    PlayerTerrain,
    Horizon,
    FarTerrain,
    NearTerrain,
    PrefetchTerrain,
}

impl GenerationPriority {
    /// Wire representation, for handing a job to a browser worker.
    pub const fn code(self) -> u8 {
        match self {
            Self::PlayerTerrain => 0,
            Self::Horizon => 1,
            Self::FarTerrain => 2,
            Self::NearTerrain => 3,
            Self::PrefetchTerrain => 4,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::PlayerTerrain),
            1 => Some(Self::Horizon),
            2 => Some(Self::FarTerrain),
            3 => Some(Self::NearTerrain),
            4 => Some(Self::PrefetchTerrain),
            _ => None,
        }
    }
}

/// Everything needed to regenerate one terrain mesh, at either tier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TerrainMeshSpec {
    Far(FarTerrainMeshSpec),
    Near(ChunkMeshSpec),
}

impl TerrainMeshSpec {
    /// The horizontal grid this spec's water surface is sampled on.
    ///
    /// Water is meshed as a flat sheet at both tiers, aligned with whatever
    /// terrain resolution the tier uses.
    ///
    /// # Errors
    ///
    /// Returns [`MeshingError::UnsupportedLod`] for an LOD outside the
    /// streamed range.
    pub fn surface_grid(self) -> Result<SurfaceGridSpec, MeshingError> {
        match self {
            Self::Far(spec) => Ok(spec.surface_grid()),
            Self::Near(spec) => {
                let subdivisions =
                    ChunkIndex::subdivisions(spec.lod).ok_or(MeshingError::UnsupportedLod)?;
                let origin = spec.chunk.sample_origin();
                Ok(SurfaceGridSpec::new(
                    origin.x,
                    origin.z,
                    [subdivisions; 2],
                    ChunkIndex::edge_meters() / usize_as_f64(subdivisions),
                ))
            }
        }
    }
}

/// One finished terrain job.
///
/// Meshing failures travel with the result rather than aborting the job, so a
/// single bad chunk cannot stall streaming; the caller retries it later.
#[derive(Clone, Debug)]
pub struct GeneratedTerrainMesh {
    pub spec: TerrainMeshSpec,
    pub priority: GenerationPriority,
    pub mesh: Result<Mesh, MeshingError>,
    pub lake_mesh: Option<Result<Mesh, MeshingError>>,
    pub terrain_generation_time: Duration,
    pub lake_generation_time: Duration,
    pub cache_hit: bool,
}

type TerrainMeshGenerator<F> = fn(&F, TerrainMeshSpec) -> Result<Mesh, MeshingError>;
type LakeMeshGenerator<F> = fn(&F, TerrainMeshSpec) -> Result<Mesh, MeshingError>;

/// Generates one complete terrain result on the calling thread.
///
/// Browser workers call this directly, so they follow exactly the same contract
/// as the native queue's workers.
pub fn generate_world_terrain_mesh(
    field: &WorldTerrain,
    priority: GenerationPriority,
    spec: TerrainMeshSpec,
) -> GeneratedTerrainMesh {
    generate(
        field,
        Some(WorldTerrain::render_mesh),
        Some(WorldTerrain::lake_surface_mesh),
        priority,
        spec,
    )
}

/// Builds a terrain mesh and its optional water sheet, timing each.
///
/// A field with no custom generators falls back to plain terrain meshing, which
/// is what the analytic reference fields use.
fn generate<F>(
    field: &F,
    terrain_mesh_generator: Option<TerrainMeshGenerator<F>>,
    lake_mesh_generator: Option<LakeMeshGenerator<F>>,
    priority: GenerationPriority,
    spec: TerrainMeshSpec,
) -> GeneratedTerrainMesh
where
    F: DensityField + SurfaceField,
{
    let terrain_started = Instant::now();
    let mesh = terrain_mesh_generator.map_or_else(
        || match spec {
            TerrainMeshSpec::Far(spec) => far_terrain_mesh(field, spec),
            TerrainMeshSpec::Near(spec) => {
                transvoxel_chunk(field, spec.chunk, spec.lod, spec.transition_faces)
            }
        },
        |generator| generator(field, spec),
    );
    let terrain_generation_time = terrain_started.elapsed();

    let lake_started = Instant::now();
    let lake_mesh = lake_mesh_generator.map(|generator| generator(field, spec));
    let lake_generation_time = if lake_mesh.is_some() {
        lake_started.elapsed()
    } else {
        Duration::ZERO
    };

    GeneratedTerrainMesh {
        spec,
        priority,
        mesh,
        lake_mesh,
        terrain_generation_time,
        lake_generation_time,
        cache_hit: false,
    }
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::FarTileIndex;
    use treeline_voxel::TransitionFaces;

    #[test]
    fn priorities_order_player_terrain_ahead_of_speculation() {
        assert!(GenerationPriority::PlayerTerrain < GenerationPriority::Horizon);
        assert!(GenerationPriority::Horizon < GenerationPriority::FarTerrain);
        assert!(GenerationPriority::FarTerrain < GenerationPriority::NearTerrain);
        assert!(GenerationPriority::NearTerrain < GenerationPriority::PrefetchTerrain);
    }

    #[test]
    fn priority_codes_round_trip_and_reject_unknown_values() {
        for priority in [
            GenerationPriority::PlayerTerrain,
            GenerationPriority::Horizon,
            GenerationPriority::FarTerrain,
            GenerationPriority::NearTerrain,
            GenerationPriority::PrefetchTerrain,
        ] {
            assert_eq!(
                GenerationPriority::from_code(priority.code()),
                Some(priority)
            );
        }
        assert_eq!(GenerationPriority::from_code(200), None);
    }

    #[test]
    fn near_and_far_water_grids_cover_their_own_footprint() {
        let far = TerrainMeshSpec::Far(FarTerrainMeshSpec {
            tile: FarTileIndex::new(0, 0),
        })
        .surface_grid()
        .expect("far grids are always supported");
        assert_eq!(far.cell_counts, [FarTileIndex::CELLS_PER_EDGE; 2]);

        let near = TerrainMeshSpec::Near(ChunkMeshSpec {
            chunk: ChunkIndex::new(0, 0),
            lod: ChunkIndex::NEAR_LOD,
            transition_faces: TransitionFaces::none(),
        })
        .surface_grid()
        .expect("the near LOD is streamed");
        let subdivisions =
            ChunkIndex::subdivisions(ChunkIndex::NEAR_LOD).expect("the near LOD is streamed");
        assert_eq!(near.cell_counts, [subdivisions; 2]);
        assert!(
            (near.spacing_meters * usize_as_f64(subdivisions) - ChunkIndex::edge_meters()).abs()
                < 1.0e-9
        );
    }
}
