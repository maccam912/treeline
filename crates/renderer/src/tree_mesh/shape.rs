//! The primitive solids tree geometry is assembled from.
//!
//! Trunks and branches are tapered cylinders; broadleaf crowns are octahedra.
//! Keeping them here lets the tree grammar read as structure rather than vertex
//! arithmetic. Conifer foliage is not here at all: it is shelled, and lives in
//! [`crate::tree_mesh::cluster`].

use glam::Vec3;

use crate::RendererError;
use crate::tree_mesh::color::CylinderMaterial;
use crate::tree_mesh::geometry::TreeGeometry;
use crate::vertex::{
    SURFACE_KIND_OAK_BARK, SURFACE_KIND_PINE_BARK, local_vertex, material_vertex, usize_as_f32,
};

pub(crate) struct CylinderSpec {
    pub(crate) start: Vec3,
    pub(crate) end: Vec3,
    pub(crate) start_radius: f32,
    pub(crate) end_radius: f32,
    pub(crate) sides: usize,
    pub(crate) color: [f32; 4],
    pub(crate) material: CylinderMaterial,
}

pub(crate) fn append_tapered_cylinder(
    geometry: &mut TreeGeometry,
    spec: &CylinderSpec,
) -> Result<(), RendererError> {
    let vertices = &mut geometry.vertices;
    let indices = &mut geometry.indices;
    let axis = (spec.end - spec.start).normalize_or_zero();
    if axis == Vec3::ZERO {
        return Ok(());
    }
    let reference = if axis.y.abs() < 0.92 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = axis.cross(reference).normalize_or_zero();
    let bitangent = axis.cross(tangent).normalize_or_zero();
    let base_index = u32::try_from(vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    let is_bark = spec.material.surface_kind >= SURFACE_KIND_PINE_BARK;
    let vertices_per_ring = if is_bark { spec.sides + 1 } else { spec.sides };
    let average_radius = (spec.start_radius + spec.end_radius) * 0.5;
    let is_pine_bark = spec.material.surface_kind < SURFACE_KIND_OAK_BARK;
    let repeat_width_meters = if is_pine_bark { 2.0 } else { 1.0 };
    let around_repeats =
        libm::roundf((std::f32::consts::TAU * average_radius / repeat_width_meters).max(1.0))
            .clamp(1.0, 12.0);
    let axial_repeats_per_meter = if is_pine_bark { 0.5 } else { 1.0 };
    let axis_length = (spec.end - spec.start).length();
    let u_offset = spec.material.seed * 7.0;
    let v_offset = (spec.material.seed * 17.0).fract();
    for ring in 0..2 {
        let (center, radius) = if ring == 0 {
            (spec.start, spec.start_radius)
        } else {
            (spec.end, spec.end_radius)
        };
        for side in 0..vertices_per_ring {
            let angle = usize_as_f32(side) / usize_as_f32(spec.sides) * std::f32::consts::TAU;
            let radial = (tangent * libm::cosf(angle)) + (bitangent * libm::sinf(angle));
            let position = center + (radial * radius);
            if is_bark {
                vertices.push(material_vertex(
                    position,
                    radial,
                    spec.color,
                    spec.material.surface_kind,
                    [
                        u_offset + (usize_as_f32(side) / usize_as_f32(spec.sides) * around_repeats),
                        v_offset + (usize_as_f32(ring) * axis_length * axial_repeats_per_meter),
                    ],
                ));
            } else {
                vertices.push(local_vertex(position, radial, spec.color, 0.0));
            }
        }
    }
    for side in 0..spec.sides {
        let next = if is_bark {
            side + 1
        } else {
            (side + 1) % spec.sides
        };
        let side = u32::try_from(side).map_err(|_| RendererError::TooManyIndices)?;
        let next = u32::try_from(next).map_err(|_| RendererError::TooManyIndices)?;
        let ring_stride =
            u32::try_from(vertices_per_ring).map_err(|_| RendererError::TooManyIndices)?;
        indices.extend_from_slice(&[
            base_index + side,
            base_index + next,
            base_index + ring_stride + side,
            base_index + next,
            base_index + ring_stride + next,
            base_index + ring_stride + side,
        ]);
    }
    Ok(())
}

pub(crate) fn append_octahedral_crown(
    geometry: &mut TreeGeometry,
    center: Vec3,
    radius: Vec3,
    color: [f32; 4],
) -> Result<(), RendererError> {
    let vertices = &mut geometry.vertices;
    let indices = &mut geometry.indices;
    let base_index = u32::try_from(vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    let offsets = [
        Vec3::Y * radius.y,
        Vec3::X * radius.x,
        Vec3::Z * radius.z,
        -Vec3::X * radius.x,
        -Vec3::Z * radius.z,
        -Vec3::Y * radius.y,
    ];
    for offset in offsets {
        vertices.push(local_vertex(
            center + offset,
            offset.normalize_or_zero(),
            color,
            0.0,
        ));
    }
    for triangle in [
        [0, 2, 1],
        [0, 3, 2],
        [0, 4, 3],
        [0, 1, 4],
        [5, 1, 2],
        [5, 2, 3],
        [5, 3, 4],
        [5, 4, 1],
    ] {
        indices.extend(triangle.map(|index| base_index + index));
    }
    Ok(())
}
