//! The branches that reach out of a trunk.
//!
//! A branch is a tapered cylinder of bark, placed by the individual's branching
//! angle and density and shortened by whatever damage it carries. What hangs off
//! the end of one is foliage, and foliage is drawn nowhere at the moment — see
//! [`super::append_crown`].

use glam::Vec3;
use treeline_ecology::{CrownShape, ProceduralTree, TreeCondition};

use crate::RendererError;
use crate::tree_mesh::color::{bark_color, bark_cylinder_material};
use crate::tree_mesh::geometry::TreeGeometry;
use crate::tree_mesh::shape::{CylinderSpec, append_tapered_cylinder};
use crate::tree_mesh::{TreeFrame, crown_start};
use crate::vertex::{f64_as_f32, hash_lane, usize_as_f32};

pub(crate) fn append_branches(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    frame: TreeFrame,
) -> Result<(), RendererError> {
    let branch_count = branch_count(tree);
    for branch_index in 0..branch_count {
        append_tree_branch(geometry, tree, frame, branch_index, branch_count)?;
    }
    Ok(())
}

fn append_tree_branch(
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
    )
}

fn branch_count(tree: ProceduralTree) -> usize {
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
