use treeline_coordinates::{CellIndex, WorldIdentity};
use treeline_geography::{Climate, RegionalProfile};
use treeline_terrain::WildernessTerrain;

/// Generator version that first grows deterministic shallow-ocean reef frameworks.
pub const REEF_GENERATOR_VERSION: u32 = 15;

const DOMAIN_REEF_PATCH: u64 = 0x5245_4546_5041_5443;
const DOMAIN_REEF_CHANNEL: u64 = 0x5245_4546_4348_414e;
const DOMAIN_CURRENT_X: u64 = 0x5245_4546_4355_5258;
const DOMAIN_CURRENT_Z: u64 = 0x5245_4546_4355_525a;
const REEF_PATCH_EDGE_METERS: f64 = 1_600.0;
const CURRENT_EDGE_METERS: f64 = 24_000.0;
const SEA_LEVEL_METERS: f64 = 0.0;
const SHORE_SAMPLE_DISTANCES_METERS: [f64; 4] = [1_000.0, 3_000.0, 8_000.0, 20_000.0];
const SHORE_SAMPLE_DIRECTIONS: [[f64; 2]; 8] = [
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

/// Reef planforms emerge from coast distance and exposure rather than presets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReefForm {
    Fringing,
    Patch,
    Barrier,
}

impl ReefForm {
    pub const ALL: [Self; 3] = [Self::Fringing, Self::Patch, Self::Barrier];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Fringing => "fringing reef",
            Self::Patch => "patch reef",
            Self::Barrier => "barrier-like reef",
        }
    }
}

/// Relative expression of the generated reef planforms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReefComposition {
    pub fringing_fraction: f64,
    pub patch_fraction: f64,
    pub barrier_fraction: f64,
}

impl ReefComposition {
    pub fn fraction(self, form: ReefForm) -> f64 {
        match form {
            ReefForm::Fringing => self.fringing_fraction,
            ReefForm::Patch => self.patch_fraction,
            ReefForm::Barrier => self.barrier_fraction,
        }
    }

    pub fn dominant(self) -> ReefForm {
        let mut dominant = ReefForm::Fringing;
        for form in [ReefForm::Patch, ReefForm::Barrier] {
            if self.fraction(form) > self.fraction(dominant) {
                dominant = form;
            }
        }
        dominant
    }
}

/// Explainable reef growth and structure at one horizontal ocean position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReefSample {
    pub coverage_fraction: f64,
    pub growth_potential_fraction: f64,
    pub water_depth_meters: f64,
    pub temperature_suitability_fraction: f64,
    pub wave_exposure_fraction: f64,
    pub clarity_fraction: f64,
    pub substrate_suitability_fraction: f64,
    pub current_direction: [f64; 2],
    pub current_speed_fraction: f64,
    pub distance_to_shore_meters: f64,
    pub framework_height_meters: f64,
    pub channel_fraction: f64,
    pub lagoon_fraction: f64,
    pub composition: ReefComposition,
}

impl ReefSample {
    pub fn dominant_form(self) -> ReefForm {
        self.composition.dominant()
    }
}

/// Functional reef growth constrained by the physical shallow-ocean setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReefDistribution {
    pub world: WorldIdentity,
}

impl ReefDistribution {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    pub fn sample(self, x: f64, z: f64) -> Option<ReefSample> {
        if self.world.generator_version < REEF_GENERATOR_VERSION {
            return None;
        }

