//! Functional signed-density fields for smooth terrain.

use treeline_coordinates::WorldPosition;
use treeline_coordinates::{CellIndex, WorldIdentity};
use treeline_geography::{
    Climate, MacroElevation, MacroTerrainSample, OROGRAPHIC_CLIMATE_GENERATOR_VERSION,
    PROVINCE_GENERATOR_VERSION, ProvincePlan, ProvinceSample, RegionalProfile,
};

const DOMAIN_ROLLING_HILLS: u64 = 0x524f_4c4c_494e_4753;
const DOMAIN_WILDERNESS_DETAIL: u64 = 0x5749_4c44_4445_544c;
const DOMAIN_EROSION_MICRO: u64 = 0x4552_4f53_4d49_4352;
const DOMAIN_PROVINCE_REGIONAL_RELIEF: u64 = 0x5052_4f56_5245_474c;
const DOMAIN_PROVINCE_LANDSCAPE_RELIEF: u64 = 0x5052_4f56_4c41_4e44;
const DOMAIN_PROVINCE_RUGGED_RELIEF: u64 = 0x5052_4f56_5255_4747;
const DOMAIN_PROVINCE_GLACIAL_RELIEF: u64 = 0x5052_4f56_474c_4143;
const DOMAIN_PROVINCE_DUNE_RIPPLES: u64 = 0x5052_4f56_4455_4e45;
const EROSION_SLOPE_SAMPLE_RADIUS_METERS: f64 = 256.0;

/// Broad material channels used before renderer-specific material expansion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Material {
    Air,
    Bedrock,
    Rock,
    Soil,
    Sand,
    Scree,
}

/// A deterministic sample of pristine terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainSample {
    /// Signed distance-like density in meters: negative is solid.
    pub density: f64,
    pub material: Material,
}

impl TerrainSample {
    pub const fn new(density: f64, material: Material) -> Self {
        Self { density, material }
    }

    pub fn is_solid(self) -> bool {
        self.density <= 0.0
    }
}

/// A terrain source that can be evaluated independently at any position.
pub trait DensityField {
    fn sample(&self, position: WorldPosition) -> TerrainSample;
}

/// A terrain source with a single-valued surface suitable for distant meshes.
///
/// Near terrain remains a volumetric [`DensityField`]. Implementing this trait
/// opts a field into the cheaper far representation where caves and overhangs
/// are intentionally not evaluated.
pub trait SurfaceField {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64>;

    /// Extends the vertical interval that a volumetric mesh must sample.
    ///
    /// The default heightfield implementation needs no layers away from the
    /// visible surface. Fields with caves or overhangs can widen this interval
    /// for one horizontal footprint without changing their far-surface
    /// representation.
    fn volume_bounds(
        &self,
        _min_x: f64,
        _min_z: f64,
        _max_x: f64,
        _max_z: f64,
    ) -> Option<(f64, f64)> {
        None
    }
}

/// A horizontal ground plane useful for tests and the first terrain prototype.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundPlane {
    pub surface_height: f64,
    pub material: Material,
}

impl DensityField for GroundPlane {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        TerrainSample::new(position.y - self.surface_height, self.material)
    }
}

impl SurfaceField for GroundPlane {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        if x.is_finite() && z.is_finite() {
            Some(self.surface_height)
        } else {
            None
        }
    }
}

/// A deterministic rolling landscape used by the first playable terrain toy.
///
/// The field uses interpolated, stable-hash lattice values rather than
/// platform trigonometry. This keeps the same world inputs tied to the same
/// terrain while still producing broad hills with smaller undulations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RollingHills {
    pub world: WorldIdentity,
}

impl RollingHills {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Returns the terrain surface elevation at a horizontal world position.
    pub fn height_at(self, x: f64, z: f64) -> Option<f64> {
        let broad = height_layer(self.world, x, z, 48.0, DOMAIN_ROLLING_HILLS)?;
        let detail = height_layer(self.world, x, z, 18.0, DOMAIN_ROLLING_HILLS.wrapping_add(1))?;
        Some(3.0 + (broad * 13.0) + ((detail - 0.5) * 4.0))
    }
}

impl DensityField for RollingHills {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        let Some(surface_height) = self.height_at(position.x, position.z) else {
            return TerrainSample::new(f64::INFINITY, Material::Air);
        };
        let density = position.y - surface_height;
        let material = if density > 0.0 {
            Material::Air
        } else if density > -1.25 {
            Material::Soil
        } else {
            Material::Rock
        };
        TerrainSample::new(density, material)
    }
}

