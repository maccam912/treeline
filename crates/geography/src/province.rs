//! Deterministic top-down geographical province planning.
//!
//! A province is a bounded, reproducible description of the causes that shape
//! hundreds of kilometers of world. The artifact owns explicit parent-scale
//! boundary conditions while its feature evaluators inspect a fixed halo of
//! neighboring owners. Sampling is therefore independent of visitation order,
//! cache state, and which side of a province boundary requested the value.

use std::cell::RefCell;
use std::collections::VecDeque;

use treeline_coordinates::{CellIndex, WorldIdentity, stable_hash};

/// Generator contract that introduces geographical provinces and landscape diversity.
pub const PROVINCE_GENERATOR_VERSION: u32 = 18;
/// Horizontal edge of one bounded geographical province.
pub const PROVINCE_EDGE_METERS: f64 = 512_000.0;
/// Fixed feature-ownership halo evaluated around every province.
pub const PROVINCE_HALO_METERS: f64 = 128_000.0;

const PARENT_EDGE_METERS: f64 = 2_048_000.0;
const CONTINENT_EDGE_METERS: f64 = 1_024_000.0;
const REGIONAL_EDGE_METERS: f64 = 256_000.0;

const DOMAIN_PLAN: u64 = 0x5052_4f56_504c_414e;
const DOMAIN_PARENT: u64 = 0x5052_4f56_5041_524e;
const DOMAIN_CONTINENT: u64 = 0x434f_4e54_494e_454e;
const DOMAIN_LAND_DETAIL: u64 = 0x4c41_4e44_4445_544c;
const DOMAIN_CRUST_AGE: u64 = 0x4352_5553_545f_4147;
const DOMAIN_ROCK_HARDNESS: u64 = 0x524f_434b_4841_5244;
const DOMAIN_CARBONATE: u64 = 0x4341_5242_4f4e_4154;
const DOMAIN_UPLIFT: u64 = 0x5052_4f56_5550_4c46;
const DOMAIN_STRATA: u64 = 0x5354_5241_5441_544c;
const DOMAIN_VOLCANISM: u64 = 0x564f_4c43_414e_4953;
const DOMAIN_GLACIATION: u64 = 0x474c_4143_4941_5445;
const DOMAIN_EROSION: u64 = 0x5052_4f56_4552_4f53;
const DOMAIN_SEDIMENT: u64 = 0x5345_4449_4d45_4e54;
const DOMAIN_DRAINAGE: u64 = 0x4452_4149_4e41_4745;
const DOMAIN_MOISTURE: u64 = 0x4d4f_4953_5455_5245;
const DOMAIN_TEMPERATURE: u64 = 0x5445_4d50_4552_4154;
const DOMAIN_ARIDITY: u64 = 0x4152_4944_4954_595f;
const DOMAIN_BASIN: u64 = 0x434c_4f53_4544_4241;
const DOMAIN_DISTURBANCE: u64 = 0x4449_5354_5552_4241;
const DOMAIN_ECOLOGICAL_MEMORY: u64 = 0x4543_4f4c_4f47_4d45;
const DOMAIN_TECTONIC_FEATURE: u64 = 0x5445_4354_4f4e_4943;
const DOMAIN_SCARP_FEATURE: u64 = 0x5343_4152_505f_4645;
const DOMAIN_DUNE_DIRECTION: u64 = 0x4455_4e45_4449_5245;
// Exact-coordinate memoization only: eviction and visitation order cannot
// change generated values.
const PROVINCE_SAMPLE_CACHE_ENTRIES: usize = 256;

/// Offline-calibratable coefficients for version-18+ province morphology.
///
/// Production generation always uses [`Self::VERSION_18`]. Explicit values are
/// accepted only by inspection and calibration entry points, so an optimizer
/// cannot change an existing world contract or contaminate the normal cache.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProvinceParameters {
    pub base_elevation_meters: f64,
    pub continental_relief_meters: f64,
    pub boundary_uplift_meters: f64,
    pub closed_basin_base_depth_meters: f64,
    pub closed_basin_depth_range_meters: f64,
    pub tectonic_half_length_base_meters: f64,
    pub tectonic_half_length_range_meters: f64,
    pub tectonic_width_base_meters: f64,
    pub tectonic_width_range_meters: f64,
    pub tectonic_peak_base_meters: f64,
    pub tectonic_peak_range_meters: f64,
    pub glacial_valley_base_depth_meters: f64,
    pub glacial_valley_depth_range_meters: f64,
    pub volcanic_relief_meters: f64,
    pub plateau_base_relief_meters: f64,
    pub plateau_relief_range_meters: f64,
    pub scarp_step_base_meters: f64,
    pub scarp_step_range_meters: f64,
    pub dune_base_amplitude_meters: f64,
    pub dune_sediment_amplitude_meters: f64,
    pub dune_aridity_amplitude_meters: f64,
}

impl ProvinceParameters {
    pub const VERSION_18: Self = Self {
        base_elevation_meters: -640.0,
        continental_relief_meters: 1_280.0,
        boundary_uplift_meters: 170.0,
        closed_basin_base_depth_meters: 90.0,
        closed_basin_depth_range_meters: 240.0,
        tectonic_half_length_base_meters: 170_000.0,
        tectonic_half_length_range_meters: 240_000.0,
        tectonic_width_base_meters: 28_000.0,
        tectonic_width_range_meters: 78_000.0,
        tectonic_peak_base_meters: 900.0,
        tectonic_peak_range_meters: 2_650.0,
        glacial_valley_base_depth_meters: 120.0,
        glacial_valley_depth_range_meters: 520.0,
        volcanic_relief_meters: 1_050.0,
        plateau_base_relief_meters: 180.0,
        plateau_relief_range_meters: 460.0,
        scarp_step_base_meters: 110.0,
        scarp_step_range_meters: 780.0,
        dune_base_amplitude_meters: 7.0,
        dune_sediment_amplitude_meters: 41.0,
        dune_aridity_amplitude_meters: 18.0,
    };

    /// Rejects non-finite and physically unusable optimizer proposals.
    pub fn is_valid(self) -> bool {
        let positive = [
            self.continental_relief_meters,
            self.boundary_uplift_meters,
            self.closed_basin_base_depth_meters,
            self.closed_basin_depth_range_meters,
            self.tectonic_half_length_base_meters,
            self.tectonic_half_length_range_meters,
            self.tectonic_width_base_meters,
            self.tectonic_width_range_meters,
            self.tectonic_peak_base_meters,
            self.tectonic_peak_range_meters,
            self.glacial_valley_base_depth_meters,
            self.glacial_valley_depth_range_meters,
            self.volcanic_relief_meters,
            self.plateau_base_relief_meters,
            self.plateau_relief_range_meters,
            self.scarp_step_base_meters,
            self.scarp_step_range_meters,
            self.dune_base_amplitude_meters,
            self.dune_sediment_amplitude_meters,
            self.dune_aridity_amplitude_meters,
        ];
        self.base_elevation_meters.is_finite()
            && positive
                .into_iter()
                .all(|value| value.is_finite() && value > 0.0)
    }
}

impl Default for ProvinceParameters {
    fn default() -> Self {
        Self::VERSION_18
    }
}

thread_local! {
    static PROVINCE_SAMPLE_CACHE: RefCell<VecDeque<ProvinceCacheEntry>> =
        const { RefCell::new(VecDeque::new()) };
}

