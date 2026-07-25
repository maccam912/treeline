//! Marching Cubes extraction and mesh output shared with future Transvoxel LODs.

use std::error::Error;
use std::fmt::{Display, Formatter};

use mcubes::{MarchingCubes, MeshSide};
use transvoxel::prelude::{
    Block, FieldCaching, GenericMeshBuilder, TransitionSide, extract_from_field,
};
use treeline_coordinates::WorldPosition;
use treeline_terrain::DensityField;
use treeline_voxel::{ChunkFace, ChunkIndex, LodLevel, TransitionFaces};

/// Renderer-neutral indexed triangle mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_well_formed(&self) -> bool {
        let Ok(vertex_count) = u32::try_from(self.positions.len()) else {
            return false;
        };
        self.positions.len() == self.normals.len()
            && self.indices.len() % 3 == 0
            && self.indices.iter().all(|&index| index < vertex_count)
    }
}

/// Regular density-sample lattice consumed by Marching Cubes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSpec {
    pub origin: WorldPosition,
    pub sample_counts: [usize; 3],
    pub spacing_meters: f64,
}

impl GridSpec {
    pub const fn new(
        origin: WorldPosition,
        sample_counts: [usize; 3],
        spacing_meters: f64,
    ) -> Self {
        Self {
            origin,
            sample_counts,
            spacing_meters,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeshingError {
    InvalidGrid,
    GridTooLarge,
    TooManyVertices,
    UnsupportedLod,
}

impl Display for MeshingError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGrid => formatter.write_str("the sample grid is invalid"),
            Self::GridTooLarge => formatter.write_str("the sample grid is too large"),
            Self::TooManyVertices => formatter.write_str("the mesh exceeds u32 index capacity"),
            Self::UnsupportedLod => formatter.write_str("the chunk LOD is not supported"),
        }
    }
}

impl Error for MeshingError {}

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
        [
            f64_as_f32(spec.origin.x),
            f64_as_f32(spec.origin.y),
            f64_as_f32(spec.origin.z),
        ]
        .into(),
        densities,
        0.0,
    )
    .map_err(|_| MeshingError::InvalidGrid)?;
    let extracted = extractor.generate(MeshSide::OutsideOnly);

    let positions = extracted
        .vertices
        .iter()
        .map(|vertex| [vertex.posit.x, vertex.posit.y, vertex.posit.z])
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
    field: &impl DensityField,
    chunk: ChunkIndex,
    lod: LodLevel,
    transition_faces: TransitionFaces,
) -> Result<Mesh, MeshingError> {
    let subdivisions = ChunkIndex::subdivisions(lod).ok_or(MeshingError::UnsupportedLod)?;
    let origin = chunk.sample_origin();
    let block = Block::new(
        [
            f64_as_f32(origin.x),
            f64_as_f32(origin.y),
            f64_as_f32(origin.z),
        ],
        f64_as_f32(ChunkIndex::edge_meters()),
        subdivisions,
    );
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
    let density = |x: f32, y: f32, z: f32| {
        -f64_as_f32(
            field
                .sample(WorldPosition::new(f64::from(x), f64::from(y), f64::from(z)))
                .density,
        )
    };
    let extracted = extract_from_field(
        &density,
        FieldCaching::CacheNothing,
        block,
        sides,
        0.0,
        GenericMeshBuilder::new(),
    )
    .build();

    let positions = extracted
        .positions
        .chunks_exact(3)
        .map(|position| [position[0], position[1], position[2]])
        .collect();
    let normals = extracted
        .normals
        .chunks_exact(3)
        .map(|normal| [normal[0], normal[1], normal[2]])
        .collect();
    let indices = extracted
        .triangle_indices
        .into_iter()
        .map(|index| u32::try_from(index).map_err(|_| MeshingError::TooManyVertices))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Mesh {
        positions,
        normals,
        indices,
    })
}