        let terrain = WildernessTerrain::new(self.world);
        let surface_height = terrain.height_at(x, z)?;
        let water_depth_meters = (SEA_LEVEL_METERS - surface_height).max(0.0);
        let climate = Climate::new(self.world).sample(x, z)?;
        let profile = RegionalProfile::sample(self.world, x, z)?;
        let erosion = terrain.erosion_at(x, z)?;
        let patch = value_field(self.world, DOMAIN_REEF_PATCH, x, z, REEF_PATCH_EDGE_METERS)?;
        let channel_noise = value_field(
            self.world,
            DOMAIN_REEF_CHANNEL,
            x,
            z,
            REEF_PATCH_EDGE_METERS * 2.4,
        )?;
        let channel_fraction = smoothstep(0.72, 0.91, channel_noise);
        let depth_suitability = smoothstep(1.0, 4.0, water_depth_meters)
            * (1.0 - smoothstep(34.0, 58.0, water_depth_meters));
        let warmth = climate.warmth_fraction();
        let temperature_suitability_fraction =
            smoothstep(0.58, 0.76, warmth) * (1.0 - smoothstep(0.96, 1.0, warmth));
        let wave_exposure_fraction =
            (climate.ocean_proximity_fraction * (0.38 + (patch * 0.62))).clamp(0.0, 1.0);
        let clarity_fraction = (1.0
            - (climate.precipitation_fraction() * 0.48)
            - (erosion.sediment_deposition_meters / 18.0).clamp(0.0, 1.0) * 0.32
            + (wave_exposure_fraction * 0.14))
            .clamp(0.0, 1.0);
        let substrate_suitability_fraction = (profile.rock_hardness * 0.46
            + erosion.rock_exposure * 0.38
            + (1.0 - erosion.scree_cover) * 0.16)
            .clamp(0.0, 1.0);
        let current_x =
            value_field(self.world, DOMAIN_CURRENT_X, x, z, CURRENT_EDGE_METERS)? * 2.0 - 1.0;
        let current_z =
            value_field(self.world, DOMAIN_CURRENT_Z, x, z, CURRENT_EDGE_METERS)? * 2.0 - 1.0;
        let current_length = libm::hypot(current_x, current_z);
        let current_direction = if current_length <= f64::EPSILON {
            [1.0, 0.0]
        } else {
            [current_x / current_length, current_z / current_length]
        };
        let current_speed_fraction = current_length.clamp(0.0, 1.0);
        let current_suitability =
            (1.0 - (current_speed_fraction - 0.48).abs() * 1.25).clamp(0.2, 1.0);
        let distance_to_shore_meters = distance_to_shore(terrain, x, z, surface_height)?;
        let ocean_presence = f64::from(surface_height < SEA_LEVEL_METERS);
        let growth_potential_fraction = (ocean_presence
            * depth_suitability
            * temperature_suitability_fraction
            * clarity_fraction
            * (0.35 + (substrate_suitability_fraction * 0.65))
            * current_suitability)
            .clamp(0.0, 1.0);
        let coverage_fraction = (growth_potential_fraction
            * (0.56 + (patch * 0.64))
            * (1.0 - (channel_fraction * 0.94)))
            .clamp(0.0, 1.0);

        let (composition, lagoon_fraction) =
            reef_planform(distance_to_shore_meters, wave_exposure_fraction, patch);
        let available_growth_height = (water_depth_meters - 1.5).clamp(0.0, 22.0);
        let framework_height_meters = (available_growth_height
            * growth_potential_fraction
            * (0.52 + (patch * 0.60))
            * (1.0 - (channel_fraction * 0.88)))
            .clamp(0.0, available_growth_height);

        Some(ReefSample {
            coverage_fraction,
            growth_potential_fraction,
            water_depth_meters,
            temperature_suitability_fraction,
            wave_exposure_fraction,
            clarity_fraction,
            substrate_suitability_fraction,
            current_direction,
            current_speed_fraction,
            distance_to_shore_meters,
            framework_height_meters,
            channel_fraction,
            lagoon_fraction,
            composition,
        })
    }
}

