//! Streaming-world lifecycle and deterministic terrain-LOD planning.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use treeline_coordinates::WorldPosition;
use treeline_voxel::{ChunkFace, ChunkIndex, LodLevel, TransitionFaces};

/// Lifecycle of one region in an effectively infinite world.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RegionState {
    #[default]
    Ungenerated,
    Generated,
    Active,
    Frozen,
}

impl RegionState {
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Ungenerated, Self::Generated)
                | (Self::Generated, Self::Active | Self::Frozen)
                | (Self::Active, Self::Frozen)
                | (Self::Frozen, Self::Active)
        )
    }
}

/// Job tiers make distant terrain visible before near-world detail finishes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GenerationPriority {
    Horizon,
    FarTerrain,
    NearTerrain,
    Vegetation,
    SurfaceDetail,
}

/// Validated near-terrain residency radii measured in chunks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkStreamingConfig {
    load_radius: u64,
    retain_radius: u64,
    near_lod_radius: u64,
    middle_lod_radius: u64,
}

impl ChunkStreamingConfig {
    /// Creates a three-ring policy with a retention margin.
    ///
    /// The two outermost load rings use successively coarser meshes. This
    /// guarantees that adjacent chunks differ by no more than one LOD even for
    /// very small residency radii.
    pub const fn new(load_radius: u64, retain_radius: u64) -> Option<Self> {
        if retain_radius < load_radius {
            return None;
        }
        Some(Self {
            load_radius,
            retain_radius,
            near_lod_radius: load_radius.saturating_sub(2),
            middle_lod_radius: load_radius.saturating_sub(1),
        })
    }

    pub const fn load_radius(self) -> u64 {
        self.load_radius
    }

    pub const fn retain_radius(self) -> u64 {
        self.retain_radius
    }

    pub const fn lod_for_distance(self, distance: u64) -> LodLevel {
        if distance <= self.near_lod_radius {
            ChunkIndex::NEAR_LOD
        } else if distance <= self.middle_lod_radius {
            LodLevel::new(ChunkIndex::NEAR_LOD.get() + 1)
        } else {
            ChunkIndex::MAX_LOD
        }
    }
}

impl Default for ChunkStreamingConfig {
    fn default() -> Self {
        Self::new(4, 5).expect("the default streaming radii are valid")
    }
}

/// Complete inputs needed to deterministically regenerate one resident mesh.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkMeshSpec {
    pub chunk: ChunkIndex,
    pub lod: LodLevel,
    pub transition_faces: TransitionFaces,
}

/// Deterministic changes needed to reconcile loaded chunks with player position.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChunkStreamingPlan {
    pub center: ChunkIndex,
    pub load: Vec<ChunkMeshSpec>,
    pub unload: Vec<ChunkIndex>,
}

/// Computes near-terrain residency without owning generation or GPU resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkStreamer {
    config: ChunkStreamingConfig,
}

impl ChunkStreamer {
    pub const fn new(config: ChunkStreamingConfig) -> Self {
        Self { config }
    }

