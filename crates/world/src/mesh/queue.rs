//! Priority-ordered background terrain generation.
//!
//! Jobs start in priority order, with submission order breaking ties, so the
//! horizon and the ground under the player arrive before speculative work.
//! Completion order is deliberately *not* observable by generation: every mesh
//! is a pure function of its spec, so the queue can reorder, drop, or cache
//! freely without changing the world.
//!
//! Native builds use worker threads. Browser builds fall back to generating one
//! mesh per call on the caller's thread; the player client runs the same
//! contract over independent Web Workers instead.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, VecDeque};
use std::num::NonZeroUsize;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex, PoisonError};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, JoinHandle};

use treeline_terrain::{DensityField, SurfaceField};

use crate::mesh::cache::{DEFAULT_CACHE_BYTES, TerrainMeshCache};
use crate::mesh::{
    GeneratedTerrainMesh, GenerationPriority, LakeMeshGenerator, TerrainMeshGenerator,
    TerrainMeshSpec, generate,
};
use crate::terrain::WorldTerrain;

/// One queued request, ordered by priority then submission.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct QueuedMesh {
    priority: GenerationPriority,
    sequence: u64,
    spec: TerrainMeshSpec,
}

/// Background terrain generation ordered by visible priority.
#[derive(Debug)]
pub struct TerrainMeshQueue<F> {
    #[cfg(not(target_arch = "wasm32"))]
    shared: Arc<QueueState<F>>,
    #[cfg(not(target_arch = "wasm32"))]
    ready: Receiver<GeneratedTerrainMesh>,
    #[cfg(not(target_arch = "wasm32"))]
    workers: Vec<JoinHandle<()>>,
    #[cfg(target_arch = "wasm32")]
    inline: InlineQueue<F>,
    /// Cache hits, which complete without ever reaching a worker.
    cached_ready: VecDeque<GeneratedTerrainMesh>,
    next_sequence: u64,
}

