use std::cell::RefCell;
use std::collections::VecDeque;

use treeline_coordinates::WorldIdentity;
use treeline_geography::{
    Climate, ClimateSample, PROVINCE_GENERATOR_VERSION, ProvincePlan, ProvinceSample,
};

use crate::Soil;

/// Generator contract that first exposes broad, continuous ecosystem regimes.
pub const ECOSYSTEM_GENERATOR_VERSION: u32 = PROVINCE_GENERATOR_VERSION;
// Exact-coordinate memoization only: eviction and visitation order cannot
// change generated values.
const ECOSYSTEM_SAMPLE_CACHE_ENTRIES: usize = 64;

thread_local! {
    static ECOSYSTEM_SAMPLE_CACHE: RefCell<VecDeque<EcosystemCacheEntry>> =
        const { RefCell::new(VecDeque::new()) };
}

#[derive(Clone, Copy)]
struct EcosystemCacheEntry {
    world: WorldIdentity,
    x_bits: u64,
    z_bits: u64,
    sample: EcosystemSample,
}

/// Overlapping ecosystem potentials and the physical controls that explain them.
///
/// Potentials are deliberately not a mutually exclusive biome assignment. A
/// wooded steppe, alpine wetland, or shrub-dotted grassland can express several
/// values at once. [`Self::relative_potentials`] is available for audits that
/// need normalized proportions without discarding the original overlap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EcosystemSample {
    pub land_fraction: f64,
    pub potential_evapotranspiration_millimeters: f64,
    pub climatic_water_balance_millimeters: f64,
    pub water_balance_fraction: f64,
    pub tree_line_elevation_meters: f64,
    pub above_tree_line_fraction: f64,
    pub exposure_fraction: f64,
    pub fire_pressure_fraction: f64,
    pub disturbance_fraction: f64,
    pub sediment_fraction: f64,
    pub salinity_fraction: f64,
    pub closed_basin_fraction: f64,
    pub closed_forest_potential: f64,
    pub open_woodland_potential: f64,
    pub grassland_prairie_potential: f64,
    pub steppe_potential: f64,
    pub shrubland_potential: f64,
    pub desert_potential: f64,
    pub tundra_potential: f64,
    pub exposed_alpine_potential: f64,
    pub wetland_potential: f64,
}

impl EcosystemSample {
    /// Returns the nine overlapping landscape potentials in stable audit order.
    pub const fn potentials(self) -> [f64; 9] {
        [
            self.closed_forest_potential,
            self.open_woodland_potential,
            self.grassland_prairie_potential,
            self.steppe_potential,
            self.shrubland_potential,
            self.desert_potential,
            self.tundra_potential,
            self.exposed_alpine_potential,
            self.wetland_potential,
        ]
    }

    /// Normalizes the potential vector for comparisons without selecting a biome.
    pub fn relative_potentials(self) -> [f64; 9] {
        let potentials = self.potentials();
        let total = potentials.iter().sum::<f64>();
        if total <= f64::EPSILON {
            return [0.0; 9];
        }
        potentials.map(|potential| potential / total)
    }

    /// Strongest non-forest expression used to keep open country genuinely open.
    pub(crate) fn open_land_potential(self) -> f64 {
        [
            self.grassland_prairie_potential,
            self.steppe_potential,
            self.shrubland_potential,
            self.desert_potential,
            self.tundra_potential,
            self.exposed_alpine_potential,
        ]
        .into_iter()
        .fold(0.0, f64::max)
    }
}

/// Functional sampler for broad ecosystem structure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EcosystemDistribution {
    pub world: WorldIdentity,
}

