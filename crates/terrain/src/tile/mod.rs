//! The embedded surveyed bundle: one 10 km square of measured Michigan terrain.
//!
//! Four layers cover the same footprint in one projected frame whose units are
//! meters: bare-earth elevation, mapped lakes, lidar canopy, and natural-color
//! imagery. `SURVEYED_WORLD.md` is the contract these artifacts satisfy, and
//! `tools/surveyed_tile/prepare.py` produces them.
//!
//! Each layer decodes once on first use and is then read-only, so sampling is a
//! pure function of position with no ordering or wall-clock dependency.

mod canopy;
mod color;
mod elevation;
mod raster;
mod water;

use std::sync::OnceLock;

pub use canopy::CanopySample;
pub use water::{LakeSample, WATER_MASK_SPACING_METERS};

/// Versioned settings identity selecting this bundle.
///
/// Any incompatible change to a layer's bytes, coordinate frame, sampler, or
/// meaning must take a new value so saved worlds cannot silently change.
pub const SURVEYED_SETTINGS_HASH: u64 = 0x5355_5256_4559_0003;

/// Edge length of the bundle's footprint in local world meters.
pub const SURVEYED_TILE_EDGE_METERS: f64 = 10_000.0;

/// Spawn position, in local world meters east of the tile's west edge.
///
/// This is 46.16084629042455, -88.3374704874157 in the tile's local frame.
pub const SURVEYED_SPAWN_X: f64 = 6_737.563_408_352;

/// Spawn position, in local world meters south of the tile's north edge.
pub const SURVEYED_SPAWN_Z: f64 = 7_211.701_769_280;

static ELEVATION: OnceLock<elevation::Elevation> = OnceLock::new();
static COLOR: OnceLock<raster::Raster> = OnceLock::new();
static WATER: OnceLock<water::Water> = OnceLock::new();
static CANOPY: OnceLock<raster::Raster> = OnceLock::new();

/// Samples the bare-earth surface in meters above the bundle's vertical datum.
pub fn height_at(x: f64, z: f64) -> Option<f64> {
    is_finite(x, z).then(|| ELEVATION.get_or_init(elevation::decode).height_at(x, z))
}

/// Samples natural-color surface appearance, graded for the terrain shader.
pub fn color_at(x: f64, z: f64) -> Option<[f32; 4]> {
    is_finite(x, z).then(|| color::color_at(*COLOR.get_or_init(color::decode), x, z))
}

/// Samples the mapped lake covering a horizontal position, if any.
pub fn lake_at(x: f64, z: f64) -> Option<LakeSample> {
    is_finite(x, z)
        .then(|| WATER.get_or_init(water::decode).lake_at(x, z))
        .flatten()
}

/// Samples measured canopy cover and height at a horizontal position.
pub fn canopy_at(x: f64, z: f64) -> Option<CanopySample> {
    is_finite(x, z)
        .then(|| canopy::canopy_at(*CANOPY.get_or_init(canopy::decode), x, z))
        .flatten()
}

fn is_finite(x: f64, z: f64) -> bool {
    x.is_finite() && z.is_finite()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_metadata_agrees_with_the_selecting_identity() {
        assert!(
            include_str!("../../assets/michigan_tile.json")
                .contains("\"settings_identity\": \"0x5355525645590003\"")
        );
        assert_eq!(SURVEYED_SETTINGS_HASH, 0x5355_5256_4559_0003);
    }

    #[test]
    fn elevation_layer_decodes_with_the_expected_footprint() {
        let elevation = elevation::decode();
        let raster = elevation.raster();
        assert_eq!((raster.width, raster.height), (5_000, 5_000));
        assert_eq!(raster.spacing_meters.to_bits(), 2.0_f64.to_bits());
        assert_eq!(raster.west_pixel_center_x.to_bits(), 1.0_f64.to_bits());
        assert_eq!(raster.north_pixel_center_z.to_bits(), 9_999.0_f64.to_bits());
        assert_eq!(elevation.decimeters().iter().copied().min(), Some(4_061));
        assert_eq!(elevation.decimeters().iter().copied().max(), Some(4_874));
    }

    #[test]
    fn spawn_sits_inside_the_tile_at_a_measured_elevation() {
        let height = height_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z).expect("spawn is measured");
        assert!((406.0..=488.0).contains(&height));
    }

    #[test]
    fn world_z_increases_south_through_source_raster_rows() {
        let elevation = elevation::decode();
        let raster = elevation.raster();
        let north_west = f64::from(elevation.decimeters()[0]) * raster::QUANTIZATION_METERS;
        let south_west = f64::from(elevation.decimeters()[raster.slot(0, raster.height - 1)])
            * raster::QUANTIZATION_METERS;

        assert_eq!(height_at(1.0, 1.0), Some(north_west));
        assert_eq!(height_at(1.0, 9_999.0), Some(south_west));
    }

    #[test]
    fn color_stays_inside_the_unit_range() {
        let color = color_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z).expect("spawn has imagery");
        assert!(
            color
                .into_iter()
                .all(|channel| (0.0..=1.0).contains(&channel))
        );
    }

    #[test]
    fn mapped_lakes_keep_their_source_identity_and_level() {
        let upper_holmes = lake_at(7_364.0, 6_894.0).expect("Upper Holmes Lake is mapped");
        assert_eq!(upper_holmes.id, 19);
        assert!((upper_holmes.surface_elevation_meters - 415.5).abs() < f64::EPSILON);
        assert!(lake_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z).is_none());
    }

    #[test]
    fn lake_footprints_expand_one_cell_beyond_the_mapped_shore() {
        let water = water::decode();
        let (shore_x, shore_z, id) = water
            .east_facing_shore()
            .expect("surveyed water contains an east-facing shore");

        assert_eq!(lake_at(shore_x, shore_z).map(|lake| lake.id), Some(id));
    }

    #[test]
    fn canopy_layer_varies_cover_and_matches_the_spawn_stand() {
        let raster = canopy::decode();
        assert_eq!((raster.width, raster.height), (1_667, 1_667));
        assert_eq!(raster.spacing_meters.to_bits(), 6.0_f64.to_bits());
        assert!(canopy::samples().any(|(cover, _)| cover == 0));
        assert!(canopy::samples().any(|(cover, _)| cover == u8::MAX));
        assert!(canopy::samples().any(|(_, height)| height >= 50));

        let spawn = canopy_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z).expect("spawn has canopy");
        assert_eq!(spawn.cover_fraction.to_bits(), 1.0_f64.to_bits());
        assert_eq!(spawn.top_height_meters.to_bits(), 5.5_f64.to_bits());
    }

    #[test]
    fn non_finite_positions_are_rejected_by_every_layer() {
        assert_eq!(height_at(f64::NAN, 0.0), None);
        assert_eq!(color_at(0.0, f64::INFINITY), None);
        assert_eq!(lake_at(f64::NAN, 0.0), None);
        assert_eq!(canopy_at(f64::NAN, 0.0), None);
    }
}