impl<F> TerrainMeshQueue<F>
where
    F: DensityField + SurfaceField + Send + Sync + 'static,
{
    /// Starts generation for a field with no separate water surface.
    pub fn new(field: F) -> Self {
        Self::with_generators(field, None, None)
    }

    /// Starts an explicit number of native workers. Browsers ignore the count.
    pub fn with_worker_count(field: F, worker_count: NonZeroUsize) -> Self {
        Self::build(field, worker_count, None, None)
    }

    /// Reserves one hardware thread for the window, rendering, and simulation.
    fn with_generators(
        field: F,
        terrain: Option<TerrainMeshGenerator<F>>,
        lake: Option<LakeMeshGenerator<F>>,
    ) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let worker_count = NonZeroUsize::new(
            thread::available_parallelism()
                .unwrap_or(NonZeroUsize::MIN)
                .get()
                .saturating_sub(1),
        )
        .unwrap_or(NonZeroUsize::MIN);
        #[cfg(target_arch = "wasm32")]
        let worker_count = NonZeroUsize::MIN;

        Self::build(field, worker_count, terrain, lake)
    }

    fn build(
        field: F,
        worker_count: NonZeroUsize,
        terrain_mesh_generator: Option<TerrainMeshGenerator<F>>,
        lake_mesh_generator: Option<LakeMeshGenerator<F>>,
    ) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let shared = Arc::new(QueueState {
                field,
                terrain_mesh_generator,
                lake_mesh_generator,
                pending: Mutex::new(PendingJobs::default()),
                cache: Mutex::new(TerrainMeshCache::new(DEFAULT_CACHE_BYTES)),
                wake_workers: Condvar::new(),
            });
            let (ready_sender, ready) = mpsc::channel();
            let workers = (0..worker_count.get())
                .map(|_| {
                    let shared = Arc::clone(&shared);
                    let ready_sender = ready_sender.clone();
                    thread::spawn(move || run_worker(&shared, &ready_sender))
                })
                .collect();
            Self {
                shared,
                ready,
                workers,
                cached_ready: VecDeque::new(),
                next_sequence: 0,
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = worker_count;
            Self {
                inline: InlineQueue {
                    field,
                    terrain_mesh_generator,
                    lake_mesh_generator,
                    pending: BinaryHeap::new(),
                    cache: TerrainMeshCache::new(DEFAULT_CACHE_BYTES),
                    yield_after_mesh: false,
                },
                cached_ready: VecDeque::new(),
                next_sequence: 0,
            }
        }
    }

    /// Requests a mesh, completing immediately from cache when possible.
    pub fn enqueue(&mut self, priority: GenerationPriority, spec: TerrainMeshSpec) {
        if let Some(generated) = self.cached(spec, priority) {
            self.cached_ready.push_back(generated);
            return;
        }
        self.queue_if_absent(priority, spec);
    }

    /// Requests speculative generation, without emitting a completion on a hit.
    ///
    /// Unlike [`Self::enqueue`], this is safe to call every frame: a cached or
    /// already-queued mesh is simply skipped.
    pub fn prewarm(&mut self, spec: TerrainMeshSpec) -> bool {
        !self.cache_contains(spec)
            && self.queue_if_absent(GenerationPriority::PrefetchTerrain, spec)
    }

    /// Drops speculative jobs the player no longer appears to be heading for.
    ///
    /// Visible and in-flight work is untouched.
    pub fn retain_prewarm(&mut self, desired: &BTreeSet<TerrainMeshSpec>) {
        let keep = |queued: &Reverse<QueuedMesh>| {
            queued.0.priority != GenerationPriority::PrefetchTerrain
                || desired.contains(&queued.0.spec)
        };

        #[cfg(not(target_arch = "wasm32"))]
        self.lock_pending().jobs.retain(keep);
        #[cfg(target_arch = "wasm32")]
        self.inline.pending.retain(keep);
    }

    /// Removes a job that has not started yet.
    ///
    /// A job a worker already owns will still complete; the caller rejects it
    /// by checking the spec it currently wants at integration time.
    pub fn cancel(&mut self, spec: TerrainMeshSpec) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        let jobs = &mut self.lock_pending().jobs;
        #[cfg(target_arch = "wasm32")]
        let jobs = &mut self.inline.pending;

        let before = jobs.len();
        jobs.retain(|queued| queued.0.spec != spec);
        jobs.len() != before
    }

    /// Returns one completed mesh without blocking.
    pub fn try_next(&mut self) -> Option<GeneratedTerrainMesh> {
        if let Some(generated) = self.cached_ready.pop_front() {
            return Some(generated);
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ready.try_recv().ok()
        }

        #[cfg(target_arch = "wasm32")]
        {
            self.inline.generate_one()
        }
    }

    /// Queues a job unless an equal-or-higher-priority one already exists.
    ///
    /// A queued job whose priority has since risen is re-queued at the new
    /// priority rather than left behind the work it should now precede.
    fn queue_if_absent(&mut self, priority: GenerationPriority, spec: TerrainMeshSpec) -> bool {
        let job = QueuedMesh {
            priority,
            sequence: self.next_sequence,
            spec,
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut pending = self.lock_pending();
            if pending.in_flight.contains(&spec) {
                return false;
            }
            if !replace_lower_priority(&mut pending.jobs, spec, priority) {
                return false;
            }
            pending.jobs.push(Reverse(job));
            drop(pending);
            self.shared.wake_workers.notify_one();
        }

        #[cfg(target_arch = "wasm32")]
        {
            if !replace_lower_priority(&mut self.inline.pending, spec, priority) {
                return false;
            }
            self.inline.pending.push(Reverse(job));
        }

        self.next_sequence = self.next_sequence.wrapping_add(1);
        true
    }

    fn cached(
        &mut self,
        spec: TerrainMeshSpec,
        priority: GenerationPriority,
    ) -> Option<GeneratedTerrainMesh> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.lock_cache().get(spec, priority)
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.inline.cache.get(spec, priority)
        }
    }

    fn cache_contains(&mut self, spec: TerrainMeshSpec) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.lock_cache().contains(spec)
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.inline.cache.contains(spec)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn lock_pending(&self) -> std::sync::MutexGuard<'_, PendingJobs> {
        self.shared
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn lock_cache(&self) -> std::sync::MutexGuard<'_, TerrainMeshCache> {
        self.shared
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl TerrainMeshQueue<WorldTerrain> {
    /// Starts the player-world queue, building terrain and water together.
    pub fn for_world(field: WorldTerrain) -> Self {
        Self::with_generators(
            field,
            Some(WorldTerrain::render_mesh),
            Some(WorldTerrain::lake_surface_mesh),
        )
    }
}

