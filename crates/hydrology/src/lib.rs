//! Deterministic river networks and water-state primitives.

use std::collections::{BTreeMap, BTreeSet};

use treeline_coordinates::{CellIndex, WorldIdentity, WorldPosition};
use treeline_geography::{
    Climate, DRAINAGE_CELL_EDGE_METERS, DrainageCellIndex, MacroElevation,
    OROGRAPHIC_CLIMATE_GENERATOR_VERSION, RegionalProfile, SEASONAL_CLIMATE_GENERATOR_VERSION,
    WatershedRegion, WatershedRegionIndex,
};

const SECONDS_PER_YEAR: f64 = 31_556_952.0;
const MIN_CHANNEL_CATCHMENT_CELLS: u64 = 8;
const DOMAIN_GULLY_BEND: u64 = 0x4755_4c4c_5942_454e;
pub const MAX_RIVER_INFLUENCE_METERS: f64 = 900.0;
pub const MAX_GULLY_INFLUENCE_METERS: f64 = 120.0;

/// A directed segment in a generated river graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiverSegment {
    pub source_cell: DrainageCellIndex,
    pub mouth_cell: DrainageCellIndex,
    pub source: WorldPosition,
    pub mouth: WorldPosition,
    pub drainage_area_square_kilometers: f64,
    pub discharge_cubic_meters_per_second: f64,
}

impl RiverSegment {
    /// Rivers may be level locally, but cannot spontaneously flow uphill.
    pub fn descends_or_is_level(self) -> bool {
        self.source.y >= self.mouth.y
    }

    /// Evaluates the valley carved by this segment at a horizontal position.
    ///
    /// Channel and valley scale grow continuously with catchment and discharge
    /// rather than selecting a river preset. Fused operations, square roots,
    /// and distances use `libm` and are part of the generation contract.
    pub fn terrain_influence(self, x: f64, z: f64) -> Option<RiverTerrainInfluence> {
        if !x.is_finite() || !z.is_finite() {
            return None;
        }
        let segment_x = self.mouth.x - self.source.x;
        let segment_z = self.mouth.z - self.source.z;
        let length_squared = libm::fma(segment_x, segment_x, segment_z * segment_z);
        if length_squared <= 0.0 {
            return None;
        }
        let offset_x = x - self.source.x;
        let offset_z = z - self.source.z;
        let along =
            (libm::fma(offset_x, segment_x, offset_z * segment_z) / length_squared).clamp(0.0, 1.0);
        let nearest_x = self.source.x + (segment_x * along);
        let nearest_z = self.source.z + (segment_z * along);
        let distance_meters = libm::hypot(x - nearest_x, z - nearest_z);
        let catchment_scale = libm::sqrt(self.drainage_area_square_kilometers);
        let discharge_scale = libm::sqrt(self.discharge_cubic_meters_per_second);
        let valley_half_width_meters =
            (96.0 + (catchment_scale * 18.0)).clamp(160.0, MAX_RIVER_INFLUENCE_METERS);
        if distance_meters > valley_half_width_meters {
            return None;
        }
        let channel_half_width_meters = (2.0 + (discharge_scale * 2.5)).clamp(2.0, 30.0);
        let incision_depth_meters =
            (16.0 + (catchment_scale * 0.12) + (discharge_scale * 0.5)).clamp(16.0, 48.0);
        let centerline_elevation_meters = self.source.y + ((self.mouth.y - self.source.y) * along);
        let normalized = 1.0 - (distance_meters / valley_half_width_meters);
        let blend = normalized * normalized * (3.0 - (2.0 * normalized));
        let fast_water = self.fast_water_at(x, z, along, blend);

        Some(RiverTerrainInfluence {
            segment: self,
            distance_meters,
            along_fraction: along,
            centerline_elevation_meters,
            channel_half_width_meters,
            valley_half_width_meters,
            incision_depth_meters,
            blend,
            fast_water,
        })
    }

    fn fast_water_at(
        self,
        x: f64,
        z: f64,
        along_fraction: f64,
        channel_blend: f64,
    ) -> Option<GeneratedFastWater> {
        let length = libm::hypot(self.mouth.x - self.source.x, self.mouth.z - self.source.z);
        if length <= 0.0 {
            return None;
        }
        let drop_meters = (self.source.y - self.mouth.y).max(0.0);
        let gradient = drop_meters / length;
        let kind = if drop_meters >= 18.0
            && gradient >= 0.012
            && self.discharge_cubic_meters_per_second >= 0.08
        {
            FastWaterKind::Waterfall
        } else if drop_meters >= 6.0
            && gradient >= 0.004
            && self.discharge_cubic_meters_per_second >= 0.03
        {
            FastWaterKind::Cascade
        } else {
            return None;
        };
        let energy = self.discharge_cubic_meters_per_second * drop_meters;
        let plunge_pool_depth_meters = (0.45 + (libm::sqrt(energy) * 0.24)).clamp(0.45, 18.0);
        let plunge_pool_radius_meters = (4.0
            + (libm::sqrt(self.discharge_cubic_meters_per_second) * libm::sqrt(drop_meters) * 2.2))
            .clamp(4.0, 56.0);
        let pool_distance = libm::hypot(x - self.mouth.x, z - self.mouth.z);
        let pool_normalized = (1.0 - (pool_distance / plunge_pool_radius_meters)).clamp(0.0, 1.0);
        let plunge_pool_blend = pool_normalized * pool_normalized * (3.0 - (2.0 * pool_normalized));
        let downstream_gorge_depth_meters =
            (drop_meters * 0.24 + libm::sqrt(energy) * 0.3).clamp(0.5, 28.0);
        let downstream = ((along_fraction - 0.58) / 0.42).clamp(0.0, 1.0);
        let gorge_blend = downstream * channel_blend;
        Some(GeneratedFastWater {
            kind,
            drop_meters,
            gradient,
            plunge_pool_depth_meters,
            plunge_pool_radius_meters,
            plunge_pool_blend,
            downstream_gorge_depth_meters,
            gorge_blend,
        })
    }
}

/// Condition-driven cascade or waterfall morphology on a generated river.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeneratedFastWater {
    pub kind: FastWaterKind,
    pub drop_meters: f64,
    pub gradient: f64,
    pub plunge_pool_depth_meters: f64,
    pub plunge_pool_radius_meters: f64,
    pub plunge_pool_blend: f64,
    pub downstream_gorge_depth_meters: f64,
    pub gorge_blend: f64,
}

/// Explainable terrain-shaping values contributed by one river segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiverTerrainInfluence {
    pub segment: RiverSegment,
    pub distance_meters: f64,
    pub along_fraction: f64,
    pub centerline_elevation_meters: f64,
    pub channel_half_width_meters: f64,
    pub valley_half_width_meters: f64,
    pub incision_depth_meters: f64,
    pub blend: f64,
    pub fast_water: Option<GeneratedFastWater>,
}

/// One minor drainage path below the river-network catchment threshold.
///
/// Endpoints come from the same filled drainage graph as rivers. A stable,
/// bounded midpoint offset removes the coarse grid's straight-line fingerprint
/// without changing connectivity or downstream elevation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GullySegment {
    pub source_cell: DrainageCellIndex,
    pub mouth_cell: DrainageCellIndex,
    pub source: WorldPosition,
    pub bend: WorldPosition,
    pub mouth: WorldPosition,
    pub flow_accumulation_cells: u64,
    pub half_width_meters: f64,
    pub incision_depth_meters: f64,
}

impl GullySegment {
    pub fn descends_or_is_level(self) -> bool {
        self.source.y >= self.bend.y && self.bend.y >= self.mouth.y
    }

