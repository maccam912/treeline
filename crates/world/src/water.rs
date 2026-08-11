//! The lake sheet drawn over mapped waterbodies.
//!
//! Water is a separate render surface sampled at the measured footprint mask's
//! resolution. It never changes terrain density, so the shoreline the player
//! walks and the shoreline they see come from the same measured mask.

use std::collections::BTreeMap;

use treeline_mesher::{Mesh, MeshingError, SurfaceGridSpec};
use treeline_terrain::WATER_MASK_SPACING_METERS;

use crate::mesh::TerrainMeshSpec;
use crate::terrain::WorldTerrain;

/// Empirical correction applied to the representative source level.
const LEVEL_OFFSET_METERS: f64 = 0.05;

/// Keeps the corrected sheet just above its level to avoid z-fighting.
const ANTI_Z_FIGHTING_LIFT_METERS: f64 = 0.05;

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
    let grid = water_grid(spec.surface_grid()?)?;

    let [cells_x, cells_z] = grid.cell_counts;
    let mut mesh = Mesh::default();
    let mut vertices = BTreeMap::new();
    for cell_z in 0..cells_z {
        for cell_x in 0..cells_x {
            let (min, max) = cell_bounds(grid, cell_x, cell_z);
            let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
            let Some(lake) = terrain.lake_at(center[0], center[1]) else {
                continue;
            };
            append_quad(
                &mut mesh,
                &mut vertices,
                cell_x,
                cell_z,
                min,
                max,
                lake.surface_elevation_meters + LEVEL_OFFSET_METERS + ANTI_Z_FIGHTING_LIFT_METERS,
            )?;
        }
    }
    Ok(mesh)
}

fn water_grid(terrain_grid: SurfaceGridSpec) -> Result<SurfaceGridSpec, MeshingError> {
    let valid = !terrain_grid.cell_counts.contains(&0)
        && terrain_grid.origin_x.is_finite()
        && terrain_grid.origin_z.is_finite()
        && terrain_grid.spacing_meters.is_finite()
        && terrain_grid.spacing_meters > 0.0;
    if !valid {
        return Err(MeshingError::InvalidGrid);
    }

    let cells_x = aligned_water_cells(terrain_grid.cell_counts[0], terrain_grid.spacing_meters)?;
    let cells_z = aligned_water_cells(terrain_grid.cell_counts[1], terrain_grid.spacing_meters)?;
    let origin_x = aligned_water_origin(terrain_grid.origin_x)?;
    let origin_z = aligned_water_origin(terrain_grid.origin_z)?;
    Ok(SurfaceGridSpec::new(
        origin_x,
        origin_z,
        [cells_x, cells_z],
        WATER_MASK_SPACING_METERS,
    ))
}

fn aligned_water_cells(terrain_cells: usize, terrain_spacing: f64) -> Result<usize, MeshingError> {
    let extent = usize_as_f64(terrain_cells) * terrain_spacing;
    let water_cells = extent / WATER_MASK_SPACING_METERS;
    let rounded = libm::round(water_cells);
    if (water_cells - rounded).abs() > 1.0e-9 || rounded < 1.0 || rounded > usize_as_f64(usize::MAX)
    {
        return Err(MeshingError::InvalidGrid);
    }
    Ok(f64_as_usize(rounded))
}

fn aligned_water_origin(origin: f64) -> Result<f64, MeshingError> {
    let lattice = origin / WATER_MASK_SPACING_METERS;
    ((lattice - libm::round(lattice)).abs() < 1.0e-9)
        .then_some(origin)
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
    vertices: &mut BTreeMap<(usize, usize, u64), u32>,
    cell_x: usize,
    cell_z: usize,
    min: [f64; 2],
    max: [f64; 2],
    surface: f64,
) -> Result<(), MeshingError> {
    let top_left = vertex(mesh, vertices, cell_x, cell_z, min[0], min[1], surface)?;
    let bottom_left = vertex(mesh, vertices, cell_x, cell_z + 1, min[0], max[1], surface)?;
    let top_right = vertex(mesh, vertices, cell_x + 1, cell_z, max[0], min[1], surface)?;
    let bottom_right = vertex(
        mesh,
        vertices,
        cell_x + 1,
        cell_z + 1,
        max[0],
        max[1],
        surface,
    )?;
    mesh.indices.extend([
        top_left,
        bottom_left,
        top_right,
        top_right,
        bottom_left,
        bottom_right,
    ]);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn vertex(
    mesh: &mut Mesh,
    vertices: &mut BTreeMap<(usize, usize, u64), u32>,
    x: usize,
    z: usize,
    world_x: f64,
    world_z: f64,
    surface: f64,
) -> Result<u32, MeshingError> {
    let key = (x, z, surface.to_bits());
    if let Some(&index) = vertices.get(&key) {
        return Ok(index);
    }
    let index = u32::try_from(mesh.positions.len()).map_err(|_| MeshingError::TooManyVertices)?;
    mesh.positions.push([world_x, surface, world_z]);
    mesh.normals.push([0.0, 1.0, 0.0]);
    mesh.colors.push(WATER_COLOR);
    vertices.insert(key, index);
    Ok(index)
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn f64_as_usize(value: f64) -> usize {
    value as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::{ChunkMeshSpec, FarTerrainMeshSpec, FarTileIndex};
    use crate::{DEFAULT_WORLD_IDENTITY, TerrainMeshSpec};
    use treeline_coordinates::WorldPosition;
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
        let mesh = lake_sheet(TERRAIN, chunk_spec([128.0, 128.0])).expect("valid grid");
        assert!(mesh.indices.is_empty());
    }

    #[test]
    fn water_uses_the_calibrated_level_and_render_lift() {
        let mesh = lake_sheet(TERRAIN, chunk_spec(LAKE_INTERIOR)).expect("valid grid");
        let level = TERRAIN
            .lake_at(LAKE_INTERIOR[0], LAKE_INTERIOR[1])
            .expect("mapped lake")
            .surface_elevation_meters;
        for position in &mesh.positions {
            assert!(
                (position[1] - level - LEVEL_OFFSET_METERS - ANTI_Z_FIGHTING_LIFT_METERS).abs()
                    < 1.0e-9
            );
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
        assert!(maximum_triangle_axis_step(&mesh) <= WATER_MASK_SPACING_METERS);
    }

    #[test]
    fn generation_is_repeatable() {
        let spec = chunk_spec(LAKE_INTERIOR);
        assert_eq!(
            lake_sheet(TERRAIN, spec).expect("valid grid"),
            lake_sheet(TERRAIN, spec).expect("valid grid")
        );
    }

    fn maximum_triangle_axis_step(mesh: &Mesh) -> f64 {
        mesh.indices
            .chunks_exact(3)
            .flat_map(|triangle| {
                [(0, 1), (1, 2), (2, 0)]
                    .into_iter()
                    .flat_map(move |(a, b)| {
                        let a = mesh.positions[triangle[a] as usize];
                        let b = mesh.positions[triangle[b] as usize];
                        [(a[0] - b[0]).abs(), (a[2] - b[2]).abs()]
                    })
            })
            .fold(0.0, f64::max)
    }
}