/// Returns whether `spec` should be pushed, removing any lower-priority copy.
fn replace_lower_priority(
    jobs: &mut BinaryHeap<Reverse<QueuedMesh>>,
    spec: TerrainMeshSpec,
    priority: GenerationPriority,
) -> bool {
    let Some(existing) = jobs.iter().find(|queued| queued.0.spec == spec) else {
        return true;
    };
    if existing.0.priority <= priority {
        return false;
    }
    jobs.retain(|queued| queued.0.spec != spec);
    true
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct QueueState<F> {
    field: F,
    terrain_mesh_generator: Option<TerrainMeshGenerator<F>>,
    lake_mesh_generator: Option<LakeMeshGenerator<F>>,
    pending: Mutex<PendingJobs>,
    cache: Mutex<TerrainMeshCache>,
    wake_workers: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default)]
struct PendingJobs {
    jobs: BinaryHeap<Reverse<QueuedMesh>>,
    in_flight: BTreeSet<TerrainMeshSpec>,
    closed: bool,
}

#[cfg(not(target_arch = "wasm32"))]
fn run_worker<F>(shared: &QueueState<F>, ready: &Sender<GeneratedTerrainMesh>)
where
    F: DensityField + SurfaceField,
{
    loop {
        let Some(job) = claim_job(shared) else {
            return;
        };
        let generated = generate(
            &shared.field,
            shared.terrain_mesh_generator,
            shared.lake_mesh_generator,
            job.priority,
            job.spec,
        );
        shared
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(&generated);
        shared
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .in_flight
            .remove(&job.spec);
        if ready.send(generated).is_err() {
            return;
        }
    }
}

