//! Keeping resident terrain in step with where the player is.
//!
//! This is the bridge between the world crate's residency plans and the GPU:
//! it asks the streamers what should exist, feeds requests to the mesh queue,
//! and uploads whatever comes back. Terrain generation happens elsewhere; this
//! only decides what to ask for and what to keep.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

use bevy::prelude::{
    Assets, Commands, Entity, Mesh as BevyMesh, Mesh3d, MeshMaterial3d, Resource, Transform,
};
use treeline_coordinates::WorldPosition;
use treeline_mesher::Mesh;
use treeline_renderer::{
    WorldMaterials, WorldMeshOrigin, prepare_terrain_mesh, prepare_water_mesh,
};
use treeline_voxel::ChunkIndex;
use treeline_world::{
    ChunkMeshSpec, ChunkStreamer, ChunkStreamingConfig, FarTerrainMeshSpec, FarTerrainStreamer,
    FarTerrainStreamingConfig, FarTileIndex, GenerationPriority, Season, TerrainMeshSpec,
    WorldTerrain,
};
use web_time::{Duration, Instant};

use crate::TerrainMeshQueue;
use crate::progress::LoadProgress;

/// How many completed meshes to upload per frame, and the time budget for them.
///
/// Uploading is main-thread work, so a frame takes a couple of chunks and stops
/// rather than stalling to drain a full queue after a warp.
const MAX_INTEGRATIONS_PER_FRAME: usize = 2;
const INTEGRATION_BUDGET: Duration = Duration::from_millis(3);

/// Residency centers to build ahead of the player along their heading.
const PREFETCH_CENTERS_AHEAD: u64 = 2;

/// The season the terrain surface is dressed for.
///
/// Snow is currently a fixed presentation choice rather than a simulated clock.
const SURFACE_SEASON: Season = Season::Winter;

/// One near chunk resident on the GPU.
#[derive(Debug)]
pub struct ResidentChunk {
    pub spec: ChunkMeshSpec,
    pub terrain: Entity,
    pub water: Option<Entity>,
}

/// One far tile resident on the GPU.
#[derive(Debug)]
pub struct ResidentFarTile {
    pub spec: FarTerrainMeshSpec,
    pub terrain: Entity,
    pub water: Option<Entity>,
}

/// Everything resident, and everything asked for but not yet delivered.
#[derive(Debug, Default, Resource)]
pub struct ResidentTerrain {
    pub chunks: BTreeMap<ChunkIndex, ResidentChunk>,
    pub far_tiles: BTreeMap<FarTileIndex, ResidentFarTile>,
    requested_chunks: BTreeMap<ChunkIndex, ChunkMeshSpec>,
    requested_far_tiles: BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
}

impl ResidentTerrain {
    /// Drops everything and cancels outstanding work, for a warp.
    pub fn clear(&mut self, commands: &mut Commands, jobs: &mut TerrainMeshQueue) {
        for (_, spec) in std::mem::take(&mut self.requested_chunks) {
            jobs.cancel(TerrainMeshSpec::Near(spec));
        }
        for (_, spec) in std::mem::take(&mut self.requested_far_tiles) {
            jobs.cancel(TerrainMeshSpec::Far(spec));
        }
        jobs.retain_prewarm(&BTreeSet::new());
        for resident in std::mem::take(&mut self.chunks).into_values() {
            despawn_surface(commands, resident.terrain, resident.water);
        }
        for resident in std::mem::take(&mut self.far_tiles).into_values() {
            despawn_surface(commands, resident.terrain, resident.water);
        }
    }

    /// Chunks resident or on the way, which the streamer plans against.
    fn tracked_chunks(&self) -> BTreeMap<ChunkIndex, ChunkMeshSpec> {
        let mut tracked = self
            .chunks
            .iter()
            .map(|(&chunk, resident)| (chunk, resident.spec))
            .collect::<BTreeMap<_, _>>();
        tracked.extend(&self.requested_chunks);
        tracked
    }

