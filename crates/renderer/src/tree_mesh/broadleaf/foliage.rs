//! Faceted leaf cloudlets with real air between them.

use glam::Vec3;

use crate::RendererError;
use crate::tree_mesh::geometry::TreeGeometry;
use crate::vertex::{SURFACE_KIND_BROADLEAF_FOLIAGE, hash_fraction, material_vertex, usize_as_f32};

pub(super) struct LeafClusterSpec {
    pub(super) center: Vec3,
    pub(super) up: Vec3,
    pub(super) long: Vec3,
    pub(super) across: Vec3,
    pub(super) inner_color: [f32; 4],
    pub(super) outer_color: [f32; 4],
    pub(super) seed: u64,
}

/// Appends an irregular three-ring cloudlet for branch-scale foliage.
pub(super) fn append_leaf_cluster(
    geometry: &mut TreeGeometry,
    spec: &LeafClusterSpec,
) -> Result<(), RendererError> {
    append_faceted_cluster(
        geometry,
        spec,
        &[(-0.52, 0.72, 0.18), (-0.02, 1.00, 0.46), (0.50, 0.76, 0.72)],
    )
}

/// Appends the aligned two-ring version of a defining lobe for the far tier.
pub(super) fn append_leaf_silhouette(
    geometry: &mut TreeGeometry,
    spec: &LeafClusterSpec,
) -> Result<(), RendererError> {
    append_faceted_cluster(geometry, spec, &[(-0.42, 0.90, 0.24), (0.42, 0.90, 0.68)])
}

fn append_faceted_cluster(
    geometry: &mut TreeGeometry,
    spec: &LeafClusterSpec,
    rings: &[(f32, f32, f32)],
) -> Result<(), RendererError> {
    if invalid_axes(spec) {
        return Ok(());
    }
    let base = u32::try_from(geometry.vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    geometry.vertices.push(leaf_vertex(
        spec.center - (spec.up * 0.82),
        -spec.up.normalize_or(Vec3::Y),
        spec.inner_color,
    ));
    for (ring, &(height, radius, color_mix)) in rings.iter().enumerate() {
        append_ring(geometry, spec, height, radius, ring, color_mix);
    }
    geometry.vertices.push(leaf_vertex(
        spec.center + (spec.up * 0.82),
        spec.up.normalize_or(Vec3::Y),
        spec.outer_color,
    ));

    let sides = u32::try_from(CLUSTER_SIDES).expect("cluster sides fit u32");
    let first_ring = base + 1;
    let top = first_ring + sides * u32::try_from(rings.len()).expect("cluster ring count fits u32");
    for side in 0..sides {
        let next = (side + 1) % sides;
        geometry.indices.extend_from_slice(&[
            base,
            first_ring + next,
            first_ring + side,
            top - sides + side,
            top - sides + next,
            top,
        ]);
        for ring in 0..rings.len() - 1 {
            let lower =
                first_ring + sides * u32::try_from(ring).expect("cluster ring index fits u32");
            let upper = lower + sides;
            geometry.indices.extend_from_slice(&[
                lower + side,
                lower + next,
                upper + side,
                lower + next,
                upper + next,
                upper + side,
            ]);
        }
    }
    Ok(())
}

/// Appends a cheaper asymmetric octahedron for interior and satellite masses.
pub(super) fn append_leaf_mass(
    geometry: &mut TreeGeometry,
    spec: &LeafClusterSpec,
) -> Result<(), RendererError> {
    if invalid_axes(spec) {
        return Ok(());
    }
    let base = u32::try_from(geometry.vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    let turn = hash_fraction(spec.seed, 0x5455_524e) * std::f32::consts::TAU;
    let (sine, cosine) = libm::sincosf(turn);
    let long = (spec.long * cosine) + (spec.across * sine);
    let across = (-spec.long * sine) + (spec.across * cosine);
    for (offset, color) in [
        (spec.up, spec.outer_color),
        (long, mix_color(spec.inner_color, spec.outer_color, 0.68)),
        (across, mix_color(spec.inner_color, spec.outer_color, 0.58)),
        (-long, mix_color(spec.inner_color, spec.outer_color, 0.48)),
        (-across, mix_color(spec.inner_color, spec.outer_color, 0.42)),
        (-spec.up, spec.inner_color),
    ] {
        geometry.vertices.push(leaf_vertex(
            spec.center + offset,
            offset.normalize_or(Vec3::Y),
            color,
        ));
    }
    for face in OCTAHEDRON_FACES {
        geometry.indices.extend(face.map(|corner| base + corner));
    }
    Ok(())
}

fn append_ring(
    geometry: &mut TreeGeometry,
    spec: &LeafClusterSpec,
    height: f32,
    radius: f32,
    ring: usize,
    color_mix: f32,
) {
    let step = std::f32::consts::TAU / usize_as_f32(CLUSTER_SIDES);
    let turn = (hash_fraction(spec.seed, 0x5455_524e) * std::f32::consts::TAU)
        + (usize_as_f32(ring) * step * 0.22);
    let color = mix_color(spec.inner_color, spec.outer_color, color_mix);
    for side in 0..CLUSTER_SIDES {
        let angle =
            turn + (usize_as_f32(side) / usize_as_f32(CLUSTER_SIDES) * std::f32::consts::TAU);
        let (sine, cosine) = libm::sincosf(angle);
        let lane =
            0x200 + u64::try_from((ring * CLUSTER_SIDES) + side).expect("leaf ring lane fits u64");
        let variation = 0.90 + (hash_fraction(spec.seed, lane) * 0.18);
        let radial = ((spec.long * cosine) + (spec.across * sine)) * radius * variation;
        let offset = radial + (spec.up * height);
        let normal = (radial.normalize_or(Vec3::X) + (spec.up.normalize_or(Vec3::Y) * height))
            .normalize_or(Vec3::Y);
        geometry
            .vertices
            .push(leaf_vertex(spec.center + offset, normal, color));
    }
}

fn invalid_axes(spec: &LeafClusterSpec) -> bool {
    [spec.up, spec.long, spec.across]
        .into_iter()
        .any(|axis| axis.length_squared() <= f32::EPSILON)
}

fn leaf_vertex(position: Vec3, normal: Vec3, color: [f32; 4]) -> crate::vertex::TerrainVertex {
    material_vertex(
        position,
        normal,
        color,
        SURFACE_KIND_BROADLEAF_FOLIAGE,
        [0.0; 2],
    )
}

fn mix_color(inner: [f32; 4], outer: [f32; 4], amount: f32) -> [f32; 4] {
    std::array::from_fn(|channel| inner[channel] + ((outer[channel] - inner[channel]) * amount))
}

const OCTAHEDRON_FACES: [[u32; 3]; 8] = [
    [0, 2, 1],
    [0, 3, 2],
    [0, 4, 3],
    [0, 1, 4],
    [5, 1, 2],
    [5, 2, 3],
    [5, 3, 4],
    [5, 4, 1],
];

const CLUSTER_SIDES: usize = 5;
