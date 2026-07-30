//! Ecosystem primitives built from environmental variables rather than biome IDs.

mod ecosystem;
mod ground_vegetation;
mod reefs;
mod rocks;
mod wetlands;

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

use treeline_coordinates::{CellIndex, WorldIdentity, stable_hash};
use treeline_geography::{Climate, ProvincePlan, ProvinceSample, RegionalProfile};
use treeline_terrain::WildernessTerrain;

pub use ecosystem::{ECOSYSTEM_GENERATOR_VERSION, EcosystemDistribution, EcosystemSample};
pub use ground_vegetation::{
    GROUND_VEGETATION_GENERATOR_VERSION, GroundCoverGroup, GroundPlant, GroundPlantGenotype,
    GroundVegetation, GroundVegetationBounds, GroundVegetationComposition,
    GroundVegetationDistribution, GroundVegetationSample,
};
pub use reefs::{REEF_GENERATOR_VERSION, ReefComposition, ReefDistribution, ReefForm, ReefSample};
pub use rocks::{
    RockBounds, RockForm, RockGenotype, SURFACE_ROCK_GENERATOR_VERSION, SurfaceRock,
    SurfaceRockDistribution, SurfaceRockSample, SurfaceRocks,
};
pub use wetlands::{
    WETLAND_GENERATOR_VERSION, WetlandComposition, WetlandDistribution, WetlandHydrology,
    WetlandKind, WetlandSample,
};

/// Generator version that first exposes deterministic soil profiles.
pub const SOIL_GENERATOR_VERSION: u32 = 8;
/// Generator version that first exposes deterministic forest distributions.
pub const FOREST_GENERATOR_VERSION: u32 = 9;
/// Generator version that first exposes deterministic procedural tree individuals.
pub const TREE_GENERATOR_VERSION: u32 = 10;

const DOMAIN_FOREST_PATCHES: u64 = 0x464f_5245_5354_5041;
const DOMAIN_FOREST_STANDS: u64 = 0x464f_5245_5354_5354;
const DOMAIN_STAND_AGE: u64 = 0x5354_414e_445f_4147;
// Exact-coordinate memoization only: eviction and visitation order cannot
// change generated values.
const ECOLOGY_SAMPLE_CACHE_ENTRIES: usize = 64;

thread_local! {
    static SOIL_SAMPLE_CACHE: RefCell<VecDeque<SoilCacheEntry>> =
        const { RefCell::new(VecDeque::new()) };
    static FOREST_SAMPLE_CACHE: RefCell<VecDeque<ForestCacheEntry>> =
        const { RefCell::new(VecDeque::new()) };
}

#[derive(Clone, Copy)]
struct SoilCacheEntry {
    world: WorldIdentity,
    x_bits: u64,
    z_bits: u64,
    sample: SoilSample,
}

#[derive(Clone, Copy)]
struct ForestCacheEntry {
    world: WorldIdentity,
    x_bits: u64,
    z_bits: u64,
    sample: ForestSample,
}
const DOMAIN_FIRE_HISTORY: u64 = 0x4649_5245_4849_5354;
const DOMAIN_WINDTHROW_HISTORY: u64 = 0x5749_4e44_5448_524f;
const DOMAIN_FLOOD_HISTORY: u64 = 0x464c_4f4f_445f_4849;
const DOMAIN_LANDSLIDE_HISTORY: u64 = 0x4c41_4e44_534c_4944;
const FOREST_PATCH_EDGE_METERS: f64 = 2_000.0;
const FOREST_STAND_EDGE_METERS: f64 = 12_000.0;
const FOREST_HISTORY_EDGE_METERS: f64 = 32_000.0;
const DOMAIN_TREE_INDIVIDUALS: u64 = 0x5452_4545_5f49_4e44;
const TREE_PLACEMENT_CELL_EDGE_METERS: f64 = 6.0;
const TREE_ENVIRONMENT_CELL_EDGE_METERS: f64 = 48.0;

/// Broad, explainable soil texture derived from mineral fractions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoilTexture {
    Sandy,
    Loam,
    Silty,
    Clay,
}

impl SoilTexture {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sandy => "sandy",
            Self::Loam => "loam",
            Self::Silty => "silty",
            Self::Clay => "clay",
        }
    }
}

/// Mineral fractions in the fine earth portion of a soil profile.
///
/// The three fractions are normalized to sum to one. They are kept alongside
/// [`SoilTexture`] so later ecology can inspect continuous composition rather
/// than branching only on a soil label.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoilComposition {
    pub sand_fraction: f64,
    pub silt_fraction: f64,
    pub clay_fraction: f64,
}

impl SoilComposition {
    pub fn texture(self) -> SoilTexture {
        if self.clay_fraction >= 0.4 {
            SoilTexture::Clay
        } else if self.sand_fraction >= 0.65 {
            SoilTexture::Sandy
        } else if self.silt_fraction >= 0.6 {
            SoilTexture::Silty
        } else {
            SoilTexture::Loam
        }
    }
}

/// Explainable soil conditions at one horizontal world position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoilSample {
    pub composition: SoilComposition,
    pub texture: SoilTexture,
    pub depth_meters: f64,
    pub surface_moisture: f64,
    pub potential_evapotranspiration_millimeters: f64,
    pub climatic_water_balance_millimeters: f64,
    pub climatic_water_balance_fraction: f64,
    pub salinity_fraction: f64,
    pub acidity_ph: f64,
    pub organic_matter_fraction: f64,
    pub rock_exposure: f64,
    pub slope: f64,
}

impl SoilSample {
    /// Normalized acidity, where zero is alkaline and one is strongly acidic.
    pub fn acidity_fraction(self) -> f64 {
        ((8.5 - self.acidity_ph) / 5.0).clamp(0.0, 1.0)
    }

    /// Soil depth normalized against the current erosion model's 3.5 m cap.
    pub fn depth_fraction(self) -> f64 {
        (self.depth_meters / 3.5).clamp(0.0, 1.0)
    }
}

/// Functional soil sampler derived from geology, erosion, and annual climate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Soil {
    pub world: WorldIdentity,
}

impl Soil {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Samples a deterministic soil profile without consulting simulation state.
    ///
    /// Texture follows parent-rock hardness, weathering, scree, and deposited
    /// sediment. Moisture balances precipitation and snowmelt against
    /// temperature, slope, exposure, and texture-dependent drainage. Acidity
    /// responds to carbonate-like karst geology, leaching, and organic matter.
    ///
    /// The arithmetic and square roots used by lower-level terrain and climate
    /// samplers are part of the versioned generation contract.
    #[allow(clippy::too_many_lines)]
    pub fn sample(self, x: f64, z: f64) -> Option<SoilSample> {
        if self.world.generator_version < SOIL_GENERATOR_VERSION {
            return None;
        }
        let x_bits = x.to_bits();
        let z_bits = z.to_bits();
        if let Some(sample) = SOIL_SAMPLE_CACHE.with(|cache| {
            cache
                .borrow()
                .iter()
                .rev()
                .find(|entry| {
                    entry.world == self.world && entry.x_bits == x_bits && entry.z_bits == z_bits
                })
                .map(|entry| entry.sample)
        }) {
            return Some(sample);
        }

        let profile = RegionalProfile::sample(self.world, x, z)?;
        let erosion = WildernessTerrain::new(self.world).erosion_at(x, z)?;
        let climate = Climate::new(self.world).sample(x, z)?;
        let province = if self.world.generator_version >= ECOSYSTEM_GENERATOR_VERSION {
            Some(ProvincePlan::sample_at(self.world, x, z)?)
        } else {
            None
        };
        let softness = 1.0 - profile.rock_hardness;
        let deposition = (erosion.sediment_deposition_meters / 18.0).clamp(0.0, 1.0);

        let mut sand_weight = 0.2 + (profile.rock_hardness * 0.45) + (erosion.scree_cover * 0.3);
        let mut silt_weight = 0.18 + (deposition * 0.55) + (profile.erosion_age * 0.22);
        let mut clay_weight = 0.12
            + (softness * 0.38)
            + (profile.erosion_age * 0.25)
            + (profile.karst_probability * 0.08);
        if let Some(province) = province {
            sand_weight += province.dune * 0.58 + province.rock_hardness * 0.12;
            silt_weight += province.sediment * 0.46 + province.plains * 0.12;
            clay_weight +=
                province.closed_basin * province.sediment * 0.34 + province.erosion * 0.08;
        }
        let mineral_total = sand_weight + silt_weight + clay_weight;
        let composition = SoilComposition {
            sand_fraction: sand_weight / mineral_total,
            silt_fraction: silt_weight / mineral_total,
            clay_fraction: clay_weight / mineral_total,
        };

        let precipitation = climate.precipitation_fraction();
        let warmth = climate.warmth_fraction();
        let temperate_productivity = (1.0 - ((warmth - 0.55).abs() * 1.8)).clamp(0.0, 1.0);
        let stable_ground =
            (1.0 - (erosion.rock_exposure * 0.65) - (erosion.scree_cover * 0.35)).clamp(0.0, 1.0);
        let organic_matter_fraction = (0.01
            + (0.16
                * precipitation
                * temperate_productivity
                * stable_ground
                * (0.3 + (profile.erosion_age * 0.7))))
            .clamp(0.01, 0.17);
        let organic_fraction = organic_matter_fraction / 0.17;

        let water_holding = (composition.clay_fraction * 0.55)
            + (composition.silt_fraction * 0.25)
            + (organic_fraction * 0.2);
        let slope_drainage = (erosion.slope / 0.12).clamp(0.0, 1.0);
        let drainage = (composition.sand_fraction * 0.38)
            + (slope_drainage * 0.45)
            + (erosion.rock_exposure * 0.17);
        let snowmelt_fraction = (climate.annual_snowmelt_millimeters
            / climate.annual_precipitation_millimeters.max(1.0))
        .clamp(0.0, 1.0);
        let depth_fraction = (erosion.soil_depth_meters / 3.5).clamp(0.0, 1.0);
        let mut surface_moisture = ((precipitation * 0.62)
            + (snowmelt_fraction * 0.15)
            + (water_holding * 0.35)
            + (depth_fraction * 0.12)
            - (warmth * 0.2)
            - (drainage * 0.32))
            .clamp(0.0, 1.0);

        let mut potential_evapotranspiration_millimeters = 0.0;
        let mut climatic_water_balance_millimeters = 0.0;
        let mut climatic_water_balance_fraction = surface_moisture;
        let mut salinity_fraction = 0.0;
        if let Some(province) = province {
            let balance = ecosystem::climate_water_balance(climate, &province);
            potential_evapotranspiration_millimeters =
                balance.potential_evapotranspiration_millimeters;
            climatic_water_balance_millimeters = balance.balance_millimeters;
            climatic_water_balance_fraction = balance.fraction;
            salinity_fraction = (province.salinity
                * (0.52 + (province.aridity * 0.32) + (province.closed_basin * 0.16)))
                .clamp(0.0, 1.0);
            surface_moisture = ((surface_moisture * 0.42)
                + (balance.fraction * 0.34)
                + (province.moisture * 0.24)
                - (salinity_fraction * 0.18))
                .clamp(0.0, 1.0);
        }

        let mut acidity_ph =
            (6.8 + (profile.karst_probability * 1.2) + (profile.rock_hardness * 0.25)
                - (precipitation * 0.95)
                - (organic_fraction * 0.55))
                .clamp(3.5, 8.5);
        if let Some(province) = province {
            acidity_ph =
                (acidity_ph + (province.carbonate_fraction * 0.34) + (salinity_fraction * 0.46))
                    .clamp(3.5, 8.5);
        }

        let sample = SoilSample {
            composition,
            texture: composition.texture(),
            depth_meters: erosion.soil_depth_meters,
            surface_moisture,
            potential_evapotranspiration_millimeters,
            climatic_water_balance_millimeters,
            climatic_water_balance_fraction,
            salinity_fraction,
            acidity_ph,
            organic_matter_fraction,
            rock_exposure: erosion.rock_exposure,
            slope: erosion.slope,
        };
        SOIL_SAMPLE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= ECOLOGY_SAMPLE_CACHE_ENTRIES {
                cache.pop_front();
            }
            cache.push_back(SoilCacheEntry {
                world: self.world,
                x_bits,
                z_bits,
                sample,
            });
        });
        Some(sample)
    }
}

