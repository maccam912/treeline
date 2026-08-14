//! Broadleaf crowns built as forked wood carrying separated leaf cloudlets.

mod architecture;
mod foliage;
#[cfg(test)]
mod tests;
mod wood;

use treeline_ecology::{ProceduralTree, TreeCondition, TreeFunctionalGroup};

use crate::tree_mesh::TreeFrame;
use crate::tree_mesh::color::broadleaf_foliage_colors;
use crate::tree_mesh::geometry::TreeGeometry;
use crate::{RendererError, TreeMeshDetail};
#[cfg(test)]
use architecture::Scaffold;
use architecture::{BroadleafCrown, LeafCluster};
use foliage::{LeafClusterSpec, append_leaf_cluster, append_leaf_mass, append_leaf_silhouette};
use wood::{append_forked_scaffolds, append_silhouette_fork};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeafGeometry {
    BranchCloudlet,
    SilhouetteLobe,
    InteriorMass,
}

pub(super) fn trunk_end_fraction(tree: ProceduralTree, frame: TreeFrame) -> f32 {
    if matches!(
        tree.condition,
        TreeCondition::DeadStanding | TreeCondition::Fallen
    ) {
        return 1.0;
    }
    BroadleafCrown::new(tree, frame).map_or(1.0, |crown| crown.trunk_end_fraction())
}

pub(super) fn append_broadleaf_crown(
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
    let Some(crown) = BroadleafCrown::new(tree, frame) else {
        return Ok(());
    };
    match detail {
        TreeMeshDetail::Simplified => append_forked_scaffolds(geometry, &crown)?,
        TreeMeshDetail::Silhouette => append_silhouette_fork(geometry, &crown)?,
    }
    append_leaf_cloudlets(geometry, &crown, detail)
}

fn append_leaf_cloudlets(
    geometry: &mut TreeGeometry,
    crown: &BroadleafCrown,
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    for index in 0..crown.cluster_count() {
        if !crown.cluster_present(index) {
            continue;
        }
        let cluster = crown.cluster(index);
        // Defining lobes keep the same centers and envelopes in both tiers so
        // projected coverage and macro windows stay stable across the boundary.
        let macro_detail = match detail {
            TreeMeshDetail::Simplified => LeafGeometry::BranchCloudlet,
            TreeMeshDetail::Silhouette => LeafGeometry::SilhouetteLobe,
        };
        append_cluster(geometry, crown.tree, cluster, macro_detail)?;
    }
    for index in 0..crown.fan_count {
        let first_cluster = index * 2;
        if crown.cluster_present(first_cluster) || crown.cluster_present(first_cluster + 1) {
            append_cluster(
                geometry,
                crown.tree,
                crown.branch_cluster(index),
                LeafGeometry::InteriorMass,
            )?;
        }
    }
    Ok(())
}

fn append_cluster(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    cluster: LeafCluster,
    detail: LeafGeometry,
) -> Result<(), RendererError> {
    let [inner_color, outer_color] = broadleaf_foliage_colors(tree, cluster.seed, cluster.exposure);
    let mass_scale = match (detail, tree.genotype.functional_group) {
        (LeafGeometry::InteriorMass, TreeFunctionalGroup::TemperateBroadleaf) => 1.12,
        (LeafGeometry::InteriorMass, TreeFunctionalGroup::ColdDeciduous) => 1.05,
        _ => 1.0,
    };
    let mass_height = if detail == LeafGeometry::InteriorMass
        && tree.genotype.functional_group == TreeFunctionalGroup::TemperateBroadleaf
    {
        0.82
    } else {
        1.0
    };
    let spec = LeafClusterSpec {
        center: cluster.center,
        up: cluster.up * mass_height,
        long: cluster.long * mass_scale,
        across: cluster.across * mass_scale,
        inner_color,
        outer_color,
        seed: cluster.seed,
    };
    match detail {
        LeafGeometry::BranchCloudlet => append_leaf_cluster(geometry, &spec),
        LeafGeometry::SilhouetteLobe => append_leaf_silhouette(geometry, &spec),
        LeafGeometry::InteriorMass => append_leaf_mass(geometry, &spec),
    }
}

#[cfg(test)]
fn planned_cluster_envelopes(
    tree: ProceduralTree,
    frame: TreeFrame,
) -> Vec<(glam::Vec3, glam::Vec3, glam::Vec3, glam::Vec3)> {
    BroadleafCrown::new(tree, frame).map_or_else(Vec::new, |crown| {
        (0..crown.cluster_count())
            .filter(|&index| crown.cluster_present(index))
            .map(|index| {
                let cluster = crown.cluster(index);
                (cluster.center, cluster.up, cluster.long, cluster.across)
            })
            .collect()
    })
}

#[cfg(test)]
fn planned_scaffolds(tree: ProceduralTree, frame: TreeFrame) -> Vec<Scaffold> {
    BroadleafCrown::new(tree, frame).map_or_else(Vec::new, |crown| {
        (0..crown.fan_count)
            .map(|index| crown.scaffold(index))
            .collect()
    })
}