impl SurfaceField for RollingHills {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        self.height_at(x, z)
    }
}

/// The first terrain field to connect macro geography to playable detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WildernessTerrain {
    pub world: WorldIdentity,
}

/// Explainable local expression of a version-18 geographical province plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LandformSurfaceSample {
    pub macro_sample: MacroTerrainSample,
    pub province: ProvinceSample,
    pub plain_relief_meters: f64,
    pub rolling_upland_relief_meters: f64,
    pub plateau_terrace_meters: f64,
    pub rugged_mountain_relief_meters: f64,
    pub weathered_mountain_relief_meters: f64,
    pub glacial_relief_meters: f64,
    pub dune_ripple_meters: f64,
    pub ground_detail_meters: f64,
    pub surface_height_meters: f64,
}

/// Explainable non-fluvial erosion applied to a pristine terrain surface.
///
/// Macro weathering rounds uplifted terrain and deposits sediment on old,
/// low-gradient ground. Micro relief and surface composition respond to the
/// same slope, rock, climate, and erosion-age inputs instead of choosing a
/// terrain preset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ErosionSurfaceSample {
    pub base_height_meters: f64,
    pub macro_weathering_meters: f64,
    pub sediment_deposition_meters: f64,
    pub micro_relief_meters: f64,
    pub slope: f64,
    pub rock_exposure: f64,
    pub scree_cover: f64,
    pub soil_depth_meters: f64,
}

impl ErosionSurfaceSample {
    pub fn surface_height_meters(self) -> f64 {
        self.base_height_meters - self.macro_weathering_meters
            + self.sediment_deposition_meters
            + self.micro_relief_meters
    }
}

impl WildernessTerrain {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Samples condition-driven local morphology from the shared province plan.
    ///
    /// Broad province elevation already contains continental relief, tectonic
    /// systems, plateaus, scarps, glacial valleys, closed basins, and primary
    /// dune fields. This stage gives each cause an appropriate local surface
    /// vocabulary without changing the parent drainage surface or falling
    /// back to one globally uniform hill amplitude.
    pub fn landform_at(self, x: f64, z: f64) -> Option<LandformSurfaceSample> {
        if self.world.generator_version < PROVINCE_GENERATOR_VERSION {
            return None;
        }
        let macro_sample = MacroElevation::new(self.world).sample(x, z)?;
        let province = ProvincePlan::sample_at(self.world, x, z)?;
        let regional = height_layer(self.world, x, z, 9_600.0, DOMAIN_PROVINCE_REGIONAL_RELIEF)?;
        let landscape = height_layer(self.world, x, z, 2_400.0, DOMAIN_PROVINCE_LANDSCAPE_RELIEF)?;
        let rugged = height_layer(self.world, x, z, 1_100.0, DOMAIN_PROVINCE_RUGGED_RELIEF)?;
        let glacial = height_layer(self.world, x, z, 6_400.0, DOMAIN_PROVINCE_GLACIAL_RELIEF)?;
        let ground = height_layer(
            self.world,
            x,
            z,
            84.0,
            DOMAIN_WILDERNESS_DETAIL.wrapping_add(19),
        )?;

        let quiet_ground = (1.0 - province.plains * 0.84)
            * (1.0 - province.closed_basin * 0.76)
            * (1.0 - province.dune * 0.58);
        let plain_relief_meters =
            ((regional - 0.5) * 7.0 + (landscape - 0.5) * 1.8) * province.plains;
        let rolling_upland_relief_meters = ((regional - 0.5) * 142.0 + (landscape - 0.5) * 34.0)
            * province.rolling_uplands
            * (1.0 - province.mountain * 0.42);

        let terrace_step_meters = 56.0 + (province.strata_tilt * 128.0);
        let terraced_height = terraced_height(
            macro_sample.elevation_meters,
            terrace_step_meters,
            0.36,
            0.64,
        );
        let plateau_terrace_meters = (terraced_height - macro_sample.elevation_meters)
            * province.plateau
            * (0.58 + (province.rock_hardness * 0.34));

        let ridge = 1.0 - ((rugged * 2.0) - 1.0).abs();
        let sharp_ridge = ridge * ridge * (3.0 - (2.0 * ridge));
        let rugged_mountain_relief_meters = (sharp_ridge - 0.42)
            * (90.0 + (province.uplift * 290.0))
            * province.mountain
            * (1.0 - province.erosion * 0.72);
        let weathered_mountain_relief_meters = ((regional - 0.5) * 116.0
            + (landscape - 0.5) * 52.0)
            * province.mountain
            * province.erosion;

        let valley_axis = ((glacial * 2.0) - 1.0).abs();
        let valley_floor = 1.0 - smoothstep_range(0.12, 0.58, valley_axis);
        let glacial_relief_meters =
            (-34.0 - (province.glaciation * 210.0)) * valley_floor * province.glacial
                + ((landscape - 0.5) * 38.0 * province.glacial);

        let dune_ripple_meters = if let Some(dune) = province.dune_geometry {
            dune.detail_height_offset_meters
        } else {
            (height_layer(self.world, x, z, 96.0, DOMAIN_PROVINCE_DUNE_RIPPLES)? - 0.5)
                * 2.0
                * province.dune
        };

        let ground_detail_meters =
            (ground - 0.5) * 2.6 * quiet_ground * (0.38 + (province.exposure * 0.92));
        let surface_height_meters = macro_sample.elevation_meters
            + plain_relief_meters
            + rolling_upland_relief_meters
            + plateau_terrace_meters
            + rugged_mountain_relief_meters
            + weathered_mountain_relief_meters
            + glacial_relief_meters
            + dune_ripple_meters
            + ground_detail_meters;

        Some(LandformSurfaceSample {
            macro_sample,
            province,
            plain_relief_meters,
            rolling_upland_relief_meters,
            plateau_terrace_meters,
            rugged_mountain_relief_meters,
            weathered_mountain_relief_meters,
            glacial_relief_meters,
            dune_ripple_meters,
            ground_detail_meters,
            surface_height_meters,
        })
    }

