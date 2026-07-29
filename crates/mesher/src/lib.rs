//! Marching Cubes extraction and mesh output shared with future Transvoxel LODs.

use std::error::Error;
use std::fmt::{Display, Formatter};

use mcubes::{MarchingCubes, MeshSide};
use transvoxel::prelude::{
    Block, FieldCaching, GenericMeshBuilder, TransitionSide, extract_from_field,
};
use treeline_coordinates::WorldPosition;
use treeline_terrain::{DensityField, SurfaceField};
use treeline_voxel::{ChunkFace, ChunkIndex, LodLevel, TransitionFaces};

/// Renderer-neutral indexed triangle mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// Absolute world-space positions. These remain double precision until the
    /// renderer splits them into camera-relative GPU coordinates.
    pub positions: Vec<[f64; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Optional RGBA vertex colors. Alpha blends from terrain shading to color.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_well_formed(&self) -> bool {
        let Ok(vertex_count) = u32::try_from(self.positions.len()) else {
            return false;
        };
        self.positions.len() == self.normals.len()
            && (self.colors.is_empty() || self.positions.len() == self.colors.len())
            && self.indices.len().is_multiple_of(3)
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

/// Horizontal rectangle whose cells are omitted from a surface mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceCutout {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
}

impl SurfaceCutout {
    pub const fn new(min_x: f64, max_x: f64, min_z: f64, max_z: f64) -> Self {
        Self {
            min_x,
            max_x,
            min_z,
            max_z,
        }
    }

    /// Returns whether an aligned surface cell is fully inside this cutout.
    pub fn contains_cell(self, min_x: f64, max_x: f64, min_z: f64, max_z: f64) -> bool {
        min_x >= self.min_x && max_x <= self.max_x && min_z >= self.min_z && max_z <= self.max_z
    }
}

/// Regular height-sample lattice used by the dedicated far representation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceGridSpec {
    pub origin_x: f64,
    pub origin_z: f64,
    pub cell_counts: [usize; 2],
    pub spacing_meters: f64,
    pub cutout: Option<SurfaceCutout>,
}

impl SurfaceGridSpec {
    pub const fn new(
        origin_x: f64,
        origin_z: f64,
        cell_counts: [usize; 2],
        spacing_meters: f64,
    ) -> Self {
        Self {
            origin_x,
            origin_z,
            cell_counts,
            spacing_meters,
            cutout: None,
        }
    }

    #[must_use]
    pub const fn with_cutout(mut self, cutout: SurfaceCutout) -> Self {
        self.cutout = Some(cutout);
        self
    }
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
    MissingSurface,
    TooManyVertices,
    UnsupportedLod,
}

impl Display for MeshingError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGrid => formatter.write_str("the sample grid is invalid"),
            Self::GridTooLarge => formatter.write_str("the sample grid is too large"),
            Self::MissingSurface => formatter.write_str("the terrain has no surface at a sample"),
            Self::TooManyVertices => formatter.write_str("the mesh exceeds u32 index capacity"),
            Self::UnsupportedLod => formatter.write_str("the chunk LOD is not supported"),
        }
    }
}