#[derive(Clone, Copy)]
struct ProvinceCacheEntry {
    world: WorldIdentity,
    x_bits: u64,
    z_bits: u64,
    sample: ProvinceSample,
}

/// Integer identity of one bounded geographical province.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProvinceIndex {
    pub x: i64,
    pub z: i64,
}

impl ProvinceIndex {
    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    pub fn containing(x: f64, z: f64) -> Option<Self> {
        CellIndex::containing(x, z, 0, PROVINCE_EDGE_METERS).map(|cell| Self::new(cell.x, cell.z))
    }

    pub fn origin(self) -> [f64; 2] {
        [
            index_as_f64(self.x) * PROVINCE_EDGE_METERS,
            index_as_f64(self.z) * PROVINCE_EDGE_METERS,
        ]
    }

    pub fn center(self) -> [f64; 2] {
        let origin = self.origin();
        [
            origin[0] + (PROVINCE_EDGE_METERS * 0.5),
            origin[1] + (PROVINCE_EDGE_METERS * 0.5),
        ]
    }

    fn key(self, world: WorldIdentity, domain: u64) -> u64 {
        CellIndex::new(self.x, self.z, 0).generation_key(world, domain)
    }
}

/// Parent-scale values fixed at one shared province corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProvinceBoundaryCondition {
    pub continentalness: f64,
    pub uplift: f64,
    pub moisture_supply: f64,
    pub drainage_bias: f64,
}

/// Explicit boundary values shared with all adjacent province artifacts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProvinceBoundaryConditions {
    pub southwest: ProvinceBoundaryCondition,
    pub southeast: ProvinceBoundaryCondition,
    pub northwest: ProvinceBoundaryCondition,
    pub northeast: ProvinceBoundaryCondition,
}

/// Analytic geometry for one coherent fault scarp or bluff face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScarpGeometry {
    /// Negative values lie on the high side of the face.
    pub signed_distance_meters: f64,
    pub low_elevation_meters: f64,
    pub high_elevation_meters: f64,
    pub face_strength: f64,
    pub face_normal: [f64; 2],
    pub half_width_meters: f64,
    pub elevation_offset_meters: f64,
    pub undercut_depth_meters: f64,
}

/// Wind-aligned dune geometry shared by near and far terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuneGeometry {
    pub downwind_direction: [f64; 2],
    pub ridge_direction: [f64; 2],
    pub wavelength_meters: f64,
    pub phase_radians: f64,
    pub amplitude_meters: f64,
    pub strength: f64,
    pub height_offset_meters: f64,
    pub detail_height_offset_meters: f64,
}

/// Continuous causes and morphology outcomes sampled from a province plan.
///
/// Outcome weights intentionally overlap. They explain the expression of the
/// physical controls and are not mutually exclusive biome assignments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProvinceSample {
    pub province: ProvinceIndex,
    pub continentalness: f64,
    pub land_fraction: f64,
    pub coast_fraction: f64,
    pub crust_age: f64,
    pub rock_hardness: f64,
    pub carbonate_fraction: f64,
    pub uplift: f64,
    pub faulting: f64,
    pub strata_tilt: f64,
    pub volcanism: f64,
    pub glaciation: f64,
    pub erosion: f64,
    pub plains: f64,
    pub rolling_uplands: f64,
    pub plateau: f64,
    pub scarp: f64,
    pub mountain: f64,
    pub glacial: f64,
    pub dune: f64,
    pub closed_basin: f64,
    pub sediment: f64,
    pub drainage: f64,
    pub temperature: f64,
    pub aridity: f64,
    pub moisture: f64,
    pub salinity: f64,
    pub exposure: f64,
    pub disturbance: f64,
    /// A broad historical/dispersal axis available to downstream ecology.
    pub ecological_memory: f64,
    pub base_elevation_meters: f64,
    pub macro_relief_meters: f64,
    pub elevation_meters: f64,
    pub scarp_geometry: Option<ScarpGeometry>,
    pub dune_geometry: Option<DuneGeometry>,
}

/// A bounded top-down province artifact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProvincePlan {
    pub world: WorldIdentity,
    pub index: ProvinceIndex,
    pub plan_key: u64,
    pub parent_key: u64,
    pub boundary_conditions: ProvinceBoundaryConditions,
}

impl ProvincePlan {
    /// Generates the compact province artifact from explicit world identity.
    pub fn generate(world: WorldIdentity, index: ProvinceIndex) -> Option<Self> {
        if world.generator_version < PROVINCE_GENERATOR_VERSION {
            return None;
        }
        let origin = index.origin();
        let next_x = origin[0] + PROVINCE_EDGE_METERS;
        let next_z = origin[1] + PROVINCE_EDGE_METERS;
        let parent = CellIndex::containing(
            origin[0] + (PROVINCE_EDGE_METERS * 0.5),
            origin[1] + (PROVINCE_EDGE_METERS * 0.5),
            0,
            PARENT_EDGE_METERS,
        )?;
        Some(Self {
            world,
            index,
            plan_key: index.key(world, DOMAIN_PLAN),
            parent_key: parent.generation_key(world, DOMAIN_PARENT),
            boundary_conditions: ProvinceBoundaryConditions {
                southwest: boundary_condition(world, origin[0], origin[1])?,
                southeast: boundary_condition(world, next_x, origin[1])?,
                northwest: boundary_condition(world, origin[0], next_z)?,
                northeast: boundary_condition(world, next_x, next_z)?,
            },
        })
    }

    /// Samples inside the province or its fixed generation halo.
    pub fn sample(self, x: f64, z: f64) -> Option<ProvinceSample> {
        self.sample_with_parameters(x, z, ProvinceParameters::VERSION_18)
    }

    /// Samples with explicit offline calibration parameters.
    pub fn sample_with_parameters(
        self,
        x: f64,
        z: f64,
        parameters: ProvinceParameters,
    ) -> Option<ProvinceSample> {
        if !parameters.is_valid() {
            return None;
        }
        let origin = self.index.origin();
        let minimum_x = origin[0] - PROVINCE_HALO_METERS;
        let minimum_z = origin[1] - PROVINCE_HALO_METERS;
        let maximum_x = origin[0] + PROVINCE_EDGE_METERS + PROVINCE_HALO_METERS;
        let maximum_z = origin[1] + PROVINCE_EDGE_METERS + PROVINCE_HALO_METERS;
        if x < minimum_x || x > maximum_x || z < minimum_z || z > maximum_z {
            return None;
        }
        let owner = ProvinceIndex::containing(x, z)?;
        if owner == self.index {
            sample_owned(self, x, z, parameters)
        } else {
            // Halo sampling resolves the neighboring owner explicitly. This
            // keeps the caller bounded while ensuring both sides consume the
            // same artifact content rather than extrapolating local corners.
            sample_owned(ProvincePlan::generate(self.world, owner)?, x, z, parameters)
        }
    }

