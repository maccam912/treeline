//! Small opaque solids that stand in for masses of pine needles.
//!
//! A branchful of needles reads as one long, ragged lobe; these primitives
//! draw that perceptual unit directly, so the simplified tier stays one opaque
//! draw per tree tile.

use glam::Vec3;

use crate::RendererError;
use crate::tree_mesh::geometry::TreeGeometry;
use crate::vertex::{SURFACE_KIND_PINE_FOLIAGE, hash_fraction, material_vertex, usize_as_f32};

pub(super) struct BoughSpec {
    pub(super) start: Vec3,
    pub(super) end: Vec3,
    pub(super) radius: f32,
    pub(super) sides: usize,
    pub(super) inner_color: [f32; 4],
    pub(super) outer_color: [f32; 4],
    pub(super) seed: u64,
}

/// The branch-local volume one faceted needle mass occupies.
#[derive(Clone, Copy, Debug)]
pub(super) struct BoughEnvelope {
    pub(super) axis: Vec3,
    pub(super) tangent: Vec3,
    pub(super) bitangent: Vec3,
    nodes: [BoughNode; 6],
    turn: f32,
}

#[derive(Clone, Copy, Debug)]
struct BoughNode {
    center: Vec3,
    vertical_reach: f32,
}

impl BoughEnvelope {
    pub(super) fn new(spec: &BoughSpec) -> Option<Self> {
        let axis_vector = spec.end - spec.start;
        let length = axis_vector.length();
        if length <= f32::EPSILON || spec.radius <= 0.01 {
            return None;
        }
        let axis = axis_vector / length;
        let (tangent, bitangent) = perpendicular_frame(axis);
        let rings: [BoughNode; RING_ALONG.len()] = std::array::from_fn(|ring| {
            let vertical_reach =
                spec.radius * BOUGH_HEIGHT_SCALE * RING_RADIUS[ring] * MAX_RING_VARIATION;
            let bend = (tangent * signed(spec.seed, 1 + ring) * spec.radius * 0.34)
                + (bitangent * signed(spec.seed, 3 + ring) * spec.radius * 0.28);
            BoughNode {
                center: spec.start + (axis * length * RING_ALONG[ring]) + bend,
                vertical_reach,
            }
        });
        let point = |center| BoughNode {
            center,
            vertical_reach: 0.0,
        };
        Some(Self {
            axis,
            tangent,
            bitangent,
            nodes: [
                point(spec.start),
                rings[0],
                rings[1],
                rings[2],
                rings[3],
                point(spec.end),
            ],
            turn: hash_fraction(spec.seed, 0) * std::f32::consts::TAU,
        })
    }
}

/// Appends a pointed, bent multi-knot solid aligned with one branch tip.
pub(super) fn append_bough(
    geometry: &mut TreeGeometry,
    spec: &BoughSpec,
) -> Result<(), RendererError> {
    let Some(envelope) = BoughEnvelope::new(spec) else {
        return Ok(());
    };
    if spec.sides < 3 {
        return Ok(());
    }
    let base = u32::try_from(geometry.vertices.len()).map_err(|_| RendererError::TooManyIndices)?;

    geometry
        .vertices
        .push(foliage_vertex(spec.start, -envelope.axis, spec.inner_color));
    for (ring, (&along, node)) in RING_ALONG
        .iter()
        .zip(&envelope.nodes[1..=RING_ALONG.len()])
        .enumerate()
    {
        let nominal_height = node.vertical_reach / MAX_RING_VARIATION;
        for side in 0..spec.sides {
            let angle = envelope.turn
                + (usize_as_f32(side) / usize_as_f32(spec.sides) * std::f32::consts::TAU);
            let (sine, cosine) = libm::sincosf(angle);
            let radial = (envelope.tangent * cosine) + (envelope.bitangent * sine);
            let lane = 8 + ((ring * spec.sides + side) * 2);
            let height_reach =
                nominal_height * (0.84 + (hash_fraction(spec.seed, lane_as_u64(lane)) * 0.30));
            let horizontal = (radial.x * radial.x + radial.z * radial.z).sqrt();
            let vertical = radial.y;
            let width_reach = height_reach * BOUGH_WIDTH_RATIO;
            let scaled_radial = if horizontal > f32::EPSILON {
                let horizontal_dir = Vec3::new(radial.x / horizontal, 0.0, radial.z / horizontal);
                horizontal_dir * (horizontal * width_reach) + Vec3::Y * (vertical * height_reach)
            } else {
                radial * height_reach
            };
            let slope = match ring {
                0 => -0.26,
                1 => -0.08,
                2 => 0.10,
                _ => 0.24,
            };
            let normal = (radial + (envelope.axis * slope)).normalize_or(radial);
            let color = varied_color(
                mix_color(spec.inner_color, spec.outer_color, 0.16 + (along * 0.76)),
                signed(spec.seed, lane + 1) * 0.025,
            );
            geometry
                .vertices
                .push(foliage_vertex(node.center + scaled_radial, normal, color));
        }
    }
    geometry
        .vertices
        .push(foliage_vertex(spec.end, envelope.axis, spec.outer_color));

    append_bough_faces(&mut geometry.indices, base, spec.sides)?;
    Ok(())
}