    /// Returns both the explainable macro sample and the final local surface.
    pub fn inspect(self, x: f64, z: f64) -> Option<(MacroTerrainSample, f64)> {
        if self.world.generator_version >= PROVINCE_GENERATOR_VERSION {
            let landform = self.landform_at(x, z)?;
            return Some((landform.macro_sample, landform.surface_height_meters));
        }
        let macro_sample = MacroElevation::new(self.world).sample(x, z)?;
        let foothills = height_layer(self.world, x, z, 420.0, DOMAIN_WILDERNESS_DETAIL)?;
        let ground_detail = height_layer(
            self.world,
            x,
            z,
            72.0,
            DOMAIN_WILDERNESS_DETAIL.wrapping_add(1),
        )?;
        let local_relief = ((foothills - 0.5) * 22.0) + ((ground_detail - 0.5) * 4.0);
        Some((macro_sample, macro_sample.elevation_meters + local_relief))
    }

    pub fn height_at(self, x: f64, z: f64) -> Option<f64> {
        self.inspect(x, z).map(|(_, height)| height)
    }

    /// Evaluates terrain density while preserving the shared far-surface zero.
    ///
    /// A coherent cavity band beneath strong scarp faces creates true undercuts
    /// in near terrain. The carving field is negative at the far-surface
    /// height, so both representations keep exactly the same visible zero
    /// surface while the near representation can expose volume below it.
    pub fn density_at_surface(self, position: WorldPosition, surface_height: f64) -> f64 {
        let vertical_density = position.y - surface_height;
        if self.world.generator_version < PROVINCE_GENERATOR_VERSION {
            return vertical_density;
        }
        let Some(province) = ProvincePlan::sample_at(self.world, position.x, position.z) else {
            return f64::INFINITY;
        };
        let Some(scarp) = province.scarp_geometry else {
            return vertical_density;
        };
        if scarp.face_strength < 0.12 || scarp.undercut_depth_meters <= 0.5 {
            return vertical_density;
        }
        if scarp.signed_distance_meters <= 0.0 {
            return vertical_density;
        }

        let depth = scarp.undercut_depth_meters;
        let below_surface = surface_height - position.y;
        let vertical_band = (depth * 0.44) - (below_surface - (depth * 0.66)).abs();
        let horizontal_band =
            (depth * 1.18) - (scarp.signed_distance_meters - (depth * 0.46)).abs();
        let cavity_density = vertical_band.min(horizontal_band) * scarp.face_strength;
        vertical_density.max(cavity_density)
    }

