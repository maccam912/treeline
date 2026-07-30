use std::collections::BTreeMap;

use treeline_coordinates::{CellIndex, WorldIdentity, stable_hash};
use treeline_geography::Climate;

use crate::{ECOSYSTEM_GENERATOR_VERSION, EcosystemDistribution, ForestDistribution, Soil};

/// Generator version that first exposes deterministic ground vegetation.
pub const GROUND_VEGETATION_GENERATOR_VERSION: u32 = 13;

const DOMAIN_GROUND_VEGETATION_PATCHES: u64 = 0x4752_4f55_4e44_5041;
const DOMAIN_GROUND_VEGETATION_INDIVIDUALS: u64 = 0x4752_4f55_4e44_494e;
const PATCH_FIELD_EDGE_METERS: f64 = 180.0;
const PLACEMENT_CELL_EDGE_METERS: f64 = 2.5;
const ENVIRONMENT_CELL_EDGE_METERS: f64 = 40.0;

/// A ground-layer growth strategy, used as continuous composition rather than a biome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroundCoverGroup {
    Graminoid,
    Forb,
    Fern,
    LowShrub,
    Moss,
}

impl GroundCoverGroup {
    pub const ALL: [Self; 5] = [
        Self::Graminoid,
        Self::Forb,
        Self::Fern,
        Self::LowShrub,
        Self::Moss,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Graminoid => "grass and sedge",
            Self::Forb => "flowering forb",
            Self::Fern => "fern",
            Self::LowShrub => "low shrub",
            Self::Moss => "moss cushion",
        }
    }

    const fn typical_height_meters(self) -> f64 {
        match self {
            Self::Graminoid => 0.42,
            Self::Forb => 0.58,
            Self::Fern => 0.72,
            Self::LowShrub => 1.05,
            Self::Moss => 0.10,
        }
    }
}

/// Relative abundance of ground-layer growth strategies at one location.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundVegetationComposition {
    pub graminoid_fraction: f64,
    pub forb_fraction: f64,
    pub fern_fraction: f64,
    pub low_shrub_fraction: f64,
    pub moss_fraction: f64,
}

impl GroundVegetationComposition {
    pub fn fraction(self, group: GroundCoverGroup) -> f64 {
        match group {
            GroundCoverGroup::Graminoid => self.graminoid_fraction,
            GroundCoverGroup::Forb => self.forb_fraction,
            GroundCoverGroup::Fern => self.fern_fraction,
            GroundCoverGroup::LowShrub => self.low_shrub_fraction,
            GroundCoverGroup::Moss => self.moss_fraction,
        }
    }

    pub fn dominant(self) -> GroundCoverGroup {
        let mut dominant = GroundCoverGroup::Graminoid;
        for group in [
            GroundCoverGroup::Forb,
            GroundCoverGroup::Fern,
            GroundCoverGroup::LowShrub,
            GroundCoverGroup::Moss,
        ] {
            if self.fraction(group) > self.fraction(dominant) {
                dominant = group;
            }
        }
        dominant
    }
}

/// Explainable continuous ground-layer conditions at one horizontal position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundVegetationSample {
    pub ground_cover_fraction: f64,
    pub patch_density_per_hectare: f64,
    pub mean_height_meters: f64,
    pub flowering_fraction: f64,
    pub sunlight_fraction: f64,
    pub composition: GroundVegetationComposition,
}

impl GroundVegetationSample {
    pub fn dominant_group(self) -> GroundCoverGroup {
        self.composition.dominant()
    }
}

/// Functional ground-vegetation sampler derived from climate, soil, and forest structure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroundVegetationDistribution {
    pub world: WorldIdentity,
}

