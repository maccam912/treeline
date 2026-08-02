//! Meshing the distant height surface.
//!
//! Distant terrain is a surface rather than a volume, so it is meshed directly
//! from elevations. Normals use central differences that reach past the tile
//! edge, which is what makes adjacent tiles agree on both position and normal
//! at the seam between them.

use treeline_terrain::SurfaceField;

use crate::grid::{SurfaceGridSpec, validate_surface_grid};
use crate::{Mesh, MeshingError, f64_as_f32, index_as_f64, normalize};

/// Triangulates a deterministic surface-height field without sampling a volume.
///
/// Vertex normals use central differences beyond the tile boundary, so
/// adjacent tiles share both positions and normals. Optional cutouts omit only
/// whole aligned cells and are used when near voxel terrain is resident.
///
/// # Errors
///
/// Returns [`MeshingError`] when the grid is invalid or too large, a surface
/// sample is unavailable, or the mesh exceeds `u32` index capacity.
pub fn surface_grid(
    field: &impl SurfaceField,
    spec: SurfaceGridSpec,
) -> Result<Mesh, MeshingError> {
    validate_surface_grid(spec)?;
    let [cells_x, cells_z] = spec.cell_counts;
    let count_x = cells_x.checked_add(1).ok_or(MeshingError::GridTooLarge)?;
    let count_z = cells_z.checked_add(1).ok_or(MeshingError::GridTooLarge)?;
    let vertex_count = count_x
        .checked_mul(count_z)
        .ok_or(MeshingError::GridTooLarge)?;
    if u32::try_from(vertex_count).is_err() {
        return Err(MeshingError::TooManyVertices);
    }

    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    for z in 0..count_z {
        let world_z = spec.origin_z + (index_as_f64(z) * spec.spacing_meters);
        for x in 0..count_x {
            let world_x = spec.origin_x + (index_as_f64(x) * spec.spacing_meters);
            let height = field
                .surface_height(world_x, world_z)
                .ok_or(MeshingError::MissingSurface)?;
            let low_x = field
                .surface_height(world_x - spec.spacing_meters, world_z)
                .ok_or(MeshingError::MissingSurface)?;
            let high_x = field
                .surface_height(world_x + spec.spacing_meters, world_z)
                .ok_or(MeshingError::MissingSurface)?;
            let low_z = field
                .surface_height(world_x, world_z - spec.spacing_meters)
                .ok_or(MeshingError::MissingSurface)?;
            let high_z = field
                .surface_height(world_x, world_z + spec.spacing_meters)
                .ok_or(MeshingError::MissingSurface)?;
            let normal = normalize([
                f64_as_f32(low_x - high_x),
                f64_as_f32(2.0 * spec.spacing_meters),
                f64_as_f32(low_z - high_z),
            ]);
            positions.push([world_x, height, world_z]);
            normals.push(normal);
        }
    }

    let index_capacity = cells_x
        .checked_mul(cells_z)
        .and_then(|cells| cells.checked_mul(6))
        .ok_or(MeshingError::GridTooLarge)?;
    let mut indices = Vec::with_capacity(index_capacity);
    for z in 0..cells_z {
        let min_z = spec.origin_z + (index_as_f64(z) * spec.spacing_meters);
        let max_z = min_z + spec.spacing_meters;
        for x in 0..cells_x {
            let min_x = spec.origin_x + (index_as_f64(x) * spec.spacing_meters);
            let max_x = min_x + spec.spacing_meters;
            if spec
                .cutout
                .is_some_and(|cutout| cutout.contains_cell(min_x, max_x, min_z, max_z))
            {
                continue;
            }
            let top_left = z
                .checked_mul(count_x)
                .and_then(|row| row.checked_add(x))
                .ok_or(MeshingError::GridTooLarge)?;
            let bottom_left = top_left
                .checked_add(count_x)
                .ok_or(MeshingError::GridTooLarge)?;
            let top_right = top_left.checked_add(1).ok_or(MeshingError::GridTooLarge)?;
            let bottom_right = bottom_left
                .checked_add(1)
                .ok_or(MeshingError::GridTooLarge)?;
            indices.extend([
                u32::try_from(top_left).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(bottom_left).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(top_right).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(top_right).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(bottom_left).map_err(|_| MeshingError::TooManyVertices)?,
                u32::try_from(bottom_right).map_err(|_| MeshingError::TooManyVertices)?,
            ]);
        }
    }

    Ok(Mesh {
        positions,
        normals,
        colors: Vec::new(),
        indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SurfaceCutout;
    use crate::test_support::assert_front_facing;
    use std::collections::BTreeSet;
    use treeline_terrain::{GroundPlane, Material, SmoothHills};

    #[test]
    fn far_world_surface_meshes_keep_submeter_vertex_spacing() {
        let field = GroundPlane {
            surface_height: 725.25,
            material: Material::Soil,
        };
        let mesh = surface_grid(
            &field,
            SurfaceGridSpec::new(5_000_000.0, -5_000_000.0, [2, 1], 0.125),
        )
        .expect("valid far-world surface grid");

        assert!((mesh.positions[1][0] - mesh.positions[0][0] - 0.125).abs() < f64::EPSILON);
        assert!((mesh.positions[3][2] - mesh.positions[0][2] - 0.125).abs() < f64::EPSILON);
    }

    #[test]
    fn surface_grid_is_repeatable_and_faces_upward() {
        let field = SmoothHills;
        let spec = SurfaceGridSpec::new(-64.0, 128.0, [8, 8], 8.0);
        let first = surface_grid(&field, spec).expect("valid surface grid");
        let second = surface_grid(&field, spec).expect("valid surface grid");

        assert_eq!(first, second);
        assert!(first.is_well_formed());
        assert_eq!(first.indices.len(), 8 * 8 * 6);
        assert!(first.normals.iter().all(|normal| normal[1] > 0.0));
        assert_front_facing(&first);
    }

    #[test]
    fn adjacent_surface_tiles_share_positions_and_normals() {
        let field = SmoothHills;
        let left = surface_grid(&field, SurfaceGridSpec::new(-64.0, 0.0, [8, 8], 8.0))
            .expect("left surface");
        let right = surface_grid(&field, SurfaceGridSpec::new(0.0, 0.0, [8, 8], 8.0))
            .expect("right surface");

        assert_eq!(
            surface_boundary_vertices(&left, 0.0),
            surface_boundary_vertices(&right, 0.0)
        );
    }

    #[test]
    fn aligned_surface_cutout_omits_only_covered_cells() {
        let field = GroundPlane {
            surface_height: 0.0,
            material: Material::Soil,
        };
        let full = surface_grid(&field, SurfaceGridSpec::new(0.0, 0.0, [4, 4], 8.0))
            .expect("full surface");
        let cut = surface_grid(
            &field,
            SurfaceGridSpec::new(0.0, 0.0, [4, 4], 8.0)
                .with_cutout(SurfaceCutout::new(8.0, 24.0, 8.0, 24.0)),
        )
        .expect("cut surface");

        assert_eq!(full.indices.len() - cut.indices.len(), 4 * 6);
        assert!(cut.is_well_formed());
    }

    fn surface_boundary_vertices(mesh: &Mesh, boundary_x: f64) -> BTreeSet<[u64; 6]> {
        mesh.positions
            .iter()
            .zip(&mesh.normals)
            .filter(|(position, _)| (position[0] - boundary_x).abs() < f64::EPSILON)
            .map(|(position, normal)| {
                [
                    position[0].to_bits(),
                    position[1].to_bits(),
                    position[2].to_bits(),
                    u64::from(normal[0].to_bits()),
                    u64::from(normal[1].to_bits()),
                    u64::from(normal[2].to_bits()),
                ]
            })
            .collect()
    }
}