    /// Returns the deepest scarp cavity intersecting a horizontal rectangle.
    pub fn undercut_depth_in(self, min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> Option<f64> {
        if self.world.generator_version < PROVINCE_GENERATOR_VERSION
            || ![min_x, min_z, max_x, max_z].into_iter().all(f64::is_finite)
            || min_x > max_x
            || min_z > max_z
        {
            return None;
        }
        let center_x = (min_x + max_x) * 0.5;
        let center_z = (min_z + max_z) * 0.5;
        let scarp = ProvincePlan::sample_at(self.world, center_x, center_z)?.scarp_geometry?;
        if scarp.face_strength < 0.12 || scarp.undercut_depth_meters <= 0.5 {
            return None;
        }
        let signed_distances = [
            [min_x, min_z],
            [max_x, min_z],
            [min_x, max_z],
            [max_x, max_z],
        ]
        .map(|[x, z]| {
            scarp.signed_distance_meters
                + ((x - center_x) * scarp.face_normal[0])
                + ((z - center_z) * scarp.face_normal[1])
        });
        let minimum = signed_distances.into_iter().fold(f64::INFINITY, f64::min);
        let maximum = signed_distances
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max);
        (maximum > 0.0 && minimum < scarp.undercut_depth_meters * 1.64)
            .then_some(scarp.undercut_depth_meters)
    }

    /// Extends near-volume sampling around version-18 scarp undercuts.
    pub fn volumetric_bounds(
        self,
        min_x: f64,
        min_z: f64,
        max_x: f64,
        max_z: f64,
    ) -> Option<(f64, f64)> {
        let depth = self.undercut_depth_in(min_x, min_z, max_x, max_z)?;
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for z_fraction in [0.0, 0.5, 1.0] {
            for x_fraction in [0.0, 0.5, 1.0] {
                let x = min_x + ((max_x - min_x) * x_fraction);
                let z = min_z + ((max_z - min_z) * z_fraction);
                let surface = self.height_at(x, z)?;
                minimum = minimum.min(surface - depth - 2.0);
                maximum = maximum.max(surface + 2.0);
            }
        }
        Some((minimum, maximum))
    }

    /// Samples macro weathering, sediment deposition, and micro surface form.
    ///
    /// Central differences deliberately use a fixed world-space radius, so the
    /// erosion result is independent of voxel LOD and sampling order.
    pub fn erosion_at(self, x: f64, z: f64) -> Option<ErosionSurfaceSample> {
        let (macro_sample, base_height_meters) = self.inspect(x, z)?;
        let profile = RegionalProfile::sample(self.world, x, z)?;
        let precipitation = if self.world.generator_version >= OROGRAPHIC_CLIMATE_GENERATOR_VERSION
        {
            Climate::new(self.world)
                .sample(x, z)?
                .precipitation_fraction()
        } else {
            profile.precipitation
        };
        let left = self.height_at(x - EROSION_SLOPE_SAMPLE_RADIUS_METERS, z)?;
        let right = self.height_at(x + EROSION_SLOPE_SAMPLE_RADIUS_METERS, z)?;
        let down = self.height_at(x, z - EROSION_SLOPE_SAMPLE_RADIUS_METERS)?;
        let up = self.height_at(x, z + EROSION_SLOPE_SAMPLE_RADIUS_METERS)?;
        let sample_span = EROSION_SLOPE_SAMPLE_RADIUS_METERS * 2.0;
        let slope_x = (right - left) / sample_span;
        let slope_z = (up - down) / sample_span;
        let slope = libm::hypot(slope_x, slope_z);

        let softness = 1.0 - profile.rock_hardness;
        let macro_weathering_meters = macro_sample.mountain_uplift_meters
            * (0.04 + (profile.erosion_age * 0.16))
            * (0.35 + (softness * 0.65));
        let flatness = 1.0 - (slope / 0.08).clamp(0.0, 1.0);
        let lowland = 1.0 - (macro_sample.elevation_meters.max(0.0) / 600.0).clamp(0.0, 1.0);
        let sediment_deposition_meters = 18.0
            * profile.erosion_age
            * precipitation
            * (0.25 + (softness * 0.75))
            * flatness
            * lowland;

        let steepness = (slope / 0.12).clamp(0.0, 1.0);
        let rock_exposure = (steepness
            * (0.35 + (profile.rock_hardness * 0.65))
            * (0.4 + ((1.0 - profile.erosion_age) * 0.6)))
            .clamp(0.0, 1.0);
        let scree_cover = (((steepness - 0.18) / 0.62).clamp(0.0, 1.0)
            * (0.3 + (profile.erosion_age * 0.7))
            * (0.25 + (profile.rock_hardness * 0.75)))
            .clamp(0.0, 1.0);
        let soil_depth_meters = (0.2
            + (3.3 * flatness * (0.25 + (profile.erosion_age * 0.75)) * (0.3 + (softness * 0.7))))
            .clamp(0.2, 3.5);

        let coarse = height_layer(self.world, x, z, 42.0, DOMAIN_EROSION_MICRO)?;
        let fine = height_layer(self.world, x, z, 13.0, DOMAIN_EROSION_MICRO.wrapping_add(1))?;
        let micro_amplitude = 0.2 + (rock_exposure * 2.4) + (scree_cover * 1.2);
        let micro_relief_meters =
            ((coarse - 0.5) * 2.0 * micro_amplitude) + ((fine - 0.5) * 0.8 * scree_cover);

        Some(ErosionSurfaceSample {
            base_height_meters,
            macro_weathering_meters,
            sediment_deposition_meters,
            micro_relief_meters,
            slope,
            rock_exposure,
            scree_cover,
            soil_depth_meters,
        })
    }
}