impl GroundVegetationDistribution {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Samples ground cover without assigning a biome or consulting simulation state.
    #[allow(clippy::too_many_lines)]
    pub fn sample(self, x: f64, z: f64) -> Option<GroundVegetationSample> {
        if self.world.generator_version < GROUND_VEGETATION_GENERATOR_VERSION {
            return None;
        }

        let climate = Climate::new(self.world).sample(x, z)?;
        let soil = Soil::new(self.world).sample(x, z)?;
        let forest = ForestDistribution::new(self.world).sample(x, z)?;
        let ecosystem = if self.world.generator_version >= ECOSYSTEM_GENERATOR_VERSION {
            Some(EcosystemDistribution::new(self.world).sample(x, z)?)
        } else {
            None
        };
        let warmth = climate.warmth_fraction();
        let moisture = soil.surface_moisture;
        let depth = soil.depth_fraction();
        let exposure = soil.rock_exposure;
        let slope = (soil.slope / 0.22).clamp(0.0, 1.0);
        let canopy = forest.canopy_cover_fraction;
        let sunlight = (0.96 - (canopy * 0.78)).clamp(0.08, 1.0);
        let shade = 1.0 - sunlight;
        let disturbance = forest.disturbance_severity;
        let permanent_snow =
            (climate.permanent_snowpack_water_equivalent_millimeters / 1_200.0).clamp(0.0, 1.0);

        let temperate_vigor = (1.0 - ((warmth - 0.56).abs() * 1.55)).clamp(0.0, 1.0);
        let mut graminoid = (0.12 + (sunlight * 0.72) + (disturbance * 0.34))
            * (0.34 + (moisture * 0.66))
            * (0.45 + (temperate_vigor * 0.55));
        let mut forb = (0.08 + (sunlight * 0.74))
            * (0.28 + (depth * 0.72))
            * (0.35 + (temperate_vigor * 0.65))
            * (1.0 - (moisture - 0.58).abs() * 0.72).clamp(0.24, 1.0)
            * (0.72 + (disturbance * 0.28));
        let mut fern = (0.06 + (shade * 0.94))
            * (0.12 + (moisture * 0.88))
            * (0.35 + (depth * 0.65))
            * (0.72 + (soil.acidity_fraction() * 0.28));
        let mut low_shrub = (0.18 + (sunlight * 0.42) + (shade * 0.22))
            * (0.36 + (depth * 0.64))
            * (0.52 + ((1.0 - disturbance) * 0.48))
            * (0.62 + ((1.0 - moisture) * warmth * 0.38));
        let mut moss = (0.08 + (shade * 0.74))
            * (0.14 + (moisture * 0.86))
            * (0.52 + (soil.acidity_fraction() * 0.28) + (exposure * 0.20));
        if let Some(ecosystem) = ecosystem {
            graminoid *= 0.18
                + (ecosystem.grassland_prairie_potential * 2.10)
                + (ecosystem.steppe_potential * 1.15)
                + (ecosystem.tundra_potential * 0.45)
                + (ecosystem.wetland_potential * 0.40);
            forb *= 0.30
                + (ecosystem.grassland_prairie_potential * 0.72)
                + (ecosystem.open_woodland_potential * 0.34)
                + (ecosystem.wetland_potential * 0.42);
            fern *= 0.18
                + (ecosystem.closed_forest_potential * 1.72)
                + (ecosystem.wetland_potential * 0.82);
            low_shrub *= 0.20
                + (ecosystem.shrubland_potential * 2.0)
                + (ecosystem.steppe_potential * 0.72)
                + (ecosystem.desert_potential * 0.58)
                + (ecosystem.tundra_potential * 0.68)
                + (ecosystem.open_woodland_potential * 0.48);
            moss *= 0.20
                + (ecosystem.tundra_potential * 1.58)
                + (ecosystem.wetland_potential * 1.08)
                + (ecosystem.closed_forest_potential * 0.42)
                + (ecosystem.exposed_alpine_potential * 0.52);
        }
        let total = graminoid + forb + fern + low_shrub + moss;
        let composition = GroundVegetationComposition {
            graminoid_fraction: graminoid / total,
            forb_fraction: forb / total,
            fern_fraction: fern / total,
            low_shrub_fraction: low_shrub / total,
            moss_fraction: moss / total,
        };

        let patchiness = value_field(
            self.world,
            DOMAIN_GROUND_VEGETATION_PATCHES,
            x,
            z,
            PATCH_FIELD_EDGE_METERS,
        )?;
        let substrate =
            (0.18 + (depth * 0.82)) * (1.0 - (exposure * 0.70)) * (1.0 - (slope * 0.58));
        let water_vigor = (0.30 + (moisture * 0.92) - (moisture * moisture * 0.22)).clamp(0.0, 1.0);
        let snow_free = 1.0 - permanent_snow;
        let mut ground_cover_fraction = (substrate
            * water_vigor
            * (0.44 + (temperate_vigor * 0.56))
            * (0.58 + (patchiness * 0.58))
            * (0.62 + (canopy * 0.14))
            * snow_free)
            .clamp(0.0, 1.0);
        if let Some(ecosystem) = ecosystem {
            let target_cover = ((ecosystem.closed_forest_potential * 0.46)
                + (ecosystem.open_woodland_potential * 0.62)
                + (ecosystem.grassland_prairie_potential * 0.98)
                + (ecosystem.steppe_potential * 0.68)
                + (ecosystem.shrubland_potential * 0.64)
                + (ecosystem.desert_potential * 0.12)
                + (ecosystem.tundra_potential * 0.54)
                + (ecosystem.wetland_potential * 0.88))
                .clamp(0.0, 1.0);
            ground_cover_fraction = ((((ground_cover_fraction * 0.24)
                + (target_cover * substrate * (0.56 + (patchiness * 0.48))))
                * (1.0 - (ecosystem.exposed_alpine_potential * 0.82)))
                * ecosystem.land_fraction)
                .clamp(0.0, 1.0);
        }
        let patch_density_per_hectare =
            (ground_cover_fraction * (680.0 + (1_720.0 * patchiness))).clamp(0.0, 2_400.0);
        let mut mean_height_meters = GroundCoverGroup::ALL
            .into_iter()
            .map(|group| composition.fraction(group) * group.typical_height_meters())
            .sum::<f64>()
            * (0.58 + (ground_cover_fraction * 0.62));
        if let Some(ecosystem) = ecosystem {
            mean_height_meters *= (1.0
                - (ecosystem.desert_potential * 0.42)
                - (ecosystem.tundra_potential * 0.24)
                - (ecosystem.exposed_alpine_potential * 0.48))
                .clamp(0.18, 1.0);
        }
        let flowering_fraction = (composition.forb_fraction
            * sunlight
            * (0.35 + (warmth * 0.65))
            * (0.76 + (moisture * 0.24)))
            .clamp(0.0, 1.0);

        Some(GroundVegetationSample {
            ground_cover_fraction,
            patch_density_per_hectare,
            mean_height_meters,
            flowering_fraction,
            sunlight_fraction: sunlight,
            composition,
        })
    }
}