    pub fn terrain_influence(self, x: f64, z: f64) -> Option<GullyTerrainInfluence> {
        if !x.is_finite() || !z.is_finite() {
            return None;
        }
        let first = closest_on_segment(x, z, self.source, self.bend, 0.0, 0.5)?;
        let second = closest_on_segment(x, z, self.bend, self.mouth, 0.5, 1.0)?;
        let (distance_meters, along) = if first.0 < second.0
            || (first.0.to_bits() == second.0.to_bits() && first.1 <= second.1)
        {
            first
        } else {
            second
        };
        if distance_meters > self.half_width_meters {
            return None;
        }
        let centerline_elevation_meters = self.source.y + ((self.mouth.y - self.source.y) * along);
        let normalized = 1.0 - (distance_meters / self.half_width_meters);
        let blend = normalized * normalized * (3.0 - (2.0 * normalized));
        Some(GullyTerrainInfluence {
            segment: self,
            distance_meters,
            centerline_elevation_meters,
            blend,
        })
    }
}

/// Explainable meso-scale incision from a minor drainage path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GullyTerrainInfluence {
    pub segment: GullySegment,
    pub distance_meters: f64,
    pub centerline_elevation_meters: f64,
    pub blend: f64,
}

/// Deterministic meso-scale drainage erosion for one watershed artifact.
#[derive(Clone, Debug, PartialEq)]
pub struct GullyNetwork {
    pub world: WorldIdentity,
    pub region: WatershedRegionIndex,
    segments: Vec<GullySegment>,
}

impl GullyNetwork {
    pub fn generate(world: WorldIdentity, region: WatershedRegionIndex) -> Option<Self> {
        let watershed = WatershedRegion::generate(world, region)?;
        Self::from_watershed(&watershed)
    }

    pub fn from_watershed(watershed: &WatershedRegion) -> Option<Self> {
        let cells = watershed.cells();
        let slots = cells
            .iter()
            .enumerate()
            .map(|(slot, cell)| (cell.index, slot))
            .collect::<BTreeMap<_, _>>();
        let terrain = MacroElevation::new(watershed.world);
        let mut segments = Vec::new();
        for cell in cells {
            if cell.flow_accumulation_cells >= MIN_CHANNEL_CATCHMENT_CELLS || cell.basin.is_some() {
                continue;
            }
            let Some(mouth_cell) = cell.flow_to else {
                continue;
            };
            let [source_x, source_z] = cell.index.center();
            let [mouth_x, mouth_z] = mouth_cell.center();
            let source_y = cell.filled_elevation_meters;
            let mouth_y = if let Some(&mouth_slot) = slots.get(&mouth_cell) {
                cells[mouth_slot].filled_elevation_meters
            } else {
                terrain.sample(mouth_x, mouth_z)?.elevation_meters
            };
            if mouth_y > source_y {
                return None;
            }

            let delta_x = mouth_x - source_x;
            let delta_z = mouth_z - source_z;
            let length = libm::hypot(delta_x, delta_z);
            if length <= 0.0 {
                return None;
            }
            let key = CellIndex::new(cell.index.x, cell.index.z, 0)
                .generation_key(watershed.world, DOMAIN_GULLY_BEND);
            let signed_bend =
                ((hash53_as_f64(key >> 11) / 9_007_199_254_740_991.0) - 0.5) * length * 0.28;
            let perpendicular_x = -delta_z / length;
            let perpendicular_z = delta_x / length;
            let bend_x = ((source_x + mouth_x) * 0.5) + (perpendicular_x * signed_bend);
            let bend_z = ((source_z + mouth_z) * 0.5) + (perpendicular_z * signed_bend);
            let bend_y = (source_y + mouth_y) * 0.5;

            let profile = RegionalProfile::sample(watershed.world, source_x, source_z)?;
            let precipitation =
                if watershed.world.generator_version >= OROGRAPHIC_CLIMATE_GENERATOR_VERSION {
                    Climate::new(watershed.world)
                        .sample(source_x, source_z)?
                        .precipitation_fraction()
                } else {
                    profile.precipitation
                };
            let softness = 1.0 - profile.rock_hardness;
            let erodibility = (0.2 + (softness * 0.8))
                * (0.25 + (profile.erosion_age * 0.75))
                * (0.3 + (precipitation * 0.7));
            let catchment_scale = libm::sqrt(u64_as_f64(cell.flow_accumulation_cells));
            let gradient = ((source_y - mouth_y) / length).clamp(0.0, 1.0);
            let half_width_meters = ((28.0 + (catchment_scale * 12.0))
                * (0.7 + (precipitation * 0.6)))
                .clamp(24.0, MAX_GULLY_INFLUENCE_METERS);
            let incision_depth_meters = ((1.0 + (catchment_scale * 2.4))
                * (0.35 + (erodibility * 0.65))
                * (0.55 + ((gradient / 0.08).clamp(0.0, 1.0) * 0.75)))
                .clamp(1.0, 14.0);
            let segment = GullySegment {
                source_cell: cell.index,
                mouth_cell,
                source: WorldPosition::new(source_x, source_y, source_z),
                bend: WorldPosition::new(bend_x, bend_y, bend_z),
                mouth: WorldPosition::new(mouth_x, mouth_y, mouth_z),
                flow_accumulation_cells: cell.flow_accumulation_cells,
                half_width_meters,
                incision_depth_meters,
            };
            if !segment.descends_or_is_level() {
                return None;
            }
            segments.push(segment);
        }
        segments.sort_by_key(|segment| segment.source_cell);
        Some(Self {
            world: watershed.world,
            region: watershed.index,
            segments,
        })
    }

    pub fn segments(&self) -> &[GullySegment] {
        &self.segments
    }

    pub fn segment_from(&self, source: DrainageCellIndex) -> Option<&GullySegment> {
        self.segments
            .binary_search_by_key(&source, |segment| segment.source_cell)
            .ok()
            .map(|slot| &self.segments[slot])
    }
}

fn closest_on_segment(
    x: f64,
    z: f64,
    source: WorldPosition,
    mouth: WorldPosition,
    along_start: f64,
    along_end: f64,
) -> Option<(f64, f64)> {
    let segment_x = mouth.x - source.x;
    let segment_z = mouth.z - source.z;
    let length_squared = libm::fma(segment_x, segment_x, segment_z * segment_z);
    if length_squared <= 0.0 {
        return None;
    }
    let local_along = (libm::fma(x - source.x, segment_x, (z - source.z) * segment_z)
        / length_squared)
        .clamp(0.0, 1.0);
    let nearest_x = source.x + (segment_x * local_along);
    let nearest_z = source.z + (segment_z * local_along);
    let distance = libm::hypot(x - nearest_x, z - nearest_z);
    let along = along_start + ((along_end - along_start) * local_along);
    Some((distance, along))
}

/// River channels derived from one deterministic regional drainage artifact.
///
/// Rainfall becomes local runoff in every drainage cell, then accumulates
/// downstream through the region's flow graph. A channel begins once at least
/// 32 km² (eight coarse cells) contribute to it. Cross-region segments retain
/// an explicit mouth, while discharge includes only catchment represented by
/// this artifact; larger multi-region accumulation belongs to a later
/// hierarchical drainage layer.
#[derive(Clone, Debug, PartialEq)]
pub struct RiverNetwork {
    pub world: WorldIdentity,
    pub region: WatershedRegionIndex,
    segments: Vec<RiverSegment>,
}

impl RiverNetwork {
    pub fn generate(world: WorldIdentity, region: WatershedRegionIndex) -> Option<Self> {
        let watershed = WatershedRegion::generate(world, region)?;
        Self::from_watershed(&watershed)
    }

