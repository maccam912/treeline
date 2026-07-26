//! Streaming-world lifecycle and deterministic terrain-LOD planning.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::num::NonZeroUsize;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, PoisonError, RwLock};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, JoinHandle};

use treeline_coordinates::{WorldIdentity, WorldPosition};
use treeline_ecology::FOREST_GENERATOR_VERSION;
use treeline_geography::{DrainageCellIndex, WatershedRegionIndex};
use treeline_hydrology::{
    GullyNetwork, GullyTerrainInfluence, Lake, LakeNetwork, RiverNetwork, RiverTerrainInfluence,
};
use treeline_mesher::{
    Mesh, MeshingError, SurfaceCutout, SurfaceGridSpec, surface_grid, transvoxel_chunk,
};
use treeline_terrain::{
    DensityField, ErosionSurfaceSample, Material, SurfaceField, TerrainSample, WildernessTerrain,
};
use treeline_voxel::{ChunkFace, ChunkIndex, LodLevel, TransitionFaces};

/// Generator version that first makes regional rivers shape terrain.
pub const RIVER_TERRAIN_GENERATOR_VERSION: u32 = 3;
/// Generator version that first exposes filled drainage basins as lakes.
pub const LAKE_GENERATOR_VERSION: u32 = 4;
/// Generator version that first composes macro, meso, and micro erosion.
pub const EROSION_GENERATOR_VERSION: u32 = 5;
/// Latest generator contract used for newly created prototype worlds.
pub const CURRENT_GENERATOR_VERSION: u32 = FOREST_GENERATOR_VERSION;

/// Equilibrium lake water at one horizontal world position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LakeSurfaceSample {
    pub lake: Lake,
    pub terrain_elevation_meters: f64,
    pub water_depth_meters: f64,
}

/// Explainable contributors to the versioned multi-scale erosion surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldErosionSample {
    pub surface: ErosionSurfaceSample,
    pub gully: Option<GullyTerrainInfluence>,
    pub river: Option<RiverTerrainInfluence>,
    pub final_height_meters: f64,
}

/// Pristine terrain composed with cached deterministic regional artifacts.
///
/// Watershed generation is much more expensive than a density sample, so all
/// clones share an immutable river-network cache. Cache contents affect only
/// performance: every entry is a pure function of world identity and region.
#[derive(Clone, Debug)]
pub struct GeneratedWorldTerrain {
    base: WildernessTerrain,
    river_networks: Arc<RwLock<BTreeMap<WatershedRegionIndex, Arc<RiverNetwork>>>>,
    gully_networks: Arc<RwLock<BTreeMap<WatershedRegionIndex, Arc<GullyNetwork>>>>,
    lake_networks: Arc<RwLock<BTreeMap<WatershedRegionIndex, Arc<LakeNetwork>>>>,
}

impl GeneratedWorldTerrain {
    pub fn new(world: WorldIdentity) -> Self {
        Self {
            base: WildernessTerrain::new(world),
            river_networks: Arc::new(RwLock::new(BTreeMap::new())),
            gully_networks: Arc::new(RwLock::new(BTreeMap::new())),
            lake_networks: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub const fn world(&self) -> WorldIdentity {
        self.base.world
    }

    /// Returns the strongest nearby river contribution, if terrain carving is
    /// enabled by this world's generation version.
    pub fn river_influence_at(&self, x: f64, z: f64) -> Option<RiverTerrainInfluence> {
        if self.world().generator_version < RIVER_TERRAIN_GENERATOR_VERSION {
            return None;
        }
        let containing = DrainageCellIndex::containing(x, z)?;
        let mut strongest = None;
        for z_offset in -1_i64..=1 {
            for x_offset in -1_i64..=1 {
                let source = DrainageCellIndex::new(
                    containing.x.checked_add(x_offset)?,
                    containing.z.checked_add(z_offset)?,
                );
                let region = WatershedRegionIndex::containing_cell(source);
                let network = self.river_network(region)?;
                let Some(influence) = network
                    .segment_from(source)
                    .and_then(|segment| segment.terrain_influence(x, z))
                else {
                    continue;
                };
                let carve_strength = influence.blend * influence.incision_depth_meters;
                if strongest.is_none_or(|current: RiverTerrainInfluence| {
                    let current_strength = current.blend * current.incision_depth_meters;
                    carve_strength > current_strength
                        || (carve_strength.to_bits() == current_strength.to_bits()
                            && influence.segment.source_cell < current.segment.source_cell)
                }) {
                    strongest = Some(influence);
                }
            }
        }
        strongest
    }

    /// Returns the strongest nearby minor-drainage contribution.
    pub fn gully_influence_at(&self, x: f64, z: f64) -> Option<GullyTerrainInfluence> {
        if self.world().generator_version < EROSION_GENERATOR_VERSION {
            return None;
        }
        let containing = DrainageCellIndex::containing(x, z)?;
        let mut strongest = None;
        for z_offset in -1_i64..=1 {
            for x_offset in -1_i64..=1 {
                let source = DrainageCellIndex::new(
                    containing.x.checked_add(x_offset)?,
                    containing.z.checked_add(z_offset)?,
                );
                let region = WatershedRegionIndex::containing_cell(source);
                let network = self.gully_network(region)?;
                let Some(influence) = network
                    .segment_from(source)
                    .and_then(|segment| segment.terrain_influence(x, z))
                else {
                    continue;
                };
                let carve_strength = influence.blend * influence.segment.incision_depth_meters;
                if strongest.is_none_or(|current: GullyTerrainInfluence| {
                    let current_strength = current.blend * current.segment.incision_depth_meters;
                    carve_strength > current_strength
                        || (carve_strength.to_bits() == current_strength.to_bits()
                            && influence.segment.source_cell < current.segment.source_cell)
                }) {
                    strongest = Some(influence);
                }
            }
        }
        strongest
    }

    fn river_network(&self, region: WatershedRegionIndex) -> Option<Arc<RiverNetwork>> {
        if let Some(network) = self
            .river_networks
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&region)
            .cloned()
        {
            return Some(network);
        }

        let mut cache = self
            .river_networks
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(network) = cache.get(&region) {
            return Some(Arc::clone(network));
        }
        let generated = Arc::new(RiverNetwork::generate(self.world(), region)?);
        Some(
            cache
                .entry(region)
                .or_insert_with(|| Arc::clone(&generated))
                .clone(),
        )
    }

    fn gully_network(&self, region: WatershedRegionIndex) -> Option<Arc<GullyNetwork>> {
        if let Some(network) = self
            .gully_networks
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&region)
            .cloned()
        {
            return Some(network);
        }

        let mut cache = self
            .gully_networks
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(network) = cache.get(&region) {
            return Some(Arc::clone(network));
        }
        let generated = Arc::new(GullyNetwork::generate(self.world(), region)?);
        Some(
            cache
                .entry(region)
                .or_insert_with(|| Arc::clone(&generated))
                .clone(),
        )
    }

