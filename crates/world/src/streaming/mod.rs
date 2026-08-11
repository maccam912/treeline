//! Deciding which terrain is resident.
//!
//! Streaming is pure planning: given where the player is and what is already
//! loaded, it returns the meshes to build and the ones to drop. It owns no GPU
//! resources and starts no work, which keeps residency policy testable on its
//! own and independent of how fast generation actually runs.

mod far;

pub use far::{
    FarTerrainMeshSpec, FarTerrainStreamer, FarTerrainStreamingConfig, FarTerrainStreamingPlan,
    FarTileIndex, far_terrain_mesh,
};

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use treeline_coordinates::WorldPosition;
use treeline_voxel::{ChunkFace, ChunkIndex, LodLevel, TransitionFaces};

/// Near-terrain residency radii, in chunks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub struct ChunkStreamingConfig {
    load_radius: u64,
    retain_radius: u64,
    full_detail_radius: u64,
}

/// LOD levels available between [`ChunkIndex::NEAR_LOD`] and [`ChunkIndex::MAX_LOD`].
const MAX_COARSENING: u64 = (ChunkIndex::MAX_LOD.get() - ChunkIndex::NEAR_LOD.get()) as u64;

impl ChunkStreamingConfig {
    /// Creates a residency policy with a retention margin.
    ///
    /// The innermost rings hold full detail; each ring beyond them coarsens by
    /// exactly one level until the coarsest is reached. Sizing the full-detail
    /// core so the remaining rings can absorb every level is what makes
    /// adjacent chunks differ by at most one LOD — the condition transition
    /// meshes are built to bridge.
    pub const fn new(load_radius: u64, retain_radius: u64) -> Option<Self> {
        if retain_radius < load_radius {
            return None;
        }
        Some(Self {
            load_radius,
            retain_radius,
            full_detail_radius: load_radius.saturating_sub(MAX_COARSENING),
        })
    }

    pub const fn load_radius(self) -> u64 {
        self.load_radius
    }

    pub const fn retain_radius(self) -> u64 {
        self.retain_radius
    }

    /// Coarsens by one level per ring outside the full-detail core.
    ///
    /// Stepping one level at a time is what bounds the difference between
    /// neighbours, at any radius. Beyond the coarsest level the LOD simply
    /// holds, which is why the retention band can extend past the load radius
    /// without introducing a jump.
    pub const fn lod_for_distance(self, distance: u64) -> LodLevel {
        let rings_beyond_core = distance.saturating_sub(self.full_detail_radius);
        if rings_beyond_core >= MAX_COARSENING {
            return ChunkIndex::MAX_LOD;
        }
        #[allow(clippy::cast_possible_truncation)]
        LodLevel::new(ChunkIndex::NEAR_LOD.get() + rings_beyond_core as u8)
    }
}

impl Default for ChunkStreamingConfig {
    fn default() -> Self {
        Self::new(4, 5).expect("the default streaming radii are valid")
    }
}

/// Everything needed to regenerate one resident near chunk.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkMeshSpec {
    pub chunk: ChunkIndex,
    pub lod: LodLevel,
    pub transition_faces: TransitionFaces,
}

/// Which near chunks to build and which to drop.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChunkStreamingPlan {
    pub center: ChunkIndex,
    pub load: Vec<ChunkMeshSpec>,
    pub unload: Vec<ChunkIndex>,
}

/// Plans near-terrain residency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkStreamer {
    config: ChunkStreamingConfig,
}

impl ChunkStreamer {
    pub const fn new(config: ChunkStreamingConfig) -> Self {
        Self { config }
    }

    pub const fn config(self) -> ChunkStreamingConfig {
        self.config
    }

