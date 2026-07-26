//! Ecosystem primitives built from environmental variables rather than biome IDs.

use treeline_coordinates::WorldIdentity;
use treeline_geography::{Climate, RegionalProfile};
use treeline_terrain::WildernessTerrain;

/// Generator version that first exposes deterministic soil profiles.
pub const SOIL_GENERATOR_VERSION: u32 = 8;

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

#[cfg(test)]
mod tests {
    use treeline_coordinates::stable_hash;

    use super::*;

    const TEST_WORLD: WorldIdentity = WorldIdentity::new(0x5eed, SOIL_GENERATOR_VERSION, 0);

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
}
