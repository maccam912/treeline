//! Turning tree individuals into geometry.
//!
//! A tree is drawn from its genotype rather than from a model library: trunk
//! taper, branch angles, and crown shape all come from the individual. Detail
//! levels drop branches first, then crown clusters, so a distant stand keeps
//! its silhouette at a fraction of the vertex cost.

mod color;
mod shape;

use glam::Vec3;
use treeline_ecology::{CrownShape, ProceduralTree, TreeCondition};

use crate::vertex::{TerrainVertex, f64_as_f32, hash_lane, translate_local_vertices, usize_as_f32};
use crate::{RendererError, TreeMeshDetail};
use color::{
    CylinderMaterial, bark_color, bark_cylinder_material, foliage_color, tree_has_foliage,
};
use shape::{CylinderSpec, append_conical_crown, append_octahedral_crown, append_tapered_cylinder};

pub(crate) fn procedural_tree_geometry(
    trees: &[ProceduralTree],
    detail: TreeMeshDetail,
    mut surface_height: impl FnMut(f64, f64) -> Option<f64>,
) -> Result<(Vec<TerrainVertex>, Vec<u32>), RendererError> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for tree in trees {
        let Some(base_y) = surface_height(tree.x, tree.z) else {
            continue;
        };
        let first_vertex = vertices.len();
        append_tree(&mut vertices, &mut indices, *tree, detail, Vec3::ZERO)?;
        translate_local_vertices(&mut vertices[first_vertex..], [tree.x, base_y, tree.z]);
    }
    Ok((vertices, indices))
}

pub(crate) fn append_tree(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
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
        vertices,
        indices,
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
        append_sapling_crown(vertices, indices, tree, base, top)?;
        return Ok(());
    }

    let frame = TreeFrame {
        base,
        top,
        trunk_vector,
        trunk_radius,
    };
    if detail == TreeMeshDetail::Full {
        append_tree_crown(vertices, indices, tree, frame)
    } else if tree_has_foliage(tree) {
        let crown_start = match tree.genotype.crown_shape {
            CrownShape::Conical => 0.24,
            CrownShape::Columnar => 0.38,
            CrownShape::Rounded => 0.46,
        };
        append_terminal_crown(
            vertices,
            indices,
            tree,
            frame,
            crown_start,
            f64_as_f32(tree.crown_radius_meters),
            foliage_color(tree),
        )
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TreeFrame {
    base: Vec3,
    top: Vec3,
    trunk_vector: Vec3,
    trunk_radius: f32,
}

pub(crate) fn append_tree_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
) -> Result<(), RendererError> {
    let branch_count = branch_count(tree);
    let crown_start = match tree.genotype.crown_shape {
        CrownShape::Conical => 0.24,
        CrownShape::Columnar => 0.38,
        CrownShape::Rounded => 0.46,
    };
    let crown_radius = f64_as_f32(tree.crown_radius_meters);
    let foliage = foliage_color(tree);
    for branch_index in 0..branch_count {
        append_tree_branch(
            vertices,
            indices,
            tree,
            frame,
            crown_start,
            branch_index,
            branch_count,
        )?;
    }

    if !tree_has_foliage(tree) {
        return Ok(());
    }
    append_terminal_crown(
        vertices,
        indices,
        tree,
        frame,
        crown_start,
        crown_radius,
        foliage,
    )
}