    /// Derives channels without regenerating an already-cached watershed.
    pub fn from_watershed(watershed: &WatershedRegion) -> Option<Self> {
        let cells = watershed.cells();
        let slots = cells
            .iter()
            .enumerate()
            .map(|(slot, cell)| (cell.index, slot))
            .collect::<BTreeMap<_, _>>();
        let mut incoming = vec![0_usize; cells.len()];
        let mut discharge = Vec::with_capacity(cells.len());

        for cell in cells {
            discharge.push(local_runoff(watershed.world, cell.index)?);
            if let Some(target) = cell.flow_to.and_then(|target| slots.get(&target)) {
                incoming[*target] = incoming[*target].checked_add(1)?;
            }
        }

        let mut ready = incoming
            .iter()
            .enumerate()
            .filter_map(|(slot, &count)| (count == 0).then_some(slot))
            .collect::<BTreeSet<_>>();
        let mut processed = 0_usize;
        while let Some(slot) = ready.pop_first() {
            processed = processed.checked_add(1)?;
            let Some(target) = cells[slot].flow_to.and_then(|target| slots.get(&target)) else {
                continue;
            };
            discharge[*target] += discharge[slot];
            incoming[*target] = incoming[*target].checked_sub(1)?;
            if incoming[*target] == 0 {
                ready.insert(*target);
            }
        }
        if processed != cells.len() {
            return None;
        }

        let terrain = MacroElevation::new(watershed.world);
        let cell_area_square_kilometers =
            DRAINAGE_CELL_EDGE_METERS * DRAINAGE_CELL_EDGE_METERS / 1_000_000.0;
        let mut segments = Vec::new();
        for (slot, cell) in cells.iter().enumerate() {
            if cell.flow_accumulation_cells < MIN_CHANNEL_CATCHMENT_CELLS {
                continue;
            }
            let Some(mouth_cell) = cell.flow_to else {
                continue;
            };
            let [source_x, source_z] = cell.index.center();
            let source_y = cell.filled_elevation_meters;
            let [mouth_x, mouth_z] = mouth_cell.center();
            let mouth_y = if let Some(&mouth_slot) = slots.get(&mouth_cell) {
                cells[mouth_slot].filled_elevation_meters
            } else {
                terrain.sample(mouth_x, mouth_z)?.elevation_meters
            };
            let segment = RiverSegment {
                source_cell: cell.index,
                mouth_cell,
                source: WorldPosition::new(source_x, source_y, source_z),
                mouth: WorldPosition::new(mouth_x, mouth_y, mouth_z),
                drainage_area_square_kilometers: u64_as_f64(cell.flow_accumulation_cells)
                    * cell_area_square_kilometers,
                discharge_cubic_meters_per_second: discharge[slot],
            };
            if !segment.descends_or_is_level() {
                return None;
            }
            segments.push(segment);
        }
        segments.sort_by_key(|segment| segment.source_cell);

        Some(Self {
            world: watershed.world,
            region: watershed.index,
            segments,
        })
    }

    pub fn segments(&self) -> &[RiverSegment] {
        &self.segments
    }

    pub fn segment_from(&self, source: DrainageCellIndex) -> Option<&RiverSegment> {
        self.segments
            .binary_search_by_key(&source, |segment| segment.source_cell)
            .ok()
            .map(|slot| &self.segments[slot])
    }
}

/// One deterministic lake occupying a filled drainage depression.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lake {
    pub id: u64,
    pub bottom: DrainageCellIndex,
    pub bottom_elevation_meters: f64,
    pub surface_elevation_meters: f64,
    pub outlet: DrainageCellIndex,
    pub cell_count: u64,
}

impl Lake {
    /// Returns the equilibrium water depth above a terrain elevation.
    pub fn water_depth_at(self, terrain_elevation_meters: f64) -> Option<f64> {
        terrain_elevation_meters
            .is_finite()
            .then_some((self.surface_elevation_meters - terrain_elevation_meters).max(0.0))
    }
}

/// Lakes derived from one deterministic regional drainage artifact.
///
/// Priority-Flood already identifies every depression cell and its shared
/// spill elevation. This artifact turns those basin labels into queryable
/// lakes without simulating water or depending on region visitation order.
#[derive(Clone, Debug, PartialEq)]
pub struct LakeNetwork {
    pub world: WorldIdentity,
    pub region: WatershedRegionIndex,
    lakes: Vec<Lake>,
    lake_by_cell: BTreeMap<DrainageCellIndex, usize>,
}

impl LakeNetwork {
    pub fn generate(world: WorldIdentity, region: WatershedRegionIndex) -> Option<Self> {
        let watershed = WatershedRegion::generate(world, region)?;
        Self::from_watershed(&watershed)
    }

    pub fn from_watershed(watershed: &WatershedRegion) -> Option<Self> {
        let lakes = watershed
            .basins()
            .iter()
            .map(|basin| Lake {
                id: basin.id,
                bottom: basin.bottom,
                bottom_elevation_meters: basin.bottom_elevation_meters,
                surface_elevation_meters: basin.spill_elevation_meters,
                outlet: basin.outlet,
                cell_count: basin.cell_count,
            })
            .collect::<Vec<_>>();
        let lake_slots = lakes
            .iter()
            .enumerate()
            .map(|(slot, lake)| (lake.id, slot))
            .collect::<BTreeMap<_, _>>();
        let lake_by_cell = watershed
            .cells()
            .iter()
            .filter_map(|cell| {
                let lake_id = cell.basin?;
                Some((cell.index, *lake_slots.get(&lake_id)?))
            })
            .collect::<BTreeMap<_, _>>();
        let assigned_cell_count = lakes
            .iter()
            .try_fold(0_u64, |total, lake| total.checked_add(lake.cell_count))?;
        if usize::try_from(assigned_cell_count).ok()? != lake_by_cell.len() {
            return None;
        }

        Some(Self {
            world: watershed.world,
            region: watershed.index,
            lakes,
            lake_by_cell,
        })
    }

    pub fn lakes(&self) -> &[Lake] {
        &self.lakes
    }

    pub fn lake_for_cell(&self, cell: DrainageCellIndex) -> Option<Lake> {
        self.lake_by_cell
            .get(&cell)
            .and_then(|&slot| self.lakes.get(slot))
            .copied()
    }
}

fn local_runoff(world: WorldIdentity, cell: DrainageCellIndex) -> Option<f64> {
    let [x, z] = cell.center();
    let cell_area_square_meters = DRAINAGE_CELL_EDGE_METERS * DRAINAGE_CELL_EDGE_METERS;
    if world.generator_version >= SEASONAL_CLIMATE_GENERATOR_VERSION {
        let climate = Climate::new(world).sample(x, z)?;
        let rainfall_meters = (climate.annual_precipitation_millimeters
            - climate.annual_snowfall_water_equivalent_millimeters)
            / 1_000.0;
        let snowmelt_meters = climate.annual_snowmelt_millimeters / 1_000.0;
        let rainfall_runoff_fraction =
            (0.72 - (climate.warmth_fraction() * 0.42)).clamp(0.15, 0.75);
        let annual_runoff_depth_meters =
            (rainfall_meters * rainfall_runoff_fraction) + (snowmelt_meters * 0.85);
        return Some(annual_runoff_depth_meters * cell_area_square_meters / SECONDS_PER_YEAR);
    }
    let (annual_precipitation_meters, runoff_fraction) =
        if world.generator_version >= OROGRAPHIC_CLIMATE_GENERATOR_VERSION {
            let climate = Climate::new(world).sample(x, z)?;
            (
                climate.annual_precipitation_millimeters / 1_000.0,
                (0.72 - (climate.warmth_fraction() * 0.42)).clamp(0.15, 0.75),
            )
        } else {
            let profile = RegionalProfile::sample(world, x, z)?;
            (
                0.25 + (profile.precipitation * 2.75),
                (0.75 - (profile.mean_temperature * 0.5)).clamp(0.15, 0.75),
            )
        };
    Some(annual_precipitation_meters * cell_area_square_meters * runoff_fraction / SECONDS_PER_YEAR)
}

/// A terrain basin filled to a single equilibrium surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basin {
    pub bottom_elevation: f64,
    pub spill_elevation: f64,
}

impl Basin {
    pub fn water_depth_at_spill(self) -> Option<f64> {
        (self.spill_elevation >= self.bottom_elevation)
            .then_some(self.spill_elevation - self.bottom_elevation)
    }
}

/// Compact persistent state used when a local water simulation is frozen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrozenWaterState {
    pub volume_cubic_meters: f64,
    pub surface_elevation: f64,
    pub outflow_cubic_meters_per_second: f64,
}

impl FrozenWaterState {
    pub fn is_physical(self) -> bool {
        self.volume_cubic_meters.is_finite()
            && self.volume_cubic_meters >= 0.0
            && self.surface_elevation.is_finite()
            && self.outflow_cubic_meters_per_second.is_finite()
            && self.outflow_cubic_meters_per_second >= 0.0
    }
}

