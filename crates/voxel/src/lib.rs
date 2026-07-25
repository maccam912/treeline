//! Smooth-voxel sampling and level-of-detail alignment.

use treeline_coordinates::WorldPosition;
use treeline_terrain::{DensityField, TerrainSample};

/// A voxel resolution level. Level zero uses half-meter sample spacing.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LodLevel(u8);

impl LodLevel {
    pub const BASE_SPACING_METERS: f64 = 0.5;

    pub const fn new(level: u8) -> Self {
        Self(level)
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub fn spacing_meters(self) -> f64 {
        Self::BASE_SPACING_METERS * 2.0_f64.powi(i32::from(self.0))
    }

    /// Snaps a coordinate to this LOD's sample lattice.
    pub fn align(self, coordinate: f64) -> Option<f64> {
        if !coordinate.is_finite() {
            return None;
        }
        let spacing = self.spacing_meters();
        Some((coordinate / spacing).floor() * spacing)
    }
}

/// Samples a functional terrain field on an LOD-aligned lattice.
pub fn sample_aligned(
    field: &impl DensityField,
    position: WorldPosition,
    lod: LodLevel,
) -> Option<TerrainSample> {
    let aligned = WorldPosition::new(
        lod.align(position.x)?,
        lod.align(position.y)?,
        lod.align(position.z)?,
    );
    Some(field.sample(aligned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_terrain::{GroundPlane, Material};

    #[test]
    fn spacing_doubles_at_each_lod() {
        for level in 0..8 {
            let current = LodLevel::new(level).spacing_meters();
            let next = LodLevel::new(level + 1).spacing_meters();
            assert!((next - (current * 2.0)).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn coarse_samples_align_with_the_fine_lattice() {
        let position = 13.37;
        let coarse = LodLevel::new(4).align(position).expect("finite");
        let fine = LodLevel::new(0).align(coarse).expect("finite");
        assert!((coarse - fine).abs() < f64::EPSILON);
    }

    #[test]
    fn negative_coordinates_align_downward() {
        let aligned = LodLevel::new(1).align(-0.1).expect("finite");
        assert!((aligned - -1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn aligned_sampling_is_repeatable() {
        let ground = GroundPlane {
            surface_height: 10.0,
            material: Material::Soil,
        };
        let position = WorldPosition::new(0.2, 10.7, -0.2);
        assert_eq!(
            sample_aligned(&ground, position, LodLevel::new(1)),
            sample_aligned(&ground, position, LodLevel::new(1))
        );
    }
}
