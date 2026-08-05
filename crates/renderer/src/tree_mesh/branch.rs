//! Branches, and the crowns that hang off them.
//!
//! A broadleaf's crown is its branches: clusters of foliage on the ends of
//! cylinders that reach out of the trunk. A conifer's is not — its whorls are
//! an opaque skirt, and every branch drawn inside one is triangles nobody sees.

use glam::Vec3;
use treeline_ecology::{CrownShape, ProceduralTree, TreeCondition};

use crate::tree_mesh::color::{
    bark_color, bark_cylinder_material, foliage_color, tree_has_foliage,
};
use crate::tree_mesh::geometry::TreeGeometry;
use crate::tree_mesh::shape::{CylinderSpec, append_octahedral_crown, append_tapered_cylinder};
use crate::tree_mesh::{TreeFrame, append_terminal_crown, crown_start};
use crate::vertex::{f64_as_f32, hash_lane, usize_as_f32};
use crate::{RendererError, TreeMeshDetail};

/// Whether a tree's branches are worth drawing.
///
/// A living conifer's whorls are its crown, and the crown is opaque, so every
/// branch cylinder inside it would be triangles nobody ever sees. A dead one
/// has nothing to hide behind and is mostly branches.
fn branches_are_visible(tree: ProceduralTree) -> bool {
    tree.genotype.crown_shape != CrownShape::Conical || !tree_has_foliage(tree)
}

pub(crate) fn append_tree_crown(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    frame: TreeFrame,
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    if branches_are_visible(tree) {
        let branch_count = branch_count(tree);
        for branch_index in 0..branch_count {
            append_tree_branch(geometry, tree, frame, branch_index, branch_count)?;
        }
    }

    if !tree_has_foliage(tree) {
        return Ok(());
    }
    append_terminal_crown(
        geometry,
        tree,
        frame,
        f64_as_f32(tree.crown_radius_meters),
        foliage_color(tree),
        detail,
    )
}

pub(crate) fn append_tree_branch(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    frame: TreeFrame,
    branch_index: usize,
    branch_count: usize,
) -> Result<(), RendererError> {
    let ordinal = usize_as_f32(branch_index);
    let count = usize_as_f32(branch_count);
    let start_fraction = crown_start(tree);
    let branch_fraction =
        start_fraction + ((ordinal + 1.0) / (count + 1.0) * (0.88 - start_fraction));
    let start = frame.base + (frame.trunk_vector * branch_fraction);
    let turn = f64_as_f32(tree.rotation_turns)
        + (ordinal * 0.618_034)
        + (hash_lane(tree.id, branch_index) * 0.16);
    let (azimuth_sine, azimuth_cosine) = libm::sincosf(turn * std::f32::consts::TAU);
    let horizontal = Vec3::new(azimuth_cosine, 0.0, azimuth_sine);
    let branch_angle = f64_as_f32(tree.genotype.branching_angle_radians);
    let direction = (horizontal * libm::sinf(branch_angle)) + (Vec3::Y * libm::cosf(branch_angle));
    let height_taper = 1.0 - (branch_fraction * 0.52);
    let shape_scale = match tree.genotype.crown_shape {
        CrownShape::Conical => height_taper,
        CrownShape::Columnar => 0.58 + (height_taper * 0.20),
        CrownShape::Rounded => 0.76 + (height_taper * 0.16),
    };
    let damage_scale = 1.0 - (f64_as_f32(tree.damage_fraction) * 0.48);
    let crown_radius = f64_as_f32(tree.crown_radius_meters);
    let length = crown_radius * shape_scale * damage_scale;
    let end = start + (direction.normalize_or_zero() * length);
    append_tapered_cylinder(
        geometry,
        &CylinderSpec {
            start,
            end,
            start_radius: frame.trunk_radius * (0.20 * height_taper).max(0.07),
            end_radius: frame.trunk_radius * 0.045,
            sides: 4,
            color: bark_color(tree),
            material: bark_cylinder_material(tree, branch_index + 1),
        },
    )?;
    if tree_has_foliage(tree) && tree.genotype.crown_shape != CrownShape::Conical {
        let cluster_radius = crown_radius
            * (0.22 + (f64_as_f32(tree.genotype.leaf_density_fraction) * 0.16))
            * damage_scale;
        let vertical_scale = match tree.genotype.crown_shape {
            CrownShape::Columnar => 1.35,
            CrownShape::Conical | CrownShape::Rounded => 1.0,
        };
        append_octahedral_crown(
            geometry,
            end,
            Vec3::new(
                cluster_radius,
                cluster_radius * vertical_scale,
                cluster_radius,
            ),
            foliage_color(tree),
        )?;
    }
    Ok(())
}

pub(crate) fn branch_count(tree: ProceduralTree) -> usize {
    let density = tree.genotype.branch_density_fraction * (1.0 - (tree.damage_fraction * 0.58));
    let mut count = 4_usize;
    for threshold in [0.18, 0.32, 0.46, 0.60, 0.74, 0.88] {
        if density >= threshold {
            count += 1;
        }
    }
    if tree.condition == TreeCondition::StormBroken {
        count.saturating_sub(2)
    } else {
        count
    }
}
