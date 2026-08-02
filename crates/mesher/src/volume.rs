//! Meshing near terrain as a volume.
//!
//! Marching Cubes extracts the surface from signed density. Transvoxel does the
//! same but adds transition cells on the faces where a chunk meets a finer
//! neighbour, which is what closes the cracks a plain LOD boundary would leave.
//!
//! Both bracket their vertical range around the surface the field reports,
//! rather than meshing a fixed-height column, so a chunk costs what its terrain
//! actually needs.

use mcubes::{MarchingCubes, MeshSide};
use transvoxel::prelude::{
    Block, FieldCaching, GenericMeshBuilder, TransitionSide, extract_from_field,
};
use treeline_coordinates::WorldPosition;
use treeline_terrain::{DensityField, SurfaceField};
use treeline_voxel::{ChunkFace, ChunkIndex, LodLevel, TransitionFaces};

use crate::grid::{GridSpec, validate_grid};
use crate::{Mesh, MeshingError, f64_as_f32, i64_as_f64, index_as_f64};

/// Samples a functional density field and extracts its zero-isosurface.
///
/// # Errors
///
/// Returns [`MeshingError`] when the grid is invalid, its sample count
/// overflows, or the resulting mesh cannot use the renderer's `u32` indices.
pub fn marching_cubes(field: &impl DensityField, spec: GridSpec) -> Result<Mesh, MeshingError> {
    validate_grid(spec)?;

    let [count_x, count_y, count_z] = spec.sample_counts;
    let sample_count = count_x
        .checked_mul(count_y)
        .and_then(|count| count.checked_mul(count_z))
        .ok_or(MeshingError::GridTooLarge)?;
    let mut densities = Vec::with_capacity(sample_count);

    // mcubes stores X as the fastest-moving density coordinate.
    for z in 0..count_z {
        for y in 0..count_y {
            for x in 0..count_x {
                let position = WorldPosition::new(
                    spec.origin.x + (index_as_f64(x) * spec.spacing_meters),
                    spec.origin.y + (index_as_f64(y) * spec.spacing_meters),
                    spec.origin.z + (index_as_f64(z) * spec.spacing_meters),
                );
                densities.push(f64_as_f32(field.sample(position).density));
            }
        }
    }

    let spacing = f64_as_f32(spec.spacing_meters);
    let extractor = MarchingCubes::new(
        (count_x, count_y, count_z),
        (spacing, spacing, spacing),
        (1.0, 1.0, 1.0),
        [0.0, 0.0, 0.0].into(),
        densities,
        0.0,
    )
    .map_err(|_| MeshingError::InvalidGrid)?;
    let extracted = extractor.generate(MeshSide::OutsideOnly);

    let positions = extracted
        .vertices
        .iter()
        .map(|vertex| {
            [
                spec.origin.x + f64::from(vertex.posit.x),
                spec.origin.y + f64::from(vertex.posit.y),
                spec.origin.z + f64::from(vertex.posit.z),
            ]
        })
        .collect();
    // mcubes defines its outward normal for positive-inside fields. Treeline's
    // density is negative inside solid, so its result is reversed here.
    let normals = extracted
        .vertices
        .iter()
        .map(|vertex| [-vertex.normal.x, -vertex.normal.y, -vertex.normal.z])
        .collect();
    let indices = extracted
        .indices
        .into_iter()
        .map(|index| u32::try_from(index).map_err(|_| MeshingError::TooManyVertices))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Mesh {
        positions,
        normals,
        colors: Vec::new(),
        indices,
    })
}

/// Extracts one deterministic terrain chunk on the shared near-world lattice.
///
/// # Errors
///
/// Returns [`MeshingError`] if the standard chunk grid cannot be meshed.
pub fn marching_cubes_chunk(
    field: &impl DensityField,
    chunk: ChunkIndex,
) -> Result<Mesh, MeshingError> {
    marching_cubes(
        field,
        GridSpec::new(
            chunk.sample_origin(),
            ChunkIndex::sample_counts(),
            ChunkIndex::LOD.spacing_meters(),
        ),
    )
}

