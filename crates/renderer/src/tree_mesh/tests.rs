//! What a stand of trees has to be true of, whatever the tier.
//!
//! Foliage has the same determinism and geometry obligations as the wood, plus
//! crown-specific ones: visible air, measured bounds, stable individual
//! silhouettes, and a hard cost ceiling at every distance tier.

use glam::Vec3;
use treeline_ecology::{
    BarkStyle, ForestComposition, GrowthConditions, Stand, TreeCondition, TreeFunctionalGroup,
    grow_tree,
};

use super::{
    ProceduralTree, TreeFrame, TreeGeometry, TreeMeshDetail, append_tree, bark_cylinder_material,
    pine, procedural_tree_geometry,
};
use crate::vertex::{SURFACE_KIND_PINE_FOLIAGE, TerrainVertex, f64_as_f32};

const TIERS: [TreeMeshDetail; 2] = [TreeMeshDetail::Simplified, TreeMeshDetail::Silhouette];

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

fn mature_conifer() -> ProceduralTree {
    (1..=256)
        .map(tree)
        .find(|tree| {
            tree.genotype.functional_group == TreeFunctionalGroup::EvergreenNeedleleaf
                && matches!(
                    tree.condition,
                    TreeCondition::Mature | TreeCondition::Ancient | TreeCondition::WindDamaged
                )
        })
        .expect("the fixture range contains a living mature conifer")
}

fn local_tree_geometry(tree: ProceduralTree, detail: TreeMeshDetail) -> TreeGeometry {
    let mut geometry = TreeGeometry::default();
    append_tree(&mut geometry, tree, detail, Vec3::ZERO).expect("one tree fits u32 addressing");
    geometry
}

fn foliage_vertices(geometry: &TreeGeometry) -> impl Iterator<Item = &TerrainVertex> {
    geometry
        .vertices
        .iter()
        .filter(|vertex| vertex.surface_kind.to_bits() == SURFACE_KIND_PINE_FOLIAGE.to_bits())
}

fn total_vertex_count(geometry: &TreeGeometry) -> usize {
    geometry.vertices.len()
}

fn total_index_count(geometry: &TreeGeometry) -> usize {
    geometry.indices.len()
}

fn upright_frame(tree: ProceduralTree) -> TreeFrame {
    TreeFrame {
        base: Vec3::ZERO,
        trunk_vector: Vec3::Y * f64_as_f32(tree.height_meters),
        trunk_radius: f64_as_f32(tree.trunk_base_radius_meters),
        trunk_top_radius: f64_as_f32(tree.trunk_base_radius_meters) * 0.12,
    }
}

fn position(vertex: &TerrainVertex) -> Vec3 {
    Vec3::from_array(vertex.world_position.map(f64_as_f32))
}

type VertexBits = ([u64; 3], [u32; 3], [u32; 4], u32, [u32; 2]);

fn vertex_bits(vertices: &[TerrainVertex]) -> Vec<VertexBits> {
    vertices
        .iter()
        .map(|vertex| {
            (
                vertex.world_position.map(f64::to_bits),
                vertex.normal.map(f32::to_bits),
                vertex.color.map(f32::to_bits),
                vertex.surface_kind.to_bits(),
                vertex.material_uv.map(f32::to_bits),
            )
        })
        .collect()
}

fn assert_well_formed(geometry: &TreeGeometry) {
    assert!(!geometry.vertices.is_empty());
    assert_eq!(geometry.vertices.is_empty(), geometry.indices.is_empty());
    assert!(geometry.indices.len().is_multiple_of(3));
    assert!(
        geometry
            .indices
            .iter()
            .all(|&index| usize::try_from(index).is_ok_and(|index| index < geometry.vertices.len()))
    );
    assert!(geometry.vertices.iter().all(|vertex| {
        vertex.world_position.into_iter().all(f64::is_finite)
            && vertex.normal.into_iter().all(f32::is_finite)
            && (vertex.color[3] - 1.0).abs() < f32::EPSILON
    }));
}

fn append_geometry(target: &mut TreeGeometry, source: &mut TreeGeometry) {
    let base = u32::try_from(target.vertices.len()).expect("an addressable stand");
    for index in &mut source.indices {
        *index += base;
    }
    target.vertices.append(&mut source.vertices);
    target.indices.append(&mut source.indices);
}