    /// Plans coarse loads first and unloads in stable coordinate order.
    ///
    /// Returns `None` only when the player position cannot map to the finite
    /// integer chunk index space.
    pub fn plan(
        self,
        player_position: WorldPosition,
        loaded: &BTreeMap<ChunkIndex, ChunkMeshSpec>,
    ) -> Option<ChunkStreamingPlan> {
        let center = ChunkIndex::containing(player_position)?;
        let load_radius = i64::try_from(self.config.load_radius).ok()?;
        let mut desired_lods = BTreeMap::new();

        for z_offset in -load_radius..=load_radius {
            for x_offset in -load_radius..=load_radius {
                let chunk = ChunkIndex::new(
                    center.x.checked_add(x_offset)?,
                    center.z.checked_add(z_offset)?,
                );
                desired_lods.insert(
                    chunk,
                    self.config
                        .lod_for_distance(chunk.chebyshev_distance(center)),
                );
            }
        }

        // Keep the hysteresis band resident, but allow it to become coarse as
        // it moves away from the player.
        for &chunk in loaded.keys() {
            let distance = chunk.chebyshev_distance(center);
            if distance <= self.config.retain_radius {
                desired_lods
                    .entry(chunk)
                    .or_insert_with(|| self.config.lod_for_distance(distance));
            }
        }

        let desired = desired_lods
            .iter()
            .map(|(&chunk, &lod)| {
                let transition_faces = transition_faces(chunk, lod, &desired_lods);
                (
                    chunk,
                    ChunkMeshSpec {
                        chunk,
                        lod,
                        transition_faces,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        let mut load = desired
            .values()
            .copied()
            .filter(|spec| loaded.get(&spec.chunk) != Some(spec))
            .collect::<Vec<_>>();
        load.sort_by_key(|spec| {
            (
                Reverse(spec.lod),
                Reverse(spec.chunk.chebyshev_distance(center)),
                spec.chunk.z,
                spec.chunk.x,
            )
        });

        let unload = loaded
            .keys()
            .copied()
            .filter(|chunk| !desired.contains_key(chunk))
            .collect();

        Some(ChunkStreamingPlan {
            center,
            load,
            unload,
        })
    }
}

fn transition_faces(
    chunk: ChunkIndex,
    lod: LodLevel,
    desired_lods: &BTreeMap<ChunkIndex, LodLevel>,
) -> TransitionFaces {
    let mut transitions = TransitionFaces::none();
    for face in ChunkFace::ALL {
        let (x_offset, z_offset) = face.neighbour_offset();
        let Some(neighbour_x) = chunk.x.checked_add(x_offset) else {
            continue;
        };
        let Some(neighbour_z) = chunk.z.checked_add(z_offset) else {
            continue;
        };
        let neighbour = ChunkIndex::new(neighbour_x, neighbour_z);
        let Some(neighbour_lod) = desired_lods.get(&neighbour) else {
            continue;
        };
        if lod.get() == neighbour_lod.get().saturating_add(1) {
            transitions = transitions.with(face);
        }
    }
    transitions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_regions_freeze_but_do_not_become_ungenerated() {
        assert!(RegionState::Active.can_transition_to(RegionState::Frozen));
        assert!(!RegionState::Active.can_transition_to(RegionState::Ungenerated));
    }

    #[test]
    fn horizon_jobs_sort_before_surface_detail() {
        assert!(GenerationPriority::Horizon < GenerationPriority::SurfaceDetail);
    }

    #[test]
    fn initial_plan_loads_nearest_chunk_first() {
        let streamer = ChunkStreamer::new(ChunkStreamingConfig::default());
        let plan = streamer
            .plan(WorldPosition::new(0.0, 0.0, 0.0), &BTreeMap::new())
            .expect("finite position");
        assert_eq!(plan.center, ChunkIndex::new(0, 0));
        assert_eq!(plan.load.len(), 81);
        assert_eq!(plan.load[0].lod, ChunkIndex::MAX_LOD);
        assert_eq!(
            plan.load
                .iter()
                .find(|spec| spec.chunk == plan.center)
                .expect("center spec")
                .lod,
            ChunkIndex::NEAR_LOD
        );
        assert!(plan.unload.is_empty());
    }

    #[test]
    fn crossing_a_boundary_loads_only_the_new_edge() {
        let streamer = ChunkStreamer::new(ChunkStreamingConfig::new(1, 1).expect("valid radii"));
        let origin_plan = streamer
            .plan(WorldPosition::new(0.0, 0.0, 0.0), &BTreeMap::new())
            .expect("finite position");
        let loaded = specs_by_chunk(origin_plan.load);
        let moved = streamer
            .plan(
                WorldPosition::new(ChunkIndex::edge_meters(), 0.0, 0.0),
                &loaded,
            )
            .expect("finite position");

        assert_eq!(moved.center, ChunkIndex::new(1, 0));
        assert!(moved.load.len() >= 3);
        assert_eq!(moved.unload.len(), 3);
        assert!(
            moved
                .load
                .iter()
                .filter(|spec| !loaded.contains_key(&spec.chunk))
                .all(|spec| spec.chunk.x == 2)
        );
        assert!(moved.unload.iter().all(|chunk| chunk.x == -1));
    }

    #[test]
    fn retention_margin_prevents_boundary_thrashing() {
        let streamer = ChunkStreamer::new(ChunkStreamingConfig::default());
        let loaded = specs_by_chunk(
            streamer
                .plan(WorldPosition::new(0.0, 0.0, 0.0), &BTreeMap::new())
                .expect("finite position")
                .load,
        );
        let moved = streamer
            .plan(
                WorldPosition::new(ChunkIndex::edge_meters(), 0.0, 0.0),
                &loaded,
            )
            .expect("finite position");
        assert!(moved.load.len() >= 9);
        assert!(moved.unload.is_empty());
    }

    #[test]
    fn invalid_streaming_radii_are_rejected() {
        assert!(ChunkStreamingConfig::new(3, 2).is_none());
    }

    #[test]
    fn lod_rings_are_aligned_and_never_skip_a_level() {
        let streamer = ChunkStreamer::new(ChunkStreamingConfig::default());
        let plan = streamer
            .plan(WorldPosition::new(0.0, 0.0, 0.0), &BTreeMap::new())
            .expect("finite position");
        let specs = specs_by_chunk(plan.load);

        for spec in specs.values() {
            assert!(ChunkIndex::subdivisions(spec.lod).is_some());
            for face in ChunkFace::ALL {
                let (x_offset, z_offset) = face.neighbour_offset();
                let neighbour = ChunkIndex::new(spec.chunk.x + x_offset, spec.chunk.z + z_offset);
                if let Some(neighbour) = specs.get(&neighbour) {
                    assert!(spec.lod.get().abs_diff(neighbour.lod.get()) <= 1);
                }
            }
        }
    }

    #[test]
    fn only_coarse_chunks_transition_toward_finer_neighbours() {
        let streamer = ChunkStreamer::new(ChunkStreamingConfig::default());
        let plan = streamer
            .plan(WorldPosition::new(0.0, 0.0, 0.0), &BTreeMap::new())
            .expect("finite position");
        let specs = specs_by_chunk(plan.load);
        let coarse = specs
            .get(&ChunkIndex::new(3, 0))
            .expect("middle ring chunk");
        assert_eq!(coarse.lod, LodLevel::new(3));
        assert!(coarse.transition_faces.contains(ChunkFace::LowX));
        assert!(!coarse.transition_faces.contains(ChunkFace::HighX));

        let fine = specs.get(&ChunkIndex::new(2, 0)).expect("near ring chunk");
        assert_eq!(fine.lod, ChunkIndex::NEAR_LOD);
        assert!(fine.transition_faces.is_empty());
    }

    fn specs_by_chunk(specs: Vec<ChunkMeshSpec>) -> BTreeMap<ChunkIndex, ChunkMeshSpec> {
        specs.into_iter().map(|spec| (spec.chunk, spec)).collect()
    }
}