/// Stable identity for one storage element in an active water region.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WaterCellId(pub u64);

/// The hydrological setting represented by an active water cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaterCellKind {
    Surface,
    CaveStream,
    Sump,
    Coast,
}

/// One bounded storage element in the active-region water model.
///
/// Cells deliberately represent local storage and routing rather than general
/// fluid voxels. Their regenerated bed and connection graph stay outside the
/// frozen state; only changing water depth needs to be retained.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterCell {
    pub id: WaterCellId,
    pub kind: WaterCellKind,
    pub bed_elevation_meters: f64,
    pub bank_elevation_meters: f64,
    pub area_square_meters: f64,
    pub water_depth_meters: f64,
    pub source_cubic_meters_per_second: f64,
    pub infiltration_cubic_meters_per_second: f64,
}

impl WaterCell {
    pub fn surface_elevation_meters(self) -> f64 {
        self.bed_elevation_meters + self.water_depth_meters
    }

    pub fn volume_cubic_meters(self) -> f64 {
        self.water_depth_meters * self.area_square_meters
    }

    pub fn flooded(self) -> bool {
        self.surface_elevation_meters() > self.bank_elevation_meters
    }

    fn is_physical(self) -> bool {
        self.bed_elevation_meters.is_finite()
            && self.bank_elevation_meters.is_finite()
            && self.bank_elevation_meters >= self.bed_elevation_meters
            && self.area_square_meters.is_finite()
            && self.area_square_meters > 0.0
            && self.water_depth_meters.is_finite()
            && self.water_depth_meters >= 0.0
            && self.source_cubic_meters_per_second.is_finite()
            && self.source_cubic_meters_per_second >= 0.0
            && self.infiltration_cubic_meters_per_second.is_finite()
            && self.infiltration_cubic_meters_per_second >= 0.0
    }
}

/// A spill-controlled connection between local water stores.
///
/// `to == None` is a boundary outlet. Internal connections are directional:
/// water can cross only from `from` toward `to`, matching the generated
/// drainage and cave graphs and preventing order-dependent oscillation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterConnection {
    pub from: WaterCellId,
    pub to: Option<WaterCellId>,
    pub sill_elevation_meters: f64,
    pub width_meters: f64,
    pub conductance: f64,
}

impl WaterConnection {
    fn is_physical(self) -> bool {
        self.sill_elevation_meters.is_finite()
            && self.width_meters.is_finite()
            && self.width_meters > 0.0
            && self.conductance.is_finite()
            && self.conductance > 0.0
            && self.to != Some(self.from)
    }
}

/// A controlled terrain deviation supplied to the active water model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterTerrainChange {
    pub cell: WaterCellId,
    pub new_bed_elevation_meters: f64,
    pub new_bank_elevation_meters: f64,
}

/// Observable fast-water morphology at one active connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FastWaterKind {
    Cascade,
    Waterfall,
}

/// Generated visible consequences of a steep, flowing connection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FastWaterFeature {
    pub connection_index: usize,
    pub kind: FastWaterKind,
    pub drop_meters: f64,
    pub discharge_cubic_meters_per_second: f64,
    pub plunge_pool_depth_meters: f64,
    pub plunge_pool_radius_meters: f64,
    pub downstream_gorge_depth_meters: f64,
}

/// Conservation and visible-state diagnostics returned by a fixed water step.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterStepReport {
    pub elapsed_seconds: f64,
    pub source_volume_cubic_meters: f64,
    pub boundary_outflow_volume_cubic_meters: f64,
    pub infiltrated_volume_cubic_meters: f64,
    pub storage_change_cubic_meters: f64,
    pub flooded_cells: Vec<WaterCellId>,
    pub fast_water: Vec<FastWaterFeature>,
}

impl WaterStepReport {
    pub fn conservation_error_cubic_meters(&self) -> f64 {
        self.storage_change_cubic_meters
            + self.boundary_outflow_volume_cubic_meters
            + self.infiltrated_volume_cubic_meters
            - self.source_volume_cubic_meters
    }
}

/// Compact changing state retained while an active water region is frozen.
///
/// Terrain, cell kinds, sources, and connections are pure generated data and
/// are reconstructed separately. Millimeter depths keep the summary compact
/// and give a deliberate, platform-independent persistence precision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenWaterRegion {
    pub elapsed_milliseconds: u64,
    pub cell_ids: Vec<WaterCellId>,
    pub depth_millimeters: Vec<u32>,
    pub last_boundary_outflow_milliliters_per_second: u64,
}

/// Errors that reject malformed active water data instead of allowing NaNs or
/// topology mistakes into the deterministic simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActiveWaterError {
    DuplicateCell,
    InvalidCell,
    InvalidConnection,
    MissingCell,
    InvalidTimeStep,
    InvalidTerrainChange,
    FrozenTopologyMismatch,
}

/// Deterministic, bounded active-region water storage and routing.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveWaterRegion {
    cells: Vec<WaterCell>,
    connections: Vec<WaterConnection>,
    elapsed_seconds: f64,
    last_boundary_outflow_cubic_meters_per_second: f64,
}

