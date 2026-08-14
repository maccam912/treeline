//! Broadleaf-specific geometry and crown-porosity invariants.

mod projection;

use glam::Vec3;
use treeline_ecology::{
    ForestComposition, GrowthConditions, ProceduralTree, Stand, TreeCondition, TreeFunctionalGroup,
    grow_tree,
};

use super::{planned_cluster_envelopes, planned_scaffolds};
use crate::TreeMeshDetail;
use crate::tree_mesh::{TreeFrame, TreeGeometry, append_tree};
use crate::vertex::{SURFACE_KIND_BROADLEAF_FOLIAGE, TerrainVertex, f64_as_f32};

const TIERS: [TreeMeshDetail; 2] = [TreeMeshDetail::Simplified, TreeMeshDetail::Silhouette];

fn tree(id: u64, group: TreeFunctionalGroup) -> ProceduralTree {
    let weights = match group {
        TreeFunctionalGroup::EvergreenNeedleleaf => [1.0, 0.0, 0.0],
        TreeFunctionalGroup::ColdDeciduous => [0.0, 1.0, 0.0],
        TreeFunctionalGroup::TemperateBroadleaf => [0.0, 0.0, 1.0],
    };
    grow_tree(
        id,
        0.0,
        0.0,
        GrowthConditions {
            stand: Stand::measured(0.68, 25.0).expect("measured test stand"),
            composition: ForestComposition::new(weights).expect("one functional group"),
            prevailing_wind: [0.8, 0.6],
        },
    )
}

fn mature_tree(group: TreeFunctionalGroup) -> ProceduralTree {
    (1..=1_024)
        .map(|id| tree(id, group))
        .find(|tree| {
            matches!(
                tree.condition,
                TreeCondition::Mature | TreeCondition::Ancient | TreeCondition::WindDamaged
            )
        })
        .expect("the fixture range contains a living mature tree")
}

fn upright_frame(tree: ProceduralTree) -> TreeFrame {
    let trunk_radius = f64_as_f32(tree.trunk_base_radius_meters);
    TreeFrame {
        base: Vec3::ZERO,
        trunk_vector: Vec3::Y * f64_as_f32(tree.height_meters),
        trunk_radius,
        trunk_top_radius: trunk_radius * 0.12,
    }
}

fn local_geometry(tree: ProceduralTree, detail: TreeMeshDetail) -> TreeGeometry {
    let mut geometry = TreeGeometry::default();
    append_tree(&mut geometry, tree, detail, Vec3::ZERO).expect("one tree fits u32 addressing");
    geometry
}

fn foliage_vertices(geometry: &TreeGeometry) -> impl Iterator<Item = &TerrainVertex> {
    geometry
        .vertices
        .iter()
        .filter(|vertex| vertex.surface_kind.to_bits() == SURFACE_KIND_BROADLEAF_FOLIAGE.to_bits())
}

#[test]
fn only_living_broadleaves_carry_broadleaf_foliage() {
    for group in [
        TreeFunctionalGroup::ColdDeciduous,
        TreeFunctionalGroup::TemperateBroadleaf,
    ] {
        let living = mature_tree(group);
        assert!(
            foliage_vertices(&local_geometry(living, TreeMeshDetail::Simplified))
                .next()
                .is_some()
        );

        let mut dead = living;
        dead.condition = TreeCondition::DeadStanding;
        assert!(
            foliage_vertices(&local_geometry(dead, TreeMeshDetail::Simplified))
                .next()
                .is_none()
        );
    }

    let conifer = mature_tree(TreeFunctionalGroup::EvergreenNeedleleaf);
    assert!(
        foliage_vertices(&local_geometry(conifer, TreeMeshDetail::Simplified))
            .next()
            .is_none()
    );
}

#[test]
fn broadleaf_foliage_stays_inside_the_measured_crown_envelope() {
    for group in [
        TreeFunctionalGroup::ColdDeciduous,
        TreeFunctionalGroup::TemperateBroadleaf,
    ] {
        let mut checked = 0;
        for tree in (1..=256).map(|id| tree(id, group)).filter(|tree| {
            !matches!(
                tree.condition,
                TreeCondition::DeadStanding | TreeCondition::Fallen
            )
        }) {
            for detail in TIERS {
                let geometry = local_geometry(tree, detail);
                assert!(
                    geometry
                        .vertices
                        .iter()
                        .all(|vertex| vertex.world_position[1] <= tree.height_meters + 0.01),
                    "tree {} at {detail:?} exceeded the measured canopy top",
                    tree.id
                );
                for vertex in foliage_vertices(&geometry) {
                    checked += 1;
                    let [x, y, z] = vertex.world_position;
                    let stem_fraction = (y / tree.height_meters).clamp(0.0, 1.0);
                    let axis_x = tree.lean_direction[0]
                        * tree.height_meters
                        * tree.lean_fraction
                        * stem_fraction;
                    let axis_z = tree.lean_direction[1]
                        * tree.height_meters
                        * tree.lean_fraction
                        * stem_fraction;
                    assert!(
                        libm::hypot(x - axis_x, z - axis_z) <= tree.crown_radius_meters * 1.08,
                        "tree {} at {detail:?} exceeded the measured crown radius",
                        tree.id
                    );
                }
            }
        }
        assert!(checked > 10_000, "the bound sample covers living crowns");
    }
}

