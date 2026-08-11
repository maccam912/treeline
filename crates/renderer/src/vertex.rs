//! CPU-side vertex data used while deterministic tree geometry is assembled.

use glam::Vec3;

pub(crate) const SURFACE_KIND_SOLID: f32 = 0.0;
pub(crate) const SURFACE_KIND_PINE_BARK: f32 = 2.0;
pub(crate) const SURFACE_KIND_OAK_BARK: f32 = 3.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct TerrainVertex {
    pub(crate) world_position: [f64; 3],
    pub(crate) normal: [f32; 3],
    pub(crate) color: [f32; 4],
    pub(crate) surface_kind: f32,
    pub(crate) material_uv: [f32; 2],
}

pub(crate) fn local_vertex(
    position: Vec3,
    normal: Vec3,
    color: [f32; 4],
    _snow_coverage: f32,
) -> TerrainVertex {
    TerrainVertex {
        world_position: position.as_dvec3().to_array(),
        normal: normal.to_array(),
        color,
        surface_kind: SURFACE_KIND_SOLID,
        material_uv: [0.0; 2],
    }
}

pub(crate) fn material_vertex(
    position: Vec3,
    normal: Vec3,
    color: [f32; 4],
    surface_kind: f32,
    material_uv: [f32; 2],
) -> TerrainVertex {
    let mut vertex = local_vertex(position, normal, color, 0.0);
    vertex.surface_kind = surface_kind;
    vertex.material_uv = material_uv;
    vertex
}

pub(crate) fn translate_local_vertices(vertices: &mut [TerrainVertex], origin: [f64; 3]) {
    for vertex in vertices {
        for (coordinate, offset) in vertex.world_position.iter_mut().zip(origin) {
            *coordinate += offset;
        }
    }
}

pub(crate) fn hash_lane(key: u64, lane: usize) -> f32 {
    let lane = u32::try_from(lane % 8).expect("hash lane is bounded");
    let byte = u8::try_from((key >> (lane * 8)) & 0xff).expect("masked hash lane fits u8");
    f32::from(byte) / 255.0
}

#[allow(clippy::cast_possible_truncation)]
pub(crate) fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn usize_as_f32(value: usize) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translating_tree_geometry_preserves_submeter_world_positions() {
        let mut vertices = vec![local_vertex(
            Vec3::new(0.125, 2.0, -0.0625),
            Vec3::Y,
            [1.0; 4],
            0.0,
        )];
        translate_local_vertices(&mut vertices, [5_000_000.0, 410.0, -5_000_000.0]);
        assert!((vertices[0].world_position[0] - 5_000_000.125).abs() < f64::EPSILON);
        assert!((vertices[0].world_position[2] + 5_000_000.062_5).abs() < f64::EPSILON);
    }

    #[test]
    fn bark_surface_kinds_are_distinct() {
        assert_ne!(
            SURFACE_KIND_SOLID.to_bits(),
            SURFACE_KIND_PINE_BARK.to_bits()
        );
        assert_ne!(
            SURFACE_KIND_PINE_BARK.to_bits(),
            SURFACE_KIND_OAK_BARK.to_bits()
        );
    }
}
