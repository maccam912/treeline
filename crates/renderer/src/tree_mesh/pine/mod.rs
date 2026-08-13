//! Pine crowns built from branch whorls and distance-scaled needle masses.

mod architecture;

use glam::Vec3;
use treeline_ecology::{ProceduralTree, TreeCondition};

use crate::tree_mesh::TreeFrame;
use crate::tree_mesh::color::pine_foliage_colors;
use crate::tree_mesh::foliage::{BoughSpec, LayerMassSpec, append_bough, append_layer_mass};
use crate::tree_mesh::geometry::TreeGeometry;
use crate::vertex::{f64_as_f32, hash_fraction};
use crate::{RendererError, TreeMeshDetail};
use architecture::PineCrown;

pub(super) fn append_pine_crown(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    frame: TreeFrame,
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    if matches!(
        tree.condition,
        TreeCondition::DeadStanding | TreeCondition::Fallen
    ) {
        return Ok(());
    }
    let Some(crown) = PineCrown::new(tree, frame) else {
        return Ok(());
    };
    match detail {
        TreeMeshDetail::Simplified => append_branch_boughs(geometry, &crown)?,
        TreeMeshDetail::Silhouette => append_silhouette_layers(geometry, &crown)?,
    }
    append_leader(geometry, &crown, detail)
}

/// One bent, faceted needle mass per branch arm, the scale and shape unresolved
/// needles collapse into at the simplified tier.
fn append_branch_boughs(
    geometry: &mut TreeGeometry,
    crown: &PineCrown,
) -> Result<(), RendererError> {
    for layer_index in 0..crown.layer_count {
        let layer = crown.layer(layer_index);
        for branch_index in 0..crown.branch_count(layer) {
            let Some(arm) = crown.arm(layer, branch_index) else {
                continue;
            };
            let [inner_color, outer_color] = pine_foliage_colors(crown.tree, arm.seed);
            append_bough(
                geometry,
                &BoughSpec {
                    start: arm.foliage_start,
                    end: arm.tip,
                    radius: arm.foliage_radius,
                    sides: 4,
                    inner_color,
                    outer_color,
                    seed: arm.seed,
                },
            )?;
        }
    }
    Ok(())
}

fn append_silhouette_layers(
    geometry: &mut TreeGeometry,
    crown: &PineCrown,
) -> Result<(), RendererError> {
    let mass_count = if crown.layer_count >= 7 && crown.tree.genotype.leaf_density_fraction > 0.82 {
        4
    } else if crown.layer_count <= 3 {
        2
    } else {
        3
    };
    let last_layer = crown.layer_count.saturating_sub(2);
    for slot in 0..mass_count {
        let index = if mass_count == 1 {
            0
        } else {
            slot * last_layer / (mass_count - 1)
        };
        let layer = crown.layer(index);
        let seed = layer_seed(crown.tree.id, index);
        let angle = layer.turn * std::f32::consts::TAU;
        let (sine, cosine) = libm::sincosf(angle);
        let radial = (crown.tangent * cosine) + (crown.across * sine);
        let across = radial.cross(crown.up).normalize_or(Vec3::Z);
        let [inner_color, outer_color] = pine_foliage_colors(crown.tree, seed);
        append_layer_mass(
            geometry,
            &LayerMassSpec {
                center: layer.center
                    + (radial * (hash_fraction(seed, 1) - 0.5) * layer.reach * 0.20),
                up: crown.up * (layer.spacing * 0.16).min(layer.reach * 0.18).max(0.035),
                long: radial * layer.reach * 0.82,
                across: across * layer.reach * (0.30 + (hash_fraction(seed, 2) * 0.12)),
                inner_color,
                outer_color,
            },
        )?;
    }
    Ok(())
}

fn append_leader(
    geometry: &mut TreeGeometry,
    crown: &PineCrown,
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    let apex = crown.frame.base + crown.frame.trunk_vector;
    let length = (crown.radius * 0.44).min(crown.length * 0.15).max(0.12);
    let seed = crown.tree.id.rotate_left(11) ^ 0x4c45_4144_4552_5f5f;
    let [inner_color, outer_color] = pine_foliage_colors(crown.tree, seed);
    let radius = (length * 0.17).min(f64_as_f32(crown.tree.crown_radius_meters) * 0.16);
    let start = apex - (crown.up * length);
    let bough = BoughSpec {
        start,
        end: apex,
        radius,
        sides: match detail {
            TreeMeshDetail::Simplified => 4,
            TreeMeshDetail::Silhouette => 3,
        },
        inner_color,
        outer_color,
        seed,
    };
    append_bough(geometry, &bough)
}

fn layer_seed(id: u64, layer: usize) -> u64 {
    id.rotate_left(37)
        ^ u64::try_from(layer)
            .expect("pine layer fits u64")
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

#[cfg(test)]
pub(super) fn planned_layer_fractions(tree: ProceduralTree, frame: TreeFrame) -> Vec<f32> {
    PineCrown::new(tree, frame).map_or_else(Vec::new, |crown| {
        (0..crown.layer_count)
            .map(|index| {
                let center = crown.layer(index).center - crown.frame.base;
                center.dot(crown.up) / crown.frame.trunk_vector.length()
            })
            .collect()
    })
}
