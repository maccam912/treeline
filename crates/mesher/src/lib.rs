//! Turning terrain fields into triangles.
//!
//! Meshing is the boundary between the world's continuous description of itself
//! and something a GPU can draw. Two paths cover the two terrain tiers:
//! [`marching_cubes`] and [`transvoxel_chunk`] extract near terrain from signed
//! density, while [`surface_grid`] builds the distant height surface directly
//! from elevations.
//!
//! Everything here is a pure function of a field and a grid spec, which is what
//! lets meshing run on background workers in any order.

mod grid;
mod surface;
#[cfg(test)]
mod test_support;
mod volume;

pub use grid::{GridSpec, SurfaceGridSpec};
pub use surface::surface_grid;
pub use volume::{marching_cubes, marching_cubes_chunk, transvoxel_chunk};

use std::error::Error;
use std::fmt::{Display, Formatter};

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

impl Error for MeshingError {}

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
    use super::*;

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
    fn a_consistent_mesh_is_well_formed() {
        let mesh = Mesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]; 3],
            colors: Vec::new(),
            indices: vec![0, 1, 2],
        };
        assert!(mesh.is_well_formed());
    }

    #[test]
    fn mismatched_attribute_counts_are_rejected() {
        let mesh = Mesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]; 2],
            colors: Vec::new(),
            indices: vec![0, 1, 2],
        };
        assert!(!mesh.is_well_formed());
    }

    #[test]
    fn every_meshing_error_describes_itself() {
        for error in [
            MeshingError::InvalidGrid,
            MeshingError::GridTooLarge,
            MeshingError::MissingSurface,
            MeshingError::TooManyVertices,
            MeshingError::UnsupportedLod,
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
