//! Conversion from generator meshes to camera-local Bevy assets.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::Mesh as BevyMesh;
use treeline_mesher::Mesh;

use crate::RendererError;
use crate::snow::SnowDepthGrid;
use crate::tree_mesh::TreeGeometry;

/// A GPU-ready mesh plus its double-precision placement in the measured world.
#[derive(Debug)]
pub struct PreparedMesh {
    pub mesh: BevyMesh,
    pub world_origin: [f64; 3],
}

/// Converts generated ground geometry into a camera-local Bevy mesh.
///
/// # Errors
///
/// Returns [`RendererError::TooManyIndices`] when the geometry cannot be
/// represented by Bevy's `u32` index buffer.
pub fn prepare_terrain_mesh(
    source: &Mesh,
    mut snow_depth_at: impl FnMut(f64, f64) -> Option<f64>,
) -> Result<PreparedMesh, RendererError> {
    let snow = SnowDepthGrid::sample(source, &mut snow_depth_at);
    prepared_generator_mesh(source, SurfaceAppearance::Terrain, |position| {
        snow.coverage_at(position)
    })
}

/// Converts generated lake geometry into a camera-local Bevy mesh.
///
/// # Errors
///
/// Returns [`RendererError::TooManyIndices`] when the geometry cannot be
/// represented by Bevy's `u32` index buffer.
pub fn prepare_water_mesh(source: &Mesh) -> Result<PreparedMesh, RendererError> {
    prepared_generator_mesh(source, SurfaceAppearance::Water, |_| 0.0)
}

pub(crate) fn prepared_tree_mesh(geometry: TreeGeometry) -> Option<PreparedMesh> {
    if geometry.indices.is_empty() || geometry.vertices.is_empty() {
        return None;
    }
    let origin = geometry_origin(geometry.vertices.iter().map(|vertex| vertex.world_position));
    let positions = geometry
        .vertices
        .iter()
        .map(|vertex| relative_position(vertex.world_position, origin))
        .collect::<Vec<_>>();
    let normals = geometry
        .vertices
        .iter()
        .map(|vertex| vertex.normal)
        .collect::<Vec<_>>();
    let colors = geometry
        .vertices
        .iter()
        .map(|vertex| vertex.color)
        .collect::<Vec<_>>();
    let uvs = geometry
        .vertices
        .iter()
        .map(|vertex| vertex.material_uv)
        .collect::<Vec<_>>();
    Some(PreparedMesh {
        mesh: bevy_mesh(positions, normals, colors, uvs, geometry.indices),
        world_origin: origin,
    })
}

#[derive(Clone, Copy)]
enum SurfaceAppearance {
    Terrain,
    Water,
}

fn prepared_generator_mesh(
    source: &Mesh,
    appearance: SurfaceAppearance,
    mut snow_depth: impl FnMut([f64; 3]) -> f64,
) -> Result<PreparedMesh, RendererError> {
    let origin = geometry_origin(source.positions.iter().copied());
    let positions = source
        .positions
        .iter()
        .copied()
        .map(|position| relative_position(position, origin))
        .collect::<Vec<_>>();
    let colors = source
        .positions
        .iter()
        .enumerate()
        .map(|(index, &position)| {
            surface_color(
                appearance,
                source.colors.get(index).copied(),
                snow_depth(position),
            )
        })
        .collect::<Vec<_>>();
    let uvs = source
        .positions
        .iter()
        .map(|position| {
            [
                f64_as_f32(position[0] * 0.08),
                f64_as_f32(position[2] * 0.08),
            ]
        })
        .collect::<Vec<_>>();
    let indices = source.indices.clone();
    let _ = u32::try_from(indices.len()).map_err(|_| RendererError::TooManyIndices)?;
    Ok(PreparedMesh {
        mesh: bevy_mesh(positions, source.normals.clone(), colors, uvs, indices),
        world_origin: origin,
    })
}

fn bevy_mesh(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
) -> BevyMesh {
    let mut mesh = BevyMesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn geometry_origin(positions: impl Iterator<Item = [f64; 3]>) -> [f64; 3] {
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(position[axis]);
            maximum[axis] = maximum[axis].max(position[axis]);
        }
    }
    if minimum.iter().any(|value| !value.is_finite()) {
        return [0.0; 3];
    }
    std::array::from_fn(|axis| (minimum[axis] + maximum[axis]) * 0.5)
}

fn relative_position(position: [f64; 3], origin: [f64; 3]) -> [f32; 3] {
    std::array::from_fn(|axis| f64_as_f32(position[axis] - origin[axis]))
}

fn surface_color(
    appearance: SurfaceAppearance,
    measured: Option<[f32; 4]>,
    snow_depth_meters: f64,
) -> [f32; 4] {
    match appearance {
        SurfaceAppearance::Water => measured.unwrap_or([0.035, 0.16, 0.24, 1.0]),
        SurfaceAppearance::Terrain => {
            let measured = measured.unwrap_or([0.16, 0.28, 0.12, 0.0]);
            let weight = measured[3].clamp(0.0, 1.0);
            let base = [
                0.16 + ((measured[0] - 0.16) * weight),
                0.28 + ((measured[1] - 0.28) * weight),
                0.12 + ((measured[2] - 0.12) * weight),
            ];
            let snow = f64_as_f32((snow_depth_meters / 0.12).clamp(0.0, 1.0));
            [
                base[0] + ((0.82 - base[0]) * snow),
                base[1] + ((0.86 - base[1]) * snow),
                base[2] + ((0.88 - base[2]) * snow),
                1.0,
            ]
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_world_positions_become_small_local_vertices() {
        let mesh = Mesh {
            positions: vec![
                [5_000_000.0, 410.0, -5_000_000.0],
                [5_000_001.0, 410.0, -5_000_000.0],
                [5_000_000.0, 411.0, -5_000_000.0],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 3],
            colors: Vec::new(),
            indices: vec![0, 1, 2],
        };
        let prepared = prepare_terrain_mesh(&mesh, |_, _| None).expect("prepared mesh");
        assert!((prepared.world_origin[0] - 5_000_000.5).abs() < f64::EPSILON);
        let positions = prepared
            .mesh
            .attribute(BevyMesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("float positions");
        assert!(positions.iter().flatten().all(|value| value.abs() < 2.0));
    }
}