    /// Returns equilibrium lake water above the generated terrain, if present.
    pub fn lake_surface_at(&self, x: f64, z: f64) -> Option<LakeSurfaceSample> {
        if self.world().generator_version < LAKE_GENERATOR_VERSION {
            return None;
        }
        let cell = DrainageCellIndex::containing(x, z)?;
        let network = self.lake_network(WatershedRegionIndex::containing_cell(cell))?;
        let lake = network.lake_for_cell(cell)?;
        let terrain_elevation_meters = self.shaped_height(x, z)?.height;
        let water_depth_meters = lake.water_depth_at(terrain_elevation_meters)?;
        (water_depth_meters > 0.0).then_some(LakeSurfaceSample {
            lake,
            terrain_elevation_meters,
            water_depth_meters,
        })
    }

    fn lake_network(&self, region: WatershedRegionIndex) -> Option<Arc<LakeNetwork>> {
        if let Some(network) = self
            .lake_networks
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&region)
            .cloned()
        {
            return Some(network);
        }

        let mut cache = self
            .lake_networks
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(network) = cache.get(&region) {
            return Some(Arc::clone(network));
        }
        let generated = Arc::new(LakeNetwork::generate(self.world(), region)?);
        Some(
            cache
                .entry(region)
                .or_insert_with(|| Arc::clone(&generated))
                .clone(),
        )
    }

    /// Builds the lake surface aligned with one near or far terrain mesh.
    ///
    /// Lake water remains a separate render surface; it never changes the
    /// signed terrain density or the far-terrain height contract.
    ///
    /// # Errors
    ///
    /// Returns [`MeshingError`] when the requested LOD is unsupported, the
    /// surface grid is invalid, or the generated mesh exceeds index capacity.
    pub fn lake_surface_mesh(&self, spec: TerrainMeshSpec) -> Result<Mesh, MeshingError> {
        let grid = match spec {
            TerrainMeshSpec::Far(spec) => spec.surface_grid(),
            TerrainMeshSpec::Near(spec) => {
                let subdivisions =
                    ChunkIndex::subdivisions(spec.lod).ok_or(MeshingError::UnsupportedLod)?;
                let origin = spec.chunk.sample_origin();
                SurfaceGridSpec::new(
                    origin.x,
                    origin.z,
                    [subdivisions; 2],
                    ChunkIndex::edge_meters() / usize_as_f64(subdivisions),
                )
            }
        };
        lake_surface_grid(self, grid)
    }

    /// Reports all three erosion scales at a horizontal position.
    pub fn erosion_at(&self, x: f64, z: f64) -> Option<WorldErosionSample> {
        if self.world().generator_version < EROSION_GENERATOR_VERSION {
            return None;
        }
        let shape = self.shaped_height(x, z)?;
        Some(WorldErosionSample {
            surface: shape.erosion?,
            gully: shape.gully,
            river: shape.river,
            final_height_meters: shape.height,
        })
    }

    fn shaped_height(&self, x: f64, z: f64) -> Option<TerrainShape> {
        let erosion = (self.world().generator_version >= EROSION_GENERATOR_VERSION)
            .then(|| self.base.erosion_at(x, z))
            .flatten();
        let base_height = erosion.map_or_else(
            || self.base.height_at(x, z),
            |sample| Some(sample.surface_height_meters()),
        )?;
        let gully = self.gully_influence_at(x, z);
        let gully_height = gully.map_or(base_height, |influence| {
            let channel_bed = base_height.min(influence.centerline_elevation_meters)
                - influence.segment.incision_depth_meters;
            base_height + ((channel_bed - base_height) * influence.blend)
        });
        let river = self.river_influence_at(x, z);
        let height = river.map_or(gully_height, |river| {
            let channel_bed = river.centerline_elevation_meters - river.incision_depth_meters;
            let target = gully_height.min(channel_bed);
            gully_height + ((target - gully_height) * river.blend)
        });
        Some(TerrainShape {
            height,
            erosion,
            gully,
            river,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct TerrainShape {
    height: f64,
    erosion: Option<ErosionSurfaceSample>,
    gully: Option<GullyTerrainInfluence>,
    river: Option<RiverTerrainInfluence>,
}

impl DensityField for GeneratedWorldTerrain {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        let Some(shape) = self.shaped_height(position.x, position.z) else {
            return TerrainSample::new(f64::INFINITY, Material::Air);
        };
        let density = position.y - shape.height;
        let material = if density > 0.0 {
            Material::Air
        } else if shape.river.is_some_and(|influence| {
            influence.distance_meters <= influence.channel_half_width_meters && density > -1.0
        }) {
            Material::Sand
        } else if shape
            .erosion
            .is_some_and(|erosion| erosion.scree_cover >= 0.3 && density > -1.2)
        {
            Material::Scree
        } else if shape
            .erosion
            .is_some_and(|erosion| erosion.rock_exposure >= 0.4 && density > -1.0)
        {
            Material::Rock
        } else if density
            > -shape
                .erosion
                .map_or(1.5, |erosion| erosion.soil_depth_meters)
        {
            Material::Soil
        } else {
            Material::Rock
        };
        TerrainSample::new(density, material)
    }
}

impl SurfaceField for GeneratedWorldTerrain {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        self.shaped_height(x, z).map(|shape| shape.height)
    }
}

fn lake_surface_grid(
    terrain: &GeneratedWorldTerrain,
    spec: SurfaceGridSpec,
) -> Result<Mesh, MeshingError> {
    const WATER_RENDER_OFFSET_METERS: f64 = 0.05;
    const WATER_COLOR: [f32; 4] = [0.04, 0.34, 0.58, 1.0];

    if spec.cell_counts.contains(&0)
        || !spec.origin_x.is_finite()
        || !spec.origin_z.is_finite()
        || !spec.spacing_meters.is_finite()
        || spec.spacing_meters <= 0.0
    {
        return Err(MeshingError::InvalidGrid);
    }

    let [cells_x, cells_z] = spec.cell_counts;
    let mut mesh = Mesh::default();
    for z in 0..cells_z {
        let min_z = spec.origin_z + (usize_as_f64(z) * spec.spacing_meters);
        let max_z = min_z + spec.spacing_meters;
        for x in 0..cells_x {
            let min_x = spec.origin_x + (usize_as_f64(x) * spec.spacing_meters);
            let max_x = min_x + spec.spacing_meters;
            if spec
                .cutout
                .is_some_and(|cutout| cutout.contains_cell(min_x, max_x, min_z, max_z))
            {
                continue;
            }
            let center_x = (min_x + max_x) * 0.5;
            let center_z = (min_z + max_z) * 0.5;
            let Some(water) = terrain.lake_surface_at(center_x, center_z) else {
                continue;
            };
            let surface = water.lake.surface_elevation_meters + WATER_RENDER_OFFSET_METERS;
            let vertex_offset =
                u32::try_from(mesh.positions.len()).map_err(|_| MeshingError::TooManyVertices)?;
            mesh.positions.extend([
                [f64_as_f32(min_x), f64_as_f32(surface), f64_as_f32(min_z)],
                [f64_as_f32(min_x), f64_as_f32(surface), f64_as_f32(max_z)],
                [f64_as_f32(max_x), f64_as_f32(surface), f64_as_f32(min_z)],
                [f64_as_f32(max_x), f64_as_f32(surface), f64_as_f32(max_z)],
            ]);
            mesh.normals.extend([[0.0, 1.0, 0.0]; 4]);
            mesh.colors.extend([WATER_COLOR; 4]);
            mesh.indices.extend([
                vertex_offset,
                vertex_offset
                    .checked_add(1)
                    .ok_or(MeshingError::TooManyVertices)?,
                vertex_offset
                    .checked_add(2)
                    .ok_or(MeshingError::TooManyVertices)?,
                vertex_offset
                    .checked_add(2)
                    .ok_or(MeshingError::TooManyVertices)?,
                vertex_offset
                    .checked_add(1)
                    .ok_or(MeshingError::TooManyVertices)?,
                vertex_offset
                    .checked_add(3)
                    .ok_or(MeshingError::TooManyVertices)?,
            ]);
        }
    }
    Ok(mesh)
}

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
    PlayerTerrain,
    Horizon,
    FarTerrain,
    NearTerrain,
    Vegetation,
    SurfaceDetail,
}

