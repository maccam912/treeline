//! Deterministic, spatially continuous regional parameter fields.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, VecDeque};

use treeline_coordinates::{CellIndex, WorldIdentity, stable_hash};

const DOMAIN_UPLIFT: u64 = 0x5550_4c49_4654;
const DOMAIN_EROSION: u64 = 0x0045_524f_5349_4f4e;
const DOMAIN_ROCK: u64 = 0x524f_434b;
const DOMAIN_RAIN: u64 = 0x5241_494e;
const DOMAIN_TEMP: u64 = 0x5445_4d50;
const DOMAIN_KARST: u64 = 0x004b_4152_5354;
const DOMAIN_BASE_ELEVATION: u64 = 0x4241_5345_454c_4556;
const DOMAIN_MOUNTAIN_RIDGE: u64 = 0x4d4f_554e_5441_494e;
const DOMAIN_DRAINAGE_BASIN: u64 = 0x4452_4149_4e42_4153;
const DOMAIN_WIND_X: u64 = 0x5749_4e44_5f58;
const DOMAIN_WIND_Z: u64 = 0x5749_4e44_5f5a;

const MACRO_CELL_EDGE_METERS: f64 = 64_000.0;
const CLIMATE_CELL_EDGE_METERS: f64 = 100_000.0;
const WIND_CELL_EDGE_METERS: f64 = 400_000.0;
const LATITUDE_HALF_CYCLE_METERS: f64 = 2_000_000.0;
const OCEAN_SEARCH_RADIUS_METERS: f64 = 800_000.0;
const OCEAN_SAMPLE_DISTANCES_METERS: [f64; 2] = [250_000.0, 750_000.0];
const OCEAN_SAMPLE_DIRECTIONS: [[f64; 2]; 8] = [
    [1.0, 0.0],
    [-1.0, 0.0],
    [0.0, 1.0],
    [0.0, -1.0],
    [
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    ],
    [
        -std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    ],
    [
        std::f64::consts::FRAC_1_SQRT_2,
        -std::f64::consts::FRAC_1_SQRT_2,
    ],
    [
        -std::f64::consts::FRAC_1_SQRT_2,
        -std::f64::consts::FRAC_1_SQRT_2,
    ],
];
const OROGRAPHIC_SAMPLE_STEP_METERS: f64 = 12_000.0;
const OROGRAPHIC_SAMPLE_COUNT: u32 = 4;
pub const DRAINAGE_CELL_EDGE_METERS: f64 = 2_000.0;
pub const WATERSHED_REGION_CELLS: usize = 64;
const WATERSHED_REGION_CELLS_I64: i64 = 64;
pub const WATERSHED_REGION_EDGE_METERS: f64 = 128_000.0;
/// Generator version that first derives climate from elevation and prevailing wind.
pub const OROGRAPHIC_CLIMATE_GENERATOR_VERSION: u32 = 6;
/// Generator version that adds latitude, continentality, seasons, and snowpack.
pub const SEASONAL_CLIMATE_GENERATOR_VERSION: u32 = 7;

/// A representative quarter of the deterministic annual climate cycle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Season {
    #[default]
    Winter,
    Spring,
    Summer,
    Autumn,
}

impl Season {
    pub const ALL: [Self; 4] = [Self::Winter, Self::Spring, Self::Summer, Self::Autumn];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Winter => "winter",
            Self::Spring => "spring",
            Self::Summer => "summer",
            Self::Autumn => "autumn",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Winter => Self::Spring,
            Self::Spring => Self::Summer,
            Self::Summer => Self::Autumn,
            Self::Autumn => Self::Winter,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Winter => 0,
            Self::Spring => 1,
            Self::Summer => 2,
            Self::Autumn => 3,
        }
    }

    const fn temperature_factor(self) -> f64 {
        match self {
            Self::Winter => -0.85,
            Self::Spring => -0.15,
            Self::Summer => 0.85,
            Self::Autumn => 0.15,
        }
    }
}

/// Coherent environmental parameters sampled at a horizontal world position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionalProfile {
    pub uplift: f64,
    pub erosion_age: f64,
    pub rock_hardness: f64,
    pub precipitation: f64,
    pub mean_temperature: f64,
    pub karst_probability: f64,
}

impl RegionalProfile {
    /// Samples correlated fields whose values remain in the inclusive range 0–1.
    pub fn sample(world: WorldIdentity, x: f64, z: f64) -> Option<Self> {
        Some(Self {
            uplift: value_field(world, DOMAIN_UPLIFT, x, z, CLIMATE_CELL_EDGE_METERS)?,
            erosion_age: value_field(world, DOMAIN_EROSION, x, z, CLIMATE_CELL_EDGE_METERS)?,
            rock_hardness: value_field(world, DOMAIN_ROCK, x, z, CLIMATE_CELL_EDGE_METERS)?,
            precipitation: value_field(world, DOMAIN_RAIN, x, z, CLIMATE_CELL_EDGE_METERS)?,
            mean_temperature: value_field(world, DOMAIN_TEMP, x, z, CLIMATE_CELL_EDGE_METERS)?,
            karst_probability: value_field(world, DOMAIN_KARST, x, z, CLIMATE_CELL_EDGE_METERS)?,
        })
    }
}

/// Explainable components of the macro-scale surface elevation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacroTerrainSample {
    pub elevation_meters: f64,
    pub base_elevation_meters: f64,
    pub mountain_uplift_meters: f64,
    pub dominant_ridge: Option<CellIndex>,
}

/// Deterministic continental relief assembled from elongated mountain features.
///
/// Each 64 km cell owns one compactly supported ridge segment. Sampling checks
/// the surrounding cells and evaluates their features analytically, so the
/// result has no generation-order dependency or region-edge seam. The square
/// root in segment normalization is the only non-integer operation that shapes
/// feature identity; supported targets are required to use IEEE-754 `f64`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacroElevation {
    pub world: WorldIdentity,
}

impl MacroElevation {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Samples broad elevation and reports the ridge responsible for uplift.
    pub fn sample(self, x: f64, z: f64) -> Option<MacroTerrainSample> {
        let containing = CellIndex::containing(x, z, 0, MACRO_CELL_EDGE_METERS)?;
        let uplift = value_field(self.world, DOMAIN_UPLIFT, x, z, 100_000.0)?;
        let base_control = value_field(self.world, DOMAIN_BASE_ELEVATION, x, z, 160_000.0)?;
        let base_elevation_meters = -40.0 + (base_control * 260.0) + ((uplift - 0.5) * 120.0);

        let mut mountain_uplift_meters = 0.0;
        let mut dominant_ridge = None;
        for z_offset in -1..=1 {
            for x_offset in -1..=1 {
                let Some(cell_x) = containing.x.checked_add(x_offset) else {
                    continue;
                };
                let Some(cell_z) = containing.z.checked_add(z_offset) else {
                    continue;
                };
                let cell = CellIndex::new(cell_x, cell_z, 0);
                let ridge = MountainRidge::from_cell(self.world, cell)?;
                let uplift = ridge.uplift_at(x, z);
                if uplift > mountain_uplift_meters {
                    mountain_uplift_meters = uplift;
                    dominant_ridge = Some(cell);
                }
            }
        }

        Some(MacroTerrainSample {
            elevation_meters: base_elevation_meters + mountain_uplift_meters,
            base_elevation_meters,
            mountain_uplift_meters,
            dominant_ridge,
        })
    }
}

/// Functional climate sampler derived from regional controls and macro terrain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Climate {
    pub world: WorldIdentity,
}