    fn tracked_far_tiles(&self) -> BTreeMap<FarTileIndex, FarTerrainMeshSpec> {
        let mut tracked = self
            .far_tiles
            .iter()
            .map(|(&tile, resident)| (tile, resident.spec))
            .collect::<BTreeMap<_, _>>();
        tracked.extend(&self.requested_far_tiles);
        tracked
    }

    /// The outstanding requests a load screen is waiting on.
    pub fn outstanding(
        &self,
    ) -> (
        &BTreeMap<ChunkIndex, ChunkMeshSpec>,
        &BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    ) {
        (&self.requested_chunks, &self.requested_far_tiles)
    }
}

/// Where the player is and where they are heading.
///
/// Residency and prefetching both need these together, and neither is
/// meaningful without the other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerMotion {
    pub position: WorldPosition,
    /// Unit horizontal heading, or zero when standing still.
    pub travel_direction: [f64; 2],
}

impl PlayerMotion {
    /// A player who has just arrived somewhere and is not yet moving.
    pub const fn arrived(position: WorldPosition) -> Self {
        Self {
            position,
            travel_direction: [0.0, 0.0],
        }
    }
}

/// Streaming policy for both terrain tiers.
#[derive(Clone, Copy, Debug, Resource)]
pub struct Streamers {
    pub near: ChunkStreamer,
    pub far: FarTerrainStreamer,
}

impl Default for Streamers {
    /// Browsers get a smaller near radius; both tiers cover the whole tile.
    fn default() -> Self {
        let near = if cfg!(target_arch = "wasm32") {
            ChunkStreamingConfig::new(2, 3).expect("the browser streaming radii are valid")
        } else {
            ChunkStreamingConfig::default()
        };
        Self {
            near: ChunkStreamer::new(near),
            far: FarTerrainStreamer::new(
                FarTerrainStreamingConfig::new(1, 1).expect("the surveyed-world radius is valid"),
            ),
        }
    }
}

/// Uploads finished meshes, then reconciles residency with the player position.
///
/// # Errors
///
/// Returns an error when the player leaves the representable coordinate range
/// or a mesh cannot be uploaded.
pub fn update(
    commands: &mut Commands,
    meshes: &mut Assets<BevyMesh>,
    materials: &WorldMaterials,
    terrain: &WorldTerrain,
    streamers: Streamers,
    motion: PlayerMotion,
    resident: &mut ResidentTerrain,
    jobs: &mut TerrainMeshQueue,
    progress: &mut LoadProgress,
) -> Result<(), Box<dyn Error>> {
    integrate_completed(
        commands, meshes, materials, terrain, resident, jobs, progress,
    );
    schedule(commands, streamers, motion, resident, jobs)
}