    /// Samples the containing artifact with a bounded, output-neutral thread cache.
    pub fn sample_at(world: WorldIdentity, x: f64, z: f64) -> Option<ProvinceSample> {
        if world.generator_version < PROVINCE_GENERATOR_VERSION {
            return None;
        }
        let x_bits = x.to_bits();
        let z_bits = z.to_bits();
        if let Some(sample) = PROVINCE_SAMPLE_CACHE.with(|cache| {
            cache
                .borrow()
                .iter()
                .rev()
                .find(|entry| {
                    entry.world == world && entry.x_bits == x_bits && entry.z_bits == z_bits
                })
                .map(|entry| entry.sample)
        }) {
            return Some(sample);
        }
        let owner = ProvinceIndex::containing(x, z)?;
        let sample = sample_owned(
            ProvincePlan::generate(world, owner)?,
            x,
            z,
            ProvinceParameters::VERSION_18,
        )?;
        PROVINCE_SAMPLE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= PROVINCE_SAMPLE_CACHE_ENTRIES {
                cache.pop_front();
            }
            cache.push_back(ProvinceCacheEntry {
                world,
                x_bits,
                z_bits,
                sample,
            });
        });
        Some(sample)
    }

    /// Samples arbitrary offline parameters without using the production cache.
    pub fn sample_at_with_parameters(
        world: WorldIdentity,
        x: f64,
        z: f64,
        parameters: ProvinceParameters,
    ) -> Option<ProvinceSample> {
        if world.generator_version < PROVINCE_GENERATOR_VERSION || !parameters.is_valid() {
            return None;
        }
        let owner = ProvinceIndex::containing(x, z)?;
        sample_owned(ProvincePlan::generate(world, owner)?, x, z, parameters)
    }
}

#[allow(clippy::too_many_lines)]
fn sample_owned(
    plan: ProvincePlan,
    x: f64,
    z: f64,
    parameters: ProvinceParameters,
) -> Option<ProvinceSample> {
    if !x.is_finite() || !z.is_finite() {
        return None;
    }
    let world = plan.world;
    let province = ProvinceIndex::containing(x, z)?;
    if province != plan.index {
        return None;
    }
    let planned_boundary = interpolate_boundaries(plan, x, z);
    let parent_land = planned_boundary.continentalness;
    let continental_land = value_field(
        world,
        DOMAIN_CONTINENT.wrapping_add(1),
        x,
        z,
        CONTINENT_EDGE_METERS,
    )?;
    let land_detail = value_field(world, DOMAIN_LAND_DETAIL, x, z, REGIONAL_EDGE_METERS)?;
    let continentalness =
        ((parent_land * 0.56) + (continental_land * 0.32) + (land_detail * 0.12)).clamp(0.0, 1.0);

    let crust_age = value_field(world, DOMAIN_CRUST_AGE, x, z, CONTINENT_EDGE_METERS)?;
    let rock_hardness = value_field(world, DOMAIN_ROCK_HARDNESS, x, z, REGIONAL_EDGE_METERS)?;
    let carbonate_fraction = value_field(world, DOMAIN_CARBONATE, x, z, REGIONAL_EDGE_METERS)?;
    let uplift_control = planned_boundary.uplift;
    let strata_tilt = value_field(world, DOMAIN_STRATA, x, z, REGIONAL_EDGE_METERS)?;
    let volcanism = value_field(world, DOMAIN_VOLCANISM, x, z, CONTINENT_EDGE_METERS)?;
    let glaciation_control = value_field(world, DOMAIN_GLACIATION, x, z, CONTINENT_EDGE_METERS)?;
    let erosion = value_field(world, DOMAIN_EROSION, x, z, CONTINENT_EDGE_METERS)?;
    let sediment_control = value_field(world, DOMAIN_SEDIMENT, x, z, REGIONAL_EDGE_METERS)?;
    let drainage_control = planned_boundary.drainage_bias;
    let moisture_supply = planned_boundary.moisture_supply;
    let temperature = value_field(world, DOMAIN_TEMPERATURE, x, z, CONTINENT_EDGE_METERS)?;
    let dry_control = value_field(world, DOMAIN_ARIDITY, x, z, CONTINENT_EDGE_METERS)?;
    let basin_control = value_field(world, DOMAIN_BASIN, x, z, REGIONAL_EDGE_METERS)?;
    let disturbance_control = value_field(world, DOMAIN_DISTURBANCE, x, z, REGIONAL_EDGE_METERS)?;
    let ecological_memory =
        value_field(world, DOMAIN_ECOLOGICAL_MEMORY, x, z, CONTINENT_EDGE_METERS)?;

    let tectonic = strongest_tectonic_feature(world, province, x, z, parameters)?;
    let faulting = tectonic.faulting;
    let uplift = (uplift_control * 0.46 + tectonic.strength * 0.54).clamp(0.0, 1.0);
    let moisture = (moisture_supply + ((continentalness - 0.5) * -0.16)
        - (tectonic.lee_shadow * 0.28))
        .clamp(0.0, 1.0);
    let aridity =
        ((dry_control * 0.48) + ((1.0 - moisture) * 0.37) + (temperature * 0.15)).clamp(0.0, 1.0);
    let glaciation =
        (glaciation_control * (0.38 + ((1.0 - temperature) * 0.62)) * (0.32 + (uplift * 0.68)))
            .clamp(0.0, 1.0);
    let drainage = (drainage_control * 0.48 + moisture * 0.28 + uplift * 0.24
        - basin_control * 0.18)
        .clamp(0.0, 1.0);
    let sediment =
        (sediment_control * 0.42 + erosion * 0.30 + drainage * 0.18 + (1.0 - uplift) * 0.10)
            .clamp(0.0, 1.0);
    let closed_basin = (smoothstep_range(0.56, 0.86, basin_control)
        * (0.42 + (aridity * 0.58))
        * (1.0 - drainage * 0.58))
        .clamp(0.0, 1.0);

    let provisional_base = parameters.base_elevation_meters
        + (continentalness * parameters.continental_relief_meters)
        + (uplift_control * parameters.boundary_uplift_meters)
        - (closed_basin * 130.0);
    let land_fraction = smoothstep_range(-90.0, 90.0, provisional_base);
    let coast_fraction =
        (1.0 - smoothstep_range(20.0, 360.0, provisional_base.abs())).clamp(0.0, 1.0);

    let mountain = (smoothstep_range(0.34, 0.78, uplift)
        * (0.46 + (tectonic.strength * 0.54))
        * land_fraction)
        .clamp(0.0, 1.0);
    let plateau = (smoothstep_range(0.48, 0.80, strata_tilt)
        * smoothstep_range(0.28, 0.68, uplift)
        * (1.0 - erosion * 0.54)
        * land_fraction)
        .clamp(0.0, 1.0);
    let glacial = (glaciation * (0.28 + (mountain * 0.72))).clamp(0.0, 1.0);
    let dune = (smoothstep_range(0.52, 0.86, aridity)
        * smoothstep_range(0.42, 0.78, sediment)
        * (1.0 - mountain * 0.86)
        * land_fraction)
        .clamp(0.0, 1.0);
    let plains = (smoothstep_range(0.42, 0.78, sediment)
        * (1.0 - uplift * 0.78)
        * (1.0 - plateau * 0.58)
        * land_fraction)
        .clamp(0.0, 1.0);
    let rolling_uplands = ((0.32 + (erosion * 0.48) + (uplift * 0.40))
        * (1.0 - plains * 0.64)
        * (1.0 - mountain * 0.58)
        * land_fraction)
        .clamp(0.0, 1.0);

    let scarp_influence = combined_scarp_features(
        world,
        province,
        x,
        z,
        provisional_base,
        faulting,
        plateau,
        rock_hardness,
        land_fraction,
        parameters,
    )?;
    let scarp_geometry = scarp_influence.strongest_geometry;
    let scarp = scarp_influence.strength;
    let dune_geometry = dune_geometry(world, x, z, dune, sediment, aridity, parameters)?;

    let tectonic_relief = tectonic.relief_meters * mountain * (0.72 + ((1.0 - erosion) * 0.28));
    let weathering_relief = tectonic.weathered_relief_meters * mountain * erosion;
    let volcanic_relief = volcanism
        * smoothstep_range(0.50, 0.82, uplift)
        * tectonic.envelope
        * parameters.volcanic_relief_meters;
    let plateau_relief = plateau
        * (parameters.plateau_base_relief_meters
            + (strata_tilt * parameters.plateau_relief_range_meters));
    let basin_relief = -closed_basin
        * (parameters.closed_basin_base_depth_meters
            + (basin_control * parameters.closed_basin_depth_range_meters));
    let glacial_relief = tectonic.glacial_valley_meters * glacial;
    let scarp_relief = scarp_influence.elevation_offset_meters;
    let dune_relief = dune_geometry.map_or(0.0, |geometry| geometry.height_offset_meters);
    let macro_relief_meters = tectonic_relief
        + weathering_relief
        + volcanic_relief
        + plateau_relief
        + basin_relief
        + glacial_relief
        + scarp_relief
        + dune_relief;
    let base_elevation_meters = provisional_base;
    let elevation_meters = base_elevation_meters + macro_relief_meters;

    let salinity = (closed_basin * (0.36 + (aridity * 0.64))
        + coast_fraction * (0.20 + (aridity * 0.28)))
        .clamp(0.0, 1.0);
    let exposure = (mountain * 0.38 + scarp * 0.34 + aridity * 0.18 + faulting * 0.10
        - sediment * 0.16)
        .clamp(0.0, 1.0);
    let disturbance = (disturbance_control * 0.42
        + faulting * 0.14
        + volcanism * 0.12
        + aridity * moisture * 0.18
        + exposure * 0.14)
        .clamp(0.0, 1.0);

    Some(ProvinceSample {
        province,
        continentalness,
        land_fraction,
        coast_fraction,
        crust_age,
        rock_hardness,
        carbonate_fraction,
        uplift,
        faulting,
        strata_tilt,
        volcanism,
        glaciation,
        erosion,
        plains,
        rolling_uplands,
        plateau,
        scarp,
        mountain,
        glacial,
        dune,
        closed_basin,
        sediment,
        drainage,
        temperature,
        aridity,
        moisture,
        salinity,
        exposure,
        disturbance,
        ecological_memory,
        base_elevation_meters,
        macro_relief_meters,
        elevation_meters,
        scarp_geometry,
        dune_geometry,
    })
}