impl Climate {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Samples explainable mean climate at one horizontal world position.
    ///
    /// Wind is a spatially correlated vector field. Four fixed-distance
    /// upwind terrain samples establish windward lift and lee-side rain
    /// shadow. These world-space samples make the result independent of voxel
    /// LOD, job order, and the watershed artifact containing the point.
    /// Version 7 adds a broad repeating latitude-like field and fixed
    /// world-space macro-elevation samples as an explainable ocean-proximity
    /// proxy. Wind normalization and all latitude, continentality, and
    /// seasonal `f64` operations are part of the generation contract. Wind
    /// normalization deliberately uses the same explicitly sequenced square
    /// root on every target rather than the platform `hypot` implementation.
    pub fn sample(self, x: f64, z: f64) -> Option<ClimateSample> {
        let profile = RegionalProfile::sample(self.world, x, z)?;
        let elevation_meters = MacroElevation::new(self.world)
            .sample(x, z)?
            .elevation_meters;
        let prevailing_wind = prevailing_wind(self.world, x, z)?;

        let mut highest_upwind_elevation_meters = elevation_meters;
        let mut weighted_upwind_elevation_meters = 0.0;
        let mut total_weight = 0.0;
        for step in 1..=OROGRAPHIC_SAMPLE_COUNT {
            let distance = f64::from(step) * OROGRAPHIC_SAMPLE_STEP_METERS;
            let upwind_x = x - (prevailing_wind[0] * distance);
            let upwind_z = z - (prevailing_wind[1] * distance);
            let upwind_elevation = MacroElevation::new(self.world)
                .sample(upwind_x, upwind_z)?
                .elevation_meters;
            highest_upwind_elevation_meters = highest_upwind_elevation_meters.max(upwind_elevation);
            let weight = f64::from(OROGRAPHIC_SAMPLE_COUNT + 1 - step);
            weighted_upwind_elevation_meters += upwind_elevation * weight;
            total_weight += weight;
        }
        let upwind_elevation_meters = weighted_upwind_elevation_meters / total_weight;
        let orographic_lift_meters = (elevation_meters - upwind_elevation_meters).max(0.0);
        let rain_shadow_meters = (highest_upwind_elevation_meters - elevation_meters).max(0.0);

        let temperature = temperature_controls(self.world, profile, x, z, elevation_meters)?;
        let elevation_cooling_celsius = elevation_meters.max(0.0) * 0.0065;
        let mean_temperature_celsius =
            temperature.baseline_temperature_celsius - elevation_cooling_celsius;

        let baseline_annual_precipitation_millimeters = 250.0 + (profile.precipitation * 2_250.0);
        let windward_gain = 0.8 * (orographic_lift_meters / 1_000.0).clamp(0.0, 1.0);
        let rain_shadow_loss = 0.75 * (rain_shadow_meters / 1_200.0).clamp(0.0, 1.0);
        let precipitation_multiplier = (1.0 + windward_gain - rain_shadow_loss).clamp(0.2, 1.8);
        let annual_precipitation_millimeters =
            baseline_annual_precipitation_millimeters * precipitation_multiplier;

        let snow = if self.world.generator_version >= SEASONAL_CLIMATE_GENERATOR_VERSION {
            snow_cycle(
                mean_temperature_celsius,
                temperature.seasonal_temperature_amplitude_celsius,
                annual_precipitation_millimeters,
                temperature.precipitation_seasonality_fraction,
            )
        } else {
            SnowCycle::default()
        };

        Some(ClimateSample {
            elevation_meters,
            prevailing_wind,
            upwind_elevation_meters,
            highest_upwind_elevation_meters,
            orographic_lift_meters,
            rain_shadow_meters,
            latitude_warmth_fraction: temperature.latitude_warmth_fraction,
            ocean_proximity_fraction: temperature.ocean_proximity_fraction,
            continentality_fraction: temperature.continentality_fraction,
            latitude_temperature_celsius: temperature.latitude_temperature_celsius,
            regional_temperature_anomaly_celsius: temperature.regional_temperature_anomaly_celsius,
            continentality_adjustment_celsius: temperature.continentality_adjustment_celsius,
            baseline_temperature_celsius: temperature.baseline_temperature_celsius,
            elevation_cooling_celsius,
            mean_temperature_celsius,
            seasonal_temperature_amplitude_celsius: temperature
                .seasonal_temperature_amplitude_celsius,
            precipitation_seasonality_fraction: temperature.precipitation_seasonality_fraction,
            baseline_annual_precipitation_millimeters,
            annual_precipitation_millimeters,
            annual_snowfall_water_equivalent_millimeters: snow.annual_snowfall,
            permanent_snowpack_water_equivalent_millimeters: snow.permanent_snowpack,
            maximum_snowpack_water_equivalent_millimeters: snow.maximum_snowpack,
            annual_snowmelt_millimeters: snow.annual_snowmelt,
        })
    }

    /// Samples one explicit season without consulting simulation or wall-clock state.
    pub fn sample_season(self, x: f64, z: f64, season: Season) -> Option<SeasonalClimateSample> {
        let annual = self.sample(x, z)?;
        if self.world.generator_version < SEASONAL_CLIMATE_GENERATOR_VERSION {
            return Some(SeasonalClimateSample {
                season,
                mean_temperature_celsius: annual.mean_temperature_celsius,
                precipitation_millimeters: annual.annual_precipitation_millimeters * 0.25,
                rainfall_millimeters: annual.annual_precipitation_millimeters * 0.25,
                snowfall_water_equivalent_millimeters: 0.0,
                snowpack_water_equivalent_millimeters: 0.0,
                snowmelt_millimeters: 0.0,
            });
        }
        let snow = snow_cycle(
            annual.mean_temperature_celsius,
            annual.seasonal_temperature_amplitude_celsius,
            annual.annual_precipitation_millimeters,
            annual.precipitation_seasonality_fraction,
        );
        let index = season.index();
        Some(SeasonalClimateSample {
            season,
            mean_temperature_celsius: seasonal_temperature(
                annual.mean_temperature_celsius,
                annual.seasonal_temperature_amplitude_celsius,
                season,
            ),
            precipitation_millimeters: snow.precipitation[index],
            rainfall_millimeters: snow.rainfall[index],
            snowfall_water_equivalent_millimeters: snow.snowfall[index],
            snowpack_water_equivalent_millimeters: snow.snowpack[index],
            snowmelt_millimeters: snow.snowmelt[index],
        })
    }
}

/// Explainable climate contributors at one horizontal world position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClimateSample {
    pub elevation_meters: f64,
    pub prevailing_wind: [f64; 2],
    pub upwind_elevation_meters: f64,
    pub highest_upwind_elevation_meters: f64,
    pub orographic_lift_meters: f64,
    pub rain_shadow_meters: f64,
    pub latitude_warmth_fraction: f64,
    pub ocean_proximity_fraction: f64,
    pub continentality_fraction: f64,
    pub latitude_temperature_celsius: f64,
    pub regional_temperature_anomaly_celsius: f64,
    pub continentality_adjustment_celsius: f64,
    pub baseline_temperature_celsius: f64,
    pub elevation_cooling_celsius: f64,
    pub mean_temperature_celsius: f64,
    pub seasonal_temperature_amplitude_celsius: f64,
    pub precipitation_seasonality_fraction: f64,
    pub baseline_annual_precipitation_millimeters: f64,
    pub annual_precipitation_millimeters: f64,
    pub annual_snowfall_water_equivalent_millimeters: f64,
    pub permanent_snowpack_water_equivalent_millimeters: f64,
    pub maximum_snowpack_water_equivalent_millimeters: f64,
    pub annual_snowmelt_millimeters: f64,
}

impl ClimateSample {
    pub fn precipitation_fraction(self) -> f64 {
        ((self.annual_precipitation_millimeters - 250.0) / 2_250.0).clamp(0.0, 1.0)
    }

    pub fn warmth_fraction(self) -> f64 {
        ((self.mean_temperature_celsius + 20.0) / 55.0).clamp(0.0, 1.0)
    }
}