/// Local environmental conditions normalized to the inclusive range 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    pub temperature: f64,
    pub moisture: f64,
    pub soil_depth: f64,
    pub soil_acidity: f64,
    pub sand_fraction: f64,
    pub clay_fraction: f64,
    pub rock_exposure: f64,
    pub sunlight: f64,
    pub disturbance: f64,
}

impl Environment {
    /// Combines a generated soil profile with ecology inputs owned by callers.
    pub fn from_soil(temperature: f64, sunlight: f64, disturbance: f64, soil: SoilSample) -> Self {
        Self {
            temperature: temperature.clamp(0.0, 1.0),
            moisture: soil.surface_moisture,
            soil_depth: soil.depth_fraction(),
            soil_acidity: soil.acidity_fraction(),
            sand_fraction: soil.composition.sand_fraction,
            clay_fraction: soil.composition.clay_fraction,
            rock_exposure: soil.rock_exposure,
            sunlight: sunlight.clamp(0.0, 1.0),
            disturbance: disturbance.clamp(0.0, 1.0),
        }
    }
}

/// Preferred environmental center and radial tolerance for a procedural species.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeciesPreference {
    pub temperature: f64,
    pub moisture: f64,
    pub soil_depth: f64,
    pub soil_acidity: f64,
    pub sand_fraction: f64,
    pub clay_fraction: f64,
    pub rock_exposure: f64,
    pub tolerance: f64,
}

impl SpeciesPreference {
    /// Returns a normalized suitability score without assigning a biome label.
    pub fn suitability(self, environment: Environment) -> f64 {
        if self.tolerance <= 0.0 {
            return 0.0;
        }
        let squared_distance = [
            environment.temperature - self.temperature,
            environment.moisture - self.moisture,
            environment.soil_depth - self.soil_depth,
            environment.soil_acidity - self.soil_acidity,
            environment.sand_fraction - self.sand_fraction,
            environment.clay_fraction - self.clay_fraction,
            environment.rock_exposure - self.rock_exposure,
        ]
        .into_iter()
        .map(|delta| delta * delta)
        .sum::<f64>();
        (1.0 - (libm::sqrt(squared_distance) / self.tolerance)).clamp(0.0, 1.0)
    }
}

/// A tree growth strategy, used as a continuous composition axis rather than a biome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeFunctionalGroup {
    EvergreenNeedleleaf,
    ColdDeciduous,
    TemperateBroadleaf,
    DryWoodland,
}

impl TreeFunctionalGroup {
    pub const ALL: [Self; 4] = [
        Self::EvergreenNeedleleaf,
        Self::ColdDeciduous,
        Self::TemperateBroadleaf,
        Self::DryWoodland,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::EvergreenNeedleleaf => "evergreen needleleaf",
            Self::ColdDeciduous => "cold deciduous",
            Self::TemperateBroadleaf => "temperate broadleaf",
            Self::DryWoodland => "dry woodland",
        }
    }

    const fn preference(self) -> SpeciesPreference {
        match self {
            Self::EvergreenNeedleleaf => SpeciesPreference {
                temperature: 0.32,
                moisture: 0.62,
                soil_depth: 0.52,
                soil_acidity: 0.72,
                sand_fraction: 0.42,
                clay_fraction: 0.22,
                rock_exposure: 0.24,
                tolerance: 1.18,
            },
            Self::ColdDeciduous => SpeciesPreference {
                temperature: 0.43,
                moisture: 0.58,
                soil_depth: 0.62,
                soil_acidity: 0.55,
                sand_fraction: 0.34,
                clay_fraction: 0.28,
                rock_exposure: 0.16,
                tolerance: 1.12,
            },
            Self::TemperateBroadleaf => SpeciesPreference {
                temperature: 0.61,
                moisture: 0.66,
                soil_depth: 0.76,
                soil_acidity: 0.43,
                sand_fraction: 0.30,
                clay_fraction: 0.32,
                rock_exposure: 0.10,
                tolerance: 1.02,
            },
            Self::DryWoodland => SpeciesPreference {
                temperature: 0.70,
                moisture: 0.28,
                soil_depth: 0.42,
                soil_acidity: 0.35,
                sand_fraction: 0.52,
                clay_fraction: 0.18,
                rock_exposure: 0.30,
                tolerance: 1.08,
            },
        }
    }

    const fn mature_height_meters(self) -> f64 {
        match self {
            Self::EvergreenNeedleleaf => 31.0,
            Self::ColdDeciduous => 24.0,
            Self::TemperateBroadleaf => 29.0,
            Self::DryWoodland => 13.0,
        }
    }
}

/// Relative abundance of each tree growth strategy at one location.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForestComposition {
    pub evergreen_needleleaf_fraction: f64,
    pub cold_deciduous_fraction: f64,
    pub temperate_broadleaf_fraction: f64,
    pub dry_woodland_fraction: f64,
}

impl ForestComposition {
    pub fn fraction(self, group: TreeFunctionalGroup) -> f64 {
        match group {
            TreeFunctionalGroup::EvergreenNeedleleaf => self.evergreen_needleleaf_fraction,
            TreeFunctionalGroup::ColdDeciduous => self.cold_deciduous_fraction,
            TreeFunctionalGroup::TemperateBroadleaf => self.temperate_broadleaf_fraction,
            TreeFunctionalGroup::DryWoodland => self.dry_woodland_fraction,
        }
    }

    pub fn dominant(self) -> TreeFunctionalGroup {
        let mut dominant = TreeFunctionalGroup::EvergreenNeedleleaf;
        for group in [
            TreeFunctionalGroup::ColdDeciduous,
            TreeFunctionalGroup::TemperateBroadleaf,
            TreeFunctionalGroup::DryWoodland,
        ] {
            if self.fraction(group) > self.fraction(dominant) {
                dominant = group;
            }
        }
        dominant
    }
}

/// The most influential recent stand-replacing process at a location.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForestDisturbance {
    Fire,
    Windthrow,
    Flood,
    Landslide,
}

impl ForestDisturbance {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fire => "fire",
            Self::Windthrow => "windthrow",
            Self::Flood => "flood",
            Self::Landslide => "landslide",
        }
    }
}

/// Explainable forest coverage, composition, and stand structure at one position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForestSample {
    pub canopy_cover_fraction: f64,
    pub tree_density_per_hectare: f64,
    pub aboveground_biomass_kg_per_square_meter: f64,
    pub mean_canopy_height_meters: f64,
    pub stand_age_years: f64,
    pub disturbance_severity: f64,
    pub dominant_disturbance: ForestDisturbance,
    pub composition: ForestComposition,
}

impl ForestSample {
    pub fn dominant_group(self) -> TreeFunctionalGroup {
        self.composition.dominant()
    }
}

