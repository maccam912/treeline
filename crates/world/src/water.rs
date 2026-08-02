//! The lake sheet drawn over mapped waterbodies.
//!
//! Water is a separate render surface: a horizontal quad per grid cell whose
//! center falls inside a mapped footprint. It never changes terrain density, so
//! the shoreline the player walks and the shoreline they see come from the same
//! measured mask.

use treeline_mesher::{Mesh, MeshingError, SurfaceGridSpec};

use crate::mesh::TerrainMeshSpec;
use crate::terrain::WorldTerrain;

/// Lifts water above the terrain surface so the two do not z-fight.
const RENDER_OFFSET_METERS: f64 = 0.05;

/// Depth given to a lake cell that the recorded level does not reach.
///
/// The bundle's footprint is dilated one cell past the mapped polygon so the
/// sheet meets the shore. Those cells can sit above the recorded level; they
/// still need a visible film.
pub const MINIMUM_VISIBLE_DEPTH_METERS: f64 = 0.05;

const WATER_COLOR: [f32; 4] = [0.04, 0.34, 0.58, 1.0];

/// Builds the lake sheet covering one terrain mesh's footprint.
///
/// # Errors
///
/// Returns [`MeshingError`] when the LOD is unsupported, the grid is invalid,
/// or the sheet exceeds `u32` index capacity.
pub fn lake_sheet(terrain: WorldTerrain, spec: TerrainMeshSpec) -> Result<Mesh, MeshingError> {
    let grid = spec.surface_grid()?;
    validate(grid)?;

    let [cells_x, cells_z] = grid.cell_counts;
    let mut mesh = Mesh::default();
    for cell_z in 0..cells_z {
        for cell_x in 0..cells_x {
            let (min, max) = cell_bounds(grid, cell_x, cell_z);
            if grid
                .cutout
                .is_some_and(|cutout| cutout.contains_cell(min[0], max[0], min[1], max[1]))
            {
                continue;
            }
            let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
            let Some(lake) = terrain.lake_at(center[0], center[1]) else {
                continue;
            };
            append_quad(
                &mut mesh,
                min,
                max,
                lake.surface_elevation_meters + RENDER_OFFSET_METERS,
            )?;
        }
    }
    Ok(mesh)
}

fn validate(grid: SurfaceGridSpec) -> Result<(), MeshingError> {
    (!grid.cell_counts.contains(&0)
        && grid.origin_x.is_finite()
        && grid.origin_z.is_finite()
        && grid.spacing_meters.is_finite()
        && grid.spacing_meters > 0.0)
        .then_some(())
        .ok_or(MeshingError::InvalidGrid)
}

fn cell_bounds(grid: SurfaceGridSpec, cell_x: usize, cell_z: usize) -> ([f64; 2], [f64; 2]) {
    let min = [
        grid.origin_x + (usize_as_f64(cell_x) * grid.spacing_meters),
        grid.origin_z + (usize_as_f64(cell_z) * grid.spacing_meters),
    ];
    (
        min,
        [min[0] + grid.spacing_meters, min[1] + grid.spacing_meters],
    )
}

/// Appends one upward-facing water quad at a fixed elevation.
fn append_quad(
    mesh: &mut Mesh,
    min: [f64; 2],
    max: [f64; 2],
    surface: f64,
) -> Result<(), MeshingError> {
    let base = u32::try_from(mesh.positions.len()).map_err(|_| MeshingError::TooManyVertices)?;
    let corner = |offset: u32| {
        base.checked_add(offset)
            .ok_or(MeshingError::TooManyVertices)
    };

    mesh.positions.extend([
        [min[0], surface, min[1]],
        [min[0], surface, max[1]],
        [max[0], surface, min[1]],
        [max[0], surface, max[1]],
    ]);
    mesh.normals.extend([[0.0, 1.0, 0.0]; 4]);
    mesh.colors.extend([WATER_COLOR; 4]);
    mesh.indices.extend([
        base,
        corner(1)?,
        corner(2)?,
        corner(2)?,
        corner(1)?,
        corner(3)?,
    ]);
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::{ChunkMeshSpec, FarTerrainMeshSpec, FarTileIndex};
    use crate::{DEFAULT_WORLD_IDENTITY, TerrainMeshSpec};
    use treeline_coordinates::WorldPosition;
    use treeline_terrain::{SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z};
    use treeline_voxel::{ChunkIndex, TransitionFaces};

    const TERRAIN: WorldTerrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);
    const LAKE_INTERIOR: [f64; 2] = [7_364.0, 6_894.0];

    fn chunk_spec(position: [f64; 2]) -> TerrainMeshSpec {
        let chunk = ChunkIndex::containing(WorldPosition::new(position[0], 0.0, position[1]))
            .expect("position is inside chunk range");
        TerrainMeshSpec::Near(ChunkMeshSpec {
            chunk,
            lod: ChunkIndex::NEAR_LOD,
            transition_faces: TransitionFaces::none(),
        })
    }

    #[test]
    fn a_chunk_over_a_mapped_lake_gets_water() {
        let mesh = lake_sheet(TERRAIN, chunk_spec(LAKE_INTERIOR)).expect("valid grid");
        assert!(mesh.is_well_formed());
        assert!(!mesh.indices.is_empty());
        assert!(mesh.normals.iter().all(|normal| normal[1] > 0.99));
    }

    #[test]
    fn dry_ground_gets_no_water() {
        let mesh = lake_sheet(TERRAIN, chunk_spec([SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z]))
            .expect("valid grid");
        assert!(mesh.indices.is_empty());
    }

    #[test]
    fn water_sits_just_above_its_recorded_level() {
        let mesh = lake_sheet(TERRAIN, chunk_spec(LAKE_INTERIOR)).expect("valid grid");
        let level = TERRAIN
            .lake_at(LAKE_INTERIOR[0], LAKE_INTERIOR[1])
            .expect("mapped lake")
            .surface_elevation_meters;
        for position in &mesh.positions {
            assert!((position[1] - level - RENDER_OFFSET_METERS).abs() < 1.0e-9);
        }
    }

    #[test]
    fn far_tiles_produce_the_same_kind_of_sheet() {
        let tile =
            FarTileIndex::containing(WorldPosition::new(LAKE_INTERIOR[0], 0.0, LAKE_INTERIOR[1]))
                .expect("position is inside far tile range");
        let mesh = lake_sheet(TERRAIN, TerrainMeshSpec::Far(FarTerrainMeshSpec { tile }))
            .expect("valid grid");
        assert!(mesh.is_well_formed());
    }

    #[test]
    fn generation_is_repeatable() {
        let spec = chunk_spec(LAKE_INTERIOR);
        assert_eq!(
            lake_sheet(TERRAIN, spec).expect("valid grid"),
            lake_sheet(TERRAIN, spec).expect("valid grid")
        );
    }
}
