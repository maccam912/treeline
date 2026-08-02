//! Least-recently-used cache of completed terrain meshes.
//!
//! Terrain is regenerated rather than persisted, and walking a short loop
//! revisits the same chunks constantly. Caching by [`TerrainMeshSpec`] is safe
//! precisely because a mesh is a pure function of its spec: a hit and a
//! regeneration are indistinguishable apart from timing.

use std::collections::BTreeMap;
use std::mem::size_of;

use treeline_mesher::Mesh;
use web_time::Duration;

use crate::mesh::{GeneratedTerrainMesh, GenerationPriority, TerrainMeshSpec};

/// Cache budget. Browsers get less because the whole heap is smaller there.
#[cfg(not(target_arch = "wasm32"))]
pub const DEFAULT_CACHE_BYTES: usize = 192 * 1024 * 1024;
#[cfg(target_arch = "wasm32")]
pub const DEFAULT_CACHE_BYTES: usize = 48 * 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_CACHE_ENTRIES: usize = 2_048;
#[cfg(target_arch = "wasm32")]
const DEFAULT_CACHE_ENTRIES: usize = 512;

#[derive(Clone, Debug)]
struct CachedMesh {
    generated: GeneratedTerrainMesh,
    bytes: usize,
    last_used: u64,
}

#[derive(Debug)]
pub struct TerrainMeshCache {
    entries: BTreeMap<TerrainMeshSpec, CachedMesh>,
    used_bytes: usize,
    max_bytes: usize,
    max_entries: usize,
    clock: u64,
}

impl TerrainMeshCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            used_bytes: 0,
            max_bytes,
            max_entries: DEFAULT_CACHE_ENTRIES,
            clock: 0,
        }
    }

    pub fn contains(&self, spec: TerrainMeshSpec) -> bool {
        self.entries.contains_key(&spec)
    }

    /// Takes a copy of a cached mesh, re-tagged for the requesting priority.
    ///
    /// Generation times are zeroed and `cache_hit` set, so timing reports
    /// measure real generation rather than counting cached work twice.
    pub fn get(
        &mut self,
        spec: TerrainMeshSpec,
        priority: GenerationPriority,
    ) -> Option<GeneratedTerrainMesh> {
        self.clock = self.clock.wrapping_add(1);
        let cached = self.entries.get_mut(&spec)?;
        cached.last_used = self.clock;
        Some(GeneratedTerrainMesh {
            priority,
            terrain_generation_time: Duration::ZERO,
            lake_generation_time: Duration::ZERO,
            cache_hit: true,
            ..cached.generated.clone()
        })
    }

    /// Stores a successful result, evicting least-recently-used entries.
    ///
    /// Failures are never cached: a mesh that failed should be retried, not
    /// remembered.
    pub fn insert(&mut self, generated: &GeneratedTerrainMesh) {
        if generated.mesh.is_err() || generated.lake_mesh.as_ref().is_some_and(Result::is_err) {
            return;
        }
        let bytes = mesh_bytes(generated);
        if bytes > self.max_bytes {
            return;
        }

        self.clock = self.clock.wrapping_add(1);
        if let Some(previous) = self.entries.remove(&generated.spec) {
            self.used_bytes = self.used_bytes.saturating_sub(previous.bytes);
        }
        self.used_bytes = self.used_bytes.saturating_add(bytes);
        self.entries.insert(
            generated.spec,
            CachedMesh {
                generated: generated.clone(),
                bytes,
                last_used: self.clock,
            },
        );
        self.evict_until_within_budget();
    }

    /// Drops the oldest entries until both the byte and entry budgets hold.
    ///
    /// Ties break on spec order so eviction is deterministic.
    fn evict_until_within_budget(&mut self) {
        while self.used_bytes > self.max_bytes || self.entries.len() > self.max_entries {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(spec, cached)| (cached.last_used, *spec))
                .map(|(&spec, _)| spec)
            else {
                return;
            };
            let Some(removed) = self.entries.remove(&oldest) else {
                return;
            };
            self.used_bytes = self.used_bytes.saturating_sub(removed.bytes);
        }
    }
}

