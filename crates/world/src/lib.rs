//! Streaming-world lifecycle and deterministic terrain-LOD planning.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};
use std::mem::size_of;
use std::num::NonZeroUsize;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, JoinHandle};

pub use treeline_caves::{
    CAVE_GENERATOR_VERSION, CaveEntrance, CaveFamily, CaveInfluence, CaveNode, CaveNodeKind,
    CaveRegionIndex, CaveSystem, UndergroundRiver,
};
use treeline_coordinates::{CellIndex, WorldIdentity, WorldPosition, stable_hash};
use treeline_ecology::{
    EcosystemDistribution, EcosystemSample, ForestDistribution, ForestSample,
    GroundVegetationDistribution, GroundVegetationSample, REEF_GENERATOR_VERSION, ReefDistribution,
    ReefSample, Soil, SoilSample, WETLAND_GENERATOR_VERSION, WetlandDistribution, WetlandHydrology,
    WetlandKind, WetlandSample,
};
pub use treeline_geography::Season;
use treeline_geography::{
    Climate, DrainageCellIndex, PROVINCE_GENERATOR_VERSION, ProvincePlan, RegionalProfile,
    WatershedRegionIndex,
};
pub use treeline_hydrology::Lake;
use treeline_hydrology::{
    ActiveWaterError, ActiveWaterRegion, GullyNetwork, GullyTerrainInfluence,
    LOCAL_CHANNEL_ALIGNMENT_GENERATOR_VERSION, LakeNetwork, RiverNetwork, RiverTerrainInfluence,
    WaterCell, WaterCellId, WaterCellKind, WaterConnection,
};
use treeline_mesher::{Mesh, MeshingError, SurfaceGridSpec, surface_grid, transvoxel_chunk};
use treeline_terrain::{
    DEFAULT_SURVEYED_SETTINGS_HASH, DensityField, ErosionSurfaceSample, Material, SurfaceField,
    SurveyedCanopySample, TerrainSample, WildernessTerrain,
};
use treeline_voxel::{ChunkFace, ChunkIndex, LodLevel, TransitionFaces};
use web_time::{Duration, Instant};

/// Generator version that first makes regional rivers shape terrain.
pub const RIVER_TERRAIN_GENERATOR_VERSION: u32 = 3;
/// Generator version that first exposes filled drainage basins as lakes.
pub const LAKE_GENERATOR_VERSION: u32 = 4;
/// Generator version that first composes macro, meso, and micro erosion.
pub const EROSION_GENERATOR_VERSION: u32 = 5;
/// Generator version that first exposes active-region living-water topology.
pub const LIVING_WATER_GENERATOR_VERSION: u32 = 17;
/// Generator version that resets generation around top-down geographical provinces.
pub const LANDSCAPE_DIVERSITY_GENERATOR_VERSION: u32 = PROVINCE_GENERATOR_VERSION;
/// Generator version that makes calibrated multiscale terrain the default.
pub const CALIBRATED_TERRAIN_GENERATOR_VERSION: u32 =
    treeline_geography::CALIBRATED_PROVINCE_GENERATOR_VERSION;
/// Latest generator contract used for newly created prototype worlds.
pub const CURRENT_GENERATOR_VERSION: u32 = LOCAL_CHANNEL_ALIGNMENT_GENERATOR_VERSION;
/// Default world loaded by the player-facing client.
///
/// The seed still supplies stable identities for procedural tree individuals;
/// the settings hash selects the versioned surveyed terrain bundle.
pub const DEFAULT_WORLD_IDENTITY: WorldIdentity = WorldIdentity::new(
    0x5eed,
    CURRENT_GENERATOR_VERSION,
    DEFAULT_SURVEYED_SETTINGS_HASH,
);

const SNOW_SLOPE_SAMPLE_RADIUS_METERS: f64 = 16.0;
const DOMAIN_SURFACE_WATER_CELL: u64 = 0x5355_5246_5741_5445;
const DOMAIN_CAVE_WATER_CELL: u64 = 0x4341_5645_5741_5445;
/// Terrain-shape cache budget, in entries.
///
/// Entries are large (roughly half a kilobyte each), and every browser terrain
/// worker owns an independent cache, so the ceiling is multiplied by the worker
/// count. Reuse is overwhelmingly within a single mesh job rather than across
/// jobs: a far tile touches about 1,200 positions and a near chunk a few
/// thousand. Measuring nine far tiles plus twenty-five near chunks showed no
/// difference between this budget and one eight times larger, while budgets at
/// or below 4,096 entries began recomputing inside a single job.
const MAX_TERRAIN_SHAPE_CACHE_ENTRIES: usize = 16_384;

/// Deterministic snow retained by the generated terrain surface.
///
/// Coverage is sampled at a fixed world-space scale, rather than from mesh
/// normals, so the same location receives the same snow treatment at every
/// terrain LOD.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnowCoverageSample {
    pub season: Season,
    pub snowpack_water_equivalent_millimeters: f64,
    pub terrain_slope: f64,
    pub coverage_fraction: f64,
}

/// Equilibrium lake water at one horizontal world position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LakeSurfaceSample {
    pub lake: Lake,
    pub terrain_elevation_meters: f64,
    pub water_depth_meters: f64,
}

/// Equilibrium sea-level water above generated ocean terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OceanSurfaceSample {
    pub surface_elevation_meters: f64,
    pub terrain_elevation_meters: f64,
    pub water_depth_meters: f64,
}

/// A regular surface footprint used to reconstruct local active water.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveWaterRegionSpec {
    pub origin_x: f64,
    pub origin_z: f64,
    pub cell_counts: [usize; 2],
    pub spacing_meters: f64,
}

impl ActiveWaterRegionSpec {
    pub fn new(
        origin_x: f64,
        origin_z: f64,
        cell_counts: [usize; 2],
        spacing_meters: f64,
    ) -> Option<Self> {
        (origin_x.is_finite()
            && origin_z.is_finite()
            && !cell_counts.contains(&0)
            && spacing_meters.is_finite()
            && spacing_meters > 0.0)
            .then_some(Self {
                origin_x,
                origin_z,
                cell_counts,
                spacing_meters,
            })
    }
}

/// Top-down Generator Lab description of a subterranean system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveMapSample {
    pub family: CaveFamily,
    pub system_key: u64,
    pub depth_below_surface_meters: f64,
    pub horizontal_distance_meters: f64,
    pub has_underground_river: bool,
}

/// Explainable contributors to the versioned multi-scale erosion surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldErosionSample {
    pub surface: ErosionSurfaceSample,
    pub gully: Option<GullyTerrainInfluence>,
    pub river: Option<RiverTerrainInfluence>,
    pub reef: Option<ReefSample>,
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
    river_networks: Arc<NetworkCache<RiverNetwork>>,
    gully_networks: Arc<NetworkCache<GullyNetwork>>,
    lake_networks: Arc<NetworkCache<LakeNetwork>>,
    cave_systems: Arc<CaveCache>,
    cave_neighborhoods: Arc<CaveNeighborhoodCache>,
    terrain_shapes: Arc<RwLock<BTreeMap<(u64, u64), TerrainShape>>>,
}

type NetworkSlot<T> = Arc<OnceLock<Option<Arc<T>>>>;
type NetworkCache<T> = RwLock<BTreeMap<WatershedRegionIndex, NetworkSlot<T>>>;
type CaveCache = RwLock<BTreeMap<CaveRegionIndex, NetworkSlot<CaveSystem>>>;
type CaveNeighborhoodCache = RwLock<BTreeMap<CaveRegionIndex, Arc<Vec<Arc<CaveSystem>>>>>;