/// Functional forest sampler derived from climate, soil, terrain, and stand history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForestDistribution {
    pub world: WorldIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ForestHistory {
    stand_age_years: f64,
    severity: f64,
    dominant_disturbance: ForestDisturbance,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HistoryEnvironment {
    dryness: f64,
    warmth: f64,
    precipitation: f64,
    surface_moisture: f64,
    slope: f64,
}

impl ForestDistribution {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Samples forest distribution without assigning a biome or consulting simulation state.
    ///
    /// The returned cover is spatially continuous and combines tree-group
    /// suitability with soil depth, exposed rock, slope, permanent snow,
    /// coherent stand patchiness, and a deterministic disturbance history.
    /// The value fields and IEEE-754 arithmetic are part of generator version 9.
    #[allow(clippy::too_many_lines)]
    pub fn sample(self, x: f64, z: f64) -> Option<ForestSample> {
        if self.world.generator_version < FOREST_GENERATOR_VERSION {
            return None;
        }
        let x_bits = x.to_bits();
        let z_bits = z.to_bits();
        if let Some(sample) = FOREST_SAMPLE_CACHE.with(|cache| {
            cache
                .borrow()
                .iter()
                .rev()
                .find(|entry| {
                    entry.world == self.world && entry.x_bits == x_bits && entry.z_bits == z_bits
                })
                .map(|entry| entry.sample)
        }) {
            return Some(sample);
        }

        let climate = Climate::new(self.world).sample(x, z)?;
        let soil = Soil::new(self.world).sample(x, z)?;
        let warmth = climate.warmth_fraction();
        let precipitation = climate.precipitation_fraction();
        let sunlight = (0.88 - (precipitation * 0.28)).clamp(0.0, 1.0);
        let slope_fraction = (soil.slope / 0.22).clamp(0.0, 1.0);
        let dryness = 1.0 - soil.surface_moisture;
        let ecosystem = if self.world.generator_version >= ECOSYSTEM_GENERATOR_VERSION {
            Some(EcosystemDistribution::new(self.world).sample(x, z)?)
        } else {
            None
        };
        let province = if self.world.generator_version >= ECOSYSTEM_GENERATOR_VERSION {
            Some(ProvincePlan::sample_at(self.world, x, z)?)
        } else {
            None
        };

        let history = sample_forest_history(
            self.world,
            x,
            z,
            HistoryEnvironment {
                dryness,
                warmth,
                precipitation,
                surface_moisture: soil.surface_moisture,
                slope: slope_fraction,
            },
        )?;
        let disturbance_severity = ecosystem.map_or(history.severity, |ecosystem| {
            ((history.severity * 0.52)
                + (ecosystem.disturbance_fraction * 0.28)
                + (ecosystem.fire_pressure_fraction * 0.20))
                .clamp(0.0, 1.0)
        });
        let stand_age_years = ecosystem.map_or(history.stand_age_years, |ecosystem| {
            history.stand_age_years * (1.0 - (ecosystem.disturbance_fraction * 0.46))
        });
        let dominant_disturbance = ecosystem.map_or(history.dominant_disturbance, |ecosystem| {
            if ecosystem.fire_pressure_fraction > history.severity {
                ForestDisturbance::Fire
            } else {
                history.dominant_disturbance
            }
        });

        let environment = Environment::from_soil(warmth, sunlight, disturbance_severity, soil);
        let permanent_snow_fraction =
            (climate.permanent_snowpack_water_equivalent_millimeters / 1_200.0).clamp(0.0, 1.0);
        let (composition, best_suitability) = match (ecosystem, province) {
            (Some(ecosystem), Some(province)) => forest_composition_v18(
                environment,
                permanent_snow_fraction,
                disturbance_severity,
                ecosystem,
                &province,
            ),
            _ => forest_composition(environment, permanent_snow_fraction, disturbance_severity),
        };
        let patchiness = (value_field(
            self.world,
            DOMAIN_FOREST_PATCHES,
            x,
            z,
            FOREST_PATCH_EDGE_METERS,
        )? * 0.38)
            + (value_field(
                self.world,
                DOMAIN_FOREST_STANDS,
                x,
                z,
                FOREST_STAND_EDGE_METERS,
            )? * 0.62);
        let substrate = (0.18 + (soil.depth_fraction() * 0.82))
            * (1.0 - (soil.rock_exposure * 0.78))
            * (1.0 - (slope_fraction * 0.60));
        let succession_cover = (stand_age_years / 55.0).clamp(0.12, 1.0);
        let mut canopy_cover_fraction =
            (best_suitability * substrate * (0.42 + (patchiness * 0.72)) * succession_cover)
                .clamp(0.0, 1.0);
        if let Some(ecosystem) = ecosystem {
            let target_cover = (ecosystem.closed_forest_potential * 0.92)
                + (ecosystem.open_woodland_potential * 0.36)
                + (ecosystem.wetland_potential * 0.08);
            let open_excess = (ecosystem.open_land_potential()
                - (ecosystem.closed_forest_potential * 0.68)
                - (ecosystem.open_woodland_potential * 0.24))
                .max(0.0);
            let local_canopy = ((canopy_cover_fraction * 0.24)
                + (target_cover * substrate * succession_cover * (0.66 + (patchiness * 0.42))))
                .clamp(0.0, 1.0);
            // The ecosystem potential has already paid the broad penalties for
            // water balance, substrate, exposure, fire, elevation, and salinity.
            // Reapplying the full local substrate and stand-age terms made even
            // strongly mesic provinces read as sparse woodland. Preserve local
            // gaps, but let a sustained causal forest signal close the canopy.
            let mesic_closure = (target_cover
                * (0.82 + (substrate * 0.18))
                * (0.88 + (patchiness * 0.12))
                * (0.86 + ((1.0 - disturbance_severity) * 0.14)))
                .clamp(0.0, 1.0);
            canopy_cover_fraction = ((local_canopy.max(mesic_closure)
                * (1.0 - (open_excess * 0.88)))
                * ecosystem.land_fraction)
                .clamp(0.0, 1.0);
        }

        let mature_height_meters = TreeFunctionalGroup::ALL
            .into_iter()
            .map(|group| composition.fraction(group) * group.mature_height_meters())
            .sum::<f64>();
        let maturity = (stand_age_years / 120.0).clamp(0.0, 1.0);
        let mean_canopy_height_meters = mature_height_meters
            * (0.20 + (maturity * 0.80))
            * (0.45 + (best_suitability * 0.55))
            * (1.0 - (soil.rock_exposure * 0.30));
        let normalized_height = (mean_canopy_height_meters / 31.0).clamp(0.0, 1.0);
        let tree_density_per_hectare =
            canopy_cover_fraction * (260.0 + (1_040.0 * (1.0 - normalized_height)));
        let aboveground_biomass_kg_per_square_meter =
            canopy_cover_fraction * mean_canopy_height_meters * (0.38 + (maturity * 0.34));

        let sample = ForestSample {
            canopy_cover_fraction,
            tree_density_per_hectare,
            aboveground_biomass_kg_per_square_meter,
            mean_canopy_height_meters,
            stand_age_years,
            disturbance_severity,
            dominant_disturbance,
            composition,
        };
        FOREST_SAMPLE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= ECOLOGY_SAMPLE_CACHE_ENTRIES {
                cache.pop_front();
            }
            cache.push_back(ForestCacheEntry {
                world: self.world,
                x_bits,
                z_bits,
                sample,
            });
        });
        Some(sample)
    }
}

/// Half-open horizontal area used to request individual trees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeBounds {
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
}

impl TreeBounds {
    /// Creates finite, non-empty tree-generation bounds.
    pub fn new(min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> Option<Self> {
        [min_x, min_z, max_x, max_z]
            .into_iter()
            .all(f64::is_finite)
            .then_some(())
            .filter(|()| min_x < max_x && min_z < max_z)?;
        Some(Self {
            min_x,
            min_z,
            max_x,
            max_z,
        })
    }

    fn contains(self, x: f64, z: f64) -> bool {
        x >= self.min_x && x < self.max_x && z >= self.min_z && z < self.max_z
    }
}

/// Broad crown architecture selected by a tree's procedural genotype.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrownShape {
    Conical,
    Columnar,
    Rounded,
    Spreading,
}

/// Bark architecture used to vary trunk color and surface treatment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarkStyle {
    Scaly,
    Smooth,
    Furrowed,
    Plated,
}

/// Visible life history of one tree individual.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeCondition {
    Sapling,
    Mature,
    Ancient,
    WindDamaged,
    Fallen,
    DeadStanding,
    StormBroken,
}

impl TreeCondition {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sapling => "sapling",
            Self::Mature => "mature",
            Self::Ancient => "ancient",
            Self::WindDamaged => "wind-damaged",
            Self::Fallen => "fallen",
            Self::DeadStanding => "dead standing",
            Self::StormBroken => "storm-broken",
        }
    }
}

/// Continuous architecture and environmental responses for one procedural tree.
///
/// Functional groups provide ecological strategy, while these values vary per
/// individual. Renderers consume the genotype as a grammar rather than choosing
/// from a fixed library of tree models.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeGenotype {
    pub functional_group: TreeFunctionalGroup,
    pub mature_height_meters: f64,
    pub height_variation_fraction: f64,
    pub trunk_taper_fraction: f64,
    pub branching_angle_radians: f64,
    pub branch_density_fraction: f64,
    pub crown_shape: CrownShape,
    pub leaf_density_fraction: f64,
    pub bark_style: BarkStyle,
    pub slope_response_fraction: f64,
    pub wind_response_fraction: f64,
    pub competition_response_fraction: f64,
}