/// Complete inputs needed to regenerate either terrain representation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TerrainMeshSpec {
    Far(FarTerrainMeshSpec),
    Near(ChunkMeshSpec),
}

/// Completed output from one asynchronous terrain-mesh job.
#[derive(Debug)]
pub struct GeneratedTerrainMesh {
    pub spec: TerrainMeshSpec,
    pub mesh: Result<Mesh, MeshingError>,
}

/// Terrain generation queue ordered by visible generation priority.
///
/// Jobs already being generated are allowed to finish. Pending jobs always
/// start in priority order, with submission order breaking ties. Completion
/// order is deliberately not observable by generation itself: every mesh is a
/// pure function of its field and [`ChunkMeshSpec`]. Native builds use worker
/// threads; browser builds complete one queued mesh at a time between event-loop
/// turns.
#[derive(Debug)]
pub struct TerrainMeshQueue<F> {
    #[cfg(not(target_arch = "wasm32"))]
    shared: Arc<QueueState<F>>,
    #[cfg(not(target_arch = "wasm32"))]
    ready: Receiver<GeneratedTerrainMesh>,
    #[cfg(not(target_arch = "wasm32"))]
    workers: Vec<JoinHandle<()>>,
    #[cfg(target_arch = "wasm32")]
    field: F,
    #[cfg(target_arch = "wasm32")]
    pending: BinaryHeap<Reverse<QueuedTerrainMesh>>,
    #[cfg(target_arch = "wasm32")]
    yield_after_mesh: bool,
    next_sequence: u64,
}