fn boundary_condition(world: WorldIdentity, x: f64, z: f64) -> Option<ProvinceBoundaryCondition> {
    Some(ProvinceBoundaryCondition {
        continentalness: value_field(world, DOMAIN_CONTINENT, x, z, PARENT_EDGE_METERS)?,
        uplift: value_field(world, DOMAIN_UPLIFT, x, z, CONTINENT_EDGE_METERS)?,
        moisture_supply: value_field(world, DOMAIN_MOISTURE, x, z, CONTINENT_EDGE_METERS)?,
        drainage_bias: value_field(world, DOMAIN_DRAINAGE, x, z, CONTINENT_EDGE_METERS)?,
    })
}

fn interpolate_boundaries(plan: ProvincePlan, x: f64, z: f64) -> ProvinceBoundaryCondition {
    let origin = plan.index.origin();
    let x_blend = smoothstep01((x - origin[0]) / PROVINCE_EDGE_METERS);
    let z_blend = smoothstep01((z - origin[1]) / PROVINCE_EDGE_METERS);
    let boundaries = plan.boundary_conditions;
    ProvinceBoundaryCondition {
        continentalness: lerp(
            lerp(
                boundaries.southwest.continentalness,
                boundaries.southeast.continentalness,
                x_blend,
            ),
            lerp(
                boundaries.northwest.continentalness,
                boundaries.northeast.continentalness,
                x_blend,
            ),
            z_blend,
        ),
        uplift: lerp(
            lerp(
                boundaries.southwest.uplift,
                boundaries.southeast.uplift,
                x_blend,
            ),
            lerp(
                boundaries.northwest.uplift,
                boundaries.northeast.uplift,
                x_blend,
            ),
            z_blend,
        ),
        moisture_supply: lerp(
            lerp(
                boundaries.southwest.moisture_supply,
                boundaries.southeast.moisture_supply,
                x_blend,
            ),
            lerp(
                boundaries.northwest.moisture_supply,
                boundaries.northeast.moisture_supply,
                x_blend,
            ),
            z_blend,
        ),
        drainage_bias: lerp(
            lerp(
                boundaries.southwest.drainage_bias,
                boundaries.southeast.drainage_bias,
                x_blend,
            ),
            lerp(
                boundaries.northwest.drainage_bias,
                boundaries.northeast.drainage_bias,
                x_blend,
            ),
            z_blend,
        ),
    }
}

#[derive(Clone, Copy, Debug)]
struct TectonicInfluence {
    strength: f64,
    envelope: f64,
    faulting: f64,
    relief_meters: f64,
    weathered_relief_meters: f64,
    glacial_valley_meters: f64,
    lee_shadow: f64,
}

#[derive(Clone, Copy, Debug)]
struct ScarpInfluence {
    strength: f64,
    elevation_offset_meters: f64,
    strongest_geometry: Option<ScarpGeometry>,
}

fn strongest_tectonic_feature(
    world: WorldIdentity,
    containing: ProvinceIndex,
    x: f64,
    z: f64,
    parameters: ProvinceParameters,
) -> Option<TectonicInfluence> {
    let mut combined = TectonicInfluence {
        strength: 0.0,
        envelope: 0.0,
        faulting: 0.0,
        relief_meters: 0.0,
        weathered_relief_meters: 0.0,
        glacial_valley_meters: 0.0,
        lee_shadow: 0.0,
    };
    for z_offset in -1_i64..=1 {
        for x_offset in -1_i64..=1 {
            let owner = ProvinceIndex::new(
                containing.x.checked_add(x_offset)?,
                containing.z.checked_add(z_offset)?,
            );
            let key = owner.key(world, DOMAIN_TECTONIC_FEATURE);
            let center = owner.center();
            let direction = feature_direction(key, 0x4449_5245_4354);
            let half_length = parameters.tectonic_half_length_base_meters
                + (hash_fraction(key, 1) * parameters.tectonic_half_length_range_meters);
            let width = parameters.tectonic_width_base_meters
                + (hash_fraction(key, 2) * parameters.tectonic_width_range_meters);
            let jitter_x = (hash_fraction(key, 3) - 0.5) * PROVINCE_EDGE_METERS * 0.42;
            let jitter_z = (hash_fraction(key, 4) - 0.5) * PROVINCE_EDGE_METERS * 0.42;
            let start = [
                center[0] + jitter_x - (direction[0] * half_length),
                center[1] + jitter_z - (direction[1] * half_length),
            ];
            let end = [
                center[0] + jitter_x + (direction[0] * half_length),
                center[1] + jitter_z + (direction[1] * half_length),
            ];
            let (distance, along) = distance_to_segment(x, z, start, end)?;
            if distance >= width {
                continue;
            }
            let cross = 1.0 - (distance / width);
            let end_taper =
                smoothstep_range(0.0, 0.16, along) * (1.0 - smoothstep_range(0.84, 1.0, along));
            let envelope = smoothstep01(cross) * end_taper;
            let feature_strength = (0.18 + (hash_fraction(key, 5) * 0.82)) * envelope;
            let youth = hash_fraction(key, 6);
            let peak = parameters.tectonic_peak_base_meters
                + (hash_fraction(key, 7) * parameters.tectonic_peak_range_meters);
            let sharp_cross = libm::pow(cross, 0.72 + (youth * 1.9));
            let weathered_cross = smoothstep01(cross) * (1.0 - youth) * 0.46;
            let valley_width = width * (0.13 + (hash_fraction(key, 8) * 0.20));
            let valley_cross = (1.0 - (distance / valley_width)).clamp(0.0, 1.0);
            let glacial_valley_meters = -(parameters.glacial_valley_base_depth_meters
                + (hash_fraction(key, 9) * parameters.glacial_valley_depth_range_meters))
                * smoothstep01(valley_cross)
                * end_taper;
            combined.strength = smooth_union(combined.strength, feature_strength);
            combined.envelope = smooth_union(combined.envelope, envelope);
            combined.faulting = smooth_union(
                combined.faulting,
                (feature_strength * (0.45 + (youth * 0.55))).clamp(0.0, 1.0),
            );
            combined.relief_meters += peak * sharp_cross * end_taper;
            combined.weathered_relief_meters += peak * weathered_cross * end_taper;
            combined.glacial_valley_meters += glacial_valley_meters;
            combined.lee_shadow = smooth_union(
                combined.lee_shadow,
                feature_strength * smoothstep_range(0.48, 1.0, signed_side(x, z, start, direction)),
            );
        }
    }
    Some(combined)
}