/// A deterministic tree individual positioned on the global horizontal lattice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProceduralTree {
    pub id: u64,
    pub x: f64,
    pub z: f64,
    pub age_years: f64,
    pub height_meters: f64,
    pub trunk_base_radius_meters: f64,
    pub crown_radius_meters: f64,
    pub lean_direction: [f64; 2],
    pub lean_fraction: f64,
    pub damage_fraction: f64,
    pub rotation_turns: f64,
    pub condition: TreeCondition,
    pub genotype: TreeGenotype,
}

/// Functional generator for spatially stable tree individuals and architectures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProceduralTrees {
    pub world: WorldIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TreeCellEnvironment {
    forest: ForestSample,
    soil: SoilSample,
    prevailing_wind: [f64; 2],
}

impl ProceduralTrees {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Generates all tree bases inside a half-open horizontal area.
    ///
    /// A global six-meter lattice owns placement candidates. Each cell samples
    /// the forest field once, then stochastically rounds its expected stem count
    /// and jitters those stems inside the cell. Filtering after generation makes
    /// adjacent requests share exact boundary behavior and keeps output
    /// independent of request or job order. Generator version 11 standardizes
    /// non-basic floating-point operations on the pure-Rust `libm`
    /// implementation across native and WebAssembly targets.
    pub fn trees_in(self, bounds: TreeBounds) -> Option<Vec<ProceduralTree>> {
        if self.world.generator_version < TREE_GENERATOR_VERSION {
            return None;
        }
        let minimum = CellIndex::containing(
            bounds.min_x,
            bounds.min_z,
            0,
            TREE_PLACEMENT_CELL_EDGE_METERS,
        )?;
        let maximum = CellIndex::containing(
            bounds.max_x,
            bounds.max_z,
            0,
            TREE_PLACEMENT_CELL_EDGE_METERS,
        )?;
        let forest = ForestDistribution::new(self.world);
        let soil = Soil::new(self.world);
        let climate = Climate::new(self.world);
        let mut environments = BTreeMap::new();
        let mut trees = Vec::new();
        let mut cell_z = minimum.z;
        loop {
            let mut cell_x = minimum.x;
            loop {
                let cell = CellIndex::new(cell_x, cell_z, 0);
                let origin_x = index_as_f64(cell_x) * TREE_PLACEMENT_CELL_EDGE_METERS;
                let origin_z = index_as_f64(cell_z) * TREE_PLACEMENT_CELL_EDGE_METERS;
                let center_x = origin_x + (TREE_PLACEMENT_CELL_EDGE_METERS * 0.5);
                let center_z = origin_z + (TREE_PLACEMENT_CELL_EDGE_METERS * 0.5);
                let environment_cell = CellIndex::containing(
                    center_x,
                    center_z,
                    0,
                    TREE_ENVIRONMENT_CELL_EDGE_METERS,
                )?;
                let environment_key = (environment_cell.x, environment_cell.z);
                let environment = if let Some(environment) = environments.get(&environment_key) {
                    *environment
                } else {
                    let environment_x = (index_as_f64(environment_cell.x) + 0.5)
                        * TREE_ENVIRONMENT_CELL_EDGE_METERS;
                    let environment_z = (index_as_f64(environment_cell.z) + 0.5)
                        * TREE_ENVIRONMENT_CELL_EDGE_METERS;
                    let climate_sample = climate.sample(environment_x, environment_z)?;
                    let environment = TreeCellEnvironment {
                        forest: forest.sample(environment_x, environment_z)?,
                        soil: soil.sample(environment_x, environment_z)?,
                        prevailing_wind: climate_sample.prevailing_wind,
                    };
                    environments.insert(environment_key, environment);
                    environment
                };
                let cell_key = cell.generation_key(self.world, DOMAIN_TREE_INDIVIDUALS);
                let expected_stems = environment.forest.tree_density_per_hectare
                    * TREE_PLACEMENT_CELL_EDGE_METERS
                    * TREE_PLACEMENT_CELL_EDGE_METERS
                    / 10_000.0;
                let stem_count =
                    stochastic_count(expected_stems, random_fraction(cell_key, 0x0043_4f55_4e54));
                for ordinal in 0..stem_count {
                    let id = stable_hash(&[cell_key, u64::from(ordinal)]);
                    let x = origin_x
                        + (TREE_PLACEMENT_CELL_EDGE_METERS
                            * lerp(0.06, 0.94, random_fraction(id, 0x585f_4a49_5454)));
                    let z = origin_z
                        + (TREE_PLACEMENT_CELL_EDGE_METERS
                            * lerp(0.06, 0.94, random_fraction(id, 0x5a5f_4a49_5454)));
                    if bounds.contains(x, z) {
                        trees.push(tree_individual(
                            id,
                            x,
                            z,
                            environment.forest,
                            environment.soil,
                            environment.prevailing_wind,
                        ));
                    }
                }

                if cell_x == maximum.x {
                    break;
                }
                cell_x = cell_x.checked_add(1)?;
            }
            if cell_z == maximum.z {
                break;
            }
            cell_z = cell_z.checked_add(1)?;
        }
        trees.sort_by_key(|tree| tree.id);
        Some(trees)
    }
}

fn stochastic_count(expected: f64, rounding_fraction: f64) -> u8 {
    let mut count = 0_u8;
    let mut remainder = expected.max(0.0);
    while remainder >= 1.0 && count < 16 {
        count += 1;
        remainder -= 1.0;
    }
    count + u8::from(rounding_fraction < remainder)
}

fn tree_individual(
    id: u64,
    x: f64,
    z: f64,
    forest: ForestSample,
    soil: SoilSample,
    prevailing_wind: [f64; 2],
) -> ProceduralTree {
    let group = select_functional_group(forest.composition, random_fraction(id, 0x0047_524f_5550));
    let genotype = tree_genotype(group, id);
    let regeneration_chance = 0.08 + (forest.disturbance_severity * 0.24);
    let regeneration = random_fraction(id, 0x0052_4547_454e) < regeneration_chance;
    let age_years = if regeneration {
        1.0 + (random_fraction(id, 0x0059_4f55_4e47) * 13.0)
    } else {
        forest.stand_age_years * lerp(0.55, 1.45, random_fraction(id, 0x4147_455f_5641))
    };
    let event_roll = random_fraction(id, 0x0045_5645_4e54);
    let damage_noise = random_fraction(id, 0x4441_4d41_4745);
    let damage_fraction = ((forest.disturbance_severity * lerp(0.55, 1.25, damage_noise))
        + (soil.rock_exposure * genotype.slope_response_fraction * 0.12))
        .clamp(0.0, 1.0);
    let condition = tree_condition(
        age_years,
        damage_fraction,
        event_roll,
        forest.dominant_disturbance,
        forest.disturbance_severity,
    );
    let maturity = libm::sqrt((age_years / 90.0).clamp(0.0, 1.0));
    let condition_height = match condition {
        TreeCondition::Sapling => 0.28,
        TreeCondition::StormBroken => 0.62,
        TreeCondition::Fallen => 0.88,
        TreeCondition::Mature
        | TreeCondition::Ancient
        | TreeCondition::WindDamaged
        | TreeCondition::DeadStanding => 1.0,
    };
    let competition = forest.canopy_cover_fraction * genotype.competition_response_fraction;
    let height_meters = (genotype.mature_height_meters
        * lerp(0.72, 1.28, genotype.height_variation_fraction)
        * lerp(0.22, 1.0, maturity)
        * (1.0 - (competition * 0.12))
        * condition_height)
        .max(0.8);
    let ancient_girth = if condition == TreeCondition::Ancient {
        1.28
    } else {
        1.0
    };
    let trunk_base_radius_meters =
        (height_meters * lerp(0.025, 0.043, 1.0 - genotype.trunk_taper_fraction) * ancient_girth)
            .max(0.045);
    let crown_scale = match genotype.crown_shape {
        CrownShape::Conical => 0.18,
        CrownShape::Columnar => 0.20,
        CrownShape::Rounded => 0.26,
        CrownShape::Spreading => 0.34,
    };
    let crown_radius_meters = (height_meters
        * crown_scale
        * lerp(0.72, 1.12, genotype.leaf_density_fraction)
        * (1.0 - (competition * 0.24)))
        .max(0.25);
    let direction_jitter = [
        (random_fraction(id, 0x004c_4541_4e58) - 0.5) * 0.42,
        (random_fraction(id, 0x004c_4541_4e5a) - 0.5) * 0.42,
    ];
    let lean_direction = normalized_direction([
        prevailing_wind[0] + direction_jitter[0],
        prevailing_wind[1] + direction_jitter[1],
    ]);
    let slope_fraction = (soil.slope / 0.22).clamp(0.0, 1.0);
    let ordinary_lean = (genotype.wind_response_fraction
        * (0.018 + (forest.disturbance_severity * 0.12)))
        + (genotype.slope_response_fraction * slope_fraction * 0.035);
    let lean_fraction = match condition {
        TreeCondition::Fallen => 0.92,
        TreeCondition::WindDamaged => ordinary_lean + 0.12,
        _ => ordinary_lean,
    }
    .clamp(0.0, 0.96);

    ProceduralTree {
        id,
        x,
        z,
        age_years,
        height_meters,
        trunk_base_radius_meters,
        crown_radius_meters,
        lean_direction,
        lean_fraction,
        damage_fraction,
        rotation_turns: random_fraction(id, 0x524f_5441_5445),
        condition,
        genotype,
    }
}