fn assert_same_geometry(left: &TreeGeometry, right: &TreeGeometry) {
    assert_eq!(vertex_bits(&left.vertices), vertex_bits(&right.vertices));
    assert_eq!(left.indices, right.indices);
}

/// Every triangle in a tree has to carry area: a sliver is a seam of stretched
/// bark that catches the light wrongly.
#[test]
fn no_face_of_a_tree_is_a_sliver() {
    for detail in TIERS {
        let geometry =
            procedural_tree_geometry(&stand(), detail, |_, _| Some(42.0)).expect("tree geometry");
        for triangle in geometry.indices.chunks_exact(3) {
            let corners = [0, 1, 2].map(|corner| {
                let index = usize::try_from(triangle[corner]).expect("an addressable vertex index");
                position(&geometry.vertices[index])
            });
            let area = (corners[1] - corners[0])
                .cross(corners[2] - corners[0])
                .length()
                * 0.5;
            assert!(
                area > 1.0e-5,
                "{detail:?} drew a sliver of {area} at {}",
                corners[0]
            );
        }
    }
}

/// Geometry must be bit-stable for one input and identical whether trees are
/// meshed together or one at a time.
#[test]
fn a_trees_geometry_is_bit_stable_and_neighbor_independent() {
    let stand = stand();
    let batch = procedural_tree_geometry(&stand, TreeMeshDetail::Simplified, |_, _| Some(42.0))
        .expect("tree geometry");
    let again = procedural_tree_geometry(&stand, TreeMeshDetail::Simplified, |_, _| Some(42.0))
        .expect("tree geometry");
    assert_same_geometry(&batch, &again);

    let mut concatenated = TreeGeometry::default();
    for tree in &stand {
        let mut alone =
            procedural_tree_geometry(&[*tree], TreeMeshDetail::Simplified, |_, _| Some(42.0))
                .expect("tree geometry");
        append_geometry(&mut concatenated, &mut alone);
    }
    assert_same_geometry(&batch, &concatenated);
}

#[test]
fn a_stand_builds_well_formed_colored_geometry() {
    let geometry = procedural_tree_geometry(&stand(), TreeMeshDetail::Simplified, |x, z| {
        Some((x + z) * 0.01)
    })
    .expect("tree geometry");
    assert_well_formed(&geometry);
}

#[test]
fn trees_without_a_surface_sample_are_skipped() {
    let geometry = procedural_tree_geometry(&stand(), TreeMeshDetail::Simplified, |_, _| None)
        .expect("tree geometry");
    assert!(geometry.vertices.is_empty());
    assert!(geometry.indices.is_empty());
}

#[test]
fn trunk_bases_are_buried_across_steep_ground() {
    let mut tree = tree(1);
    tree.lean_fraction = 0.0;
    let surface_height = |x: f64, z: f64| (x * 4.0) + (z * 1.5) + 42.0;
    let geometry = procedural_tree_geometry(&[tree], TreeMeshDetail::Simplified, |x, z| {
        Some(surface_height(x, z))
    })
    .expect("tree geometry");

    // Simplified bark has five sides and repeats its first vertex at the seam,
    // so the first ring is six vertices.
    for vertex in &geometry.vertices[..6] {
        let [x, y, z] = vertex.world_position;
        assert!(
            y <= surface_height(x, z),
            "trunk base floated at [{x}, {y}, {z}]"
        );
    }
}

#[test]
fn leaning_trunk_base_is_embedded_on_flat_ground() {
    let mut tree = tree(1);
    tree.condition = treeline_ecology::TreeCondition::Fallen;
    tree.lean_direction = [1.0, 0.0];
    tree.lean_fraction = 0.92;
    let geometry = procedural_tree_geometry(&[tree], TreeMeshDetail::Simplified, |_, _| Some(42.0))
        .expect("tree geometry");

    assert!(
        geometry.vertices[..6]
            .iter()
            .all(|vertex| vertex.world_position[1] <= 42.0)
    );
}