#[allow(clippy::too_many_arguments)]
fn combined_scarp_features(
    world: WorldIdentity,
    containing: ProvinceIndex,
    x: f64,
    z: f64,
    base_elevation_meters: f64,
    faulting: f64,
    plateau: f64,
    rock_hardness: f64,
    land_fraction: f64,
    parameters: ProvinceParameters,
) -> Option<ScarpInfluence> {
    let mut strongest: Option<ScarpGeometry> = None;
    let mut combined_strength = 0.0;
    let mut elevation_offset_meters = 0.0;
    for z_offset in -1_i64..=1 {
        for x_offset in -1_i64..=1 {
            let owner = ProvinceIndex::new(
                containing.x.checked_add(x_offset)?,
                containing.z.checked_add(z_offset)?,
            );
            let key = owner.key(world, DOMAIN_SCARP_FEATURE);
            let center = owner.center();
            let direction = feature_direction(key, 0x5343_4152_5044);
            let normal = [-direction[1], direction[0]];
            let half_length = 100_000.0 + (hash_fraction(key, 1) * 210_000.0);
            let jitter_x = (hash_fraction(key, 2) - 0.5) * PROVINCE_EDGE_METERS * 0.56;
            let jitter_z = (hash_fraction(key, 3) - 0.5) * PROVINCE_EDGE_METERS * 0.56;
            let feature_center = [center[0] + jitter_x, center[1] + jitter_z];
            let along =
                ((x - feature_center[0]) * direction[0]) + ((z - feature_center[1]) * direction[1]);
            if along.abs() >= half_length {
                continue;
            }
            let end_taper = 1.0 - smoothstep_range(half_length * 0.72, half_length, along.abs());
            let signed_distance =
                ((x - feature_center[0]) * normal[0]) + ((z - feature_center[1]) * normal[1]);
            let half_width = 90.0 + (hash_fraction(key, 4) * 520.0);
            let proximity =
                1.0 - smoothstep_range(half_width, half_width * 5.0, signed_distance.abs());
            let cause = (0.12 + faulting * 0.46 + plateau * 0.28 + rock_hardness * 0.14)
                .clamp(0.0, 1.0)
                * (0.52 + (hash_fraction(key, 5) * 0.48))
                * land_fraction
                * end_taper;
            let face_strength = (cause * proximity).clamp(0.0, 1.0);
            if face_strength <= f64::EPSILON {
                continue;
            }
            let step_height = (parameters.scarp_step_base_meters
                + (hash_fraction(key, 6) * parameters.scarp_step_range_meters))
                * cause;
            let transition = 1.0 - smoothstep_range(-half_width, half_width, signed_distance);
            let contribution = step_height * transition * end_taper * proximity;
            combined_strength = smooth_union(combined_strength, face_strength);
            elevation_offset_meters += contribution;
            if strongest.is_none_or(|current| current.face_strength < face_strength) {
                strongest = Some(ScarpGeometry {
                    signed_distance_meters: signed_distance,
                    low_elevation_meters: base_elevation_meters,
                    high_elevation_meters: base_elevation_meters + step_height,
                    face_strength,
                    face_normal: normal,
                    half_width_meters: half_width,
                    elevation_offset_meters: contribution,
                    undercut_depth_meters: (step_height * 0.12 * cause).clamp(0.0, 74.0),
                });
            }
        }
    }
    Some(ScarpInfluence {
        strength: combined_strength,
        elevation_offset_meters,
        strongest_geometry: strongest,
    })
}

#[allow(clippy::option_option)]
fn dune_geometry(
    world: WorldIdentity,
    x: f64,
    z: f64,
    dune: f64,
    sediment: f64,
    aridity: f64,
    parameters: ProvinceParameters,
) -> Option<Option<DuneGeometry>> {
    if dune <= 0.03 {
        return Some(None);
    }
    let wave = stationary_dune_wave(world, x, z)?;
    let downwind_direction = wave.downwind_direction;
    let ridge_direction = [-downwind_direction[1], downwind_direction[0]];
    let strength = (dune * (0.54 + (aridity * 0.46))).clamp(0.0, 1.0);
    let amplitude_meters = parameters.dune_base_amplitude_meters
        + (sediment * parameters.dune_sediment_amplitude_meters)
        + (aridity * parameters.dune_aridity_amplitude_meters);
    Some(Some(DuneGeometry {
        downwind_direction,
        ridge_direction,
        wavelength_meters: wave.wavelength_meters,
        phase_radians: wave.phase_radians,
        amplitude_meters,
        strength,
        height_offset_meters: wave.primary_height * amplitude_meters * strength,
        detail_height_offset_meters: wave.detail_height
            * (1.2 + (amplitude_meters * 0.08))
            * strength,
    }))
}

#[derive(Clone, Copy, Debug)]
struct StationaryDuneWave {
    downwind_direction: [f64; 2],
    wavelength_meters: f64,
    phase_radians: f64,
    primary_height: f64,
    detail_height: f64,
}