impl GeneratedWorldTerrain {
    pub fn new(world: WorldIdentity) -> Self {
        Self {
            base: WildernessTerrain::new(world),
            river_networks: Arc::new(RwLock::new(BTreeMap::new())),
            gully_networks: Arc::new(RwLock::new(BTreeMap::new())),
            lake_networks: Arc::new(RwLock::new(BTreeMap::new())),
            cave_systems: Arc::new(RwLock::new(BTreeMap::new())),
            cave_neighborhoods: Arc::new(RwLock::new(BTreeMap::new())),
            terrain_shapes: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub const fn world(&self) -> WorldIdentity {
        self.base.world
    }

    /// Whether this world uses the fixed Michigan surveyed-terrain artifact.
    pub const fn is_surveyed_tile(&self) -> bool {
        self.base.is_surveyed_tile()
    }

    /// Samples lidar-derived canopy structure for the fixed surveyed bundle.
    pub fn surveyed_canopy_at(&self, x: f64, z: f64) -> Option<SurveyedCanopySample> {
        self.base.surveyed_canopy_at(x, z)
    }

    /// Samples seasonal snow cover for the composed terrain surface.
    ///
    /// Seasonal climate supplies the available snowpack. A fixed-radius slope
    /// sample retains it on flats and sheltered inclines while exposing steep
    /// terrain. This is a renderable surface layer; it does not alter the
    /// signed density field or collision surface.
    pub fn snow_coverage_at(&self, x: f64, z: f64, season: Season) -> Option<SnowCoverageSample> {
        let left = self
            .shaped_height(x - SNOW_SLOPE_SAMPLE_RADIUS_METERS, z)?
            .height;
        let right = self
            .shaped_height(x + SNOW_SLOPE_SAMPLE_RADIUS_METERS, z)?
            .height;
        let down = self
            .shaped_height(x, z - SNOW_SLOPE_SAMPLE_RADIUS_METERS)?
            .height;
        let up = self
            .shaped_height(x, z + SNOW_SLOPE_SAMPLE_RADIUS_METERS)?
            .height;
        let span = SNOW_SLOPE_SAMPLE_RADIUS_METERS * 2.0;
        let terrain_slope = libm::hypot((right - left) / span, (up - down) / span);
        self.snow_coverage_for_slope(x, z, season, terrain_slope)
    }

    /// Samples seasonal snow cover using a slope already available to the
    /// caller.
    ///
    /// Render meshes already carry surface normals derived while generating
    /// their geometry. Reusing that slope avoids four additional composed
    /// terrain queries per vertex on the render thread. Call
    /// [`Self::snow_coverage_at`] when a mesh-independent fixed-scale sample is
    /// required instead.
    pub fn snow_coverage_for_slope(
        &self,
        x: f64,
        z: f64,
        season: Season,
        terrain_slope: f64,
    ) -> Option<SnowCoverageSample> {
        if !terrain_slope.is_finite() || terrain_slope < 0.0 {
            return None;
        }
        let climate = Climate::new(self.world()).sample_season(x, z, season)?;
        let snowpack = climate.snowpack_water_equivalent_millimeters;
        let depth_cover = smoothstep(8.0, 240.0, snowpack);
        let slope_retention = 1.0 - smoothstep(0.32, 1.15, terrain_slope);
        Some(SnowCoverageSample {
            season,
            snowpack_water_equivalent_millimeters: snowpack,
            terrain_slope,
            coverage_fraction: (depth_cover * slope_retention).clamp(0.0, 1.0),
        })
    }

    /// Returns the strongest nearby river contribution, if terrain carving is
    /// enabled by this world's generation version.
    pub fn river_influence_at(&self, x: f64, z: f64) -> Option<RiverTerrainInfluence> {
        if self.base.is_surveyed_tile()
            || self.world().generator_version < RIVER_TERRAIN_GENERATOR_VERSION
        {
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
                // Skip a region that cannot produce a network rather than
                // discarding contributions from the neighbours that can, which
                // would carve a discontinuity along the artifact boundary.
                let Some(network) = self.river_network(region) else {
                    continue;
                };
                let Some(mut influence) = network
                    .segment_from(source)
                    .and_then(|segment| segment.terrain_influence(x, z))
                else {
                    continue;
                };
                if self.world().generator_version >= LANDSCAPE_DIVERSITY_GENERATOR_VERSION {
                    let Some(province) = ProvincePlan::sample_at(self.world(), x, z) else {
                        continue;
                    };
                    let canyon_strength = ((province.mountain * 0.32)
                        + (province.plateau * 0.18)
                        + (province.scarp * 0.18)
                        + (province.rock_hardness * 0.17)
                        + (province.aridity * 0.15))
                        .clamp(0.0, 1.0);
                    let floodplain_strength =
                        (province.plains * province.sediment * (0.45 + (province.moisture * 0.55)))
                            .clamp(0.0, 1.0);
                    let width_scale = (1.0 - (canyon_strength * 0.58)
                        + (floodplain_strength * 0.18))
                        .clamp(0.42, 1.0);
                    influence.valley_half_width_meters *= width_scale;
                    if influence.distance_meters > influence.valley_half_width_meters {
                        continue;
                    }
                    influence.channel_half_width_meters *=
                        (0.82 + (floodplain_strength * 0.28)).clamp(0.82, 1.08);
                    influence.incision_depth_meters *=
                        0.82 + (canyon_strength * 2.45) + (province.glacial * 0.38);
                    let normalized =
                        1.0 - (influence.distance_meters / influence.valley_half_width_meters);
                    influence.blend = normalized * normalized * (3.0 - (2.0 * normalized));
                }
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
        if self.base.is_surveyed_tile()
            || self.world().generator_version < EROSION_GENERATOR_VERSION
        {
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
                // Skip a region that cannot produce a network rather than
                // discarding contributions from the neighbours that can, which
                // would carve a discontinuity along the artifact boundary.
                let Some(network) = self.gully_network(region) else {
                    continue;
                };
                let Some(mut influence) = network
                    .segment_from(source)
                    .and_then(|segment| segment.terrain_influence(x, z))
                else {
                    continue;
                };
                if self.world().generator_version >= LANDSCAPE_DIVERSITY_GENERATOR_VERSION {
                    let Some(province) = ProvincePlan::sample_at(self.world(), x, z) else {
                        continue;
                    };
                    let ruggedness = (province.mountain * 0.48
                        + province.plateau * 0.22
                        + province.scarp * 0.30)
                        .clamp(0.0, 1.0);
                    let incision_scale = 0.72
                        + (ruggedness * 1.62)
                        + (province.aridity * province.rock_hardness * 0.58);
                    let width_scale =
                        (1.0 - (ruggedness * 0.48) + (province.plains * 0.12)).clamp(0.48, 1.0);
                    influence.segment.half_width_meters *= width_scale;
                    if influence.distance_meters > influence.segment.half_width_meters {
                        continue;
                    }
                    influence.segment.incision_depth_meters *= incision_scale;
                    let normalized =
                        1.0 - (influence.distance_meters / influence.segment.half_width_meters);
                    influence.blend = normalized * normalized * (3.0 - (2.0 * normalized));
                }
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
        let slot = network_slot(&self.river_networks, region);
        slot.get_or_init(|| RiverNetwork::generate(self.world(), region).map(Arc::new))
            .clone()
    }

    fn gully_network(&self, region: WatershedRegionIndex) -> Option<Arc<GullyNetwork>> {
        let slot = network_slot(&self.gully_networks, region);
        slot.get_or_init(|| GullyNetwork::generate(self.world(), region).map(Arc::new))
            .clone()
    }

    /// Returns equilibrium lake water above the generated terrain, if present.
    pub fn lake_surface_at(&self, x: f64, z: f64) -> Option<LakeSurfaceSample> {
        if self.base.is_surveyed_tile() {
            return self.surveyed_lake_surface_at(x, z);
        }
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

    /// Returns seasonally high or low lake water, including spring playa flooding.
    pub fn lake_surface_at_season(
        &self,
        x: f64,
        z: f64,
        season: Season,
    ) -> Option<LakeSurfaceSample> {
        if self.base.is_surveyed_tile() {
            return self.surveyed_lake_surface_at(x, z);
        }
        if self.world().generator_version < LAKE_GENERATOR_VERSION {
            return None;
        }
        let cell = DrainageCellIndex::containing(x, z)?;
        let network = self.lake_network(WatershedRegionIndex::containing_cell(cell))?;
        let lake = network.lake_for_cell(cell)?;
        let terrain_elevation_meters = self.shaped_height(x, z)?.height;
        let water_depth_meters = lake.water_depth_at_season(terrain_elevation_meters, season)?;
        (water_depth_meters > 0.0).then_some(LakeSurfaceSample {
            lake,
            terrain_elevation_meters,
            water_depth_meters,
        })
    }

    fn surveyed_lake_surface_at(&self, x: f64, z: f64) -> Option<LakeSurfaceSample> {
        let (lake_id, surface_elevation_meters) = self.base.surveyed_lake_at(x, z)?;
        let terrain_elevation_meters = self.shaped_height(x, z)?.height;
        let cell = DrainageCellIndex::containing(x, z)?;
        let water_depth_meters = (surface_elevation_meters - terrain_elevation_meters).max(0.05);
        Some(LakeSurfaceSample {
            lake: Lake {
                id: u64::from(lake_id),
                bottom: cell,
                bottom_elevation_meters: surface_elevation_meters - 2.0,
                surface_elevation_meters,
                outlet: cell,
                cell_count: 1,
                spill_elevation_meters: surface_elevation_meters + 0.5,
                surface_outlet: Some(cell),
                fill_fraction: 1.0,
                water_balance_fraction: 1.0,
                closed_basin_fraction: 0.0,
                seasonal_fraction: 0.0,
                salinity_fraction: 0.0,
                playa_fraction: 0.0,
            },
            terrain_elevation_meters,
            water_depth_meters,
        })
    }

    /// Lists lakes in the watershed artifact containing a horizontal position.
    ///
    /// This uses the same immutable regional cache as surface sampling, making
    /// it suitable for inspection and travel tools that need to select a body
    /// before querying its fine shoreline.
    pub fn regional_lakes_at(&self, x: f64, z: f64) -> Option<Vec<Lake>> {
        if self.base.is_surveyed_tile() || self.world().generator_version < LAKE_GENERATOR_VERSION {
            return None;
        }
        let region = WatershedRegionIndex::containing(x, z)?;
        self.lake_network(region)
            .map(|network| network.lakes().to_vec())
    }

    /// Returns equilibrium ocean water above terrain below global sea level.
    pub fn ocean_surface_at(&self, x: f64, z: f64) -> Option<OceanSurfaceSample> {
        if self.base.is_surveyed_tile() || self.world().generator_version < REEF_GENERATOR_VERSION {
            return None;
        }
        let terrain_elevation_meters = self.shaped_height(x, z)?.height;
        let water_depth_meters = -terrain_elevation_meters;
        (water_depth_meters > 0.0).then_some(OceanSurfaceSample {
            surface_elevation_meters: 0.0,
            terrain_elevation_meters,
            water_depth_meters,
        })
    }

    /// Reconstructs deterministic local storage and routing for living water.
    ///
    /// The surface lattice samples generated terrain, equilibrium lakes,
    /// rivers, coasts, and climate runoff. Wet cave graph nodes are appended
    /// and entrances or sinkholes receive explicit surface-to-cave links.
    /// Terrain deviations remain an input to [`ActiveWaterRegion`] after this
    /// pure topology has been regenerated.
    ///
    /// # Errors
    ///
    /// Rejects invalid or unrepresentable samples and any generated cell or
    /// connection topology that violates active-water invariants.
    #[allow(clippy::too_many_lines)]
    pub fn active_water_region(
        &self,
        spec: ActiveWaterRegionSpec,
    ) -> Result<ActiveWaterRegion, ActiveWaterError> {
        const SECONDS_PER_YEAR: f64 = 31_556_952.0;

        if self.base.is_surveyed_tile() {
            return ActiveWaterRegion::new(Vec::new(), Vec::new());
        }
        if self.world().generator_version < LIVING_WATER_GENERATOR_VERSION {
            return ActiveWaterRegion::new(Vec::new(), Vec::new());
        }
        let [cells_x, cells_z] = spec.cell_counts;
        let cell_area = spec.spacing_meters * spec.spacing_meters;
        let count = cells_x
            .checked_mul(cells_z)
            .ok_or(ActiveWaterError::InvalidCell)?;
        let mut cells = Vec::with_capacity(count);
        let mut surface_ids = Vec::with_capacity(count);
        for local_z in 0..cells_z {
            let z = spec.origin_z + ((usize_as_f64(local_z) + 0.5) * spec.spacing_meters);
            for local_x in 0..cells_x {
                let x = spec.origin_x + ((usize_as_f64(local_x) + 0.5) * spec.spacing_meters);
                let lattice = CellIndex::containing(x, z, 0, spec.spacing_meters)
                    .ok_or(ActiveWaterError::InvalidCell)?;
                let id =
                    WaterCellId(lattice.generation_key(self.world(), DOMAIN_SURFACE_WATER_CELL));
                let bed = self
                    .surface_height(x, z)
                    .ok_or(ActiveWaterError::InvalidCell)?;
                let lake = self.lake_surface_at(x, z);
                let ocean = self.ocean_surface_at(x, z);
                let river = self.river_influence_at(x, z).filter(|influence| {
                    influence.distance_meters <= influence.channel_half_width_meters
                });
                let (kind, water_depth) = if let Some(water) = ocean {
                    (WaterCellKind::Coast, water.water_depth_meters)
                } else if let Some(water) = lake {
                    (WaterCellKind::Surface, water.water_depth_meters)
                } else if let Some(influence) = river {
                    (
                        WaterCellKind::Surface,
                        (0.12
                            + (libm::sqrt(influence.segment.discharge_cubic_meters_per_second)
                                * 0.08))
                            .clamp(0.12, 2.0),
                    )
                } else {
                    (WaterCellKind::Surface, 0.0)
                };
                let bank = if let Some(water) = lake {
                    water.lake.surface_elevation_meters + 0.35
                } else if ocean.is_some() {
                    0.6_f64.max(bed)
                } else if let Some(influence) = river {
                    bed + (influence.incision_depth_meters * 0.42).max(0.8)
                } else {
                    bed + 0.8
                };
                let climate = Climate::new(self.world())
                    .sample(x, z)
                    .ok_or(ActiveWaterError::InvalidCell)?;
                let runoff_depth_per_year = climate.annual_precipitation_millimeters / 1_000.0
                    * (0.18 + ((1.0 - climate.warmth_fraction()) * 0.34));
                cells.push(WaterCell {
                    id,
                    kind,
                    bed_elevation_meters: bed,
                    bank_elevation_meters: bank,
                    area_square_meters: cell_area,
                    water_depth_meters: water_depth,
                    source_cubic_meters_per_second: runoff_depth_per_year * cell_area
                        / SECONDS_PER_YEAR,
                    infiltration_cubic_meters_per_second: if lake.is_some()
                        || ocean.is_some()
                        || river.is_some()
                    {
                        0.0
                    } else {
                        runoff_depth_per_year * cell_area / SECONDS_PER_YEAR * 0.16
                    },
                });
                surface_ids.push(id);
            }
        }

        let mut connections = Vec::new();
        for local_z in 0..cells_z {
            for local_x in 0..cells_x {
                let slot = local_z * cells_x + local_x;
                if local_x + 1 < cells_x {
                    append_surface_water_connection(
                        &mut connections,
                        &cells,
                        surface_ids[slot],
                        surface_ids[slot + 1],
                        spec.spacing_meters,
                    )?;
                }
                if local_z + 1 < cells_z {
                    append_surface_water_connection(
                        &mut connections,
                        &cells,
                        surface_ids[slot],
                        surface_ids[slot + cells_x],
                        spec.spacing_meters,
                    )?;
                }
                if cells[slot].kind == WaterCellKind::Coast
                    && (local_x == 0
                        || local_z == 0
                        || local_x + 1 == cells_x
                        || local_z + 1 == cells_z)
                {
                    connections.push(WaterConnection {
                        from: surface_ids[slot],
                        to: None,
                        sill_elevation_meters: 0.0,
                        width_meters: spec.spacing_meters,
                        conductance: 0.22,
                    });
                }
            }
        }

        let max_x = spec.origin_x + (usize_as_f64(cells_x) * spec.spacing_meters);
        let max_z = spec.origin_z + (usize_as_f64(cells_z) * spec.spacing_meters);
        for system in self.cave_systems_intersecting(spec.origin_x, spec.origin_z, max_x, max_z) {
            let mut cave_ids = Vec::with_capacity(system.graph.nodes.len());
            for (node_index, node) in system.graph.nodes.iter().enumerate() {
                let id = WaterCellId(stable_hash(&[
                    system.system_key,
                    u64::try_from(node_index).map_err(|_| ActiveWaterError::InvalidCell)?,
                    DOMAIN_CAVE_WATER_CELL,
                ]));
                let bed = node.position.y - (node.radius_meters * 0.56);
                let is_wet = system
                    .graph
                    .underground_rivers
                    .iter()
                    .any(|river| river.flow_from == node_index || river.flow_to == node_index);
                cells.push(WaterCell {
                    id,
                    kind: if node.kind == CaveNodeKind::Sump {
                        WaterCellKind::Sump
                    } else {
                        WaterCellKind::CaveStream
                    },
                    bed_elevation_meters: bed,
                    bank_elevation_meters: node.position.y + (node.radius_meters * 0.2),
                    area_square_meters: (node.radius_meters
                        * node.radius_meters
                        * core::f64::consts::PI)
                        .max(1.0),
                    water_depth_meters: if is_wet {
                        (node.radius_meters * 0.12).max(0.15)
                    } else {
                        0.0
                    },
                    source_cubic_meters_per_second: 0.0,
                    infiltration_cubic_meters_per_second: if node.kind == CaveNodeKind::Sump {
                        0.01
                    } else {
                        0.0
                    },
                });
                cave_ids.push(id);

                if matches!(node.kind, CaveNodeKind::Entrance | CaveNodeKind::Sinkhole)
                    && node.position.x >= spec.origin_x
                    && node.position.x < max_x
                    && node.position.z >= spec.origin_z
                    && node.position.z < max_z
                {
                    let local_x = active_grid_offset(
                        node.position.x,
                        spec.origin_x,
                        spec.spacing_meters,
                        cells_x,
                    )
                    .ok_or(ActiveWaterError::InvalidCell)?;
                    let local_z = active_grid_offset(
                        node.position.z,
                        spec.origin_z,
                        spec.spacing_meters,
                        cells_z,
                    )
                    .ok_or(ActiveWaterError::InvalidCell)?;
                    let surface_id = surface_ids[local_z * cells_x + local_x];
                    connections.push(WaterConnection {
                        from: surface_id,
                        to: Some(id),
                        sill_elevation_meters: node.position.y,
                        width_meters: node.radius_meters,
                        conductance: 0.11,
                    });
                }
            }
            for river in &system.graph.underground_rivers {
                let to = system.graph.nodes[river.flow_to];
                connections.push(WaterConnection {
                    from: cave_ids[river.flow_from],
                    to: Some(cave_ids[river.flow_to]),
                    sill_elevation_meters: to.position.y - (to.radius_meters * 0.56),
                    width_meters: river.width_meters,
                    conductance: 0.16,
                });
                let from_slot = cells
                    .iter()
                    .position(|cell| cell.id == cave_ids[river.flow_from])
                    .ok_or(ActiveWaterError::MissingCell)?;
                cells[from_slot].source_cubic_meters_per_second +=
                    river.discharge_cubic_meters_per_second * 0.08;
            }
        }
        ActiveWaterRegion::new(cells, connections)
    }

    /// Resolves a surface sample to the stable cell identity used by
    /// [`Self::active_water_region`].
    pub fn active_water_cell_id_at(
        &self,
        spec: ActiveWaterRegionSpec,
        x: f64,
        z: f64,
    ) -> Option<WaterCellId> {
        if x < spec.origin_x
            || z < spec.origin_z
            || x >= spec.origin_x + usize_as_f64(spec.cell_counts[0]) * spec.spacing_meters
            || z >= spec.origin_z + usize_as_f64(spec.cell_counts[1]) * spec.spacing_meters
        {
            return None;
        }
        let lattice = CellIndex::containing(x, z, 0, spec.spacing_meters)?;
        Some(WaterCellId(
            lattice.generation_key(self.world(), DOMAIN_SURFACE_WATER_CELL),
        ))
    }

    /// Samples equilibrium wetland ecology using cached lake and river artifacts.
    pub fn wetland_at(&self, x: f64, z: f64) -> Option<WetlandSample> {
        if self.world().generator_version < WETLAND_GENERATOR_VERSION {
            return None;
        }
        let shape = self.shaped_height(x, z)?;
        let cell = DrainageCellIndex::containing(x, z)?;
        let lake_depth = self
            .lake_network(WatershedRegionIndex::containing_cell(cell))
            .and_then(|network| network.lake_for_cell(cell))
            .and_then(|lake| lake.water_depth_at(shape.height))
            .unwrap_or(0.0);
        let ocean_depth = if self.world().generator_version >= REEF_GENERATOR_VERSION {
            (-shape.height).max(0.0)
        } else {
            0.0
        };
        let equilibrium_water_depth_meters = lake_depth.max(ocean_depth);
        let floodplain_fraction = shape.river.map_or(0.0, |river| {
            let outside_channel = smoothstep(
                river.channel_half_width_meters,
                river.channel_half_width_meters * 2.5,
                river.distance_meters,
            );
            river.blend * outside_channel
        });
        let discharge = shape
            .river
            .map_or(0.0, |river| river.segment.discharge_cubic_meters_per_second);
        let hydrology = WetlandHydrology::new(
            shape.height,
            equilibrium_water_depth_meters,
            floodplain_fraction,
            discharge,
        )?;
        WetlandDistribution::new(self.world()).sample(x, z, hydrology)
    }

    /// Samples environmentally constrained reef growth at one position.
    pub fn reef_at(&self, x: f64, z: f64) -> Option<ReefSample> {
        ReefDistribution::new(self.world()).sample(x, z)
    }

    fn lake_network(&self, region: WatershedRegionIndex) -> Option<Arc<LakeNetwork>> {
        let slot = network_slot(&self.lake_networks, region);
        slot.get_or_init(|| LakeNetwork::generate(self.world(), region).map(Arc::new))
            .clone()
    }

    fn cave_system(&self, region: CaveRegionIndex) -> Option<Arc<CaveSystem>> {
        if self.world().generator_version < CAVE_GENERATOR_VERSION {
            return None;
        }
        let slot = cave_slot(&self.cave_systems, region);
        slot.get_or_init(|| {
            let placement_surface = CavePlacementSurface {
                base: self.base,
                world: self.world(),
            };
            CaveSystem::generate(self.world(), region, &placement_surface).map(Arc::new)
        })
        .clone()
    }

    fn cave_systems_intersecting(
        &self,
        min_x: f64,
        min_z: f64,
        max_x: f64,
        max_z: f64,
    ) -> Vec<Arc<CaveSystem>> {
        if self.world().generator_version < CAVE_GENERATOR_VERSION
            || !min_x.is_finite()
            || !min_z.is_finite()
            || !max_x.is_finite()
            || !max_z.is_finite()
            || min_x > max_x
            || min_z > max_z
        {
            return Vec::new();
        }
        let Some(minimum) = CaveRegionIndex::containing(min_x, min_z) else {
            return Vec::new();
        };
        let Some(maximum) = CaveRegionIndex::containing(max_x, max_z) else {
            return Vec::new();
        };
        let mut systems = BTreeMap::new();
        for region_z in minimum.z.saturating_sub(1)..=maximum.z.saturating_add(1) {
            for region_x in minimum.x.saturating_sub(1)..=maximum.x.saturating_add(1) {
                let neighborhood = self.cave_neighborhood(CaveRegionIndex::new(region_x, region_z));
                for system in neighborhood.iter() {
                    if system
                        .bounds
                        .intersects_horizontal(min_x, min_z, max_x, max_z)
                    {
                        systems.insert(system.system_key, Arc::clone(system));
                    }
                }
            }
        }
        systems.into_values().collect()
    }

    fn cave_neighborhood(&self, center: CaveRegionIndex) -> Arc<Vec<Arc<CaveSystem>>> {
        if let Some(systems) = self
            .cave_neighborhoods
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&center)
            .cloned()
        {
            return systems;
        }
        let mut systems = Vec::new();
        for z_offset in -1_i64..=1 {
            for x_offset in -1_i64..=1 {
                let region = CaveRegionIndex::new(
                    center.x.saturating_add(x_offset),
                    center.z.saturating_add(z_offset),
                );
                if let Some(system) = self.cave_system(region) {
                    systems.push(system);
                }
            }
        }
        systems.sort_by_key(|system| system.system_key);
        let systems = Arc::new(systems);
        Arc::clone(
            self.cave_neighborhoods
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(center)
                .or_insert(systems),
        )
    }

    /// Returns the strongest cave subtraction at a 3D world position.
    ///
    /// Samples that fall outside every passage's evaluated reach are reported
    /// as absent rather than at the subtraction field's saturated floor, so
    /// composing the result never clamps terrain density inside a system's
    /// bounding box.
    pub fn cave_influence_at(&self, position: WorldPosition) -> Option<CaveInfluence> {
        if self.base.is_surveyed_tile() || self.world().generator_version < CAVE_GENERATOR_VERSION {
            return None;
        }
        let region = CaveRegionIndex::containing(position.x, position.z)?;
        self.cave_neighborhood(region)
            .iter()
            .filter(|system| {
                const CAVE_SAMPLE_MARGIN_METERS: f64 = CaveInfluence::REACH_METERS;
                position.x >= system.bounds.min.x - CAVE_SAMPLE_MARGIN_METERS
                    && position.x <= system.bounds.max.x + CAVE_SAMPLE_MARGIN_METERS
                    && position.y >= system.bounds.min.y - CAVE_SAMPLE_MARGIN_METERS
                    && position.y <= system.bounds.max.y + CAVE_SAMPLE_MARGIN_METERS
                    && position.z >= system.bounds.min.z - CAVE_SAMPLE_MARGIN_METERS
                    && position.z <= system.bounds.max.z + CAVE_SAMPLE_MARGIN_METERS
            })
            .map(|system| system.influence_at(position))
            .filter(|influence| influence.is_within_reach())
            .max_by(|left, right| {
                left.void_density
                    .total_cmp(&right.void_density)
                    .then_with(|| left.system_key.cmp(&right.system_key).reverse())
            })
    }

    /// Describes a cave footprint below one top-down inspection position.
    pub fn cave_map_at(&self, x: f64, z: f64) -> Option<CaveMapSample> {
        let system = self
            .cave_systems_intersecting(x, z, x, z)
            .into_iter()
            .filter_map(|system| {
                let distance = system.horizontal_distance_at(x, z);
                (distance <= 0.0).then_some((distance, system))
            })
            .min_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| left.1.system_key.cmp(&right.1.system_key))
            })?;
        let surface = self.shaped_height(x, z)?.height;
        let highest = system
            .1
            .graph
            .nodes
            .iter()
            .filter(|node| !matches!(node.kind, CaveNodeKind::Entrance | CaveNodeKind::Sinkhole))
            .map(|node| node.position.y + node.radius_meters)
            .fold(f64::NEG_INFINITY, f64::max);
        Some(CaveMapSample {
            family: system.1.family,
            system_key: system.1.system_key,
            depth_below_surface_meters: (surface - highest).max(0.0),
            horizontal_distance_meters: system.0,
            has_underground_river: !system.1.graph.underground_rivers.is_empty(),
        })
    }

    /// Finds the nearest generated surface connection in a square region
    /// search, used by inspection tools and the prototype's cave warp.
    pub fn nearest_cave_entrance(
        &self,
        position: WorldPosition,
        region_radius: u64,
    ) -> Option<CaveEntrance> {
        if self.base.is_surveyed_tile() {
            return None;
        }
        let center = CaveRegionIndex::containing(position.x, position.z)?;
        let radius = i64::try_from(region_radius).ok()?;
        let mut nearest = None;
        for region_z in center.z.saturating_sub(radius)..=center.z.saturating_add(radius) {
            for region_x in center.x.saturating_sub(radius)..=center.x.saturating_add(radius) {
                let Some(system) = self.cave_system(CaveRegionIndex::new(region_x, region_z))
                else {
                    continue;
                };
                for entrance in system.entrances() {
                    let distance_squared = ((entrance.position.x - position.x)
                        * (entrance.position.x - position.x))
                        + ((entrance.position.z - position.z) * (entrance.position.z - position.z));
                    if nearest.is_none_or(|(current_distance, current): (f64, CaveEntrance)| {
                        distance_squared < current_distance
                            || (distance_squared.to_bits() == current_distance.to_bits()
                                && entrance.system_key < current.system_key)
                    }) {
                        nearest = Some((distance_squared, entrance));
                    }
                }
            }
        }
        nearest.map(|(_, entrance)| entrance)
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
        let mut mesh = lake_surface_grid(self, grid)?;
        if !self.base.is_surveyed_tile()
            && let TerrainMeshSpec::Near(near) = spec
        {
            self.append_underground_rivers(&mut mesh, near.chunk)?;
        }
        Ok(mesh)
    }

    fn append_underground_rivers(
        &self,
        mesh: &mut Mesh,
        chunk: ChunkIndex,
    ) -> Result<(), MeshingError> {
        let origin = chunk.sample_origin();
        let edge_meters = ChunkIndex::edge_meters();
        let max_x = origin.x + edge_meters;
        let max_z = origin.z + edge_meters;
        for system in self.cave_systems_intersecting(origin.x, origin.z, max_x, max_z) {
            for river in &system.graph.underground_rivers {
                let edge = system.graph.edges[river.edge_index];
                let start = system.graph.nodes[edge.from];
                let end = system.graph.nodes[edge.to];
                if let Some((clipped_start, clipped_end)) =
                    clip_cave_edge_to_chunk(start, end, origin.x, origin.z, max_x, max_z)
                {
                    append_underground_river_quad(mesh, clipped_start, clipped_end, *river)?;
                }
            }
        }
        Ok(())
    }

    /// Builds the visible terrain representation for the requested terrain
    /// tier. Vegetation is streamed independently so terrain LOD never replaces
    /// individual trees with a continuous canopy surface.
    ///
    /// # Errors
    ///
    /// Returns [`MeshingError`] when the requested terrain LOD is unsupported,
    /// a surface sample is unavailable, or the combined mesh exceeds index
    /// capacity.
    pub fn render_mesh(&self, spec: TerrainMeshSpec) -> Result<Mesh, MeshingError> {
        let mut mesh = match spec {
            TerrainMeshSpec::Near(spec) => {
                transvoxel_chunk(self, spec.chunk, spec.lod, spec.transition_faces)
            }
            TerrainMeshSpec::Far(spec) => far_terrain_mesh(self, spec),
        }?;
        self.apply_ecosystem_surface_colors(&mut mesh);
        Ok(mesh)
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
            reef: shape.reef,
            final_height_meters: shape.height,
        })
    }

    fn shaped_height(&self, x: f64, z: f64) -> Option<TerrainShape> {
        if !x.is_finite() || !z.is_finite() {
            return None;
        }
        let key = (x.to_bits(), z.to_bits());
        if let Some(shape) = self
            .terrain_shapes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .copied()
        {
            return Some(shape);
        }
        if self.base.is_surveyed_tile() {
            let shape = TerrainShape {
                height: self.base.height_at(x, z)?,
                erosion: None,
                gully: None,
                river: None,
                reef: None,
            };
            let mut cache = self
                .terrain_shapes
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if cache.len() >= MAX_TERRAIN_SHAPE_CACHE_ENTRIES {
                cache.clear();
            }
            cache.insert(key, shape);
            return Some(shape);
        }
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
        let hydrological_height = river.map_or(gully_height, |river| {
            let channel_bed = river.centerline_elevation_meters - river.incision_depth_meters;
            let target = gully_height.min(channel_bed);
            let incised = gully_height + ((target - gully_height) * river.blend);
            if self.world().generator_version < LIVING_WATER_GENERATOR_VERSION {
                return incised;
            }
            let morphology_depth = river.fast_water.map_or(0.0, |feature| {
                (feature.plunge_pool_depth_meters * feature.plunge_pool_blend)
                    .max(feature.downstream_gorge_depth_meters * feature.gorge_blend)
            });
            incised - morphology_depth
        });
        let reef = ReefDistribution::new(self.world()).sample(x, z);
        let height = hydrological_height + reef.map_or(0.0, |reef| reef.framework_height_meters);
        let shape = TerrainShape {
            height,
            erosion,
            gully,
            river,
            reef,
        };
        let mut cache = self
            .terrain_shapes
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= MAX_TERRAIN_SHAPE_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(key, shape);
        Some(shape)
    }

    fn apply_ecosystem_surface_colors(&self, mesh: &mut Mesh) {
        mesh.colors.clear();
        mesh.colors.reserve(mesh.positions.len());
        for position in &mesh.positions {
            let world_position = WorldPosition::new(position[0], position[1], position[2]);
            if let Some(cave) = self.cave_influence_at(world_position)
                && cave.void_density >= -2.5
                && self
                    .shaped_height(position[0], position[2])
                    .is_some_and(|surface| position[1] < surface.height - 1.5)
            {
                mesh.colors.push(cave_wall_color(cave.family));
                continue;
            }
            mesh.colors.push(
                self.surface_color_at(position[0], position[2])
                    .unwrap_or([1.0, 1.0, 1.0, 0.0]),
            );
        }
        if mesh.colors.iter().all(|color| color[3] <= f32::EPSILON) {
            mesh.colors.clear();
        }
    }

    /// Returns the world-aligned terrain material color shared by near and far meshes.
    pub fn surface_color_at(&self, x: f64, z: f64) -> Option<[f32; 4]> {
        if !x.is_finite() || !z.is_finite() {
            return None;
        }
        if self.base.is_surveyed_tile() {
            return self.base.surveyed_color_at(x, z);
        }
        geography_surface_color(&SurfaceColorInputs {
            profile: RegionalProfile::sample(self.world(), x, z),
            soil: Soil::new(self.world()).sample(x, z),
            forest: ForestDistribution::new(self.world()).sample(x, z),
            ground: GroundVegetationDistribution::new(self.world()).sample(x, z),
            erosion: self.erosion_at(x, z),
            wetland: self.wetland_at(x, z),
            reef: self.reef_at(x, z),
            ecosystem: EcosystemDistribution::new(self.world()).sample(x, z),
        })
    }
}