fn reef_planform(
    distance_to_shore_meters: f64,
    wave_exposure_fraction: f64,
    patch: f64,
) -> (ReefComposition, f64) {
    let fringing = 0.08
        + (1.0 - smoothstep(1_500.0, 6_000.0, distance_to_shore_meters))
            * (0.55 + (wave_exposure_fraction * 0.45));
    let barrier = 0.08
        + smoothstep(3_000.0, 9_000.0, distance_to_shore_meters)
            * (1.0 - smoothstep(18_000.0, 24_000.0, distance_to_shore_meters))
            * (0.50 + (wave_exposure_fraction * 0.50));
    let patch_form = 0.18
        + (1.0 - (patch - 0.5).abs() * 1.35).clamp(0.0, 1.0)
            * (0.58 + ((1.0 - wave_exposure_fraction) * 0.42));
    let total = fringing + patch_form + barrier;
    let composition = ReefComposition {
        fringing_fraction: fringing / total,
        patch_fraction: patch_form / total,
        barrier_fraction: barrier / total,
    };
    let lagoon_fraction = (composition.barrier_fraction
        * (1.0 - wave_exposure_fraction)
        * smoothstep(5_000.0, 14_000.0, distance_to_shore_meters))
    .clamp(0.0, 1.0);
    (composition, lagoon_fraction)
}

fn distance_to_shore(terrain: WildernessTerrain, x: f64, z: f64, local_height: f64) -> Option<f64> {
    if local_height >= SEA_LEVEL_METERS {
        return Some(0.0);
    }
    for distance in SHORE_SAMPLE_DISTANCES_METERS {
        for [direction_x, direction_z] in SHORE_SAMPLE_DIRECTIONS {
            if terrain.height_at(x + (direction_x * distance), z + (direction_z * distance))?
                >= SEA_LEVEL_METERS
            {
                return Some(distance);
            }
        }
    }
    Some(SHORE_SAMPLE_DISTANCES_METERS[SHORE_SAMPLE_DISTANCES_METERS.len() - 1] * 1.25)
}