/// Explainable climate state for one representative season.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeasonalClimateSample {
    pub season: Season,
    pub mean_temperature_celsius: f64,
    pub precipitation_millimeters: f64,
    pub rainfall_millimeters: f64,
    pub snowfall_water_equivalent_millimeters: f64,
    pub snowpack_water_equivalent_millimeters: f64,
    pub snowmelt_millimeters: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SnowCycle {
    precipitation: [f64; 4],
    rainfall: [f64; 4],
    snowfall: [f64; 4],
    snowpack: [f64; 4],
    snowmelt: [f64; 4],
    annual_snowfall: f64,
    permanent_snowpack: f64,
    maximum_snowpack: f64,
    annual_snowmelt: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TemperatureControls {
    latitude_warmth_fraction: f64,
    ocean_proximity_fraction: f64,
    continentality_fraction: f64,
    latitude_temperature_celsius: f64,
    regional_temperature_anomaly_celsius: f64,
    continentality_adjustment_celsius: f64,
    baseline_temperature_celsius: f64,
    seasonal_temperature_amplitude_celsius: f64,
    precipitation_seasonality_fraction: f64,
}

/// Integer identity of a cell on the global drainage lattice.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DrainageCellIndex {
    pub x: i64,
    pub z: i64,
}

impl DrainageCellIndex {
    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    pub fn containing(x: f64, z: f64) -> Option<Self> {
        CellIndex::containing(x, z, 0, DRAINAGE_CELL_EDGE_METERS)
            .map(|cell| Self::new(cell.x, cell.z))
    }

    pub fn center(self) -> [f64; 2] {
        [
            (index_as_f64(self.x) + 0.5) * DRAINAGE_CELL_EDGE_METERS,
            (index_as_f64(self.z) + 0.5) * DRAINAGE_CELL_EDGE_METERS,
        ]
    }
}

/// Integer identity of one independently reproducible drainage artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WatershedRegionIndex {
    pub x: i64,
    pub z: i64,
}

impl WatershedRegionIndex {
    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    pub fn containing(x: f64, z: f64) -> Option<Self> {
        CellIndex::containing(x, z, 0, WATERSHED_REGION_EDGE_METERS)
            .map(|cell| Self::new(cell.x, cell.z))
    }

    pub fn containing_cell(cell: DrainageCellIndex) -> Self {
        Self::new(
            cell.x.div_euclid(WATERSHED_REGION_CELLS_I64),
            cell.z.div_euclid(WATERSHED_REGION_CELLS_I64),
        )
    }

    pub fn origin(self) -> [f64; 2] {
        [
            index_as_f64(self.x) * WATERSHED_REGION_EDGE_METERS,
            index_as_f64(self.z) * WATERSHED_REGION_EDGE_METERS,
        ]
    }

    fn global_cell(self, local_x: usize, local_z: usize) -> Option<DrainageCellIndex> {
        let cells = i64::try_from(WATERSHED_REGION_CELLS).ok()?;
        let local_x = i64::try_from(local_x).ok()?;
        let local_z = i64::try_from(local_z).ok()?;
        Some(DrainageCellIndex::new(
            self.x.checked_mul(cells)?.checked_add(local_x)?,
            self.z.checked_mul(cells)?.checked_add(local_z)?,
        ))
    }
}

/// Explainable hydrology values for one coarse drainage cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrainageCell {
    pub index: DrainageCellIndex,
    pub elevation_meters: f64,
    pub filled_elevation_meters: f64,
    pub flow_to: Option<DrainageCellIndex>,
    pub flow_accumulation_cells: u64,
    /// The boundary exit (or terminal boundary cell) that owns this catchment.
    pub watershed_outlet: DrainageCellIndex,
    pub basin: Option<u64>,
}

impl DrainageCell {
    pub fn is_depression(self) -> bool {
        self.filled_elevation_meters > self.elevation_meters
    }
}

/// A level depression and its deterministic spill route.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrainageBasin {
    pub id: u64,
    pub bottom: DrainageCellIndex,
    pub bottom_elevation_meters: f64,
    pub spill_elevation_meters: f64,
    pub outlet: DrainageCellIndex,
    pub cell_count: u64,
}

/// A deterministic regional drainage artifact derived from macro elevation.
///
/// Priority-Flood starts from the artifact boundary and raises enclosed cells
/// only to their spill elevation. Boundary cells may point to a strictly lower
/// cell in a neighboring artifact; those global exits are explicit so artifact
/// generation never depends on which neighboring region was generated first.
#[derive(Clone, Debug, PartialEq)]
pub struct WatershedRegion {
    pub world: WorldIdentity,
    pub index: WatershedRegionIndex,
    cells: Vec<DrainageCell>,
    basins: Vec<DrainageBasin>,
}

impl WatershedRegion {
    pub fn generate(world: WorldIdentity, index: WatershedRegionIndex) -> Option<Self> {
        let cell_count = WATERSHED_REGION_CELLS.checked_mul(WATERSHED_REGION_CELLS)?;
        let terrain = MacroElevation::new(world);
        let mut indices = Vec::with_capacity(cell_count);
        let mut elevations = Vec::with_capacity(cell_count);
        for local_z in 0..WATERSHED_REGION_CELLS {
            for local_x in 0..WATERSHED_REGION_CELLS {
                let cell = index.global_cell(local_x, local_z)?;
                let [x, z] = cell.center();
                indices.push(cell);
                elevations.push(terrain.sample(x, z)?.elevation_meters);
            }
        }

        let (filled, parents, flood_rank) = priority_flood(&elevations);
        let boundary_flows = boundary_outflows(world, index, &indices, &elevations)?;
        let mut flow_to = parents
            .iter()
            .map(|parent| parent.map(|slot| indices[slot]))
            .collect::<Vec<_>>();
        for (slot, external) in boundary_flows {
            flow_to[slot] = external;
        }

        let mut accumulation = vec![1_u64; cell_count];
        let mut slots_by_descending_rank = (0..cell_count).collect::<Vec<_>>();
        slots_by_descending_rank.sort_by_key(|&slot| Reverse(flood_rank[slot]));
        for slot in slots_by_descending_rank {
            if let Some(parent) = parents[slot] {
                accumulation[parent] = accumulation[parent].saturating_add(accumulation[slot]);
            }
        }

        let mut watershed_outlets = vec![DrainageCellIndex::new(0, 0); cell_count];
        let mut slots_by_rank = (0..cell_count).collect::<Vec<_>>();
        slots_by_rank.sort_by_key(|&slot| flood_rank[slot]);
        for slot in slots_by_rank {
            watershed_outlets[slot] = parents[slot].map_or_else(
                || flow_to[slot].unwrap_or(indices[slot]),
                |parent| watershed_outlets[parent],
            );
        }

        let mut cells = indices
            .iter()
            .enumerate()
            .map(|(slot, &cell_index)| DrainageCell {
                index: cell_index,
                elevation_meters: elevations[slot],
                filled_elevation_meters: filled[slot],
                flow_to: flow_to[slot],
                flow_accumulation_cells: accumulation[slot],
                watershed_outlet: watershed_outlets[slot],
                basin: None,
            })
            .collect::<Vec<_>>();
        let basins = identify_basins(world, &mut cells, &parents)?;

        Some(Self {
            world,
            index,
            cells,
            basins,
        })
    }

    pub fn cells(&self) -> &[DrainageCell] {
        &self.cells
    }

    pub fn basins(&self) -> &[DrainageBasin] {
        &self.basins
    }

    pub fn cell(&self, local_x: usize, local_z: usize) -> Option<&DrainageCell> {
        (local_x < WATERSHED_REGION_CELLS && local_z < WATERSHED_REGION_CELLS)
            .then(|| &self.cells[grid_slot(local_x, local_z)])
    }

    pub fn cell_at(&self, x: f64, z: f64) -> Option<&DrainageCell> {
        if WatershedRegionIndex::containing(x, z)? != self.index {
            return None;
        }
        let drainage_cell = CellIndex::containing(x, z, 0, DRAINAGE_CELL_EDGE_METERS)?;
        let region_cells = i64::try_from(WATERSHED_REGION_CELLS).ok()?;
        let local_x = usize::try_from(drainage_cell.x.rem_euclid(region_cells)).ok()?;
        let local_z = usize::try_from(drainage_cell.z.rem_euclid(region_cells)).ok()?;
        self.cell(local_x, local_z)
    }
}