/// Extracts one fixed-footprint terrain chunk with optional Transvoxel faces.
///
/// Transition faces belong on the coarser chunk wherever an adjacent chunk is
/// sampled at exactly twice its resolution. The resulting boundary follows the
/// finer lattice without skirts or overlapping geometry.
///
/// # Errors
///
/// Returns [`MeshingError::UnsupportedLod`] if `lod` is outside the streamed
/// voxel range, or [`MeshingError::TooManyVertices`] if indices do not fit the
/// renderer's `u32` format.
pub fn transvoxel_chunk(
    field: &(impl DensityField + SurfaceField),
    chunk: ChunkIndex,
    lod: LodLevel,
    transition_faces: TransitionFaces,
) -> Result<Mesh, MeshingError> {
    let subdivisions = ChunkIndex::subdivisions(lod).ok_or(MeshingError::UnsupportedLod)?;
    let horizontal_origin = chunk.sample_origin();
    let edge_meters = ChunkIndex::edge_meters();
    let (minimum_height, maximum_height) =
        chunk_surface_bounds(field, horizontal_origin, edge_meters, subdivisions)?;
    let minimum_layer = vertical_layer(minimum_height, edge_meters)?;
    let maximum_layer = vertical_layer(maximum_height, edge_meters)?;
    let mut sides = TransitionSide::none();
    if transition_faces.contains(ChunkFace::LowX) {
        sides |= TransitionSide::LowX;
    }
    if transition_faces.contains(ChunkFace::HighX) {
        sides |= TransitionSide::HighX;
    }
    if transition_faces.contains(ChunkFace::LowZ) {
        sides |= TransitionSide::LowZ;
    }
    if transition_faces.contains(ChunkFace::HighZ) {
        sides |= TransitionSide::HighZ;
    }

    // Transvoxel defines values above the threshold as solid. Negating
    // Treeline's negative-inside field also gives the extractor outward-facing
    // normals without a post-process winding reversal.
    let mut mesh = Mesh::default();
    for layer in minimum_layer..=maximum_layer {
        let origin_y = ChunkIndex::MIN_Y_METERS + (i64_as_f64(layer) * edge_meters);
        let density = |x: f32, y: f32, z: f32| {
            -f64_as_f32(
                field
                    .sample(WorldPosition::new(
                        horizontal_origin.x + f64::from(x),
                        origin_y + f64::from(y),
                        horizontal_origin.z + f64::from(z),
                    ))
                    .density,
            )
        };
        let block = Block::new([0.0, 0.0, 0.0], f64_as_f32(edge_meters), subdivisions);
        let extracted = extract_from_field(
            &density,
            FieldCaching::CacheNothing,
            block,
            sides,
            0.0,
            GenericMeshBuilder::new(),
        )
        .build();
        append_transvoxel_mesh(
            &mut mesh,
            extracted,
            WorldPosition::new(horizontal_origin.x, origin_y, horizontal_origin.z),
        )?;
    }
    Ok(mesh)
}

fn chunk_surface_bounds(
    field: &impl SurfaceField,
    origin: WorldPosition,
    edge_meters: f64,
    subdivisions: usize,
) -> Result<(f64, f64), MeshingError> {
    let spacing = edge_meters / index_as_f64(subdivisions);
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    for z in 0..=subdivisions {
        for x in 0..=subdivisions {
            let height = field
                .surface_height(
                    origin.x + (index_as_f64(x) * spacing),
                    origin.z + (index_as_f64(z) * spacing),
                )
                .ok_or(MeshingError::MissingSurface)?;
            minimum = minimum.min(height);
            maximum = maximum.max(height);
        }
    }
    if let Some((volume_minimum, volume_maximum)) = field.volume_bounds(
        origin.x,
        origin.z,
        origin.x + edge_meters,
        origin.z + edge_meters,
    ) {
        if !volume_minimum.is_finite()
            || !volume_maximum.is_finite()
            || volume_minimum > volume_maximum
        {
            return Err(MeshingError::InvalidGrid);
        }
        minimum = minimum.min(volume_minimum);
        maximum = maximum.max(volume_maximum);
    }
    Ok((minimum, maximum))
}

