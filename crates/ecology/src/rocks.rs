use std::collections::BTreeMap;

use treeline_coordinates::{CellIndex, WorldIdentity, stable_hash};
use treeline_geography::RegionalProfile;
use treeline_terrain::WildernessTerrain;

use crate::Soil;

/// Generator version that first exposes deterministic surface-rock fields and individuals.
pub const SURFACE_ROCK_GENERATOR_VERSION: u32 = 12;

const DOMAIN_SURFACE_ROCK_INDIVIDUALS: u64 = 0x524f_434b_5f49_4e44;
const ROCK_PLACEMENT_CELL_EDGE_METERS: f64 = 4.0;
const ROCK_ENVIRONMENT_CELL_EDGE_METERS: f64 = 32.0;

/// Broad shape produced by local geology and weathering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RockForm {
    RoundedBoulder,
    AngularBlock,
    Slab,
    ScreeFragment,
}

impl RockForm {
    pub const fn label(self) -> &'static str {
        match self {
            Self::RoundedBoulder => "rounded boulder",
            Self::AngularBlock => "angular block",
            Self::Slab => "weathered slab",
            Self::ScreeFragment => "scree fragment",
        }
    }
}

/// Explainable continuous conditions controlling loose rock at one position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceRockSample {
    pub density_per_hectare: f64,
    pub mean_radius_meters: f64,
    pub rock_exposure_fraction: f64,
    pub scree_cover_fraction: f64,
    pub hardness_fraction: f64,
    pub weathering_fraction: f64,
    pub carbonate_fraction: f64,
    pub moisture_staining_fraction: f64,
    pub slope: f64,
}

/// Functional loose-rock distribution derived from geology, erosion, and soil.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceRockDistribution {
    pub world: WorldIdentity,
}

impl SurfaceRockDistribution {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Samples the expected rock scatter without creating individual geometry.
    pub fn sample(self, x: f64, z: f64) -> Option<SurfaceRockSample> {
        if self.world.generator_version < SURFACE_ROCK_GENERATOR_VERSION {
            return None;
        }

        let profile = RegionalProfile::sample(self.world, x, z)?;
        let erosion = WildernessTerrain::new(self.world).erosion_at(x, z)?;
        let soil = Soil::new(self.world).sample(x, z)?;
        let thin_soil = 1.0 - soil.depth_fraction();
        let slope_fraction = (erosion.slope / 0.22).clamp(0.0, 1.0);
        let density_per_hectare = (28.0
            + (erosion.rock_exposure * 760.0)
            + (erosion.scree_cover * 940.0)
            + (thin_soil * 170.0)
            + (profile.rock_hardness * slope_fraction * 150.0))
            .clamp(0.0, 2_100.0);
        let mean_radius_meters = (0.16
            + (erosion.rock_exposure * 0.72)
            + (profile.rock_hardness * 0.34)
            + ((1.0 - profile.erosion_age) * slope_fraction * 0.28))
            .clamp(0.12, 1.5);
        let moisture_staining_fraction = (soil.surface_moisture
            * (0.35 + (profile.erosion_age * 0.65))
            * (1.0 - (slope_fraction * 0.28)))
            .clamp(0.0, 1.0);

        Some(SurfaceRockSample {
            density_per_hectare,
            mean_radius_meters,
            rock_exposure_fraction: erosion.rock_exposure,
            scree_cover_fraction: erosion.scree_cover,
            hardness_fraction: profile.rock_hardness,
            weathering_fraction: profile.erosion_age,
            carbonate_fraction: profile.karst_probability,
            moisture_staining_fraction,
            slope: erosion.slope,
        })
    }
}

/// Half-open horizontal area used to request surface-rock individuals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RockBounds {
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
}

