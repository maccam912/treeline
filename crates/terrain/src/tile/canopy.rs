//! Measured forest structure decoded from the bundle's `.tlcanopy` layer.

use super::raster::{HEADER_BYTES, Raster, read_u8};

const ARTIFACT: &[u8] = include_bytes!("../../assets/michigan_tile.tlcanopy");
const MAGIC: &[u8; 8] = b"TLCAN01\0";
/// Half-meter quantization of the terrain-normalized canopy-top height.
const HEIGHT_QUANTIZATION_METERS: f64 = 0.5;

/// Lidar-derived forest structure of one canopy cell.
///
/// These are aggregate stand measurements. They bound how much forest stands
/// where and how tall it grows; they do not identify individual trees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanopySample {
    /// Fraction of the cell's source cells holding an above-ground return.
    pub cover_fraction: f64,
    /// Tallest terrain-normalized return in the cell, in meters.
    pub top_height_meters: f64,
}

/// Reads the canopy layer header and checks the payload length.
///
/// # Panics
///
/// Panics when the embedded artifact does not match the bundle contract.
pub fn decode() -> Raster {
    let raster = Raster::decode(ARTIFACT, *MAGIC);
    assert_eq!(
        ARTIFACT.len(),
        HEADER_BYTES + (raster.cell_count() * 2),
        "surveyed canopy payload length matches its header"
    );
    raster
}

/// Reads the canopy cell containing a horizontal position.
///
/// Cover is a per-cell aggregate rather than a continuous field, so this reads
/// the containing cell instead of interpolating between stands. Positions
/// outside the measured footprint report no canopy rather than a nearby one.
pub fn canopy_at(raster: Raster, x: f64, z: f64) -> Option<CanopySample> {
    let (cell_x, cell_z) = raster.containing_cell(x, z, 0.0)?;
    let cell_x = usize::try_from(cell_x).ok()?;
    let cell_z = usize::try_from(cell_z).ok()?;
    let offset = HEADER_BYTES + (raster.slot(cell_x, cell_z) * 2);
    Some(CanopySample {
        cover_fraction: f64::from(read_u8(ARTIFACT, offset)) / 255.0,
        top_height_meters: f64::from(read_u8(ARTIFACT, offset + 1)) * HEIGHT_QUANTIZATION_METERS,
    })
}

#[cfg(test)]
pub fn samples() -> impl Iterator<Item = (u8, u8)> {
    ARTIFACT[HEADER_BYTES..]
        .chunks_exact(2)
        .map(|sample| (sample[0], sample[1]))
}