impl<F> TerrainMeshQueue<F>
where
    F: DensityField + SurfaceField + Send + Sync + 'static,
{
    /// Starts native workers while reserving one available hardware thread for
    /// the window, rendering, and simulation work.
    ///
    /// Browser builds retain the same priority queue but generate incrementally
    /// on the main thread because GitHub Pages does not provide the headers
    /// required for shared-memory WebAssembly threads.
    pub fn new(field: F) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let available = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
            let worker_count =
                NonZeroUsize::new(available.get().saturating_sub(1)).unwrap_or(NonZeroUsize::MIN);
            Self::with_worker_count(field, worker_count)
        }

        #[cfg(target_arch = "wasm32")]
        {
            Self::with_worker_count(field, NonZeroUsize::MIN)
        }
    }

    /// Starts an explicit non-zero number of native terrain workers.
    ///
    /// Browser builds ignore `worker_count` and use their incremental queue.
    pub fn with_worker_count(field: F, worker_count: NonZeroUsize) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let shared = Arc::new(QueueState {
                field,
                pending: Mutex::new(PendingJobs::default()),
                wake_workers: Condvar::new(),
            });
            let (ready_sender, ready) = mpsc::channel();
            let workers = (0..worker_count.get())
                .map(|_| {
                    let shared = Arc::clone(&shared);
                    let ready_sender = ready_sender.clone();
                    thread::spawn(move || terrain_worker(&shared, &ready_sender))
                })
                .collect();

            Self {
                shared,
                ready,
                workers,
                next_sequence: 0,
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = worker_count;
            Self {
                field,
                pending: BinaryHeap::new(),
                yield_after_mesh: false,
                next_sequence: 0,
            }
        }
    }

    /// Adds a deterministic chunk request without blocking for generation.
    pub fn enqueue(&mut self, priority: GenerationPriority, spec: TerrainMeshSpec) {
        let job = QueuedTerrainMesh {
            priority,
            sequence: self.next_sequence,
            spec,
        };
        self.next_sequence = self.next_sequence.wrapping_add(1);

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.shared
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .jobs
                .push(Reverse(job));
            self.shared.wake_workers.notify_one();
        }

        #[cfg(target_arch = "wasm32")]
        self.pending.push(Reverse(job));
    }

    /// Returns one completed mesh without waiting for a worker.
    pub fn try_next(&mut self) -> Option<GeneratedTerrainMesh> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ready.try_recv().ok()
        }

        #[cfg(target_arch = "wasm32")]
        {
            if self.yield_after_mesh {
                self.yield_after_mesh = false;
                return None;
            }
            let Reverse(job) = self.pending.pop()?;
            let mesh = match job.spec {
                TerrainMeshSpec::Far(spec) => far_terrain_mesh(&self.field, spec),
                TerrainMeshSpec::Near(spec) => {
                    transvoxel_chunk(&self.field, spec.chunk, spec.lod, spec.transition_faces)
                }
            };
            self.yield_after_mesh = true;
            Some(GeneratedTerrainMesh {
                spec: job.spec,
                mesh,
            })
        }
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

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct QueueState<F> {
    field: F,
    pending: Mutex<PendingJobs>,
    wake_workers: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default)]
struct PendingJobs {
    jobs: BinaryHeap<Reverse<QueuedTerrainMesh>>,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct QueuedTerrainMesh {
    priority: GenerationPriority,
    sequence: u64,
    spec: TerrainMeshSpec,
}

#[cfg(not(target_arch = "wasm32"))]
fn terrain_worker<F>(shared: &QueueState<F>, ready: &Sender<GeneratedTerrainMesh>)
where
    F: DensityField + SurfaceField,
{
    loop {
        let job = {
            let mut pending = shared
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            while pending.jobs.is_empty() && !pending.closed {
                pending = shared
                    .wake_workers
                    .wait(pending)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if pending.closed {
                return;
            }
            let Some(Reverse(job)) = pending.jobs.pop() else {
                continue;
            };
            job
        };
        let mesh = match job.spec {
            TerrainMeshSpec::Far(spec) => far_terrain_mesh(&shared.field, spec),
            TerrainMeshSpec::Near(spec) => {
                transvoxel_chunk(&shared.field, spec.chunk, spec.lod, spec.transition_faces)
            }
        };
        if ready
            .send(GeneratedTerrainMesh {
                spec: job.spec,
                mesh,
            })
            .is_err()
        {
            return;
        }
    }
}

/// Validated near-terrain residency radii measured in chunks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
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

/// Stable identity of one surface-only far-terrain tile.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FarTileIndex {
    pub x: i64,
    pub z: i64,
}

impl FarTileIndex {
    /// A far tile spans sixty-four near chunks, or 2,048 meters.
    pub const CHUNKS_PER_EDGE: i64 = 64;
    /// Sixty-four-meter surface samples retain mountain silhouettes while
    /// avoiding volumetric work at vista distance.
    pub const CELLS_PER_EDGE: usize = 32;

    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    pub fn edge_meters() -> f64 {
        ChunkIndex::edge_meters() * i64_as_f64(Self::CHUNKS_PER_EDGE)
    }

    pub fn containing(position: WorldPosition) -> Option<Self> {
        let chunk = ChunkIndex::containing(position)?;
        Some(Self::new(
            chunk.x.div_euclid(Self::CHUNKS_PER_EDGE),
            chunk.z.div_euclid(Self::CHUNKS_PER_EDGE),
        ))
    }

    pub fn chebyshev_distance(self, other: Self) -> u64 {
        self.x.abs_diff(other.x).max(self.z.abs_diff(other.z))
    }

