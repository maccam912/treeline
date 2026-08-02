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