impl DensityField for WildernessTerrain {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        let Some(surface_height) = self.height_at(position.x, position.z) else {
            return TerrainSample::new(f64::INFINITY, Material::Air);
        };
        let density = self.density_at_surface(position, surface_height);
        let material = if density > 0.0 {
            Material::Air
        } else if density > -1.5 {
            Material::Soil
        } else {
            Material::Rock
        };
        TerrainSample::new(density, material)
    }
}

impl SurfaceField for WildernessTerrain {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        self.height_at(x, z)
    }
}

fn height_layer(world: WorldIdentity, x: f64, z: f64, cell_size: f64, domain: u64) -> Option<f64> {
    let cell = CellIndex::containing(x, z, 0, cell_size)?;
    let local_x = (x / cell_size) - index_as_f64(cell.x);
    let local_z = (z / cell_size) - index_as_f64(cell.z);
    let blend_x = smoothstep(local_x);
    let blend_z = smoothstep(local_z);

    let bottom = lerp(
        lattice_value(world, cell.x, cell.z, domain),
        lattice_value(world, cell.x + 1, cell.z, domain),
        blend_x,
    );
    let top = lerp(
        lattice_value(world, cell.x, cell.z + 1, domain),
        lattice_value(world, cell.x + 1, cell.z + 1, domain),
        blend_x,
    );
    Some(lerp(bottom, top, blend_z))
}

fn lattice_value(world: WorldIdentity, x: i64, z: i64, domain: u64) -> f64 {
    let hash = CellIndex::new(x, z, 0).generation_key(world, domain);
    hash53_as_f64(hash >> 11) / 9_007_199_254_740_991.0
}

fn smoothstep(value: f64) -> f64 {
    value * value * (3.0 - (2.0 * value))
}

fn smoothstep_range(edge0: f64, edge1: f64, value: f64) -> f64 {
    let amount = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    smoothstep(amount)
}

fn terraced_height(height: f64, step: f64, transition_start: f64, transition_end: f64) -> f64 {
    let scaled = height / step;
    let shelf = libm::floor(scaled);
    let fraction = scaled - shelf;
    (shelf + smoothstep_range(transition_start, transition_end, fraction)) * step
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

/// A spherical solid that can represent a first boulder or density test shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    pub center: WorldPosition,
    pub radius: f64,
    pub material: Material,
}

impl DensityField for Sphere {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        let offset_x = position.x - self.center.x;
        let offset_y = position.y - self.center.y;
        let offset_z = position.z - self.center.z;
        let distance = libm::hypot(libm::hypot(offset_x, offset_y), offset_z);
        TerrainSample::new(distance - self.radius, self.material)
    }
}

/// The union of two density fields, preserving the material of the closer field.
#[derive(Clone, Copy, Debug)]
pub struct Union<A, B> {
    pub a: A,
    pub b: B,
}

impl<A: DensityField, B: DensityField> DensityField for Union<A, B> {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        let a = self.a.sample(position);
        let b = self.b.sample(position);
        if a.density <= b.density { a } else { b }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_coordinates::stable_hash;