    fn origin(self) -> (f64, f64) {
        let edge = Self::edge_meters();
        (i64_as_f64(self.x) * edge, i64_as_f64(self.z) * edge)
    }

    fn intersects(self, cutout: NearTerrainCutout) -> bool {
        let (tile_min_x, tile_min_z) = self.origin();
        let tile_max_x = tile_min_x + Self::edge_meters();
        let tile_max_z = tile_min_z + Self::edge_meters();
        let bounds = cutout.world_bounds();
        tile_min_x < bounds.max_x
            && tile_max_x > bounds.min_x
            && tile_min_z < bounds.max_z
            && tile_max_z > bounds.min_z
    }
}

/// Half-open chunk rectangle covered by a complete near-terrain residency set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NearTerrainCutout {
    pub min: ChunkIndex,
    pub max_exclusive: ChunkIndex,
}

impl NearTerrainCutout {
    pub const fn new(min: ChunkIndex, max_exclusive: ChunkIndex) -> Option<Self> {
        if min.x >= max_exclusive.x || min.z >= max_exclusive.z {
            return None;
        }
        Some(Self { min, max_exclusive })
    }

    pub fn around(center: ChunkIndex, radius: u64) -> Option<Self> {
        let radius = i64::try_from(radius).ok()?;
        Some(Self {
            min: ChunkIndex::new(center.x.checked_sub(radius)?, center.z.checked_sub(radius)?),
            max_exclusive: ChunkIndex::new(
                center.x.checked_add(radius)?.checked_add(1)?,
                center.z.checked_add(radius)?.checked_add(1)?,
            ),
        })
    }

    fn world_bounds(self) -> SurfaceCutout {
        let edge = ChunkIndex::edge_meters();
        SurfaceCutout::new(
            i64_as_f64(self.min.x) * edge,
            i64_as_f64(self.max_exclusive.x) * edge,
            i64_as_f64(self.min.z) * edge,
            i64_as_f64(self.max_exclusive.z) * edge,
        )
    }
}

/// Complete deterministic inputs for one coarse, surface-only terrain tile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FarTerrainMeshSpec {
    pub tile: FarTileIndex,
    pub near_cutout: Option<NearTerrainCutout>,
}

impl FarTerrainMeshSpec {
    fn surface_grid(self) -> SurfaceGridSpec {
        let (origin_x, origin_z) = self.tile.origin();
        let mut grid = SurfaceGridSpec::new(
            origin_x,
            origin_z,
            [FarTileIndex::CELLS_PER_EDGE; 2],
            FarTileIndex::edge_meters() / usize_as_f64(FarTileIndex::CELLS_PER_EDGE),
        );
        if let Some(cutout) = self.near_cutout {
            grid = grid.with_cutout(cutout.world_bounds());
        }
        grid
    }
}

fn far_terrain_mesh(
    field: &impl SurfaceField,
    spec: FarTerrainMeshSpec,
) -> Result<Mesh, MeshingError> {
    surface_grid(field, spec.surface_grid())
}

/// Validated far-terrain residency radii measured in 2,048-meter tiles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FarTerrainStreamingConfig {
    load_radius: u64,
    retain_radius: u64,
}

impl FarTerrainStreamingConfig {
    pub const fn new(load_radius: u64, retain_radius: u64) -> Option<Self> {
        if retain_radius < load_radius {
            return None;
        }
        Some(Self {
            load_radius,
            retain_radius,
        })
    }

    pub const fn load_radius(self) -> u64 {
        self.load_radius
    }
}

impl Default for FarTerrainStreamingConfig {
    fn default() -> Self {
        Self::new(10, 11).expect("the default far-terrain radii are valid")
    }
}

/// Deterministic changes needed to reconcile coarse surface tiles.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FarTerrainStreamingPlan {
    pub center: FarTileIndex,
    pub load: Vec<FarTerrainMeshSpec>,
    pub unload: Vec<FarTileIndex>,
}

/// Plans surface-only terrain independently of voxel chunk residency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FarTerrainStreamer {
    config: FarTerrainStreamingConfig,
}

impl FarTerrainStreamer {
    pub const fn new(config: FarTerrainStreamingConfig) -> Self {
        Self { config }
    }

    pub const fn config(self) -> FarTerrainStreamingConfig {
        self.config
    }

    /// Plans horizon tiles first so a broad landscape appears quickly.
    pub fn plan(
        self,
        player_position: WorldPosition,
        loaded: &BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
        near_cutout: Option<NearTerrainCutout>,
    ) -> Option<FarTerrainStreamingPlan> {
        let center = FarTileIndex::containing(player_position)?;
        let load_radius = i64::try_from(self.config.load_radius).ok()?;
        let mut desired = BTreeMap::new();

        for z_offset in -load_radius..=load_radius {
            for x_offset in -load_radius..=load_radius {
                let tile = FarTileIndex::new(
                    center.x.checked_add(x_offset)?,
                    center.z.checked_add(z_offset)?,
                );
                desired.insert(tile, far_spec(tile, near_cutout));
            }
        }
        for &tile in loaded.keys() {
            if tile.chebyshev_distance(center) <= self.config.retain_radius {
                desired
                    .entry(tile)
                    .or_insert_with(|| far_spec(tile, near_cutout));
            }
        }

        let mut load = desired
            .values()
            .copied()
            .filter(|spec| loaded.get(&spec.tile) != Some(spec))
            .collect::<Vec<_>>();
        load.sort_by_key(|spec| {
            (
                Reverse(spec.tile.chebyshev_distance(center)),
                spec.tile.z,
                spec.tile.x,
            )
        });
        let unload = loaded
            .keys()
            .copied()
            .filter(|tile| !desired.contains_key(tile))
            .collect();

        Some(FarTerrainStreamingPlan {
            center,
            load,
            unload,
        })
    }
}

