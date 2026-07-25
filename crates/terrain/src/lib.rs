//! Functional signed-density fields for smooth terrain.

use treeline_coordinates::WorldPosition;
use treeline_coordinates::{CellIndex, WorldIdentity};

const DOMAIN_ROLLING_HILLS: u64 = 0x524f_4c4c_494e_4753;

/// Broad material channels used before renderer-specific material expansion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Material {
    Air,
    Bedrock,
    Rock,
    Soil,
    Sand,
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
}
