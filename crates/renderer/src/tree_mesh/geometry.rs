//! The buffers a stand of trees is built into.
//!
//! Trunks, branches, and crowns are all triangles in one vertex format, so a
//! tile's whole stand uploads as a single mesh sharing one vertex buffer. What
//! separates bark from foliage is the surface kind each vertex carries.
//!
//! Their indices are sorted apart all the same, into three lists that three
//! different passes want. Needle shells cut their own silhouette per fragment
//! and so cannot be depth-tested before they shade, while bark and wood can, so
//! the two cannot share a pipeline. And a ball's outermost shell encloses every
//! shell inside it, so the sun sees the same crown whether the inner ones are
//! drawn into a cascade or not — which makes four fifths of a crown's shadow
//! geometry work the shadow pass never needed to do.
//!
//! Sorting all of that out at build time costs two more `Vec`s and is paid for
//! once per tile rather than once per frame.

use crate::RendererError;
use crate::vertex::{TerrainVertex, usize_as_u32};

#[derive(Debug, Default)]
pub(crate) struct TreeGeometry {
    pub(crate) vertices: Vec<TerrainVertex>,
    /// Trunks, branches, and broadleaf crowns: opaque, and never cut into.
    pub(crate) indices: Vec<u32>,
    /// The outermost shell of every ball of needles — the one that bounds it,
    /// and so the only one worth casting a shadow with.
    pub(crate) foliage_hull_indices: Vec<u32>,
    /// The shells nested inside those, which are seen only through the gaps
    /// between the needles in front of them.
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

    /// Every index the stand draws, in the order the three lists upload in:
    /// opaque, then needle hulls, then the shells behind them.
    ///
    /// Each pass takes a run of that. The ground pipeline draws the first, the
    /// shadow cascades the first two, and the foliage pipeline the last two.
    pub(crate) fn all_indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.indices
            .iter()
            .chain(&self.foliage_hull_indices)
            .chain(&self.foliage_interior_indices)
            .copied()
    }
}
