//! Smooth-voxel sampling and level-of-detail alignment.

use treeline_coordinates::{CellIndex, WorldIdentity, WorldPosition};
use treeline_terrain::{DensityField, TerrainSample};

const DOMAIN_TERRAIN_CHUNK: u64 = 0x5445_5252_4348_554e;

/// A voxel resolution level. Level zero uses half-meter sample spacing.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LodLevel(u8);

impl LodLevel {
    pub const BASE_SPACING_METERS: f64 = 0.5;

    pub const fn new(level: u8) -> Self {
        Self(level)
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub const fn next_coarser(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(level) => Some(Self(level)),
            None => None,
        }
    }

    pub fn spacing_meters(self) -> f64 {
        Self::BASE_SPACING_METERS * libm::scalbn(1.0, i32::from(self.0))
    }

    /// Snaps a coordinate to this LOD's sample lattice.
    pub fn align(self, coordinate: f64) -> Option<f64> {
        if !coordinate.is_finite() {
            return None;
        }
        let spacing = self.spacing_meters();
        Some(libm::floor(coordinate / spacing) * spacing)
    }
}

/// Horizontal identity of a near-terrain chunk.
///
/// Chunks are columns rather than finite 3D bricks for now because the terrain
/// toy has a deliberately bounded vertical sampling range. The index remains
/// stable as streaming and rendering representations evolve.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkIndex {
    pub x: i64,
    pub z: i64,
}

impl ChunkIndex {
    /// Number of sample intervals along either horizontal chunk edge.
    pub const HORIZONTAL_CELLS: usize = 16;
    /// Number of sample intervals in the prototype's vertical range.
    pub const VERTICAL_CELLS: usize = 24;
    /// Finest sampling resolution currently used by streamed terrain.
    pub const NEAR_LOD: LodLevel = LodLevel::new(2);
    /// Coarsest volumetric resolution used before the dedicated far renderer.
    pub const MAX_LOD: LodLevel = LodLevel::new(4);
    /// Backward-compatible name for the finest streamed resolution.
    pub const LOD: LodLevel = Self::NEAR_LOD;
    /// Bottom of the prototype's vertical sampling range.
    pub const MIN_Y_METERS: f64 = -8.0;

    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    pub fn edge_meters() -> f64 {
        usize_as_f64(Self::HORIZONTAL_CELLS) * Self::NEAR_LOD.spacing_meters()
    }

    /// Resolves a finite world position into its half-open horizontal chunk.
    pub fn containing(position: WorldPosition) -> Option<Self> {
        let cell = CellIndex::containing(position.x, position.z, 0, Self::edge_meters())?;
        Some(Self::new(cell.x, cell.z))
    }

    /// World-space origin of this chunk's shared sample lattice.
    pub fn sample_origin(self) -> WorldPosition {
        let edge = Self::edge_meters();
        WorldPosition::new(
            i64_as_f64(self.x) * edge,
            Self::MIN_Y_METERS,
            i64_as_f64(self.z) * edge,
        )
    }

    /// Sample counts include both ends so adjacent chunks share a full plane.
    pub const fn sample_counts() -> [usize; 3] {
        [
            Self::HORIZONTAL_CELLS + 1,
            Self::VERTICAL_CELLS + 1,
            Self::HORIZONTAL_CELLS + 1,
        ]
    }

    /// Number of cubic Transvoxel cells along an edge at this resolution.
    ///
    /// All streamed LODs retain the same world-space footprint, so halving
    /// subdivisions doubles sample spacing and keeps every coarse sample on
    /// the finer lattice.
    pub const fn subdivisions(lod: LodLevel) -> Option<usize> {
        if lod.get() < Self::NEAR_LOD.get() || lod.get() > Self::MAX_LOD.get() {
            return None;
        }
        Some(Self::HORIZONTAL_CELLS >> (lod.get() - Self::NEAR_LOD.get()))
    }

    /// Stable generation identity independent of load or visitation order.
    pub fn generation_key(self, world: WorldIdentity) -> u64 {
        CellIndex::new(self.x, self.z, 0).generation_key(world, DOMAIN_TERRAIN_CHUNK)
    }

    /// Squared Chebyshev distance, matching square streaming neighborhoods.
    pub fn chebyshev_distance(self, other: Self) -> u64 {
        self.x.abs_diff(other.x).max(self.z.abs_diff(other.z))
    }
}

/// Horizontal chunk faces on which a coarse mesh can meet a finer neighbour
/// through a transition cell.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ChunkFace {
    LowX,
    HighX,
    LowZ,
    HighZ,
}

impl ChunkFace {
    pub const ALL: [Self; 4] = [Self::LowX, Self::HighX, Self::LowZ, Self::HighZ];

    pub const fn neighbour_offset(self) -> (i64, i64) {
        match self {
            Self::LowX => (-1, 0),
            Self::HighX => (1, 0),
            Self::LowZ => (0, -1),
            Self::HighZ => (0, 1),
        }
    }
}