impl RockBounds {
    /// Creates finite, non-empty rock-generation bounds.
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

/// Geological and weathering traits consumed by the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RockGenotype {
    pub form: RockForm,
    pub hardness_fraction: f64,
    pub weathering_fraction: f64,
    pub fracture_fraction: f64,
    pub roundness_fraction: f64,
    pub carbonate_fraction: f64,
}

/// One deterministic loose rock positioned on the global horizontal lattice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceRock {
    pub id: u64,
    pub x: f64,
    pub z: f64,
    /// Horizontal, vertical, and horizontal radii before renderer irregularity.
    pub radii_meters: [f64; 3],
    pub rotation_turns: f64,
    pub tilt_direction: [f64; 2],
    pub tilt_fraction: f64,
    pub embedded_fraction: f64,
    pub moss_fraction: f64,
    pub genotype: RockGenotype,
}

/// Functional generator for spatially stable surface-rock individuals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceRocks {
    pub world: WorldIdentity,
}

impl SurfaceRocks {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Generates all rock centers inside a half-open horizontal area.
    ///
    /// A global four-meter lattice owns candidates. Coarser environmental cells
    /// control expected density, while stable per-individual hashes control
    /// placement and form. Filtering after generation makes tiled requests
    /// identical to one combined request, including at negative coordinates.
    pub fn rocks_in(self, bounds: RockBounds) -> Option<Vec<SurfaceRock>> {
        if self.world.generator_version < SURFACE_ROCK_GENERATOR_VERSION {
            return None;
        }
        let minimum = CellIndex::containing(
            bounds.min_x,
            bounds.min_z,
            0,
            ROCK_PLACEMENT_CELL_EDGE_METERS,
        )?;
        let maximum = CellIndex::containing(
            bounds.max_x,
            bounds.max_z,
            0,
            ROCK_PLACEMENT_CELL_EDGE_METERS,
        )?;
        let distribution = SurfaceRockDistribution::new(self.world);
        let mut environments = BTreeMap::new();
        let mut rocks = Vec::new();
        let mut cell_z = minimum.z;
        loop {
            let mut cell_x = minimum.x;
            loop {
                let cell = CellIndex::new(cell_x, cell_z, 0);
                let origin_x = index_as_f64(cell_x) * ROCK_PLACEMENT_CELL_EDGE_METERS;
                let origin_z = index_as_f64(cell_z) * ROCK_PLACEMENT_CELL_EDGE_METERS;
                let center_x = origin_x + (ROCK_PLACEMENT_CELL_EDGE_METERS * 0.5);
                let center_z = origin_z + (ROCK_PLACEMENT_CELL_EDGE_METERS * 0.5);
                let environment_cell = CellIndex::containing(
                    center_x,
                    center_z,
                    0,
                    ROCK_ENVIRONMENT_CELL_EDGE_METERS,
                )?;
                let environment_key = (environment_cell.x, environment_cell.z);
                let environment = if let Some(environment) = environments.get(&environment_key) {
                    *environment
                } else {
                    let environment_x = (index_as_f64(environment_cell.x) + 0.5)
                        * ROCK_ENVIRONMENT_CELL_EDGE_METERS;
                    let environment_z = (index_as_f64(environment_cell.z) + 0.5)
                        * ROCK_ENVIRONMENT_CELL_EDGE_METERS;
                    let environment = distribution.sample(environment_x, environment_z)?;
                    environments.insert(environment_key, environment);
                    environment
                };
                let cell_key = cell.generation_key(self.world, DOMAIN_SURFACE_ROCK_INDIVIDUALS);
                let expected_count = environment.density_per_hectare
                    * ROCK_PLACEMENT_CELL_EDGE_METERS
                    * ROCK_PLACEMENT_CELL_EDGE_METERS
                    / 10_000.0;
                let count =
                    stochastic_count(expected_count, random_fraction(cell_key, 0x0043_4f55_4e54));
                for ordinal in 0..count {
                    let id = stable_hash(&[cell_key, u64::from(ordinal)]);
                    let x = origin_x
                        + (ROCK_PLACEMENT_CELL_EDGE_METERS
                            * lerp(0.05, 0.95, random_fraction(id, 0x585f_4a49_5454)));
                    let z = origin_z
                        + (ROCK_PLACEMENT_CELL_EDGE_METERS
                            * lerp(0.05, 0.95, random_fraction(id, 0x5a5f_4a49_5454)));
                    if bounds.contains(x, z) {
                        rocks.push(rock_individual(id, x, z, environment));
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
        rocks.sort_by_key(|rock| rock.id);
        Some(rocks)
    }
}

fn rock_individual(id: u64, x: f64, z: f64, environment: SurfaceRockSample) -> SurfaceRock {
    let form_roll = random_fraction(id, 0x464f_524d);
    let form = if environment.scree_cover_fraction > 0.28
        && form_roll < 0.34 + (environment.scree_cover_fraction * 0.44)
    {
        RockForm::ScreeFragment
    } else if environment.carbonate_fraction > 0.52
        && form_roll < 0.24 + (environment.carbonate_fraction * 0.34)
    {
        RockForm::Slab
    } else if form_roll
        < 0.18
            + (environment.weathering_fraction * 0.52)
            + (environment.moisture_staining_fraction * 0.12)
    {
        RockForm::RoundedBoulder
    } else {
        RockForm::AngularBlock
    };
    let size_roll = random_fraction(id, 0x5349_5a45);
    let rare_scale = if size_roll > 0.985 { 2.6 } else { 1.0 };
    let base_radius = (0.10
        + (environment.mean_radius_meters * (0.18 + (size_roll * size_roll * 1.42))))
        * rare_scale;
    let width_variation = lerp(0.72, 1.28, random_fraction(id, 0x0057_4944_5448));
    let depth_variation = lerp(0.68, 1.32, random_fraction(id, 0x0044_4550_5448));
    let (horizontal_scale, vertical_scale) = match form {
        RockForm::RoundedBoulder => (1.0, 0.82),
        RockForm::AngularBlock => (0.94, 0.76),
        RockForm::Slab => (1.28, 0.34),
        RockForm::ScreeFragment => (0.46, 0.38),
    };
    let radii_meters = [
        (base_radius * horizontal_scale * width_variation).clamp(0.08, 6.0),
        (base_radius * vertical_scale).clamp(0.06, 4.0),
        (base_radius * horizontal_scale * depth_variation).clamp(0.08, 6.0),
    ];
    let direction_turns = random_fraction(id, 0x5449_4c54_4449);
    let direction_angle = direction_turns * std::f64::consts::TAU;
    let tilt_direction = [libm::cos(direction_angle), libm::sin(direction_angle)];
    let slope_fraction = (environment.slope / 0.22).clamp(0.0, 1.0);
    let tilt_fraction =
        (0.02 + (slope_fraction * 0.24) + (random_fraction(id, 0x5449_4c54) * 0.12))
            .clamp(0.0, 0.42);
    let embedded_base = match form {
        RockForm::RoundedBoulder => 0.22,
        RockForm::AngularBlock => 0.16,
        RockForm::Slab => 0.10,
        RockForm::ScreeFragment => 0.08,
    };
    let embedded_fraction =
        (embedded_base + (random_fraction(id, 0x0045_4d42_4544) * 0.22)).clamp(0.08, 0.48);
    let moss_fraction = (environment.moisture_staining_fraction
        * lerp(0.35, 1.0, random_fraction(id, 0x4d4f_5353))
        * (0.55 + (environment.weathering_fraction * 0.45)))
        .clamp(0.0, 1.0);
    let fracture_fraction = ((1.0 - environment.weathering_fraction) * 0.55
        + (environment.hardness_fraction * 0.30)
        + (random_fraction(id, 0x4652_4143) * 0.15))
        .clamp(0.0, 1.0);
    let roundness_fraction = match form {
        RockForm::RoundedBoulder => lerp(0.68, 1.0, environment.weathering_fraction),
        RockForm::AngularBlock => lerp(0.08, 0.34, environment.weathering_fraction),
        RockForm::Slab => lerp(0.18, 0.46, environment.weathering_fraction),
        RockForm::ScreeFragment => lerp(0.06, 0.28, environment.weathering_fraction),
    };

    SurfaceRock {
        id,
        x,
        z,
        radii_meters,
        rotation_turns: random_fraction(id, 0x524f_5441_5445),
        tilt_direction,
        tilt_fraction,
        embedded_fraction,
        moss_fraction,
        genotype: RockGenotype {
            form,
            hardness_fraction: environment.hardness_fraction,
            weathering_fraction: environment.weathering_fraction,
            fracture_fraction,
            roundness_fraction,
            carbonate_fraction: environment.carbonate_fraction,
        },
    }
}

fn stochastic_count(expected: f64, rounding_fraction: f64) -> u8 {
    let mut count = 0_u8;
    let mut remainder = expected.max(0.0);
    while remainder >= 1.0 && count < 8 {
        count += 1;
        remainder -= 1.0;
    }
    count + u8::from(count < 8 && rounding_fraction < remainder)
}

fn random_fraction(key: u64, domain: u64) -> f64 {
    hash53_as_f64(stable_hash(&[key, domain]) >> 11) / 9_007_199_254_740_991.0
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: i64) -> f64 {
    index as f64
}

#[allow(clippy::cast_precision_loss)]
fn hash53_as_f64(hash: u64) -> f64 {
    hash as f64
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROCK_WORLD: WorldIdentity = WorldIdentity::new(0x5eed, SURFACE_ROCK_GENERATOR_VERSION, 0);

    #[test]
    fn distribution_is_deterministic_bounded_and_explainable() {
        let distribution = SurfaceRockDistribution::new(ROCK_WORLD);
        let first = distribution
            .sample(-73_125.0, 19_875.0)
            .expect("rock distribution");
        let second = distribution
            .sample(-73_125.0, 19_875.0)
            .expect("same rock distribution");

        assert_eq!(first, second);
        assert!((0.0..=2_100.0).contains(&first.density_per_hectare));
        assert!((0.12..=1.5).contains(&first.mean_radius_meters));
        assert!((0.0..=1.0).contains(&first.rock_exposure_fraction));
        assert!((0.0..=1.0).contains(&first.scree_cover_fraction));
        assert!((0.0..=1.0).contains(&first.moisture_staining_fraction));
    }

    #[test]
    fn surface_rocks_are_deterministic_bounded_and_geologically_valid() {
        let generator = SurfaceRocks::new(ROCK_WORLD);
        let bounds = RockBounds::new(-128.0, -128.0, 128.0, 128.0).expect("valid bounds");
        let first = generator.rocks_in(bounds).expect("rock generation");
        let second = generator.rocks_in(bounds).expect("same rock generation");

        assert_eq!(first, second);
        assert!(!first.is_empty());
        assert!(first.iter().all(|rock| {
            bounds.contains(rock.x, rock.z)
                && rock.radii_meters.into_iter().all(|radius| radius > 0.0)
                && (0.0..=0.42).contains(&rock.tilt_fraction)
                && (0.08..=0.48).contains(&rock.embedded_fraction)
                && (0.0..=1.0).contains(&rock.moss_fraction)
                && (libm::hypot(rock.tilt_direction[0], rock.tilt_direction[1]) - 1.0).abs()
                    < 1.0e-12
        }));
        assert!(first.windows(2).all(|pair| pair[0].id <= pair[1].id));
    }

    #[test]
    fn adjacent_requests_match_one_combined_request_at_negative_boundaries() {
        let generator = SurfaceRocks::new(ROCK_WORLD);
        let combined = RockBounds::new(-64.0, -64.0, 64.0, 64.0).expect("combined");
        let mut tiled = [
            RockBounds::new(-64.0, -64.0, 0.0, 0.0).expect("southwest"),
            RockBounds::new(0.0, -64.0, 64.0, 0.0).expect("southeast"),
            RockBounds::new(-64.0, 0.0, 0.0, 64.0).expect("northwest"),
            RockBounds::new(0.0, 0.0, 64.0, 64.0).expect("northeast"),
        ]
        .into_iter()
        .rev()
        .flat_map(|bounds| generator.rocks_in(bounds).expect("tile generation"))
        .collect::<Vec<_>>();
        tiled.sort_by_key(|rock| rock.id);

        assert_eq!(
            generator.rocks_in(combined).expect("combined generation"),
            tiled
        );
    }

    #[test]
    fn distant_regions_produce_distinct_rock_character() {
        let distribution = SurfaceRockDistribution::new(ROCK_WORLD);
        let samples = [
            distribution.sample(-820_000.0, -640_000.0).expect("rocks"),
            distribution.sample(-240_000.0, 510_000.0).expect("rocks"),
            distribution.sample(370_000.0, -760_000.0).expect("rocks"),
            distribution.sample(910_000.0, 430_000.0).expect("rocks"),
        ];
        let density_range = samples
            .iter()
            .map(|sample| sample.density_per_hectare)
            .fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
            );
        let hardness_range = samples.iter().map(|sample| sample.hardness_fraction).fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
        );

        assert!(density_range.1 - density_range.0 > 10.0);
        assert!(hardness_range.1 - hardness_range.0 > 0.05);
    }

    #[test]
    fn rock_generation_requires_its_versioned_contract() {
        let old_world = WorldIdentity::new(0x5eed, SURFACE_ROCK_GENERATOR_VERSION - 1, 0);
        let bounds = RockBounds::new(0.0, 0.0, 32.0, 32.0).expect("valid bounds");

        assert!(
            SurfaceRockDistribution::new(old_world)
                .sample(0.0, 0.0)
                .is_none()
        );
        assert!(SurfaceRocks::new(old_world).rocks_in(bounds).is_none());
    }

    #[test]
    fn surface_rocks_have_a_golden_fingerprint() {
        let rocks = SurfaceRocks::new(ROCK_WORLD)
            .rocks_in(RockBounds::new(-96.0, -64.0, 96.0, 64.0).expect("bounds"))
            .expect("rock generation");
        let fingerprint = stable_hash(
            &rocks
                .iter()
                .flat_map(|rock| {
                    [
                        rock.id,
                        rock.x.to_bits(),
                        rock.z.to_bits(),
                        rock.radii_meters[0].to_bits(),
                        rock.radii_meters[1].to_bits(),
                        rock.radii_meters[2].to_bits(),
                        rock.rotation_turns.to_bits(),
                        rock.tilt_direction[0].to_bits(),
                        rock.tilt_direction[1].to_bits(),
                        rock.tilt_fraction.to_bits(),
                        rock.embedded_fraction.to_bits(),
                        rock.moss_fraction.to_bits(),
                        rock_form_fingerprint(rock.genotype.form),
                        rock.genotype.hardness_fraction.to_bits(),
                        rock.genotype.weathering_fraction.to_bits(),
                        rock.genotype.fracture_fraction.to_bits(),
                        rock.genotype.roundness_fraction.to_bits(),
                        rock.genotype.carbonate_fraction.to_bits(),
                    ]
                })
                .collect::<Vec<_>>(),
        );

        assert_eq!(
            fingerprint, 7_971_873_006_945_869_553,
            "changing this value changes generated surface rocks"
        );
    }

    const fn rock_form_fingerprint(form: RockForm) -> u64 {
        match form {
            RockForm::RoundedBoulder => 0,
            RockForm::AngularBlock => 1,
            RockForm::Slab => 2,
            RockForm::ScreeFragment => 3,
        }
    }
}