#[test]
fn maple_scaffolds_fork_and_cold_deciduous_stays_narrower() {
    let rounded = mature_tree(TreeFunctionalGroup::TemperateBroadleaf);
    let columnar = mature_tree(TreeFunctionalGroup::ColdDeciduous);
    let rounded_frame = upright_frame(rounded);
    let columnar_frame = upright_frame(columnar);
    let rounded_scaffolds = planned_scaffolds(rounded, rounded_frame);
    let columnar_scaffolds = planned_scaffolds(columnar, columnar_frame);

    assert!((3..=5).contains(&rounded_scaffolds.len()));
    assert!((2..=3).contains(&columnar_scaffolds.len()));
    for scaffold in &rounded_scaffolds {
        let arms = scaffold.tips.map(|tip| (tip - scaffold.elbow).normalize());
        assert!(arms[0].dot(arms[1]) < 0.96, "a scaffold must visibly fork");
        assert!(scaffold.root.distance(scaffold.elbow) > f64_as_f32(rounded.height_meters) * 0.025);
        for fork in 0..2 {
            let before = (scaffold.forks[fork] - scaffold.elbow).normalize();
            let after = (scaffold.tips[fork] - scaffold.forks[fork]).normalize();
            assert!(before.dot(after) < 0.999, "a secondary arm must bend");
        }
    }

    let normalized_reach = |tree: ProceduralTree, frame: TreeFrame| {
        planned_cluster_envelopes(tree, frame)
            .into_iter()
            .map(|(center, _, _, _)| {
                let offset = center - frame.base;
                Vec3::new(offset.x, 0.0, offset.z).length() / f64_as_f32(tree.crown_radius_meters)
            })
            .reduce(f32::max)
            .expect("planned clusters")
    };
    assert!(normalized_reach(rounded, rounded_frame) > normalized_reach(columnar, columnar_frame));

    let mut sparse = rounded;
    sparse.genotype.branch_density_fraction = 0.56;
    let mut dense = rounded;
    dense.genotype.branch_density_fraction = 0.92;
    assert!(
        planned_scaffolds(dense, rounded_frame).len()
            > planned_scaffolds(sparse, rounded_frame).len()
    );

    let mut upright = rounded;
    upright.genotype.branching_angle_radians = 0.62;
    let mut spreading = rounded;
    spreading.genotype.branching_angle_radians = 1.08;
    assert!(normalized_reach(spreading, rounded_frame) > normalized_reach(upright, rounded_frame));
}

#[test]
fn broadleaf_detail_tiers_have_fixed_geometry_budgets() {
    let budgets = [(600, 2_800), (300, 1_500)];
    let mut broadleaves = 0;
    for group in [
        TreeFunctionalGroup::ColdDeciduous,
        TreeFunctionalGroup::TemperateBroadleaf,
    ] {
        for tree in (1..=1_024).map(|id| tree(id, group)).filter(|tree| {
            !matches!(
                tree.condition,
                TreeCondition::DeadStanding | TreeCondition::Fallen
            )
        }) {
            broadleaves += 1;
            for (detail, (max_vertices, max_indices)) in TIERS.into_iter().zip(budgets) {
                let geometry = local_geometry(tree, detail);
                assert!(
                    geometry.vertices.len() <= max_vertices,
                    "tree {} at {detail:?} used {} vertices",
                    tree.id,
                    geometry.vertices.len()
                );
                assert!(
                    geometry.indices.len() <= max_indices,
                    "tree {} at {detail:?} used {} indices",
                    tree.id,
                    geometry.indices.len()
                );
            }
        }
    }
    assert!(broadleaves > 1_000, "the budget sample covers both groups");
}

#[test]
fn damage_never_adds_broadleaf_foliage() {
    let mut healthy = mature_tree(TreeFunctionalGroup::TemperateBroadleaf);
    healthy.damage_fraction = 0.0;
    let mut damaged = healthy;
    damaged.damage_fraction = 0.55;
    for detail in TIERS {
        let healthy_count = foliage_vertices(&local_geometry(healthy, detail)).count();
        let damaged_count = foliage_vertices(&local_geometry(damaged, detail)).count();
        assert!(damaged_count <= healthy_count);
    }
}