/// Coarser tiers must keep every individual and only shed geometry, so a
/// distant stand thins rather than losing trees.
#[test]
fn coarser_detail_sheds_geometry_without_dropping_trees() {
    let stand = stand();
    let tiers = TIERS.map(|detail| {
        procedural_tree_geometry(&stand, detail, |_, _| Some(42.0)).expect("tree geometry")
    });

    for geometry in &tiers {
        assert_well_formed(geometry);
    }
    for pair in tiers.windows(2) {
        assert!(total_vertex_count(&pair[0]) > total_vertex_count(&pair[1]));
        assert!(total_index_count(&pair[0]) > total_index_count(&pair[1]));
    }
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

#[test]
fn only_living_conifers_carry_pine_foliage() {
    let conifer = mature_conifer();
    let living = local_tree_geometry(conifer, TreeMeshDetail::Simplified);
    assert!(foliage_vertices(&living).next().is_some());

    let mut dead = conifer;
    dead.condition = TreeCondition::DeadStanding;
    let dead = local_tree_geometry(dead, TreeMeshDetail::Simplified);
    assert!(foliage_vertices(&dead).next().is_none());

    let broadleaf = (1..=256)
        .map(tree)
        .find(|tree| tree.genotype.functional_group != TreeFunctionalGroup::EvergreenNeedleleaf)
        .expect("the fixture range contains a broadleaf");
    let broadleaf = local_tree_geometry(broadleaf, TreeMeshDetail::Simplified);
    assert!(foliage_vertices(&broadleaf).next().is_none());
}

#[test]
fn pine_layers_are_ordered_separated_and_not_evenly_distributed() {
    let tree = mature_conifer();
    let fractions = pine::planned_layer_fractions(tree, upright_frame(tree));
    assert!(fractions.len() >= 4);
    let gaps = fractions
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    assert!(gaps.iter().all(|gap| *gap > 0.055));
    let smallest = gaps.iter().copied().reduce(f32::min).expect("layer gap");
    let largest = gaps.iter().copied().reduce(f32::max).expect("layer gap");
    assert!(largest - smallest > 0.012);
}

#[test]
fn pine_foliage_stays_inside_its_measured_crown_envelope() {
    let mut tree = mature_conifer();
    tree.lean_fraction = 0.0;
    tree.lean_direction = [1.0, 0.0];
    for detail in TIERS {
        let geometry = local_tree_geometry(tree, detail);
        let mut count = 0;
        for vertex in foliage_vertices(&geometry) {
            count += 1;
            let [x, y, z] = vertex.world_position;
            assert!(
                y <= tree.height_meters + 1.0e-4,
                "{detail:?} exceeded the top: {y} > {}",
                tree.height_meters
            );
            let horizontal = libm::hypot(x, z);
            assert!(
                horizontal <= tree.crown_radius_meters * 1.05,
                "{detail:?} exceeded the crown radius: {horizontal} > {}",
                tree.crown_radius_meters * 1.05
            );
        }
        assert!(count > 0);
    }
}

#[test]
fn pine_detail_tiers_have_fixed_per_tree_geometry_budgets() {
    let budgets = [(760, 4_000), (44, 186)];
    let mut conifers = 0;
    for tree in (1..=1_024).map(tree).filter(|tree| {
        tree.genotype.functional_group == TreeFunctionalGroup::EvergreenNeedleleaf
            && !matches!(
                tree.condition,
                TreeCondition::DeadStanding | TreeCondition::Fallen
            )
    }) {
        conifers += 1;
        for (detail, (max_vertices, max_indices)) in TIERS.into_iter().zip(budgets) {
            let geometry = local_tree_geometry(tree, detail);
            assert!(
                total_vertex_count(&geometry) <= max_vertices,
                "tree {} at {detail:?} used {} vertices",
                tree.id,
                total_vertex_count(&geometry)
            );
            assert!(
                total_index_count(&geometry) <= max_indices,
                "tree {} at {detail:?} used {} indices",
                tree.id,
                total_index_count(&geometry)
            );
        }
    }
    assert!(conifers > 100, "the budget sample must cover many conifers");
}

#[test]
fn damage_never_adds_pine_foliage() {
    let mut healthy = mature_conifer();
    healthy.damage_fraction = 0.0;
    let mut damaged = healthy;
    damaged.damage_fraction = 0.55;
    for detail in TIERS {
        let healthy_count = foliage_vertices(&local_tree_geometry(healthy, detail)).count();
        let damaged_count = foliage_vertices(&local_tree_geometry(damaged, detail)).count();
        assert!(damaged_count <= healthy_count);
    }
}
