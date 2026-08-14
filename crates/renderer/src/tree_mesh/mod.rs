//! Turning tree individuals into geometry.
//!
//! A tree is drawn from its genotype rather than from a model library. Pine
//! crowns grow as separated branch whorls carrying elongated needle masses.
//! Broadleaves split into bent scaffold limbs carrying terminal leaf cloudlets,
//! so both distance tiers preserve air through each individual crown.

mod broadleaf;
mod color;
mod foliage;
mod geometry;
mod pine;
mod shape;
#[cfg(test)]
mod tests;

use glam::Vec3;
use treeline_ecology::{ProceduralTree, TreeCondition, TreeFunctionalGroup};

use crate::vertex::{f64_as_f32, translate_local_vertices};
use crate::{RendererError, TreeMeshDetail};
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
    let trunk_radius = f64_as_f32(tree.trunk_base_radius_meters);
    let trunk_top_radius = (trunk_radius
        * (1.0 - (f64_as_f32(tree.genotype.trunk_taper_fraction) * 0.88)))
        .max(trunk_radius * 0.08);
    let frame = TreeFrame {
        base,
        trunk_vector,
        trunk_radius,
        trunk_top_radius,
    };
    let trunk_end_fraction = match tree.genotype.functional_group {
        TreeFunctionalGroup::EvergreenNeedleleaf => 1.0,
        TreeFunctionalGroup::ColdDeciduous | TreeFunctionalGroup::TemperateBroadleaf => {
            broadleaf::trunk_end_fraction(tree, frame)
        }
    };
    let top = base + (trunk_vector * trunk_end_fraction);
    let top_radius = trunk_radius + ((trunk_top_radius - trunk_radius) * trunk_end_fraction);
    let trunk_sides = match detail {
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
    append_crown(geometry, tree, frame, detail)
}

/// The trunk a crown and its branches hang off, in the tree's local space.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TreeFrame {
    base: Vec3,
    trunk_vector: Vec3,
    trunk_radius: f32,
    trunk_top_radius: f32,
}

/// Draws the foliage grammar implemented for this individual's strategy.
fn append_crown(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    frame: TreeFrame,
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    match tree.genotype.functional_group {
        TreeFunctionalGroup::EvergreenNeedleleaf => {
            pine::append_pine_crown(geometry, tree, frame, detail)
        }
        TreeFunctionalGroup::ColdDeciduous | TreeFunctionalGroup::TemperateBroadleaf => {
            broadleaf::append_broadleaf_crown(geometry, tree, frame, detail)
        }
    }
}
