//! The buffers a stand of trees is built into.
//!
//! Trunks and branches are triangles in one vertex format, so a tile's whole
//! stand uploads as a single mesh sharing one vertex buffer and one index list.
//! What a vertex is made of is carried by the surface kind it holds, not by
//! which buffer it landed in.
//!
//! When foliage returns it may need a list of its own — a surface that cuts
//! holes in itself cannot share a pipeline with one that never does — but a
//! second list that is always empty is worth less than the one it splits.

use crate::vertex::TerrainVertex;

#[derive(Debug, Default)]
pub(crate) struct TreeGeometry {
    pub(crate) vertices: Vec<TerrainVertex>,
    /// Trunks and branches: opaque, and never cut into.
    pub(crate) indices: Vec<u32>,
}