fn tree_condition(
    age_years: f64,
    damage_fraction: f64,
    event_roll: f64,
    disturbance: ForestDisturbance,
    severity: f64,
) -> TreeCondition {
    if age_years < 15.0 {
        return TreeCondition::Sapling;
    }
    if severity > 0.18 && event_roll < severity * 0.10 {
        return TreeCondition::Fallen;
    }
    if event_roll < severity * 0.18 {
        return TreeCondition::DeadStanding;
    }
    if disturbance == ForestDisturbance::Windthrow && event_roll < severity * 0.40 {
        return TreeCondition::StormBroken;
    }
    if age_years > 240.0 && event_roll > 0.36 {
        return TreeCondition::Ancient;
    }
    if damage_fraction > 0.42 {
        return TreeCondition::WindDamaged;
    }
    TreeCondition::Mature
}

fn select_functional_group(composition: ForestComposition, selection: f64) -> TreeFunctionalGroup {
    let mut cumulative = 0.0;
    for group in TreeFunctionalGroup::ALL {
        cumulative += composition.fraction(group);
        if selection <= cumulative {
            return group;
        }
    }
    TreeFunctionalGroup::DryWoodland
}

fn tree_genotype(group: TreeFunctionalGroup, id: u64) -> TreeGenotype {
    let architecture = random_fraction(id, 0x4152_4348);
    let foliage = random_fraction(id, 0x464f_4c49_4147);
    let response = random_fraction(id, 0x5245_5350);
    match group {
        TreeFunctionalGroup::EvergreenNeedleleaf => TreeGenotype {
            functional_group: group,
            mature_height_meters: lerp(23.0, 38.0, architecture),
            height_variation_fraction: random_fraction(id, 0x4845_4947_4854),
            trunk_taper_fraction: lerp(0.64, 0.88, architecture),
            branching_angle_radians: lerp(0.82, 1.28, foliage),
            branch_density_fraction: lerp(0.70, 1.0, foliage),
            crown_shape: CrownShape::Conical,
            leaf_density_fraction: lerp(0.72, 1.0, foliage),
            bark_style: BarkStyle::Scaly,
            slope_response_fraction: lerp(0.34, 0.58, response),
            wind_response_fraction: lerp(0.52, 0.82, response),
            competition_response_fraction: lerp(0.66, 0.90, architecture),
        },
        TreeFunctionalGroup::ColdDeciduous => TreeGenotype {
            functional_group: group,
            mature_height_meters: lerp(17.0, 29.0, architecture),
            height_variation_fraction: random_fraction(id, 0x4845_4947_4854),
            trunk_taper_fraction: lerp(0.48, 0.72, architecture),
            branching_angle_radians: lerp(0.70, 1.16, foliage),
            branch_density_fraction: lerp(0.48, 0.82, foliage),
            crown_shape: CrownShape::Columnar,
            leaf_density_fraction: lerp(0.50, 0.86, foliage),
            bark_style: BarkStyle::Smooth,
            slope_response_fraction: lerp(0.40, 0.68, response),
            wind_response_fraction: lerp(0.58, 0.88, response),
            competition_response_fraction: lerp(0.58, 0.84, architecture),
        },
        TreeFunctionalGroup::TemperateBroadleaf => TreeGenotype {
            functional_group: group,
            mature_height_meters: lerp(21.0, 35.0, architecture),
            height_variation_fraction: random_fraction(id, 0x4845_4947_4854),
            trunk_taper_fraction: lerp(0.38, 0.64, architecture),
            branching_angle_radians: lerp(0.62, 1.08, foliage),
            branch_density_fraction: lerp(0.56, 0.92, foliage),
            crown_shape: CrownShape::Rounded,
            leaf_density_fraction: lerp(0.62, 1.0, foliage),
            bark_style: BarkStyle::Furrowed,
            slope_response_fraction: lerp(0.28, 0.52, response),
            wind_response_fraction: lerp(0.36, 0.68, response),
            competition_response_fraction: lerp(0.62, 0.92, architecture),
        },
        TreeFunctionalGroup::DryWoodland => TreeGenotype {
            functional_group: group,
            mature_height_meters: lerp(7.0, 18.0, architecture),
            height_variation_fraction: random_fraction(id, 0x4845_4947_4854),
            trunk_taper_fraction: lerp(0.30, 0.58, architecture),
            branching_angle_radians: lerp(0.48, 0.92, foliage),
            branch_density_fraction: lerp(0.30, 0.68, foliage),
            crown_shape: CrownShape::Spreading,
            leaf_density_fraction: lerp(0.26, 0.68, foliage),
            bark_style: BarkStyle::Plated,
            slope_response_fraction: lerp(0.46, 0.78, response),
            wind_response_fraction: lerp(0.42, 0.76, response),
            competition_response_fraction: lerp(0.24, 0.58, architecture),
        },
    }
}

fn normalized_direction(direction: [f64; 2]) -> [f64; 2] {
    let length = libm::hypot(direction[0], direction[1]);
    if length <= f64::EPSILON {
        [1.0, 0.0]
    } else {
        [direction[0] / length, direction[1] / length]
    }
}

fn random_fraction(key: u64, lane: u64) -> f64 {
    hash53_as_f64(stable_hash(&[key, lane]) >> 11) / 9_007_199_254_740_991.0
}

fn sample_forest_history(
    world: WorldIdentity,
    x: f64,
    z: f64,
    environment: HistoryEnvironment,
) -> Option<ForestHistory> {
    let hazards = [
        (
            ForestDisturbance::Fire,
            (value_field(world, DOMAIN_FIRE_HISTORY, x, z, FOREST_HISTORY_EDGE_METERS)? * 0.55)
                + (environment.dryness * 0.30)
                + (environment.warmth * 0.15),
        ),
        (
            ForestDisturbance::Windthrow,
            (value_field(
                world,
                DOMAIN_WINDTHROW_HISTORY,
                x,
                z,
                FOREST_HISTORY_EDGE_METERS,
            )? * 0.55)
                + (environment.precipitation * 0.20)
                + (environment.slope * 0.25),
        ),
        (
            ForestDisturbance::Flood,
            (value_field(
                world,
                DOMAIN_FLOOD_HISTORY,
                x,
                z,
                FOREST_HISTORY_EDGE_METERS,
            )? * 0.50)
                + (environment.surface_moisture * 0.38)
                + ((1.0 - environment.slope) * 0.12),
        ),
        (
            ForestDisturbance::Landslide,
            (value_field(
                world,
                DOMAIN_LANDSLIDE_HISTORY,
                x,
                z,
                FOREST_HISTORY_EDGE_METERS,
            )? * 0.45)
                + (environment.slope * 0.45)
                + (environment.precipitation * 0.10),
        ),
    ];
    let mut dominant = hazards[0];
    for hazard in hazards.into_iter().skip(1) {
        if hazard.1 > dominant.1 {
            dominant = hazard;
        }
    }
    let severity = ((dominant.1 - 0.48) / 0.52).clamp(0.0, 1.0);
    let age_control = value_field(world, DOMAIN_STAND_AGE, x, z, FOREST_HISTORY_EDGE_METERS)?;
    Some(ForestHistory {
        stand_age_years: (8.0 + (age_control * 442.0)) * (1.0 - (severity * 0.88)),
        severity,
        dominant_disturbance: dominant.0,
    })
}

fn forest_composition(
    environment: Environment,
    permanent_snow_fraction: f64,
    disturbance_severity: f64,
) -> (ForestComposition, f64) {
    let mut scores = TreeFunctionalGroup::ALL.map(|group| {
        let succession_response = match group {
            TreeFunctionalGroup::EvergreenNeedleleaf => 1.0 - (disturbance_severity * 0.18),
            TreeFunctionalGroup::ColdDeciduous => 0.82 + (disturbance_severity * 0.18),
            TreeFunctionalGroup::TemperateBroadleaf => 1.0 - (disturbance_severity * 0.42),
            TreeFunctionalGroup::DryWoodland => 0.88 + (disturbance_severity * 0.12),
        };
        group.preference().suitability(environment)
            * succession_response
            * (1.0 - (permanent_snow_fraction * 0.92))
    });
    let score_total = scores.iter().sum::<f64>();
    if score_total <= f64::EPSILON {
        scores = [0.25; 4];
    } else {
        for score in &mut scores {
            *score /= score_total;
        }
    }
    let best_suitability = TreeFunctionalGroup::ALL
        .into_iter()
        .map(|group| group.preference().suitability(environment))
        .fold(0.0, f64::max);
    (
        ForestComposition {
            evergreen_needleleaf_fraction: scores[0],
            cold_deciduous_fraction: scores[1],
            temperate_broadleaf_fraction: scores[2],
            dry_woodland_fraction: scores[3],
        },
        best_suitability,
    )
}

