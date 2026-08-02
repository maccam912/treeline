//! The one vertex format every render path shares.
//!
//! Terrain, water, and trees all feed the same pipeline, distinguished by
//! `surface_kind` rather than by separate shaders. World positions are split
//! into high and low `f32` halves so the GPU can reconstruct them relative to
//! the camera without losing precision at world scale.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use treeline_mesher::Mesh;

use crate::RendererError;

pub(crate) const SURFACE_KIND_SOLID: f32 = 0.0;
pub(crate) const SURFACE_KIND_WATER: f32 = 1.0;
pub(crate) const SURFACE_KIND_PINE_BARK: f32 = 2.0;
pub(crate) const SURFACE_KIND_OAK_BARK: f32 = 3.0;
pub(crate) const SURFACE_KIND_NEEDLE_FOLIAGE: f32 = 4.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct TerrainVertex {
    pub(crate) position_high: [f32; 3],
    pub(crate) normal: [f32; 3],
    pub(crate) color: [f32; 4],
    pub(crate) snow_coverage: f32,
    pub(crate) position_low: [f32; 3],
    pub(crate) surface_kind: f32,
    pub(crate) material_uv: [f32; 2],
}

impl TerrainVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x4,
        3 => Float32,
        4 => Float32x3,
        5 => Float32,
        6 => Float32x2,
    ];

    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

pub(crate) fn terrain_vertex(
    position: [f64; 3],
    normal: [f32; 3],
    color: [f32; 4],
    snow_coverage: f32,
) -> TerrainVertex {
    let (position_high, position_low) = split_position(position);
    TerrainVertex {
        position_high,
        normal,
        color,
        snow_coverage,
        position_low,
        surface_kind: SURFACE_KIND_SOLID,
        material_uv: [0.0; 2],
    }
}

pub(crate) fn mesh_vertices(mesh: &Mesh, surface_kind: f32) -> Vec<TerrainVertex> {
    mesh.positions
        .iter()
        .zip(&mesh.normals)
        .enumerate()
        .map(|(index, (&position, &normal))| {
            let mut vertex = terrain_vertex(
                position,
                normal,
                mesh.colors
                    .get(index)
                    .copied()
                    .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                0.0,
            );
            vertex.surface_kind = surface_kind;
            vertex
        })
        .collect()
}

pub(crate) fn local_vertex(
    position: Vec3,
    normal: Vec3,
    color: [f32; 4],
    snow_coverage: f32,
) -> TerrainVertex {
    terrain_vertex(
        position.as_dvec3().to_array(),
        normal.to_array(),
        color,
        snow_coverage,
    )
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

pub(crate) fn split_position(position: [f64; 3]) -> ([f32; 3], [f32; 3]) {
    let split = position.map(split_f64);
    (
        [split[0][0], split[1][0], split[2][0]],
        [split[0][1], split[1][1], split[2][1]],
    )
}

pub(crate) fn split_f64(value: f64) -> [f32; 2] {
    let high = f64_as_f32(value);
    [high, f64_as_f32(value - f64::from(high))]
}

pub(crate) fn translate_local_vertices(vertices: &mut [TerrainVertex], origin: [f64; 3]) {
    for vertex in vertices {
        let local: [f64; 3] = std::array::from_fn(|axis| {
            f64::from(vertex.position_high[axis]) + f64::from(vertex.position_low[axis])
        });
        let (position_high, position_low) =
            split_position(std::array::from_fn(|axis| origin[axis] + local[axis]));
        vertex.position_high = position_high;
        vertex.position_low = position_low;
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

#[allow(dead_code)]
pub(crate) fn usize_as_u32(value: usize) -> Result<u32, RendererError> {
    u32::try_from(value).map_err(|_| RendererError::TooManyIndices)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// World coordinates exceed `f32` precision, so the split has to survive a
    /// long warp: the camera-relative difference must stay sub-meter exact.
    #[test]
    fn splitting_preserves_submeter_offsets_at_world_scale() {
        let origin = [5_000_000.0, 800.0, -5_000_000.0];
        let position = [5_000_000.125, 799.9375, -4_999_999.875];
        let (origin_high, origin_low) = split_position(origin);
        let (position_high, position_low) = split_position(position);
        let relative: [f32; 3] = std::array::from_fn(|axis| {
            (position_high[axis] - origin_high[axis]) + (position_low[axis] - origin_low[axis])
        });

        for (actual, expected) in relative.into_iter().zip([0.125, -0.0625, 0.125]) {
            assert!((actual - expected).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn water_and_terrain_share_a_format_but_not_a_surface_kind() {
        let mesh = Mesh {
            positions: vec![[0.0, 4.0, 0.0]],
            normals: vec![[0.0, 1.0, 0.0]],
            colors: vec![[0.04, 0.34, 0.58, 1.0]],
            indices: Vec::new(),
        };
        let terrain = mesh_vertices(&mesh, SURFACE_KIND_SOLID);
        let water = mesh_vertices(&mesh, SURFACE_KIND_WATER);

        assert!(terrain[0].surface_kind.abs() < f32::EPSILON);
        assert!((water[0].surface_kind - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            terrain[0].color.map(f32::to_bits),
            water[0].color.map(f32::to_bits)
        );
    }

    #[test]
    fn a_mesh_without_colors_gets_a_zero_weight_placeholder() {
        let mesh = Mesh {
            positions: vec![[0.0; 3]],
            normals: vec![[0.0, 1.0, 0.0]],
            colors: Vec::new(),
            indices: Vec::new(),
        };
        assert!(mesh_vertices(&mesh, SURFACE_KIND_SOLID)[0].color[3].abs() < f32::EPSILON);
    }

    #[test]
    fn translating_local_geometry_keeps_it_reconstructible() {
        let mut vertices = vec![local_vertex(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::Y,
            [1.0; 4],
            0.0,
        )];
        translate_local_vertices(&mut vertices, [1_000_000.0, 0.0, -1_000_000.0]);
        let reconstructed: [f64; 3] = std::array::from_fn(|axis| {
            f64::from(vertices[0].position_high[axis]) + f64::from(vertices[0].position_low[axis])
        });

        assert!((reconstructed[0] - 1_000_001.0).abs() < 1.0e-6);
        assert!((reconstructed[2] + 999_997.0).abs() < 1.0e-6);
    }

    #[test]
    fn every_surface_kind_occupies_a_distinct_band() {
        const _: () = assert!(SURFACE_KIND_NEEDLE_FOLIAGE > SURFACE_KIND_OAK_BARK);
        let mut kinds = vec![
            SURFACE_KIND_SOLID,
            SURFACE_KIND_WATER,
            SURFACE_KIND_PINE_BARK,
            SURFACE_KIND_OAK_BARK,
            SURFACE_KIND_NEEDLE_FOLIAGE,
        ];
        kinds.sort_by(f32::total_cmp);
        kinds.dedup();
        assert_eq!(kinds.len(), 5);
    }
}