/// Approximate heap footprint of one completed result.
fn mesh_bytes(generated: &GeneratedTerrainMesh) -> usize {
    fn bytes(mesh: &Mesh) -> usize {
        mesh.positions
            .len()
            .saturating_mul(size_of::<[f64; 3]>())
            .saturating_add(mesh.normals.len().saturating_mul(size_of::<[f32; 3]>()))
            .saturating_add(mesh.colors.len().saturating_mul(size_of::<[f32; 4]>()))
            .saturating_add(mesh.indices.len().saturating_mul(size_of::<u32>()))
    }

    let terrain = generated.mesh.as_ref().map_or(0, bytes);
    let lake = generated
        .lake_mesh
        .as_ref()
        .and_then(|mesh| mesh.as_ref().ok())
        .map_or(0, bytes);
    terrain.saturating_add(lake)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::{ChunkMeshSpec, FarTerrainMeshSpec, FarTileIndex};
    use treeline_mesher::MeshingError;
    use treeline_voxel::{ChunkIndex, TransitionFaces};

    fn spec(x: i64) -> TerrainMeshSpec {
        TerrainMeshSpec::Near(ChunkMeshSpec {
            chunk: ChunkIndex::new(x, 0),
            lod: ChunkIndex::NEAR_LOD,
            transition_faces: TransitionFaces::none(),
        })
    }

    fn generated(spec: TerrainMeshSpec, vertices: usize) -> GeneratedTerrainMesh {
        GeneratedTerrainMesh {
            spec,
            priority: GenerationPriority::NearTerrain,
            mesh: Ok(Mesh {
                positions: vec![[0.0; 3]; vertices],
                normals: vec![[0.0, 1.0, 0.0]; vertices],
                colors: Vec::new(),
                indices: Vec::new(),
            }),
            lake_mesh: None,
            terrain_generation_time: Duration::from_millis(7),
            lake_generation_time: Duration::ZERO,
            cache_hit: false,
        }
    }

    #[test]
    fn a_hit_reports_the_requested_priority_and_no_generation_time() {
        let mut cache = TerrainMeshCache::new(DEFAULT_CACHE_BYTES);
        cache.insert(&generated(spec(0), 4));

        let hit = cache
            .get(spec(0), GenerationPriority::Horizon)
            .expect("cached mesh");
        assert!(hit.cache_hit);
        assert_eq!(hit.priority, GenerationPriority::Horizon);
        assert_eq!(hit.terrain_generation_time, Duration::ZERO);
    }

    #[test]
    fn a_miss_is_reported_as_absent() {
        let mut cache = TerrainMeshCache::new(DEFAULT_CACHE_BYTES);
        assert!(!cache.contains(spec(0)));
        assert!(
            cache
                .get(spec(0), GenerationPriority::NearTerrain)
                .is_none()
        );
    }

    #[test]
    fn failures_are_not_cached() {
        let mut cache = TerrainMeshCache::new(DEFAULT_CACHE_BYTES);
        let mut failed = generated(spec(0), 4);
        failed.mesh = Err(MeshingError::MissingSurface);
        cache.insert(&failed);

        assert!(!cache.contains(spec(0)));
    }

    #[test]
    fn the_least_recently_used_entry_is_evicted_first() {
        // Two four-vertex meshes fit; a third forces one out.
        let entry_bytes = (size_of::<[f64; 3]>() + size_of::<[f32; 3]>()) * 4;
        let mut cache = TerrainMeshCache::new(entry_bytes * 2);
        cache.insert(&generated(spec(0), 4));
        cache.insert(&generated(spec(1), 4));
        let _touch = cache.get(spec(0), GenerationPriority::NearTerrain);
        cache.insert(&generated(spec(2), 4));

        assert!(cache.contains(spec(0)));
        assert!(!cache.contains(spec(1)));
        assert!(cache.contains(spec(2)));
    }

    #[test]
    fn a_mesh_larger_than_the_whole_budget_is_refused() {
        let mut cache = TerrainMeshCache::new(64);
        cache.insert(&generated(spec(0), 1_000));

        assert!(!cache.contains(spec(0)));
    }

    #[test]
    fn near_and_far_specs_do_not_collide() {
        let mut cache = TerrainMeshCache::new(DEFAULT_CACHE_BYTES);
        let far = TerrainMeshSpec::Far(FarTerrainMeshSpec {
            tile: FarTileIndex::new(0, 0),
        });
        cache.insert(&generated(spec(0), 4));
        cache.insert(&generated(far, 4));

        assert!(cache.contains(spec(0)));
        assert!(cache.contains(far));
    }
}