fn stationary_dune_wave(world: WorldIdentity, x: f64, z: f64) -> Option<StationaryDuneWave> {
    let cell = CellIndex::containing(x, z, 0, CONTINENT_EDGE_METERS)?;
    let next_x = cell.x.checked_add(1)?;
    let next_z = cell.z.checked_add(1)?;
    let local_x = ((x / CONTINENT_EDGE_METERS) - index_as_f64(cell.x)).clamp(0.0, 1.0);
    let local_z = ((z / CONTINENT_EDGE_METERS) - index_as_f64(cell.z)).clamp(0.0, 1.0);
    let blend_x = smoothstep01(local_x);
    let blend_z = smoothstep01(local_z);
    let corners = [
        (cell.x, cell.z, (1.0 - blend_x) * (1.0 - blend_z)),
        (next_x, cell.z, blend_x * (1.0 - blend_z)),
        (cell.x, next_z, (1.0 - blend_x) * blend_z),
        (next_x, next_z, blend_x * blend_z),
    ];

    let mut direction = [0.0_f64; 2];
    let mut wavelength_meters = 0.0;
    let mut phase_vector = [0.0_f64; 2];
    let mut primary_height = 0.0;
    let mut detail_height = 0.0;
    for (corner_x, corner_z, weight) in corners {
        let key =
            CellIndex::new(corner_x, corner_z, 0).generation_key(world, DOMAIN_DUNE_DIRECTION);
        let angle = hash_fraction(key, 1) * core::f64::consts::TAU;
        let corner_direction = [libm::cos(angle), libm::sin(angle)];
        let wavelength = 170.0 + (hash_fraction(key, 2) * 620.0);
        let phase = hash_fraction(key, 3) * core::f64::consts::TAU;
        let detail_phase = hash_fraction(key, 4) * core::f64::consts::TAU;
        let origin_x = index_as_f64(corner_x) * CONTINENT_EDGE_METERS;
        let origin_z = index_as_f64(corner_z) * CONTINENT_EDGE_METERS;
        let projected =
            ((x - origin_x) * corner_direction[0]) + ((z - origin_z) * corner_direction[1]);
        let primary_phase = (projected / wavelength * core::f64::consts::TAU) + phase;
        let detail_wave_phase =
            (projected / (wavelength * 0.23) * core::f64::consts::TAU) + detail_phase;

        direction[0] += corner_direction[0] * weight;
        direction[1] += corner_direction[1] * weight;
        wavelength_meters += wavelength * weight;
        phase_vector[0] += libm::cos(phase) * weight;
        phase_vector[1] += libm::sin(phase) * weight;
        primary_height +=
            (libm::sin(primary_phase) + (libm::sin((primary_phase * 2.0) + 0.65) * 0.32)) * weight;
        detail_height += (libm::sin(detail_wave_phase)
            + (libm::sin((detail_wave_phase * 2.0) + 0.31) * 0.18))
            * weight;
    }
    let direction_length = libm::hypot(direction[0], direction[1]);
    let downwind_direction = if direction_length <= 1.0e-12 {
        [1.0, 0.0]
    } else {
        [
            direction[0] / direction_length,
            direction[1] / direction_length,
        ]
    };
    Some(StationaryDuneWave {
        downwind_direction,
        wavelength_meters,
        phase_radians: libm::atan2(phase_vector[1], phase_vector[0]),
        primary_height,
        detail_height,
    })
}

fn feature_direction(key: u64, domain: u64) -> [f64; 2] {
    let directions = [
        [1.0, 0.0],
        [0.0, 1.0],
        [
            core::f64::consts::FRAC_1_SQRT_2,
            core::f64::consts::FRAC_1_SQRT_2,
        ],
        [
            core::f64::consts::FRAC_1_SQRT_2,
            -core::f64::consts::FRAC_1_SQRT_2,
        ],
        [0.894_427_190_999_915_9, 0.447_213_595_499_957_9],
        [0.894_427_190_999_915_9, -0.447_213_595_499_957_9],
        [0.447_213_595_499_957_9, 0.894_427_190_999_915_9],
        [0.447_213_595_499_957_9, -0.894_427_190_999_915_9],
    ];
    let selected = stable_hash(&[key, domain]) & 7;
    directions[usize::try_from(selected).expect("masked direction fits usize")]
}

fn distance_to_segment(x: f64, z: f64, start: [f64; 2], end: [f64; 2]) -> Option<(f64, f64)> {
    let delta_x = end[0] - start[0];
    let delta_z = end[1] - start[1];
    let length_squared = libm::fma(delta_x, delta_x, delta_z * delta_z);
    if length_squared <= 0.0 {
        return None;
    }
    let along = (libm::fma(x - start[0], delta_x, (z - start[1]) * delta_z) / length_squared)
        .clamp(0.0, 1.0);
    let nearest_x = start[0] + (delta_x * along);
    let nearest_z = start[1] + (delta_z * along);
    Some((libm::hypot(x - nearest_x, z - nearest_z), along))
}

fn signed_side(x: f64, z: f64, start: [f64; 2], direction: [f64; 2]) -> f64 {
    let normal = [-direction[1], direction[0]];
    (((x - start[0]) * normal[0]) + ((z - start[1]) * normal[1])) / 120_000.0
}

fn value_field(world: WorldIdentity, domain: u64, x: f64, z: f64, edge: f64) -> Option<f64> {
    let cell = CellIndex::containing(x, z, 0, edge)?;
    let next_x = cell.x.checked_add(1)?;
    let next_z = cell.z.checked_add(1)?;
    let local_x = ((x / edge) - index_as_f64(cell.x)).clamp(0.0, 1.0);
    let local_z = ((z / edge) - index_as_f64(cell.z)).clamp(0.0, 1.0);
    let blend_x = smoothstep01(local_x);
    let blend_z = smoothstep01(local_z);
    let southwest = field_corner(world, domain, cell.x, cell.z);
    let southeast = field_corner(world, domain, next_x, cell.z);
    let northwest = field_corner(world, domain, cell.x, next_z);
    let northeast = field_corner(world, domain, next_x, next_z);
    Some(lerp(
        lerp(southwest, southeast, blend_x),
        lerp(northwest, northeast, blend_x),
        blend_z,
    ))
}

fn field_corner(world: WorldIdentity, domain: u64, x: i64, z: i64) -> f64 {
    let key = CellIndex::new(x, z, 0).generation_key(world, domain);
    hash53_as_f64(key >> 11) / 9_007_199_254_740_991.0
}

fn hash_fraction(key: u64, lane: u64) -> f64 {
    let hash = stable_hash(&[key, lane, DOMAIN_PLAN]);
    hash53_as_f64(hash >> 11) / 9_007_199_254_740_991.0
}

fn smoothstep01(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - (2.0 * value))
}

fn smoothstep_range(edge0: f64, edge1: f64, value: f64) -> f64 {
    smoothstep01((value - edge0) / (edge1 - edge0))
}