fn forest_composition_v18(
    environment: Environment,
    permanent_snow_fraction: f64,
    disturbance_severity: f64,
    ecosystem: EcosystemSample,
    province: &ProvinceSample,
) -> (ForestComposition, f64) {
    let memory = province.ecological_memory;
    let memory_bands = [
        1.0 - smoothstep(((memory - 0.08) / 0.34).clamp(0.0, 1.0)),
        triangular(memory, 0.36, 0.34),
        triangular(memory, 0.68, 0.34),
        smoothstep(((memory - 0.62) / 0.32).clamp(0.0, 1.0)),
    ];
    let regional_identity = [
        (0.16
            + ((1.0 - environment.temperature) * 0.28)
            + (province.mountain * 0.18)
            + (province.glacial * 0.12)
            + (memory_bands[0] * 0.26))
            .clamp(0.0, 1.0),
        (0.14
            + ((1.0 - environment.temperature) * 0.20)
            + (environment.moisture * 0.18)
            + (province.crust_age * 0.12)
            + (memory_bands[1] * 0.36))
            .clamp(0.0, 1.0),
        (0.10
            + (environment.temperature * 0.20)
            + (environment.moisture * 0.26)
            + (province.sediment * 0.10)
            + (memory_bands[2] * 0.34))
            .clamp(0.0, 1.0),
        (0.12
            + (province.aridity * 0.22)
            + (ecosystem.open_woodland_potential * 0.18)
            + (ecosystem.shrubland_potential * 0.14)
            + (memory_bands[3] * 0.34))
            .clamp(0.0, 1.0),
    ];
    let mut scores = TreeFunctionalGroup::ALL.map(|group| {
        let index = match group {
            TreeFunctionalGroup::EvergreenNeedleleaf => 0,
            TreeFunctionalGroup::ColdDeciduous => 1,
            TreeFunctionalGroup::TemperateBroadleaf => 2,
            TreeFunctionalGroup::DryWoodland => 3,
        };
        let succession_response = match group {
            TreeFunctionalGroup::EvergreenNeedleleaf => 1.0 - (disturbance_severity * 0.18),
            TreeFunctionalGroup::ColdDeciduous => 0.82 + (disturbance_severity * 0.18),
            TreeFunctionalGroup::TemperateBroadleaf => 1.0 - (disturbance_severity * 0.42),
            TreeFunctionalGroup::DryWoodland => 0.88 + (disturbance_severity * 0.12),
        };
        group.preference().suitability(environment)
            * succession_response
            * (1.0 - (permanent_snow_fraction * 0.92))
            * (0.025 + (regional_identity[index] * regional_identity[index] * 2.40))
    });
    let score_total = scores.iter().sum::<f64>();
    if score_total <= f64::EPSILON {
        scores = [0.25; 4];
    } else {
        for score in &mut scores {
            *score /= score_total;
        }
    }
    let tree_envelope = (ecosystem.closed_forest_potential
        + (ecosystem.open_woodland_potential * 0.56))
        .clamp(0.0, 1.0);
    let best_suitability = TreeFunctionalGroup::ALL
        .into_iter()
        .map(|group| group.preference().suitability(environment))
        .fold(0.0, f64::max)
        * tree_envelope;
    (
        ForestComposition {
            evergreen_needleleaf_fraction: scores[0],
            cold_deciduous_fraction: scores[1],
            temperate_broadleaf_fraction: scores[2],
            dry_woodland_fraction: scores[3],
        },
        best_suitability,
    )
}

fn triangular(value: f64, center: f64, half_width: f64) -> f64 {
    (1.0 - ((value - center).abs() / half_width)).clamp(0.0, 1.0)
}

fn value_field(world: WorldIdentity, domain: u64, x: f64, z: f64, edge: f64) -> Option<f64> {
    let cell = CellIndex::containing(x, z, 0, edge)?;
    let right_x = cell.x.checked_add(1)?;
    let top_z = cell.z.checked_add(1)?;
    let local_x = (x / edge) - index_as_f64(cell.x);
    let local_z = (z / edge) - index_as_f64(cell.z);
    let blend_x = smoothstep(local_x);
    let blend_z = smoothstep(local_z);
    let bottom = lerp(
        field_corner(world, domain, cell.x, cell.z),
        field_corner(world, domain, right_x, cell.z),
        blend_x,
    );
    let top = lerp(
        field_corner(world, domain, cell.x, top_z),
        field_corner(world, domain, right_x, top_z),
        blend_x,
    );
    Some(lerp(bottom, top, blend_z))
}