fn far_spec(tile: FarTileIndex, near_cutout: Option<NearTerrainCutout>) -> FarTerrainMeshSpec {
    FarTerrainMeshSpec {
        tile,
        near_cutout: near_cutout.filter(|cutout| tile.intersects(*cutout)),
    }
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

    pub const fn config(self) -> ChunkStreamingConfig {
        self.config
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

#[allow(clippy::cast_precision_loss)]
fn i64_as_f64(value: i64) -> f64 {
    value as f64
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use treeline_coordinates::{WorldIdentity, stable_hash};
    use treeline_geography::WatershedRegion;
    use treeline_hydrology::{GullyNetwork, RiverNetwork};
    use treeline_terrain::RollingHills;

    #[test]
    fn version_three_rivers_lower_the_shared_near_and_far_surface() {
        let world = WorldIdentity::new(0x5eed, RIVER_TERRAIN_GENERATOR_VERSION, 0);
        let network =
            RiverNetwork::generate(world, WatershedRegionIndex::new(0, 0)).expect("river network");
        let segment = network.segments().first().expect("river segment");
        let x = (segment.source.x + segment.mouth.x) * 0.5;
        let z = (segment.source.z + segment.mouth.z) * 0.5;
        let base = WildernessTerrain::new(world)
            .height_at(x, z)
            .expect("base surface");
        let terrain = GeneratedWorldTerrain::new(world);
        let carved = terrain.surface_height(x, z).expect("carved surface");

        assert!(terrain.river_influence_at(x, z).is_some());
        assert!(carved < base);
        assert!(
            terrain
                .sample(WorldPosition::new(x, carved, z))
                .density
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn version_two_worlds_retain_the_unincised_terrain_contract() {
        let world = WorldIdentity::new(0x5eed, RIVER_TERRAIN_GENERATOR_VERSION - 1, 0);
        let base = WildernessTerrain::new(world);
        let terrain = GeneratedWorldTerrain::new(world);
        for [x, z] in [[-12_000.0, 8_000.0], [0.0, 0.0], [31_000.0, -9_000.0]] {
            assert_eq!(terrain.surface_height(x, z), base.height_at(x, z));
        }
        assert!(
            terrain
                .river_networks
                .read()
                .expect("cache lock")
                .is_empty()
        );
    }

    #[test]
    fn version_five_composes_macro_meso_and_micro_erosion_on_one_surface() {
        let world = WorldIdentity::new(0x5eed, EROSION_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let (x, z, erosion) = generated_incised_gully_point(&terrain);
        let non_fluvial_height = erosion.surface.surface_height_meters();

        assert!(erosion.surface.macro_weathering_meters >= 0.0);
        assert!(erosion.surface.sediment_deposition_meters >= 0.0);
        assert!(erosion.gully.is_some());
        assert!(erosion.final_height_meters < non_fluvial_height);
        assert_eq!(
            terrain.surface_height(x, z).expect("far surface").to_bits(),
            erosion.final_height_meters.to_bits()
        );
        assert!(
            terrain
                .sample(WorldPosition::new(x, erosion.final_height_meters, z))
                .density
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn version_four_worlds_retain_the_pre_erosion_contract() {
        let world = WorldIdentity::new(0x5eed, EROSION_GENERATOR_VERSION - 1, 0);
        let terrain = GeneratedWorldTerrain::new(world);

        assert!(terrain.erosion_at(-12_000.0, 8_000.0).is_none());
        assert!(terrain.gully_influence_at(-12_000.0, 8_000.0).is_none());
        assert!(
            terrain
                .gully_networks
                .read()
                .expect("cache lock")
                .is_empty()
        );
    }

    #[test]
    fn erosion_artifact_caches_do_not_change_sampling_order_results() {
        let world = WorldIdentity::new(0x5eed, EROSION_GENERATOR_VERSION, 0);
        let positions = [
            [-129_000.0, -1_000.0],
            [-1_000.0, -129_000.0],
            [127_000.0, 127_000.0],
        ];
        let forward_terrain = GeneratedWorldTerrain::new(world);
        let forward = positions.map(|[x, z]| {
            forward_terrain
                .surface_height(x, z)
                .expect("forward surface")
        });
        let reverse_terrain = GeneratedWorldTerrain::new(world);
        let mut reverse_positions = positions;
        reverse_positions.reverse();
        let mut reverse = reverse_positions.map(|[x, z]| {
            reverse_terrain
                .surface_height(x, z)
                .expect("reverse surface")
        });
        reverse.reverse();

        assert_eq!(forward.map(f64::to_bits), reverse.map(f64::to_bits));
    }

    #[test]
    fn multi_scale_erosion_has_a_golden_fingerprint() {
        let terrain =
            GeneratedWorldTerrain::new(WorldIdentity::new(0x5eed, EROSION_GENERATOR_VERSION, 0));
        let positions = [
            [-91_125.0, -37_375.0],
            [-64_250.0, 63_875.0],
            [-125.0, 375.0],
            [28_625.0, -52_375.0],
            [117_125.0, 83_625.0],
        ];
        let words = positions.map(|[x, z]| {
            terrain
                .surface_height(x, z)
                .expect("eroded terrain")
                .to_bits()
        });

        assert_eq!(
            stable_hash(&words),
            12_925_604_737_521_515_665,
            "changing this value changes generated multi-scale erosion"
        );
    }

    #[test]
    fn micro_erosion_exposes_rock_and_scree_materials() {
        let terrain =
            GeneratedWorldTerrain::new(WorldIdentity::new(0x5eed, EROSION_GENERATOR_VERSION, 0));
        let mut found_rock = false;
        let mut found_scree = false;
        for z in -8..=8 {
            for x in -8..=8 {
                let world_x = f64::from(x) * 8_000.0;
                let world_z = f64::from(z) * 8_000.0;
                let height = terrain
                    .surface_height(world_x, world_z)
                    .expect("eroded surface");
                match terrain
                    .sample(WorldPosition::new(world_x, height, world_z))
                    .material
                {
                    Material::Rock => found_rock = true,
                    Material::Scree => found_scree = true,
                    Material::Air | Material::Bedrock | Material::Soil | Material::Sand => {}
                }
            }
        }

        assert!(found_rock);
        assert!(found_scree);
    }

    #[test]
    fn river_shaped_terrain_has_a_golden_fingerprint() {
        let world = WorldIdentity::new(0x5eed, RIVER_TERRAIN_GENERATOR_VERSION, 0);
        let network =
            RiverNetwork::generate(world, WatershedRegionIndex::new(-1, 1)).expect("river network");
        let terrain = GeneratedWorldTerrain::new(world);
        let words = network
            .segments()
            .iter()
            .step_by(19)
            .flat_map(|segment| {
                let x = (segment.source.x + segment.mouth.x) * 0.5;
                let z = (segment.source.z + segment.mouth.z) * 0.5;
                [
                    u64::from_le_bytes(segment.source_cell.x.to_le_bytes()),
                    u64::from_le_bytes(segment.source_cell.z.to_le_bytes()),
                    terrain
                        .surface_height(x, z)
                        .expect("river-shaped surface")
                        .to_bits(),
                ]
            })
            .collect::<Vec<_>>();

        assert_eq!(
            stable_hash(&words),
            6_285_394_433_838_367_765,
            "changing this value changes river-shaped terrain"
        );
    }

    #[test]
    fn version_four_exposes_level_lake_water_without_changing_terrain_density() {
        let world = WorldIdentity::new(0x5eed, LAKE_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let (x, z, water) = generated_lake_point(&terrain);

        assert!(water.water_depth_meters > 0.0);
        assert_eq!(
            water.water_depth_meters.to_bits(),
            (water.lake.surface_elevation_meters - water.terrain_elevation_meters).to_bits()
        );
        assert!(
            terrain
                .sample(WorldPosition::new(x, water.terrain_elevation_meters, z))
                .density
                .abs()
                < f64::EPSILON
        );
        assert!(
            terrain
                .sample(WorldPosition::new(
                    x,
                    water.lake.surface_elevation_meters,
                    z
                ))
                .density
                > 0.0
        );
    }

    #[test]
    fn version_three_worlds_do_not_expose_lakes() {
        let terrain =
            GeneratedWorldTerrain::new(WorldIdentity::new(0x5eed, LAKE_GENERATOR_VERSION - 1, 0));

        assert!(terrain.lake_surface_at(-31_000.0, 17_000.0).is_none());
        assert!(terrain.lake_networks.read().expect("cache lock").is_empty());
    }

    #[test]
    fn near_lake_mesh_is_level_aligned_and_deterministic() {
        let world = WorldIdentity::new(0x5eed, LAKE_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let (x, z, water) = generated_lake_point(&terrain);
        let chunk = ChunkIndex::containing(WorldPosition::new(x, 0.0, z)).expect("lake chunk");
        let spec = TerrainMeshSpec::Near(ChunkMeshSpec {
            chunk,
            lod: ChunkIndex::NEAR_LOD,
            transition_faces: TransitionFaces::none(),
        });
        let first = terrain.lake_surface_mesh(spec).expect("lake mesh");
        let second = terrain.lake_surface_mesh(spec).expect("lake mesh again");

        assert_eq!(first, second);
        assert!(first.is_well_formed());
        assert!(!first.indices.is_empty());
        assert!(
            first
                .colors
                .iter()
                .all(|color| color[3].to_bits() == 1.0_f32.to_bits())
        );
        assert!(first.positions.iter().all(|position| {
            (f64::from(position[1]) - water.lake.surface_elevation_meters - 0.05).abs() < 0.001
        }));
    }

    #[test]
    fn active_regions_freeze_but_do_not_become_ungenerated() {
        assert!(RegionState::Active.can_transition_to(RegionState::Frozen));
        assert!(!RegionState::Active.can_transition_to(RegionState::Ungenerated));
    }

    #[test]
    fn visible_terrain_priorities_sort_before_detail() {
        assert!(GenerationPriority::PlayerTerrain < GenerationPriority::Horizon);
        assert!(GenerationPriority::Horizon < GenerationPriority::SurfaceDetail);
    }

    #[test]
    fn queued_mesh_matches_direct_deterministic_generation() {
        let terrain = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let spec = ChunkMeshSpec {
            chunk: ChunkIndex::new(-3, 2),
            lod: ChunkIndex::MAX_LOD,
            transition_faces: TransitionFaces::none(),
        };
        let expected = transvoxel_chunk(&terrain, spec.chunk, spec.lod, spec.transition_faces)
            .expect("direct mesh");
        let mut queue = TerrainMeshQueue::with_worker_count(terrain, NonZeroUsize::MIN);
        queue.enqueue(GenerationPriority::NearTerrain, TerrainMeshSpec::Near(spec));
        let generated = queue
            .ready
            .recv_timeout(Duration::from_secs(2))
            .expect("worker completes the mesh");

        assert_eq!(generated.spec, TerrainMeshSpec::Near(spec));
        assert_eq!(generated.mesh.expect("queued mesh"), expected);
    }

    #[test]
    fn queued_far_mesh_matches_direct_surface_generation() {
        let terrain = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let spec = FarTerrainMeshSpec {
            tile: FarTileIndex::new(-1, 2),
            near_cutout: None,
        };
        let expected = far_terrain_mesh(&terrain, spec).expect("direct far mesh");
        let mut queue = TerrainMeshQueue::with_worker_count(terrain, NonZeroUsize::MIN);
        queue.enqueue(GenerationPriority::FarTerrain, TerrainMeshSpec::Far(spec));
        let generated = queue
            .ready
            .recv_timeout(Duration::from_secs(2))
            .expect("worker completes the far mesh");

        assert_eq!(generated.spec, TerrainMeshSpec::Far(spec));
        assert_eq!(generated.mesh.expect("queued far mesh"), expected);
    }

    #[test]
    fn pending_jobs_order_player_then_horizon_then_near_terrain() {
        let spec = ChunkMeshSpec {
            chunk: ChunkIndex::new(0, 0),
            lod: ChunkIndex::MAX_LOD,
            transition_faces: TransitionFaces::none(),
        };
        let mut pending = BinaryHeap::new();
        pending.push(Reverse(QueuedTerrainMesh {
            priority: GenerationPriority::NearTerrain,
            sequence: 0,
            spec: TerrainMeshSpec::Near(spec),
        }));
        pending.push(Reverse(QueuedTerrainMesh {
            priority: GenerationPriority::Horizon,
            sequence: 1,
            spec: TerrainMeshSpec::Near(spec),
        }));
        pending.push(Reverse(QueuedTerrainMesh {
            priority: GenerationPriority::PlayerTerrain,
            sequence: 2,
            spec: TerrainMeshSpec::Near(spec),
        }));

        assert_eq!(
            pending.pop().expect("queued player terrain job").0.priority,
            GenerationPriority::PlayerTerrain
        );
        assert_eq!(
            pending.pop().expect("queued horizon job").0.priority,
            GenerationPriority::Horizon
        );
        assert_eq!(
            pending.pop().expect("queued near job").0.priority,
            GenerationPriority::NearTerrain
        );
    }

    #[test]
    fn far_tiles_handle_negative_boundaries_and_load_horizon_first() {
        let streamer = FarTerrainStreamer::new(FarTerrainStreamingConfig::new(2, 3).unwrap());
        let plan = streamer
            .plan(
                WorldPosition::new(-0.01, 0.0, -0.01),
                &BTreeMap::new(),
                None,
            )
            .expect("finite position");

        assert_eq!(plan.center, FarTileIndex::new(-1, -1));
        assert_eq!(plan.load.len(), 25);
        assert_eq!(plan.load[0].tile.chebyshev_distance(plan.center), 2);
        assert_eq!(
            plan.load.last().expect("center tile").tile,
            FarTileIndex::new(-1, -1)
        );
    }

    #[test]
    fn complete_near_bounds_cut_only_intersecting_far_tiles() {
        let streamer = FarTerrainStreamer::new(FarTerrainStreamingConfig::new(1, 1).unwrap());
        let cutout = NearTerrainCutout::around(ChunkIndex::new(0, 0), 4).expect("valid bounds");
        let plan = streamer
            .plan(
                WorldPosition::new(0.0, 0.0, 0.0),
                &BTreeMap::new(),
                Some(cutout),
            )
            .expect("finite position");

        assert_eq!(
            plan.load
                .iter()
                .filter(|spec| spec.near_cutout.is_some())
                .map(|spec| spec.tile)
                .collect::<Vec<_>>(),
            vec![
                FarTileIndex::new(-1, -1),
                FarTileIndex::new(0, -1),
                FarTileIndex::new(-1, 0),
                FarTileIndex::new(0, 0),
            ]
        );
        assert!(
            plan.load
                .iter()
                .filter(|spec| spec.near_cutout.is_some())
                .all(|spec| far_terrain_mesh(
                    &RollingHills::new(WorldIdentity::new(0x5eed, 1, 0)),
                    *spec
                )
                .expect("cut mesh")
                .indices
                .len()
                    < FarTileIndex::CELLS_PER_EDGE * FarTileIndex::CELLS_PER_EDGE * 6)
        );
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

    fn generated_lake_point(terrain: &GeneratedWorldTerrain) -> (f64, f64, LakeSurfaceSample) {
        for region_z in -2..=2 {
            for region_x in -2..=2 {
                let region = WatershedRegion::generate(
                    terrain.world(),
                    WatershedRegionIndex::new(region_x, region_z),
                )
                .expect("watershed");
                for cell in region.cells().iter().filter(|cell| cell.basin.is_some()) {
                    let [x, z] = cell.index.center();
                    if let Some(water) = terrain.lake_surface_at(x, z) {
                        return (x, z, water);
                    }
                }
            }
        }
        panic!("test world should contain a visible generated lake");
    }

    fn generated_incised_gully_point(
        terrain: &GeneratedWorldTerrain,
    ) -> (f64, f64, WorldErosionSample) {
        for region_z in -1..=1 {
            for region_x in -1..=1 {
                let network = GullyNetwork::generate(
                    terrain.world(),
                    WatershedRegionIndex::new(region_x, region_z),
                )
                .expect("gully network");
                for segment in network.segments() {
                    let x = segment.bend.x;
                    let z = segment.bend.z;
                    let Some(erosion) = terrain.erosion_at(x, z) else {
                        continue;
                    };
                    if erosion.final_height_meters < erosion.surface.surface_height_meters() {
                        return (x, z, erosion);
                    }
                }
            }
        }
        panic!("test world should contain an incised generated gully");
    }
}
