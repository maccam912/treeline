//! Multi-bend scaffold wood for broadleaf distance tiers.

use glam::Vec3;

use super::architecture::BroadleafCrown;
use crate::RendererError;
use crate::tree_mesh::color::{CylinderMaterial, bark_color, bark_cylinder_material};
use crate::tree_mesh::geometry::TreeGeometry;
use crate::tree_mesh::shape::{CylinderSpec, append_tapered_cylinder};

pub(super) fn append_forked_scaffolds(
    geometry: &mut TreeGeometry,
    crown: &BroadleafCrown,
) -> Result<(), RendererError> {
    for index in 0..crown.fan_count {
        let scaffold = crown.scaffold(index);
        let first_cluster = index * 2;
        let present = [
            crown.cluster_present(first_cluster),
            crown.cluster_present(first_cluster + 1),
        ];
        if !present.into_iter().any(|value| value) {
            continue;
        }
        append_branch_segment(
            geometry,
            crown,
            scaffold.root,
            scaffold.elbow,
            scaffold.root_radius,
            scaffold.elbow_radius,
            bark_cylinder_material(crown.tree, index + 1),
        )?;
        for (fork, tip) in scaffold.tips.into_iter().enumerate() {
            if !present[fork] {
                continue;
            }
            append_branch_segment(
                geometry,
                crown,
                scaffold.elbow,
                scaffold.forks[fork],
                scaffold.elbow_radius,
                scaffold.fork_radius,
                CylinderMaterial::UNTEXTURED,
            )?;
            append_branch_segment(
                geometry,
                crown,
                scaffold.forks[fork],
                tip,
                scaffold.fork_radius,
                scaffold.tip_radius,
                CylinderMaterial::UNTEXTURED,
            )?;
        }
    }
    append_leader(geometry, crown)
}

fn append_leader(geometry: &mut TreeGeometry, crown: &BroadleafCrown) -> Result<(), RendererError> {
    let apex_index = crown.cluster_count() - 1;
    if !crown.cluster_present(apex_index) {
        return Ok(());
    }
    let bend = crown.leader_bend();
    let tip = crown.cluster(apex_index).center;
    let root_radius = (crown.frame.trunk_radius * 0.24).max(0.012);
    append_branch_segment(
        geometry,
        crown,
        crown.junction(),
        bend,
        root_radius,
        root_radius * 0.48,
        CylinderMaterial::UNTEXTURED,
    )?;
    append_branch_segment(
        geometry,
        crown,
        bend,
        tip,
        root_radius * 0.48,
        (root_radius * 0.13).max(0.004),
        CylinderMaterial::UNTEXTURED,
    )
}

pub(super) fn append_silhouette_fork(
    geometry: &mut TreeGeometry,
    crown: &BroadleafCrown,
) -> Result<(), RendererError> {
    let apex_index = crown.cluster_count() - 1;
    let mut appended = 0;
    if crown.cluster_present(apex_index) {
        let cluster = crown.cluster(apex_index);
        let start_radius = (crown.frame.trunk_radius * 0.10).max(0.010);
        append_branch_segment(
            geometry,
            crown,
            crown.junction(),
            cluster.center,
            start_radius,
            (start_radius * 0.12).max(0.003),
            CylinderMaterial::UNTEXTURED,
        )?;
        appended = 1;
    }
    for index in (0..crown.cluster_count().saturating_sub(1)).rev() {
        if !crown.cluster_present(index) {
            continue;
        }
        let cluster = crown.cluster(index);
        let start_radius = (crown.frame.trunk_radius * 0.085).max(0.012);
        append_branch_segment(
            geometry,
            crown,
            crown.junction(),
            cluster.center,
            start_radius,
            (start_radius * 0.14).max(0.004),
            CylinderMaterial::UNTEXTURED,
        )?;
        appended += 1;
        if appended == 2 {
            break;
        }
    }
    Ok(())
}

fn append_branch_segment(
    geometry: &mut TreeGeometry,
    crown: &BroadleafCrown,
    start: Vec3,
    end: Vec3,
    start_radius: f32,
    end_radius: f32,
    material: CylinderMaterial,
) -> Result<(), RendererError> {
    append_tapered_cylinder(
        geometry,
        &CylinderSpec {
            start,
            end,
            start_radius,
            end_radius,
            sides: 3,
            color: bark_color(crown.tree),
            material,
        },
    )
}