fn field_corner(world: WorldIdentity, domain: u64, x: i64, z: i64) -> f64 {
    let hash = CellIndex::new(x, z, 0).generation_key(world, domain);
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

    const TEST_WORLD: WorldIdentity = WorldIdentity::new(0x5eed, SOIL_GENERATOR_VERSION, 0);
    const FOREST_WORLD: WorldIdentity = WorldIdentity::new(0x5eed, FOREST_GENERATOR_VERSION, 0);
    const TREE_WORLD: WorldIdentity = WorldIdentity::new(
        0x5eed,
        treeline_coordinates::DETERMINISTIC_MATH_GENERATOR_VERSION,
        0,
    );

    #[test]
    fn soil_is_deterministic_bounded_and_composition_is_conserved() {
        let soil = Soil::new(TEST_WORLD);
        let first = soil.sample(-73_125.0, 19_875.0).expect("finite soil");
        let second = soil.sample(-73_125.0, 19_875.0).expect("same soil");
        let mineral_total = first.composition.sand_fraction
            + first.composition.silt_fraction
            + first.composition.clay_fraction;

        assert_eq!(first, second);
        assert!((mineral_total - 1.0).abs() < 1.0e-12);
        assert!((0.0..=1.0).contains(&first.surface_moisture));
        assert!((3.5..=8.5).contains(&first.acidity_ph));
        assert!((0.01..=0.17).contains(&first.organic_matter_fraction));
    }

    #[test]
    fn soil_is_continuous_across_negative_regional_boundaries() {
        let soil = Soil::new(TEST_WORLD);
        let left = soil.sample(-100_000.01, -12_000.0).expect("left soil");
        let right = soil.sample(-99_999.99, -12_000.0).expect("right soil");

        assert!((left.surface_moisture - right.surface_moisture).abs() < 0.01);
        assert!((left.acidity_ph - right.acidity_ph).abs() < 0.01);
        assert!((left.composition.clay_fraction - right.composition.clay_fraction).abs() < 0.01);
    }

    #[test]
    fn soil_requires_its_generator_contract() {
        let old_world = WorldIdentity::new(0x5eed, SOIL_GENERATOR_VERSION - 1, 0);
        assert!(Soil::new(old_world).sample(0.0, 0.0).is_none());
    }

    #[test]
    fn suitability_responds_to_generated_soil_conditions() {
        let soil = Soil::new(TEST_WORLD)
            .sample(-73_125.0, 19_875.0)
            .expect("soil");
        let environment = Environment::from_soil(0.4, 0.5, 0.1, soil);
        let matching = SpeciesPreference {
            temperature: environment.temperature,
            moisture: environment.moisture,
            soil_depth: environment.soil_depth,
            soil_acidity: environment.soil_acidity,
            sand_fraction: environment.sand_fraction,
            clay_fraction: environment.clay_fraction,
            rock_exposure: environment.rock_exposure,
            tolerance: 0.5,
        };
        let alkaline_mismatch = SpeciesPreference {
            soil_acidity: 0.0,
            ..matching
        };

        assert!((matching.suitability(environment) - 1.0).abs() < f64::EPSILON);
        assert!(alkaline_mismatch.suitability(environment) < matching.suitability(environment));
    }

    #[test]
    fn soil_has_a_golden_fingerprint() {
        let soil = Soil::new(TEST_WORLD);
        let samples = [
            (-73_125.0, 19_875.0),
            (0.0, 0.0),
            (128_000.0, -96_000.0),
            (-240_500.0, -300_250.0),
        ];
        let fingerprint = stable_hash(
            &samples
                .into_iter()
                .flat_map(|(x, z)| {
                    let sample = soil.sample(x, z).expect("finite");
                    [
                        sample.composition.sand_fraction.to_bits(),
                        sample.composition.silt_fraction.to_bits(),
                        sample.composition.clay_fraction.to_bits(),
                        sample.depth_meters.to_bits(),
                        sample.surface_moisture.to_bits(),
                        sample.acidity_ph.to_bits(),
                        sample.organic_matter_fraction.to_bits(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            fingerprint, 7_753_859_854_880_270_456,
            "changing this value changes generated soil profiles"
        );
    }

    #[test]
    fn forest_is_deterministic_bounded_and_composition_is_conserved() {
        let forest = ForestDistribution::new(FOREST_WORLD);
        let first = forest.sample(-73_125.0, 19_875.0).expect("finite forest");
        let second = forest.sample(-73_125.0, 19_875.0).expect("same forest");
        let composition_total = TreeFunctionalGroup::ALL
            .into_iter()
            .map(|group| first.composition.fraction(group))
            .sum::<f64>();

        assert_eq!(first, second);
        assert!((composition_total - 1.0).abs() < 1.0e-12);
        assert!((0.0..=1.0).contains(&first.canopy_cover_fraction));
        assert!((0.0..=1.0).contains(&first.disturbance_severity));
        assert!(first.tree_density_per_hectare >= 0.0);
        assert!(first.aboveground_biomass_kg_per_square_meter >= 0.0);
        assert!((0.0..=31.0).contains(&first.mean_canopy_height_meters));
        assert!(first.stand_age_years >= 0.0);
    }

    #[test]
    fn forest_is_continuous_across_negative_stand_boundaries() {
        let forest = ForestDistribution::new(FOREST_WORLD);
        let left = forest.sample(-12_000.01, -4_000.0).expect("left forest");
        let right = forest.sample(-11_999.99, -4_000.0).expect("right forest");

        assert!((left.canopy_cover_fraction - right.canopy_cover_fraction).abs() < 0.01);
        assert!(
            (left.composition.evergreen_needleleaf_fraction
                - right.composition.evergreen_needleleaf_fraction)
                .abs()
                < 0.01
        );
        assert!((left.stand_age_years - right.stand_age_years).abs() < 0.01);
    }

    #[test]
    fn forest_sampling_is_generation_order_independent() {
        let forest = ForestDistribution::new(FOREST_WORLD);
        let positions = [
            (-73_125.0, 19_875.0),
            (0.0, 0.0),
            (128_000.0, -96_000.0),
            (-240_500.0, -300_250.0),
        ];
        let forward = positions.map(|(x, z)| forest.sample(x, z).expect("forward sample"));
        let mut reverse_positions = positions;
        reverse_positions.reverse();
        let mut reverse =
            reverse_positions.map(|(x, z)| forest.sample(x, z).expect("reverse sample"));
        reverse.reverse();

        assert_eq!(forward, reverse);
    }

    #[test]
    fn forest_requires_its_generator_contract() {
        let old_world = WorldIdentity::new(0x5eed, FOREST_GENERATOR_VERSION - 1, 0);
        assert!(
            ForestDistribution::new(old_world)
                .sample(0.0, 0.0)
                .is_none()
        );
    }

    #[test]
    fn distant_forests_have_distinct_structure_and_composition() {
        let forest = ForestDistribution::new(FOREST_WORLD);
        let samples = [
            forest.sample(-820_000.0, -640_000.0).expect("forest"),
            forest.sample(-240_000.0, 510_000.0).expect("forest"),
            forest.sample(370_000.0, -760_000.0).expect("forest"),
            forest.sample(910_000.0, 430_000.0).expect("forest"),
        ];
        let canopy_range = samples
            .iter()
            .map(|sample| sample.canopy_cover_fraction)
            .fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
            );
        let evergreen_range = samples
            .iter()
            .map(|sample| sample.composition.evergreen_needleleaf_fraction)
            .fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
            );

        assert!(canopy_range.1 - canopy_range.0 > 0.05);
        assert!(evergreen_range.1 - evergreen_range.0 > 0.05);
    }

    #[test]
    fn forest_has_a_golden_fingerprint() {
        let forest = ForestDistribution::new(FOREST_WORLD);
        let samples = [
            (-73_125.0, 19_875.0),
            (0.0, 0.0),
            (128_000.0, -96_000.0),
            (-240_500.0, -300_250.0),
        ];
        let fingerprint = stable_hash(
            &samples
                .into_iter()
                .flat_map(|(x, z)| {
                    let sample = forest.sample(x, z).expect("finite");
                    [
                        sample.canopy_cover_fraction.to_bits(),
                        sample.tree_density_per_hectare.to_bits(),
                        sample.aboveground_biomass_kg_per_square_meter.to_bits(),
                        sample.mean_canopy_height_meters.to_bits(),
                        sample.stand_age_years.to_bits(),
                        sample.disturbance_severity.to_bits(),
                        sample.composition.evergreen_needleleaf_fraction.to_bits(),
                        sample.composition.cold_deciduous_fraction.to_bits(),
                        sample.composition.temperate_broadleaf_fraction.to_bits(),
                        sample.composition.dry_woodland_fraction.to_bits(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            fingerprint, 12_136_453_503_781_334_745,
            "changing this value changes generated forest distributions"
        );
    }

    #[test]
    fn procedural_trees_are_deterministic_bounded_and_architecturally_valid() {
        let generator = ProceduralTrees::new(TREE_WORLD);
        let bounds = TreeBounds::new(-96.0, -96.0, 96.0, 96.0).expect("valid bounds");
        let first = generator.trees_in(bounds).expect("tree generation");
        let second = generator.trees_in(bounds).expect("same tree generation");

        assert_eq!(first, second);
        assert!(!first.is_empty());
        assert!(first.iter().all(|tree| {
            bounds.contains(tree.x, tree.z)
                && tree.height_meters >= 0.8
                && tree.trunk_base_radius_meters >= 0.045
                && tree.crown_radius_meters >= 0.25
                && (0.0..=1.0).contains(&tree.damage_fraction)
                && (0.0..=0.96).contains(&tree.lean_fraction)
                && (libm::hypot(tree.lean_direction[0], tree.lean_direction[1]) - 1.0).abs()
                    < 1.0e-12
        }));
        assert!(first.windows(2).all(|pair| pair[0].id <= pair[1].id));
    }

    #[test]
    fn adjacent_tree_requests_match_one_combined_request_at_negative_boundaries() {
        let generator = ProceduralTrees::new(TREE_WORLD);
        let combined = TreeBounds::new(-64.0, -64.0, 64.0, 64.0).expect("combined bounds");
        let mut tiled = [
            TreeBounds::new(-64.0, -64.0, 0.0, 0.0).expect("southwest"),
            TreeBounds::new(0.0, -64.0, 64.0, 0.0).expect("southeast"),
            TreeBounds::new(-64.0, 0.0, 0.0, 64.0).expect("northwest"),
            TreeBounds::new(0.0, 0.0, 64.0, 64.0).expect("northeast"),
        ]
        .into_iter()
        .rev()
        .flat_map(|bounds| generator.trees_in(bounds).expect("tile generation"))
        .collect::<Vec<_>>();
        tiled.sort_by_key(|tree| tree.id);

        assert_eq!(
            generator.trees_in(combined).expect("combined generation"),
            tiled
        );
    }

    #[test]
    fn tree_direction_normalization_has_stable_bits() {
        let direction = normalized_direction([0.61, -0.92]);

        assert_eq!(direction[0].to_bits(), 4_603_152_668_769_828_234);
        assert_eq!(direction[1].to_bits(), 13_829_054_229_001_015_622);
    }

    #[test]
    fn tree_generation_requires_its_versioned_contract() {
        let old_world = WorldIdentity::new(0x5eed, TREE_GENERATOR_VERSION - 1, 0);
        let bounds = TreeBounds::new(0.0, 0.0, 32.0, 32.0).expect("valid bounds");

        assert!(ProceduralTrees::new(old_world).trees_in(bounds).is_none());
    }

    #[test]
    fn distant_tree_stands_use_distinct_grammars_and_life_histories() {
        let generator = ProceduralTrees::new(TREE_WORLD);
        let mut trees = Vec::new();
        for [x, z] in [
            [-820_000.0, -640_000.0],
            [-240_000.0, 510_000.0],
            [370_000.0, -760_000.0],
            [910_000.0, 430_000.0],
        ] {
            trees.extend(
                generator
                    .trees_in(TreeBounds::new(x, z, x + 96.0, z + 96.0).expect("valid stand"))
                    .expect("stand generation"),
            );
        }

        assert!(!trees.is_empty());
        let first_group = trees[0].genotype.functional_group;
        let first_shape = trees[0].genotype.crown_shape;
        assert!(
            trees
                .iter()
                .any(|tree| tree.genotype.functional_group != first_group)
        );
        assert!(
            trees
                .iter()
                .any(|tree| tree.genotype.crown_shape != first_shape)
        );
        assert!(
            trees
                .iter()
                .any(|tree| tree.condition != TreeCondition::Mature)
        );
    }

    #[test]
    fn version_seventeen_ecology_retains_its_pre_reset_contract() {
        let world = WorldIdentity::new(0x5eed, ECOSYSTEM_GENERATOR_VERSION - 1, 0);
        let hydrology = WetlandHydrology::new(1.5, 0.12, 0.68, 14.0).expect("valid hydrology");
        let words = [
            [-91_125.0, -37_375.0],
            [-64_250.0, 63_875.0],
            [28_625.0, -52_375.0],
        ]
        .into_iter()
        .flat_map(|[x, z]| {
            let soil = Soil::new(world).sample(x, z).expect("soil");
            let forest = ForestDistribution::new(world).sample(x, z).expect("forest");
            let ground = GroundVegetationDistribution::new(world)
                .sample(x, z)
                .expect("ground vegetation");
            let wetland = WetlandDistribution::new(world)
                .sample(x, z, hydrology)
                .expect("wetland");
            [
                soil.composition.sand_fraction.to_bits(),
                soil.composition.silt_fraction.to_bits(),
                soil.composition.clay_fraction.to_bits(),
                soil.surface_moisture.to_bits(),
                soil.acidity_ph.to_bits(),
                forest.canopy_cover_fraction.to_bits(),
                forest.tree_density_per_hectare.to_bits(),
                forest.composition.evergreen_needleleaf_fraction.to_bits(),
                forest.composition.cold_deciduous_fraction.to_bits(),
                forest.composition.temperate_broadleaf_fraction.to_bits(),
                forest.composition.dry_woodland_fraction.to_bits(),
                ground.ground_cover_fraction.to_bits(),
                ground.composition.graminoid_fraction.to_bits(),
                ground.composition.low_shrub_fraction.to_bits(),
                ground.composition.moss_fraction.to_bits(),
                wetland.coverage_fraction.to_bits(),
                wetland.salinity_fraction.to_bits(),
            ]
        })
        .collect::<Vec<_>>();

        assert_eq!(
            stable_hash(&words),
            3_251_190_482_585_119_889,
            "changing this value changes the frozen generator version 17 ecology contract"
        );
    }

    #[test]
    fn version_eighteen_produces_large_contiguous_open_landscapes() {
        let world = WorldIdentity::new(0x5eed, ECOSYSTEM_GENERATOR_VERSION, 0);
        let ecosystem = EcosystemDistribution::new(world);
        let forest = ForestDistribution::new(world);
        let mut best_center = [0.0, 0.0];
        let mut best_open_excess = f64::NEG_INFINITY;
        for z in -8..=8 {
            for x in -8..=8 {
                let center = [f64::from(x) * 384_000.0, f64::from(z) * 384_000.0];
                let sample = ecosystem.sample(center[0], center[1]).expect("ecosystem");
                let open_excess = sample.open_land_potential()
                    - sample.closed_forest_potential
                    - (sample.open_woodland_potential * 0.35);
                if open_excess > best_open_excess {
                    best_open_excess = open_excess;
                    best_center = center;
                }
            }
        }
        assert!(
            best_open_excess > 0.18,
            "best open-land excess was only {best_open_excess}"
        );

        let mut open_samples = 0;
        for z in -2..=2 {
            for x in -2..=2 {
                let sample = forest
                    .sample(
                        best_center[0] + (f64::from(x) * 8_000.0),
                        best_center[1] + (f64::from(z) * 8_000.0),
                    )
                    .expect("forest");
                open_samples += usize::from(sample.canopy_cover_fraction < 0.20);
            }
        }
        assert!(
            open_samples >= 20,
            "only {open_samples}/25 nearby samples were genuinely open"
        );
    }

    #[test]
    fn version_eighteen_mesic_causes_close_broad_forests_without_erasing_open_country() {
        let world = WorldIdentity::new(0x5eed, ECOSYSTEM_GENERATOR_VERSION, 0);
        let ecosystem = EcosystemDistribution::new(world);
        let forest = ForestDistribution::new(world);
        let mesic_centers = [
            [3_072_000.0, 1_920_000.0],
            [-3_072_000.0, -1_152_000.0],
            [-768_000.0, -3_072_000.0],
        ];
        let mut minimum_mesic_canopy = 1.0_f64;
        for center in mesic_centers {
            let mut closed_forest = 0.0;
            let mut open_land = 0.0;
            let mut water_balance = 0.0;
            let mut canopy = 0.0;
            let mut visibly_closed = 0;
            for patch_z in -2..=2 {
                for patch_x in -2..=2 {
                    let x = center[0] + (f64::from(patch_x) * 8_000.0);
                    let z = center[1] + (f64::from(patch_z) * 8_000.0);
                    let causes = ecosystem.sample(x, z).expect("mesic ecosystem");
                    let trees = forest.sample(x, z).expect("mesic forest");
                    closed_forest += causes.closed_forest_potential;
                    open_land += causes.open_land_potential();
                    water_balance += causes.water_balance_fraction;
                    canopy += trees.canopy_cover_fraction;
                    visibly_closed += usize::from(
                        trees.canopy_cover_fraction >= 0.48
                            && trees.mean_canopy_height_meters >= 8.0,
                    );
                }
            }
            let mean_closed_forest = closed_forest / 25.0;
            let mean_open_land = open_land / 25.0;
            let mean_water_balance = water_balance / 25.0;
            let mean_canopy = canopy / 25.0;
            minimum_mesic_canopy = minimum_mesic_canopy.min(mean_canopy);
            assert!(
                mean_closed_forest >= 0.58
                    && mean_closed_forest >= mean_open_land + 0.24
                    && mean_water_balance >= 0.82,
                "site {center:?} was not causally mesic: forest {mean_closed_forest}, \
                 open {mean_open_land}, water {mean_water_balance}"
            );
            assert!(
                mean_canopy >= 0.58 && visibly_closed >= 20,
                "site {center:?} only reached canopy {mean_canopy} with \
                 {visibly_closed}/25 visibly closed samples"
            );
        }

        let mut best_open_center = [0.0, 0.0];
        let mut best_open_excess = f64::NEG_INFINITY;
        for z in -8..=8 {
            for x in -8..=8 {
                let position = [f64::from(x) * 384_000.0, f64::from(z) * 384_000.0];
                let causes = ecosystem
                    .sample(position[0], position[1])
                    .expect("ecosystem");
                let open_excess = causes.open_land_potential()
                    - causes.closed_forest_potential
                    - (causes.open_woodland_potential * 0.35);
                if open_excess > best_open_excess {
                    best_open_excess = open_excess;
                    best_open_center = position;
                }
            }
        }
        let mut open_canopy = 0.0;
        let mut genuinely_open = 0;
        for patch_z in -2..=2 {
            for patch_x in -2..=2 {
                let trees = forest
                    .sample(
                        best_open_center[0] + (f64::from(patch_x) * 8_000.0),
                        best_open_center[1] + (f64::from(patch_z) * 8_000.0),
                    )
                    .expect("open-country forest sample");
                open_canopy += trees.canopy_cover_fraction;
                genuinely_open += usize::from(trees.canopy_cover_fraction < 0.20);
            }
        }
        let mean_open_canopy = open_canopy / 25.0;
        assert!(best_open_excess > 0.18, "{best_open_excess}");
        assert!(
            genuinely_open >= 20
                && mean_open_canopy < 0.20
                && minimum_mesic_canopy >= mean_open_canopy + 0.38,
            "open contrast failed: {genuinely_open}/25 open, open canopy \
             {mean_open_canopy}, weakest mesic canopy {minimum_mesic_canopy}"
        );
    }

    #[test]
    fn version_eighteen_forests_have_strong_regional_tree_group_identities() {
        let world = WorldIdentity::new(0x5eed, ECOSYSTEM_GENERATOR_VERSION, 0);
        let forest = ForestDistribution::new(world);
        let mut maxima = [0.0_f64; 4];
        for z in -8..=8 {
            for x in -8..=8 {
                let sample = forest
                    .sample(f64::from(x) * 384_000.0, f64::from(z) * 384_000.0)
                    .expect("forest");
                let fractions = [
                    sample.composition.evergreen_needleleaf_fraction,
                    sample.composition.cold_deciduous_fraction,
                    sample.composition.temperate_broadleaf_fraction,
                    sample.composition.dry_woodland_fraction,
                ];
                for (maximum, fraction) in maxima.iter_mut().zip(fractions) {
                    *maximum = maximum.max(fraction);
                }
            }
        }
        for (group, maximum) in maxima.into_iter().enumerate() {
            assert!(
                maximum > 0.55,
                "tree functional group {group} peaked at only {maximum}"
            );
        }
    }

    #[test]
    fn version_eighteen_deep_ocean_has_no_terrestrial_vegetation() {
        let world = WorldIdentity::new(0x5eed, ECOSYSTEM_GENERATOR_VERSION, 0);
        let mut ocean = None;
        'outer: for z in -12..=12 {
            for x in -12..=12 {
                let position = [f64::from(x) * 512_000.0, f64::from(z) * 512_000.0];
                let ecosystem = EcosystemDistribution::new(world)
                    .sample(position[0], position[1])
                    .expect("ecosystem");
                if ecosystem.land_fraction <= 1.0e-6 {
                    ocean = Some(position);
                    break 'outer;
                }
            }
        }
        let [x, z] = ocean.expect("survey contains deep ocean");
        let forest = ForestDistribution::new(world).sample(x, z).expect("forest");
        let ground = GroundVegetationDistribution::new(world)
            .sample(x, z)
            .expect("ground");

        assert_eq!(forest.canopy_cover_fraction.to_bits(), 0.0_f64.to_bits());
        assert_eq!(forest.tree_density_per_hectare.to_bits(), 0.0_f64.to_bits());
        assert_eq!(ground.ground_cover_fraction.to_bits(), 0.0_f64.to_bits());
        assert_eq!(
            ground.patch_density_per_hectare.to_bits(),
            0.0_f64.to_bits()
        );
    }

    #[test]
    fn procedural_trees_have_a_golden_fingerprint() {
        let generator = ProceduralTrees::new(TREE_WORLD);
        let bounds = TreeBounds::new(-96.0, -64.0, 96.0, 64.0).expect("valid bounds");
        let trees = generator.trees_in(bounds).expect("tree generation");
        let fingerprint = stable_hash(
            &trees
                .iter()
                .flat_map(|tree| {
                    [
                        tree.id,
                        tree.x.to_bits(),
                        tree.z.to_bits(),
                        tree.age_years.to_bits(),
                        tree.height_meters.to_bits(),
                        tree.trunk_base_radius_meters.to_bits(),
                        tree.crown_radius_meters.to_bits(),
                        tree.lean_direction[0].to_bits(),
                        tree.lean_direction[1].to_bits(),
                        tree.lean_fraction.to_bits(),
                        tree.damage_fraction.to_bits(),
                        tree.rotation_turns.to_bits(),
                        tree_condition_fingerprint(tree.condition),
                        tree_group_fingerprint(tree.genotype.functional_group),
                        tree.genotype.mature_height_meters.to_bits(),
                        tree.genotype.branching_angle_radians.to_bits(),
                        tree.genotype.branch_density_fraction.to_bits(),
                        tree.genotype.leaf_density_fraction.to_bits(),
                    ]
                })
                .collect::<Vec<_>>(),
        );

        assert_eq!(
            fingerprint, 14_498_776_166_268_426_461,
            "changing this value changes generated procedural trees"
        );
    }

    const fn tree_condition_fingerprint(condition: TreeCondition) -> u64 {
        match condition {
            TreeCondition::Sapling => 0,
            TreeCondition::Mature => 1,
            TreeCondition::Ancient => 2,
            TreeCondition::WindDamaged => 3,
            TreeCondition::Fallen => 4,
            TreeCondition::DeadStanding => 5,
            TreeCondition::StormBroken => 6,
        }
    }

    const fn tree_group_fingerprint(group: TreeFunctionalGroup) -> u64 {
        match group {
            TreeFunctionalGroup::EvergreenNeedleleaf => 0,
            TreeFunctionalGroup::ColdDeciduous => 1,
            TreeFunctionalGroup::TemperateBroadleaf => 2,
            TreeFunctionalGroup::DryWoodland => 3,
        }
    }
}
