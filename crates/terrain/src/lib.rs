//! Functional signed-density fields for smooth terrain.

use treeline_coordinates::WorldPosition;
use treeline_coordinates::{CellIndex, WorldIdentity};
use treeline_geography::{MacroElevation, MacroTerrainSample, RegionalProfile};

const DOMAIN_ROLLING_HILLS: u64 = 0x524f_4c4c_494e_4753;
const DOMAIN_WILDERNESS_DETAIL: u64 = 0x5749_4c44_4445_544c;
const DOMAIN_EROSION_MICRO: u64 = 0x4552_4f53_4d49_4352;
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

    /// Returns both the explainable macro sample and the final local surface.
    pub fn inspect(self, x: f64, z: f64) -> Option<(MacroTerrainSample, f64)> {
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

    /// Samples macro weathering, sediment deposition, and micro surface form.
    ///
    /// Central differences deliberately use a fixed world-space radius, so the
    /// erosion result is independent of voxel LOD and sampling order.
    pub fn erosion_at(self, x: f64, z: f64) -> Option<ErosionSurfaceSample> {
        let (macro_sample, base_height_meters) = self.inspect(x, z)?;
        let profile = RegionalProfile::sample(self.world, x, z)?;
        let left = self.height_at(x - EROSION_SLOPE_SAMPLE_RADIUS_METERS, z)?;
        let right = self.height_at(x + EROSION_SLOPE_SAMPLE_RADIUS_METERS, z)?;
        let down = self.height_at(x, z - EROSION_SLOPE_SAMPLE_RADIUS_METERS)?;
        let up = self.height_at(x, z + EROSION_SLOPE_SAMPLE_RADIUS_METERS)?;
        let sample_span = EROSION_SLOPE_SAMPLE_RADIUS_METERS * 2.0;
        let slope_x = (right - left) / sample_span;
        let slope_z = (up - down) / sample_span;
        let slope = slope_x.hypot(slope_z);

        let softness = 1.0 - profile.rock_hardness;
        let macro_weathering_meters = macro_sample.mountain_uplift_meters
            * (0.04 + (profile.erosion_age * 0.16))
            * (0.35 + (softness * 0.65));
        let flatness = 1.0 - (slope / 0.08).clamp(0.0, 1.0);
        let lowland = 1.0 - (macro_sample.elevation_meters.max(0.0) / 600.0).clamp(0.0, 1.0);
        let sediment_deposition_meters = 18.0
            * profile.erosion_age
            * profile.precipitation
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
        let density = position.y - surface_height;
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
        let distance = offset_x.hypot(offset_y).hypot(offset_z);
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
}