/// Triangulates a deterministic surface-height field without sampling a volume.
///
/// Vertex normals use central differences beyond the tile boundary, so
/// adjacent tiles share both positions and normals. Optional cutouts omit only
/// whole aligned cells and are used when near voxel terrain is resident.
///
/// # Errors
///
/// Returns [`MeshingError`] when the grid is invalid or too large, a surface
/// sample is unavailable, or the mesh exceeds `u32` index capacity.
pub fn surface_grid(
    field: &impl SurfaceField,
    spec: SurfaceGridSpec,
) -> Result<Mesh, MeshingError> {
    validate_surface_grid(spec)?;
    let [cells_x, cells_z] = spec.cell_counts;
    let count_x = cells_x.checked_add(1).ok_or(MeshingError::GridTooLarge)?;
    let count_z = cells_z.checked_add(1).ok_or(MeshingError::GridTooLarge)?;
    let vertex_count = count_x
        .checked_mul(count_z)
        .ok_or(MeshingError::GridTooLarge)?;
    if u32::try_from(vertex_count).is_err() {
        return Err(MeshingError::TooManyVertices);
    }

    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    for z in 0..count_z {
        let world_z = spec.origin_z + (index_as_f64(z) * spec.spacing_meters);
        for x in 0..count_x {
            let world_x = spec.origin_x + (index_as_f64(x) * spec.spacing_meters);
            let height = field
                .surface_height(world_x, world_z)
                .ok_or(MeshingError::MissingSurface)?;
            let low_x = field
                .surface_height(world_x - spec.spacing_meters, world_z)
                .ok_or(MeshingError::MissingSurface)?;
            let high_x = field
                .surface_height(world_x + spec.spacing_meters, world_z)
                .ok_or(MeshingError::MissingSurface)?;
            let low_z = field
                .surface_height(world_x, world_z - spec.spacing_meters)
                .ok_or(MeshingError::MissingSurface)?;
            let high_z = field
                .surface_height(world_x, world_z + spec.spacing_meters)
                .ok_or(MeshingError::MissingSurface)?;
            let normal = normalize([
                f64_as_f32(low_x - high_x),
                f64_as_f32(2.0 * spec.spacing_meters),
                f64_as_f32(low_z - high_z),
            ]);
            positions.push([world_x, height, world_z]);
            normals.push(normal);
        }
    }

    let index_capacity = cells_x
        .checked_mul(cells_z)
        .and_then(|cells| cells.checked_mul(6))
        .ok_or(MeshingError::GridTooLarge)?;
    let mut indices = Vec::with_capacity(index_capacity);
    for z in 0..cells_z {
        let min_z = spec.origin_z + (index_as_f64(z) * spec.spacing_meters);
        let max_z = min_z + spec.spacing_meters;
        for x in 0..cells_x {
            let min_x = spec.origin_x + (index_as_f64(x) * spec.spacing_meters);
            let max_x = min_x + spec.spacing_meters;
            if spec
                .cutout
                .is_some_and(|cutout| cutout.contains_cell(min_x, max_x, min_z, max_z))
            {
                continue;
            }
            let top_left = z
                .checked_mul(count_x)
                .and_then(|row| row.checked_add(x))
                .ok_or(MeshingError::GridTooLarge)?;
            let bottom_left = top_left
                .checked_add(count_x)
                .ok_or(MeshingError::GridTooLarge)?;
            let top_right = top_left.checked_add(1).ok_or(MeshingError::GridTooLarge)?;
            let bottom_right = bottom_left
                .checked_add(1)
                .ok_or(MeshingError::GridTooLarge)?;
            indices.extend([
                u32::try_from(top_left).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(bottom_left).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(top_right).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(top_right).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(bottom_left).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(bottom_right).map_err(|_| MeshingError::TooManyVertices)?,
            ]);
        }
    }

    Ok(Mesh {
        positions,
        normals,
        colors: Vec::new(),
        indices,
    })
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

fn validate_surface_grid(spec: SurfaceGridSpec) -> Result<(), MeshingError> {
    let cutout_is_valid = spec.cutout.is_none_or(|cutout| {
        cutout.min_x.is_finite()
            && cutout.max_x.is_finite()
            && cutout.min_z.is_finite()
            && cutout.max_z.is_finite()
            && cutout.min_x <= cutout.max_x
            && cutout.min_z <= cutout.max_z
    });
    if spec.cell_counts.contains(&0)
        || !spec.spacing_meters.is_finite()
        || spec.spacing_meters <= 0.0
        || !spec.origin_x.is_finite()
        || !spec.origin_z.is_finite()
        || !cutout_is_valid
    {
        return Err(MeshingError::InvalidGrid);
    }
    Ok(())
}

fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = libm::sqrtf(libm::fmaf(
        vector[0],
        vector[0],
        libm::fmaf(vector[1], vector[1], vector[2] * vector[2]),
    ));
    [vector[0] / length, vector[1] / length, vector[2] / length]
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: usize) -> f64 {
    index as f64
}

