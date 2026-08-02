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
    CylinderMaterial, bark_color, bark_cylinder_material, foliage_color, puff_color,
    tree_has_foliage,
};
use shape::{CylinderSpec, append_needle_puff, append_octahedral_crown, append_tapered_cylinder};

#[cfg(test)]
use crate::vertex::SURFACE_KIND_NEEDLE_FOLIAGE;

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
        append_tree_crown(vertices, indices, tree, frame, detail)
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
            detail,
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
    detail: TreeMeshDetail,
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
        detail,
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

/// A conifer crown as a cloud of crossed-quad needle puffs.
///
/// Puffs spiral up the same envelope the old cone used, so every detail tier
/// keeps the crown silhouette aligned. Puff count scales with the genotype's
/// combined branch and leaf density; quad size stays constant so coarser tiers
/// never poke past the fuller one.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_needle_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    crown_base: Vec3,
    apex: Vec3,
    crown_radius: f32,
    foliage: [f32; 4],
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    let axis = apex - crown_base;
    let axis_length = axis.length();
    if axis_length <= f32::EPSILON {
        return Ok(());
    }
    let direction = axis / axis_length;
    let density =
        f64_as_f32(tree.genotype.branch_density_fraction * tree.genotype.leaf_density_fraction);
    let (planes, base_count) = match detail {
        TreeMeshDetail::Full => (3, 12.0 + density * 12.0),
        TreeMeshDetail::Simplified => (2, 6.0 + density * 6.0),
        TreeMeshDetail::Silhouette => (2, 4.0 + density * 3.0),
    };
    let count = usize::try_from(libm::roundf(base_count.clamp(1.0, 48.0)) as i32)
        .expect("puff count fits usize");
    let half_extent =
        crown_radius * (0.15 + f64_as_f32(tree.genotype.leaf_density_fraction) * 0.10);
    let reference = if direction.y.abs() < 0.92 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = direction.cross(reference).normalize_or_zero();
    let bitangent = direction.cross(tangent).normalize_or_zero();
    let golden = 0.618_034_f32;
    for index in 0..count {
        let t = usize_as_f32(index + 1) / usize_as_f32(count + 1);
        let azimuth = (golden * usize_as_f32(index) + hash_lane(tree.id, index) * 0.5)
            * std::f32::consts::TAU;
        let radial = (tangent * libm::cosf(azimuth)) + (bitangent * libm::sinf(azimuth));
        let envelope_radius =
            crown_radius * (1.0 - t) * (0.55 + hash_lane(tree.id, index + 8) * 0.45);
        let position = (crown_base + (direction * (axis_length * t))) + (radial * envelope_radius);
        let rotation = hash_lane(tree.id, index + 16) * std::f32::consts::TAU;
        append_needle_puff(
            vertices,
            indices,
            position,
            half_extent,
            planes,
            rotation,
            puff_color(tree, foliage, index),
        )?;
    }
    append_needle_puff(
        vertices,
        indices,
        apex,
        half_extent * 0.7,
        planes,
        hash_lane(tree.id, 31) * std::f32::consts::TAU,
        puff_color(tree, foliage, count),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_terminal_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
    crown_start: f32,
    crown_radius: f32,
    foliage: [f32; 4],
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    match tree.genotype.crown_shape {
        CrownShape::Conical => append_needle_crown(
            vertices,
            indices,
            tree,
            frame.base + (frame.trunk_vector * crown_start),
            frame.top + (Vec3::Y * crown_radius * 0.18),
            crown_radius,
            foliage,
            detail,
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
        append_needle_crown(
            vertices,
            indices,
            tree,
            base + ((top - base) * 0.36),
            top,
            radius,
            foliage_color(tree),
            TreeMeshDetail::Simplified,
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
    fn a_needle_puff_builds_crossed_front_facing_quads() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        append_needle_puff(
            &mut vertices,
            &mut indices,
            Vec3::ZERO,
            1.0,
            2,
            0.0,
            [0.3, 0.5, 0.3, 1.0],
        )
        .expect("puff geometry");
        assert_eq!(vertices.len(), 2 * 4);
        assert_eq!(indices.len(), 2 * 2 * 3);
        assert_well_formed(&vertices, &indices);
        assert!(
            vertices
                .iter()
                .all(|vertex| vertex.surface_kind == SURFACE_KIND_NEEDLE_FOLIAGE)
        );
    }

    fn conifer() -> ProceduralTree {
        stand()
            .into_iter()
            .find(|tree| tree.genotype.crown_shape == CrownShape::Conical)
            .expect("a conifer in the mixture")
    }

    fn sapling() -> ProceduralTree {
        (0..10_000_u64)
            .map(tree)
            .find(|tree| {
                tree.condition == TreeCondition::Sapling
                    && tree.genotype.crown_shape == CrownShape::Conical
            })
            .expect("a conifer sapling in the population")
    }

    /// Every tier places its puffs inside the same cone envelope, so distant
    /// crowns stay spatially aligned with near ones.
    #[test]
    fn needle_puffs_stay_within_the_crown_envelope_at_every_tier() {
        let tree = conifer();
        let crown_base = Vec3::ZERO;
        let apex = Vec3::new(0.0, 20.0, 0.0);
        let crown_radius = 4.0;
        let half_extent =
            crown_radius * (0.15 + f64_as_f32(tree.genotype.leaf_density_fraction) * 0.10);
        let margin = half_extent * 1.6 + 0.05;
        let axis = apex - crown_base;
        let axis_length = axis.length();
        let axis_dir = axis / axis_length;
        let t_margin = margin / axis_length;
        for detail in [
            TreeMeshDetail::Full,
            TreeMeshDetail::Simplified,
            TreeMeshDetail::Silhouette,
        ] {
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            append_needle_crown(
                &mut vertices,
                &mut indices,
                tree,
                crown_base,
                apex,
                crown_radius,
                [0.3, 0.5, 0.3, 1.0],
                detail,
            )
            .expect("crown geometry");
            assert!(!vertices.is_empty(), "{detail:?} produced no puffs");
            for vertex in &vertices {
                let position = Vec3::new(
                    vertex.position_high[0] + vertex.position_low[0],
                    vertex.position_high[1] + vertex.position_low[1],
                    vertex.position_high[2] + vertex.position_low[2],
                );
                let relative = position - crown_base;
                let along = relative.dot(axis_dir);
                let t = along / axis_length;
                assert!(
                    t > -0.01 - t_margin && t < 1.01 + t_margin,
                    "{detail:?} puff at t={t}"
                );
                let lateral = relative - (axis_dir * along);
                let distance = lateral.length();
                let envelope = crown_radius * (1.0 - t).max(0.0);
                assert!(
                    distance <= envelope + margin,
                    "{detail:?} puff escaped the envelope: {distance} > {envelope} + {margin}"
                );
            }
        }
    }

    #[test]
    fn every_tier_keeps_needle_puffs() {
        let stand = stand();
        for detail in [
            TreeMeshDetail::Full,
            TreeMeshDetail::Simplified,
            TreeMeshDetail::Silhouette,
        ] {
            let (vertices, _) =
                procedural_tree_geometry(&stand, detail, |_, _| Some(42.0)).expect("tree geometry");
            assert!(
                vertices
                    .iter()
                    .any(|vertex| vertex.surface_kind == SURFACE_KIND_NEEDLE_FOLIAGE),
                "{detail:?} lost its needle puffs"
            );
        }
    }

    #[test]
    fn saplings_render_needle_puffs() {
        let (vertices, _) =
            procedural_tree_geometry(&[sapling()], TreeMeshDetail::Full, |_, _| Some(42.0))
                .expect("tree geometry");
        assert!(
            vertices
                .iter()
                .any(|vertex| vertex.surface_kind == SURFACE_KIND_NEEDLE_FOLIAGE)
        );
    }

    /// Geometry must be bit-stable for one input and identical whether trees
    /// are meshed together or one at a time.
    #[test]
    fn a_trees_geometry_is_bit_stable_and_neighbor_independent() {
        let stand = stand();
        let batch = procedural_tree_geometry(&stand, TreeMeshDetail::Full, |_, _| Some(42.0))
            .expect("tree geometry");
        let again = procedural_tree_geometry(&stand, TreeMeshDetail::Full, |_, _| Some(42.0))
            .expect("tree geometry");
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&batch.0),
            bytemuck::cast_slice::<_, u8>(&again.0)
        );
        assert_eq!(batch.1, again.1);

        let mut concatenated_vertices = Vec::new();
        let mut concatenated_indices = Vec::new();
        for tree in &stand {
            let (mut vertices, mut indices) =
                procedural_tree_geometry(&[*tree], TreeMeshDetail::Full, |_, _| Some(42.0))
                    .expect("tree geometry");
            let base = u32::try_from(concatenated_vertices.len()).expect("vertex count fits u32");
            for index in &mut indices {
                *index += base;
            }
            concatenated_vertices.append(&mut vertices);
            concatenated_indices.append(&mut indices);
        }
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&batch.0),
            bytemuck::cast_slice::<_, u8>(&concatenated_vertices)
        );
        assert_eq!(batch.1, concatenated_indices);
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