fn network_slot<T>(cache: &NetworkCache<T>, region: WatershedRegionIndex) -> NetworkSlot<T> {
    if let Some(slot) = cache
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&region)
        .cloned()
    {
        return slot;
    }

    Arc::clone(
        cache
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(region)
            .or_insert_with(|| Arc::new(OnceLock::new())),
    )
}

fn append_surface_water_connection(
    connections: &mut Vec<WaterConnection>,
    cells: &[WaterCell],
    first_id: WaterCellId,
    second_id: WaterCellId,
    spacing_meters: f64,
) -> Result<(), ActiveWaterError> {
    let first = cells
        .iter()
        .find(|cell| cell.id == first_id)
        .ok_or(ActiveWaterError::MissingCell)?;
    let second = cells
        .iter()
        .find(|cell| cell.id == second_id)
        .ok_or(ActiveWaterError::MissingCell)?;
    let first_surface = first.surface_elevation_meters();
    let second_surface = second.surface_elevation_meters();
    let (from, to) = if first_surface > second_surface
        || (first_surface.to_bits() == second_surface.to_bits() && first.id > second.id)
    {
        (first, second)
    } else {
        (second, first)
    };
    connections.push(WaterConnection {
        from: from.id,
        to: Some(to.id),
        sill_elevation_meters: from.bed_elevation_meters.max(to.bed_elevation_meters),
        width_meters: spacing_meters,
        conductance: 0.035,
    });
    Ok(())
}