    #[test]
    fn ground_density_uses_the_documented_sign() {
        let ground = GroundPlane {
            surface_height: 10.0,
            material: Material::Soil,
        };
        assert!(ground.sample(WorldPosition::new(0.0, 9.0, 0.0)).is_solid());
        assert!(!ground.sample(WorldPosition::new(0.0, 11.0, 0.0)).is_solid());
        let surface_density = ground.sample(WorldPosition::new(0.0, 10.0, 0.0)).density;
        assert!(surface_density.abs() < f64::EPSILON);
    }

    #[test]
    fn sphere_surface_is_at_zero() {
        let sphere = Sphere {
            center: WorldPosition::new(5.0, 5.0, 5.0),
            radius: 2.0,
            material: Material::Rock,
        };
        let surface_density = sphere.sample(WorldPosition::new(7.0, 5.0, 5.0)).density;
        assert!(surface_density.abs() < f64::EPSILON);
    }

    #[test]
    fn union_chooses_the_nearest_solid() {
        let field = Union {
            a: GroundPlane {
                surface_height: 0.0,
                material: Material::Soil,
            },
            b: Sphere {
                center: WorldPosition::new(0.0, 3.0, 0.0),
                radius: 1.0,
                material: Material::Rock,
            },
        };
        assert_eq!(
            field.sample(WorldPosition::new(0.0, 3.0, 0.0)).material,
            Material::Rock
        );
    }

    #[test]
    fn rolling_hills_are_deterministic_and_match_the_density_surface() {
        let hills = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let height = hills.height_at(-12.5, 37.25).expect("finite");
        assert_eq!(
            height.to_bits(),
            hills.height_at(-12.5, 37.25).expect("finite").to_bits()
        );
        let surface = hills.sample(WorldPosition::new(-12.5, height, 37.25));
        assert!(surface.density.abs() < f64::EPSILON);
    }

    #[test]
    fn rolling_hills_change_with_world_identity() {
        let old = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let new = RollingHills::new(WorldIdentity::new(0x5eed, 2, 0));
        assert_ne!(old.height_at(17.0, -29.0), new.height_at(17.0, -29.0));
    }

    #[test]
    fn rolling_hills_reject_non_finite_positions() {
        let hills = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        assert!(hills.height_at(f64::NAN, 0.0).is_none());
        assert!(
            !hills
                .sample(WorldPosition::new(f64::INFINITY, 0.0, 0.0))
                .is_solid()
        );
    }

    #[test]
    fn far_surface_contract_matches_the_volumetric_zero_surface() {
        let hills = RollingHills::new(WorldIdentity::new(0x5eed, 1, 0));
        let height = hills.surface_height(-96.0, 144.0).expect("finite surface");
        assert_eq!(
            hills
                .sample(WorldPosition::new(-96.0, height, 144.0))
                .density
                .to_bits(),
            0.0_f64.to_bits()
        );
    }

    #[test]
    fn wilderness_surface_matches_the_density_zero() {
        let terrain = WildernessTerrain::new(WorldIdentity::new(0x5eed, 1, 0));
        let height = terrain.height_at(-8_000.0, 12_000.0).expect("finite");
        assert_eq!(
            terrain
                .sample(WorldPosition::new(-8_000.0, height, 12_000.0))
                .density
                .to_bits(),
            0.0_f64.to_bits()
        );
    }

    #[test]
    fn wilderness_inspection_exposes_macro_contributors() {
        let terrain = WildernessTerrain::new(WorldIdentity::new(0x5eed, 1, 0));
        let (macro_sample, height) = terrain.inspect(4_000.0, -7_000.0).expect("finite");
        assert_eq!(
            macro_sample.elevation_meters.to_bits(),
            (macro_sample.base_elevation_meters + macro_sample.mountain_uplift_meters).to_bits()
        );
        assert!((height - macro_sample.elevation_meters).abs() <= 13.0);
    }

    #[test]
    fn erosion_surface_is_deterministic_and_composes_its_contributors() {
        let terrain = WildernessTerrain::new(WorldIdentity::new(0x5eed, 5, 0));
        let first = terrain
            .erosion_at(-8_125.5, 12_003.25)
            .expect("finite erosion sample");
        let second = terrain
            .erosion_at(-8_125.5, 12_003.25)
            .expect("same erosion sample");

        assert_eq!(first, second);
        assert_eq!(
            first.surface_height_meters().to_bits(),
            (first.base_height_meters - first.macro_weathering_meters
                + first.sediment_deposition_meters
                + first.micro_relief_meters)
                .to_bits()
        );
        assert!(first.macro_weathering_meters >= 0.0);
        assert!(first.sediment_deposition_meters >= 0.0);
        assert!((0.0..=1.0).contains(&first.rock_exposure));
        assert!((0.0..=1.0).contains(&first.scree_cover));
        assert!((0.2..=3.5).contains(&first.soil_depth_meters));
    }

