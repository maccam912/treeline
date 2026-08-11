//! Turning tree individuals into geometry.
//!
//! A tree is drawn from its genotype rather than from a model library: trunk
//! taper and branch angles come from the individual. Detail levels drop
//! branches first, so a distant stand keeps its silhouette at a fraction of the
//! cost.
//!
//! Crowns are not drawn at all: foliage rendering is being rebuilt from
//! scratch, and [`append_crown`] is the blank it will be rebuilt into. What a
//! crown is made of still reaches it — the individual, where its trunk ends,
//! and how wide the stand measured its crown — so nothing on the data side had
//! to be kept alive artificially for it.

mod branch;
mod color;
mod geometry;
mod shape;
#[cfg(test)]
mod tests;

use glam::Vec3;
use treeline_ecology::{CrownShape, ProceduralTree, TreeCondition};

use crate::vertex::{f64_as_f32, translate_local_vertices};
use crate::{RendererError, TreeMeshDetail};
use branch::append_branches;
use color::{CylinderMaterial, bark_color, bark_cylinder_material};
use shape::{CylinderSpec, append_tapered_cylinder};

pub(crate) use geometry::TreeGeometry;

pub(crate) fn procedural_tree_geometry(
    trees: &[ProceduralTree],
    detail: TreeMeshDetail,
    mut surface_height: impl FnMut(f64, f64) -> Option<f64>,
) -> Result<TreeGeometry, RendererError> {
    let mut geometry = TreeGeometry::default();
    for tree in trees {
        let Some(base_y) = embedded_base_height(*tree, &mut surface_height) else {
            continue;
        };
        let first_vertex = geometry.vertices.len();
        append_tree(&mut geometry, *tree, detail, Vec3::ZERO)?;
        translate_local_vertices(
            &mut geometry.vertices[first_vertex..],
            [tree.x, base_y, tree.z],
        );
    }
    Ok(geometry)
}

/// Places the root below the lowest sampled ground around the trunk footprint.
///
/// The extra base-radius embed keeps the perpendicular end of a leaning or
/// fallen trunk below ground as well as covering terrain between samples.
fn embedded_base_height(
    tree: ProceduralTree,
    surface_height: &mut impl FnMut(f64, f64) -> Option<f64>,
) -> Option<f64> {
    let mut lowest = surface_height(tree.x, tree.z)?;
    let radius = tree.trunk_base_radius_meters;
    for [x, z] in BASE_SAMPLE_DIRECTIONS {
        if let Some(height) = surface_height(tree.x + (x * radius), tree.z + (z * radius)) {
            lowest = lowest.min(height);
        }
    }
    Some(lowest - radius)
}

const DIAGONAL_COMPONENT: f64 = std::f64::consts::FRAC_1_SQRT_2;
const BASE_SAMPLE_DIRECTIONS: [[f64; 2]; 8] = [
    [1.0, 0.0],
    [DIAGONAL_COMPONENT, DIAGONAL_COMPONENT],
    [0.0, 1.0],
    [-DIAGONAL_COMPONENT, DIAGONAL_COMPONENT],
    [-1.0, 0.0],
    [-DIAGONAL_COMPONENT, -DIAGONAL_COMPONENT],
    [0.0, -1.0],
    [DIAGONAL_COMPONENT, -DIAGONAL_COMPONENT],
];

pub(crate) fn append_tree(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    detail: TreeMeshDetail,
    base: Vec3,
) -> Result<(), RendererError> {
    let height = f64_as_f32(tree.height_meters);
    let lean = Vec3::new(
        f64_as_f32(tree.lean_direction[0]),
        0.0,
        f64_as_f32(tree.lean_direction[1]),
    );
    let trunk_vector = if tree.condition == TreeCondition::Fallen {
        (lean * height * f64_as_f32(tree.lean_fraction)) + (Vec3::Y * height * 0.08)
    } else {
        (lean * height * f64_as_f32(tree.lean_fraction)) + (Vec3::Y * height)
    };
    let top = base + trunk_vector;
    let trunk_radius = f64_as_f32(tree.trunk_base_radius_meters);
    let top_radius = (trunk_radius
        * (1.0 - (f64_as_f32(tree.genotype.trunk_taper_fraction) * 0.88)))
        .max(trunk_radius * 0.08);
    let trunk_sides = match detail {
        TreeMeshDetail::Full => 7,
        TreeMeshDetail::Simplified => 5,
        TreeMeshDetail::Silhouette => 3,
    };
    append_tapered_cylinder(
        geometry,
        &CylinderSpec {
            start: base,
            end: top,
            start_radius: trunk_radius,
            end_radius: top_radius,
            sides: trunk_sides,
            color: bark_color(tree),
            material: if detail == TreeMeshDetail::Silhouette {
                CylinderMaterial::UNTEXTURED
            } else {
                bark_cylinder_material(tree, 0)
            },
        },
    )?;

    let frame = TreeFrame {
        base,
        trunk_vector,
        trunk_radius,
    };
    // A sapling is a stem, and too small to carry branches at any tier.
    if tree.condition != TreeCondition::Sapling && detail == TreeMeshDetail::Full {
        append_branches(geometry, tree, frame)?;
    }
    append_crown(geometry, tree, frame, detail)
}

/// Where a crown starts on the trunk, as a fraction of its height.
fn crown_start(tree: ProceduralTree) -> f32 {
    match tree.genotype.crown_shape {
        CrownShape::Conical => 0.24,
        CrownShape::Columnar => 0.38,
        CrownShape::Rounded => 0.46,
    }
}

/// The trunk a crown and its branches hang off, in the tree's local space.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TreeFrame {
    base: Vec3,
    trunk_vector: Vec3,
    trunk_radius: f32,
}

/// The foliage a tree carries on its trunk. Draws nothing.
///
/// Foliage rendering is being written from scratch, and this is where it hooks
/// back in. Everything the old crowns were built from still arrives here: the
/// individual carries its crown radius, shape, and condition, and `frame` says
/// where its trunk runs.
///
/// The `Result` is what a crown will hand back the moment it appends anything:
/// every other geometry call here can outgrow `u32` addressing, and this one
/// will too.
#[allow(clippy::unnecessary_wraps)]
fn append_crown(
    _geometry: &mut TreeGeometry,
    _tree: ProceduralTree,
    _frame: TreeFrame,
    _detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    Ok(())
}
