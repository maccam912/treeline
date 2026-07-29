use treeline_coordinates::WorldIdentity;
use treeline_geography::Climate;

use crate::{ForestDistribution, Soil};

/// Generator version that first exposes hydrology-constrained wetlands.
pub const WETLAND_GENERATOR_VERSION: u32 = 14;

/// The water setting supplied by the world-scale hydrology layer.
///
/// Keeping these values explicit lets ecology remain independent of regional
/// cache ownership while still deriving wetlands from real rivers and filled
/// basins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WetlandHydrology {
    pub surface_height_meters: f64,
    pub equilibrium_water_depth_meters: f64,
    pub floodplain_fraction: f64,
    pub river_discharge_cubic_meters_per_second: f64,
}

impl WetlandHydrology {
    pub fn new(
        surface_height_meters: f64,
        equilibrium_water_depth_meters: f64,
        floodplain_fraction: f64,
        river_discharge_cubic_meters_per_second: f64,
    ) -> Option<Self> {
        [
            surface_height_meters,
            equilibrium_water_depth_meters,
            floodplain_fraction,
            river_discharge_cubic_meters_per_second,
        ]
        .into_iter()
        .all(f64::is_finite)
        .then_some(Self {
            surface_height_meters,
            equilibrium_water_depth_meters: equilibrium_water_depth_meters.max(0.0),
            floodplain_fraction: floodplain_fraction.clamp(0.0, 1.0),
            river_discharge_cubic_meters_per_second: river_discharge_cubic_meters_per_second
                .max(0.0),
        })
    }
}

/// Emergent wetland growth strategies, blended continuously at every point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WetlandKind {
    EmergentMarsh,
    ForestedSwamp,
    Peatland,
    SeasonalWetland,
    SaltMarsh,
}

impl WetlandKind {
    pub const ALL: [Self; 5] = [
        Self::EmergentMarsh,
        Self::ForestedSwamp,
        Self::Peatland,
        Self::SeasonalWetland,
        Self::SaltMarsh,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::EmergentMarsh => "emergent marsh",
            Self::ForestedSwamp => "forested swamp",
            Self::Peatland => "peatland",
            Self::SeasonalWetland => "seasonal wetland",
            Self::SaltMarsh => "salt marsh",
        }
    }
}

/// Relative expression of wetland growth strategies at one location.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WetlandComposition {
    pub emergent_marsh_fraction: f64,
    pub forested_swamp_fraction: f64,
    pub peatland_fraction: f64,
    pub seasonal_wetland_fraction: f64,
    pub salt_marsh_fraction: f64,
}

impl WetlandComposition {
    pub fn fraction(self, kind: WetlandKind) -> f64 {
        match kind {
            WetlandKind::EmergentMarsh => self.emergent_marsh_fraction,
            WetlandKind::ForestedSwamp => self.forested_swamp_fraction,
            WetlandKind::Peatland => self.peatland_fraction,
            WetlandKind::SeasonalWetland => self.seasonal_wetland_fraction,
            WetlandKind::SaltMarsh => self.salt_marsh_fraction,
        }
    }

    pub fn dominant(self) -> WetlandKind {
        let mut dominant = WetlandKind::EmergentMarsh;
        for kind in [
            WetlandKind::ForestedSwamp,
            WetlandKind::Peatland,
            WetlandKind::SeasonalWetland,
            WetlandKind::SaltMarsh,
        ] {
            if self.fraction(kind) > self.fraction(dominant) {
                dominant = kind;
            }
        }
        dominant
    }
}

/// Explainable equilibrium wetland conditions at one horizontal position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WetlandSample {
    pub coverage_fraction: f64,
    pub surface_saturation_fraction: f64,
    pub hydroperiod_fraction: f64,
    pub flood_frequency_fraction: f64,
    pub open_water_fraction: f64,
    pub peat_depth_meters: f64,
    pub salinity_fraction: f64,
    pub composition: WetlandComposition,
}

impl WetlandSample {
    pub fn dominant_kind(self) -> WetlandKind {
        self.composition.dominant()
    }
}

/// Functional wetland ecology derived from explicit water, climate, soil, and forest state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WetlandDistribution {
    pub world: WorldIdentity,
}