fn smooth_union(left: f64, right: f64) -> f64 {
    1.0 - ((1.0 - left.clamp(0.0, 1.0)) * (1.0 - right.clamp(0.0, 1.0)))
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(value: i64) -> f64 {
    value as f64
}

#[allow(clippy::cast_precision_loss)]
fn hash53_as_f64(value: u64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, PROVINCE_GENERATOR_VERSION, 0);

    #[test]
    fn negative_coordinates_use_half_open_provinces() {
        assert_eq!(
            ProvinceIndex::containing(-0.001, -PROVINCE_EDGE_METERS),
            Some(ProvinceIndex::new(-1, -1))
        );
        assert_eq!(
            ProvinceIndex::containing(0.0, -PROVINCE_EDGE_METERS),
            Some(ProvinceIndex::new(0, -1))
        );
    }

    #[test]
    fn adjacent_artifacts_share_exact_boundary_conditions() {
        let left = ProvincePlan::generate(WORLD, ProvinceIndex::new(-1, 2)).expect("left");
        let right = ProvincePlan::generate(WORLD, ProvinceIndex::new(0, 2)).expect("right");
        assert_eq!(
            left.boundary_conditions.southeast,
            right.boundary_conditions.southwest
        );
        assert_eq!(
            left.boundary_conditions.northeast,
            right.boundary_conditions.northwest
        );
    }

    #[test]
    fn explicit_default_parameters_preserve_cached_version_eighteen_samples() {
        for [x, z] in [
            [-712_000.0, 943_000.0],
            [0.0, 0.0],
            [3_500_000.0, -840_000.0],
        ] {
            let cached = ProvincePlan::sample_at(WORLD, x, z).expect("cached sample");
            let explicit = ProvincePlan::sample_at_with_parameters(
                WORLD,
                x,
                z,
                ProvinceParameters::VERSION_18,
            )
            .expect("explicit sample");
            assert_eq!(cached, explicit);
        }
    }

    #[test]
    fn calibration_parameters_change_morphology_without_changing_world_identity() {
        let [x, z] = [47_968_000.0, -36_696_000.0];
        let original = ProvincePlan::sample_at(WORLD, x, z).expect("original");
        let mut parameters = ProvinceParameters::VERSION_18;
        parameters.tectonic_peak_range_meters *= 1.5;
        let changed =
            ProvincePlan::sample_at_with_parameters(WORLD, x, z, parameters).expect("changed");
        assert_ne!(
            original.elevation_meters.to_bits(),
            changed.elevation_meters.to_bits()
        );
        assert_eq!(original.province, changed.province);
    }

    #[test]
    fn halo_and_containing_artifact_return_identical_physical_samples() {
        let x = PROVINCE_EDGE_METERS - 12_000.0;
        let z = -48_000.0;
        let neighboring_halo = ProvincePlan::generate(WORLD, ProvinceIndex::new(1, -1))
            .expect("neighboring plan")
            .sample(x, z)
            .expect("neighboring halo sample");
        let containing = ProvincePlan::sample_at(WORLD, x, z).expect("containing sample");
        assert_eq!(neighboring_halo, containing);
    }

    #[test]
    fn stored_boundary_conditions_causally_drive_the_owned_sample() {
        let plan = ProvincePlan::generate(WORLD, ProvinceIndex::new(-2, 3)).expect("plan");
        let center = plan.index.center();
        let original = plan.sample(center[0], center[1]).expect("original");
        let changed = ProvincePlan {
            boundary_conditions: ProvinceBoundaryConditions {
                southwest: ProvinceBoundaryCondition {
                    continentalness: 1.0,
                    ..plan.boundary_conditions.southwest
                },
                southeast: ProvinceBoundaryCondition {
                    continentalness: 1.0,
                    ..plan.boundary_conditions.southeast
                },
                northwest: ProvinceBoundaryCondition {
                    continentalness: 1.0,
                    ..plan.boundary_conditions.northwest
                },
                northeast: ProvinceBoundaryCondition {
                    continentalness: 1.0,
                    ..plan.boundary_conditions.northeast
                },
            },
            ..plan
        };
        let altered = changed.sample(center[0], center[1]).expect("altered");
        assert_ne!(
            original.continentalness.to_bits(),
            altered.continentalness.to_bits()
        );
        assert_ne!(
            original.base_elevation_meters.to_bits(),
            altered.base_elevation_meters.to_bits()
        );
    }

    #[test]
    fn physical_outputs_are_continuous_across_province_boundaries() {
        for seed in [0x5eed, 0xa11c_e5ed, 0xd15c_0a7e] {
            let world = WorldIdentity::new(seed, PROVINCE_GENERATOR_VERSION, 0);
            for z in [-1_300_000.0, -12_345.0, 840_000.0] {
                let boundary_x = PROVINCE_EDGE_METERS;
                let left =
                    ProvincePlan::sample_at(world, boundary_x - 0.01, z).expect("left sample");
                let right =
                    ProvincePlan::sample_at(world, boundary_x + 0.01, z).expect("right sample");
                assert!(
                    (left.elevation_meters - right.elevation_meters).abs() < 0.25,
                    "seed {seed:x}, z {z}: {} versus {}",
                    left.elevation_meters,
                    right.elevation_meters
                );
                assert!((left.moisture - right.moisture).abs() < 1.0e-6);
                assert!((left.uplift - right.uplift).abs() < 1.0e-6);
            }
        }
    }

    #[test]
    fn far_translated_dunes_keep_bounded_meter_scale_frequency() {
        let mut dune_site = None;
        'outer: for z_offset in -16..=16 {
            for x_offset in -16..=16 {
                let x = 48_000_000.0 + (f64::from(x_offset) * 64_000.0);
                let z = -37_000_000.0 + (f64::from(z_offset) * 64_000.0);
                let sample = ProvincePlan::sample_at(WORLD, x, z).expect("far sample");
                if sample.dune > 0.20 && sample.dune_geometry.is_some() {
                    dune_site = Some([x, z]);
                    break 'outer;
                }
            }
        }
        let [x, z] = dune_site.expect("far survey contains a dune field");
        for offset in 0..64 {
            let sample_x = x + f64::from(offset);
            let first = ProvincePlan::sample_at(WORLD, sample_x, z)
                .and_then(|sample| sample.dune_geometry)
                .expect("dune");
            let second = ProvincePlan::sample_at(WORLD, sample_x + 1.0, z)
                .and_then(|sample| sample.dune_geometry)
                .expect("adjacent dune");
            assert!(
                (first.height_offset_meters - second.height_offset_meters).abs() < 8.0,
                "dune frequency aliased at {sample_x}, {z}"
            );
            assert!(
                (first.detail_height_offset_meters - second.detail_height_offset_meters).abs()
                    < 4.0,
                "detail frequency aliased at {sample_x}, {z}"
            );
        }
    }

    #[test]
    fn scarp_support_tapers_without_a_parallel_height_jump() {
        let world = WorldIdentity::new(0x1234_5678_9abc_def0, PROVINCE_GENERATOR_VERSION, 0);
        let [x, z] = [47_968_000.0, -36_536_000.0];
        let scarp = ProvincePlan::sample_at(world, x, z)
            .and_then(|sample| sample.scarp_geometry)
            .expect("curated strong scarp");
        assert!(scarp.face_strength >= 0.55);
        let step = scarp.half_width_meters / 32.0;
        let mut previous: Option<f64> = None;
        for index in -168..=168 {
            let target_signed = f64::from(index) * step;
            let shift = target_signed - scarp.signed_distance_meters;
            let sample = ProvincePlan::sample_at(
                world,
                x + (scarp.face_normal[0] * shift),
                z + (scarp.face_normal[1] * shift),
            )
            .expect("transect sample");
            if let Some(previous_elevation) = previous {
                assert!(
                    (sample.elevation_meters - previous_elevation).abs() < 120.0,
                    "scarp support jumped from {previous_elevation} to {}",
                    sample.elevation_meters
                );
            }
            previous = Some(sample.elevation_meters);
        }
    }

    #[test]
    fn province_sampling_is_order_independent_and_bounded() {
        let positions = [
            [-1_100_000.0, 730_000.0],
            [-512_000.0, -0.001],
            [0.0, 0.0],
            [940_000.0, -1_820_000.0],
        ];
        let forward = positions.map(|[x, z]| ProvincePlan::sample_at(WORLD, x, z).expect("sample"));
        let mut reversed_positions = positions;
        reversed_positions.reverse();
        let mut reversed =
            reversed_positions.map(|[x, z]| ProvincePlan::sample_at(WORLD, x, z).expect("sample"));
        reversed.reverse();
        assert_eq!(forward, reversed);
        for sample in forward {
            for value in [
                sample.continentalness,
                sample.land_fraction,
                sample.coast_fraction,
                sample.crust_age,
                sample.rock_hardness,
                sample.carbonate_fraction,
                sample.uplift,
                sample.faulting,
                sample.strata_tilt,
                sample.volcanism,
                sample.glaciation,
                sample.erosion,
                sample.plains,
                sample.rolling_uplands,
                sample.plateau,
                sample.scarp,
                sample.mountain,
                sample.glacial,
                sample.dune,
                sample.closed_basin,
                sample.sediment,
                sample.drainage,
                sample.temperature,
                sample.aridity,
                sample.moisture,
                sample.salinity,
                sample.exposure,
                sample.disturbance,
                sample.ecological_memory,
            ] {
                assert!((0.0..=1.0).contains(&value), "{value}");
            }
            assert!(sample.elevation_meters.is_finite());
        }
    }

    #[test]
    fn far_apart_provinces_express_multiple_landform_causes() {
        let mut maxima = [0.0_f64; 8];
        for z in -24..=24 {
            for x in -24..=24 {
                let sample = ProvincePlan::sample_at(
                    WORLD,
                    f64::from(x) * 96_000.0,
                    f64::from(z) * 96_000.0,
                )
                .expect("province sample");
                let values = [
                    sample.plains,
                    sample.rolling_uplands,
                    sample.plateau,
                    sample.scarp,
                    sample.mountain,
                    sample.glacial,
                    sample.dune,
                    sample.closed_basin,
                ];
                for (maximum, value) in maxima.iter_mut().zip(values) {
                    *maximum = maximum.max(value);
                }
            }
        }
        for (index, maximum) in maxima.into_iter().enumerate() {
            assert!(maximum > 0.12, "landform axis {index} peaked at {maximum}");
        }
    }

    #[test]
    fn curated_far_translated_landforms_are_strongly_expressed_and_spatially_broad() {
        // Mountain, dune, glacial, and closed-basin sites found by the
        // five-seed, three-continent prevalence audit.
        let sites = [
            (
                0x1234_5678_9abc_def0,
                [47_968_000.0, -36_696_000.0],
                0_usize,
                0.95,
                0.40,
                24_usize,
            ),
            (
                0xd15c_0a7e,
                [48_128_000.0, -37_160_000.0],
                1_usize,
                0.85,
                0.30,
                40_usize,
            ),
            (
                0x1234_5678_9abc_def0,
                [47_968_000.0, -36_696_000.0],
                2_usize,
                0.55,
                0.25,
                24_usize,
            ),
            (
                0xd15c_0a7e,
                [48_288_000.0, -37_352_000.0],
                3_usize,
                0.80,
                0.30,
                16_usize,
            ),
        ];
        for (seed, [x, z], axis, minimum_peak, extent_threshold, minimum_run) in sites {
            let world = WorldIdentity::new(seed, PROVINCE_GENERATOR_VERSION, 0);
            let center = ProvincePlan::sample_at(world, x, z).expect("curated landform");
            let center_values = [
                center.mountain,
                center.dune,
                center.glacial,
                center.closed_basin,
            ];
            assert!(
                center_values[axis] >= minimum_peak,
                "landform axis {axis} peaked at {}",
                center_values[axis]
            );

            let mut maximum_run = 0_usize;
            for direction in [[1.0, 0.0], [0.0, 1.0]] {
                let mut current_run = 0_usize;
                for offset in -32..=32 {
                    let distance = f64::from(offset) * 16_000.0;
                    let sample = ProvincePlan::sample_at(
                        world,
                        x + (direction[0] * distance),
                        z + (direction[1] * distance),
                    )
                    .expect("landform transect");
                    let values = [
                        sample.mountain,
                        sample.dune,
                        sample.glacial,
                        sample.closed_basin,
                    ];
                    if values[axis] >= extent_threshold {
                        current_run += 1;
                        maximum_run = maximum_run.max(current_run);
                    } else {
                        current_run = 0;
                    }
                }
            }
            assert!(
                maximum_run >= minimum_run,
                "landform axis {axis} extended only {} km",
                maximum_run * 16
            );
        }
    }

    #[test]
    fn strong_scarp_expression_spans_a_coherent_face() {
        let scarp_world = WorldIdentity::new(0x1234_5678_9abc_def0, PROVINCE_GENERATOR_VERSION, 0);
        let scarp_position = [47_968_000.0, -36_536_000.0];
        let scarp = ProvincePlan::sample_at(scarp_world, scarp_position[0], scarp_position[1])
            .and_then(|sample| sample.scarp_geometry)
            .expect("audit scarp");
        let face = [
            scarp_position[0] - (scarp.face_normal[0] * scarp.signed_distance_meters),
            scarp_position[1] - (scarp.face_normal[1] * scarp.signed_distance_meters),
        ];
        let tangent = [-scarp.face_normal[1], scarp.face_normal[0]];
        let mut along_run = 0_usize;
        let mut maximum_along_run = 0_usize;
        for offset in -128..=128 {
            let distance = f64::from(offset) * 4_000.0;
            let strength = ProvincePlan::sample_at(
                scarp_world,
                face[0] + (tangent[0] * distance),
                face[1] + (tangent[1] * distance),
            )
            .expect("scarp transect")
            .scarp;
            if strength >= 0.20 {
                along_run += 1;
                maximum_along_run = maximum_along_run.max(along_run);
            } else {
                along_run = 0;
            }
        }
        let mut across_run = 0_usize;
        let mut maximum_across_run = 0_usize;
        for offset in -100..=100 {
            let distance = f64::from(offset) * 50.0;
            let strength = ProvincePlan::sample_at(
                scarp_world,
                face[0] + (scarp.face_normal[0] * distance),
                face[1] + (scarp.face_normal[1] * distance),
            )
            .expect("scarp transect")
            .scarp;
            if strength >= 0.20 {
                across_run += 1;
                maximum_across_run = maximum_across_run.max(across_run);
            } else {
                across_run = 0;
            }
        }
        assert!(
            scarp.face_strength >= 0.55,
            "scarp peaked at {}",
            scarp.face_strength
        );
        assert!(
            maximum_along_run >= 12,
            "strong face extended only {} km",
            maximum_along_run * 4
        );
        assert!(
            maximum_across_run >= 50,
            "scarp support covered only {} m",
            maximum_across_run * 50
        );
    }

    #[test]
    fn province_plan_has_a_golden_fingerprint() {
        let positions = [
            [-1_420_125.0, 812_375.0],
            [-512_000.0, -0.001],
            [0.0, 0.0],
            [2_960_500.0, -4_180_250.0],
        ];
        let words = positions
            .into_iter()
            .flat_map(|[x, z]| {
                let sample = ProvincePlan::sample_at(WORLD, x, z).expect("sample");
                [
                    sample.base_elevation_meters.to_bits(),
                    sample.macro_relief_meters.to_bits(),
                    sample.elevation_meters.to_bits(),
                    sample.uplift.to_bits(),
                    sample.faulting.to_bits(),
                    sample.plateau.to_bits(),
                    sample.mountain.to_bits(),
                    sample.dune.to_bits(),
                    sample.closed_basin.to_bits(),
                    sample.aridity.to_bits(),
                    sample.moisture.to_bits(),
                    sample.salinity.to_bits(),
                ]
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stable_hash(&words),
            2_998_011_128_221_680_516,
            "changing this value changes generator version 18 province plans"
        );
    }

    #[test]
    fn older_worlds_do_not_expose_province_artifacts() {
        let old = WorldIdentity::new(0x5eed, PROVINCE_GENERATOR_VERSION - 1, 0);
        assert!(ProvincePlan::sample_at(old, 0.0, 0.0).is_none());
        assert!(ProvincePlan::generate(old, ProvinceIndex::new(0, 0)).is_none());
    }
}
