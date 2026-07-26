//! Ecosystem primitives built from environmental variables rather than biome IDs.

use treeline_coordinates::{CellIndex, WorldIdentity};
use treeline_geography::{Climate, RegionalProfile};
use treeline_terrain::WildernessTerrain;

/// Generator version that first exposes deterministic soil profiles.
pub const SOIL_GENERATOR_VERSION: u32 = 8;
/// Generator version that first exposes deterministic forest distributions.
pub const FOREST_GENERATOR_VERSION: u32 = 9;

const DOMAIN_FOREST_PATCHES: u64 = 0x464f_5245_5354_5041;
const DOMAIN_FOREST_STANDS: u64 = 0x464f_5245_5354_5354;
const DOMAIN_STAND_AGE: u64 = 0x5354_414e_445f_4147;
const DOMAIN_FIRE_HISTORY: u64 = 0x4649_5245_4849_5354;
const DOMAIN_WINDTHROW_HISTORY: u64 = 0x5749_4e44_5448_524f;
const DOMAIN_FLOOD_HISTORY: u64 = 0x464c_4f4f_445f_4849;
const DOMAIN_LANDSLIDE_HISTORY: u64 = 0x4c41_4e44_534c_4944;
const FOREST_PATCH_EDGE_METERS: f64 = 2_000.0;
const FOREST_STAND_EDGE_METERS: f64 = 12_000.0;
const FOREST_HISTORY_EDGE_METERS: f64 = 32_000.0;

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
    pub fn sample(self, x: f64, z: f64) -> Option<SoilSample> {
        if self.world.generator_version < SOIL_GENERATOR_VERSION {
            return None;
        }

        let profile = RegionalProfile::sample(self.world, x, z)?;
        let erosion = WildernessTerrain::new(self.world).erosion_at(x, z)?;
        let climate = Climate::new(self.world).sample(x, z)?;
        let softness = 1.0 - profile.rock_hardness;
        let deposition = (erosion.sediment_deposition_meters / 18.0).clamp(0.0, 1.0);

        let sand_weight = 0.2 + (profile.rock_hardness * 0.45) + (erosion.scree_cover * 0.3);
        let silt_weight = 0.18 + (deposition * 0.55) + (profile.erosion_age * 0.22);
        let clay_weight = 0.12
            + (softness * 0.38)
            + (profile.erosion_age * 0.25)
            + (profile.karst_probability * 0.08);
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
        let surface_moisture = ((precipitation * 0.62)
            + (snowmelt_fraction * 0.15)
            + (water_holding * 0.35)
            + (depth_fraction * 0.12)
            - (warmth * 0.2)
            - (drainage * 0.32))
            .clamp(0.0, 1.0);

        let acidity_ph = (6.8 + (profile.karst_probability * 1.2) + (profile.rock_hardness * 0.25)
            - (precipitation * 0.95)
            - (organic_fraction * 0.55))
            .clamp(3.5, 8.5);

        Some(SoilSample {
            composition,
            texture: composition.texture(),
            depth_meters: erosion.soil_depth_meters,
            surface_moisture,
            acidity_ph,
            organic_matter_fraction,
            rock_exposure: erosion.rock_exposure,
            slope: erosion.slope,
        })
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
        (1.0 - (squared_distance.sqrt() / self.tolerance)).clamp(0.0, 1.0)
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
    pub fn sample(self, x: f64, z: f64) -> Option<ForestSample> {
        if self.world.generator_version < FOREST_GENERATOR_VERSION {
            return None;
        }

        let climate = Climate::new(self.world).sample(x, z)?;
        let soil = Soil::new(self.world).sample(x, z)?;
        let warmth = climate.warmth_fraction();
        let precipitation = climate.precipitation_fraction();
        let sunlight = (0.88 - (precipitation * 0.28)).clamp(0.0, 1.0);
        let slope_fraction = (soil.slope / 0.22).clamp(0.0, 1.0);
        let dryness = 1.0 - soil.surface_moisture;

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

        let environment = Environment::from_soil(warmth, sunlight, history.severity, soil);
        let permanent_snow_fraction =
            (climate.permanent_snowpack_water_equivalent_millimeters / 1_200.0).clamp(0.0, 1.0);
        let (composition, best_suitability) =
            forest_composition(environment, permanent_snow_fraction, history.severity);
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
        let succession_cover = (history.stand_age_years / 55.0).clamp(0.12, 1.0);
        let canopy_cover_fraction =
            (best_suitability * substrate * (0.42 + (patchiness * 0.72)) * succession_cover)
                .clamp(0.0, 1.0);

        let mature_height_meters = TreeFunctionalGroup::ALL
            .into_iter()
            .map(|group| composition.fraction(group) * group.mature_height_meters())
            .sum::<f64>();
        let maturity = (history.stand_age_years / 120.0).clamp(0.0, 1.0);
        let mean_canopy_height_meters = mature_height_meters
            * (0.20 + (maturity * 0.80))
            * (0.45 + (best_suitability * 0.55))
            * (1.0 - (soil.rock_exposure * 0.30));
        let normalized_height = (mean_canopy_height_meters / 31.0).clamp(0.0, 1.0);
        let tree_density_per_hectare =
            canopy_cover_fraction * (260.0 + (1_040.0 * (1.0 - normalized_height)));
        let aboveground_biomass_kg_per_square_meter =
            canopy_cover_fraction * mean_canopy_height_meters * (0.38 + (maturity * 0.34));

        Some(ForestSample {
            canopy_cover_fraction,
            tree_density_per_hectare,
            aboveground_biomass_kg_per_square_meter,
            mean_canopy_height_meters,
            stand_age_years: history.stand_age_years,
            disturbance_severity: history.severity,
            dominant_disturbance: history.dominant_disturbance,
            composition,
        })
    }
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
    use treeline_coordinates::stable_hash;

    use super::*;

    const TEST_WORLD: WorldIdentity = WorldIdentity::new(0x5eed, SOIL_GENERATOR_VERSION, 0);
    const FOREST_WORLD: WorldIdentity = WorldIdentity::new(0x5eed, FOREST_GENERATOR_VERSION, 0);

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
}