fn vertical_layer(height: f64, edge_meters: f64) -> Result<i64, MeshingError> {
    let layer = libm::floor((height - ChunkIndex::MIN_Y_METERS) / edge_meters);
    if !layer.is_finite() || layer < i64_as_f64(i64::MIN) || layer >= i64_as_f64(i64::MAX) {
        return Err(MeshingError::InvalidGrid);
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(layer as i64)
}

fn append_transvoxel_mesh(
    mesh: &mut Mesh,
    extracted: transvoxel::structs::generic_mesh::Mesh<f32>,
    world_origin: WorldPosition,
) -> Result<(), MeshingError> {
    let vertex_offset =
        u32::try_from(mesh.positions.len()).map_err(|_| MeshingError::TooManyVertices)?;
    mesh.positions
        .extend(extracted.positions.chunks_exact(3).map(|position| {
            [
                world_origin.x + f64::from(position[0]),
                world_origin.y + f64::from(position[1]),
                world_origin.z + f64::from(position[2]),
            ]
        }));
    mesh.normals.extend(
        extracted
            .normals
            .chunks_exact(3)
            .map(|normal| [normal[0], normal[1], normal[2]]),
    );
    mesh.indices.extend(
        extracted
            .triangle_indices
            .into_iter()
            .map(|index| {
                u32::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_add(vertex_offset))
                    .ok_or(MeshingError::TooManyVertices)
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{assert_front_facing, mesh_fingerprint};
    use treeline_terrain::{GroundPlane, Material, SmoothHills};

    #[test]
    fn ground_plane_produces_a_well_formed_surface() {
        let field = GroundPlane {
            surface_height: 0.25,
            material: Material::Soil,
        };
        let mesh = marching_cubes(
            &field,
            GridSpec::new(WorldPosition::new(-2.0, -2.0, -2.0), [5, 5, 5], 1.0),
        )
        .expect("valid grid");

        assert!(mesh.is_well_formed());
        assert!(!mesh.indices.is_empty());
        assert!(
            mesh.positions
                .iter()
                .all(|position| (position[1] - 0.25).abs() < f64::EPSILON)
        );
        assert!(mesh.normals.iter().all(|normal| normal[1] > 0.99));
        assert_front_facing(&mesh);
    }

    #[test]
    fn rolling_hill_mesh_is_repeatable() {
        let field = SmoothHills;
        let spec = GridSpec::new(WorldPosition::new(-8.0, -4.0, -8.0), [9, 13, 9], 2.0);
        let first = marching_cubes(&field, spec).expect("valid grid");
        let second = marching_cubes(&field, spec).expect("valid grid");
        assert_eq!(first, second);
        assert!(first.is_well_formed());
        assert!(!first.indices.is_empty());
    }

    #[test]
    fn chunk_meshes_are_repeatable_and_order_independent() {
        let field = SmoothHills;
        let first_chunk = ChunkIndex::new(-3, 2);
        let second_chunk = ChunkIndex::new(-2, 2);

        let first_then_second = (
            marching_cubes_chunk(&field, first_chunk).expect("valid chunk"),
            marching_cubes_chunk(&field, second_chunk).expect("valid chunk"),
        );
        let second_then_first = (
            marching_cubes_chunk(&field, second_chunk).expect("valid chunk"),
            marching_cubes_chunk(&field, first_chunk).expect("valid chunk"),
        );

        assert_eq!(first_then_second.0, second_then_first.1);
        assert_eq!(first_then_second.1, second_then_first.0);
        assert!(first_then_second.0.is_well_formed());
        assert!(first_then_second.1.is_well_formed());
    }

    /// Adjacent chunks must produce the same surface on the plane they share.
    ///
    /// Positions are compared with tolerance rather than bit-for-bit. Each
    /// chunk brackets its own vertical range before meshing, so two chunks
    /// solve the shared plane's surface crossings against slightly different
    /// vertical grids. The disagreement that leaves is interpolation error over
    /// one voxel — under a micron here — not a hole.
    #[test]
    fn adjacent_chunk_meshes_meet_on_the_shared_plane() {
        let field = SmoothHills;
        let left = marching_cubes_chunk(&field, ChunkIndex::new(0, 0)).expect("valid chunk");
        let right = marching_cubes_chunk(&field, ChunkIndex::new(1, 0)).expect("valid chunk");
        let boundary_x = ChunkIndex::edge_meters();

        let left_boundary = boundary_positions(&left, boundary_x);
        let right_boundary = boundary_positions(&right, boundary_x);
        assert!(!left_boundary.is_empty());
        assert!(boundaries_match(&left_boundary, &right_boundary));
    }

    #[test]
    fn chunk_mesh_has_a_golden_fingerprint() {
        let field = SmoothHills;
        let mesh = marching_cubes_chunk(&field, ChunkIndex::new(-3, 2)).expect("valid chunk");
        assert_eq!(
            mesh_fingerprint(&mesh),
            17_199_403_916_769_715_255,
            "changing this value changes generated terrain chunks"
        );
    }

    #[test]
    fn transvoxel_lods_are_repeatable_and_progressively_coarser() {
        let field = SmoothHills;
        let chunk = ChunkIndex::new(-3, 2);
        let fine = transvoxel_chunk(&field, chunk, ChunkIndex::NEAR_LOD, TransitionFaces::none())
            .expect("fine chunk");
        let fine_again =
            transvoxel_chunk(&field, chunk, ChunkIndex::NEAR_LOD, TransitionFaces::none())
                .expect("fine chunk");
        let coarse = transvoxel_chunk(&field, chunk, ChunkIndex::MAX_LOD, TransitionFaces::none())
            .expect("coarse chunk");

        assert_eq!(fine, fine_again);
        assert!(fine.is_well_formed());
        assert!(coarse.is_well_formed());
        assert!(!fine.indices.is_empty());
        assert!(coarse.indices.len() < fine.indices.len());
        assert_front_facing(&fine);
        assert_front_facing(&coarse);
    }

    #[test]
    fn transvoxel_follows_surfaces_above_the_original_terrain_slab() {
        let field = GroundPlane {
            surface_height: 1_001.25,
            material: Material::Rock,
        };
        let mesh = transvoxel_chunk(
            &field,
            ChunkIndex::new(-2, 3),
            ChunkIndex::NEAR_LOD,
            TransitionFaces::none(),
        )
        .expect("high terrain chunk");

        assert!(!mesh.indices.is_empty());
        assert!(
            mesh.positions
                .iter()
                .all(|position| (position[1] - 1_001.25).abs() < 0.001)
        );
    }

    #[test]
    fn transvoxel_transition_matches_the_finer_boundary() {
        let field = SmoothHills;
        let fine_chunk = ChunkIndex::new(2, 0);
        let coarse_chunk = ChunkIndex::new(3, 0);
        let fine = transvoxel_chunk(
            &field,
            fine_chunk,
            ChunkIndex::NEAR_LOD,
            TransitionFaces::none(),
        )
        .expect("fine chunk");
        let coarse = transvoxel_chunk(
            &field,
            coarse_chunk,
            LodLevel::new(3),
            TransitionFaces::none().with(ChunkFace::LowX),
        )
        .expect("transition chunk");
        let boundary_x = coarse_chunk.sample_origin().x;

        let fine_boundary = boundary_positions(&fine, boundary_x);
        let coarse_boundary = boundary_positions(&coarse, boundary_x);
        assert!(!fine_boundary.is_empty());
        assert!(boundaries_match(&fine_boundary, &coarse_boundary));
    }

    #[test]
    fn transvoxel_rejects_lods_outside_the_streamed_range() {
        let field = SmoothHills;
        assert_eq!(
            transvoxel_chunk(
                &field,
                ChunkIndex::new(0, 0),
                LodLevel::new(1),
                TransitionFaces::none(),
            ),
            Err(MeshingError::UnsupportedLod)
        );
    }

    fn boundary_positions(mesh: &Mesh, boundary_x: f64) -> Vec<[f64; 2]> {
        mesh.positions
            .iter()
            .filter(|position| (position[0] - boundary_x).abs() < 0.000_1)
            .map(|position| [position[1], position[2]])
            .collect()
    }

    fn boundaries_match(left: &[[f64; 2]], right: &[[f64; 2]]) -> bool {
        const EPSILON: f64 = 0.000_1;
        let contains = |haystack: &[[f64; 2]], needle: &[f64; 2]| {
            haystack.iter().any(|candidate| {
                (candidate[0] - needle[0]).abs() < EPSILON
                    && (candidate[1] - needle[1]).abs() < EPSILON
            })
        };
        left.iter().all(|position| contains(right, position))
            && right.iter().all(|position| contains(left, position))
    }

    #[test]
    fn invalid_grids_are_rejected() {
        let field = GroundPlane {
            surface_height: 0.0,
            material: Material::Soil,
        };
        assert_eq!(
            marching_cubes(
                &field,
                GridSpec::new(WorldPosition::new(0.0, 0.0, 0.0), [1, 2, 2], 1.0)
            ),
            Err(MeshingError::InvalidGrid)
        );
    }
}
