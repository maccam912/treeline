//! What a stand of trees has to be true of, whatever the tier.

use glam::Vec3;
use treeline_ecology::{
    BarkStyle, ForestComposition, GrowthConditions, Stand, TreeFunctionalGroup, grow_tree,
};

use super::conifer::append_conifer_crown;
use super::{
    CrownShape, ProceduralTree, TreeCondition, TreeGeometry, TreeMeshDetail,
    bark_cylinder_material, procedural_tree_geometry,
};
use crate::vertex::{SURFACE_KIND_NEEDLE_FOLIAGE, TerrainVertex};

const CROWN_BASE: Vec3 = Vec3::ZERO;
const CROWN_APEX: Vec3 = Vec3::new(0.0, 20.0, 0.0);
const CROWN_RADIUS: f32 = 4.0;
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

fn position(vertex: &TerrainVertex) -> Vec3 {
    Vec3::from(vertex.position_high) + Vec3::from(vertex.position_low)
}

fn is_foliage(vertex: &TerrainVertex) -> bool {
    (vertex.surface_kind - SURFACE_KIND_NEEDLE_FOLIAGE).abs() < f32::EPSILON
}

fn foliage_positions(geometry: &TreeGeometry) -> Vec<Vec3> {
    geometry
        .vertices
        .iter()
        .filter(|vertex| is_foliage(vertex))
        .map(position)
        .collect()
}

/// One crown on its own, in crown space.
fn crown(detail: TreeMeshDetail) -> TreeGeometry {
    let mut geometry = TreeGeometry::default();
    append_conifer_crown(
        &mut geometry,
        conifer(),
        CROWN_BASE,
        CROWN_APEX,
        CROWN_RADIUS,
        [0.06, 0.24, 0.12, 1.0],
        detail,
    )
    .expect("a crown fits u32 addressing");
    geometry
}

fn assert_well_formed(geometry: &TreeGeometry) {
    let vertices = &geometry.vertices;
    assert!(!vertices.is_empty());
    // Trunks and crown hulls are always populated; the interior list is empty
    // by design now that a crown is one volume rather than a stack of shells.
    for indices in [&geometry.indices, &geometry.foliage_hull_indices] {
        assert!(!indices.is_empty());
        assert!(indices.len().is_multiple_of(3));
        assert!(
            indices
                .iter()
                .all(|&index| usize::try_from(index).is_ok_and(|index| index < vertices.len()))
        );
    }
    assert!(geometry.foliage_interior_indices.is_empty());
    assert!(vertices.iter().all(|vertex| {
        vertex.position_high.into_iter().all(f32::is_finite)
            && vertex.position_low.into_iter().all(f32::is_finite)
            && vertex.normal.into_iter().all(f32::is_finite)
            && (vertex.color[3] - 1.0).abs() < f32::EPSILON
    }));
}

/// A crown has to keep tapering: what it carries near the apex is far less than
/// what it carries at the base, or the tree stops reading as a conifer.
///
/// The band measured is the top fifth rather than the top third. A ball hung off
/// the highest whorl of the third below reaches up into it, so a third measures
/// the whorl under the spire as much as the spire.
#[test]
fn a_conifer_crown_tapers_to_its_apex_at_every_tier() {
    for detail in TIERS {
        let foliage = foliage_positions(&crown(detail));
        assert!(!foliage.is_empty(), "{detail:?} produced no foliage");
        let widest = |range: std::ops::Range<f32>| {
            foliage
                .iter()
                .filter(|point| range.contains(&point.y))
                .map(|point| Vec3::new(point.x, 0.0, point.z).length())
                .fold(0.0_f32, f32::max)
        };
        let bottom = widest(0.0..(CROWN_APEX.y / 3.0));
        let top = widest((CROWN_APEX.y * 0.8)..CROWN_APEX.y);
        assert!(
            top < bottom * 0.5,
            "{detail:?} crown is {top} wide on top against {bottom} at the base"
        );
    }
}

/// Every tier fills the same cone, so a crown does not visibly change shape
/// when a tile crosses a detail boundary.
#[test]
fn foliage_stays_inside_the_crown_at_every_tier() {
    for detail in TIERS {
        for point in foliage_positions(&crown(detail)) {
            let height = point.y / CROWN_APEX.y;
            assert!(
                (-0.2..1.05).contains(&height),
                "{detail:?} foliage sits at {height} of the crown"
            );
            let reach = Vec3::new(point.x, 0.0, point.z).length();
            assert!(
                reach <= CROWN_RADIUS * 1.15,
                "{detail:?} foliage reached {reach}, past the crown radius"
            );
        }
    }
}