    #[test]
    fn erosion_sampling_is_continuous_across_negative_micro_cells() {
        let terrain = WildernessTerrain::new(WorldIdentity::new(0x5eed, 5, 0));
        let left = terrain
            .erosion_at(-42.001, -13.0)
            .expect("left erosion sample")
            .surface_height_meters();
        let right = terrain
            .erosion_at(-41.999, -13.0)
            .expect("right erosion sample")
            .surface_height_meters();

        assert!((left - right).abs() < 0.1);
    }

    #[test]
    fn erosion_layers_create_weathering_deposition_and_surface_variety() {
        let terrain = WildernessTerrain::new(WorldIdentity::new(0x5eed, 5, 0));
        let mut maximum_weathering = 0.0_f64;
        let mut maximum_deposition = 0.0_f64;
        let mut maximum_rock_exposure = 0.0_f64;
        let mut maximum_scree_cover = 0.0_f64;
        for z in -8..=8 {
            for x in -8..=8 {
                let erosion = terrain
                    .erosion_at(f64::from(x) * 8_000.0, f64::from(z) * 8_000.0)
                    .expect("finite erosion sample");
                maximum_weathering = maximum_weathering.max(erosion.macro_weathering_meters);
                maximum_deposition = maximum_deposition.max(erosion.sediment_deposition_meters);
                maximum_rock_exposure = maximum_rock_exposure.max(erosion.rock_exposure);
                maximum_scree_cover = maximum_scree_cover.max(erosion.scree_cover);
            }
        }

        assert!(maximum_weathering > 20.0, "{maximum_weathering}");
        assert!(maximum_deposition > 1.0, "{maximum_deposition}");
        assert!(maximum_rock_exposure > 0.4, "{maximum_rock_exposure}");
        assert!(maximum_scree_cover > 0.3, "{maximum_scree_cover}");
    }

    #[test]
    fn province_landforms_are_deterministic_and_compose_exactly() {
        let terrain =
            WildernessTerrain::new(WorldIdentity::new(0x5eed, PROVINCE_GENERATOR_VERSION, 0));
        let first = terrain
            .landform_at(-812_375.0, 1_440_125.0)
            .expect("version 18 landform");
        let second = terrain
            .landform_at(-812_375.0, 1_440_125.0)
            .expect("same landform");
        let composed = first.macro_sample.elevation_meters
            + first.plain_relief_meters
            + first.rolling_upland_relief_meters
            + first.plateau_terrace_meters
            + first.rugged_mountain_relief_meters
            + first.weathered_mountain_relief_meters
            + first.glacial_relief_meters
            + first.dune_ripple_meters
            + first.ground_detail_meters;

        assert_eq!(first, second);
        assert_eq!(first.surface_height_meters.to_bits(), composed.to_bits());
        assert_eq!(
            terrain
                .height_at(-812_375.0, 1_440_125.0)
                .expect("height")
                .to_bits(),
            first.surface_height_meters.to_bits()
        );
    }

    #[test]
    fn version_eighteen_landform_surfaces_have_a_golden_fingerprint() {
        let terrain =
            WildernessTerrain::new(WorldIdentity::new(0x5eed, PROVINCE_GENERATOR_VERSION, 0));
        let positions = [
            [-1_420_125.0, 812_375.0],
            [-512_000.0, -0.001],
            [0.0, 0.0],
            [2_960_500.0, -4_180_250.0],
        ];
        let mut words = Vec::new();
        for [x, z] in positions {
            let sample = terrain.landform_at(x, z).expect("landform");
            words.extend([
                sample.macro_sample.elevation_meters.to_bits(),
                sample.plain_relief_meters.to_bits(),
                sample.rolling_upland_relief_meters.to_bits(),
                sample.plateau_terrace_meters.to_bits(),
                sample.rugged_mountain_relief_meters.to_bits(),
                sample.weathered_mountain_relief_meters.to_bits(),
                sample.glacial_relief_meters.to_bits(),
                sample.dune_ripple_meters.to_bits(),
                sample.ground_detail_meters.to_bits(),
                sample.surface_height_meters.to_bits(),
            ]);
        }

        assert_eq!(
            stable_hash(&words),
            6_616_975_272_258_147_281,
            "changing this value changes generator version 18 landform surfaces"
        );
    }