fn active_grid_offset(
    coordinate: f64,
    origin: f64,
    spacing_meters: f64,
    count: usize,
) -> Option<usize> {
    let offset = libm::floor((coordinate - origin) / spacing_meters);
    if !(0.0..usize_as_f64(count)).contains(&offset) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(offset as usize)
}

fn cave_slot(cache: &CaveCache, region: CaveRegionIndex) -> NetworkSlot<CaveSystem> {
    if let Some(slot) = cache
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&region)
        .cloned()
    {
        return slot;
    }

    Arc::clone(
        cache
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(region)
            .or_insert_with(|| Arc::new(OnceLock::new())),
    )
}

fn smoothstep(edge0: f64, edge1: f64, value: f64) -> f64 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - (2.0 * t))
}

#[derive(Clone, Copy, Debug)]
struct SurfaceColorInputs {
    profile: Option<RegionalProfile>,
    soil: Option<SoilSample>,
    forest: Option<ForestSample>,
    ground: Option<GroundVegetationSample>,
    erosion: Option<WorldErosionSample>,
    wetland: Option<WetlandSample>,
    reef: Option<ReefSample>,
    ecosystem: Option<EcosystemSample>,
}

#[allow(clippy::too_many_lines)]
fn geography_surface_color(inputs: &SurfaceColorInputs) -> Option<[f32; 4]> {
    let profile = inputs.profile?;
    let soil = inputs.soil?;
    let erosion = inputs.erosion?.surface;
    let moisture = f64_as_f32(soil.surface_moisture);
    let organic = f64_as_f32(soil.organic_matter_fraction / 0.17);
    let carbonate = f64_as_f32(profile.karst_probability);
    let hardness = f64_as_f32(profile.rock_hardness);
    let sediment = f64_as_f32((erosion.sediment_deposition_meters / 18.0).clamp(0.0, 1.0));
    let rock_exposure = f64_as_f32(erosion.rock_exposure);
    let scree = f64_as_f32(erosion.scree_cover);

    let mineral_rock = [
        0.31 + (carbonate * 0.16) + (hardness * 0.05),
        0.30 + (carbonate * 0.15) + (hardness * 0.04),
        0.28 + (carbonate * 0.11),
    ];
    let mineral_soil = [
        0.25 + (f64_as_f32(soil.composition.sand_fraction) * 0.15),
        0.17 + (organic * 0.06) + (moisture * 0.04),
        0.09 + (f64_as_f32(soil.composition.clay_fraction) * 0.08),
    ];
    let sediment_color = [0.43, 0.34, 0.20];
    let vegetation_color = [
        0.12 + ((1.0 - moisture) * 0.13),
        0.25 + (moisture * 0.15),
        0.09 + (moisture * 0.07),
    ];
    let canopy = f64_as_f32(
        inputs
            .forest
            .map_or(0.0, |forest| forest.canopy_cover_fraction),
    );
    let ground_cover = f64_as_f32(
        inputs
            .ground
            .map_or(0.0, |ground| ground.ground_cover_fraction),
    );
    let vegetation = (ground_cover * (1.0 - (canopy * 0.35)) + (canopy * 0.45))
        * (1.0 - rock_exposure)
        * (1.0 - (scree * 0.7));

    let soil_base = mix_rgb(mineral_soil, sediment_color, sediment * 0.58);
    let substrate = mix_rgb(
        soil_base,
        mineral_rock,
        (rock_exposure + (scree * 0.55)).clamp(0.0, 1.0),
    );
    let mut color = mix_rgb(substrate, vegetation_color, vegetation.clamp(0.0, 0.82));
    let mut strength: f32 = 0.88;

    if let Some(ecosystem) = inputs.ecosystem {
        let potentials = ecosystem.relative_potentials();
        let ecosystem_colors = [
            [0.055, 0.205, 0.075],
            [0.22, 0.31, 0.105],
            [0.38, 0.46, 0.13],
            [0.50, 0.41, 0.18],
            [0.42, 0.30, 0.17],
            [0.66, 0.48, 0.24],
            [0.40, 0.46, 0.34],
            [0.43, 0.44, 0.45],
            [0.20, 0.39, 0.25],
        ];
        let mut ecosystem_color = [0.0_f32; 3];
        for (potential, regime_color) in potentials.into_iter().zip(ecosystem_colors) {
            for channel in 0..3 {
                ecosystem_color[channel] += f64_as_f32(potential) * regime_color[channel];
            }
        }
        let strongest = ecosystem.potentials().into_iter().fold(0.0_f64, f64::max);
        let ecosystem_blend = f64_as_f32((0.18 + (strongest * 0.64)).clamp(0.18, 0.72));
        color = mix_rgb(color, ecosystem_color, ecosystem_blend);

        let playa = inputs.wetland.map_or(0.0, |wetland| wetland.playa_fraction);
        let salt_pan = (ecosystem.salinity_fraction
            * ecosystem.closed_basin_fraction
            * (0.42 + (playa * 0.88)))
            .clamp(0.0, 1.0);
        if salt_pan > 0.04 {
            color = mix_rgb(
                color,
                [0.78, 0.76, 0.66],
                f64_as_f32((salt_pan * 0.88).clamp(0.0, 0.82)),
            );
        }
    }

    let reef_strength = inputs.reef.map_or(0.0, |sample| sample.coverage_fraction);
    let wetland_strength = inputs
        .wetland
        .map_or(0.0, |sample| sample.coverage_fraction);
    if reef_strength > wetland_strength {
        let reef = inputs.reef?;
        let lagoon = f64_as_f32(reef.lagoon_fraction);
        let reef_color = [
            0.54 + (lagoon * 0.08),
            0.38 + (lagoon * 0.18),
            0.20 + (lagoon * 0.10),
        ];
        let reef_blend = f64_as_f32(reef_strength * 0.92);
        color = mix_rgb(color, reef_color, reef_blend);
        strength = strength.max(reef_blend);
    } else if let Some(wetland) = inputs.wetland {
        let wetland_color = match wetland.dominant_kind() {
            WetlandKind::EmergentMarsh => [0.28, 0.40, 0.12],
            WetlandKind::ForestedSwamp => [0.12, 0.28, 0.13],
            WetlandKind::Peatland => [0.29, 0.25, 0.13],
            WetlandKind::SeasonalWetland => [0.40, 0.39, 0.16],
            WetlandKind::SaltMarsh => [0.37, 0.47, 0.25],
        };
        let wetland_blend = f64_as_f32(wetland_strength * 0.88);
        color = mix_rgb(color, wetland_color, wetland_blend);
        strength = strength.max(wetland_blend);
    }

    Some([color[0], color[1], color[2], strength])
}

fn mix_rgb(start: [f32; 3], end: [f32; 3], amount: f32) -> [f32; 3] {
    std::array::from_fn(|channel| start[channel] + ((end[channel] - start[channel]) * amount))
}

const fn cave_wall_color(family: CaveFamily) -> [f32; 4] {
    match family {
        CaveFamily::Karst => [0.38, 0.36, 0.30, 0.92],
        CaveFamily::LavaTube => [0.20, 0.18, 0.17, 0.94],
        CaveFamily::Fault => [0.27, 0.27, 0.28, 0.93],
        CaveFamily::Sea => [0.25, 0.32, 0.34, 0.90],
        CaveFamily::Talus => [0.31, 0.29, 0.27, 0.90],
        CaveFamily::Glacial => [0.34, 0.40, 0.44, 0.91],
        CaveFamily::Erosional => [0.32, 0.28, 0.23, 0.91],
    }
}

fn blend_color(start: [f32; 4], end: [f32; 4], amount: f32) -> [f32; 4] {
    std::array::from_fn(|channel| start[channel] + ((end[channel] - start[channel]) * amount))
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[derive(Clone, Copy, Debug)]
struct TerrainShape {
    height: f64,
    erosion: Option<ErosionSurfaceSample>,
    gully: Option<GullyTerrainInfluence>,
    river: Option<RiverTerrainInfluence>,
    reef: Option<ReefSample>,
}

#[derive(Clone, Copy, Debug)]
struct CavePlacementSurface {
    base: WildernessTerrain,
    world: WorldIdentity,
}

impl SurfaceField for CavePlacementSurface {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        let base_height = if self.world.generator_version >= EROSION_GENERATOR_VERSION {
            self.base.erosion_at(x, z)?.surface_height_meters()
        } else {
            self.base.height_at(x, z)?
        };
        let reef_height = ReefDistribution::new(self.world)
            .sample(x, z)
            .map_or(0.0, |reef| reef.framework_height_meters);
        Some(base_height + reef_height)
    }
}

impl DensityField for GeneratedWorldTerrain {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        let Some(shape) = self.shaped_height(position.x, position.z) else {
            return TerrainSample::new(f64::INFINITY, Material::Air);
        };
        let density = self.base.density_at_surface(position, shape.height);
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
        if let Some(cave) = self.cave_influence_at(position) {
            let carved_density = density.max(cave.void_density);
            if carved_density > 0.0 {
                return TerrainSample::new(carved_density, Material::Air);
            }
            if cave.void_density > density {
                return TerrainSample::new(carved_density, Material::Rock);
            }
        }
        TerrainSample::new(density, material)
    }
}

