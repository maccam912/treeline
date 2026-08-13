//! The buffers a stand of trees is built into.
//!
//! Opaque wood and foliage share one surface list, so each tile batches into a
//! single renderable entity.

use crate::vertex::TerrainVertex;

#[derive(Debug, Default)]
pub(crate) struct TreeGeometry {
    pub(crate) vertices: Vec<TerrainVertex>,
    pub(crate) indices: Vec<u32>,
}