impl EcosystemDistribution {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Samples continuous ecosystem potentials from the v18 province causes.
    #[allow(clippy::too_many_lines)]
    pub fn sample(self, x: f64, z: f64) -> Option<EcosystemSample> {
        if self.world.generator_version < ECOSYSTEM_GENERATOR_VERSION {
            return None;
        }
        let x_bits = x.to_bits();
        let z_bits = z.to_bits();
        if let Some(sample) = ECOSYSTEM_SAMPLE_CACHE.with(|cache| {
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

        let province = ProvincePlan::sample_at(self.world, x, z)?;
        let climate = Climate::new(self.world).sample(x, z)?;
        let soil = Soil::new(self.world).sample(x, z)?;
        let water_balance = climate_water_balance(climate, &province);
        let warmth =
            ((climate.warmth_fraction() * 0.72) + (province.temperature * 0.28)).clamp(0.0, 1.0);
        let cold = 1.0 - warmth;
        let slope_fraction = (soil.slope / 0.24).clamp(0.0, 1.0);
        let exposure_fraction =
            ((province.exposure * 0.58) + (soil.rock_exposure * 0.25) + (slope_fraction * 0.17))
                .clamp(0.0, 1.0);
        let effective_moisture = ((water_balance.fraction * 0.46)
            + (province.moisture * 0.26)
            + (soil.surface_moisture * 0.28)
            - (province.salinity * 0.12))
            .clamp(0.0, 1.0);
        let effective_aridity = ((province.aridity * 0.48)
            + ((1.0 - water_balance.fraction) * 0.34)
            + ((1.0 - soil.surface_moisture) * 0.18))
            .clamp(0.0, 1.0);
        let tree_line_elevation_meters = (720.0 + (warmth * 3_050.0)
            - (province.glaciation * 620.0)
            - (climate.continentality_fraction * 180.0))
            .clamp(280.0, 3_900.0);
        let above_tree_line_fraction = smoothstep_range(
            tree_line_elevation_meters - 260.0,
            tree_line_elevation_meters + 520.0,
            province.elevation_meters,
        );
        let fire_pressure_fraction = ((warmth * (1.0 - effective_moisture) * 0.48)
            + (province.disturbance * 0.30)
            + (effective_aridity * 0.22))
            .clamp(0.0, 1.0);
        let disturbance_fraction = ((province.disturbance * 0.54)
            + (fire_pressure_fraction * 0.28)
            + (slope_fraction * province.moisture * 0.18))
            .clamp(0.0, 1.0);
        let land = smoothstep_range(0.08, 0.72, province.land_fraction);
        let below_tree_line = 1.0 - above_tree_line_fraction;
        let substrate = ((soil.depth_fraction() * 0.46)
            + ((1.0 - soil.rock_exposure) * 0.32)
            + (province.sediment * 0.22))
            .clamp(0.0, 1.0);
        let flatness = 1.0 - slope_fraction;
        let temperate = (1.0 - ((warmth - 0.56).abs() * 1.85)).clamp(0.0, 1.0);
        let mesic = smoothstep_range(0.34, 0.76, effective_moisture);
        let semiarid = (smoothstep_range(0.30, 0.62, effective_aridity)
            * (1.0 - smoothstep_range(0.72, 0.94, effective_aridity)))
        .clamp(0.0, 1.0);
        let moisture_transition = (smoothstep_range(0.20, 0.48, effective_moisture)
            * (1.0 - smoothstep_range(0.66, 0.90, effective_moisture)))
        .clamp(0.0, 1.0);

        let closed_forest_potential = (land
            * below_tree_line
            * mesic
            * (0.44 + (substrate * 0.56))
            * (1.0 - (exposure_fraction * 0.52))
            * (1.0 - (fire_pressure_fraction * 0.58))
            * (0.72 + (province.ecological_memory * 0.28)))
            .clamp(0.0, 1.0);
        let open_woodland_potential = (land
            * below_tree_line
            * moisture_transition
            * (0.52 + (warmth * 0.24) + (substrate * 0.24))
            * (0.58 + (disturbance_fraction * 0.42))
            * (1.0 - (effective_aridity * 0.28)))
            .clamp(0.0, 1.0);
        let grassland_prairie_potential = (land
            * below_tree_line
            * (0.34 + (province.plains * 0.48) + (province.rolling_uplands * 0.18))
            * (0.30 + (temperate * 0.42) + (warmth * 0.28))
            * (0.36 + (moisture_transition * 0.64))
            * (0.54 + (fire_pressure_fraction * 0.46))
            * (1.0 - (province.salinity * 0.54)))
            .clamp(0.0, 1.0);
        let steppe_potential = (land
            * below_tree_line
            * semiarid
            * (0.44 + (flatness * 0.26) + (province.rolling_uplands * 0.30))
            * (0.62 + (cold * 0.18) + (disturbance_fraction * 0.20))
            * (1.0 - (province.salinity * 0.36)))
            .clamp(0.0, 1.0);
        let shrubland_potential = (land
            * below_tree_line
            * smoothstep_range(0.40, 0.78, effective_aridity)
            * (0.42 + (warmth * 0.26) + (disturbance_fraction * 0.32))
            * (0.48 + (province.rolling_uplands * 0.24) + (exposure_fraction * 0.28))
            * (1.0 - (smoothstep_range(0.82, 0.98, effective_aridity) * 0.62)))
            .clamp(0.0, 1.0);
        let desert_potential = (land
            * smoothstep_range(0.62, 0.91, effective_aridity)
            * (1.0 - (effective_moisture * 0.76))
            * (0.50
                + (province.dune * 0.20)
                + (province.closed_basin * 0.16)
                + (province.salinity * 0.14))
            * (1.0 - (above_tree_line_fraction * 0.54)))
            .clamp(0.0, 1.0);
        let tundra_potential = (land
            * (0.18 + (cold * 0.54) + (above_tree_line_fraction * 0.42))
            * (0.44 + (effective_moisture * 0.34) + (province.glaciation * 0.22))
            * (1.0 - (effective_aridity * 0.68))
            * (1.0 - (province.salinity * 0.42)))
            .clamp(0.0, 1.0);
        let exposed_alpine_potential = (land
            * above_tree_line_fraction
            * (0.34 + (exposure_fraction * 0.66))
            * (0.34 + (province.mountain * 0.44) + (province.glacial * 0.22)))
            .clamp(0.0, 1.0);
        let basin_saturation = (province.closed_basin
            * (0.36 + (province.moisture * 0.38) + (province.drainage * 0.26)))
            .clamp(0.0, 1.0);
        let wetland_potential = (land
            * (0.36 + (flatness * 0.64))
            * (0.48 + (province.sediment * 0.28) + (basin_saturation * 0.24))
            * smoothstep_range(
                0.30,
                0.76,
                (effective_moisture * 0.72) + (basin_saturation * 0.28),
            )
            * (1.0 - (exposure_fraction * 0.62)))
            .clamp(0.0, 1.0);

        let sample = EcosystemSample {
            land_fraction: province.land_fraction,
            potential_evapotranspiration_millimeters: water_balance
                .potential_evapotranspiration_millimeters,
            climatic_water_balance_millimeters: water_balance.balance_millimeters,
            water_balance_fraction: water_balance.fraction,
            tree_line_elevation_meters,
            above_tree_line_fraction,
            exposure_fraction,
            fire_pressure_fraction,
            disturbance_fraction,
            sediment_fraction: province.sediment,
            salinity_fraction: province.salinity,
            closed_basin_fraction: province.closed_basin,
            closed_forest_potential,
            open_woodland_potential,
            grassland_prairie_potential,
            steppe_potential,
            shrubland_potential,
            desert_potential,
            tundra_potential,
            exposed_alpine_potential,
            wetland_potential,
        };
        ECOSYSTEM_SAMPLE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= ECOSYSTEM_SAMPLE_CACHE_ENTRIES {
                cache.pop_front();
            }
            cache.push_back(EcosystemCacheEntry {
                world: self.world,
                x_bits,
                z_bits,
                sample,
            });
        });
        Some(sample)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClimateWaterBalance {
    pub potential_evapotranspiration_millimeters: f64,
    pub balance_millimeters: f64,
    pub fraction: f64,
}

pub(crate) fn climate_water_balance(
    climate: ClimateSample,
    province: &ProvinceSample,
) -> ClimateWaterBalance {
    let warmth = climate.warmth_fraction();
    let potential_evapotranspiration_millimeters = (300.0
        + (warmth * 1_620.0)
        + (climate.continentality_fraction * 190.0)
        + (climate.precipitation_seasonality_fraction * 140.0)
        - (climate.ocean_proximity_fraction * 90.0))
        .clamp(240.0, 2_260.0);
    let available_water_millimeters =
        climate.annual_precipitation_millimeters + (climate.annual_snowmelt_millimeters * 0.12);
    let balance_millimeters =
        available_water_millimeters - potential_evapotranspiration_millimeters;
    let climatic_fraction = smoothstep_range(-1_100.0, 850.0, balance_millimeters);
    let fraction = ((climatic_fraction * 0.62)
        + (province.moisture * 0.24)
        + ((1.0 - province.aridity) * 0.14))
        .clamp(0.0, 1.0);
    ClimateWaterBalance {
        potential_evapotranspiration_millimeters,
        balance_millimeters,
        fraction,
    }
}

fn smoothstep_range(edge0: f64, edge1: f64, value: f64) -> f64 {
    let amount = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    amount * amount * (3.0 - (2.0 * amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_coordinates::stable_hash;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, ECOSYSTEM_GENERATOR_VERSION, 0);

    #[test]
    fn ecosystem_sampling_is_deterministic_order_independent_bounded_and_normalizable() {
        let distribution = EcosystemDistribution::new(WORLD);
        let positions = [
            [-1_420_125.0, 812_375.0],
            [-512_000.001, -48_000.0],
            [0.0, 0.0],
            [2_960_500.0, -4_180_250.0],
        ];
        let forward = positions.map(|[x, z]| distribution.sample(x, z).expect("ecosystem"));
        let mut reversed_positions = positions;
        reversed_positions.reverse();
        let mut reversed =
            reversed_positions.map(|[x, z]| distribution.sample(x, z).expect("ecosystem"));
        reversed.reverse();
        assert_eq!(forward, reversed);

        for sample in forward {
            for value in [
                sample.water_balance_fraction,
                sample.above_tree_line_fraction,
                sample.exposure_fraction,
                sample.fire_pressure_fraction,
                sample.disturbance_fraction,
                sample.sediment_fraction,
                sample.salinity_fraction,
                sample.closed_basin_fraction,
            ]
            .into_iter()
            .chain(sample.potentials())
            {
                assert!((0.0..=1.0).contains(&value), "{value}");
            }
            assert!(sample.potential_evapotranspiration_millimeters > 0.0);
            assert!(sample.climatic_water_balance_millimeters.is_finite());
            assert!(sample.tree_line_elevation_meters.is_finite());
            let relative = sample.relative_potentials();
            let total = relative.iter().sum::<f64>();
            if sample.potentials().iter().sum::<f64>() > f64::EPSILON {
                assert!((total - 1.0).abs() < 1.0e-12);
            }
        }
    }

    #[test]
    fn ecosystem_fields_are_continuous_across_negative_province_boundaries() {
        let distribution = EcosystemDistribution::new(WORLD);
        let left = distribution
            .sample(-512_000.01, -73_000.0)
            .expect("left ecosystem");
        let right = distribution
            .sample(-511_999.99, -73_000.0)
            .expect("right ecosystem");
        for (left_value, right_value) in left.potentials().into_iter().zip(right.potentials()) {
            assert!((left_value - right_value).abs() < 0.01);
        }
        assert!((left.water_balance_fraction - right.water_balance_fraction).abs() < 0.01);
        assert!((left.salinity_fraction - right.salinity_fraction).abs() < 0.01);
    }

    #[test]
    fn old_worlds_do_not_expose_ecosystem_regimes() {
        let old = WorldIdentity::new(0x5eed, ECOSYSTEM_GENERATOR_VERSION - 1, 0);
        assert!(EcosystemDistribution::new(old).sample(0.0, 0.0).is_none());
    }

    #[test]
    fn far_apart_provinces_cover_distinct_ecosystem_regimes() {
        let distribution = EcosystemDistribution::new(WORLD);
        let mut maxima = [0.0_f64; 9];
        for z in -8..=8 {
            for x in -8..=8 {
                let sample = distribution
                    .sample(f64::from(x) * 384_000.0, f64::from(z) * 384_000.0)
                    .expect("ecosystem");
                for (maximum, potential) in maxima.iter_mut().zip(sample.potentials()) {
                    *maximum = maximum.max(potential);
                }
            }
        }
        for (axis, maximum) in maxima.into_iter().enumerate() {
            assert!(
                maximum > 0.12,
                "ecosystem potential {axis} peaked at {maximum}"
            );
        }
    }

    #[test]
    fn version_eighteen_ecosystems_have_a_golden_fingerprint() {
        let distribution = EcosystemDistribution::new(WORLD);
        let positions = [
            [-3_072_000.0, -1_152_000.0],
            [-512_000.001, -48_000.0],
            [0.0, 0.0],
            [2_960_500.0, -4_180_250.0],
        ];
        let mut words = Vec::new();
        for [x, z] in positions {
            let sample = distribution.sample(x, z).expect("ecosystem");
            words.extend([
                sample.land_fraction.to_bits(),
                sample.potential_evapotranspiration_millimeters.to_bits(),
                sample.climatic_water_balance_millimeters.to_bits(),
                sample.water_balance_fraction.to_bits(),
                sample.tree_line_elevation_meters.to_bits(),
                sample.above_tree_line_fraction.to_bits(),
                sample.exposure_fraction.to_bits(),
                sample.fire_pressure_fraction.to_bits(),
                sample.disturbance_fraction.to_bits(),
                sample.sediment_fraction.to_bits(),
                sample.salinity_fraction.to_bits(),
                sample.closed_basin_fraction.to_bits(),
            ]);
            words.extend(sample.potentials().map(f64::to_bits));
        }

        assert_eq!(
            stable_hash(&words),
            6_581_372_803_237_242_860,
            "changing this value changes generator version 18 ecosystem regimes"
        );
    }
}
