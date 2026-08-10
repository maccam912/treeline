//! What a stand of trees has to be true of, whatever the tier.
//!
//! Foliage is absent from these, because foliage is absent from the renderer:
//! what a crown has to be true of belongs with the crowns, whenever they are
//! built again.

use glam::Vec3;
use treeline_ecology::{
    BarkStyle, ForestComposition, GrowthConditions, Stand, TreeFunctionalGroup, grow_tree,
};

use super::{
    ProceduralTree, TreeGeometry, TreeMeshDetail, bark_cylinder_material, procedural_tree_geometry,
};
use crate::vertex::TerrainVertex;

const TIERS: [TreeMeshDetail; 3] = [
    TreeMeshDetail::Full,
    TreeMeshDetail::Simplified,
    TreeMeshDetail::Silhouette,
];

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

fn position(vertex: &TerrainVertex) -> Vec3 {
    Vec3::from(vertex.position_high) + Vec3::from(vertex.position_low)
}

fn assert_well_formed(geometry: &TreeGeometry) {
    let vertices = &geometry.vertices;
    assert!(!vertices.is_empty());
    assert!(!geometry.indices.is_empty());
    assert!(geometry.indices.len().is_multiple_of(3));
    assert!(
        geometry
            .indices
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
        bytemuck::cast_slice::<_, u8>(&batch.vertices),
        bytemuck::cast_slice::<_, u8>(&again.vertices)
    );
    assert_eq!(batch.indices, again.indices);

    let mut concatenated = TreeGeometry::default();
    for tree in &stand {
        let mut alone = procedural_tree_geometry(&[*tree], TreeMeshDetail::Full, |_, _| Some(42.0))
            .expect("tree geometry");
        let base = u32::try_from(concatenated.vertices.len()).expect("an addressable stand");
        for index in &mut alone.indices {
            *index += base;
        }
        concatenated.vertices.append(&mut alone.vertices);
        concatenated.indices.append(&mut alone.indices);
    }
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&batch.vertices),
        bytemuck::cast_slice::<_, u8>(&concatenated.vertices)
    );
    assert_eq!(batch.indices, concatenated.indices);
}

#[test]
fn a_stand_builds_well_formed_colored_geometry() {
    let geometry =
        procedural_tree_geometry(&stand(), TreeMeshDetail::Full, |x, z| Some((x + z) * 0.01))
            .expect("tree geometry");
    assert_well_formed(&geometry);
}

#[test]
fn trees_without_a_surface_sample_are_skipped() {
    let geometry = procedural_tree_geometry(&stand(), TreeMeshDetail::Full, |_, _| None)
        .expect("tree geometry");
    assert!(geometry.vertices.is_empty());
    assert!(geometry.indices.is_empty());
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
        assert!(pair[0].vertices.len() > pair[1].vertices.len());
        assert!(pair[0].indices.len() > pair[1].indices.len());
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