/// Half-open horizontal area used to request ground-vegetation individuals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundVegetationBounds {
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
}

impl GroundVegetationBounds {
    /// Creates finite, non-empty ground-vegetation bounds.
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

/// Growth traits consumed by the renderer for one ground-vegetation patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundPlantGenotype {
    pub group: GroundCoverGroup,
    pub leaf_count: u8,
    pub spread_fraction: f64,
    pub slenderness_fraction: f64,
    pub color_variation_fraction: f64,
}

/// One deterministic ground-vegetation patch positioned on the global lattice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundPlant {
    pub id: u64,
    pub x: f64,
    pub z: f64,
    pub height_meters: f64,
    pub radius_meters: f64,
    pub rotation_turns: f64,
    pub lean_direction: [f64; 2],
    pub lean_fraction: f64,
    pub flowering_fraction: f64,
    pub genotype: GroundPlantGenotype,
}

/// Functional generator for spatially stable ground-vegetation individuals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroundVegetation {
    pub world: WorldIdentity,
}

impl GroundVegetation {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Generates all ground-vegetation patch centers inside a half-open area.
    ///
    /// A global 2.5-meter lattice owns candidates. Coarser environmental cells
    /// control density and composition, while stable hashes control individual
    /// placement and form. Tiled requests therefore match combined requests.
    pub fn plants_in(self, bounds: GroundVegetationBounds) -> Option<Vec<GroundPlant>> {
        if self.world.generator_version < GROUND_VEGETATION_GENERATOR_VERSION {
            return None;
        }
        let minimum =
            CellIndex::containing(bounds.min_x, bounds.min_z, 0, PLACEMENT_CELL_EDGE_METERS)?;
        let maximum =
            CellIndex::containing(bounds.max_x, bounds.max_z, 0, PLACEMENT_CELL_EDGE_METERS)?;
        let distribution = GroundVegetationDistribution::new(self.world);
        let climate = Climate::new(self.world);
        let mut environments = BTreeMap::new();
        let mut plants = Vec::new();
        let mut cell_z = minimum.z;
        loop {
            let mut cell_x = minimum.x;
            loop {
                let cell = CellIndex::new(cell_x, cell_z, 0);
                let origin_x = index_as_f64(cell_x) * PLACEMENT_CELL_EDGE_METERS;
                let origin_z = index_as_f64(cell_z) * PLACEMENT_CELL_EDGE_METERS;
                let center_x = origin_x + (PLACEMENT_CELL_EDGE_METERS * 0.5);
                let center_z = origin_z + (PLACEMENT_CELL_EDGE_METERS * 0.5);
                let environment_cell =
                    CellIndex::containing(center_x, center_z, 0, ENVIRONMENT_CELL_EDGE_METERS)?;
                let environment_key = (environment_cell.x, environment_cell.z);
                let environment = if let Some(environment) = environments.get(&environment_key) {
                    *environment
                } else {
                    let environment_x =
                        (index_as_f64(environment_cell.x) + 0.5) * ENVIRONMENT_CELL_EDGE_METERS;
                    let environment_z =
                        (index_as_f64(environment_cell.z) + 0.5) * ENVIRONMENT_CELL_EDGE_METERS;
                    let environment = (
                        distribution.sample(environment_x, environment_z)?,
                        climate
                            .sample(environment_x, environment_z)?
                            .prevailing_wind,
                    );
                    environments.insert(environment_key, environment);
                    environment
                };
                let cell_key =
                    cell.generation_key(self.world, DOMAIN_GROUND_VEGETATION_INDIVIDUALS);
                let expected_count = environment.0.patch_density_per_hectare
                    * PLACEMENT_CELL_EDGE_METERS
                    * PLACEMENT_CELL_EDGE_METERS
                    / 10_000.0;
                let count =
                    stochastic_count(expected_count, random_fraction(cell_key, 0x0043_4f55_4e54));
                for ordinal in 0..count {
                    let id = stable_hash(&[cell_key, u64::from(ordinal)]);
                    let x = origin_x
                        + (PLACEMENT_CELL_EDGE_METERS
                            * lerp(0.08, 0.92, random_fraction(id, 0x585f_4a49_5454)));
                    let z = origin_z
                        + (PLACEMENT_CELL_EDGE_METERS
                            * lerp(0.08, 0.92, random_fraction(id, 0x5a5f_4a49_5454)));
                    if bounds.contains(x, z) {
                        plants.push(ground_plant(id, x, z, environment.0, environment.1));
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
        plants.sort_by_key(|plant| plant.id);
        Some(plants)
    }
}

fn ground_plant(
    id: u64,
    x: f64,
    z: f64,
    environment: GroundVegetationSample,
    prevailing_wind: [f64; 2],
) -> GroundPlant {
    let group = select_group(
        environment.composition,
        random_fraction(id, 0x0047_524f_5550),
    );
    let vigor = lerp(0.62, 1.34, random_fraction(id, 0x0056_4947_4f52))
        * (0.72 + (environment.ground_cover_fraction * 0.38));
    let group_height = group.typical_height_meters();
    let height_meters = (group_height * vigor).clamp(0.04, 1.65);
    let radius_scale = match group {
        GroundCoverGroup::Graminoid => 0.58,
        GroundCoverGroup::Forb => 0.44,
        GroundCoverGroup::Fern => 0.72,
        GroundCoverGroup::LowShrub => 0.84,
        GroundCoverGroup::Moss => 1.65,
    };
    let radius_meters =
        (height_meters * radius_scale * lerp(0.72, 1.28, random_fraction(id, 0x5241_4449_5553)))
            .clamp(0.08, 1.1);
    let leaf_count = match group {
        GroundCoverGroup::Graminoid | GroundCoverGroup::Fern => {
            4 + random_bounded(id, 0x4c45_4146, 4)
        }
        GroundCoverGroup::Forb | GroundCoverGroup::Moss => 3 + random_bounded(id, 0x4c45_4146, 4),
        GroundCoverGroup::LowShrub => 4 + random_bounded(id, 0x4c45_4146, 5),
    };
    let random_direction_angle = random_fraction(id, 0x4449_5245_4354) * std::f64::consts::TAU;
    let random_direction = [
        libm::cos(random_direction_angle),
        libm::sin(random_direction_angle),
    ];
    let wind_influence = match group {
        GroundCoverGroup::Graminoid | GroundCoverGroup::Forb | GroundCoverGroup::Fern => 0.72,
        GroundCoverGroup::LowShrub => 0.38,
        GroundCoverGroup::Moss => 0.0,
    };
    let lean_direction = normalized_direction([
        (prevailing_wind[0] * wind_influence) + (random_direction[0] * (1.0 - wind_influence)),
        (prevailing_wind[1] * wind_influence) + (random_direction[1] * (1.0 - wind_influence)),
    ]);

    GroundPlant {
        id,
        x,
        z,
        height_meters,
        radius_meters,
        rotation_turns: random_fraction(id, 0x524f_5441_5445),
        lean_direction,
        lean_fraction: match group {
            GroundCoverGroup::Graminoid => lerp(0.08, 0.32, random_fraction(id, 0x4c45_414e)),
            GroundCoverGroup::Forb => lerp(0.03, 0.16, random_fraction(id, 0x4c45_414e)),
            GroundCoverGroup::Fern => lerp(0.04, 0.20, random_fraction(id, 0x4c45_414e)),
            GroundCoverGroup::LowShrub => lerp(0.01, 0.08, random_fraction(id, 0x4c45_414e)),
            GroundCoverGroup::Moss => 0.0,
        },
        flowering_fraction: if group == GroundCoverGroup::Forb {
            (environment.flowering_fraction
                * lerp(0.62, 1.25, random_fraction(id, 0x464c_4f57_4552)))
            .clamp(0.0, 1.0)
        } else {
            0.0
        },
        genotype: GroundPlantGenotype {
            group,
            leaf_count,
            spread_fraction: lerp(0.55, 1.0, random_fraction(id, 0x5350_5245_4144)),
            slenderness_fraction: lerp(0.25, 1.0, random_fraction(id, 0x534c_454e_4445)),
            color_variation_fraction: random_fraction(id, 0x0043_4f4c_4f52),
        },
    }
}

fn select_group(composition: GroundVegetationComposition, selection: f64) -> GroundCoverGroup {
    let mut cumulative = 0.0;
    for group in GroundCoverGroup::ALL {
        cumulative += composition.fraction(group);
        if selection <= cumulative {
            return group;
        }
    }
    GroundCoverGroup::Moss
}

fn value_field(world: WorldIdentity, domain: u64, x: f64, z: f64, edge: f64) -> Option<f64> {
    let cell = CellIndex::containing(x, z, 0, edge)?;
    let origin_x = index_as_f64(cell.x) * edge;
    let origin_z = index_as_f64(cell.z) * edge;
    let blend_x = smoothstep(((x - origin_x) / edge).clamp(0.0, 1.0));
    let blend_z = smoothstep(((z - origin_z) / edge).clamp(0.0, 1.0));
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

fn stochastic_count(expected: f64, rounding_fraction: f64) -> u8 {
    let mut count = 0_u8;
    let mut remainder = expected.max(0.0);
    while remainder >= 1.0 && count < 3 {
        count += 1;
        remainder -= 1.0;
    }
    count + u8::from(count < 3 && rounding_fraction < remainder)
}

fn random_fraction(key: u64, domain: u64) -> f64 {
    hash53_as_f64(stable_hash(&[key, domain]) >> 11) / 9_007_199_254_740_991.0
}

fn random_bounded(key: u64, domain: u64, exclusive_maximum: u8) -> u8 {
    let hash = stable_hash(&[key, domain]);
    u8::try_from(hash % u64::from(exclusive_maximum)).expect("bounded hash fits u8")
}

fn normalized_direction(direction: [f64; 2]) -> [f64; 2] {
    let length = libm::hypot(direction[0], direction[1]);
    if length <= f64::EPSILON {
        [1.0, 0.0]
    } else {
        [direction[0] / length, direction[1] / length]
    }
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

    const VEGETATION_WORLD: WorldIdentity =
        WorldIdentity::new(0x5eed, GROUND_VEGETATION_GENERATOR_VERSION, 0);

    #[test]
    fn distribution_is_deterministic_bounded_and_explainable() {
        let distribution = GroundVegetationDistribution::new(VEGETATION_WORLD);
        let first = distribution
            .sample(-73_125.0, 19_875.0)
            .expect("ground vegetation");
        let second = distribution
            .sample(-73_125.0, 19_875.0)
            .expect("same ground vegetation");
        let composition_total = GroundCoverGroup::ALL
            .into_iter()
            .map(|group| first.composition.fraction(group))
            .sum::<f64>();

        assert_eq!(first, second);
        assert!((0.0..=1.0).contains(&first.ground_cover_fraction));
        assert!((0.0..=2_400.0).contains(&first.patch_density_per_hectare));
        assert!((0.0..=1.0).contains(&first.flowering_fraction));
        assert!((0.08..=1.0).contains(&first.sunlight_fraction));
        assert!((composition_total - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn plants_are_deterministic_bounded_and_structurally_valid() {
        let generator = GroundVegetation::new(VEGETATION_WORLD);
        let bounds =
            GroundVegetationBounds::new(-128.0, -128.0, 128.0, 128.0).expect("valid bounds");
        let first = generator.plants_in(bounds).expect("plant generation");
        let second = generator.plants_in(bounds).expect("same plant generation");

        assert_eq!(first, second);
        assert!(!first.is_empty());
        assert!(first.iter().all(|plant| {
            bounds.contains(plant.x, plant.z)
                && (0.04..=1.65).contains(&plant.height_meters)
                && (0.08..=1.1).contains(&plant.radius_meters)
                && (3..=8).contains(&plant.genotype.leaf_count)
                && (0.0..=1.0).contains(&plant.flowering_fraction)
                && (libm::hypot(plant.lean_direction[0], plant.lean_direction[1]) - 1.0).abs()
                    < 1.0e-12
        }));
        assert!(first.windows(2).all(|pair| pair[0].id <= pair[1].id));
    }

    #[test]
    fn adjacent_requests_match_one_combined_request_at_negative_boundaries() {
        let generator = GroundVegetation::new(VEGETATION_WORLD);
        let combined =
            GroundVegetationBounds::new(-64.0, -64.0, 64.0, 64.0).expect("combined bounds");
        let mut tiled = [
            GroundVegetationBounds::new(-64.0, -64.0, 0.0, 0.0).expect("southwest"),
            GroundVegetationBounds::new(0.0, -64.0, 64.0, 0.0).expect("southeast"),
            GroundVegetationBounds::new(-64.0, 0.0, 0.0, 64.0).expect("northwest"),
            GroundVegetationBounds::new(0.0, 0.0, 64.0, 64.0).expect("northeast"),
        ]
        .into_iter()
        .rev()
        .flat_map(|bounds| generator.plants_in(bounds).expect("tile generation"))
        .collect::<Vec<_>>();
        tiled.sort_by_key(|plant| plant.id);

        assert_eq!(
            generator.plants_in(combined).expect("combined generation"),
            tiled
        );
    }

    #[test]
    fn distant_regions_produce_distinct_ground_layers() {
        let distribution = GroundVegetationDistribution::new(VEGETATION_WORLD);
        let samples = [
            distribution.sample(-820_000.0, -640_000.0).expect("cover"),
            distribution.sample(-240_000.0, 510_000.0).expect("cover"),
            distribution.sample(370_000.0, -760_000.0).expect("cover"),
            distribution.sample(910_000.0, 430_000.0).expect("cover"),
        ];
        let density_range = samples
            .iter()
            .map(|sample| sample.patch_density_per_hectare)
            .fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
            );
        let fern_range = samples
            .iter()
            .map(|sample| sample.composition.fern_fraction)
            .fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
            );

        assert!(density_range.1 - density_range.0 > 20.0);
        assert!(fern_range.1 - fern_range.0 > 0.02);
    }

    #[test]
    fn ground_vegetation_requires_its_versioned_contract() {
        let old_world = WorldIdentity::new(0x5eed, GROUND_VEGETATION_GENERATOR_VERSION - 1, 0);
        let bounds = GroundVegetationBounds::new(0.0, 0.0, 32.0, 32.0).expect("valid bounds");

        assert!(
            GroundVegetationDistribution::new(old_world)
                .sample(0.0, 0.0)
                .is_none()
        );
        assert!(GroundVegetation::new(old_world).plants_in(bounds).is_none());
    }

    #[test]
    fn version_eighteen_ground_layers_express_grass_shrub_desert_and_tundra_structure() {
        let world = WorldIdentity::new(0x5eed, ECOSYSTEM_GENERATOR_VERSION, 0);
        let ecosystems = EcosystemDistribution::new(world);
        let ground = GroundVegetationDistribution::new(world);
        let mut best = [[0.0, 0.0]; 4];
        let mut maxima = [f64::NEG_INFINITY; 4];
        for z in -8..=8 {
            for x in -8..=8 {
                let position = [f64::from(x) * 384_000.0, f64::from(z) * 384_000.0];
                let ecosystem = ecosystems
                    .sample(position[0], position[1])
                    .expect("ecosystem");
                let potentials = [
                    ecosystem.grassland_prairie_potential,
                    ecosystem.shrubland_potential,
                    ecosystem.desert_potential,
                    ecosystem.tundra_potential,
                ];
                for (index, potential) in potentials.into_iter().enumerate() {
                    if potential > maxima[index] {
                        maxima[index] = potential;
                        best[index] = position;
                    }
                }
            }
        }

        let grass = ground
            .sample(best[0][0], best[0][1])
            .expect("grassland ground layer");
        let shrub = ground
            .sample(best[1][0], best[1][1])
            .expect("shrubland ground layer");
        let desert = ground
            .sample(best[2][0], best[2][1])
            .expect("desert ground layer");
        let tundra = ground
            .sample(best[3][0], best[3][1])
            .expect("tundra ground layer");

        assert!(grass.composition.graminoid_fraction > 0.34);
        assert!(shrub.composition.low_shrub_fraction > 0.30);
        assert!(desert.ground_cover_fraction < grass.ground_cover_fraction);
        assert!(tundra.composition.low_shrub_fraction + tundra.composition.moss_fraction > 0.34);
    }

    #[test]
    fn ground_vegetation_has_a_golden_fingerprint() {
        let plants = GroundVegetation::new(VEGETATION_WORLD)
            .plants_in(GroundVegetationBounds::new(-96.0, -64.0, 96.0, 64.0).expect("valid bounds"))
            .expect("plant generation");
        let fingerprint = stable_hash(
            &plants
                .iter()
                .flat_map(|plant| {
                    [
                        plant.id,
                        plant.x.to_bits(),
                        plant.z.to_bits(),
                        plant.height_meters.to_bits(),
                        plant.radius_meters.to_bits(),
                        plant.rotation_turns.to_bits(),
                        plant.lean_direction[0].to_bits(),
                        plant.lean_direction[1].to_bits(),
                        plant.lean_fraction.to_bits(),
                        plant.flowering_fraction.to_bits(),
                        group_fingerprint(plant.genotype.group),
                        u64::from(plant.genotype.leaf_count),
                        plant.genotype.spread_fraction.to_bits(),
                        plant.genotype.slenderness_fraction.to_bits(),
                        plant.genotype.color_variation_fraction.to_bits(),
                    ]
                })
                .collect::<Vec<_>>(),
        );

        assert_eq!(
            fingerprint, 2_353_682_241_847_834_133,
            "changing this value changes generated ground vegetation"
        );
    }

    const fn group_fingerprint(group: GroundCoverGroup) -> u64 {
        match group {
            GroundCoverGroup::Graminoid => 0,
            GroundCoverGroup::Forb => 1,
            GroundCoverGroup::Fern => 2,
            GroundCoverGroup::LowShrub => 3,
            GroundCoverGroup::Moss => 4,
        }
    }
}
