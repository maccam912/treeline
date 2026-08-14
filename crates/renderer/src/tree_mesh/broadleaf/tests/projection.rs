//! Projected far-crown coverage across viewing azimuths.

use glam::Vec3;
use treeline_ecology::TreeFunctionalGroup;

use super::{foliage_vertices, local_geometry, mature_tree};
use crate::TreeMeshDetail;
use crate::tree_mesh::TreeGeometry;
use crate::vertex::{SURFACE_KIND_BROADLEAF_FOLIAGE, f64_as_f32};

const SAMPLES: u32 = 96;

#[test]
fn far_maple_projections_keep_macro_windows() {
    let mut tree = mature_tree(TreeFunctionalGroup::TemperateBroadleaf);
    tree.lean_fraction = 0.0;
    for view_turn in [0.0, 0.125, 0.25, 0.375] {
        tree.rotation_turns += view_turn;
        let geometry = local_geometry(tree, TreeMeshDetail::Silhouette);
        let transparency = projected_transparency(&geometry);
        assert!(
            (0.08..=0.36).contains(&transparency),
            "far crown transparency at turn {view_turn} was {transparency:.3}"
        );
        tree.rotation_turns -= view_turn;
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn projected_transparency(geometry: &TreeGeometry) -> f32 {
    let foliage = foliage_vertices(geometry).collect::<Vec<_>>();
    let x_min = foliage
        .iter()
        .map(|vertex| vertex.world_position[0] as f32)
        .reduce(f32::min)
        .expect("foliage x minimum");
    let x_max = foliage
        .iter()
        .map(|vertex| vertex.world_position[0] as f32)
        .reduce(f32::max)
        .expect("foliage x maximum");
    let y_min = foliage
        .iter()
        .map(|vertex| vertex.world_position[1] as f32)
        .reduce(f32::min)
        .expect("foliage y minimum");
    let y_max = foliage
        .iter()
        .map(|vertex| vertex.world_position[1] as f32)
        .reduce(f32::max)
        .expect("foliage y maximum");
    let center = Vec3::new((x_min + x_max) * 0.5, (y_min + y_max) * 0.5, 0.0);
    let radii = Vec3::new((x_max - x_min) * 0.5, (y_max - y_min) * 0.5, 0.0);
    let triangles = geometry
        .indices
        .chunks_exact(3)
        .filter_map(|triangle| {
            let corners = [0, 1, 2].map(|corner| {
                &geometry.vertices
                    [usize::try_from(triangle[corner]).expect("addressable foliage vertex")]
            });
            corners
                .iter()
                .all(|vertex| {
                    vertex.surface_kind.to_bits() == SURFACE_KIND_BROADLEAF_FOLIAGE.to_bits()
                })
                .then(|| {
                    corners.map(|vertex| Vec3::from_array(vertex.world_position.map(f64_as_f32)))
                })
        })
        .collect::<Vec<_>>();

    let mut inside = 0_u32;
    let mut visible = 0_u32;
    for y in 0..SAMPLES {
        for x in 0..SAMPLES {
            let point = Vec3::new(
                x_min + ((x as f32 + 0.5) / SAMPLES as f32 * (x_max - x_min)),
                y_min + ((y as f32 + 0.5) / SAMPLES as f32 * (y_max - y_min)),
                0.0,
            );
            let ellipse =
                ((point.x - center.x) / radii.x).powi(2) + ((point.y - center.y) / radii.y).powi(2);
            if ellipse > 1.0 {
                continue;
            }
            inside += 1;
            visible += u32::from(
                triangles
                    .iter()
                    .any(|triangle| point_in_triangle(point, *triangle)),
            );
        }
    }
    1.0 - (visible as f32 / inside as f32)
}

fn point_in_triangle(point: Vec3, triangle: [Vec3; 3]) -> bool {
    let sign = |a: Vec3, b: Vec3| (point.x - b.x) * (a.y - b.y) - (a.x - b.x) * (point.y - b.y);
    let signs = [
        sign(triangle[0], triangle[1]),
        sign(triangle[1], triangle[2]),
        sign(triangle[2], triangle[0]),
    ];
    !(signs.iter().any(|value| *value < 0.0) && signs.iter().any(|value| *value > 0.0))
}