impl WetlandDistribution {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Classifies a hydrological setting without introducing a biome ID.
    pub fn sample(self, x: f64, z: f64, hydrology: WetlandHydrology) -> Option<WetlandSample> {
        if self.world.generator_version < WETLAND_GENERATOR_VERSION {
            return None;
        }

        let climate = Climate::new(self.world).sample(x, z)?;
        let soil = Soil::new(self.world).sample(x, z)?;
        let forest = ForestDistribution::new(self.world).sample(x, z)?;
        let water_depth = hydrology.equilibrium_water_depth_meters;
        let shallow_water = if water_depth > 0.0 {
            1.0 - smoothstep(0.35, 4.0, water_depth)
        } else {
            0.0
        };
        let open_water_fraction = smoothstep(0.05, 2.5, water_depth);
        let slope_fraction = (soil.slope / 0.08).clamp(0.0, 1.0);
        let flatness = 1.0 - slope_fraction;
        let discharge = (libm::log2(1.0 + hydrology.river_discharge_cubic_meters_per_second) / 8.0)
            .clamp(0.0, 1.0);
        let flood_frequency_fraction =
            (hydrology.floodplain_fraction * (0.55 + (discharge * 0.45))).clamp(0.0, 1.0);
        let coastal_inundation = climate.ocean_proximity_fraction
            * (1.0 - smoothstep(0.0, 6.0, hydrology.surface_height_meters.abs()))
            * flatness;
        let surface_saturation_fraction = (soil.surface_moisture * 0.62
            + shallow_water * 0.48
            + flood_frequency_fraction * 0.42
            + coastal_inundation * 0.34)
            .clamp(0.0, 1.0);
        let hydroperiod_fraction = (surface_saturation_fraction * 0.62
            + shallow_water * 0.36
            + flood_frequency_fraction * 0.24
            + flatness * 0.12)
            .clamp(0.0, 1.0);
        let wetland_signal =
            hydroperiod_fraction * (0.55 + (flatness * 0.45)) * (1.0 - (soil.rock_exposure * 0.72));
        let coverage_fraction = (smoothstep(0.42, 0.82, wetland_signal)
            * (1.0 - (open_water_fraction * 0.88)))
            .clamp(0.0, 1.0);

        let warmth = climate.warmth_fraction();
        let acidity = soil.acidity_fraction();
        let evaporation_pressure =
            (warmth * (1.0 - climate.precipitation_fraction())).clamp(0.0, 1.0);
        let salinity_fraction =
            (coastal_inundation * (0.42 + (evaporation_pressure * 0.58))).clamp(0.0, 1.0);
        let peat_potential = hydroperiod_fraction
            * acidity
            * (1.0 - smoothstep(0.58, 0.88, warmth))
            * (1.0 - (flood_frequency_fraction * 0.48));
        let peat_depth_meters = (peat_potential * peat_potential * 5.5).clamp(0.0, 5.5);
        let intermittency = (1.0 - (hydroperiod_fraction - 0.5).abs() * 2.0).clamp(0.0, 1.0);

        let marsh = (0.18 + (shallow_water * 0.82))
            * (0.38 + ((1.0 - forest.canopy_cover_fraction) * 0.62))
            * (1.0 - (salinity_fraction * 0.55));
        let swamp = (0.12 + (forest.canopy_cover_fraction * 0.88))
            * (0.42 + (hydroperiod_fraction * 0.58))
            * (0.46 + (warmth * 0.54))
            * (1.0 - (salinity_fraction * 0.72));
        let peatland = 0.08 + (peat_potential * 1.12);
        let seasonal = (0.10 + (intermittency * 0.90))
            * (0.52 + (climate.precipitation_seasonality_fraction * 0.48))
            * (1.0 - (salinity_fraction * 0.45));
        let salt_marsh = 0.04 + (salinity_fraction * 1.16);
        let total = marsh + swamp + peatland + seasonal + salt_marsh;
        let composition = WetlandComposition {
            emergent_marsh_fraction: marsh / total,
            forested_swamp_fraction: swamp / total,
            peatland_fraction: peatland / total,
            seasonal_wetland_fraction: seasonal / total,
            salt_marsh_fraction: salt_marsh / total,
        };

        Some(WetlandSample {
            coverage_fraction,
            surface_saturation_fraction,
            hydroperiod_fraction,
            flood_frequency_fraction,
            open_water_fraction,
            peat_depth_meters,
            salinity_fraction,
            composition,
        })
    }
}