    #[test]
    fn old_worlds_retain_the_legacy_local_relief_contract() {
        let old = WildernessTerrain::new(WorldIdentity::new(
            0x5eed,
            PROVINCE_GENERATOR_VERSION - 1,
            0,
        ));
        assert!(old.landform_at(0.0, 0.0).is_none());
        assert_eq!(
            old.height_at(-8_000.0, 12_000.0)
                .expect("legacy height")
                .to_bits(),
            4_640_994_249_857_118_823
        );
    }

    #[test]
    fn version_18_density_keeps_the_far_surface_zero() {
        let terrain =
            WildernessTerrain::new(WorldIdentity::new(0x5eed, PROVINCE_GENERATOR_VERSION, 0));
        for [x, z] in [
            [-1_420_125.0, 812_375.0],
            [-512_000.0, -0.001],
            [0.0, 0.0],
            [2_960_500.0, -4_180_250.0],
        ] {
            let height = terrain.height_at(x, z).expect("surface");
            assert_eq!(
                terrain
                    .sample(WorldPosition::new(x, height, z))
                    .density
                    .to_bits(),
                0.0_f64.to_bits()
            );
        }
    }

    #[test]
    fn strong_scarps_create_real_undercut_air_below_the_far_surface() {
        let terrain =
            WildernessTerrain::new(WorldIdentity::new(0x5eed, PROVINCE_GENERATOR_VERSION, 0));
        let mut candidate = None;
        'outer: for z in -64..=64 {
            for x in -64..=64 {
                let world_x = f64::from(x) * 16_000.0;
                let world_z = f64::from(z) * 16_000.0;
                let province =
                    ProvincePlan::sample_at(terrain.world, world_x, world_z).expect("province");
                if let Some(scarp) = province.scarp_geometry
                    && scarp.face_strength >= 0.12
                    && scarp.undercut_depth_meters > 0.5
                {
                    candidate = Some((world_x, world_z, scarp));
                    break 'outer;
                }
            }
        }
        let (x, z, scarp) = candidate.expect("golden survey contains a strong scarp");
        let target_signed = scarp.undercut_depth_meters * 0.46;
        let shift = target_signed - scarp.signed_distance_meters;
        let target_x = x + (scarp.face_normal[0] * shift);
        let target_z = z + (scarp.face_normal[1] * shift);
        let target = ProvincePlan::sample_at(terrain.world, target_x, target_z)
            .and_then(|sample| sample.scarp_geometry)
            .unwrap_or_else(|| panic!("same scarp exposes a low-side undercut band: {scarp:?}"));
        let surface = terrain
            .height_at(target_x, target_z)
            .expect("target surface");
        let position = WorldPosition::new(
            target_x,
            surface - (target.undercut_depth_meters * 0.66),
            target_z,
        );
        let vertical_density = position.y - surface;
        let carved = terrain.density_at_surface(position, surface);

        assert!(vertical_density < 0.0);
        assert!(carved > 0.0, "{carved}");
        assert_eq!(
            terrain.sample(position).material,
            Material::Air,
            "the undercut must be a volumetric cavity rather than a color treatment"
        );
        assert!(
            terrain
                .undercut_depth_in(
                    target_x - 16.0,
                    target_z - 16.0,
                    target_x + 16.0,
                    target_z + 16.0,
                )
                .is_some()
        );

        let high_side_shift = (-scarp.undercut_depth_meters * 0.46) - scarp.signed_distance_meters;
        let high_x = x + (scarp.face_normal[0] * high_side_shift);
        let high_z = z + (scarp.face_normal[1] * high_side_shift);
        let high_scarp = ProvincePlan::sample_at(terrain.world, high_x, high_z)
            .and_then(|sample| sample.scarp_geometry)
            .expect("same scarp remains defined on its high side");
        assert!(high_scarp.signed_distance_meters < 0.0);
        let high_surface = terrain
            .height_at(high_x, high_z)
            .expect("high-side surface");
        let high_position = WorldPosition::new(
            high_x,
            high_surface - (high_scarp.undercut_depth_meters * 0.66),
            high_z,
        );
        let high_density = terrain.density_at_surface(high_position, high_surface);
        assert_eq!(
            high_density.to_bits(),
            (high_position.y - high_surface).to_bits(),
            "solid high ground must not inherit the low-side cavity"
        );
    }
}