/// Uploads completed meshes, within this frame's budget.
fn integrate_completed(
    commands: &mut Commands,
    meshes: &mut Assets<BevyMesh>,
    materials: &WorldMaterials,
    terrain: &WorldTerrain,
    resident: &mut ResidentTerrain,
    jobs: &mut TerrainMeshQueue,
    progress: &mut LoadProgress,
) {
    let frame_started = Instant::now();
    let mut integrated = 0;
    while integrated < MAX_INTEGRATIONS_PER_FRAME
        && (integrated == 0 || frame_started.elapsed() < INTEGRATION_BUDGET)
    {
        let Some(generated) = jobs.try_next() else {
            return;
        };
        integrated += 1;
        let integration_started = Instant::now();
        let spec = generated.spec;

        // A result the streamer no longer wants is discarded. This happens
        // routinely after a warp, when in-flight jobs outlive their request.
        let wanted = match spec {
            TerrainMeshSpec::Near(near) => {
                resident.requested_chunks.get(&near.chunk) == Some(&near)
            }
            TerrainMeshSpec::Far(far) => resident.requested_far_tiles.get(&far.tile) == Some(&far),
        };
        if !wanted {
            if generated.priority != GenerationPriority::PrefetchTerrain {
                progress.record_discarded(&generated);
            }
            continue;
        }

        // The job has already left the queue, so clear the request before any
        // fallible step. Leaving it would make the streamer treat this chunk as
        // still pending and never ask again, leaving a permanent hole.
        clear_request(spec, resident);

        let (Ok(mesh), Ok(lake_mesh)) = (generated.mesh, generated.lake_mesh.transpose()) else {
            eprintln!("terrain meshing failed, retrying later");
            continue;
        };
        // A failed upload must not abort the whole update either; the cleared
        // request lets a later frame retry.
        let Ok((surface, water)) = upload(
            commands,
            meshes,
            materials,
            terrain,
            &mesh,
            lake_mesh.as_ref(),
        ) else {
            eprintln!("terrain upload failed, retrying later");
            continue;
        };
        match spec {
            TerrainMeshSpec::Near(spec) => {
                if let Some(previous) = resident.chunks.insert(
                    spec.chunk,
                    ResidentChunk {
                        spec,
                        terrain: surface,
                        water,
                    },
                ) {
                    despawn_surface(commands, previous.terrain, previous.water);
                }
            }
            TerrainMeshSpec::Far(spec) => {
                if let Some(previous) = resident.far_tiles.insert(
                    spec.tile,
                    ResidentFarTile {
                        spec,
                        terrain: surface,
                        water,
                    },
                ) {
                    despawn_surface(commands, previous.terrain, previous.water);
                }
            }
        }
        progress.record_completed(
            spec,
            generated.terrain_generation_time,
            generated.lake_generation_time,
            integration_started.elapsed(),
        );
    }
}

fn clear_request(spec: TerrainMeshSpec, resident: &mut ResidentTerrain) {
    match spec {
        TerrainMeshSpec::Near(spec) => {
            resident.requested_chunks.remove(&spec.chunk);
        }
        TerrainMeshSpec::Far(spec) => {
            resident.requested_far_tiles.remove(&spec.tile);
        }
    }
}

/// Uploads a terrain surface and its water sheet with a shared snow treatment.
///
/// Near and far tiers go through this one path, so they cannot drift apart in
/// appearance at the seam between them.
fn upload(
    commands: &mut Commands,
    meshes: &mut Assets<BevyMesh>,
    materials: &WorldMaterials,
    terrain: &WorldTerrain,
    mesh: &Mesh,
    lake_mesh: Option<&Mesh>,
) -> Result<(Entity, Option<Entity>), Box<dyn Error>> {
    let surface = prepare_terrain_mesh(mesh, |x, z| {
        terrain
            .snow_cover_for_slope(x, z, SURFACE_SEASON, 0.0)
            .map(|snow| snow.coverage_fraction)
    })?;
    let surface_entity = spawn_prepared(
        commands,
        meshes,
        materials.terrain.clone(),
        surface,
        "terrain",
    );
    let water = lake_mesh
        .filter(|mesh| !mesh.indices.is_empty())
        .map(prepare_water_mesh)
        .transpose()?
        .map(|prepared| {
            spawn_prepared(commands, meshes, materials.water.clone(), prepared, "water")
        });
    Ok((surface_entity, water))
}

fn spawn_prepared(
    commands: &mut Commands,
    meshes: &mut Assets<BevyMesh>,
    material: bevy::prelude::Handle<bevy::prelude::StandardMaterial>,
    prepared: treeline_renderer::PreparedMesh,
    name: &'static str,
) -> Entity {
    commands
        .spawn((
            bevy::prelude::Name::new(name),
            Mesh3d(meshes.add(prepared.mesh)),
            MeshMaterial3d(material),
            Transform::default(),
            WorldMeshOrigin(prepared.world_origin),
        ))
        .id()
}

fn despawn_surface(commands: &mut Commands, terrain: Entity, water: Option<Entity>) {
    commands.entity(terrain).despawn();
    if let Some(water) = water {
        commands.entity(water).despawn();
    }
}

