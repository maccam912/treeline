//! Closed-form reference surfaces used to pin down meshing and streaming.
//!
//! The player-facing world is measured, so it cannot be evaluated at arbitrary
//! coordinates or reduced to a hand-checkable expectation. These two fields
//! can: they are exact, unbounded, and free of embedded data, which makes them
//! the fixtures that meshing, LOD, and chunk-boundary contracts are stated
//! against.

use treeline_coordinates::WorldPosition;

use crate::{DensityField, Material, SurfaceField, TerrainSample};

/// A horizontal ground plane at a fixed elevation.
///
/// The simplest field satisfying the signed-density contract, and the one whose
/// meshed output is fully predictable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundPlane {
    pub surface_height: f64,
    pub material: Material,
}

impl DensityField for GroundPlane {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        let density = position.y - self.surface_height;
        TerrainSample::new(
            density,
            if density > 0.0 {
                Material::Air
            } else {
                self.material
            },
        )
    }
}

impl SurfaceField for GroundPlane {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        (x.is_finite() && z.is_finite()).then_some(self.surface_height)
    }
}

/// A smooth, non-repeating hill field with curvature on two horizontal scales.
///
/// Meshing tests need a surface that is neither flat nor axis-aligned, so that
/// triangle counts, shared chunk boundaries, and LOD coarsening are actually
/// exercised. The two wavelengths are relatively prime so no chunk edge lands
/// on a repeating pattern.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SmoothHills;

impl SmoothHills {
    const BROAD_WAVELENGTH_METERS: f64 = 96.0;
    const BROAD_AMPLITUDE_METERS: f64 = 6.0;
    const FINE_WAVELENGTH_METERS: f64 = 37.0;
    const FINE_AMPLITUDE_METERS: f64 = 1.5;

    /// Evaluates the surface elevation directly, without the density contract.
    pub fn height_at(x: f64, z: f64) -> f64 {
        let broad = std::f64::consts::TAU / Self::BROAD_WAVELENGTH_METERS;
        let fine = std::f64::consts::TAU / Self::FINE_WAVELENGTH_METERS;
        (libm::sin(x * broad) * libm::cos(z * broad) * Self::BROAD_AMPLITUDE_METERS)
            + (libm::sin((x + z) * fine) * Self::FINE_AMPLITUDE_METERS)
    }
}

impl DensityField for SmoothHills {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        let density = position.y - Self::height_at(position.x, position.z);
        TerrainSample::new(
            density,
            if density > 0.0 {
                Material::Air
            } else if density > -1.5 {
                Material::Soil
            } else {
                Material::Rock
            },
        )
    }
}

impl SurfaceField for SmoothHills {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        (x.is_finite() && z.is_finite()).then(|| Self::height_at(x, z))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_plane_is_solid_below_and_air_above_its_surface() {
        let plane = GroundPlane {
            surface_height: 12.5,
            material: Material::Soil,
        };
        assert!(plane.sample(WorldPosition::new(0.0, 13.0, 0.0)).density > 0.0);
        assert!(plane.sample(WorldPosition::new(0.0, 12.0, 0.0)).is_solid());
        assert_eq!(plane.surface_height(4.0, -9.0), Some(12.5));
    }

    #[test]
    fn smooth_hills_agree_between_the_density_and_surface_contracts() {
        for [x, z] in [[0.0, 0.0], [37.5, -128.25], [-1_024.0, 2_048.5]] {
            let height = SmoothHills.surface_height(x, z).expect("finite position");
            let at_surface = SmoothHills.sample(WorldPosition::new(x, height, z)).density;
            assert!(at_surface.abs() < 1.0e-9);
        }
    }

    #[test]
    fn smooth_hills_are_not_flat() {
        let heights = (0..16)
            .map(|step| SmoothHills::height_at(f64::from(step) * 7.0, 3.0))
            .collect::<Vec<_>>();
        let minimum = heights.iter().copied().fold(f64::INFINITY, f64::min);
        let maximum = heights.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(maximum - minimum > 1.0);
    }
}