impl SurfaceField for GeneratedWorldTerrain {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        self.shaped_height(x, z).map(|shape| shape.height)
    }

    fn volume_bounds(&self, min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> Option<(f64, f64)> {
        if self.base.is_surveyed_tile() {
            return None;
        }
        let systems = self.cave_systems_intersecting(min_x, min_z, max_x, max_z);
        let cave_minimum = systems
            .iter()
            .filter_map(|system| system.vertical_bounds_in(min_x, min_z, max_x, max_z))
            .map(|bounds| bounds.0)
            .fold(f64::INFINITY, f64::min);
        let cave_maximum = systems
            .iter()
            .filter_map(|system| system.vertical_bounds_in(min_x, min_z, max_x, max_z))
            .map(|bounds| bounds.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let terrain_bounds = self
            .base
            .undercut_depth_in(min_x, min_z, max_x, max_z)
            .and_then(|depth| {
                let mut minimum = f64::INFINITY;
                let mut maximum = f64::NEG_INFINITY;
                for z_fraction in [0.0, 0.5, 1.0] {
                    for x_fraction in [0.0, 0.5, 1.0] {
                        let x = min_x + ((max_x - min_x) * x_fraction);
                        let z = min_z + ((max_z - min_z) * z_fraction);
                        let surface = self.shaped_height(x, z)?.height;
                        minimum = minimum.min(surface - depth - 2.0);
                        maximum = maximum.max(surface + 2.0);
                    }
                }
                Some((minimum, maximum))
            });
        match (cave_minimum.is_finite(), terrain_bounds) {
            (true, Some((terrain_minimum, terrain_maximum))) => Some((
                cave_minimum.min(terrain_minimum),
                cave_maximum.max(terrain_maximum),
            )),
            (true, None) => Some((cave_minimum, cave_maximum)),
            (false, bounds) => bounds,
        }
    }
}

fn lake_surface_grid(
    terrain: &GeneratedWorldTerrain,
    spec: SurfaceGridSpec,
) -> Result<Mesh, MeshingError> {
    const WATER_RENDER_OFFSET_METERS: f64 = 0.05;
    const OCEAN_WATER_COLOR: [f32; 4] = [0.02, 0.29, 0.52, 1.0];
    const RIVER_WATER_COLOR: [f32; 4] = [0.035, 0.31, 0.49, 1.0];

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
            let (surface, color) = if let Some(ocean) = terrain.ocean_surface_at(center_x, center_z)
            {
                let reef = terrain.reef_at(center_x, center_z);
                let reef_cover = reef.map_or(0.0, |sample| sample.coverage_fraction);
                let color = blend_color(
                    OCEAN_WATER_COLOR,
                    [0.08, 0.62, 0.66, 1.0],
                    f64_as_f32(reef_cover * 0.72),
                );
                (
                    ocean.surface_elevation_meters + WATER_RENDER_OFFSET_METERS,
                    color,
                )
            } else if let Some(water) = terrain.lake_surface_at(center_x, center_z) {
                let wetland = terrain.wetland_at(center_x, center_z);
                let wetland_cover = wetland.map_or(0.0, |sample| sample.coverage_fraction);
                let color = lake_water_color(wetland_cover, water.lake.salinity_fraction);
                (
                    water.lake.surface_elevation_meters + WATER_RENDER_OFFSET_METERS,
                    color,
                )
            } else if terrain.world().generator_version >= LIVING_WATER_GENERATOR_VERSION
                && let Some(river) = terrain.river_influence_at(center_x, center_z)
                && river.distance_meters
                    <= river.channel_half_width_meters + (spec.spacing_meters * 0.6)
            {
                let bed = terrain
                    .surface_height(center_x, center_z)
                    .ok_or(MeshingError::MissingSurface)?;
                let depth = (0.12
                    + (libm::sqrt(river.segment.discharge_cubic_meters_per_second) * 0.08))
                    .clamp(0.12, 2.0);
                (bed + depth + WATER_RENDER_OFFSET_METERS, RIVER_WATER_COLOR)
            } else {
                continue;
            };
            let vertex_offset =
                u32::try_from(mesh.positions.len()).map_err(|_| MeshingError::TooManyVertices)?;
            mesh.positions.extend([
                [min_x, surface, min_z],
                [min_x, surface, max_z],
                [max_x, surface, min_z],
                [max_x, surface, max_z],
            ]);
            mesh.normals.extend([[0.0, 1.0, 0.0]; 4]);
            mesh.colors.extend([color; 4]);
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

fn lake_water_color(wetland_cover: f64, salinity: f64) -> [f32; 4] {
    let freshwater = blend_color(
        [0.04, 0.34, 0.58, 1.0],
        [0.18, 0.40, 0.24, 1.0],
        f64_as_f32(wetland_cover * 0.58),
    );
    blend_color(
        freshwater,
        [0.20, 0.48, 0.44, 1.0],
        f64_as_f32(salinity * 0.62),
    )
}

fn append_underground_river_quad(
    mesh: &mut Mesh,
    start: CaveNode,
    end: CaveNode,
    river: UndergroundRiver,
) -> Result<(), MeshingError> {
    const CAVE_WATER_COLOR: [f32; 4] = [0.03, 0.24, 0.34, 1.0];
    let direction_x = end.position.x - start.position.x;
    let direction_z = end.position.z - start.position.z;
    let horizontal_length = libm::hypot(direction_x, direction_z);
    if horizontal_length <= f64::EPSILON {
        return Ok(());
    }
    let half_width = river.width_meters * 0.5;
    let side_x = (-direction_z / horizontal_length) * half_width;
    let side_z = (direction_x / horizontal_length) * half_width;
    let start_y = start.position.y - (start.radius_meters * 0.56);
    let end_y = end.position.y - (end.radius_meters * 0.56);
    let vertex_offset =
        u32::try_from(mesh.positions.len()).map_err(|_| MeshingError::TooManyVertices)?;
    mesh.positions.extend([
        [
            start.position.x - side_x,
            start_y,
            start.position.z - side_z,
        ],
        [
            start.position.x + side_x,
            start_y,
            start.position.z + side_z,
        ],
        [end.position.x - side_x, end_y, end.position.z - side_z],
        [end.position.x + side_x, end_y, end.position.z + side_z],
    ]);
    mesh.normals.extend([[0.0, 1.0, 0.0]; 4]);
    if mesh.colors.is_empty() && vertex_offset > 0 {
        let existing = usize::try_from(vertex_offset).map_err(|_| MeshingError::TooManyVertices)?;
        mesh.colors.resize(existing, [1.0, 1.0, 1.0, 0.0]);
    }
    mesh.colors.extend([CAVE_WATER_COLOR; 4]);
    mesh.indices.extend([
        vertex_offset,
        vertex_offset + 2,
        vertex_offset + 1,
        vertex_offset + 1,
        vertex_offset + 2,
        vertex_offset + 3,
    ]);
    Ok(())
}

fn clip_cave_edge_to_chunk(
    start: CaveNode,
    end: CaveNode,
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Option<(CaveNode, CaveNode)> {
    let delta = [
        end.position.x - start.position.x,
        end.position.y - start.position.y,
        end.position.z - start.position.z,
    ];
    let mut minimum_amount = 0.0_f64;
    let mut maximum_amount = 1.0_f64;
    for (origin, direction, minimum, maximum) in [
        (start.position.x, delta[0], min_x, max_x),
        (start.position.z, delta[2], min_z, max_z),
    ] {
        if direction.abs() <= f64::EPSILON {
            if origin < minimum || origin > maximum {
                return None;
            }
            continue;
        }
        let first = (minimum - origin) / direction;
        let second = (maximum - origin) / direction;
        minimum_amount = minimum_amount.max(first.min(second));
        maximum_amount = maximum_amount.min(first.max(second));
        if minimum_amount > maximum_amount {
            return None;
        }
    }
    let interpolate = |amount: f64| CaveNode {
        position: WorldPosition::new(
            start.position.x + (delta[0] * amount),
            start.position.y + (delta[1] * amount),
            start.position.z + (delta[2] * amount),
        ),
        kind: CaveNodeKind::Passage,
        radius_meters: start.radius_meters + ((end.radius_meters - start.radius_meters) * amount),
    };
    Some((interpolate(minimum_amount), interpolate(maximum_amount)))
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
    PrefetchTerrain,
    Vegetation,
    SurfaceDetail,
}

impl GenerationPriority {
    pub const fn code(self) -> u8 {
        match self {
            Self::PlayerTerrain => 0,
            Self::Horizon => 1,
            Self::FarTerrain => 2,
            Self::NearTerrain => 3,
            Self::PrefetchTerrain => 4,
            Self::Vegetation => 5,
            Self::SurfaceDetail => 6,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::PlayerTerrain),
            1 => Some(Self::Horizon),
            2 => Some(Self::FarTerrain),
            3 => Some(Self::NearTerrain),
            4 => Some(Self::PrefetchTerrain),
            5 => Some(Self::Vegetation),
            6 => Some(Self::SurfaceDetail),
            _ => None,
        }
    }
}

/// Complete inputs needed to regenerate either terrain representation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TerrainMeshSpec {
    Far(FarTerrainMeshSpec),
    Near(ChunkMeshSpec),
}

/// Completed output from one asynchronous terrain-mesh job.
#[derive(Clone, Debug)]
pub struct GeneratedTerrainMesh {
    pub spec: TerrainMeshSpec,
    pub priority: GenerationPriority,
    pub mesh: Result<Mesh, MeshingError>,
    pub lake_mesh: Option<Result<Mesh, MeshingError>>,
    pub terrain_generation_time: Duration,
    pub lake_generation_time: Duration,
    pub cache_hit: bool,
}

type LakeMeshGenerator<F> = fn(&F, TerrainMeshSpec) -> Result<Mesh, MeshingError>;
type TerrainMeshGenerator<F> = fn(&F, TerrainMeshSpec) -> Result<Mesh, MeshingError>;

#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_TERRAIN_MESH_CACHE_BYTES: usize = 192 * 1024 * 1024;
#[cfg(target_arch = "wasm32")]
const DEFAULT_TERRAIN_MESH_CACHE_BYTES: usize = 48 * 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_TERRAIN_MESH_CACHE_ENTRIES: usize = 2_048;
#[cfg(target_arch = "wasm32")]
const DEFAULT_TERRAIN_MESH_CACHE_ENTRIES: usize = 512;

#[derive(Clone, Debug)]
struct CachedTerrainMesh {
    generated: GeneratedTerrainMesh,
    bytes: usize,
    last_used: u64,
}

#[derive(Debug)]
struct TerrainMeshCache {
    entries: BTreeMap<TerrainMeshSpec, CachedTerrainMesh>,
    used_bytes: usize,
    max_bytes: usize,
    max_entries: usize,
    clock: u64,
}

impl TerrainMeshCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            used_bytes: 0,
            max_bytes,
            max_entries: DEFAULT_TERRAIN_MESH_CACHE_ENTRIES,
            clock: 0,
        }
    }

    fn contains(&self, spec: TerrainMeshSpec) -> bool {
        self.entries.contains_key(&spec)
    }

    fn get(
        &mut self,
        spec: TerrainMeshSpec,
        priority: GenerationPriority,
    ) -> Option<GeneratedTerrainMesh> {
        self.clock = self.clock.wrapping_add(1);
        let cached = self.entries.get_mut(&spec)?;
        cached.last_used = self.clock;
        let mut generated = cached.generated.clone();
        generated.priority = priority;
        generated.terrain_generation_time = Duration::ZERO;
        generated.lake_generation_time = Duration::ZERO;
        generated.cache_hit = true;
        Some(generated)
    }

    fn insert(&mut self, generated: &GeneratedTerrainMesh) {
        if generated.mesh.is_err() || generated.lake_mesh.as_ref().is_some_and(Result::is_err) {
            return;
        }
        let bytes = generated_terrain_mesh_bytes(generated);
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
            CachedTerrainMesh {
                generated: generated.clone(),
                bytes,
                last_used: self.clock,
            },
        );

        while self.used_bytes > self.max_bytes || self.entries.len() > self.max_entries {
            let Some((&oldest_spec, _)) = self
                .entries
                .iter()
                .min_by_key(|(spec, cached)| (cached.last_used, *spec))
            else {
                break;
            };
            let Some(removed) = self.entries.remove(&oldest_spec) else {
                break;
            };
            self.used_bytes = self.used_bytes.saturating_sub(removed.bytes);
        }
    }
}

fn generated_terrain_mesh_bytes(generated: &GeneratedTerrainMesh) -> usize {
    fn mesh_bytes(mesh: &Mesh) -> usize {
        mesh.positions
            .len()
            .saturating_mul(size_of::<[f64; 3]>())
            .saturating_add(mesh.normals.len().saturating_mul(size_of::<[f32; 3]>()))
            .saturating_add(mesh.colors.len().saturating_mul(size_of::<[f32; 4]>()))
            .saturating_add(mesh.indices.len().saturating_mul(size_of::<u32>()))
    }

    let terrain = generated.mesh.as_ref().map_or(0, mesh_bytes);
    let lake = generated
        .lake_mesh
        .as_ref()
        .and_then(|mesh| mesh.as_ref().ok())
        .map_or(0, mesh_bytes);
    terrain.saturating_add(lake)
}

/// Terrain generation queue ordered by visible generation priority.
///
/// Jobs already being generated are allowed to finish. Pending jobs always
/// start in priority order, with submission order breaking ties. Completion
/// order is deliberately not observable by generation itself: every mesh is a
/// pure function of its field and [`ChunkMeshSpec`]. Native builds use worker
/// threads. This type retains an incremental Wasm fallback; the player client
/// implements the same queue contract with independent Web Workers.
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
    terrain_mesh_generator: Option<TerrainMeshGenerator<F>>,
    #[cfg(target_arch = "wasm32")]
    lake_mesh_generator: Option<LakeMeshGenerator<F>>,
    #[cfg(target_arch = "wasm32")]
    pending: BinaryHeap<Reverse<QueuedTerrainMesh>>,
    #[cfg(target_arch = "wasm32")]
    cache: TerrainMeshCache,
    #[cfg(target_arch = "wasm32")]
    yield_after_mesh: bool,
    cached_ready: VecDeque<GeneratedTerrainMesh>,
    next_sequence: u64,
}

impl<F> TerrainMeshQueue<F>
where
    F: DensityField + SurfaceField + Send + Sync + 'static,
{
    /// Starts native workers while reserving one available hardware thread for
    /// the window, rendering, and simulation work.
    ///
    /// This queue's Wasm fallback generates incrementally on the calling thread.
    /// The player client instead uses independent message-passing Wasm workers,
    /// which do not require shared-memory response headers.
    pub fn new(field: F) -> Self {
        Self::with_optional_mesh_generators(field, None, None)
    }

    /// Starts terrain workers that also build the separate equilibrium-lake
    /// surface associated with each terrain mesh.
    pub fn with_lake_mesh(field: F, generator: LakeMeshGenerator<F>) -> Self {
        Self::with_optional_mesh_generators(field, None, Some(generator))
    }

    fn with_optional_mesh_generators(
        field: F,
        terrain_mesh_generator: Option<TerrainMeshGenerator<F>>,
        lake_mesh_generator: Option<LakeMeshGenerator<F>>,
    ) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let available = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
            let worker_count =
                NonZeroUsize::new(available.get().saturating_sub(1)).unwrap_or(NonZeroUsize::MIN);
            Self::with_worker_count_and_mesh_generators(
                field,
                worker_count,
                terrain_mesh_generator,
                lake_mesh_generator,
            )
        }

        #[cfg(target_arch = "wasm32")]
        {
            Self::with_worker_count_and_mesh_generators(
                field,
                NonZeroUsize::MIN,
                terrain_mesh_generator,
                lake_mesh_generator,
            )
        }
    }

    /// Starts an explicit non-zero number of native terrain workers.
    ///
    /// Browser builds ignore `worker_count` and use their incremental queue.
    pub fn with_worker_count(field: F, worker_count: NonZeroUsize) -> Self {
        Self::with_worker_count_and_mesh_generators(field, worker_count, None, None)
    }

    fn with_worker_count_and_mesh_generators(
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
                cache: Mutex::new(TerrainMeshCache::new(DEFAULT_TERRAIN_MESH_CACHE_BYTES)),
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
                cached_ready: VecDeque::new(),
                next_sequence: 0,
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = worker_count;
            Self {
                field,
                terrain_mesh_generator,
                lake_mesh_generator,
                pending: BinaryHeap::new(),
                cache: TerrainMeshCache::new(DEFAULT_TERRAIN_MESH_CACHE_BYTES),
                yield_after_mesh: false,
                cached_ready: VecDeque::new(),
                next_sequence: 0,
            }
        }
    }

    /// Adds a deterministic chunk request without blocking for generation.
    pub fn enqueue(&mut self, priority: GenerationPriority, spec: TerrainMeshSpec) {
        if let Some(generated) = self.cached(spec, priority) {
            self.cached_ready.push_back(generated);
            return;
        }
        self.queue_if_missing(priority, spec);
    }

    /// Schedules low-priority generation only when the exact mesh is not
    /// already cached, queued, or being generated.
    ///
    /// Unlike [`Self::enqueue`], a cache hit does not emit a completion. This
    /// makes the method safe to call every frame for predictive prewarming.
    pub fn prewarm(&mut self, spec: TerrainMeshSpec) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self
            .shared
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(spec)
        {
            return false;
        }
        #[cfg(target_arch = "wasm32")]
        if self.cache.contains(spec) {
            return false;
        }

        self.queue_if_missing(GenerationPriority::PrefetchTerrain, spec)
    }

    /// Drops obsolete speculative jobs while preserving visible and in-flight
    /// work.
    pub fn retain_prewarm(&mut self, desired: &BTreeSet<TerrainMeshSpec>) {
        #[cfg(not(target_arch = "wasm32"))]
        self.shared
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .jobs
            .retain(|queued| {
                queued.0.priority != GenerationPriority::PrefetchTerrain
                    || desired.contains(&queued.0.spec)
            });

        #[cfg(target_arch = "wasm32")]
        self.pending.retain(|queued| {
            queued.0.priority != GenerationPriority::PrefetchTerrain
                || desired.contains(&queued.0.spec)
        });
    }

    fn cached(
        &mut self,
        spec: TerrainMeshSpec,
        priority: GenerationPriority,
    ) -> Option<GeneratedTerrainMesh> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.shared
                .cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(spec, priority)
        }

        #[cfg(target_arch = "wasm32")]
        {
            self.cache.get(spec, priority)
        }
    }

    fn queue_if_missing(&mut self, priority: GenerationPriority, spec: TerrainMeshSpec) -> bool {
        let job = QueuedTerrainMesh {
            priority,
            sequence: self.next_sequence,
            spec,
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut pending = self
                .shared
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if pending.in_flight.contains(&spec) {
                return false;
            }
            if let Some(existing) = pending.jobs.iter().find(|queued| queued.0.spec == spec) {
                if existing.0.priority <= priority {
                    return false;
                }
                pending.jobs.retain(|queued| queued.0.spec != spec);
            }
            pending.jobs.push(Reverse(job));
            drop(pending);
            self.shared.wake_workers.notify_one();
        }

        #[cfg(target_arch = "wasm32")]
        {
            if let Some(existing) = self.pending.iter().find(|queued| queued.0.spec == spec) {
                if existing.0.priority <= priority {
                    return false;
                }
                self.pending.retain(|queued| queued.0.spec != spec);
            }
            self.pending.push(Reverse(job));
        }

        self.next_sequence = self.next_sequence.wrapping_add(1);
        true
    }

    /// Removes a job that has not yet started.
    ///
    /// Jobs already owned by a worker may still complete and are rejected by
    /// the request-generation check at integration time.
    pub fn cancel(&mut self, spec: TerrainMeshSpec) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        let jobs = &mut self
            .shared
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .jobs;
        #[cfg(target_arch = "wasm32")]
        let jobs = &mut self.pending;

        let previous_len = jobs.len();
        jobs.retain(|queued| queued.0.spec != spec);
        jobs.len() != previous_len
    }

    /// Returns one completed mesh without waiting for a worker.
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
            if self.yield_after_mesh {
                self.yield_after_mesh = false;
                return None;
            }
            let Reverse(job) = self.pending.pop()?;
            let generated = generate_terrain_mesh(
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
}