    /// Plans coarse loads first and unloads in stable coordinate order.
    ///
    /// Returns `None` only when the player position cannot map to the finite
    /// integer chunk lattice.
    pub fn plan(
        self,
        player_position: WorldPosition,
        loaded: &BTreeMap<ChunkIndex, ChunkMeshSpec>,
    ) -> Option<ChunkStreamingPlan> {
        let center = ChunkIndex::containing(player_position)?;
        let desired_lods = self.desired_lods(center, loaded)?;
        let desired = desired_lods
            .iter()
            .map(|(&chunk, &lod)| {
                (
                    chunk,
                    ChunkMeshSpec {
                        chunk,
                        lod,
                        transition_faces: transition_faces(chunk, lod, &desired_lods),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        let mut load = desired
            .values()
            .copied()
            .filter(|spec| loaded.get(&spec.chunk) != Some(spec))
            .collect::<Vec<_>>();
        // Coarse and distant first: a complete rough neighbourhood beats a
        // detailed hole next to the player.
        load.sort_by_key(|spec| {
            (
                Reverse(spec.lod),
                Reverse(spec.chunk.chebyshev_distance(center)),
                spec.chunk.z,
                spec.chunk.x,
            )
        });

        Some(ChunkStreamingPlan {
            center,
            load,
            unload: loaded
                .keys()
                .copied()
                .filter(|chunk| !desired.contains_key(chunk))
                .collect(),
        })
    }

    /// Resolves the LOD every resident chunk should hold.
    ///
    /// Chunks in the hysteresis band stay resident but are allowed to coarsen
    /// as the player moves away, so walking back and forth across a ring edge
    /// does not rebuild them at full detail.
    fn desired_lods(
        self,
        center: ChunkIndex,
        loaded: &BTreeMap<ChunkIndex, ChunkMeshSpec>,
    ) -> Option<BTreeMap<ChunkIndex, LodLevel>> {
        let load_radius = i64::try_from(self.config.load_radius).ok()?;
        let mut desired = BTreeMap::new();
        for z_offset in -load_radius..=load_radius {
            for x_offset in -load_radius..=load_radius {
                let chunk = ChunkIndex::new(
                    center.x.checked_add(x_offset)?,
                    center.z.checked_add(z_offset)?,
                );
                desired.insert(
                    chunk,
                    self.config
                        .lod_for_distance(chunk.chebyshev_distance(center)),
                );
            }
        }
        for &chunk in loaded.keys() {
            let distance = chunk.chebyshev_distance(center);
            if distance <= self.config.retain_radius {
                desired
                    .entry(chunk)
                    .or_insert_with(|| self.config.lod_for_distance(distance));
            }
        }
        Some(desired)
    }

    /// Returns meshes worth building before the player likely needs them.
    ///
    /// A moving player prewarms residency centers along their travel
    /// direction. An idle player prewarms the four adjacent centers instead, so
    /// initial loading can use otherwise idle workers without guessing.
    pub fn prefetch_specs(
        self,
        player_position: WorldPosition,
        travel_direction: [f64; 2],
        centers_ahead: u64,
    ) -> Option<Vec<ChunkMeshSpec>> {
        if centers_ahead == 0 {
            return Some(Vec::new());
        }
        let center = ChunkIndex::containing(player_position)?;
        if !travel_direction.into_iter().all(f64::is_finite) {
            return None;
        }

        let mut specs = BTreeSet::new();
        for future_center in future_centers(center, travel_direction, centers_ahead)? {
            let origin = future_center.sample_origin();
            let half_edge = ChunkIndex::edge_meters() * 0.5;
            let future_position = WorldPosition::new(
                origin.x + half_edge,
                player_position.y,
                origin.z + half_edge,
            );
            specs.extend(self.plan(future_position, &BTreeMap::new())?.load);
        }
        Some(specs.into_iter().collect())
    }
}

/// Chunk centers the player is heading toward, or those around a still player.
fn future_centers(
    center: ChunkIndex,
    travel_direction: [f64; 2],
    centers_ahead: u64,
) -> Option<Vec<ChunkIndex>> {
    let magnitude = travel_direction[0].abs().max(travel_direction[1].abs());
    if magnitude <= f64::EPSILON {
        return [(-1_i64, 0_i64), (1, 0), (0, -1), (0, 1)]
            .into_iter()
            .map(|(x_offset, z_offset)| {
                Some(ChunkIndex::new(
                    center.x.checked_add(x_offset)?,
                    center.z.checked_add(z_offset)?,
                ))
            })
            .collect();
    }

    // Quantize to one of eight directions: a heading within 60 degrees of an
    // axis counts as moving along it, so diagonal travel prewarms both.
    let step = |component: f64| -> i64 {
        if component.abs() < magnitude * 0.5 {
            0
        } else if component.is_sign_negative() {
            -1
        } else {
            1
        }
    };
    let (x_step, z_step) = (step(travel_direction[0]), step(travel_direction[1]));
    let centers_ahead = i64::try_from(centers_ahead).ok()?;
    (1..=centers_ahead)
        .map(|distance| {
            Some(ChunkIndex::new(
                center.x.checked_add(x_step.checked_mul(distance)?)?,
                center.z.checked_add(z_step.checked_mul(distance)?)?,
            ))
        })
        .collect()
}

/// Marks the faces where a chunk meets a neighbour exactly one LOD finer.
fn transition_faces(
    chunk: ChunkIndex,
    lod: LodLevel,
    desired_lods: &BTreeMap<ChunkIndex, LodLevel>,
) -> TransitionFaces {
    let mut transitions = TransitionFaces::none();
    for face in ChunkFace::ALL {
        let (x_offset, z_offset) = face.neighbour_offset();
        let Some(neighbour) = chunk
            .x
            .checked_add(x_offset)
            .zip(chunk.z.checked_add(z_offset))
            .map(|(x, z)| ChunkIndex::new(x, z))
        else {
            continue;
        };
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

    fn streamer(load: u64, retain: u64) -> ChunkStreamer {
        ChunkStreamer::new(ChunkStreamingConfig::new(load, retain).expect("valid radii"))
    }

    fn plan(streamer: ChunkStreamer, loaded: &[ChunkMeshSpec]) -> ChunkStreamingPlan {
        let loaded = loaded.iter().map(|&spec| (spec.chunk, spec)).collect();
        streamer
            .plan(WorldPosition::new(0.0, 0.0, 0.0), &loaded)
            .expect("origin is inside chunk range")
    }

    #[test]
    fn a_retention_radius_inside_the_load_radius_is_rejected() {
        assert_eq!(ChunkStreamingConfig::new(4, 3), None);
        assert!(ChunkStreamingConfig::new(4, 4).is_some());
    }

    #[test]
    fn detail_coarsens_with_distance() {
        let config = ChunkStreamingConfig::new(4, 5).expect("valid radii");
        assert_eq!(config.lod_for_distance(0), ChunkIndex::NEAR_LOD);
        assert_eq!(config.lod_for_distance(2), ChunkIndex::NEAR_LOD);
        assert_eq!(
            config.lod_for_distance(3).get(),
            ChunkIndex::NEAR_LOD.get() + 1
        );
        assert_eq!(config.lod_for_distance(4), ChunkIndex::MAX_LOD);
        assert_eq!(config.lod_for_distance(999), ChunkIndex::MAX_LOD);
    }

    #[test]
    fn an_empty_world_loads_the_whole_disc() {
        let plan = plan(streamer(2, 3), &[]);
        assert_eq!(plan.load.len(), 25);
        assert!(plan.unload.is_empty());
    }

    #[test]
    fn coarse_and_distant_chunks_are_queued_first() {
        let plan = plan(streamer(4, 5), &[]);
        let lods = plan
            .load
            .iter()
            .map(|spec| spec.lod.get())
            .collect::<Vec<_>>();
        assert!(
            lods.windows(2).all(|pair| pair[0] >= pair[1]),
            "detail must not increase through the load order"
        );
        assert_eq!(plan.load[plan.load.len() - 1].chunk, plan.center);
    }

    #[test]
    fn adjacent_chunks_never_differ_by_more_than_one_lod() {
        for load_radius in 1..=6 {
            let plan = plan(streamer(load_radius, load_radius), &[]);
            let lods = plan
                .load
                .iter()
                .map(|spec| (spec.chunk, spec.lod.get()))
                .collect::<BTreeMap<_, _>>();
            for (&chunk, &lod) in &lods {
                for face in ChunkFace::ALL {
                    let (x_offset, z_offset) = face.neighbour_offset();
                    let neighbour = ChunkIndex::new(chunk.x + x_offset, chunk.z + z_offset);
                    if let Some(&neighbour_lod) = lods.get(&neighbour) {
                        assert!(lod.abs_diff(neighbour_lod) <= 1);
                    }
                }
            }
        }
    }

    #[test]
    fn a_coarse_chunk_marks_the_faces_meeting_finer_neighbours() {
        let plan = plan(streamer(4, 5), &[]);
        let bridging = plan
            .load
            .iter()
            .filter(|spec| !spec.transition_faces.is_empty())
            .count();
        assert!(
            bridging > 0,
            "an LOD boundary must produce transition faces"
        );
    }

    #[test]
    fn chunks_inside_the_retention_band_stay_resident() {
        let resident = ChunkMeshSpec {
            chunk: ChunkIndex::new(3, 0),
            lod: ChunkIndex::MAX_LOD,
            transition_faces: TransitionFaces::none(),
        };
        assert!(plan(streamer(2, 3), &[resident]).unload.is_empty());
    }

    #[test]
    fn chunks_beyond_the_retention_band_are_dropped() {
        let stale = ChunkMeshSpec {
            chunk: ChunkIndex::new(4, 0),
            lod: ChunkIndex::MAX_LOD,
            transition_faces: TransitionFaces::none(),
        };
        assert_eq!(
            plan(streamer(2, 3), &[stale]).unload,
            vec![ChunkIndex::new(4, 0)]
        );
    }

    #[test]
    fn planning_is_idempotent() {
        let streamer = streamer(2, 3);
        let loaded = plan(streamer, &[]).load;
        let settled = plan(streamer, &loaded);

        assert!(settled.load.is_empty());
        assert!(settled.unload.is_empty());
    }

    #[test]
    fn planning_is_independent_of_how_residency_was_reached() {
        let streamer = streamer(2, 3);
        let mut forward = plan(streamer, &[]).load;
        forward.sort_unstable();
        let mut reversed = plan(streamer, &[]).load;
        reversed.reverse();
        let mut settled = plan(streamer, &reversed).load;
        settled.sort_unstable();

        assert!(settled.is_empty());
        assert!(!forward.is_empty());
    }

    #[test]
    fn a_still_player_prewarms_every_adjacent_center() {
        let specs = streamer(1, 2)
            .prefetch_specs(WorldPosition::new(0.0, 0.0, 0.0), [0.0, 0.0], 2)
            .expect("origin is inside chunk range");
        let chunks = specs.iter().map(|spec| spec.chunk).collect::<BTreeSet<_>>();

        for adjacent in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            assert!(chunks.contains(&ChunkIndex::new(adjacent.0, adjacent.1)));
        }
    }

    #[test]
    fn a_moving_player_prewarms_ahead_and_not_behind() {
        let specs = streamer(1, 2)
            .prefetch_specs(WorldPosition::new(0.0, 0.0, 0.0), [1.0, 0.0], 2)
            .expect("origin is inside chunk range");
        let chunks = specs.iter().map(|spec| spec.chunk).collect::<BTreeSet<_>>();

        assert!(chunks.contains(&ChunkIndex::new(3, 0)));
        assert!(!chunks.contains(&ChunkIndex::new(-3, 0)));
    }

    #[test]
    fn prefetching_nothing_is_allowed_and_non_finite_travel_is_rejected() {
        assert_eq!(
            streamer(1, 2).prefetch_specs(WorldPosition::new(0.0, 0.0, 0.0), [1.0, 0.0], 0),
            Some(Vec::new())
        );
        assert_eq!(
            streamer(1, 2).prefetch_specs(WorldPosition::new(0.0, 0.0, 0.0), [f64::NAN, 0.0], 2),
            None
        );
    }
}
