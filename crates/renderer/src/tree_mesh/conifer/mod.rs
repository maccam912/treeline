//! Conifer crowns, drawn as one closed volume per crown.
//!
//! A conifer's foliage is thousands of shoots, but drawing them is not what the
//! eye needs. What the eye needs is a crown whose edge is ragged and whose depth
//! reads as needles rather than as a lathed surface — and that is a shading
//! problem, not a geometry one. So a crown here is one low-poly cone: a solid
//! volume in the shape of the crown's envelope, with the cone's definition
//! carried on its vertices and the needles grown inside it in the fragment
//! shader.
//!
//! What this replaced was hundreds of nested-shell balls strung along whorls of
//! branches. That had the right ragged outline, but it shaded the same crown
//! pixels once per shell — five passes over the pixels a forest covers most.
//! A cone is drawn once, and the ray march in the foliage shader
//! ([`crate::gpu::pipeline::FOLIAGE_SHADER`]) steps
//! through its interior sampling a needle field as it goes. One draw call, one
//! pass per pixel, and a silhouette the march keeps ragged in the shader rather
//! than in the geometry.
//!
//! This file owns the crown's envelope — where its base and apex sit and how
//! wide it is. The cone's vertices carry the envelope plus a seed; the shader
//! reconstructs the volume and does the rest. Detail tiers differ only in how
//! many sides the cone is faceted into, so a distant crown keeps its shape and
//! loses only its smoothness.

use glam::Vec3;
use treeline_ecology::ProceduralTree;

use crate::RendererError;
use crate::TreeMeshDetail;
use crate::tree_mesh::geometry::TreeGeometry;
use crate::vertex::{crown_volume_vertex, hash_fraction, usize_as_f32};

/// Ring layers a cone is faceted into, as fractions of its height from base to
/// apex. The apex is a single vertex above the highest ring, so the highest
/// layer is where the top cap begins.
///
/// These are placed so both the lower and upper crown are sampled by the tier
/// tests: a lower band and an upper band each contain a ring.
const RING_FRACTIONS: [f32; 5] = [0.0, 0.2, 0.45, 0.7, 0.9];

/// Draws a conifer crown between `crown_base` and `apex` into `geometry` as one
/// closed cone, with its envelope carried on the foliage vertices.
pub(crate) fn append_conifer_crown(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    crown_base: Vec3,
    apex: Vec3,
    crown_radius: f32,
    foliage: [f32; 4],
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    let axis = apex - crown_base;
    let height = axis.length();
    if height <= f32::EPSILON || crown_radius <= f32::EPSILON {
        return Ok(());
    }
    let up = axis / height;
    let (tangent, bitangent) = crown_frame(up);
    let sides = cone_sides(detail);
    let base_index = geometry.base_index()?;

    let apex_offset = axis;
    let seed = hash_fraction(tree.id, 0);

    // Ring layers below the apex, widest at the base and narrowing upward. Each
    // vertex is one point on the cone surface, its offset from the crown base
    // being the "normal" the shader reads back to place the crown's base.
    for fraction in RING_FRACTIONS {
        let ring_radius = crown_radius * (1.0 - fraction);
        let height_along = height * fraction;
        for side in 0..sides {
            let around = usize_as_f32(side) / usize_as_f32(sides) * std::f32::consts::TAU;
            let (sin, cos) = libm::sincosf(around);
            let offset = (up * height_along) + ((tangent * cos + bitangent * sin) * ring_radius);
            geometry.vertices.push(crown_volume_vertex(
                offset,
                apex_offset,
                crown_radius,
                seed,
                foliage,
            ));
        }
    }
    // The apex is a single vertex at the top of the cone.
    let apex_index = as_u32(geometry.vertices.len());
    geometry.vertices.push(crown_volume_vertex(
        axis,
        apex_offset,
        crown_radius,
        seed,
        foliage,
    ));
    // The base center closes the cone's bottom so a crown is never hollow when
    // seen from below.
    let base_center_index = as_u32(geometry.vertices.len());
    geometry.vertices.push(crown_volume_vertex(
        Vec3::ZERO,
        apex_offset,
        crown_radius,
        seed,
        foliage,
    ));

    let ring = |layer: usize, side: usize| base_index + as_u32(layer * sides + side);
    let ring_layers = RING_FRACTIONS.len();

    // Appends one triangle, wound outward against `outward`.
    let mut append = |a: u32, b: u32, c: u32, outward: Vec3| {
        let first = position(geometry, a);
        let second = position(geometry, b);
        let third = position(geometry, c);
        let normal = (second - first).cross(third - first);
        let winding = if normal.dot(outward) >= 0.0 {
            [a, b, c]
        } else {
            [a, c, b]
        };
        geometry.foliage_hull_indices.extend_from_slice(&winding);
    };

    // Base cap: a fan from the base center out to the widest ring, facing down.
    for side in 0..sides {
        let next = (side + 1) % sides;
        append(base_center_index, ring(0, next), ring(0, side), -up);
    }

    // The cone's sides between each pair of ring layers.
    for layer in 0..ring_layers - 1 {
        for side in 0..sides {
            let next = (side + 1) % sides;
            let outward = radial(up, tangent, bitangent, side, sides);
            append(
                ring(layer, side),
                ring(layer + 1, side),
                ring(layer, next),
                outward,
            );
            append(
                ring(layer, next),
                ring(layer + 1, side),
                ring(layer + 1, next),
                outward,
            );
        }
    }

    // Cap the top: a fan from the apex down over the highest ring.
    for side in 0..sides {
        let next = (side + 1) % sides;
        let outward = radial(up, tangent, bitangent, side, sides);
        append(
            ring(ring_layers - 1, side),
            ring(ring_layers - 1, next),
            apex_index,
            outward,
        );
    }

    Ok(())
}

/// The direction a ring vertex at `side` sits from the crown axis, used as the
/// outward sense for winding its triangles.
fn radial(up: Vec3, tangent: Vec3, bitangent: Vec3, side: usize, sides: usize) -> Vec3 {
    let around = usize_as_f32(side) / usize_as_f32(sides) * std::f32::consts::TAU;
    let (sin, cos) = libm::sincosf(around);
    let radial = tangent * cos + bitangent * sin;
    radial - (up * radial.dot(up))
}

fn position(geometry: &TreeGeometry, index: u32) -> Vec3 {
    let index = usize::try_from(index).expect("a cone vertex index");
    let vertex = &geometry.vertices[index];
    Vec3::from(vertex.position_high) + Vec3::from(vertex.position_low)
}

#[allow(clippy::cast_possible_truncation)]
fn as_u32(value: usize) -> u32 {
    value as u32
}

/// A frame perpendicular to a crown's axis, for placing ring vertices around it.
fn crown_frame(axis: Vec3) -> (Vec3, Vec3) {
    let reference = if axis.y.abs() < 0.92 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = axis.cross(reference).normalize_or_zero();
    (tangent, axis.cross(tangent).normalize_or_zero())
}

/// Radial sides the crown volume is faceted into per detail tier. Coarser tiers
/// keep the crown's shape and shed only its smoothness.
const fn cone_sides(detail: TreeMeshDetail) -> usize {
    match detail {
        TreeMeshDetail::Full => 8,
        TreeMeshDetail::Simplified => 6,
        TreeMeshDetail::Silhouette => 4,
    }
}