impl TerrainMeshQueue<GeneratedWorldTerrain> {
    /// Starts the player-world queue with terrain, lake, and distant-forest
    /// render representations generated on the same worker threads.
    pub fn for_generated_world(field: GeneratedWorldTerrain) -> Self {
        Self::with_optional_mesh_generators(
            field,
            Some(GeneratedWorldTerrain::render_mesh),
            Some(GeneratedWorldTerrain::lake_surface_mesh),
        )
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
    terrain_mesh_generator: Option<TerrainMeshGenerator<F>>,
    lake_mesh_generator: Option<LakeMeshGenerator<F>>,
    pending: Mutex<PendingJobs>,
    cache: Mutex<TerrainMeshCache>,
    wake_workers: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default)]
struct PendingJobs {
    jobs: BinaryHeap<Reverse<QueuedTerrainMesh>>,
    in_flight: BTreeSet<TerrainMeshSpec>,
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
            pending.in_flight.insert(job.spec);
            job
        };
        let generated = generate_terrain_mesh(
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

fn generate_terrain_mesh<F>(
    field: &F,
    terrain_mesh_generator: Option<TerrainMeshGenerator<F>>,
    lake_mesh_generator: Option<LakeMeshGenerator<F>>,
    priority: GenerationPriority,
    spec: TerrainMeshSpec,
) -> GeneratedTerrainMesh
where
    F: DensityField + SurfaceField,
{
    let terrain_started = Instant::now();
    let mesh = terrain_mesh_generator.map_or_else(
        || match spec {
            TerrainMeshSpec::Far(spec) => far_terrain_mesh(field, spec),
            TerrainMeshSpec::Near(spec) => {
                transvoxel_chunk(field, spec.chunk, spec.lod, spec.transition_faces)
            }
        },
        |generator| generator(field, spec),
    );
    let terrain_generation_time = terrain_started.elapsed();
    let lake_started = Instant::now();
    let lake_mesh = lake_mesh_generator.map(|generator| generator(field, spec));
    let lake_generation_time = if lake_mesh_generator.is_some() {
        lake_started.elapsed()
    } else {
        Duration::ZERO
    };

    GeneratedTerrainMesh {
        spec,
        priority,
        mesh,
        lake_mesh,
        terrain_generation_time,
        lake_generation_time,
        cache_hit: false,
    }
}

/// Generates one complete terrain result for a browser worker or an explicit
/// synchronous caller using the same native queue contract.
pub fn generate_world_terrain_mesh(
    field: &GeneratedWorldTerrain,
    priority: GenerationPriority,
    spec: TerrainMeshSpec,
) -> GeneratedTerrainMesh {
    generate_terrain_mesh(
        field,
        Some(GeneratedWorldTerrain::render_mesh),
        Some(GeneratedWorldTerrain::lake_surface_mesh),
        priority,
        spec,
    )
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
}

/// Complete deterministic inputs for one coarse, surface-only terrain tile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FarTerrainMeshSpec {
    pub tile: FarTileIndex,
}