fn validate_grid(spec: GridSpec) -> Result<(), MeshingError> {
    if spec.sample_counts.iter().any(|&count| count < 2)
        || !spec.spacing_meters.is_finite()
        || spec.spacing_meters <= 0.0
        || !spec.origin.x.is_finite()
        || !spec.origin.y.is_finite()
        || !spec.origin.z.is_finite()
    {
        return Err(MeshingError::InvalidGrid);
    }
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: usize) -> f64 {
    index as f64
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use treeline_coordinates::{WorldIdentity, stable_hash};
    use treeline_terrain::{GroundPlane, Material, RollingHills};

    #[test]
    fn malformed_triangle_indices_are_rejected() {
        let mesh = Mesh {
            positions: vec![[0.0; 3]],
            normals: vec![[0.0, 1.0, 0.0]],
            indices: vec![0, 0],
        };
        assert!(!mesh.is_well_formed());
    }

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
                .all(|position| (position[1] - 0.25).abs() < f32::EPSILON)
        );
        assert!(mesh.normals.iter().all(|normal| normal[1] > 0.99));
    }

    #[test]
    fn rolling_hill_mesh_is_repeatable() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let spec = GridSpec::new(WorldPosition::new(-8.0, -4.0, -8.0), [9, 13, 9], 2.0);
        let first = marching_cubes(&field, spec).expect("valid grid");
        let second = marching_cubes(&field, spec).expect("valid grid");
        assert_eq!(first, second);
        assert!(first.is_well_formed());
        assert!(!first.indices.is_empty());
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

    #[test]
    fn chunk_meshes_are_repeatable_and_order_independent() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
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

    #[test]
    fn adjacent_chunk_meshes_meet_on_the_shared_plane() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let left = marching_cubes_chunk(&field, ChunkIndex::new(0, 0)).expect("valid chunk");
        let right = marching_cubes_chunk(&field, ChunkIndex::new(1, 0)).expect("valid chunk");
        let boundary_x = f64_as_f32(ChunkIndex::edge_meters());

        let left_boundary = boundary_vertices(&left, boundary_x);
        let right_boundary = boundary_vertices(&right, boundary_x);
        assert!(!left_boundary.is_empty());
        assert_eq!(left_boundary, right_boundary);
    }

    #[test]
    fn chunk_mesh_has_a_golden_fingerprint() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let mesh = marching_cubes_chunk(&field, ChunkIndex::new(-3, 2)).expect("valid chunk");
        assert_eq!(
            mesh_fingerprint(&mesh),
            18_115_744_180_443_714_067,
            "changing this value changes generated terrain chunks"
        );
    }

    #[test]
    fn transvoxel_lods_are_repeatable_and_progressively_coarser() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
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
    }

    #[test]
    fn transvoxel_transition_matches_the_finer_boundary() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
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
        let boundary_x = f64_as_f32(coarse_chunk.sample_origin().x);

        let fine_boundary = boundary_positions(&fine, boundary_x);
        let coarse_boundary = boundary_positions(&coarse, boundary_x);
        assert!(!fine_boundary.is_empty());
        assert!(boundaries_match(&fine_boundary, &coarse_boundary));
    }

    #[test]
    fn transvoxel_rejects_lods_outside_the_streamed_range() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
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

    fn boundary_vertices(mesh: &Mesh, boundary_x: f32) -> BTreeSet<(u32, u32)> {
        mesh.positions
            .iter()
            .filter(|position| (position[0] - boundary_x).abs() < 0.000_1)
            .map(|position| (position[1].to_bits(), position[2].to_bits()))
            .collect()
    }

    fn boundary_positions(mesh: &Mesh, boundary_x: f32) -> Vec<[f32; 2]> {
        mesh.positions
            .iter()
            .filter(|position| (position[0] - boundary_x).abs() < 0.000_1)
            .map(|position| [position[1], position[2]])
            .collect()
    }

    fn boundaries_match(left: &[[f32; 2]], right: &[[f32; 2]]) -> bool {
        const EPSILON: f32 = 0.000_1;
        let contains = |haystack: &[[f32; 2]], needle: &[f32; 2]| {
            haystack.iter().any(|candidate| {
                (candidate[0] - needle[0]).abs() < EPSILON
                    && (candidate[1] - needle[1]).abs() < EPSILON
            })
        };
        left.iter().all(|position| contains(right, position))
            && right.iter().all(|position| contains(left, position))
    }

    fn mesh_fingerprint(mesh: &Mesh) -> u64 {
        let mut words = Vec::with_capacity(
            (mesh.positions.len() * 3) + (mesh.normals.len() * 3) + mesh.indices.len() + 3,
        );
        words.push(u64::try_from(mesh.positions.len()).expect("test mesh length fits u64"));
        words.push(u64::try_from(mesh.normals.len()).expect("test mesh length fits u64"));
        words.push(u64::try_from(mesh.indices.len()).expect("test mesh length fits u64"));
        words.extend(
            mesh.positions
                .iter()
                .flatten()
                .map(|component| u64::from(component.to_bits())),
        );
        words.extend(
            mesh.normals
                .iter()
                .flatten()
                .map(|component| u64::from(component.to_bits())),
        );
        words.extend(mesh.indices.iter().map(|&index| u64::from(index)));
        stable_hash(&words)
    }
}