impl ActiveWaterRegion {
    /// Builds a validated, stably ordered active topology.
    ///
    /// # Errors
    ///
    /// Rejects non-physical cells or connections, duplicate identities, and
    /// connections whose endpoints are absent.
    pub fn new(
        mut cells: Vec<WaterCell>,
        mut connections: Vec<WaterConnection>,
    ) -> Result<Self, ActiveWaterError> {
        cells.sort_by_key(|cell| cell.id);
        if cells.iter().any(|cell| !cell.is_physical()) {
            return Err(ActiveWaterError::InvalidCell);
        }
        if cells.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(ActiveWaterError::DuplicateCell);
        }
        connections.sort_by(|left, right| {
            (left.from, left.to, left.sill_elevation_meters.to_bits()).cmp(&(
                right.from,
                right.to,
                right.sill_elevation_meters.to_bits(),
            ))
        });
        for connection in &connections {
            if !connection.is_physical() {
                return Err(ActiveWaterError::InvalidConnection);
            }
            if cell_slot(&cells, connection.from).is_none()
                || connection
                    .to
                    .is_some_and(|target| cell_slot(&cells, target).is_none())
            {
                return Err(ActiveWaterError::MissingCell);
            }
        }
        Ok(Self {
            cells,
            connections,
            elapsed_seconds: 0.0,
            last_boundary_outflow_cubic_meters_per_second: 0.0,
        })
    }

    pub fn cells(&self) -> &[WaterCell] {
        &self.cells
    }

    pub fn connections(&self) -> &[WaterConnection] {
        &self.connections
    }

    pub fn elapsed_seconds(&self) -> f64 {
        self.elapsed_seconds
    }

    pub fn cell(&self, id: WaterCellId) -> Option<&WaterCell> {
        cell_slot(&self.cells, id).map(|slot| &self.cells[slot])
    }

    pub fn total_volume_cubic_meters(&self) -> f64 {
        self.cells
            .iter()
            .map(|cell| cell.volume_cubic_meters())
            .sum()
    }

    /// Adds a controlled precipitation, snowmelt, or boundary-inflow pulse.
    ///
    /// # Errors
    ///
    /// Rejects negative or non-finite volumes and unknown cell identities.
    pub fn add_water_volume(
        &mut self,
        cell: WaterCellId,
        volume_cubic_meters: f64,
    ) -> Result<(), ActiveWaterError> {
        if !volume_cubic_meters.is_finite() || volume_cubic_meters < 0.0 {
            return Err(ActiveWaterError::InvalidCell);
        }
        let slot = cell_slot(&self.cells, cell).ok_or(ActiveWaterError::MissingCell)?;
        self.cells[slot].water_depth_meters +=
            volume_cubic_meters / self.cells[slot].area_square_meters;
        Ok(())
    }

    /// Applies terrain edits while conserving water already in each footprint.
    ///
    /// Raising a bed displaces its water surface upward; lowering a bed
    /// deepens storage. Routing on subsequent fixed steps determines whether
    /// the displaced water backs up, spills, or drains.
    ///
    /// # Errors
    ///
    /// Rejects unknown cells and non-finite or inverted bed/bank elevations.
    pub fn apply_terrain_changes(
        &mut self,
        changes: &[WaterTerrainChange],
    ) -> Result<(), ActiveWaterError> {
        let mut ordered = changes.to_vec();
        ordered.sort_by_key(|change| change.cell);
        for change in ordered {
            if !change.new_bed_elevation_meters.is_finite()
                || !change.new_bank_elevation_meters.is_finite()
                || change.new_bank_elevation_meters < change.new_bed_elevation_meters
            {
                return Err(ActiveWaterError::InvalidTerrainChange);
            }
            let slot = cell_slot(&self.cells, change.cell).ok_or(ActiveWaterError::MissingCell)?;
            self.cells[slot].bed_elevation_meters = change.new_bed_elevation_meters;
            self.cells[slot].bank_elevation_meters = change.new_bank_elevation_meters;
        }
        Ok(())
    }

    /// Advances the local storage graph with a conservative fixed step.
    ///
    /// The 60-second ceiling keeps routing stable and makes callers explicitly
    /// subdivide long frame or lifecycle gaps.
    ///
    /// # Errors
    ///
    /// Rejects non-finite, non-positive, or greater-than-60-second steps.
    pub fn step(&mut self, delta_seconds: f64) -> Result<WaterStepReport, ActiveWaterError> {
        if !(delta_seconds.is_finite() && 0.0 < delta_seconds && delta_seconds <= 60.0) {
            return Err(ActiveWaterError::InvalidTimeStep);
        }
        let initial_storage = self.total_volume_cubic_meters();
        let mut volumes = self
            .cells
            .iter()
            .map(|cell| cell.volume_cubic_meters())
            .collect::<Vec<_>>();
        let mut source_volume = 0.0;
        for (slot, cell) in self.cells.iter().enumerate() {
            let supplied = cell.source_cubic_meters_per_second * delta_seconds;
            volumes[slot] += supplied;
            source_volume += supplied;
        }

        let mut boundary_outflow = 0.0;
        let mut flows = vec![0.0; self.connections.len()];
        for (connection_index, connection) in self.connections.iter().enumerate() {
            let from_slot =
                cell_slot(&self.cells, connection.from).ok_or(ActiveWaterError::MissingCell)?;
            if volumes[from_slot] <= 0.0 {
                continue;
            }
            let from_surface = self.cells[from_slot].bed_elevation_meters
                + (volumes[from_slot] / self.cells[from_slot].area_square_meters);
            let destination_surface = connection
                .to
                .and_then(|id| cell_slot(&self.cells, id))
                .map_or(connection.sill_elevation_meters, |slot| {
                    self.cells[slot].bed_elevation_meters
                        + (volumes[slot] / self.cells[slot].area_square_meters)
                });
            let controlling_elevation = destination_surface.max(connection.sill_elevation_meters);
            let head = from_surface - controlling_elevation;
            if head <= 0.0 {
                continue;
            }
            let discharge = connection.conductance
                * connection.width_meters
                * libm::sqrt(2.0 * 9.806_65 * head);
            let moved = (discharge * delta_seconds).min(volumes[from_slot]);
            volumes[from_slot] -= moved;
            if let Some(to_slot) = connection.to.and_then(|id| cell_slot(&self.cells, id)) {
                volumes[to_slot] += moved;
            } else {
                boundary_outflow += moved;
            }
            flows[connection_index] = moved / delta_seconds;
        }

        let mut infiltrated = 0.0;
        for (slot, cell) in self.cells.iter().enumerate() {
            let loss =
                (cell.infiltration_cubic_meters_per_second * delta_seconds).min(volumes[slot]);
            volumes[slot] -= loss;
            infiltrated += loss;
        }
        for (cell, volume) in self.cells.iter_mut().zip(volumes) {
            cell.water_depth_meters = (volume / cell.area_square_meters).max(0.0);
        }
        self.elapsed_seconds += delta_seconds;
        self.last_boundary_outflow_cubic_meters_per_second = boundary_outflow / delta_seconds;

        let flooded_cells = self
            .cells
            .iter()
            .filter(|cell| cell.flooded())
            .map(|cell| cell.id)
            .collect();
        let fast_water = self.fast_water_features(&flows);
        let storage_change = self.total_volume_cubic_meters() - initial_storage;
        Ok(WaterStepReport {
            elapsed_seconds: self.elapsed_seconds,
            source_volume_cubic_meters: source_volume,
            boundary_outflow_volume_cubic_meters: boundary_outflow,
            infiltrated_volume_cubic_meters: infiltrated,
            storage_change_cubic_meters: storage_change,
            flooded_cells,
            fast_water,
        })
    }

    fn fast_water_features(&self, flows: &[f64]) -> Vec<FastWaterFeature> {
        self.connections
            .iter()
            .zip(flows)
            .enumerate()
            .filter_map(|(connection_index, (connection, &discharge))| {
                if discharge <= 0.01 {
                    return None;
                }
                let from = *self.cell(connection.from)?;
                let destination_elevation = connection
                    .to
                    .and_then(|id| self.cell(id))
                    .map_or(connection.sill_elevation_meters, |cell| {
                        cell.surface_elevation_meters()
                    });
                let drop_meters = from.surface_elevation_meters() - destination_elevation;
                let kind = if drop_meters >= 3.0 {
                    FastWaterKind::Waterfall
                } else if drop_meters >= 0.75 {
                    FastWaterKind::Cascade
                } else {
                    return None;
                };
                let energy = discharge * drop_meters;
                Some(FastWaterFeature {
                    connection_index,
                    kind,
                    drop_meters,
                    discharge_cubic_meters_per_second: discharge,
                    plunge_pool_depth_meters: (0.35 + (libm::sqrt(energy) * 0.28))
                        .clamp(0.35, 18.0),
                    plunge_pool_radius_meters: (1.0 + (libm::sqrt(discharge) * drop_meters * 0.45))
                        .clamp(1.0, 42.0),
                    downstream_gorge_depth_meters: (drop_meters * 0.42 + libm::sqrt(discharge))
                        .clamp(0.5, 32.0),
                })
            })
            .collect()
    }

    pub fn freeze(&self) -> FrozenWaterRegion {
        FrozenWaterRegion {
            elapsed_milliseconds: seconds_as_milliseconds(self.elapsed_seconds),
            cell_ids: self.cells.iter().map(|cell| cell.id).collect(),
            depth_millimeters: self
                .cells
                .iter()
                .map(|cell| meters_as_millimeters(cell.water_depth_meters))
                .collect(),
            last_boundary_outflow_milliliters_per_second: cubic_meters_as_milliliters(
                self.last_boundary_outflow_cubic_meters_per_second,
            ),
        }
    }

    /// Reconstructs changing state on top of a regenerated cell topology.
    ///
    /// # Errors
    ///
    /// Rejects summaries whose ordered cell identities do not exactly match
    /// the regenerated topology.
    pub fn reconstruct(
        mut regenerated: Self,
        frozen: &FrozenWaterRegion,
    ) -> Result<Self, ActiveWaterError> {
        if frozen.cell_ids.len() != frozen.depth_millimeters.len()
            || regenerated.cells.len() != frozen.cell_ids.len()
            || regenerated
                .cells
                .iter()
                .map(|cell| cell.id)
                .ne(frozen.cell_ids.iter().copied())
        {
            return Err(ActiveWaterError::FrozenTopologyMismatch);
        }
        for (cell, millimeters) in regenerated.cells.iter_mut().zip(&frozen.depth_millimeters) {
            cell.water_depth_meters = f64::from(*millimeters) / 1_000.0;
        }
        regenerated.elapsed_seconds = milliseconds_as_seconds(frozen.elapsed_milliseconds);
        regenerated.last_boundary_outflow_cubic_meters_per_second =
            milliliters_as_cubic_meters(frozen.last_boundary_outflow_milliliters_per_second);
        Ok(regenerated)
    }
}