impl FarTerrainMeshSpec {
    fn surface_grid(self) -> SurfaceGridSpec {
        let (origin_x, origin_z) = self.tile.origin();
        SurfaceGridSpec::new(
            origin_x,
            origin_z,
            [FarTileIndex::CELLS_PER_EDGE; 2],
            FarTileIndex::edge_meters() / usize_as_f64(FarTileIndex::CELLS_PER_EDGE),
        )
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
                desired.insert(tile, FarTerrainMeshSpec { tile });
            }
        }
        for &tile in loaded.keys() {
            if tile.chebyshev_distance(center) <= self.config.retain_radius {
                desired.entry(tile).or_insert(FarTerrainMeshSpec { tile });
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

    /// Returns exact mesh variants worth generating before a likely chunk
    /// crossing.
    ///
    /// A moving player prewarms up to `centers_ahead` future residency centers
    /// in the quantized travel direction. An idle player prewarms the four
    /// immediately adjacent centers so initial loading can use otherwise idle
    /// workers without guessing a preferred direction.
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

        let magnitude = travel_direction[0].abs().max(travel_direction[1].abs());
        let mut future_centers = Vec::new();
        if magnitude <= f64::EPSILON {
            for (x_offset, z_offset) in [(-1_i64, 0_i64), (1, 0), (0, -1), (0, 1)] {
                future_centers.push(ChunkIndex::new(
                    center.x.checked_add(x_offset)?,
                    center.z.checked_add(z_offset)?,
                ));
            }
        } else {
            let dominant = magnitude;
            let x_step: i64 = if travel_direction[0].abs() >= dominant * 0.5 {
                if travel_direction[0].is_sign_negative() {
                    -1
                } else {
                    1
                }
            } else {
                0
            };
            let z_step: i64 = if travel_direction[1].abs() >= dominant * 0.5 {
                if travel_direction[1].is_sign_negative() {
                    -1
                } else {
                    1
                }
            } else {
                0
            };
            let centers_ahead = i64::try_from(centers_ahead).ok()?;
            for distance in 1..=centers_ahead {
                future_centers.push(ChunkIndex::new(
                    center.x.checked_add(x_step.checked_mul(distance)?)?,
                    center.z.checked_add(z_step.checked_mul(distance)?)?,
                ));
            }
        }

        let mut specs = BTreeSet::new();
        for future_center in future_centers {
            let origin = future_center.sample_origin();
            let future_position = WorldPosition::new(
                origin.x + (ChunkIndex::edge_meters() * 0.5),
                player_position.y,
                origin.z + (ChunkIndex::edge_meters() * 0.5),
            );
            let plan = self.plan(future_position, &BTreeMap::new())?;
            specs.extend(plan.load);
        }
        Some(specs.into_iter().collect())
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use treeline_coordinates::{WorldIdentity, stable_hash};
    use treeline_geography::WatershedRegion;
    use treeline_hydrology::{GullyNetwork, RiverNetwork};
    use treeline_terrain::RollingHills;

    #[test]
    fn player_default_selects_the_versioned_surveyed_bundle() {
        assert_eq!(
            DEFAULT_WORLD_IDENTITY.settings_hash,
            treeline_terrain::DEFAULT_SURVEYED_SETTINGS_HASH
        );
        assert!(GeneratedWorldTerrain::new(DEFAULT_WORLD_IDENTITY).is_surveyed_tile());
    }

    #[test]
    fn version_twenty_aligns_channels_over_the_calibrated_terrain_default() {
        assert_eq!(LANDSCAPE_DIVERSITY_GENERATOR_VERSION, 18);
        assert_eq!(CALIBRATED_TERRAIN_GENERATOR_VERSION, 19);
        assert_eq!(CURRENT_GENERATOR_VERSION, 20);

        let world = WorldIdentity::new(0x5eed, CURRENT_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        for [x, z] in [
            [-1_420_125.0, 812_375.0],
            [-512_000.0, -0.001],
            [2_960_500.0, -4_180_250.0],
        ] {
            let surface = terrain.surface_height(x, z).expect("reset surface");
            let sample = terrain.sample(WorldPosition::new(x, surface, z));
            assert_eq!(
                sample.density.to_bits(),
                0.0_f64.to_bits(),
                "near density and far surface must remain aligned at {x}, {z}"
            );
            assert!(
                EcosystemDistribution::new(world).sample(x, z).is_some(),
                "the shared province plan must feed broad ecosystem structure"
            );
        }
    }

    #[test]
    fn version_eighteen_composes_scarp_volume_with_the_final_shaped_surface() {
        let world = WorldIdentity::new(0x5eed, PROVINCE_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let mut candidate = None;
        'outer: for z in -64..=64 {
            for x in -64..=64 {
                let world_x = f64::from(x) * 16_000.0;
                let world_z = f64::from(z) * 16_000.0;
                let province = ProvincePlan::sample_at(world, world_x, world_z).expect("province");
                if let Some(scarp) = province.scarp_geometry
                    && scarp.face_strength >= 0.12
                    && scarp.undercut_depth_meters > 0.5
                {
                    candidate = Some((world_x, world_z, scarp));
                    break 'outer;
                }
            }
        }
        let (x, z, scarp) = candidate.expect("golden survey contains a strong scarp");
        let target_signed = scarp.undercut_depth_meters * 0.46;
        let shift = target_signed - scarp.signed_distance_meters;
        let target_x = x + (scarp.face_normal[0] * shift);
        let target_z = z + (scarp.face_normal[1] * shift);
        let target = ProvincePlan::sample_at(world, target_x, target_z)
            .and_then(|sample| sample.scarp_geometry)
            .expect("same scarp at low-side cavity");
        let far_surface = terrain
            .surface_height(target_x, target_z)
            .expect("final shaped surface");
        assert_eq!(
            terrain
                .sample(WorldPosition::new(target_x, far_surface, target_z))
                .density
                .to_bits(),
            0.0_f64.to_bits()
        );
        let cavity = WorldPosition::new(
            target_x,
            far_surface - (target.undercut_depth_meters * 0.66),
            target_z,
        );
        let carved = terrain.sample(cavity);
        assert!(cavity.y - far_surface < 0.0);
        assert!(carved.density > 0.0);
        assert_eq!(carved.material, Material::Air);

        let (minimum, maximum) = terrain
            .volume_bounds(
                target_x - 16.0,
                target_z - 16.0,
                target_x + 16.0,
                target_z + 16.0,
            )
            .expect("shaped undercut bounds");
        assert!(minimum <= cavity.y && cavity.y <= maximum);
    }

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
    fn living_water_carves_generated_plunge_pools_and_downstream_gorges() {
        let world = WorldIdentity::new(0x5eed, LIVING_WATER_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let mut found = None;
        'regions: for region_z in -2..=2 {
            for region_x in -2..=2 {
                let network =
                    RiverNetwork::generate(world, WatershedRegionIndex::new(region_x, region_z))
                        .expect("river network");
                for segment in network.segments() {
                    let x = segment.source.x + ((segment.mouth.x - segment.source.x) * 0.9);
                    let z = segment.source.z + ((segment.mouth.z - segment.source.z) * 0.9);
                    let Some(erosion) = terrain.erosion_at(x, z) else {
                        continue;
                    };
                    if erosion.river.and_then(|river| river.fast_water).is_some() {
                        found = Some(erosion);
                        break 'regions;
                    }
                }
            }
        }
        let erosion = found.expect("test world should generate steep fast water");
        let base = erosion.surface.surface_height_meters();
        let gully_height = erosion.gully.map_or(base, |influence| {
            let channel_bed = base.min(influence.centerline_elevation_meters)
                - influence.segment.incision_depth_meters;
            base + ((channel_bed - base) * influence.blend)
        });
        let river = erosion.river.expect("fast-water river");
        let channel_bed = river.centerline_elevation_meters - river.incision_depth_meters;
        let ordinary_incision =
            gully_height + ((gully_height.min(channel_bed) - gully_height) * river.blend);

        assert!(erosion.final_height_meters < ordinary_incision);
    }

    #[test]
    fn version_sixteen_retains_pre_living_water_terrain_and_has_no_active_topology() {
        let world = WorldIdentity::new(0x5eed, LIVING_WATER_GENERATOR_VERSION - 1, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let spec = ActiveWaterRegionSpec::new(-64.0, -64.0, [4, 4], 32.0).expect("spec");
        let water = terrain
            .active_water_region(spec)
            .expect("old water contract");
        assert!(water.cells().is_empty());
        assert!(water.connections().is_empty());

        let mut found = None;
        'regions: for region_z in -2..=2 {
            for region_x in -2..=2 {
                let network =
                    RiverNetwork::generate(world, WatershedRegionIndex::new(region_x, region_z))
                        .expect("river network");
                for segment in network.segments() {
                    let x = segment.source.x + ((segment.mouth.x - segment.source.x) * 0.9);
                    let z = segment.source.z + ((segment.mouth.z - segment.source.z) * 0.9);
                    let Some(erosion) = terrain.erosion_at(x, z) else {
                        continue;
                    };
                    if erosion.river.and_then(|river| river.fast_water).is_some() {
                        found = Some(erosion);
                        break 'regions;
                    }
                }
            }
        }
        let erosion = found.expect("old world should expose a steep diagnostic river");
        let base = erosion.surface.surface_height_meters();
        let gully_height = erosion.gully.map_or(base, |influence| {
            let channel_bed = base.min(influence.centerline_elevation_meters)
                - influence.segment.incision_depth_meters;
            base + ((channel_bed - base) * influence.blend)
        });
        let river = erosion.river.expect("fast-water river");
        let channel_bed = river.centerline_elevation_meters - river.incision_depth_meters;
        let ordinary_incision =
            gully_height + ((gully_height.min(channel_bed) - gully_height) * river.blend);
        assert_eq!(
            erosion.final_height_meters.to_bits(),
            ordinary_incision.to_bits()
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
    fn version_fourteen_combines_filled_water_with_wetland_ecology() {
        let world = WorldIdentity::new(0x5eed, WETLAND_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let (x, z, water) = generated_lake_point(&terrain);
        let wetland = terrain.wetland_at(x, z).expect("wetland sample");

        assert!(water.water_depth_meters > 0.0);
        assert!(wetland.open_water_fraction > 0.0);
        assert!(wetland.surface_saturation_fraction > 0.0);
        assert!((0.0..=1.0).contains(&wetland.coverage_fraction));
    }

    #[test]
    fn generated_river_floodplains_produce_visible_wetland_cover() {
        let world = WorldIdentity::new(0x5eed, WETLAND_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let network =
            RiverNetwork::generate(world, WatershedRegionIndex::new(0, 0)).expect("river network");
        let mut strongest = None;
        for segment in network.segments().iter().step_by(7) {
            let center_x = (segment.source.x + segment.mouth.x) * 0.5;
            let center_z = (segment.source.z + segment.mouth.z) * 0.5;
            let Some(channel) = segment.terrain_influence(center_x, center_z) else {
                continue;
            };
            let delta_x = segment.mouth.x - segment.source.x;
            let delta_z = segment.mouth.z - segment.source.z;
            let length = libm::hypot(delta_x, delta_z);
            for side in [-1.0, 1.0] {
                let bank_distance = channel.channel_half_width_meters * 2.2 * side;
                let x = center_x - (delta_z / length * bank_distance);
                let z = center_z + (delta_x / length * bank_distance);
                let Some(wetland) = terrain.wetland_at(x, z) else {
                    continue;
                };
                if strongest.is_none_or(|current: WetlandSample| {
                    wetland.coverage_fraction > current.coverage_fraction
                }) {
                    strongest = Some(wetland);
                }
            }
        }
        let strongest = strongest.expect("wetland bank sample");

        assert!(strongest.flood_frequency_fraction > 0.0);
        assert!(
            strongest.coverage_fraction > 0.05,
            "river floodplains should create visible wetland cover"
        );
    }

    #[test]
    fn version_fifteen_grows_reef_relief_below_rendered_ocean_water() {
        const REEF_X: f64 = -600_000.0;
        const REEF_Z: f64 = -1_700_000.0;
        let world = WorldIdentity::new(0x5eed, REEF_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let reef = terrain.reef_at(REEF_X, REEF_Z).expect("reef sample");
        let erosion = terrain
            .erosion_at(REEF_X, REEF_Z)
            .expect("reef terrain contributors");
        let ocean = terrain
            .ocean_surface_at(REEF_X, REEF_Z)
            .expect("ocean above reef");

        assert!(reef.coverage_fraction > 0.5);
        assert!(reef.framework_height_meters > 0.0);
        assert_eq!(erosion.reef, Some(reef));
        assert_eq!(
            ocean.water_depth_meters.to_bits(),
            (-erosion.final_height_meters).to_bits()
        );
        assert!(erosion.final_height_meters < ocean.surface_elevation_meters);
    }

    #[test]
    fn version_sixteen_subtracts_connected_caves_without_changing_far_surface() {
        let world = WorldIdentity::new(0x5eed, CAVE_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let system = generated_cave_system(&terrain);
        let passage = system
            .graph
            .nodes
            .iter()
            .find(|node| node.kind == CaveNodeKind::Passage)
            .expect("generated passage");
        let unchanged_surface = terrain
            .surface_height(passage.position.x, passage.position.z)
            .expect("far surface");
        let base_density = passage.position.y - unchanged_surface;
        let carved = terrain.sample(passage.position);

        assert!(base_density < 0.0);
        assert!(carved.density > 0.0);
        assert_eq!(carved.material, Material::Air);
        assert_eq!(
            terrain
                .surface_height(passage.position.x, passage.position.z)
                .expect("same far surface")
                .to_bits(),
            unchanged_surface.to_bits()
        );
        assert!(system.graph.is_connected());
        assert!(system.graph.has_valid_edges());
        let entrance = system.entrances().next().expect("surface connection");
        let entrance_surface = terrain
            .surface_height(entrance.position.x, entrance.position.z)
            .expect("entrance surface");
        assert!(
            terrain
                .sample(WorldPosition::new(
                    entrance.position.x,
                    entrance_surface,
                    entrance.position.z,
                ))
                .density
                > 0.0,
            "the generated entrance must open through the composed surface"
        );
    }

    #[test]
    fn cave_bounding_boxes_do_not_clamp_distant_terrain_density() {
        let world = WorldIdentity::new(0x5eed, CAVE_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let system = generated_cave_system(&terrain);
        // A column inside the system's bounding box but well clear of every
        // passage must report its true depth below the composed surface, not
        // the subtraction field's saturated floor.
        let steps = 16;
        let (x, z) = (0..=steps)
            .flat_map(|row| (0..=steps).map(move |column| (row, column)))
            .map(|(row, column)| {
                let fraction = |index: i32, min: f64, max: f64| {
                    min + ((max - min) * f64::from(index) / f64::from(steps))
                };
                (
                    fraction(column, system.bounds.min.x, system.bounds.max.x),
                    fraction(row, system.bounds.min.z, system.bounds.max.z),
                )
            })
            .max_by(|&(left_x, left_z), &(right_x, right_z)| {
                system
                    .horizontal_distance_at(left_x, left_z)
                    .total_cmp(&system.horizontal_distance_at(right_x, right_z))
            })
            .expect("the bounding box has sample positions");
        let surface = terrain.surface_height(x, z).expect("composed surface");
        // Stay inside the system's vertical bounds so the sample still reaches
        // the cave-composition path this test is guarding.
        let probe_y = (surface - 40.0).clamp(system.bounds.min.y, system.bounds.max.y);
        let probe = WorldPosition::new(x, probe_y, z);

        assert!(
            system.horizontal_distance_at(x, z) > CaveInfluence::REACH_METERS,
            "the probe must sit outside every passage footprint"
        );
        assert!(
            surface - probe_y > CaveInfluence::REACH_METERS,
            "the probe must sit deeper than the saturated cave reach"
        );
        let sample = terrain.sample(probe);
        assert!(
            sample.density < -CaveInfluence::REACH_METERS,
            "density {} was clamped to the cave reach floor",
            sample.density
        );
        assert!((sample.density - (probe.y - surface)).abs() < 1.0e-9);
    }

    #[test]
    fn cave_generation_has_a_stable_golden_fingerprint_and_cache_order() {
        let world = WorldIdentity::new(0x5eed, CAVE_GENERATOR_VERSION, 0);
        let forward = GeneratedWorldTerrain::new(world);
        let system = generated_cave_system(&forward);
        let fingerprint = system.fingerprint();

        let reverse = GeneratedWorldTerrain::new(world);
        let _ = reverse.cave_system(CaveRegionIndex::new(7, -9));
        let repeated = reverse
            .cave_system(system.region)
            .expect("same generated cave");

        assert_eq!(system.as_ref(), repeated.as_ref());
        assert_eq!(fingerprint, repeated.fingerprint());
        assert_eq!(fingerprint, 6_610_453_046_115_402_670);
    }

    #[test]
    fn cave_bounds_extend_near_voxel_meshing_and_underground_rivers_render() {
        let terrain =
            GeneratedWorldTerrain::new(WorldIdentity::new(0x5eed, CAVE_GENERATOR_VERSION, 0));
        let system = generated_cave_system(&terrain);
        let river = system
            .graph
            .underground_rivers
            .first()
            .expect("generated system has underground water");
        let edge = system.graph.edges[river.edge_index];
        let start = system.graph.nodes[edge.from];
        let end = system.graph.nodes[edge.to];
        let midpoint = WorldPosition::new(
            (start.position.x + end.position.x) * 0.5,
            (start.position.y + end.position.y) * 0.5,
            (start.position.z + end.position.z) * 0.5,
        );
        let chunk = ChunkIndex::containing(midpoint).expect("river chunk");
        let origin = chunk.sample_origin();
        let bounds = terrain
            .volume_bounds(
                origin.x,
                origin.z,
                origin.x + ChunkIndex::edge_meters(),
                origin.z + ChunkIndex::edge_meters(),
            )
            .expect("cave bounds");
        let surface = terrain
            .surface_height(midpoint.x, midpoint.z)
            .expect("surface");
        assert!(bounds.0 < surface - 5.0);

        let terrain_mesh = terrain
            .render_mesh(TerrainMeshSpec::Near(ChunkMeshSpec {
                chunk,
                lod: ChunkIndex::NEAR_LOD,
                transition_faces: TransitionFaces::none(),
            }))
            .expect("volumetric terrain mesh");
        assert!(terrain_mesh.is_well_formed());
        assert!(
            terrain_mesh
                .positions
                .iter()
                .any(|position| position[1] < surface - 5.0)
        );

        let water = terrain
            .lake_surface_mesh(TerrainMeshSpec::Near(ChunkMeshSpec {
                chunk,
                lod: ChunkIndex::NEAR_LOD,
                transition_faces: TransitionFaces::none(),
            }))
            .expect("water mesh");
        assert!(water.is_well_formed());
        assert!(water.positions.iter().any(|position| {
            position[1] < surface - 5.0
                && (position[0] - midpoint.x).abs() < 100.0
                && (position[2] - midpoint.z).abs() < 100.0
        }));
    }

    #[test]
    fn pre_cave_worlds_do_not_generate_or_cache_caves() {
        let terrain =
            GeneratedWorldTerrain::new(WorldIdentity::new(0x5eed, CAVE_GENERATOR_VERSION - 1, 0));
        assert!(
            terrain
                .cave_influence_at(WorldPosition::new(0.0, 0.0, 0.0))
                .is_none()
        );
        assert!(terrain.cave_systems.read().expect("cache lock").is_empty());
    }

    #[test]
    fn underground_river_ribbons_clip_to_chunk_ownership() {
        let start = CaveNode {
            position: WorldPosition::new(-8.0, 4.0, 16.0),
            kind: CaveNodeKind::Passage,
            radius_meters: 2.0,
        };
        let end = CaveNode {
            position: WorldPosition::new(40.0, -2.0, 16.0),
            kind: CaveNodeKind::Passage,
            radius_meters: 4.0,
        };
        let (clipped_start, clipped_end) =
            clip_cave_edge_to_chunk(start, end, 0.0, 0.0, 32.0, 32.0).expect("clipped edge");

        assert!(clipped_start.position.x.abs() < f64::EPSILON);
        assert!((clipped_end.position.x - 32.0).abs() < f64::EPSILON);
        assert!(clipped_start.position.y < start.position.y);
        assert!(clipped_end.position.y > end.position.y);
        assert!(clipped_start.radius_meters > start.radius_meters);
        assert!(clipped_end.radius_meters < end.radius_meters);
    }

    #[test]
    fn pre_ecosystem_versions_do_not_expose_wetlands_reefs_or_ocean() {
        let terrain = GeneratedWorldTerrain::new(WorldIdentity::new(
            0x5eed,
            WETLAND_GENERATOR_VERSION - 1,
            0,
        ));

        assert!(terrain.wetland_at(0.0, 0.0).is_none());
        assert!(terrain.reef_at(0.0, 0.0).is_none());
        assert!(terrain.ocean_surface_at(0.0, 0.0).is_none());
    }

    #[test]
    fn ecosystem_sampling_is_independent_of_regional_cache_order() {
        let world = WorldIdentity::new(0x5eed, CURRENT_GENERATOR_VERSION, 0);
        let positions = [
            [-600_000.0, -1_700_000.0],
            [-129_000.0, -1_000.0],
            [127_000.0, 127_000.0],
        ];
        let forward_terrain = GeneratedWorldTerrain::new(world);
        let forward = positions.map(|[x, z]| {
            (
                forward_terrain.wetland_at(x, z).expect("wetland"),
                forward_terrain.reef_at(x, z).expect("reef"),
                forward_terrain
                    .surface_height(x, z)
                    .expect("ecosystem-shaped surface")
                    .to_bits(),
            )
        });
        let reverse_terrain = GeneratedWorldTerrain::new(world);
        let mut reverse_positions = positions;
        reverse_positions.reverse();
        let mut reverse = reverse_positions.map(|[x, z]| {
            (
                reverse_terrain.wetland_at(x, z).expect("wetland"),
                reverse_terrain.reef_at(x, z).expect("reef"),
                reverse_terrain
                    .surface_height(x, z)
                    .expect("ecosystem-shaped surface")
                    .to_bits(),
            )
        });
        reverse.reverse();

        assert_eq!(forward, reverse);
    }

    #[test]
    fn snow_coverage_is_seasonal_and_independent_of_sampling_order() {
        const SURVEY_CENTER: [f64; 2] = [-41_088_000.0, 13_248_000.0];
        let world = WorldIdentity::new(0x0aa7_6435_6961_e927, CURRENT_GENERATOR_VERSION, 0);
        let forward = GeneratedWorldTerrain::new(world);
        let reverse = GeneratedWorldTerrain::new(world);
        let mut positions = (-8_i32..=8).flat_map(|z| {
            (-8_i32..=8).map(move |x| {
                [
                    SURVEY_CENTER[0] + (f64::from(x) * 16_000.0),
                    SURVEY_CENTER[1] + (f64::from(z) * 16_000.0),
                ]
            })
        });
        let [x, z] = positions
            .find(|&[x, z]| {
                let winter = forward
                    .snow_coverage_at(x, z, Season::Winter)
                    .expect("winter snow coverage");
                let summer = forward
                    .snow_coverage_at(x, z, Season::Summer)
                    .expect("summer snow coverage");
                winter.coverage_fraction > summer.coverage_fraction + 0.05
            })
            .expect("a generated world should contain seasonal snow cover");
        let winter = forward
            .snow_coverage_at(x, z, Season::Winter)
            .expect("winter snow coverage");
        let summer = reverse
            .snow_coverage_at(x, z, Season::Summer)
            .expect("summer snow coverage");
        let repeated_winter = reverse
            .snow_coverage_at(x, z, Season::Winter)
            .expect("repeated winter snow coverage");
        let render_sample = GeneratedWorldTerrain::new(world);
        let reused_slope = render_sample
            .snow_coverage_for_slope(x, z, Season::Winter, winter.terrain_slope)
            .expect("snow coverage from existing mesh slope");

        assert!(winter.snowpack_water_equivalent_millimeters > 0.0);
        assert!(winter.coverage_fraction > summer.coverage_fraction);
        assert_eq!(
            winter.coverage_fraction.to_bits(),
            repeated_winter.coverage_fraction.to_bits()
        );
        assert_eq!(
            winter.terrain_slope.to_bits(),
            repeated_winter.terrain_slope.to_bits()
        );
        assert_eq!(
            winter.coverage_fraction.to_bits(),
            reused_slope.coverage_fraction.to_bits()
        );
        assert!(
            render_sample
                .river_networks
                .read()
                .expect("cache lock")
                .is_empty()
        );
        assert!(
            render_sample
                .gully_networks
                .read()
                .expect("cache lock")
                .is_empty()
        );
        assert!(
            render_sample
                .lake_networks
                .read()
                .expect("cache lock")
                .is_empty()
        );
    }

    #[test]
    fn snow_coverage_is_absent_before_the_seasonal_climate_contract() {
        let terrain = GeneratedWorldTerrain::new(WorldIdentity::new(0x5eed, 6, 0));
        let snow = terrain
            .snow_coverage_at(0.0, 0.0, Season::Winter)
            .expect("snow coverage sample");

        assert_eq!(
            snow.snowpack_water_equivalent_millimeters.to_bits(),
            0.0_f64.to_bits()
        );
        assert_eq!(snow.coverage_fraction.to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn render_snow_depth_grid_avoids_composed_terrain_queries() {
        let terrain =
            GeneratedWorldTerrain::new(WorldIdentity::new(0x5eed, CURRENT_GENERATOR_VERSION, 0));
        for z in 0..3 {
            for x in 0..3 {
                let snow = terrain
                    .snow_coverage_for_slope(
                        f64::from(x) * 1_024.0,
                        f64::from(z) * 1_024.0,
                        Season::Winter,
                        0.0,
                    )
                    .expect("render snow depth");
                assert!(snow.coverage_fraction.is_finite());
            }
        }

        assert!(
            terrain
                .river_networks
                .read()
                .expect("cache lock")
                .is_empty()
        );
        assert!(
            terrain
                .gully_networks
                .read()
                .expect("cache lock")
                .is_empty()
        );
        assert!(terrain.lake_networks.read().expect("cache lock").is_empty());
    }

    #[test]
    fn geography_surface_materials_are_world_aligned_and_regionally_distinct() {
        let world = WorldIdentity::new(0x5eed, CURRENT_GENERATOR_VERSION, 0);
        let terrain = GeneratedWorldTerrain::new(world);
        let sample_color = |x, z| {
            geography_surface_color(&SurfaceColorInputs {
                profile: RegionalProfile::sample(world, x, z),
                soil: Soil::new(world).sample(x, z),
                forest: ForestDistribution::new(world).sample(x, z),
                ground: GroundVegetationDistribution::new(world).sample(x, z),
                erosion: terrain.erosion_at(x, z),
                wetland: terrain.wetland_at(x, z),
                reef: terrain.reef_at(x, z),
                ecosystem: EcosystemDistribution::new(world).sample(x, z),
            })
            .expect("geography material")
        };
        let first = sample_color(-80_062.0, -79_950.0);
        let repeated = sample_color(-80_062.0, -79_950.0);
        let distant = sample_color(175_000.0, -212_000.0);

        assert_eq!(first.map(f32::to_bits), repeated.map(f32::to_bits));
        assert_ne!(
            [first[0], first[1], first[2]].map(f32::to_bits),
            [distant[0], distant[1], distant[2]].map(f32::to_bits)
        );
        assert!((0.0..=1.0).contains(&first[3]));
        assert!(first[3] >= 0.8);
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
            (position[1] - water.lake.surface_elevation_meters - 0.05).abs() < 0.001
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
        assert!(GenerationPriority::NearTerrain < GenerationPriority::PrefetchTerrain);
        assert!(GenerationPriority::PrefetchTerrain < GenerationPriority::Vegetation);
        for priority in [
            GenerationPriority::PlayerTerrain,
            GenerationPriority::Horizon,
            GenerationPriority::FarTerrain,
            GenerationPriority::NearTerrain,
            GenerationPriority::PrefetchTerrain,
            GenerationPriority::Vegetation,
            GenerationPriority::SurfaceDetail,
        ] {
            assert_eq!(
                GenerationPriority::from_code(priority.code()),
                Some(priority)
            );
        }
        assert_eq!(GenerationPriority::from_code(u8::MAX), None);
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
    fn queued_generation_can_include_worker_built_lake_meshes() {
        let terrain = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let spec = ChunkMeshSpec {
            chunk: ChunkIndex::new(0, 0),
            lod: ChunkIndex::MAX_LOD,
            transition_faces: TransitionFaces::none(),
        };
        let mut queue = TerrainMeshQueue::with_worker_count_and_mesh_generators(
            terrain,
            NonZeroUsize::MIN,
            None,
            Some(empty_lake_mesh),
        );
        queue.enqueue(GenerationPriority::NearTerrain, TerrainMeshSpec::Near(spec));
        let generated = queue
            .ready
            .recv_timeout(Duration::from_secs(2))
            .expect("worker completes terrain and lake meshes");

        assert_eq!(
            generated
                .lake_mesh
                .expect("lake generation configured")
                .expect("valid lake mesh"),
            Mesh::default()
        );
    }

    #[test]
    fn queued_far_mesh_matches_direct_surface_generation() {
        let terrain = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let spec = FarTerrainMeshSpec {
            tile: FarTileIndex::new(-1, 2),
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
    fn completed_meshes_are_reused_without_regeneration() {
        let terrain = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let spec = TerrainMeshSpec::Near(ChunkMeshSpec {
            chunk: ChunkIndex::new(2, -1),
            lod: ChunkIndex::MAX_LOD,
            transition_faces: TransitionFaces::none(),
        });
        let mut queue = TerrainMeshQueue::with_worker_count(terrain, NonZeroUsize::MIN);
        queue.enqueue(GenerationPriority::NearTerrain, spec);
        let first = queue
            .ready
            .recv_timeout(Duration::from_secs(2))
            .expect("worker completes mesh");
        assert!(!first.cache_hit);

        queue.enqueue(GenerationPriority::PlayerTerrain, spec);
        let cached = queue.try_next().expect("cached completion");

        assert_eq!(cached.spec, spec);
        assert_eq!(cached.priority, GenerationPriority::PlayerTerrain);
        assert!(cached.cache_hit);
        assert_eq!(cached.terrain_generation_time, Duration::ZERO);
        assert_eq!(cached.lake_generation_time, Duration::ZERO);
        assert_eq!(cached.mesh, first.mesh);
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
            .plan(WorldPosition::new(-0.01, 0.0, -0.01), &BTreeMap::new())
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
    fn moving_between_near_chunks_does_not_rebuild_far_tiles() {
        let streamer = FarTerrainStreamer::new(FarTerrainStreamingConfig::new(1, 1).unwrap());
        let initial = streamer
            .plan(WorldPosition::new(0.0, 0.0, 0.0), &BTreeMap::new())
            .expect("finite position");
        let loaded = initial
            .load
            .into_iter()
            .map(|spec| (spec.tile, spec))
            .collect::<BTreeMap<_, _>>();
        let moved = streamer
            .plan(
                WorldPosition::new(ChunkIndex::edge_meters(), 0.0, 0.0),
                &loaded,
            )
            .expect("finite position");

        assert!(moved.load.is_empty());
        assert!(moved.unload.is_empty());
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
    fn directional_prefetch_matches_the_next_residency_center() {
        let streamer = ChunkStreamer::new(ChunkStreamingConfig::default());
        let current = WorldPosition::new(-0.01, 0.0, -0.01);
        let prefetched = streamer
            .prefetch_specs(current, [1.0, 0.0], 1)
            .expect("finite prediction")
            .into_iter()
            .collect::<BTreeSet<_>>();
        let next_center = ChunkIndex::new(0, -1);
        let next_origin = next_center.sample_origin();
        let expected = streamer
            .plan(
                WorldPosition::new(
                    next_origin.x + (ChunkIndex::edge_meters() * 0.5),
                    0.0,
                    next_origin.z + (ChunkIndex::edge_meters() * 0.5),
                ),
                &BTreeMap::new(),
            )
            .expect("finite next center")
            .load
            .into_iter()
            .collect::<BTreeSet<_>>();

        assert_eq!(prefetched, expected);
    }

    #[test]
    fn idle_prefetch_covers_each_adjacent_center_and_can_be_disabled() {
        let streamer = ChunkStreamer::new(ChunkStreamingConfig::new(1, 2).expect("valid radii"));
        let position = WorldPosition::new(0.0, 0.0, 0.0);
        let prefetched = streamer
            .prefetch_specs(position, [0.0, 0.0], 1)
            .expect("finite prediction");

        for center in [
            ChunkIndex::new(-1, 0),
            ChunkIndex::new(1, 0),
            ChunkIndex::new(0, -1),
            ChunkIndex::new(0, 1),
        ] {
            assert!(
                prefetched.iter().any(|spec| spec.chunk == center),
                "adjacent center {center:?} should be prewarmed"
            );
        }
        assert_eq!(
            streamer.prefetch_specs(position, [1.0, 0.0], 0),
            Some(Vec::new())
        );
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

    #[test]
    fn living_water_reconstructs_surface_river_lake_and_cave_topology() {
        let terrain = GeneratedWorldTerrain::new(WorldIdentity::new(
            0x5eed,
            LIVING_WATER_GENERATOR_VERSION,
            0,
        ));
        let cave = generated_cave_system(&terrain);
        let entrance = cave.entrances().next().expect("surface connection");
        let spacing = 32.0;
        let cells = 16;
        let half_span = usize_as_f64(cells) * spacing * 0.5;
        let spec = ActiveWaterRegionSpec::new(
            entrance.position.x - half_span,
            entrance.position.z - half_span,
            [cells; 2],
            spacing,
        )
        .expect("spec");
        let mut water = terrain.active_water_region(spec).expect("active water");

        assert!(
            water
                .cells()
                .iter()
                .any(|cell| cell.kind == WaterCellKind::Surface)
        );
        assert!(
            water
                .cells()
                .iter()
                .any(|cell| cell.kind == WaterCellKind::CaveStream)
        );
        assert!(
            water
                .cells()
                .iter()
                .any(|cell| cell.kind == WaterCellKind::Sump)
        );
        let cave_ids = water
            .cells()
            .iter()
            .filter(|cell| matches!(cell.kind, WaterCellKind::CaveStream | WaterCellKind::Sump))
            .map(|cell| cell.id)
            .collect::<BTreeSet<_>>();
        assert!(water.connections().iter().any(|connection| {
            !cave_ids.contains(&connection.from)
                && connection
                    .to
                    .is_some_and(|target| cave_ids.contains(&target))
        }));
        let initial = water.total_volume_cubic_meters();
        let report = water.step(1.0).expect("living-water step");
        let expected = initial + report.source_volume_cubic_meters
            - report.boundary_outflow_volume_cubic_meters
            - report.infiltrated_volume_cubic_meters;
        assert!((water.total_volume_cubic_meters() - expected).abs() < 1.0e-6);
    }

    #[test]
    fn living_water_adds_visible_surface_river_ribbons() {
        let world = WorldIdentity::new(0x5eed, LIVING_WATER_GENERATOR_VERSION, 0);
        let network =
            RiverNetwork::generate(world, WatershedRegionIndex::new(0, 0)).expect("river network");
        let segment = network.segments().first().expect("river segment");
        let midpoint = WorldPosition::new(
            (segment.source.x + segment.mouth.x) * 0.5,
            0.0,
            (segment.source.z + segment.mouth.z) * 0.5,
        );
        let chunk = ChunkIndex::containing(midpoint).expect("river chunk");
        let terrain = GeneratedWorldTerrain::new(world);
        let water = terrain
            .lake_surface_mesh(TerrainMeshSpec::Near(ChunkMeshSpec {
                chunk,
                lod: ChunkIndex::NEAR_LOD,
                transition_faces: TransitionFaces::none(),
            }))
            .expect("water mesh");

        assert!(water.is_well_formed());
        assert!(!water.indices.is_empty());
        assert!(water.positions.iter().any(|position| {
            let Some(influence) = terrain.river_influence_at(position[0], position[2]) else {
                return false;
            };
            influence.distance_meters <= influence.channel_half_width_meters + 2.0
        }));
    }

    #[test]
    fn surveyed_tile_uses_aerial_color_and_mapped_lake_water() {
        const UPPER_HOLMES_LAKE: [f64; 2] = [7_364.0, 6_894.0];
        let world = WorldIdentity::new(
            0x5eed,
            CURRENT_GENERATOR_VERSION,
            treeline_terrain::DEFAULT_SURVEYED_SETTINGS_HASH,
        );
        let terrain = GeneratedWorldTerrain::new(world);
        let color = terrain
            .surface_color_at(UPPER_HOLMES_LAKE[0], UPPER_HOLMES_LAKE[1])
            .expect("surveyed color");
        assert!(
            color
                .into_iter()
                .all(|channel| (0.0..=1.0).contains(&channel))
        );
        let water = terrain
            .lake_surface_at(UPPER_HOLMES_LAKE[0], UPPER_HOLMES_LAKE[1])
            .expect("mapped lake");
        assert_eq!(water.lake.id, 19);
        assert!((water.lake.surface_elevation_meters - 415.5).abs() < f64::EPSILON);

        let chunk = ChunkIndex::containing(WorldPosition::new(
            UPPER_HOLMES_LAKE[0],
            0.0,
            UPPER_HOLMES_LAKE[1],
        ))
        .expect("lake chunk");
        let mesh = terrain
            .lake_surface_mesh(TerrainMeshSpec::Near(ChunkMeshSpec {
                chunk,
                lod: ChunkIndex::NEAR_LOD,
                transition_faces: TransitionFaces::none(),
            }))
            .expect("mapped lake mesh");
        assert!(mesh.is_well_formed());
        assert!(!mesh.indices.is_empty());
    }

    fn specs_by_chunk(specs: Vec<ChunkMeshSpec>) -> BTreeMap<ChunkIndex, ChunkMeshSpec> {
        specs.into_iter().map(|spec| (spec.chunk, spec)).collect()
    }

    #[allow(clippy::unnecessary_wraps)]
    fn empty_lake_mesh(
        _terrain: &RollingHills,
        _spec: TerrainMeshSpec,
    ) -> Result<Mesh, MeshingError> {
        Ok(Mesh::default())
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

    fn generated_cave_system(terrain: &GeneratedWorldTerrain) -> Arc<CaveSystem> {
        for radius in 0_i64..24 {
            for z in -radius..=radius {
                for x in -radius..=radius {
                    if let Some(system) = terrain.cave_system(CaveRegionIndex::new(x, z))
                        && !system.graph.underground_rivers.is_empty()
                    {
                        return system;
                    }
                }
            }
        }
        panic!("test world should contain a wet cave system");
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