/// Reconciles residency with the player position and queues the difference.
///
/// # Errors
///
/// Returns an error when the player position leaves the representable range.
pub fn schedule(
    commands: &mut Commands,
    streamers: Streamers,
    motion: PlayerMotion,
    resident: &mut ResidentTerrain,
    jobs: &mut TerrainMeshQueue,
) -> Result<(), Box<dyn Error>> {
    let tracked_chunks = resident.tracked_chunks();
    let near_plan = streamers
        .near
        .plan(motion.position, &tracked_chunks)
        .ok_or_else(|| std::io::Error::other("player position is outside chunk index range"))?;
    let far_plan = streamers
        .far
        .plan(motion.position, &resident.tracked_far_tiles())
        .ok_or_else(|| std::io::Error::other("player position is outside far tile index range"))?;

    for chunk in &near_plan.unload {
        if let Some(removed) = resident.chunks.remove(chunk) {
            despawn_surface(commands, removed.terrain, removed.water);
        }
        if let Some(spec) = resident.requested_chunks.remove(chunk) {
            jobs.cancel(TerrainMeshSpec::Near(spec));
        }
    }
    for tile in &far_plan.unload {
        if let Some(removed) = resident.far_tiles.remove(tile) {
            despawn_surface(commands, removed.terrain, removed.water);
        }
        if let Some(spec) = resident.requested_far_tiles.remove(tile) {
            jobs.cancel(TerrainMeshSpec::Far(spec));
        }
    }

    for spec in &near_plan.load {
        if let Some(previous) = resident.requested_chunks.insert(spec.chunk, *spec) {
            jobs.cancel(TerrainMeshSpec::Near(previous));
        }
    }
    for spec in &far_plan.load {
        if let Some(previous) = resident.requested_far_tiles.insert(spec.tile, *spec) {
            jobs.cancel(TerrainMeshSpec::Far(previous));
        }
    }

    // The chunk under the player first, then the horizon, then everything else:
    // standing on ground beats seeing distant ground, which beats detail.
    if let Some(spec) = near_plan
        .load
        .iter()
        .find(|spec| spec.chunk == near_plan.center)
    {
        jobs.enqueue(
            GenerationPriority::PlayerTerrain,
            TerrainMeshSpec::Near(*spec),
        );
    }
    for spec in &far_plan.load {
        let priority = if spec.tile.chebyshev_distance(far_plan.center)
            == streamers.far.config().load_radius()
        {
            GenerationPriority::Horizon
        } else {
            GenerationPriority::FarTerrain
        };
        jobs.enqueue(priority, TerrainMeshSpec::Far(*spec));
    }
    for spec in near_plan
        .load
        .iter()
        .filter(|spec| spec.chunk != near_plan.center)
    {
        jobs.enqueue(
            GenerationPriority::NearTerrain,
            TerrainMeshSpec::Near(*spec),
        );
    }

    prefetch(streamers, motion, &tracked_chunks, jobs)?;
    Ok(())
}

/// Speculatively builds meshes ahead of the player, using idle workers.
fn prefetch(
    streamers: Streamers,
    motion: PlayerMotion,
    tracked_chunks: &BTreeMap<ChunkIndex, ChunkMeshSpec>,
    jobs: &mut TerrainMeshQueue,
) -> Result<usize, Box<dyn Error>> {
    let specs = streamers
        .near
        .prefetch_specs(
            motion.position,
            motion.travel_direction,
            PREFETCH_CENTERS_AHEAD,
        )
        .ok_or_else(|| std::io::Error::other("terrain prefetch exceeds chunk index range"))?;
    jobs.retain_prewarm(&specs.iter().copied().map(TerrainMeshSpec::Near).collect());
    Ok(specs
        .into_iter()
        .filter(|spec| tracked_chunks.get(&spec.chunk) != Some(spec))
        .filter(|spec| jobs.prewarm(TerrainMeshSpec::Near(*spec)))
        .count())
}