#[derive(Clone, Copy, Debug)]
struct FloodEntry {
    elevation: f64,
    slot: usize,
}

impl PartialEq for FloodEntry {
    fn eq(&self, other: &Self) -> bool {
        self.elevation.to_bits() == other.elevation.to_bits() && self.slot == other.slot
    }
}

impl Eq for FloodEntry {}

impl PartialOrd for FloodEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FloodEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.elevation
            .total_cmp(&other.elevation)
            .then_with(|| self.slot.cmp(&other.slot))
    }
}

fn priority_flood(elevations: &[f64]) -> (Vec<f64>, Vec<Option<usize>>, Vec<usize>) {
    let mut filled = elevations.to_vec();
    let mut parents = vec![None; elevations.len()];
    let mut flood_rank = vec![usize::MAX; elevations.len()];
    let mut queued = vec![false; elevations.len()];
    let mut pending = BinaryHeap::new();

    for local_z in 0..WATERSHED_REGION_CELLS {
        for local_x in 0..WATERSHED_REGION_CELLS {
            if is_boundary(local_x, local_z) {
                let slot = grid_slot(local_x, local_z);
                queued[slot] = true;
                pending.push(Reverse(FloodEntry {
                    elevation: elevations[slot],
                    slot,
                }));
            }
        }
    }

    let mut next_rank = 0;
    while let Some(Reverse(entry)) = pending.pop() {
        flood_rank[entry.slot] = next_rank;
        next_rank += 1;
        let (local_x, local_z) = slot_coordinates(entry.slot);
        for neighbour in neighbour_slots(local_x, local_z) {
            if queued[neighbour] {
                continue;
            }
            queued[neighbour] = true;
            filled[neighbour] = elevations[neighbour].max(filled[entry.slot]);
            parents[neighbour] = Some(entry.slot);
            pending.push(Reverse(FloodEntry {
                elevation: filled[neighbour],
                slot: neighbour,
            }));
        }
    }

    (filled, parents, flood_rank)
}

fn boundary_outflows(
    world: WorldIdentity,
    region: WatershedRegionIndex,
    indices: &[DrainageCellIndex],
    elevations: &[f64],
) -> Option<Vec<(usize, Option<DrainageCellIndex>)>> {
    let terrain = MacroElevation::new(world);
    let mut outflows = Vec::new();
    for local_z in 0..WATERSHED_REGION_CELLS {
        for local_x in 0..WATERSHED_REGION_CELLS {
            if !is_boundary(local_x, local_z) {
                continue;
            }
            let slot = grid_slot(local_x, local_z);
            let cell = indices[slot];
            let mut best = None;
            for z_offset in -1_i64..=1 {
                for x_offset in -1_i64..=1 {
                    if x_offset == 0 && z_offset == 0 {
                        continue;
                    }
                    let candidate = DrainageCellIndex::new(
                        cell.x.checked_add(x_offset)?,
                        cell.z.checked_add(z_offset)?,
                    );
                    if cell_belongs_to_region(region, candidate) {
                        continue;
                    }
                    let [x, z] = candidate.center();
                    let elevation = terrain.sample(x, z)?.elevation_meters;
                    if elevation >= elevations[slot] {
                        continue;
                    }
                    if best.is_none_or(|(best_elevation, best_index): (f64, DrainageCellIndex)| {
                        elevation < best_elevation
                            || (elevation.to_bits() == best_elevation.to_bits()
                                && candidate < best_index)
                    }) {
                        best = Some((elevation, candidate));
                    }
                }
            }
            outflows.push((slot, best.map(|(_, candidate)| candidate)));
        }
    }
    Some(outflows)
}

fn identify_basins(
    world: WorldIdentity,
    cells: &mut [DrainageCell],
    parents: &[Option<usize>],
) -> Option<Vec<DrainageBasin>> {
    let mut assigned = vec![false; cells.len()];
    let mut basins = Vec::new();
    for start in 0..cells.len() {
        if assigned[start] || !cells[start].is_depression() {
            continue;
        }
        let spill_bits = cells[start].filled_elevation_meters.to_bits();
        let mut component = Vec::new();
        let mut pending = VecDeque::from([start]);
        assigned[start] = true;
        while let Some(slot) = pending.pop_front() {
            component.push(slot);
            let (local_x, local_z) = slot_coordinates(slot);
            for neighbour in neighbour_slots(local_x, local_z) {
                if !assigned[neighbour]
                    && cells[neighbour].is_depression()
                    && cells[neighbour].filled_elevation_meters.to_bits() == spill_bits
                {
                    assigned[neighbour] = true;
                    pending.push_back(neighbour);
                }
            }
        }

        let mut in_component = vec![false; cells.len()];
        for &slot in &component {
            in_component[slot] = true;
        }
        let bottom_slot = *component.iter().min_by(|&&left, &&right| {
            cells[left]
                .elevation_meters
                .total_cmp(&cells[right].elevation_meters)
                .then_with(|| cells[left].index.cmp(&cells[right].index))
        })?;
        let outlet = component
            .iter()
            .filter_map(|&slot| {
                let target = cells[slot].flow_to?;
                let leaves_basin = parents[slot].is_none_or(|parent| !in_component[parent]);
                leaves_basin.then_some((cells[slot].index, target))
            })
            .min_by_key(|(source, target)| (*source, *target))
            .map_or(cells[bottom_slot].watershed_outlet, |(_, target)| target);
        let id = CellIndex::new(cells[bottom_slot].index.x, cells[bottom_slot].index.z, 0)
            .generation_key(world, DOMAIN_DRAINAGE_BASIN);
        for &slot in &component {
            cells[slot].basin = Some(id);
        }
        basins.push(DrainageBasin {
            id,
            bottom: cells[bottom_slot].index,
            bottom_elevation_meters: cells[bottom_slot].elevation_meters,
            spill_elevation_meters: cells[start].filled_elevation_meters,
            outlet,
            cell_count: u64::try_from(component.len()).ok()?,
        });
    }
    basins.sort_by_key(|basin| basin.id);
    Some(basins)
}

fn cell_belongs_to_region(region: WatershedRegionIndex, cell: DrainageCellIndex) -> bool {
    let cells = i64::try_from(WATERSHED_REGION_CELLS).expect("region dimensions fit i64");
    cell.x.div_euclid(cells) == region.x && cell.z.div_euclid(cells) == region.z
}

fn grid_slot(local_x: usize, local_z: usize) -> usize {
    (local_z * WATERSHED_REGION_CELLS) + local_x
}

fn slot_coordinates(slot: usize) -> (usize, usize) {
    (slot % WATERSHED_REGION_CELLS, slot / WATERSHED_REGION_CELLS)
}

fn is_boundary(local_x: usize, local_z: usize) -> bool {
    local_x == 0
        || local_z == 0
        || local_x + 1 == WATERSHED_REGION_CELLS
        || local_z + 1 == WATERSHED_REGION_CELLS
}

