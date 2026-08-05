//! Turning tree individuals into geometry.
//!
//! A tree is drawn from its genotype rather than from a model library: trunk
//! taper, branch angles, and crown shape all come from the individual. Detail
//! levels drop branches first, then thin the crown, so a distant stand keeps
//! its silhouette at a fraction of the cost.

mod branch;
mod color;
mod conifer;
mod geometry;
mod shape;
#[cfg(test)]
mod tests;

use glam::Vec3;
use treeline_ecology::{CrownShape, ProceduralTree, TreeCondition};

use crate::vertex::{f64_as_f32, translate_local_vertices};
use crate::{RendererError, TreeMeshDetail};
use branch::append_tree_crown;
use color::{
    CylinderMaterial, bark_color, bark_cylinder_material, foliage_color, tree_has_foliage,
};
use conifer::append_conifer_crown;
use shape::{CylinderSpec, append_octahedral_crown, append_tapered_cylinder};

pub(crate) use geometry::TreeGeometry;

pub(crate) fn procedural_tree_geometry(
    trees: &[ProceduralTree],
    detail: TreeMeshDetail,
    mut surface_height: impl FnMut(f64, f64) -> Option<f64>,
) -> Result<TreeGeometry, RendererError> {
    let mut geometry = TreeGeometry::default();
    for tree in trees {
        let Some(base_y) = surface_height(tree.x, tree.z) else {
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

    if tree.condition == TreeCondition::Sapling {
        return append_sapling_crown(geometry, tree, base, top);
    }

    let frame = TreeFrame {
        base,
        top,
        trunk_vector,
        trunk_radius,
    };
    if detail == TreeMeshDetail::Full {
        append_tree_crown(geometry, tree, frame, detail)
    } else if tree_has_foliage(tree) {
        append_terminal_crown(
            geometry,
            tree,
            frame,
            f64_as_f32(tree.crown_radius_meters),
            foliage_color(tree),
            detail,
        )
    } else {
        Ok(())
    }
}

/// Where a crown starts on the trunk, as a fraction of its height.
fn crown_start(tree: ProceduralTree) -> f32 {
    match tree.genotype.crown_shape {
        CrownShape::Conical => 0.24,
        CrownShape::Columnar => 0.38,
        CrownShape::Rounded => 0.46,
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TreeFrame {
    base: Vec3,
    top: Vec3,
    trunk_vector: Vec3,
    trunk_radius: f32,
}

/// The foliage mass a tree carries at the top of its trunk.
///
/// Conifers get whorls of branch skirts, which every detail tier fills the same
/// envelope with; broadleaves get one crown solid.
pub(crate) fn append_terminal_crown(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    frame: TreeFrame,
    crown_radius: f32,
    foliage: [f32; 4],
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    match tree.genotype.crown_shape {
        CrownShape::Conical => append_conifer_crown(
            geometry,
            tree,
            frame.base + (frame.trunk_vector * crown_start(tree)),
            frame.top + (Vec3::Y * crown_radius * 0.18),
            crown_radius,
            foliage,
            detail,
        ),
        CrownShape::Columnar | CrownShape::Rounded => append_octahedral_crown(
            geometry,
            frame.base + (frame.trunk_vector * 0.82),
            Vec3::new(
                crown_radius * 0.72,
                crown_radius
                    * if tree.genotype.crown_shape == CrownShape::Columnar {
                        1.25
                    } else {
                        0.82
                    },
                crown_radius * 0.72,
            ),
            foliage,
        ),
    }
}

pub(crate) fn append_sapling_crown(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    base: Vec3,
    top: Vec3,
) -> Result<(), RendererError> {
    if !tree_has_foliage(tree) {
        return Ok(());
    }
    let radius = f64_as_f32(tree.crown_radius_meters);
    if tree.genotype.crown_shape == CrownShape::Conical {
        append_conifer_crown(
            geometry,
            tree,
            base + ((top - base) * 0.36),
            top,
            radius,
            foliage_color(tree),
            TreeMeshDetail::Simplified,
        )
    } else {
        append_octahedral_crown(
            geometry,
            base + ((top - base) * 0.72),
            Vec3::new(radius, radius * 1.15, radius),
            foliage_color(tree),
        )
    }
}