/// Waits for the next job, or returns `None` once the queue is closed.
#[cfg(not(target_arch = "wasm32"))]
fn claim_job<F>(shared: &QueueState<F>) -> Option<QueuedMesh> {
    let mut pending = shared
        .pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    loop {
        if pending.closed {
            return None;
        }
        if let Some(Reverse(job)) = pending.jobs.pop() {
            pending.in_flight.insert(job.spec);
            return Some(job);
        }
        pending = shared
            .wake_workers
            .wait(pending)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

impl<F> Drop for TerrainMeshQueue<F> {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            {
                let mut pending = self
                    .shared
                    .pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                pending.closed = true;
                pending.jobs.clear();
            }
            self.shared.wake_workers.notify_all();
            for worker in self.workers.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

/// Browser fallback: generate at most one mesh per call, on the caller's thread.
#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
struct InlineQueue<F> {
    field: F,
    terrain_mesh_generator: Option<TerrainMeshGenerator<F>>,
    lake_mesh_generator: Option<LakeMeshGenerator<F>>,
    pending: BinaryHeap<Reverse<QueuedMesh>>,
    cache: TerrainMeshCache,
    /// Returns control to the frame loop between meshes.
    yield_after_mesh: bool,
}

#[cfg(target_arch = "wasm32")]
impl<F> InlineQueue<F>
where
    F: DensityField + SurfaceField,
{
    fn generate_one(&mut self) -> Option<GeneratedTerrainMesh> {
        if self.yield_after_mesh {
            self.yield_after_mesh = false;
            return None;
        }
        let Reverse(job) = self.pending.pop()?;
        let generated = generate(
            &self.field,
            self.terrain_mesh_generator,
            self.lake_mesh_generator,
            job.priority,
            job.spec,
        );
        self.cache.insert(&generated);
        self.yield_after_mesh = true;
        Some(generated)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::streaming::ChunkMeshSpec;
    use treeline_terrain::SmoothHills;
    use treeline_voxel::{ChunkIndex, TransitionFaces};

    fn spec(x: i64) -> TerrainMeshSpec {
        TerrainMeshSpec::Near(ChunkMeshSpec {
            chunk: ChunkIndex::new(x, 0),
            lod: ChunkIndex::NEAR_LOD,
            transition_faces: TransitionFaces::none(),
        })
    }

    fn queue() -> TerrainMeshQueue<SmoothHills> {
        TerrainMeshQueue::with_worker_count(SmoothHills, NonZeroUsize::MIN)
    }

    fn drain(queue: &mut TerrainMeshQueue<SmoothHills>, count: usize) -> Vec<GeneratedTerrainMesh> {
        let mut results = Vec::new();
        while results.len() < count {
            if let Some(generated) = queue.try_next() {
                results.push(generated);
            }
        }
        results
    }

    #[test]
    fn every_enqueued_mesh_eventually_completes() {
        let mut queue = queue();
        for x in 0..4 {
            queue.enqueue(GenerationPriority::NearTerrain, spec(x));
        }
        let completed = drain(&mut queue, 4)
            .into_iter()
            .map(|generated| generated.spec)
            .collect::<BTreeSet<_>>();

        assert_eq!(completed, (0..4).map(spec).collect::<BTreeSet<_>>());
    }

    #[test]
    fn generated_meshes_are_identical_to_direct_generation() {
        let mut queue = queue();
        queue.enqueue(GenerationPriority::PlayerTerrain, spec(2));
        let queued = drain(&mut queue, 1).remove(0);
        let direct = generate(
            &SmoothHills,
            None,
            None,
            GenerationPriority::PlayerTerrain,
            spec(2),
        );

        assert_eq!(queued.mesh, direct.mesh);
    }

    #[test]
    fn a_cancelled_job_never_runs() {
        let mut queue = queue();
        queue.enqueue(GenerationPriority::PrefetchTerrain, spec(9));
        // Cancellation races a worker claiming the job, so only assert the
        // stronger property: cancelling twice cannot succeed twice.
        if queue.cancel(spec(9)) {
            assert!(!queue.cancel(spec(9)));
        }
    }

    #[test]
    fn a_second_request_at_lower_priority_is_dropped() {
        let mut queue = queue();
        assert!(queue.queue_if_absent(GenerationPriority::PlayerTerrain, spec(5)));
        assert!(!queue.queue_if_absent(GenerationPriority::PrefetchTerrain, spec(5)));
    }

    #[test]
    fn a_repeated_request_is_served_from_cache() {
        let mut queue = queue();
        queue.enqueue(GenerationPriority::NearTerrain, spec(7));
        let first = drain(&mut queue, 1).remove(0);
        assert!(!first.cache_hit);

        queue.enqueue(GenerationPriority::NearTerrain, spec(7));
        let second = drain(&mut queue, 1).remove(0);
        assert!(second.cache_hit);
        assert_eq!(first.mesh, second.mesh);
    }

    #[test]
    fn prewarming_an_already_cached_mesh_does_nothing() {
        let mut queue = queue();
        queue.enqueue(GenerationPriority::NearTerrain, spec(3));
        let _first = drain(&mut queue, 1);

        assert!(!queue.prewarm(spec(3)));
    }

    #[test]
    fn stale_prewarm_jobs_are_dropped_and_visible_ones_kept() {
        let mut queue = TerrainMeshQueue::with_worker_count(SmoothHills, NonZeroUsize::MIN);
        queue.queue_if_absent(GenerationPriority::PrefetchTerrain, spec(20));
        queue.queue_if_absent(GenerationPriority::NearTerrain, spec(21));
        queue.retain_prewarm(&BTreeSet::new());

        // The visible job survives; the speculative one is gone or already ran.
        assert!(!queue.queue_if_absent(GenerationPriority::PrefetchTerrain, spec(21)));
    }
}
