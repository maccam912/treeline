//! Bevy-native rendering support for Treeline's measured world.
//!
//! World generation remains independent from the engine. This crate is the
//! adapter at the stable boundary: it turns finished terrain and tree data into
//! Bevy mesh assets, supplies the small set of materials the game uses, and
//! records the double-precision origin of each streamed mesh.

mod lighting;
mod mesh;
mod plugin;
mod snow;
mod tree_mesh;
mod vertex;

pub use lighting::{AtmosphereSettings, LightingSettings, TimeOfDay};
pub use mesh::{PreparedMesh, prepare_terrain_mesh, prepare_water_mesh};
pub use plugin::{TreelineRenderPlugin, WorldMaterials};

use std::error::Error;
use std::fmt::{Display, Formatter};

use bevy::prelude::Component;
use treeline_ecology::ProceduralTree;

/// Double-precision world location represented by a Bevy mesh entity.
///
/// Bevy transforms intentionally remain close to the camera. The client keeps
/// this authoritative origin and derives the entity's `f32` transform from the
/// current floating origin.
#[derive(Clone, Copy, Component, Debug, PartialEq)]
pub struct WorldMeshOrigin(pub [f64; 3]);

/// The only way mesh preparation fails: geometry too large for `u32` indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererError {
    TooManyIndices,
}

impl Display for RendererError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyIndices => formatter.write_str("the mesh has too many indices"),
        }
    }
}

impl Error for RendererError {}

/// Geometry detail for one deterministic set of procedural tree individuals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeMeshDetail {
    /// Branches carry one bent, faceted needle mass each.
    Simplified,
    /// The horizon keeps only separated layer masses per individual.
    Silhouette,
}

/// Builds the batched solid surface for a deterministic tile of tree
/// individuals.
///
/// # Errors
///
/// Returns [`RendererError::TooManyIndices`] when the generated tree geometry
/// cannot be represented by Bevy's `u32` index buffer.
pub fn prepare_trees(
    trees: &[ProceduralTree],
    detail: TreeMeshDetail,
    surface_height: impl FnMut(f64, f64) -> Option<f64>,
) -> Result<Option<PreparedMesh>, RendererError> {
    let geometry = tree_mesh::procedural_tree_geometry(trees, detail, surface_height)?;
    Ok(mesh::prepared_tree_mesh(geometry))
}
