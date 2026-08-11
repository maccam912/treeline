//! The sample lattices meshing runs over.
//!
//! Two shapes cover both terrain tiers: a three-dimensional lattice for
//! volumetric extraction, and a two-dimensional one for the distant height
//! surface. Both validate their own parameters, so no meshing routine has to
//! guard against a degenerate grid partway through.

use treeline_coordinates::WorldPosition;

use crate::MeshingError;

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

/// Regular height-sample lattice used by the dedicated far representation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceGridSpec {
    pub origin_x: f64,
    pub origin_z: f64,
    pub cell_counts: [usize; 2],
    pub spacing_meters: f64,
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
        }
    }
}

/// Rejects a volumetric grid that cannot produce a well-formed mesh.
pub fn validate_grid(spec: GridSpec) -> Result<(), MeshingError> {
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

pub fn validate_surface_grid(spec: SurfaceGridSpec) -> Result<(), MeshingError> {
    if spec.cell_counts.contains(&0)
        || !spec.spacing_meters.is_finite()
        || spec.spacing_meters <= 0.0
        || !spec.origin_x.is_finite()
        || !spec.origin_z.is_finite()
    {
        return Err(MeshingError::InvalidGrid);
    }
    Ok(())
}
