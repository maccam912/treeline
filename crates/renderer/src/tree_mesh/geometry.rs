//! The buffers a stand of trees is built into.
//!
//! Trunks, branches, and crowns are all triangles in one vertex format, so a
//! tile's whole stand uploads as a single mesh sharing one vertex buffer. What
//! separates bark from foliage is the surface kind each vertex carries.
//!
//! Their indices are sorted apart all the same, into two lists that different
//! passes want. Conifer crowns ray-march in the fragment shader and so cannot
//! be depth-tested before they shade, while bark and wood can, so the two
//! cannot share a pipeline. A crown volume is closed, so the sun sees the same
//! silhouette whether the interior is drawn into a cascade or not — which keeps
//! the shadow pass from ever rasterizing the inside of a crown.
//!
//! Sorting all of that out at build time costs one extra `Vec` and is paid for
//! once per tile rather than once per frame.

use crate::RendererError;
use crate::vertex::{TerrainVertex, usize_as_u32};

#[derive(Debug, Default)]
pub(crate) struct TreeGeometry {
    pub(crate) vertices: Vec<TerrainVertex>,
    /// Trunks, branches, and broadleaf crowns: opaque, and never cut into.
    pub(crate) indices: Vec<u32>,
    /// The one closed cone every conifer crown is drawn as — the crown's
    /// envelope. It bounds the crown, so it is all a shadow cascade needs.
    pub(crate) foliage_hull_indices: Vec<u32>,
    /// Kept empty: a crown is one volume, ray-marched, not a stack of shells.
    pub(crate) foliage_interior_indices: Vec<u32>,
}

impl TreeGeometry {
    /// The index the next vertex pushed will land on.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::TooManyIndices`] once a stand outgrows `u32`
    /// vertex addressing.
    pub(crate) fn base_index(&self) -> Result<u32, RendererError> {
        usize_as_u32(self.vertices.len())
    }

    /// Every index the stand draws, in the order the two lists upload in:
    /// opaque, then the crown volumes.
    ///
    /// Each pass takes a run of that. The ground pipeline draws the first, the
    /// shadow cascades the first two, and the foliage pipeline the last.
    pub(crate) fn all_indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.indices
            .iter()
            .chain(&self.foliage_hull_indices)
            .chain(&self.foliage_interior_indices)
            .copied()
    }
}