fn cell_slot(cells: &[WaterCell], id: WaterCellId) -> Option<usize> {
    cells.binary_search_by_key(&id, |cell| cell.id).ok()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn meters_as_millimeters(meters: f64) -> u32 {
    libm::round((meters.max(0.0) * 1_000.0).min(f64::from(u32::MAX))) as u32
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn seconds_as_milliseconds(seconds: f64) -> u64 {
    libm::round((seconds.max(0.0) * 1_000.0).min(u64::MAX as f64)) as u64
}

#[allow(clippy::cast_precision_loss)]
fn milliseconds_as_seconds(milliseconds: u64) -> f64 {
    milliseconds as f64 / 1_000.0
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn cubic_meters_as_milliliters(cubic_meters: f64) -> u64 {
    libm::round((cubic_meters.max(0.0) * 1_000_000.0).min(u64::MAX as f64)) as u64
}

#[allow(clippy::cast_precision_loss)]
fn milliliters_as_cubic_meters(milliliters: u64) -> f64 {
    milliliters as f64 / 1_000_000.0
}

#[allow(clippy::cast_precision_loss)]
fn u64_as_f64(value: u64) -> f64 {
    value as f64
}

#[allow(clippy::cast_precision_loss)]
fn hash53_as_f64(value: u64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_coordinates::stable_hash;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, 2, 0);
    const CLIMATE_WORLD: WorldIdentity =
        WorldIdentity::new(0x5eed, OROGRAPHIC_CLIMATE_GENERATOR_VERSION, 0);
    const SEASONAL_WORLD: WorldIdentity =
        WorldIdentity::new(0x5eed, SEASONAL_CLIMATE_GENERATOR_VERSION, 0);

    #[test]
    fn river_direction_rejects_uphill_segments() {
        let river = RiverSegment {
            source_cell: DrainageCellIndex::new(0, 0),
            mouth_cell: DrainageCellIndex::new(1, 0),
            source: WorldPosition::new(0.0, 100.0, 0.0),
            mouth: WorldPosition::new(10.0, 101.0, 0.0),
            drainage_area_square_kilometers: 32.0,
            discharge_cubic_meters_per_second: 2.0,
        };
        assert!(!river.descends_or_is_level());
    }

    #[test]
    fn local_runoff_uses_orographic_climate_in_version_six() {
        let cell = DrainageCellIndex::new(-17, 23);
        let [x, z] = cell.center();
        let climate = Climate::new(CLIMATE_WORLD)
            .sample(x, z)
            .expect("finite climate");
        let expected = (climate.annual_precipitation_millimeters / 1_000.0)
            * DRAINAGE_CELL_EDGE_METERS
            * DRAINAGE_CELL_EDGE_METERS
            * (0.72 - (climate.warmth_fraction() * 0.42)).clamp(0.15, 0.75)
            / SECONDS_PER_YEAR;

        assert_eq!(
            local_runoff(CLIMATE_WORLD, cell).expect("runoff").to_bits(),
            expected.to_bits()
        );
    }

    #[test]
    fn local_runoff_combines_rainfall_and_snowmelt_in_version_seven() {
        let cell = DrainageCellIndex::new(-17, 23);
        let [x, z] = cell.center();
        let climate = Climate::new(SEASONAL_WORLD)
            .sample(x, z)
            .expect("finite climate");
        let rainfall_meters = (climate.annual_precipitation_millimeters
            - climate.annual_snowfall_water_equivalent_millimeters)
            / 1_000.0;
        let snowmelt_meters = climate.annual_snowmelt_millimeters / 1_000.0;
        let expected_depth = rainfall_meters
            * (0.72 - (climate.warmth_fraction() * 0.42)).clamp(0.15, 0.75)
            + (snowmelt_meters * 0.85);
        let cell_area = DRAINAGE_CELL_EDGE_METERS * DRAINAGE_CELL_EDGE_METERS;
        let expected = expected_depth * cell_area / SECONDS_PER_YEAR;

        assert!(climate.annual_snowfall_water_equivalent_millimeters > 0.0);
        assert_eq!(
            local_runoff(SEASONAL_WORLD, cell)
                .expect("runoff")
                .to_bits(),
            expected.to_bits()
        );
    }

    #[test]
    fn river_terrain_influence_is_centered_and_bounded() {
        let river = RiverSegment {
            source_cell: DrainageCellIndex::new(0, 0),
            mouth_cell: DrainageCellIndex::new(1, 0),
            source: WorldPosition::new(0.0, 100.0, 0.0),
            mouth: WorldPosition::new(2_000.0, 80.0, 0.0),
            drainage_area_square_kilometers: 128.0,
            discharge_cubic_meters_per_second: 8.0,
        };
        let center = river
            .terrain_influence(1_000.0, 0.0)
            .expect("centerline influence");
        assert!(center.distance_meters.abs() < f64::EPSILON);
        assert!((center.centerline_elevation_meters - 90.0).abs() < f64::EPSILON);
        assert!((center.blend - 1.0).abs() < f64::EPSILON);
        assert!(center.channel_half_width_meters < center.valley_half_width_meters);
        assert!(
            river
                .terrain_influence(1_000.0, MAX_RIVER_INFLUENCE_METERS + 1.0)
                .is_none()
        );
    }

    #[test]
    fn steep_discharging_rivers_generate_causal_fast_water_morphology() {
        let river = RiverSegment {
            source_cell: DrainageCellIndex::new(0, 0),
            mouth_cell: DrainageCellIndex::new(1, 0),
            source: WorldPosition::new(0.0, 120.0, 0.0),
            mouth: WorldPosition::new(1_000.0, 96.0, 0.0),
            drainage_area_square_kilometers: 96.0,
            discharge_cubic_meters_per_second: 3.0,
        };
        let lip = river
            .terrain_influence(650.0, 0.0)
            .expect("river influence")
            .fast_water
            .expect("condition-driven waterfall");
        let pool = river
            .terrain_influence(1_000.0, 0.0)
            .expect("river influence")
            .fast_water
            .expect("condition-driven waterfall");

        assert_eq!(lip.kind, FastWaterKind::Waterfall);
        assert!(lip.gorge_blend > 0.0);
        assert!(pool.plunge_pool_blend > 0.99);
        assert!(pool.plunge_pool_depth_meters > 0.0);
        assert!(pool.downstream_gorge_depth_meters > 0.0);
    }

    #[test]
    fn gully_influence_follows_its_bent_centerline_and_is_bounded() {
        let gully = GullySegment {
            source_cell: DrainageCellIndex::new(0, 0),
            mouth_cell: DrainageCellIndex::new(1, 0),
            source: WorldPosition::new(0.0, 100.0, 0.0),
            bend: WorldPosition::new(1_000.0, 90.0, 160.0),
            mouth: WorldPosition::new(2_000.0, 80.0, 0.0),
            flow_accumulation_cells: 4,
            half_width_meters: 60.0,
            incision_depth_meters: 6.0,
        };
        let center = gully
            .terrain_influence(1_000.0, 160.0)
            .expect("bend influence");

        assert!(center.distance_meters.abs() < f64::EPSILON);
        assert!((center.centerline_elevation_meters - 90.0).abs() < f64::EPSILON);
        assert!((center.blend - 1.0).abs() < f64::EPSILON);
        assert!(
            gully
                .terrain_influence(1_000.0, 160.0 + MAX_GULLY_INFLUENCE_METERS + 1.0)
                .is_none()
        );
    }

    #[test]
    fn basin_depth_is_derived_from_its_spill_point() {
        let basin = Basin {
            bottom_elevation: 70.0,
            spill_elevation: 82.5,
        };
        let depth = basin.water_depth_at_spill().expect("valid basin");
        assert!((depth - 12.5).abs() < f64::EPSILON);
    }

    #[test]
    fn generated_lakes_are_level_and_match_their_spill_outlets() {
        let region = WatershedRegionIndex::new(-1, 0);
        let watershed = WatershedRegion::generate(WORLD, region).expect("watershed");
        let network = LakeNetwork::from_watershed(&watershed).expect("lake network");

        assert!(!network.lakes().is_empty());
        for lake in network.lakes() {
            assert!(lake.surface_elevation_meters >= lake.bottom_elevation_meters);
            assert!(lake.cell_count > 0);
            assert!(
                watershed
                    .cells()
                    .iter()
                    .filter(|cell| cell.basin == Some(lake.id))
                    .all(|cell| network.lake_for_cell(cell.index) == Some(*lake))
            );
            assert_eq!(
                watershed
                    .basins()
                    .iter()
                    .find(|basin| basin.id == lake.id)
                    .map(|basin| (basin.spill_elevation_meters, basin.outlet)),
                Some((lake.surface_elevation_meters, lake.outlet))
            );
        }
    }

    #[test]
    fn lake_generation_handles_negative_coordinates_and_is_order_independent() {
        let first_index = WatershedRegionIndex::new(-2, -1);
        let second_index = WatershedRegionIndex::new(-1, -1);
        let first = LakeNetwork::generate(WORLD, first_index).expect("first");
        let second = LakeNetwork::generate(WORLD, second_index).expect("second");
        let second_again = LakeNetwork::generate(WORLD, second_index).expect("second again");
        let first_again = LakeNetwork::generate(WORLD, first_index).expect("first again");

        assert_eq!(first, first_again);
        assert_eq!(second, second_again);
    }

    #[test]
    fn lake_network_has_a_golden_fingerprint() {
        let network =
            LakeNetwork::generate(WORLD, WatershedRegionIndex::new(-1, 2)).expect("network");
        let words = network
            .lakes()
            .iter()
            .flat_map(|lake| {
                [
                    lake.id,
                    u64::from_le_bytes(lake.bottom.x.to_le_bytes()),
                    u64::from_le_bytes(lake.bottom.z.to_le_bytes()),
                    lake.bottom_elevation_meters.to_bits(),
                    lake.surface_elevation_meters.to_bits(),
                    u64::from_le_bytes(lake.outlet.x.to_le_bytes()),
                    u64::from_le_bytes(lake.outlet.z.to_le_bytes()),
                    lake.cell_count,
                ]
            })
            .collect::<Vec<_>>();

        assert_eq!(
            stable_hash(&words),
            12_959_953_739_099_618_601,
            "changing this value changes generated regional lakes"
        );
    }

    #[test]
    fn impossible_basins_are_rejected() {
        let basin = Basin {
            bottom_elevation: 10.0,
            spill_elevation: 9.0,
        };
        assert_eq!(basin.water_depth_at_spill(), None);
    }

    #[test]
    fn generated_rivers_are_connected_downhill_channels() {
        let network =
            RiverNetwork::generate(WORLD, WatershedRegionIndex::new(-1, 0)).expect("network");
        assert!(!network.segments().is_empty());
        for segment in network.segments() {
            assert!(segment.descends_or_is_level());
            assert!(segment.discharge_cubic_meters_per_second > 0.0);
            assert!(
                segment.source_cell.x.abs_diff(segment.mouth_cell.x) <= 1
                    && segment.source_cell.z.abs_diff(segment.mouth_cell.z) <= 1
            );
            assert!(segment.drainage_area_square_kilometers >= 32.0);
        }
    }

    #[test]
    fn discharge_does_not_decrease_at_internal_confluences() {
        let network =
            RiverNetwork::generate(WORLD, WatershedRegionIndex::new(0, 0)).expect("network");
        for segment in network.segments() {
            if let Some(downstream) = network.segment_from(segment.mouth_cell) {
                assert!(
                    downstream.discharge_cubic_meters_per_second
                        >= segment.discharge_cubic_meters_per_second
                );
            }
        }
    }

    #[test]
    fn river_generation_is_order_independent() {
        let first_index = WatershedRegionIndex::new(-2, 1);
        let second_index = WatershedRegionIndex::new(-1, 1);
        let first = RiverNetwork::generate(WORLD, first_index).expect("first");
        let second = RiverNetwork::generate(WORLD, second_index).expect("second");
        let second_again = RiverNetwork::generate(WORLD, second_index).expect("second again");
        let first_again = RiverNetwork::generate(WORLD, first_index).expect("first again");

        assert_eq!(first, first_again);
        assert_eq!(second, second_again);
    }

    #[test]
    fn river_network_has_a_golden_fingerprint() {
        let network =
            RiverNetwork::generate(WORLD, WatershedRegionIndex::new(-1, 2)).expect("network");
        let mut words = Vec::new();
        for segment in network.segments().iter().step_by(17) {
            words.extend([
                u64::from_le_bytes(segment.source_cell.x.to_le_bytes()),
                u64::from_le_bytes(segment.source_cell.z.to_le_bytes()),
                u64::from_le_bytes(segment.mouth_cell.x.to_le_bytes()),
                u64::from_le_bytes(segment.mouth_cell.z.to_le_bytes()),
                segment.source.y.to_bits(),
                segment.mouth.y.to_bits(),
                segment.discharge_cubic_meters_per_second.to_bits(),
            ]);
        }
        assert_eq!(
            stable_hash(&words),
            13_240_554_273_518_066_894,
            "changing this value changes generated regional rivers"
        );
    }

    #[test]
    fn climate_fed_river_network_has_a_golden_fingerprint() {
        let network = RiverNetwork::generate(CLIMATE_WORLD, WatershedRegionIndex::new(-1, 2))
            .expect("network");
        let words = network
            .segments()
            .iter()
            .step_by(17)
            .flat_map(|segment| {
                [
                    u64::from_le_bytes(segment.source_cell.x.to_le_bytes()),
                    u64::from_le_bytes(segment.source_cell.z.to_le_bytes()),
                    segment.discharge_cubic_meters_per_second.to_bits(),
                ]
            })
            .collect::<Vec<_>>();

        assert_eq!(
            stable_hash(&words),
            10_865_102_493_463_890_939,
            "changing this value changes climate-fed regional rivers"
        );
    }

    #[test]
    fn seasonal_runoff_river_network_has_a_golden_fingerprint() {
        let network = RiverNetwork::generate(SEASONAL_WORLD, WatershedRegionIndex::new(-1, 2))
            .expect("network");
        let words = network
            .segments()
            .iter()
            .step_by(17)
            .flat_map(|segment| {
                [
                    u64::from_le_bytes(segment.source_cell.x.to_le_bytes()),
                    u64::from_le_bytes(segment.source_cell.z.to_le_bytes()),
                    segment.discharge_cubic_meters_per_second.to_bits(),
                ]
            })
            .collect::<Vec<_>>();

        assert_eq!(
            stable_hash(&words),
            2_193_941_950_152_600_383,
            "changing this value changes seasonal-runoff regional rivers"
        );
    }

    #[test]
    fn generated_gullies_are_connected_minor_downhill_channels() {
        let watershed =
            WatershedRegion::generate(WORLD, WatershedRegionIndex::new(-1, 0)).expect("watershed");
        let network = GullyNetwork::from_watershed(&watershed).expect("gully network");

        assert!(!network.segments().is_empty());
        for segment in network.segments() {
            assert!(segment.descends_or_is_level());
            assert!(segment.flow_accumulation_cells < MIN_CHANNEL_CATCHMENT_CELLS);
            assert!(
                segment.source_cell.x.abs_diff(segment.mouth_cell.x) <= 1
                    && segment.source_cell.z.abs_diff(segment.mouth_cell.z) <= 1
            );
            assert!((24.0..=MAX_GULLY_INFLUENCE_METERS).contains(&segment.half_width_meters));
            assert!((1.0..=14.0).contains(&segment.incision_depth_meters));
        }
    }

    #[test]
    fn gully_generation_handles_negative_coordinates_and_is_order_independent() {
        let first_index = WatershedRegionIndex::new(-2, -1);
        let second_index = WatershedRegionIndex::new(-1, -1);
        let first = GullyNetwork::generate(WORLD, first_index).expect("first");
        let second = GullyNetwork::generate(WORLD, second_index).expect("second");
        let second_again = GullyNetwork::generate(WORLD, second_index).expect("second again");
        let first_again = GullyNetwork::generate(WORLD, first_index).expect("first again");

        assert_eq!(first, first_again);
        assert_eq!(second, second_again);
    }

    #[test]
    fn gully_network_has_a_golden_fingerprint() {
        let network =
            GullyNetwork::generate(WORLD, WatershedRegionIndex::new(-1, 2)).expect("network");
        let words = network
            .segments()
            .iter()
            .step_by(31)
            .flat_map(|segment| {
                [
                    u64::from_le_bytes(segment.source_cell.x.to_le_bytes()),
                    u64::from_le_bytes(segment.source_cell.z.to_le_bytes()),
                    segment.bend.x.to_bits(),
                    segment.bend.z.to_bits(),
                    segment.half_width_meters.to_bits(),
                    segment.incision_depth_meters.to_bits(),
                ]
            })
            .collect::<Vec<_>>();

        assert_eq!(
            stable_hash(&words),
            16_211_266_877_087_865_594,
            "changing this value changes generated meso-scale gullies"
        );
    }

    fn active_test_region() -> ActiveWaterRegion {
        ActiveWaterRegion::new(
            vec![
                WaterCell {
                    id: WaterCellId(10),
                    kind: WaterCellKind::Surface,
                    bed_elevation_meters: 12.0,
                    bank_elevation_meters: 13.0,
                    area_square_meters: 100.0,
                    water_depth_meters: 1.5,
                    source_cubic_meters_per_second: 0.4,
                    infiltration_cubic_meters_per_second: 0.01,
                },
                WaterCell {
                    id: WaterCellId(20),
                    kind: WaterCellKind::Surface,
                    bed_elevation_meters: 5.0,
                    bank_elevation_meters: 6.5,
                    area_square_meters: 120.0,
                    water_depth_meters: 0.2,
                    source_cubic_meters_per_second: 0.0,
                    infiltration_cubic_meters_per_second: 0.02,
                },
                WaterCell {
                    id: WaterCellId(30),
                    kind: WaterCellKind::Sump,
                    bed_elevation_meters: 1.0,
                    bank_elevation_meters: 4.0,
                    area_square_meters: 80.0,
                    water_depth_meters: 0.0,
                    source_cubic_meters_per_second: 0.0,
                    infiltration_cubic_meters_per_second: 0.0,
                },
            ],
            vec![
                WaterConnection {
                    from: WaterCellId(10),
                    to: Some(WaterCellId(20)),
                    sill_elevation_meters: 12.3,
                    width_meters: 3.0,
                    conductance: 0.12,
                },
                WaterConnection {
                    from: WaterCellId(20),
                    to: Some(WaterCellId(30)),
                    sill_elevation_meters: 5.1,
                    width_meters: 2.0,
                    conductance: 0.09,
                },
                WaterConnection {
                    from: WaterCellId(30),
                    to: None,
                    sill_elevation_meters: 3.0,
                    width_meters: 1.0,
                    conductance: 0.08,
                },
            ],
        )
        .expect("physical active region")
    }

    #[test]
    fn active_water_steps_are_deterministic_and_conservative() {
        let mut first = active_test_region();
        let mut second = active_test_region();
        for _ in 0..40 {
            let first_report = first.step(2.0).expect("water step");
            let second_report = second.step(2.0).expect("water step");
            assert_eq!(first_report, second_report);
            assert!(
                first_report.conservation_error_cubic_meters().abs() < 1.0e-9,
                "local storage must conserve supplied water"
            );
        }
        assert_eq!(first, second);
        assert!(
            first
                .cell(WaterCellId(30))
                .expect("sump")
                .water_depth_meters
                > 0.0
        );
    }

    #[test]
    fn raised_terrain_displaces_water_and_changes_routing() {
        let mut region = active_test_region();
        let volume_before = region.total_volume_cubic_meters();
        let surface_before = region
            .cell(WaterCellId(10))
            .expect("upstream")
            .surface_elevation_meters();
        region
            .apply_terrain_changes(&[WaterTerrainChange {
                cell: WaterCellId(10),
                new_bed_elevation_meters: 13.0,
                new_bank_elevation_meters: 14.0,
            }])
            .expect("valid controlled edit");

        assert_eq!(
            region.total_volume_cubic_meters().to_bits(),
            volume_before.to_bits()
        );
        assert!(
            region
                .cell(WaterCellId(10))
                .expect("upstream")
                .surface_elevation_meters()
                > surface_before
        );
        let downstream_before = region
            .cell(WaterCellId(20))
            .expect("downstream")
            .volume_cubic_meters();
        region.step(1.0).expect("water response");
        assert!(
            region
                .cell(WaterCellId(20))
                .expect("downstream")
                .volume_cubic_meters()
                > downstream_before
        );
    }

    #[test]
    fn filling_spills_then_floods_a_bounded_basin() {
        let mut region = ActiveWaterRegion::new(
            vec![
                WaterCell {
                    id: WaterCellId(1),
                    kind: WaterCellKind::Surface,
                    bed_elevation_meters: 0.0,
                    bank_elevation_meters: 1.0,
                    area_square_meters: 10.0,
                    water_depth_meters: 0.0,
                    source_cubic_meters_per_second: 2.0,
                    infiltration_cubic_meters_per_second: 0.0,
                },
                WaterCell {
                    id: WaterCellId(2),
                    kind: WaterCellKind::Surface,
                    bed_elevation_meters: -0.5,
                    bank_elevation_meters: 0.5,
                    area_square_meters: 10.0,
                    water_depth_meters: 0.0,
                    source_cubic_meters_per_second: 0.0,
                    infiltration_cubic_meters_per_second: 0.0,
                },
            ],
            vec![WaterConnection {
                from: WaterCellId(1),
                to: Some(WaterCellId(2)),
                sill_elevation_meters: 0.6,
                width_meters: 1.0,
                conductance: 0.15,
            }],
        )
        .expect("basin");

        for _ in 0..10 {
            region.step(1.0).expect("fill");
        }
        assert!(
            region
                .cell(WaterCellId(2))
                .expect("outflow")
                .water_depth_meters
                > 0.0
        );
        assert!(
            region
                .step(10.0)
                .expect("flood")
                .flooded_cells
                .contains(&WaterCellId(1))
        );
    }

    #[test]
    fn steep_surface_to_cave_flow_generates_waterfall_and_plunge_pool() {
        let mut region = active_test_region();
        let report = region.step(1.0).expect("water step");
        assert!(report.fast_water.iter().any(|feature| {
            feature.kind == FastWaterKind::Waterfall
                && feature.plunge_pool_depth_meters > 0.0
                && feature.downstream_gorge_depth_meters > 0.0
        }));
    }

    #[test]
    fn frozen_summary_reconstructs_deterministically_on_regenerated_topology() {
        let mut active = active_test_region();
        for _ in 0..12 {
            active.step(2.5).expect("water step");
        }
        let frozen = active.freeze();
        let first =
            ActiveWaterRegion::reconstruct(active_test_region(), &frozen).expect("reconstruction");
        let second =
            ActiveWaterRegion::reconstruct(active_test_region(), &frozen).expect("reconstruction");

        assert_eq!(first, second);
        assert_eq!(first.freeze(), frozen);
        assert!(
            first
                .cells()
                .iter()
                .zip(active.cells())
                .all(|(restored, original)| {
                    (restored.water_depth_meters - original.water_depth_meters).abs() <= 0.0005
                })
        );
    }

    #[test]
    fn active_water_rejects_invalid_topology_and_time() {
        let mut region = active_test_region();
        assert_eq!(region.step(0.0), Err(ActiveWaterError::InvalidTimeStep));
        assert_eq!(
            ActiveWaterRegion::new(
                region.cells().to_vec(),
                vec![WaterConnection {
                    from: WaterCellId(999),
                    to: None,
                    sill_elevation_meters: 0.0,
                    width_meters: 1.0,
                    conductance: 1.0,
                }],
            ),
            Err(ActiveWaterError::MissingCell)
        );
    }
}
