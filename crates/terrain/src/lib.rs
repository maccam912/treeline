//! Terrain sampling for Treeline's surveyed world.
//!
//! Terrain is exposed two ways. [`DensityField`] is the volumetric contract the
//! near world is meshed from; [`SurfaceField`] is the cheaper heightfield the
//! distant world uses. Both describe the same surface, so the two
//! representations stay aligned.
//!
//! [`SurveyedTerrain`] implements both from the embedded measured bundle.
//! [`GroundPlane`] and [`SmoothHills`] implement both from a closed-form
//! expression, and exist so meshing contracts can be stated against something
//! checkable.

mod analytic;
mod tile;

pub use analytic::{GroundPlane, SmoothHills};
pub use tile::{
    CanopySample, LakeSample, SURVEYED_SETTINGS_HASH, SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z,
    SURVEYED_TILE_EDGE_METERS, WATER_MASK_SPACING_METERS,
};

use treeline_coordinates::WorldPosition;

/// Surface substance reported alongside signed density.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Material {
    #[default]
    Air,
    Soil,
    Rock,
    Sand,
    Scree,
}

/// One terrain evaluation: how far inside or outside the surface, and of what.
///
/// Density is negative inside solid terrain, zero on the surface, and positive
/// in air.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainSample {
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

/// Volumetric terrain, sampled anywhere in three dimensions.
pub trait DensityField {
    fn sample(&self, position: WorldPosition) -> TerrainSample;
}

/// Terrain as a heightfield, for representations that need no volume.
///
/// Distant terrain is a surface rather than a volume, so it only needs the
/// elevation of the ground under a horizontal position.
pub trait SurfaceField {
    /// Surface elevation in meters, or `None` outside the field's domain.
    fn surface_height(&self, x: f64, z: f64) -> Option<f64>;

    /// Vertical extent a near-world mesher must cover over a footprint.
    ///
    /// A pure heightfield needs no extra range: the mesher can bracket the
    /// surface it already knows about. A field with overhangs or voids returns
    /// the range those features occupy.
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

/// The measured Michigan bundle, sampled as terrain.
///
/// Elevation comes straight from the bare-earth model at 1:1 horizontal and
/// vertical scale, so density is simply height above that surface. The bundle
/// has no overhangs or voids, which is why [`SurfaceField::volume_bounds`]
/// keeps its default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SurveyedTerrain;

impl SurveyedTerrain {
    /// Bare-earth elevation in meters above the bundle's vertical datum.
    pub fn height_at(self, x: f64, z: f64) -> Option<f64> {
        tile::height_at(x, z)
    }

    /// Natural-color surface appearance from the bundle's aerial imagery.
    pub fn color_at(self, x: f64, z: f64) -> Option<[f32; 4]> {
        tile::color_at(x, z)
    }

    /// The mapped lake covering a horizontal position, if any.
    pub fn lake_at(self, x: f64, z: f64) -> Option<LakeSample> {
        tile::lake_at(x, z)
    }

    /// Measured canopy cover and height at a horizontal position.
    pub fn canopy_at(self, x: f64, z: f64) -> Option<CanopySample> {
        tile::canopy_at(x, z)
    }
}

impl DensityField for SurveyedTerrain {
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

impl SurfaceField for SurveyedTerrain {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        self.height_at(x, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_is_zero_on_the_measured_surface() {
        let height = SurveyedTerrain
            .height_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z)
            .expect("spawn is measured");
        let at_surface = SurveyedTerrain.sample(WorldPosition::new(
            SURVEYED_SPAWN_X,
            height,
            SURVEYED_SPAWN_Z,
        ));

        assert!(at_surface.density.abs() < 1.0e-9);
        assert!(
            SurveyedTerrain
                .sample(WorldPosition::new(
                    SURVEYED_SPAWN_X,
                    height + 1.0,
                    SURVEYED_SPAWN_Z
                ))
                .density
                > 0.0
        );
        assert!(
            SurveyedTerrain
                .sample(WorldPosition::new(
                    SURVEYED_SPAWN_X,
                    height - 1.0,
                    SURVEYED_SPAWN_Z
                ))
                .is_solid()
        );
    }

    #[test]
    fn sampling_is_repeatable_and_order_independent() {
        let positions = [
            [SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z],
            [1_024.5, 8_192.25],
            [7_364.0, 6_894.0],
        ];
        let forward = positions.map(|[x, z]| SurveyedTerrain.height_at(x, z));
        let mut reversed = positions;
        reversed.reverse();
        let mut backward = reversed.map(|[x, z]| SurveyedTerrain.height_at(x, z));
        backward.reverse();

        assert_eq!(forward, backward);
    }

    #[test]
    fn a_measured_bundle_reports_no_extra_volume() {
        assert_eq!(SurveyedTerrain.volume_bounds(0.0, 0.0, 128.0, 128.0), None);
    }
}
