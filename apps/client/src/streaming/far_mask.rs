//! Removing coarse far geometry once detailed near geometry covers it.

use std::collections::BTreeSet;

use bevy::mesh::Indices;
use bevy::prelude::{Assets, Handle, Mesh as BevyMesh};
use treeline_coordinates::WorldPosition;
use treeline_mesher::Mesh;
use treeline_voxel::ChunkIndex;

#[derive(Debug)]
pub(super) struct FarMeshMask {
    mesh: Handle<BevyMesh>,
    world_positions: Vec<[f64; 3]>,
    source_indices: Vec<u32>,
    covered_chunks: BTreeSet<ChunkIndex>,
}

impl FarMeshMask {
    pub(super) fn new(mesh: Handle<BevyMesh>, source: &Mesh) -> Self {
        Self {
            mesh,
            world_positions: source.positions.clone(),
            source_indices: source.indices.clone(),
            covered_chunks: BTreeSet::new(),
        }
    }

    pub(super) fn update(&mut self, meshes: &mut Assets<BevyMesh>, covered: &BTreeSet<ChunkIndex>) {
        if self.covered_chunks == *covered {
            return;
        }
        self.covered_chunks.clone_from(covered);
        let Some(mut mesh) = meshes.get_mut(&self.mesh) else {
            return;
        };
        mesh.insert_indices(Indices::U32(visible_indices(
            &self.world_positions,
            &self.source_indices,
            covered,
        )));
    }
}

fn visible_indices(
    positions: &[[f64; 3]],
    indices: &[u32],
    covered: &BTreeSet<ChunkIndex>,
) -> Vec<u32> {
    indices
        .chunks_exact(3)
        .filter(|triangle| {
            !triangle.iter().all(|&index| {
                positions
                    .get(index as usize)
                    .is_some_and(|position| position_is_covered(*position, covered))
            })
        })
        .flatten()
        .copied()
        .collect()
}

fn position_is_covered(position: [f64; 3], covered: &BTreeSet<ChunkIndex>) -> bool {
    let Some(chunk) = ChunkIndex::containing(WorldPosition::new(position[0], 0.0, position[2]))
    else {
        return false;
    };
    let origin = chunk.sample_origin();
    let x_offsets: &[i64] = if (position[0] - origin.x).abs() < 1.0e-9 {
        &[0, -1]
    } else {
        &[0]
    };
    let z_offsets: &[i64] = if (position[2] - origin.z).abs() < 1.0e-9 {
        &[0, -1]
    } else {
        &[0]
    };
    x_offsets.iter().any(|x| {
        z_offsets
            .iter()
            .any(|z| covered.contains(&ChunkIndex::new(chunk.x + x, chunk.z + z)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangles_fully_covered_by_near_terrain_are_hidden() {
        let positions = vec![
            [1.0, 10.0, 1.0],
            [8.0, 10.0, 1.0],
            [1.0, 10.0, 8.0],
            [33.0, 10.0, 1.0],
            [40.0, 10.0, 1.0],
            [33.0, 10.0, 8.0],
        ];
        let source = vec![0, 1, 2, 3, 4, 5];
        let covered = BTreeSet::from([ChunkIndex::new(0, 0)]);

        assert_eq!(
            visible_indices(&positions, &source, &covered),
            vec![3, 4, 5]
        );
    }

    #[test]
    fn shared_chunk_edges_count_as_covered_from_either_side() {
        let edge = ChunkIndex::edge_meters();
        let covered = BTreeSet::from([ChunkIndex::new(0, 0)]);

        assert!(position_is_covered([edge, 0.0, edge], &covered));
    }
}