/// A crown is a mass of solid balls, and every triangle in it has to carry area:
/// a sliver is a seam of stretched needles that catches the light wrongly.
///
/// Which way a face turns is a property of one ball rather than of the crown, so
/// the winding is checked against the ball's own center in [`super::cluster`].
#[test]
fn no_face_of_a_crown_is_a_sliver() {
    for detail in TIERS {
        let geometry = crown(detail);
        let indices = geometry.all_indices().collect::<Vec<_>>();
        for triangle in indices.chunks_exact(3) {
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

/// The crown is one cone the shader ray-marches, so every foliage vertex has to
/// agree on what that cone is: the same apex offset, the same base radius, and
/// the same needle-field seed. If any vertex carried a different crown, the
/// march would step through a cone no two fragments agreed on.
#[test]
fn a_crown_volume_carries_one_consistent_cone() {
    for detail in TIERS {
        let geometry = crown(detail);
        let mut apex_offset = None;
        let mut radius = None;
        let mut seed = None;
        for vertex in geometry.vertices.iter().filter(|vertex| is_foliage(vertex)) {
            let vertex_apex = Vec3::new(
                vertex.material_uv[0],
                vertex.material_uv[1],
                vertex.needle_seed,
            );
            let vertex_radius = vertex.needle_depth;
            let vertex_seed = vertex.snow_coverage;
            match apex_offset {
                None => {
                    apex_offset = Some(vertex_apex);
                    radius = Some(vertex_radius);
                    seed = Some(vertex_seed);
                }
                Some(known) => {
                    assert!(
                        approx_eq(vertex_apex, known),
                        "{detail:?} mixed apex offsets in one crown"
                    );
                    assert!(
                        approx_scalar(vertex_radius, radius.unwrap()),
                        "{detail:?} mixed radii"
                    );
                    assert!(
                        approx_scalar(vertex_seed, seed.unwrap()),
                        "{detail:?} mixed seeds"
                    );
                }
            }
        }
        assert!(
            approx_eq(apex_offset.unwrap(), CROWN_APEX - CROWN_BASE),
            "{detail:?} carried the wrong apex offset"
        );
        assert!(
            approx_scalar(radius.unwrap(), CROWN_RADIUS),
            "{detail:?} carried the wrong radius"
        );
    }
}

/// The crown is a closed volume drawn back-face culled, so every face has to
/// wind outward: a face wound inward is a hole the camera would see straight
/// through a crown.
#[test]
fn every_face_of_a_crown_volume_turns_outward() {
    for detail in TIERS {
        let geometry = crown(detail);
        let up = (CROWN_APEX - CROWN_BASE).normalize();
        let indices = geometry.foliage_hull_indices.clone();
        for triangle in indices.chunks_exact(3) {
            let corners = [0, 1, 2].map(|corner| {
                let index = usize::try_from(triangle[corner]).expect("an addressable vertex index");
                position(&geometry.vertices[index])
            });
            let winding = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
            assert!(winding.length() > 1.0e-4, "{detail:?} wound a sliver");
            let centroid = (corners[0] + corners[1] + corners[2]) / 3.0;
            // A side or top face points away from the crown axis; the base cap
            // points down. Either is outward for a cone.
            let axis_projected = centroid - (up * (centroid - CROWN_BASE).dot(up));
            let radial = axis_projected.normalize_or_zero();
            let down = -up;
            let radial_axis = radial.length_squared() > 1.0e-8;
            let outward = if radial_axis {
                winding.dot(radial) > 0.0 || winding.dot(down) > 0.0
            } else {
                winding.dot(down) > 0.0
            };
            assert!(outward, "{detail:?} wound a face into the crown");
        }
    }
}

fn approx_eq(left: Vec3, right: Vec3) -> bool {
    (left - right).length() < 1.0e-5
}

fn approx_scalar(left: f32, right: f32) -> bool {
    (left - right).abs() < 1.0e-5
}

/// The crown has to stay cheap. Shelling a ball multiplies it by the shell
/// count, which is why the near tier now draws far fewer and larger balls: the
/// budget goes on depth within a shoot rather than on more shoots. What comes
/// back is needles that stand off a branch instead of being painted onto it.
///
/// Still under an eighth of what a crown of alpha-tested billboards would cost,
/// and still opaque — the shells occlude and write depth once.
#[test]
fn a_full_detail_conifer_costs_under_six_thousand_triangles() {
    let geometry = procedural_tree_geometry(&[conifer()], TreeMeshDetail::Full, |_, _| Some(42.0))
        .expect("tree geometry");
    let triangles = geometry.all_indices().count() / 3;
    assert!(
        triangles < 6000,
        "one full-detail conifer costs {triangles} triangles"
    );
}

/// The shader marches foliage from the crown radius it reads off `needle_depth`,
/// so foliage has to carry it and nothing else may: a trunk or a patch of ground
/// that claimed a radius would stand its own marching crown up and sway in the
/// wind.
#[test]
fn only_conifer_foliage_carries_a_crown_radius() {
    let geometry = procedural_tree_geometry(&stand(), TreeMeshDetail::Full, |_, _| Some(42.0))
        .expect("tree geometry");
    assert!(
        geometry
            .vertices
            .iter()
            .all(|vertex| is_foliage(vertex) || vertex.needle_depth == 0.0)
    );
    assert!(
        geometry
            .vertices
            .iter()
            .any(|vertex| is_foliage(vertex) && vertex.needle_depth > 0.0)
    );
}

/// Which list a triangle lands in is what tells the renderer which pipeline
/// draws it, and a stand's vertices are the only record of which that should
/// be. A needle shell in the opaque list would be shaded as bark and lose its
/// needles; a trunk in a foliage list would have needles cut out of it.
#[test]
fn a_stands_index_lists_hold_exactly_their_own_surface_kind() {
    for detail in TIERS {
        let geometry =
            procedural_tree_geometry(&stand(), detail, |_, _| Some(42.0)).expect("tree geometry");
        let kind_of = |index: &u32| {
            let index = usize::try_from(*index).expect("an addressable vertex index");
            is_foliage(&geometry.vertices[index])
        };
        for foliage in [
            &geometry.foliage_hull_indices,
            &geometry.foliage_interior_indices,
        ] {
            assert!(
                foliage.iter().all(kind_of),
                "{detail:?} put something that is not foliage in a foliage list"
            );
        }
        assert!(
            !geometry.indices.iter().any(kind_of),
            "{detail:?} left foliage in the list the ground pipeline draws"
        );
    }
}

#[test]
fn every_tier_keeps_conifer_foliage() {
    let stand = stand();
    for detail in TIERS {
        let geometry =
            procedural_tree_geometry(&stand, detail, |_, _| Some(42.0)).expect("tree geometry");
        assert!(
            geometry.vertices.iter().any(is_foliage),
            "{detail:?} lost its conifer foliage"
        );
    }
}

#[test]
fn saplings_render_foliage() {
    let geometry = procedural_tree_geometry(&[sapling()], TreeMeshDetail::Full, |_, _| Some(42.0))
        .expect("tree geometry");
    assert!(geometry.vertices.iter().any(is_foliage));
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
    assert_eq!(batch.foliage_hull_indices, again.foliage_hull_indices);
    assert_eq!(
        batch.foliage_interior_indices,
        again.foliage_interior_indices
    );

    let mut concatenated = TreeGeometry::default();
    for tree in &stand {
        let mut alone = procedural_tree_geometry(&[*tree], TreeMeshDetail::Full, |_, _| Some(42.0))
            .expect("tree geometry");
        let base = concatenated.base_index().expect("an addressable stand");
        for index in alone
            .indices
            .iter_mut()
            .chain(&mut alone.foliage_hull_indices)
            .chain(&mut alone.foliage_interior_indices)
        {
            *index += base;
        }
        concatenated.vertices.append(&mut alone.vertices);
        concatenated.indices.append(&mut alone.indices);
        concatenated
            .foliage_hull_indices
            .append(&mut alone.foliage_hull_indices);
        concatenated
            .foliage_interior_indices
            .append(&mut alone.foliage_interior_indices);
    }
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&batch.vertices),
        bytemuck::cast_slice::<_, u8>(&concatenated.vertices)
    );
    assert_eq!(batch.indices, concatenated.indices);
    assert_eq!(
        batch.foliage_hull_indices,
        concatenated.foliage_hull_indices
    );
    assert_eq!(
        batch.foliage_interior_indices,
        concatenated.foliage_interior_indices
    );
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
    assert_eq!(geometry.all_indices().count(), 0);
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
        assert!(pair[0].all_indices().count() > pair[1].all_indices().count());
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
