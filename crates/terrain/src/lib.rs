//! Functional signed-density fields for smooth terrain.

use treeline_coordinates::WorldPosition;

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
}