pub(crate) fn append_tree_branch(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
    crown_start: f32,
    branch_index: usize,
    branch_count: usize,
) -> Result<(), RendererError> {
    let ordinal = usize_as_f32(branch_index);
    let count = usize_as_f32(branch_count);
    let branch_fraction = crown_start + ((ordinal + 1.0) / (count + 1.0) * (0.88 - crown_start));
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
        vertices,
        indices,
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
            vertices,
            indices,
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

pub(crate) fn append_terminal_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
    crown_start: f32,
    crown_radius: f32,
    foliage: [f32; 4],
) -> Result<(), RendererError> {
    match tree.genotype.crown_shape {
        CrownShape::Conical => append_conical_crown(
            vertices,
            indices,
            frame.base + (frame.trunk_vector * crown_start),
            frame.top + (Vec3::Y * crown_radius * 0.18),
            crown_radius,
            foliage,
        ),
        CrownShape::Columnar | CrownShape::Rounded => append_octahedral_crown(
            vertices,
            indices,
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
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    base: Vec3,
    top: Vec3,
) -> Result<(), RendererError> {
    if !tree_has_foliage(tree) {
        return Ok(());
    }
    let radius = f64_as_f32(tree.crown_radius_meters);
    if tree.genotype.crown_shape == CrownShape::Conical {
        append_conical_crown(
            vertices,
            indices,
            base + ((top - base) * 0.36),
            top,
            radius,
            foliage_color(tree),
        )
    } else {
        append_octahedral_crown(
            vertices,
            indices,
            base + ((top - base) * 0.72),
            Vec3::new(radius, radius * 1.15, radius),
            foliage_color(tree),
        )
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_ecology::{
        BarkStyle, ForestComposition, GrowthConditions, Stand, TreeFunctionalGroup, grow_tree,
    };

    fn tree(id: u64) -> ProceduralTree {
        grow_tree(
            id,
            f64::from(u32::try_from(id).expect("small fixture id")) * 8.0,
            -4.0,
            GrowthConditions {
                stand: Stand::measured(0.8, 24.0).expect("measured stand"),
                composition: ForestComposition::SURVEYED_TILE,
                prevailing_wind: [0.8, 0.6],
            },
        )
    }

    fn stand() -> Vec<ProceduralTree> {
        (1..=8).map(tree).collect()
    }

    fn assert_well_formed(vertices: &[TerrainVertex], indices: &[u32]) {
        assert!(!vertices.is_empty());
        assert!(!indices.is_empty());
        assert!(indices.len().is_multiple_of(3));
        assert!(
            indices
                .iter()
                .all(|&index| usize::try_from(index).is_ok_and(|index| index < vertices.len()))
        );
        assert!(vertices.iter().all(|vertex| {
            vertex.position_high.into_iter().all(f32::is_finite)
                && vertex.position_low.into_iter().all(f32::is_finite)
                && vertex.normal.into_iter().all(f32::is_finite)
                && (vertex.color[3] - 1.0).abs() < f32::EPSILON
        }));
    }

    #[test]
    fn a_stand_builds_well_formed_colored_geometry() {
        let (vertices, indices) =
            procedural_tree_geometry(&stand(), TreeMeshDetail::Full, |x, z| Some((x + z) * 0.01))
                .expect("tree geometry");
        assert_well_formed(&vertices, &indices);
    }

    #[test]
    fn trees_without_a_surface_sample_are_skipped() {
        let (vertices, indices) =
            procedural_tree_geometry(&stand(), TreeMeshDetail::Full, |_, _| None)
                .expect("tree geometry");
        assert!(vertices.is_empty());
        assert!(indices.is_empty());
    }

    /// Coarser tiers must keep every individual and only shed geometry, so a
    /// distant stand thins rather than losing trees.
    #[test]
    fn coarser_detail_sheds_geometry_without_dropping_trees() {
        let stand = stand();
        let tiers = [
            TreeMeshDetail::Full,
            TreeMeshDetail::Simplified,
            TreeMeshDetail::Silhouette,
        ]
        .map(|detail| {
            procedural_tree_geometry(&stand, detail, |_, _| Some(42.0)).expect("tree geometry")
        });

        for (vertices, indices) in &tiers {
            assert_well_formed(vertices, indices);
        }
        assert!(tiers[0].0.len() > tiers[1].0.len());
        assert!(tiers[1].0.len() > tiers[2].0.len());
        assert!(tiers[0].1.len() > tiers[1].1.len());
        assert!(tiers[1].1.len() > tiers[2].1.len());
    }

    #[test]
    fn conifers_and_broadleaves_sample_different_bark_textures() {
        let stand = stand();
        let conifer = stand
            .iter()
            .find(|tree| tree.genotype.functional_group == TreeFunctionalGroup::EvergreenNeedleleaf)
            .expect("a conifer in the mixture");
        let broadleaf = stand
            .iter()
            .find(|tree| tree.genotype.functional_group != TreeFunctionalGroup::EvergreenNeedleleaf)
            .expect("a broadleaf in the mixture");

        assert_eq!(conifer.genotype.bark_style, BarkStyle::Scaly);
        assert_ne!(
            bark_cylinder_material(*conifer, 0).surface_kind.to_bits(),
            bark_cylinder_material(*broadleaf, 0).surface_kind.to_bits()
        );
    }
}