fn neighbour_slots(local_x: usize, local_z: usize) -> impl Iterator<Item = usize> {
    let min_x = local_x.saturating_sub(1);
    let max_x = local_x.saturating_add(1).min(WATERSHED_REGION_CELLS - 1);
    let min_z = local_z.saturating_sub(1);
    let max_z = local_z.saturating_add(1).min(WATERSHED_REGION_CELLS - 1);
    (min_z..=max_z).flat_map(move |z| {
        (min_x..=max_x)
            .filter(move |&x| x != local_x || z != local_z)
            .map(move |x| grid_slot(x, z))
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MountainRidge {
    start_x: f64,
    start_z: f64,
    end_x: f64,
    end_z: f64,
    width_meters: f64,
    peak_uplift_meters: f64,
}

impl MountainRidge {
    fn from_cell(world: WorldIdentity, cell: CellIndex) -> Option<Self> {
        let key = cell.generation_key(world, DOMAIN_MOUNTAIN_RIDGE);
        let center_x = (index_as_f64(cell.x) + 0.5) * MACRO_CELL_EDGE_METERS;
        let center_z = (index_as_f64(cell.z) + 0.5) * MACRO_CELL_EDGE_METERS;
        let jitter_x = (hash_fraction(key, 0) - 0.5) * MACRO_CELL_EDGE_METERS * 0.4;
        let jitter_z = (hash_fraction(key, 1) - 0.5) * MACRO_CELL_EDGE_METERS * 0.4;
        let half_length = 24_000.0 + (hash_fraction(key, 2) * 20_000.0);
        let width_meters = 5_000.0 + (hash_fraction(key, 3) * 9_000.0);
        let directions = [
            (1.0, 0.0),
            (0.0, 1.0),
            (1.0, 1.0),
            (1.0, -1.0),
            (2.0, 1.0),
            (2.0, -1.0),
            (1.0, 2.0),
            (1.0, -2.0),
        ];
        let direction_index = usize::try_from((key >> 32) & 7).ok()?;
        let (direction_x, direction_z) = directions[direction_index];
        let direction_length = f64::sqrt((direction_x * direction_x) + (direction_z * direction_z));
        let extent_x = direction_x * half_length / direction_length;
        let extent_z = direction_z * half_length / direction_length;
        let center_x = center_x + jitter_x;
        let center_z = center_z + jitter_z;
        let uplift = corner(world, DOMAIN_UPLIFT, cell.x, cell.z);
        let erosion_age = corner(world, DOMAIN_EROSION, cell.x, cell.z);
        let youth = 1.0 - erosion_age;
        let peak_uplift_meters = 220.0 + (1_180.0 * uplift * (0.55 + (youth * 0.45)));

        Some(Self {
            start_x: center_x - extent_x,
            start_z: center_z - extent_z,
            end_x: center_x + extent_x,
            end_z: center_z + extent_z,
            width_meters,
            peak_uplift_meters,
        })
    }

    fn uplift_at(self, x: f64, z: f64) -> f64 {
        let segment_x = self.end_x - self.start_x;
        let segment_z = self.end_z - self.start_z;
        let segment_length_squared = (segment_x * segment_x) + (segment_z * segment_z);
        let projection = (((x - self.start_x) * segment_x) + ((z - self.start_z) * segment_z))
            / segment_length_squared;
        let projection = projection.clamp(0.0, 1.0);
        let nearest_x = self.start_x + (segment_x * projection);
        let nearest_z = self.start_z + (segment_z * projection);
        let distance_squared =
            ((x - nearest_x) * (x - nearest_x)) + ((z - nearest_z) * (z - nearest_z));
        let normalized_distance_squared =
            distance_squared / (self.width_meters * self.width_meters);
        if normalized_distance_squared >= 1.0 {
            return 0.0;
        }
        let ridge = 1.0 - normalized_distance_squared;
        self.peak_uplift_meters * ridge * ridge
    }
}

fn hash_fraction(key: u64, lane: u64) -> f64 {
    let mixed = stable_hash(&[key, lane, DOMAIN_MOUNTAIN_RIDGE]);
    hash53_as_f64(mixed >> 11) / 9_007_199_254_740_991.0
}

fn temperature_controls(
    world: WorldIdentity,
    profile: RegionalProfile,
    x: f64,
    z: f64,
    elevation_meters: f64,
) -> Option<TemperatureControls> {
    if world.generator_version < SEASONAL_CLIMATE_GENERATOR_VERSION {
        let baseline_temperature_celsius = -6.0 + (profile.mean_temperature * 34.0);
        return Some(TemperatureControls {
            latitude_warmth_fraction: profile.mean_temperature,
            ocean_proximity_fraction: 0.0,
            continentality_fraction: 1.0,
            latitude_temperature_celsius: baseline_temperature_celsius,
            regional_temperature_anomaly_celsius: 0.0,
            continentality_adjustment_celsius: 0.0,
            baseline_temperature_celsius,
            seasonal_temperature_amplitude_celsius: 0.0,
            precipitation_seasonality_fraction: 0.0,
        });
    }

    let latitude_warmth_fraction = latitude_warmth(z)?;
    let ocean_proximity_fraction = ocean_proximity(world, x, z, elevation_meters)?;
    let continentality_fraction = 1.0 - ocean_proximity_fraction;
    let latitude_temperature_celsius = -14.0 + (latitude_warmth_fraction * 36.0);
    let regional_temperature_anomaly_celsius = (profile.mean_temperature - 0.5) * 12.0;
    let continentality_adjustment_celsius = -3.0 * continentality_fraction;
    let baseline_temperature_celsius = latitude_temperature_celsius
        + regional_temperature_anomaly_celsius
        + continentality_adjustment_celsius;
    Some(TemperatureControls {
        latitude_warmth_fraction,
        ocean_proximity_fraction,
        continentality_fraction,
        latitude_temperature_celsius,
        regional_temperature_anomaly_celsius,
        continentality_adjustment_celsius,
        baseline_temperature_celsius,
        seasonal_temperature_amplitude_celsius: 5.0
            + (continentality_fraction * 18.0)
            + ((1.0 - latitude_warmth_fraction) * 4.0),
        precipitation_seasonality_fraction: 0.05 + (continentality_fraction * 0.25),
    })
}

fn latitude_warmth(z: f64) -> Option<f64> {
    if !z.is_finite() {
        return None;
    }
    let phase = (z / LATITUDE_HALF_CYCLE_METERS).rem_euclid(2.0);
    let triangular_warmth = 1.0 - (phase - 1.0).abs();
    Some(smoothstep(triangular_warmth))
}

fn ocean_proximity(
    world: WorldIdentity,
    x: f64,
    z: f64,
    local_elevation_meters: f64,
) -> Option<f64> {
    let mut proximity = ocean_likeness(local_elevation_meters);
    let terrain = MacroElevation::new(world);
    for distance in OCEAN_SAMPLE_DISTANCES_METERS {
        let distance_weight =
            1.0 - (0.65 * (distance / OCEAN_SEARCH_RADIUS_METERS).clamp(0.0, 1.0));
        for [direction_x, direction_z] in OCEAN_SAMPLE_DIRECTIONS {
            let elevation = terrain
                .sample(x + (direction_x * distance), z + (direction_z * distance))?
                .elevation_meters;
            proximity = proximity.max(ocean_likeness(elevation) * distance_weight);
        }
    }
    Some(proximity.clamp(0.0, 1.0))
}

fn ocean_likeness(elevation_meters: f64) -> f64 {
    ((60.0 - elevation_meters) / 100.0).clamp(0.0, 1.0)
}

fn seasonal_temperature(annual_mean_celsius: f64, amplitude_celsius: f64, season: Season) -> f64 {
    annual_mean_celsius + (amplitude_celsius * season.temperature_factor())
}

fn snow_fraction(temperature_celsius: f64) -> f64 {
    ((2.0 - temperature_celsius) / 4.0).clamp(0.0, 1.0)
}

fn snow_cycle(
    annual_mean_temperature_celsius: f64,
    seasonal_temperature_amplitude_celsius: f64,
    annual_precipitation_millimeters: f64,
    precipitation_seasonality_fraction: f64,
) -> SnowCycle {
    let precipitation_weights = [
        0.25 + (precipitation_seasonality_fraction * 0.5),
        0.25,
        0.25 - (precipitation_seasonality_fraction * 0.5),
        0.25,
    ];
    let mut cycle = SnowCycle::default();
    let mut temperatures = [0.0; 4];
    for season in Season::ALL {
        let index = season.index();
        temperatures[index] = seasonal_temperature(
            annual_mean_temperature_celsius,
            seasonal_temperature_amplitude_celsius,
            season,
        );
        cycle.precipitation[index] =
            annual_precipitation_millimeters * precipitation_weights[index];
        cycle.snowfall[index] = cycle.precipitation[index] * snow_fraction(temperatures[index]);
        cycle.rainfall[index] = cycle.precipitation[index] - cycle.snowfall[index];
        cycle.annual_snowfall += cycle.snowfall[index];
    }

    let warmest_temperature = temperatures.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let permanent_fraction = snow_fraction(warmest_temperature);
    cycle.permanent_snowpack = cycle.annual_snowfall * permanent_fraction * 2.0;
    let meltable_fraction = 1.0 - permanent_fraction;
    let spring_melt_weight = (temperatures[Season::Spring.index()] + 2.0).max(0.0);
    let summer_melt_weight = (temperatures[Season::Summer.index()] + 2.0).max(0.0);
    let total_melt_weight = spring_melt_weight + summer_melt_weight;
    let meltable_snowfall = cycle.annual_snowfall * meltable_fraction;
    let spring_melt_target = if total_melt_weight > 0.0 {
        meltable_snowfall * spring_melt_weight / total_melt_weight
    } else {
        0.0
    };

    let mut snowpack = cycle.permanent_snowpack;
    let mut maximum_snowpack = snowpack;
    for season in [
        Season::Autumn,
        Season::Winter,
        Season::Spring,
        Season::Summer,
    ] {
        let index = season.index();
        snowpack += cycle.snowfall[index] * meltable_fraction;
        maximum_snowpack = maximum_snowpack.max(snowpack);
        let available_to_melt = (snowpack - cycle.permanent_snowpack).max(0.0);
        let melt = match season {
            Season::Spring => spring_melt_target.min(available_to_melt),
            Season::Summer => available_to_melt,
            Season::Winter | Season::Autumn => 0.0,
        };
        snowpack -= melt;
        cycle.snowmelt[index] = melt;
        cycle.snowpack[index] = snowpack;
        cycle.annual_snowmelt += melt;
    }
    cycle.maximum_snowpack = maximum_snowpack;
    cycle
}

fn prevailing_wind(world: WorldIdentity, x: f64, z: f64) -> Option<[f64; 2]> {
    let wind_x = (value_field(world, DOMAIN_WIND_X, x, z, WIND_CELL_EDGE_METERS)? * 2.0) - 1.0;
    let wind_z = (value_field(world, DOMAIN_WIND_Z, x, z, WIND_CELL_EDGE_METERS)? * 2.0) - 1.0;
    // Keep these operations separate and ordered. `f64::hypot` may use
    // platform-specific implementations whose last-bit rounding differs.
    let length = f64::sqrt((wind_x * wind_x) + (wind_z * wind_z));
    if length <= 0.000_001 {
        return Some([1.0, 0.0]);
    }
    Some([wind_x / length, wind_z / length])
}

fn value_field(world: WorldIdentity, domain: u64, x: f64, z: f64, edge: f64) -> Option<f64> {
    let cell = CellIndex::containing(x, z, 0, edge)?;
    let local_x = (x / edge) - index_as_f64(cell.x);
    let local_z = (z / edge) - index_as_f64(cell.z);
    let blend_x = smoothstep(local_x);
    let blend_z = smoothstep(local_z);

    let bottom_left = corner(world, domain, cell.x, cell.z);
    let bottom_right = corner(world, domain, cell.x + 1, cell.z);
    let top_left = corner(world, domain, cell.x, cell.z + 1);
    let top_right = corner(world, domain, cell.x + 1, cell.z + 1);

    let bottom = lerp(bottom_left, bottom_right, blend_x);
    let top = lerp(top_left, top_right, blend_x);
    Some(lerp(bottom, top, blend_z))
}

fn corner(world: WorldIdentity, domain: u64, x: i64, z: i64) -> f64 {
    let hash = CellIndex::new(x, z, 0).generation_key(world, domain);
    // Using the high 53 bits maps exactly into the precision available in f64.
    hash53_as_f64(hash >> 11) / 9_007_199_254_740_991.0
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: i64) -> f64 {
    index as f64
}

#[allow(clippy::cast_precision_loss)]
fn hash53_as_f64(hash: u64) -> f64 {
    hash as f64
}

fn smoothstep(value: f64) -> f64 {
    value * value * (3.0 - (2.0 * value))
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, 1, 0);

    #[test]
    fn samples_are_deterministic() {
        let first = RegionalProfile::sample(WORLD, 42_000.0, -17_000.0);
        let second = RegionalProfile::sample(WORLD, 42_000.0, -17_000.0);
        assert_eq!(first, second);
    }

    #[test]
    fn all_profile_values_are_normalized() {
        let profile = RegionalProfile::sample(WORLD, -42_000.0, 317_000.0)
            .expect("finite coordinates should sample");
        for value in [
            profile.uplift,
            profile.erosion_age,
            profile.rock_hardness,
            profile.precipitation,
            profile.mean_temperature,
            profile.karst_probability,
        ] {
            assert!((0.0..=1.0).contains(&value));
        }
    }

    #[test]
    fn a_shared_corner_has_one_value_from_every_neighbor() {
        let sampled =
            value_field(WORLD, DOMAIN_RAIN, 100_000.0, -200_000.0, 100_000.0).expect("finite");
        let shared_corner = corner(WORLD, DOMAIN_RAIN, 1, -2);
        assert!((sampled - shared_corner).abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_positions_do_not_generate_profiles() {
        assert!(RegionalProfile::sample(WORLD, f64::INFINITY, 0.0).is_none());
    }

    #[test]
    fn climate_is_deterministic_and_explainable() {
        let climate = Climate::new(WorldIdentity::new(
            0x5eed,
            OROGRAPHIC_CLIMATE_GENERATOR_VERSION,
            0,
        ));
        let first = climate.sample(-73_125.0, 19_875.0).expect("finite climate");
        let second = climate.sample(-73_125.0, 19_875.0).expect("same climate");

        assert_eq!(first, second);
        assert_eq!(
            first.mean_temperature_celsius.to_bits(),
            (first.baseline_temperature_celsius - first.elevation_cooling_celsius).to_bits()
        );
        let precipitation_multiplier = first.annual_precipitation_millimeters
            / first.baseline_annual_precipitation_millimeters;
        assert!((0.2..=1.8).contains(&precipitation_multiplier));
        assert!((first.prevailing_wind[0].hypot(first.prevailing_wind[1]) - 1.0).abs() < 1.0e-12);
        assert!((0.0..=1.0).contains(&first.precipitation_fraction()));
        assert!((0.0..=1.0).contains(&first.warmth_fraction()));
    }

    #[test]
    fn climate_has_continuous_negative_coordinate_boundaries() {
        let climate = Climate::new(WorldIdentity::new(
            0x5eed,
            OROGRAPHIC_CLIMATE_GENERATOR_VERSION,
            0,
        ));
        let left = climate
            .sample(-CLIMATE_CELL_EDGE_METERS - 0.01, -12_000.0)
            .expect("left climate");
        let right = climate
            .sample(-CLIMATE_CELL_EDGE_METERS + 0.01, -12_000.0)
            .expect("right climate");

        assert!((left.mean_temperature_celsius - right.mean_temperature_celsius).abs() < 0.1);
        assert!(
            (left.annual_precipitation_millimeters - right.annual_precipitation_millimeters).abs()
                < 1.0
        );
    }

    #[test]
    fn mountains_create_windward_lift_and_rain_shadows() {
        let climate = Climate::new(WorldIdentity::new(
            0x5eed,
            OROGRAPHIC_CLIMATE_GENERATOR_VERSION,
            0,
        ));
        let mut maximum_lift = 0.0_f64;
        let mut maximum_shadow = 0.0_f64;
        for z in -12..=12 {
            for x in -12..=12 {
                let sample = climate
                    .sample(f64::from(x) * 8_000.0, f64::from(z) * 8_000.0)
                    .expect("finite climate");
                maximum_lift = maximum_lift.max(sample.orographic_lift_meters);
                maximum_shadow = maximum_shadow.max(sample.rain_shadow_meters);
                if sample.orographic_lift_meters > 0.0 {
                    assert!(
                        sample.annual_precipitation_millimeters
                            >= sample.baseline_annual_precipitation_millimeters
                                * (1.0
                                    - (0.75
                                        * (sample.rain_shadow_meters / 1_200.0).clamp(0.0, 1.0)))
                    );
                }
            }
        }

        assert!(maximum_lift > 200.0, "{maximum_lift}");
        assert!(maximum_shadow > 200.0, "{maximum_shadow}");
    }

    #[test]
    fn climate_has_a_golden_fingerprint() {
        let climate = Climate::new(WorldIdentity::new(
            0x5eed,
            OROGRAPHIC_CLIMATE_GENERATOR_VERSION,
            0,
        ));
        let samples = [
            (-91_000.0, -37_000.0),
            (-64_000.0, 64_000.0),
            (-1.0, 1.0),
            (28_000.0, -52_000.0),
            (117_000.0, 83_000.0),
        ];
        let fingerprint = stable_hash(
            &samples
                .into_iter()
                .flat_map(|(x, z)| {
                    let sample = climate.sample(x, z).expect("finite");
                    [
                        sample.mean_temperature_celsius.to_bits(),
                        sample.annual_precipitation_millimeters.to_bits(),
                        sample.prevailing_wind[0].to_bits(),
                        sample.prevailing_wind[1].to_bits(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            fingerprint, 3_353_464_691_744_025_732,
            "changing this value changes generated orographic climate"
        );
    }

    #[test]
    fn seasonal_climate_is_explicit_deterministic_and_water_balanced() {
        let climate = Climate::new(WorldIdentity::new(
            0x5eed,
            SEASONAL_CLIMATE_GENERATOR_VERSION,
            0,
        ));
        let annual = climate.sample(-73_125.0, 19_875.0).expect("annual climate");
        let first = Season::ALL.map(|season| {
            climate
                .sample_season(-73_125.0, 19_875.0, season)
                .expect("seasonal climate")
        });
        let second = Season::ALL.map(|season| {
            climate
                .sample_season(-73_125.0, 19_875.0, season)
                .expect("same seasonal climate")
        });

        assert_eq!(first, second);
        let precipitation = first
            .iter()
            .map(|sample| sample.precipitation_millimeters)
            .sum::<f64>();
        let snowfall = first
            .iter()
            .map(|sample| sample.snowfall_water_equivalent_millimeters)
            .sum::<f64>();
        let snowmelt = first
            .iter()
            .map(|sample| sample.snowmelt_millimeters)
            .sum::<f64>();
        assert!((precipitation - annual.annual_precipitation_millimeters).abs() < 1.0e-9);
        assert!((snowfall - annual.annual_snowfall_water_equivalent_millimeters).abs() < 1.0e-9);
        assert!((snowmelt - annual.annual_snowmelt_millimeters).abs() < 1.0e-9);
        for sample in first {
            assert!(
                (sample.rainfall_millimeters + sample.snowfall_water_equivalent_millimeters
                    - sample.precipitation_millimeters)
                    .abs()
                    < 1.0e-9
            );
            assert!(sample.snowpack_water_equivalent_millimeters >= 0.0);
        }
    }

    #[test]
    fn latitude_and_continentality_have_explainable_climate_effects() {
        let climate = Climate::new(WorldIdentity::new(
            0x5eed,
            SEASONAL_CLIMATE_GENERATOR_VERSION,
            0,
        ));
        let cold_band = climate.sample(0.0, 0.0).expect("cold latitude band");
        let warm_band = climate
            .sample(0.0, LATITUDE_HALF_CYCLE_METERS)
            .expect("warm latitude band");

        assert!(cold_band.latitude_temperature_celsius < warm_band.latitude_temperature_celsius);
        for sample in [cold_band, warm_band] {
            assert_eq!(
                sample.baseline_temperature_celsius.to_bits(),
                (sample.latitude_temperature_celsius
                    + sample.regional_temperature_anomaly_celsius
                    + sample.continentality_adjustment_celsius)
                    .to_bits()
            );
            assert!(
                (sample.continentality_fraction + sample.ocean_proximity_fraction - 1.0).abs()
                    < f64::EPSILON
            );
            assert_eq!(
                sample.seasonal_temperature_amplitude_celsius.to_bits(),
                (5.0 + (sample.continentality_fraction * 18.0)
                    + ((1.0 - sample.latitude_warmth_fraction) * 4.0))
                    .to_bits()
            );
        }
    }

    #[test]
    fn seasonal_climate_is_continuous_across_negative_latitude_boundaries() {
        let climate = Climate::new(WorldIdentity::new(
            0x5eed,
            SEASONAL_CLIMATE_GENERATOR_VERSION,
            0,
        ));
        let south = climate.sample(-12_000.0, -0.01).expect("south climate");
        let north = climate.sample(-12_000.0, 0.01).expect("north climate");

        assert!((south.latitude_warmth_fraction - north.latitude_warmth_fraction).abs() < 1.0e-9);
        assert!((south.mean_temperature_celsius - north.mean_temperature_celsius).abs() < 0.1);
        assert!(
            (south.maximum_snowpack_water_equivalent_millimeters
                - north.maximum_snowpack_water_equivalent_millimeters)
                .abs()
                < 1.0
        );
    }

    #[test]
    fn seasonal_climate_has_a_golden_fingerprint() {
        let climate = Climate::new(WorldIdentity::new(
            0x5eed,
            SEASONAL_CLIMATE_GENERATOR_VERSION,
            0,
        ));
        let samples = [
            (-1_891_000.0, -2_037_000.0),
            (-764_000.0, -936_000.0),
            (-1.0, 1.0),
            (828_000.0, 1_052_000.0),
            (2_117_000.0, 2_083_000.0),
        ];
        let fingerprint = stable_hash(
            &samples
                .into_iter()
                .flat_map(|(x, z)| {
                    let annual = climate.sample(x, z).expect("finite");
                    let winter = climate.sample_season(x, z, Season::Winter).expect("winter");
                    let summer = climate.sample_season(x, z, Season::Summer).expect("summer");
                    [
                        annual.mean_temperature_celsius.to_bits(),
                        annual.annual_precipitation_millimeters.to_bits(),
                        annual.ocean_proximity_fraction.to_bits(),
                        annual
                            .maximum_snowpack_water_equivalent_millimeters
                            .to_bits(),
                        winter.mean_temperature_celsius.to_bits(),
                        winter.snowpack_water_equivalent_millimeters.to_bits(),
                        summer.mean_temperature_celsius.to_bits(),
                        summer.snowmelt_millimeters.to_bits(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            fingerprint, 16_179_459_295_312_885_200,
            "changing this value changes generated seasonal climate"
        );
    }

    #[test]
    fn macro_elevation_is_deterministic_across_negative_coordinates() {
        let terrain = MacroElevation::new(WORLD);
        let first = terrain.sample(-73_125.0, 19_875.0).expect("finite");
        let second = terrain.sample(-73_125.0, 19_875.0).expect("finite");
        assert_eq!(first, second);
        assert!(first.elevation_meters.is_finite());
    }

    #[test]
    fn macro_elevation_has_no_cell_boundary_jump() {
        let terrain = MacroElevation::new(WORLD);
        let left = terrain
            .sample(MACRO_CELL_EDGE_METERS - 0.01, -12_000.0)
            .expect("finite");
        let right = terrain
            .sample(MACRO_CELL_EDGE_METERS + 0.01, -12_000.0)
            .expect("finite");
        assert!((left.elevation_meters - right.elevation_meters).abs() < 0.1);
    }

    #[test]
    fn macro_elevation_contains_mountain_scale_relief() {
        let terrain = MacroElevation::new(WORLD);
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for z in -8..=8 {
            for x in -8..=8 {
                let sample = terrain
                    .sample(f64::from(x) * 8_000.0, f64::from(z) * 8_000.0)
                    .expect("finite");
                minimum = minimum.min(sample.elevation_meters);
                maximum = maximum.max(sample.elevation_meters);
            }
        }
        assert!(maximum - minimum > 500.0);
    }

    #[test]
    fn macro_elevation_has_a_golden_fingerprint() {
        let terrain = MacroElevation::new(WORLD);
        let samples = [
            (-91_000.0, -37_000.0),
            (-64_000.0, 64_000.0),
            (-1.0, 1.0),
            (28_000.0, -52_000.0),
            (117_000.0, 83_000.0),
        ];
        let fingerprint = stable_hash(&samples.map(|(x, z)| {
            terrain
                .sample(x, z)
                .expect("finite")
                .elevation_meters
                .to_bits()
        }));
        assert_eq!(
            fingerprint, 737_748_240_385_137_715,
            "changing this value changes generated macro terrain"
        );
    }

    #[test]
    fn starter_landscape_has_a_mountain_destination_within_twenty_kilometers() {
        let terrain = MacroElevation::new(WorldIdentity::new(0x5eed, 2, 0));
        let mut maximum_uplift = 0.0_f64;
        for z in -10..=10 {
            for x in -10..=10 {
                let sample = terrain
                    .sample(f64::from(x) * 2_000.0, f64::from(z) * 2_000.0)
                    .expect("finite");
                maximum_uplift = maximum_uplift.max(sample.mountain_uplift_meters);
            }
        }
        assert!(maximum_uplift > 500.0);
    }

    #[test]
    fn sampling_order_does_not_change_macro_terrain() {
        let terrain = MacroElevation::new(WORLD);
        let positions = [
            (-12_000.0, 4_000.0),
            (70_000.0, -90_000.0),
            (-140_000.0, 31_000.0),
        ];
        let forward = positions.map(|(x, z)| terrain.sample(x, z).expect("finite"));
        let mut reverse_positions = positions;
        reverse_positions.reverse();
        let mut reverse = reverse_positions.map(|(x, z)| terrain.sample(x, z).expect("finite"));
        reverse.reverse();
        assert_eq!(forward, reverse);
    }

    #[test]
    fn drainage_cells_and_regions_share_negative_half_open_boundaries() {
        let cell = DrainageCellIndex::containing(-0.01, -128_000.0).expect("drainage cell");
        assert_eq!(cell, DrainageCellIndex::new(-1, -64));
        assert_eq!(
            WatershedRegionIndex::containing_cell(cell),
            WatershedRegionIndex::new(-1, -1)
        );
        assert_eq!(
            WatershedRegionIndex::containing(0.0, 128_000.0),
            Some(WatershedRegionIndex::new(0, 1))
        );
    }

    #[test]
    fn watershed_regions_are_deterministic_and_order_independent() {
        let first_index = WatershedRegionIndex::new(-2, 3);
        let second_index = WatershedRegionIndex::new(-1, 3);
        let first = WatershedRegion::generate(WORLD, first_index).expect("valid region");
        let second = WatershedRegion::generate(WORLD, second_index).expect("valid region");
        let second_again = WatershedRegion::generate(WORLD, second_index).expect("valid region");
        let first_again = WatershedRegion::generate(WORLD, first_index).expect("valid region");

        assert_eq!(first, first_again);
        assert_eq!(second, second_again);
    }

    #[test]
    fn every_drainage_cell_has_complete_catchment_ownership() {
        let region =
            WatershedRegion::generate(WORLD, WatershedRegionIndex::new(0, 0)).expect("region");

        assert_eq!(
            region.cells().len(),
            WATERSHED_REGION_CELLS * WATERSHED_REGION_CELLS
        );
        for cell in region.cells() {
            assert!(cell.flow_accumulation_cells >= 1);
            let outlet = cell.watershed_outlet;
            let region_cells = i64::try_from(WATERSHED_REGION_CELLS).expect("dimensions fit");
            let min_x = region.index.x * region_cells;
            let min_z = region.index.z * region_cells;
            let max_x = min_x + region_cells - 1;
            let max_z = min_z + region_cells - 1;
            if cell_belongs_to_region(region.index, outlet) {
                let local_x = usize::try_from(outlet.x.rem_euclid(region_cells)).expect("local x");
                let local_z = usize::try_from(outlet.z.rem_euclid(region_cells)).expect("local z");
                assert!(is_boundary(local_x, local_z));
            } else {
                assert!((min_x - 1..=max_x + 1).contains(&outlet.x));
                assert!((min_z - 1..=max_z + 1).contains(&outlet.z));
            }
        }
    }

    #[test]
    fn drainage_flows_downhill_on_the_filled_surface() {
        let region =
            WatershedRegion::generate(WORLD, WatershedRegionIndex::new(-1, -1)).expect("region");
        for cell in region.cells() {
            let Some(target) = cell.flow_to else {
                continue;
            };
            assert!(
                cell.index.x.abs_diff(target.x) <= 1 && cell.index.z.abs_diff(target.z) <= 1,
                "drainage may only enter a neighboring cell"
            );
            if cell_belongs_to_region(region.index, target) {
                let cells = i64::try_from(WATERSHED_REGION_CELLS).expect("dimensions fit");
                let local_x = usize::try_from(target.x.rem_euclid(cells)).expect("local x");
                let local_z = usize::try_from(target.z.rem_euclid(cells)).expect("local z");
                let target = region.cell(local_x, local_z).expect("target cell");
                assert!(
                    target.filled_elevation_meters <= cell.filled_elevation_meters,
                    "flow cannot climb the depression-filled surface"
                );
            } else {
                let [x, z] = target.center();
                let target_elevation = MacroElevation::new(WORLD)
                    .sample(x, z)
                    .expect("external target")
                    .elevation_meters;
                assert!(
                    target_elevation < cell.elevation_meters,
                    "cross-region flow requires a strictly lower neighbor"
                );
            }
        }
    }

    #[test]
    fn basins_are_level_and_spill_above_their_bottoms() {
        let region =
            WatershedRegion::generate(WORLD, WatershedRegionIndex::new(0, 0)).expect("region");
        assert!(
            !region.basins().is_empty(),
            "the golden region contains basins"
        );
        for basin in region.basins() {
            assert!(basin.spill_elevation_meters > basin.bottom_elevation_meters);
            assert!(basin.cell_count > 0);
            for cell in region
                .cells()
                .iter()
                .filter(|cell| cell.basin == Some(basin.id))
            {
                assert_eq!(
                    cell.filled_elevation_meters.to_bits(),
                    basin.spill_elevation_meters.to_bits()
                );
            }
        }
    }

    #[test]
    fn negative_region_boundaries_use_global_half_open_cells() {
        let left =
            WatershedRegion::generate(WORLD, WatershedRegionIndex::new(-1, -1)).expect("left");
        let right =
            WatershedRegion::generate(WORLD, WatershedRegionIndex::new(0, -1)).expect("right");

        assert_eq!(
            left.cell_at(-0.01, -0.01)
                .expect("left boundary cell")
                .index,
            DrainageCellIndex::new(-1, -1)
        );
        assert_eq!(
            right
                .cell_at(0.0, -0.01)
                .expect("right boundary cell")
                .index,
            DrainageCellIndex::new(0, -1)
        );
    }

    #[test]
    fn watershed_region_has_a_golden_fingerprint() {
        let region =
            WatershedRegion::generate(WORLD, WatershedRegionIndex::new(-1, 2)).expect("region");
        let mut words = Vec::new();
        for cell in region.cells().iter().step_by(97) {
            words.extend([
                u64::from_le_bytes(cell.index.x.to_le_bytes()),
                u64::from_le_bytes(cell.index.z.to_le_bytes()),
                cell.elevation_meters.to_bits(),
                cell.filled_elevation_meters.to_bits(),
                cell.flow_accumulation_cells,
                u64::from_le_bytes(cell.watershed_outlet.x.to_le_bytes()),
                u64::from_le_bytes(cell.watershed_outlet.z.to_le_bytes()),
            ]);
        }
        assert_eq!(
            stable_hash(&words),
            4_057_820_053_250_262_551,
            "changing this value changes generated regional drainage"
        );
    }
}