/// Faces of a coarse chunk that need Transvoxel transition cells.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TransitionFaces(u8);

impl TransitionFaces {
    pub const fn none() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn with(self, face: ChunkFace) -> Self {
        Self(self.0 | face_bit(face))
    }

    pub const fn contains(self, face: ChunkFace) -> bool {
        self.0 & face_bit(face) != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

const fn face_bit(face: ChunkFace) -> u8 {
    match face {
        ChunkFace::LowX => 1 << 0,
        ChunkFace::HighX => 1 << 1,
        ChunkFace::LowZ => 1 << 2,
        ChunkFace::HighZ => 1 << 3,
    }
}

/// Samples a functional terrain field on an LOD-aligned lattice.
pub fn sample_aligned(
    field: &impl DensityField,
    position: WorldPosition,
    lod: LodLevel,
) -> Option<TerrainSample> {
    let aligned = WorldPosition::new(
        lod.align(position.x)?,
        lod.align(position.y)?,
        lod.align(position.z)?,
    );
    Some(field.sample(aligned))
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[allow(clippy::cast_precision_loss)]
fn i64_as_f64(value: i64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_coordinates::WorldIdentity;
    use treeline_terrain::{GroundPlane, Material};

    #[test]
    fn spacing_doubles_at_each_lod() {
        for level in 0..8 {
            let current = LodLevel::new(level).spacing_meters();
            let next = LodLevel::new(level + 1).spacing_meters();
            assert!((next - (current * 2.0)).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn coarse_samples_align_with_the_fine_lattice() {
        let position = 13.37;
        let coarse = LodLevel::new(4).align(position).expect("finite");
        let fine = LodLevel::new(0).align(coarse).expect("finite");
        assert!((coarse - fine).abs() < f64::EPSILON);
    }

    #[test]
    fn negative_coordinates_align_downward() {
        let aligned = LodLevel::new(1).align(-0.1).expect("finite");
        assert!((aligned - -1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn aligned_sampling_is_repeatable() {
        let ground = GroundPlane {
            surface_height: 10.0,
            material: Material::Soil,
        };
        let position = WorldPosition::new(0.2, 10.7, -0.2);
        assert_eq!(
            sample_aligned(&ground, position, LodLevel::new(1)),
            sample_aligned(&ground, position, LodLevel::new(1))
        );
    }

    #[test]
    fn negative_chunk_boundaries_are_half_open() {
        assert_eq!(
            ChunkIndex::containing(WorldPosition::new(-0.01, 0.0, -32.0)),
            Some(ChunkIndex::new(-1, -1))
        );
        assert_eq!(
            ChunkIndex::containing(WorldPosition::new(32.0, 0.0, 31.99)),
            Some(ChunkIndex::new(1, 0))
        );
    }

    #[test]
    fn adjacent_chunks_share_the_same_sample_plane() {
        let left = ChunkIndex::new(-1, 4);
        let right = ChunkIndex::new(0, 4);
        let left_origin = left.sample_origin();
        let right_origin = right.sample_origin();
        let left_max_x = left_origin.x
            + (usize_as_f64(ChunkIndex::HORIZONTAL_CELLS) * ChunkIndex::LOD.spacing_meters());
        assert!((left_max_x - right_origin.x).abs() < f64::EPSILON);
        assert!((left_origin.z - right_origin.z).abs() < f64::EPSILON);
    }

    #[test]
    fn streamed_lods_keep_the_same_footprint_and_nested_lattices() {
        assert_eq!(ChunkIndex::subdivisions(LodLevel::new(2)), Some(16));
        assert_eq!(ChunkIndex::subdivisions(LodLevel::new(3)), Some(8));
        assert_eq!(ChunkIndex::subdivisions(LodLevel::new(4)), Some(4));
        for level in 2..=4 {
            let lod = LodLevel::new(level);
            let subdivisions = ChunkIndex::subdivisions(lod).expect("streamed LOD");
            let edge = usize_as_f64(subdivisions) * lod.spacing_meters();
            assert!((edge - ChunkIndex::edge_meters()).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn transition_faces_form_a_small_deterministic_set() {
        let faces = TransitionFaces::none()
            .with(ChunkFace::LowX)
            .with(ChunkFace::HighZ);
        assert!(faces.contains(ChunkFace::LowX));
        assert!(faces.contains(ChunkFace::HighZ));
        assert!(!faces.contains(ChunkFace::HighX));
        assert!(!faces.is_empty());
    }

    #[test]
    fn chunk_generation_keys_are_stable_and_spatially_distinct() {
        let world = WorldIdentity::new(0x5eed, 1, 0);
        let chunk = ChunkIndex::new(-7, 11);
        assert_eq!(chunk.generation_key(world), chunk.generation_key(world));
        assert_ne!(
            chunk.generation_key(world),
            ChunkIndex::new(-6, 11).generation_key(world)
        );
    }
}