fn append_bough_faces(
    indices: &mut Vec<u32>,
    base: u32,
    sides: usize,
) -> Result<(), RendererError> {
    let stride = u32::try_from(sides).map_err(|_| RendererError::TooManyIndices)?;
    let first_ring = base + 1;
    let ring_count = u32::try_from(RING_ALONG.len()).map_err(|_| RendererError::TooManyIndices)?;
    let end = first_ring + (stride * ring_count);
    for side in 0..sides {
        let next = (side + 1) % sides;
        let side = u32::try_from(side).map_err(|_| RendererError::TooManyIndices)?;
        let next = u32::try_from(next).map_err(|_| RendererError::TooManyIndices)?;
        indices.extend_from_slice(&[base, first_ring + next, first_ring + side]);
        for ring in 0..RING_ALONG.len() - 1 {
            let ring = u32::try_from(ring).map_err(|_| RendererError::TooManyIndices)?;
            let current = first_ring + (ring * stride);
            let following = current + stride;
            indices.extend_from_slice(&[
                current + side,
                current + next,
                following + side,
                current + next,
                following + next,
                following + side,
            ]);
        }
        let last_ring = first_ring + ((ring_count - 1) * stride);
        indices.extend_from_slice(&[last_ring + side, last_ring + next, end]);
    }
    Ok(())
}

const RING_ALONG: [f32; 4] = [0.12, 0.36, 0.64, 0.86];
const RING_RADIUS: [f32; 4] = [1.0, 1.0, 1.0, 0.55];
const BOUGH_HEIGHT_SCALE: f32 = 2.0;
const BOUGH_WIDTH_RATIO: f32 = 2.5;
const MAX_RING_VARIATION: f32 = 1.14;

pub(super) struct LayerMassSpec {
    pub(super) center: Vec3,
    pub(super) up: Vec3,
    pub(super) long: Vec3,
    pub(super) across: Vec3,
    pub(super) inner_color: [f32; 4],
    pub(super) outer_color: [f32; 4],
}

/// Appends one flattened, asymmetric octahedron for a far-away branch tier.
pub(super) fn append_layer_mass(
    geometry: &mut TreeGeometry,
    spec: &LayerMassSpec,
) -> Result<(), RendererError> {
    if [spec.up, spec.long, spec.across]
        .into_iter()
        .any(|axis| axis.length_squared() <= f32::EPSILON)
    {
        return Ok(());
    }
    let base = u32::try_from(geometry.vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    let corners = [
        (spec.up, spec.outer_color),
        (
            spec.long,
            mix_color(spec.inner_color, spec.outer_color, 0.68),
        ),
        (
            spec.across,
            mix_color(spec.inner_color, spec.outer_color, 0.54),
        ),
        (
            -spec.long,
            mix_color(spec.inner_color, spec.outer_color, 0.48),
        ),
        (
            -spec.across,
            mix_color(spec.inner_color, spec.outer_color, 0.42),
        ),
        (-spec.up, spec.inner_color),
    ];
    for (offset, color) in corners {
        geometry.vertices.push(foliage_vertex(
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

fn foliage_vertex(position: Vec3, normal: Vec3, color: [f32; 4]) -> crate::vertex::TerrainVertex {
    material_vertex(position, normal, color, SURFACE_KIND_PINE_FOLIAGE, [0.0; 2])
}

fn perpendicular_frame(axis: Vec3) -> (Vec3, Vec3) {
    let reference = if axis.y.abs() < 0.92 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = axis.cross(reference).normalize_or(Vec3::X);
    (tangent, axis.cross(tangent).normalize_or(Vec3::Z))
}

fn signed(seed: u64, lane: usize) -> f32 {
    hash_fraction(seed, lane_as_u64(lane)) - 0.5
}

fn lane_as_u64(lane: usize) -> u64 {
    u64::try_from(lane).expect("foliage variation lane fits u64")
}

fn mix_color(inner: [f32; 4], outer: [f32; 4], amount: f32) -> [f32; 4] {
    std::array::from_fn(|channel| inner[channel] + ((outer[channel] - inner[channel]) * amount))
}

fn varied_color(mut color: [f32; 4], variation: f32) -> [f32; 4] {
    for channel in &mut color[..3] {
        *channel = (*channel + variation).clamp(0.0, 1.0);
    }
    color
}