fn smoothstep(edge0: f64, edge1: f64, value: f64) -> f64 {
    let amount = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    amount * amount * (3.0 - (2.0 * amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_coordinates::stable_hash;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, WETLAND_GENERATOR_VERSION, 0);

    #[test]
    fn wetland_sampling_is_deterministic_bounded_and_normalized() {
        let hydrology =
            WetlandHydrology::new(1.2, 0.18, 0.82, 18.0).expect("valid hydrology inputs");
        let first = WetlandDistribution::new(WORLD)
            .sample(-12_500.0, 42_750.0, hydrology)
            .expect("wetland sample");
        let second = WetlandDistribution::new(WORLD)
            .sample(-12_500.0, 42_750.0, hydrology)
            .expect("same wetland sample");
        let composition_total = WetlandKind::ALL
            .into_iter()
            .map(|kind| first.composition.fraction(kind))
            .sum::<f64>();

        assert_eq!(first, second);
        assert!((0.0..=1.0).contains(&first.coverage_fraction));
        assert!((0.0..=1.0).contains(&first.surface_saturation_fraction));
        assert!((0.0..=1.0).contains(&first.hydroperiod_fraction));
        assert!((0.0..=1.0).contains(&first.flood_frequency_fraction));
        assert!((0.0..=1.0).contains(&first.open_water_fraction));
        assert!((0.0..=5.5).contains(&first.peat_depth_meters));
        assert!((0.0..=1.0).contains(&first.salinity_fraction));
        assert!((composition_total - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn explicit_water_state_changes_the_ecology_at_one_coordinate() {
        let distribution = WetlandDistribution::new(WORLD);
        let dry = distribution
            .sample(
                18_250.0,
                -9_750.0,
                WetlandHydrology::new(22.0, 0.0, 0.0, 0.0).expect("dry hydrology"),
            )
            .expect("dry sample");
        let flooded = distribution
            .sample(
                18_250.0,
                -9_750.0,
                WetlandHydrology::new(1.0, 0.2, 0.9, 24.0).expect("flooded hydrology"),
            )
            .expect("flooded sample");

        assert!(flooded.hydroperiod_fraction > dry.hydroperiod_fraction);
        assert!(flooded.flood_frequency_fraction > dry.flood_frequency_fraction);
        assert!(flooded.coverage_fraction > dry.coverage_fraction);
    }

    #[test]
    fn old_worlds_do_not_expose_wetlands() {
        let old_world = WorldIdentity::new(0x5eed, WETLAND_GENERATOR_VERSION - 1, 0);
        let hydrology = WetlandHydrology::new(0.0, 0.1, 0.8, 12.0).expect("hydrology");
        assert!(
            WetlandDistribution::new(old_world)
                .sample(0.0, 0.0, hydrology)
                .is_none()
        );
    }

    #[test]
    fn negative_coordinate_boundaries_are_finite_and_stable() {
        let distribution = WetlandDistribution::new(WORLD);
        let hydrology = WetlandHydrology::new(-0.5, 0.2, 0.7, 9.0).expect("hydrology");
        for coordinate in [-2_000.0, -0.001, 0.0, 2_000.0] {
            let first = distribution
                .sample(coordinate, -coordinate, hydrology)
                .expect("boundary wetland");
            let second = distribution
                .sample(coordinate, -coordinate, hydrology)
                .expect("same boundary wetland");
            assert_eq!(first, second);
            assert!(first.coverage_fraction.is_finite());
        }
    }

    #[test]
    fn wetland_distribution_has_a_golden_fingerprint() {
        let distribution = WetlandDistribution::new(WORLD);
        let settings = [
            [-91_125.0, -37_375.0, 2.0, 0.0, 0.75, 16.0],
            [-64_250.0, 63_875.0, -0.4, 0.3, 0.15, 2.0],
            [22_375.0, -48_625.0, 18.0, 0.0, 0.0, 0.0],
        ];
        let words = settings
            .into_iter()
            .flat_map(|[x, z, height, depth, floodplain, discharge]| {
                let hydrology =
                    WetlandHydrology::new(height, depth, floodplain, discharge).expect("hydrology");
                let sample = distribution.sample(x, z, hydrology).expect("wetland");
                [
                    sample.coverage_fraction.to_bits(),
                    sample.surface_saturation_fraction.to_bits(),
                    sample.hydroperiod_fraction.to_bits(),
                    sample.flood_frequency_fraction.to_bits(),
                    sample.open_water_fraction.to_bits(),
                    sample.peat_depth_meters.to_bits(),
                    sample.salinity_fraction.to_bits(),
                    sample.composition.emergent_marsh_fraction.to_bits(),
                    sample.composition.forested_swamp_fraction.to_bits(),
                    sample.composition.peatland_fraction.to_bits(),
                    sample.composition.seasonal_wetland_fraction.to_bits(),
                    sample.composition.salt_marsh_fraction.to_bits(),
                ]
            })
            .collect::<Vec<_>>();

        assert_eq!(
            stable_hash(&words),
            1_581_129_568_980_974_396,
            "changing this value changes generated wetlands"
        );
    }
}