fn value_field(world: WorldIdentity, domain: u64, x: f64, z: f64, edge: f64) -> Option<f64> {
    let cell = CellIndex::containing(x, z, 0, edge)?;
    let origin_x = index_as_f64(cell.x) * edge;
    let origin_z = index_as_f64(cell.z) * edge;
    let blend_x = smoothstep(0.0, 1.0, ((x - origin_x) / edge).clamp(0.0, 1.0));
    let blend_z = smoothstep(0.0, 1.0, ((z - origin_z) / edge).clamp(0.0, 1.0));
    let next_x = cell.x.checked_add(1)?;
    let next_z = cell.z.checked_add(1)?;
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

fn smoothstep(edge0: f64, edge1: f64, value: f64) -> f64 {
    let amount = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    amount * amount * (3.0 - (2.0 * amount))
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: i64) -> f64 {
    index as f64
}

#[allow(clippy::cast_precision_loss)]
fn hash53_as_f64(hash: u64) -> f64 {
    hash as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_coordinates::stable_hash;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, REEF_GENERATOR_VERSION, 0);

    #[test]
    fn reef_samples_are_deterministic_bounded_and_normalized() {
        let distribution = ReefDistribution::new(WORLD);
        for [x, z] in [
            [-420_000.0, -180_000.0],
            [-128_000.0, 96_000.0],
            [64_000.0, -320_000.0],
            [510_000.0, 275_000.0],
        ] {
            let first = distribution.sample(x, z).expect("reef sample");
            let second = distribution.sample(x, z).expect("same reef sample");
            let composition_total = ReefForm::ALL
                .into_iter()
                .map(|form| first.composition.fraction(form))
                .sum::<f64>();

            assert_eq!(first, second);
            assert!((0.0..=1.0).contains(&first.coverage_fraction));
            assert!((0.0..=1.0).contains(&first.growth_potential_fraction));
            assert!((0.0..=1.0).contains(&first.temperature_suitability_fraction));
            assert!((0.0..=1.0).contains(&first.wave_exposure_fraction));
            assert!((0.0..=1.0).contains(&first.clarity_fraction));
            assert!((0.0..=1.0).contains(&first.substrate_suitability_fraction));
            assert!((0.0..=1.0).contains(&first.current_speed_fraction));
            assert!((0.0..=1.0).contains(&first.channel_fraction));
            assert!((0.0..=1.0).contains(&first.lagoon_fraction));
            assert!(first.framework_height_meters <= (first.water_depth_meters - 1.5).max(0.0));
            assert!((composition_total - 1.0).abs() < 1.0e-12);
            assert!(
                (libm::hypot(first.current_direction[0], first.current_direction[1]) - 1.0).abs()
                    < 1.0e-12
            );
        }
    }

    #[test]
    fn generated_reef_growth_occurs_only_below_sea_level() {
        let distribution = ReefDistribution::new(WORLD);
        let mut strongest_growth = 0.0_f64;
        let mut represented_forms = [false; 3];
        for z_index in -40..=40 {
            for x_index in -40..=40 {
                let sample = distribution
                    .sample(f64::from(x_index) * 50_000.0, f64::from(z_index) * 50_000.0)
                    .expect("reef sample");
                if sample.water_depth_meters <= 0.0 {
                    assert!(sample.coverage_fraction.abs() < f64::EPSILON);
                    assert!(sample.framework_height_meters.abs() < f64::EPSILON);
                }
                if sample.coverage_fraction > 0.05 {
                    strongest_growth = strongest_growth.max(sample.coverage_fraction);
                    represented_forms[match sample.dominant_form() {
                        ReefForm::Fringing => 0,
                        ReefForm::Patch => 1,
                        ReefForm::Barrier => 2,
                    }] = true;
                }
            }
        }
        assert!(strongest_growth > 0.10, "the test world should grow reefs");
        assert!(
            represented_forms
                .into_iter()
                .filter(|present| *present)
                .count()
                >= 2,
            "coast distance and exposure should produce multiple reef forms"
        );
    }

    #[test]
    fn negative_coordinate_boundaries_are_finite_and_stable() {
        let distribution = ReefDistribution::new(WORLD);
        for coordinate in [-1_600.0, -0.001, 0.0, 1_600.0] {
            let sample = distribution
                .sample(coordinate, -coordinate)
                .expect("boundary sample");
            assert!(sample.coverage_fraction.is_finite());
            assert!(sample.framework_height_meters.is_finite());
        }
    }

    #[test]
    fn old_worlds_do_not_expose_reefs() {
        let old_world = WorldIdentity::new(0x5eed, REEF_GENERATOR_VERSION - 1, 0);
        assert!(ReefDistribution::new(old_world).sample(0.0, 0.0).is_none());
    }

    #[test]
    fn reef_distribution_has_a_golden_fingerprint() {
        let distribution = ReefDistribution::new(WORLD);
        let words = [
            [-600_000.0, -1_700_000.0],
            [-1_250_000.0, 850_000.0],
            [925_000.0, -1_450_000.0],
        ]
        .into_iter()
        .flat_map(|[x, z]| {
            let sample = distribution.sample(x, z).expect("reef");
            [
                sample.coverage_fraction.to_bits(),
                sample.growth_potential_fraction.to_bits(),
                sample.water_depth_meters.to_bits(),
                sample.temperature_suitability_fraction.to_bits(),
                sample.wave_exposure_fraction.to_bits(),
                sample.clarity_fraction.to_bits(),
                sample.substrate_suitability_fraction.to_bits(),
                sample.current_direction[0].to_bits(),
                sample.current_direction[1].to_bits(),
                sample.current_speed_fraction.to_bits(),
                sample.distance_to_shore_meters.to_bits(),
                sample.framework_height_meters.to_bits(),
                sample.channel_fraction.to_bits(),
                sample.lagoon_fraction.to_bits(),
                sample.composition.fringing_fraction.to_bits(),
                sample.composition.patch_fraction.to_bits(),
                sample.composition.barrier_fraction.to_bits(),
            ]
        })
        .collect::<Vec<_>>();

        assert_eq!(
            stable_hash(&words),
            5_167_148_220_387_966_363,
            "changing this value changes generated reefs"
        );
    }
}
