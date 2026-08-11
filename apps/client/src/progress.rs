//! Reporting how much of the world is ready.
//!
//! The window title carries progress while the world builds, because there is
//! no in-game UI yet. Counts are tracked per tier so a slow horizon and a slow
//! foreground are distinguishable.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::{Resource, Window};
use treeline_coordinates::WorldPosition;
use treeline_voxel::ChunkIndex;
use treeline_world::{
    ChunkMeshSpec, FarTerrainMeshSpec, FarTerrainStreamer, FarTileIndex, GeneratedTerrainMesh,
    TerrainMeshSpec,
};
use web_time::{Duration, Instant};

use crate::WINDOW_TITLE;

/// Progress of one round of world building, from spawn or from a warp.
#[derive(Debug, Resource)]
pub struct LoadProgress {
    started: Instant,
    /// Outer-ring far tiles, which set the visible horizon.
    horizon_tiles: Tier<FarTileIndex>,
    far_tiles: Tier<FarTileIndex>,
    near_chunks: Tier<ChunkIndex>,
    terrain_generation_time: Duration,
    lake_generation_time: Duration,
    integration_time: Duration,
    discarded_jobs: usize,
    dirty: bool,
    finished_at: Option<Duration>,
    reported: bool,
}

/// Expected and completed work for one terrain tier.
#[derive(Debug, Default)]
struct Tier<T> {
    expected: BTreeSet<T>,
    completed: BTreeSet<T>,
}

impl<T: Ord> Tier<T> {
    fn complete(&mut self, item: T) -> bool {
        self.expected.contains(&item) && self.completed.insert(item)
    }

    fn is_done(&self) -> bool {
        self.completed.len() == self.expected.len()
    }
}

impl LoadProgress {
    /// Begins tracking the work already requested for a new player position.
    ///
    /// Returns `None` when the position is outside the far-tile lattice.
    pub fn new(
        started: Instant,
        requested_chunks: &BTreeMap<ChunkIndex, ChunkMeshSpec>,
        requested_far_tiles: &BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
        far_streamer: FarTerrainStreamer,
        player_position: WorldPosition,
    ) -> Option<Self> {
        let center = FarTileIndex::containing(player_position)?;
        let horizon_radius = far_streamer.config().load_radius();
        let (horizon, nearer) = requested_far_tiles
            .keys()
            .copied()
            .partition(|tile| tile.chebyshev_distance(center) == horizon_radius);

        Some(Self {
            started,
            horizon_tiles: Tier {
                expected: horizon,
                completed: BTreeSet::new(),
            },
            far_tiles: Tier {
                expected: nearer,
                completed: BTreeSet::new(),
            },
            near_chunks: Tier {
                expected: requested_chunks.keys().copied().collect(),
                completed: BTreeSet::new(),
            },
            terrain_generation_time: Duration::ZERO,
            lake_generation_time: Duration::ZERO,
            integration_time: Duration::ZERO,
            discarded_jobs: 0,
            dirty: true,
            finished_at: None,
            reported: false,
        })
    }

    /// Counts work that finished after the streamer stopped wanting it.
    pub fn record_discarded(&mut self, generated: &GeneratedTerrainMesh) {
        if self.finished_at.is_some() {
            return;
        }
        self.terrain_generation_time += generated.terrain_generation_time;
        self.lake_generation_time += generated.lake_generation_time;
        self.discarded_jobs += 1;
    }

    /// Counts one mesh that became resident.
    pub fn record_completed(
        &mut self,
        spec: TerrainMeshSpec,
        terrain_generation_time: Duration,
        lake_generation_time: Duration,
        integration_time: Duration,
    ) {
        if self.finished_at.is_some() {
            return;
        }
        self.terrain_generation_time += terrain_generation_time;
        self.lake_generation_time += lake_generation_time;
        self.integration_time += integration_time;

        self.dirty |= match spec {
            TerrainMeshSpec::Far(spec) => {
                self.horizon_tiles.complete(spec.tile) || self.far_tiles.complete(spec.tile)
            }
            TerrainMeshSpec::Near(spec) => self.near_chunks.complete(spec.chunk),
        };
        if self.horizon_tiles.is_done() && self.far_tiles.is_done() && self.near_chunks.is_done() {
            self.finished_at = Some(self.started.elapsed());
            self.dirty = true;
        }
    }

    /// Updates the window title, and reports timings once when the world is up.
    pub fn publish(&mut self, window: &mut Window) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(wall_time) = self.finished_at else {
            window.title = self.title();
            return;
        };
        window.title = WINDOW_TITLE.into();
        if self.reported {
            return;
        }
        self.reported = true;
        eprintln!(
            "world ready in {:.2}s: worker terrain {:.2}s, worker water {:.2}s, \
             main-thread upload {:.2}s, {} discarded jobs",
            wall_time.as_secs_f64(),
            self.terrain_generation_time.as_secs_f64(),
            self.lake_generation_time.as_secs_f64(),
            self.integration_time.as_secs_f64(),
            self.discarded_jobs
        );
    }

    fn title(&self) -> String {
        format!(
            "Treeline — Building world: horizon {}/{} · far {}/{} · nearby {}/{}",
            self.horizon_tiles.completed.len(),
            self.horizon_tiles.expected.len(),
            self.far_tiles.completed.len(),
            self.far_tiles.expected.len(),
            self.near_chunks.completed.len(),
            self.near_chunks.expected.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_voxel::TransitionFaces;
    use treeline_world::FarTerrainStreamingConfig;

    fn chunk_spec(x: i64) -> ChunkMeshSpec {
        ChunkMeshSpec {
            chunk: ChunkIndex::new(x, 0),
            lod: ChunkIndex::NEAR_LOD,
            transition_faces: TransitionFaces::none(),
        }
    }

    fn progress(chunks: &[i64]) -> LoadProgress {
        LoadProgress::new(
            Instant::now(),
            &chunks
                .iter()
                .map(|&x| (ChunkIndex::new(x, 0), chunk_spec(x)))
                .collect(),
            &BTreeMap::new(),
            FarTerrainStreamer::new(FarTerrainStreamingConfig::default()),
            WorldPosition::new(0.0, 0.0, 0.0),
        )
        .expect("origin is inside far tile range")
    }

    #[test]
    fn the_title_counts_completed_work_per_tier() {
        let mut progress = progress(&[0, 1]);
        assert!(progress.title().contains("nearby 0/2"));

        progress.record_completed(
            TerrainMeshSpec::Near(chunk_spec(0)),
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        );
        assert!(progress.title().contains("nearby 1/2"));
    }

    #[test]
    fn work_is_finished_only_once_every_tier_is_complete() {
        let mut progress = progress(&[0, 1]);
        for x in [0, 1] {
            assert!(progress.finished_at.is_none());
            progress.record_completed(
                TerrainMeshSpec::Near(chunk_spec(x)),
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
            );
        }
        assert!(progress.finished_at.is_some());
    }

    #[test]
    fn a_repeated_completion_does_not_double_count() {
        let mut progress = progress(&[0, 1]);
        for _ in 0..3 {
            progress.record_completed(
                TerrainMeshSpec::Near(chunk_spec(0)),
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
            );
        }
        assert!(progress.title().contains("nearby 1/2"));
        assert!(progress.finished_at.is_none());
    }

    #[test]
    fn work_that_was_never_requested_is_ignored() {
        let mut progress = progress(&[0]);
        progress.record_completed(
            TerrainMeshSpec::Near(chunk_spec(99)),
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        );
        assert!(progress.title().contains("nearby 0/1"));
    }
}