#[allow(clippy::cast_precision_loss)]
fn i64_as_f64(value: i64) -> f64 {
    value as f64
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
    fn far_world_surface_meshes_keep_submeter_vertex_spacing() {
        let field = GroundPlane {
            surface_height: 725.25,
            material: Material::Soil,
        };
        let mesh = surface_grid(
            &field,
            SurfaceGridSpec::new(5_000_000.0, -5_000_000.0, [2, 1], 0.125),
        )
        .expect("valid far-world surface grid");

        assert!((mesh.positions[1][0] - mesh.positions[0][0] - 0.125).abs() < f64::EPSILON);
        assert!((mesh.positions[3][2] - mesh.positions[0][2] - 0.125).abs() < f64::EPSILON);
    }

    #[test]
    fn malformed_triangle_indices_are_rejected() {
        let mesh = Mesh {
            positions: vec![[0.0; 3]],
            normals: vec![[0.0, 1.0, 0.0]],
            colors: Vec::new(),
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
                .all(|position| (position[1] - 0.25).abs() < f64::EPSILON)
        );
        assert!(mesh.normals.iter().all(|normal| normal[1] > 0.99));
        assert_front_facing(&mesh);
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
    fn surface_grid_is_repeatable_and_faces_upward() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let spec = SurfaceGridSpec::new(-64.0, 128.0, [8, 8], 8.0);
        let first = surface_grid(&field, spec).expect("valid surface grid");
        let second = surface_grid(&field, spec).expect("valid surface grid");

        assert_eq!(first, second);
        assert!(first.is_well_formed());
        assert_eq!(first.indices.len(), 8 * 8 * 6);
        assert!(first.normals.iter().all(|normal| normal[1] > 0.0));
        assert_front_facing(&first);
    }

    #[test]
    fn adjacent_surface_tiles_share_positions_and_normals() {
        let field = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let left = surface_grid(&field, SurfaceGridSpec::new(-64.0, 0.0, [8, 8], 8.0))
            .expect("left surface");
        let right = surface_grid(&field, SurfaceGridSpec::new(0.0, 0.0, [8, 8], 8.0))
            .expect("right surface");

        assert_eq!(
            surface_boundary_vertices(&left, 0.0),
            surface_boundary_vertices(&right, 0.0)
        );
    }

    #[test]
    fn aligned_surface_cutout_omits_only_covered_cells() {
        let field = GroundPlane {
            surface_height: 0.0,
            material: Material::Soil,
        };
        let full = surface_grid(&field, SurfaceGridSpec::new(0.0, 0.0, [4, 4], 8.0))
            .expect("full surface");
        let cut = surface_grid(
            &field,
            SurfaceGridSpec::new(0.0, 0.0, [4, 4], 8.0)
                .with_cutout(SurfaceCutout::new(8.0, 24.0, 8.0, 24.0)),
        )
        .expect("cut surface");

        assert_eq!(full.indices.len() - cut.indices.len(), 4 * 6);
        assert!(cut.is_well_formed());
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
        let boundary_x = ChunkIndex::edge_meters();

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
            8_730_301_632_951_197_344,
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
        let boundary_x = coarse_chunk.sample_origin().x;

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

    fn boundary_vertices(mesh: &Mesh, boundary_x: f64) -> BTreeSet<(u64, u64)> {
        mesh.positions
            .iter()
            .filter(|position| (position[0] - boundary_x).abs() < 0.000_1)
            .map(|position| (position[1].to_bits(), position[2].to_bits()))
            .collect()
    }

    fn surface_boundary_vertices(mesh: &Mesh, boundary_x: f64) -> BTreeSet<[u64; 6]> {
        mesh.positions
            .iter()
            .zip(&mesh.normals)
            .filter(|(position, _)| (position[0] - boundary_x).abs() < f64::EPSILON)
            .map(|(position, normal)| {
                [
                    position[0].to_bits(),
                    position[1].to_bits(),
                    position[2].to_bits(),
                    u64::from(normal[0].to_bits()),
                    u64::from(normal[1].to_bits()),
                    u64::from(normal[2].to_bits()),
                ]
            })
            .collect()
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

    fn assert_front_facing(mesh: &Mesh) {
        for (triangle_index, triangle) in mesh.indices.chunks_exact(3).enumerate() {
            let positions = [triangle[0], triangle[1], triangle[2]].map(|index| {
                mesh.positions[usize::try_from(index).expect("test index fits usize")]
            });
            let first = [
                positions[1][0] - positions[0][0],
                positions[1][1] - positions[0][1],
                positions[1][2] - positions[0][2],
            ];
            let second = [
                positions[2][0] - positions[0][0],
                positions[2][1] - positions[0][1],
                positions[2][2] - positions[0][2],
            ];
            let geometric_normal = [
                (first[1] * second[2]) - (first[2] * second[1]),
                (first[2] * second[0]) - (first[0] * second[2]),
                (first[0] * second[1]) - (first[1] * second[0]),
            ];
            if geometric_normal
                .iter()
                .map(|value| value * value)
                .sum::<f64>()
                <= f64::EPSILON
            {
                continue;
            }
            let vertex_normal = triangle.iter().fold([0.0; 3], |sum, &index| {
                let normal = mesh.normals[usize::try_from(index).expect("test index fits usize")];
                [sum[0] + normal[0], sum[1] + normal[1], sum[2] + normal[2]]
            });
            let agreement = geometric_normal
                .into_iter()
                .zip(vertex_normal)
                .map(|(geometric, vertex)| geometric * f64::from(vertex))
                .sum::<f64>();
            assert!(
                agreement > 0.0,
                "triangle {triangle_index} faces away from its vertex normals"
            );
        }
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
                .map(|component| component.to_bits()),
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
