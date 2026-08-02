//! Natural-color surface appearance decoded from the bundle's `.tlrgb` layer.

use super::raster::{HEADER_BYTES, Raster, read_u16};

const ARTIFACT: &[u8] = include_bytes!("../../assets/michigan_tile.tlrgb");
const MAGIC: &[u8; 8] = b"TLRGB01\0";

/// Darkens raw aerial imagery so lit terrain does not read as washed out.
const SHADING_HEADROOM: f32 = 0.82;
/// Blend weight handed to the terrain shader's material treatment.
const MATERIAL_BLEND: f32 = 0.90;

/// Reads the color layer header and checks the payload length.
///
/// # Panics
///
/// Panics when the embedded artifact does not match the bundle contract.
pub fn decode() -> Raster {
    let raster = Raster::decode(ARTIFACT, *MAGIC);
    assert_eq!(
        ARTIFACT.len(),
        HEADER_BYTES + (raster.cell_count() * 2),
        "surveyed color payload length matches its header"
    );
    raster
}

/// Bilinearly samples RGB565 imagery, graded for the terrain shader.
pub fn color_at(raster: Raster, x: f64, z: f64) -> [f32; 4] {
    let cell = raster.bilinear_cell(x, z);
    let mut color = [0.0; 4];
    for (channel, value) in color[..3].iter_mut().enumerate() {
        *value = f64_as_f32(cell.interpolate(|west_east, north_south| {
            f64::from(channel_at(raster, west_east, north_south, channel))
        })) * SHADING_HEADROOM;
    }
    color[3] = MATERIAL_BLEND;
    color
}

/// Unpacks one RGB565 channel of one cell into the unit range.
fn channel_at(raster: Raster, west_east: usize, north_south: usize, channel: usize) -> f32 {
    let packed = read_u16(
        ARTIFACT,
        HEADER_BYTES + (raster.slot(west_east, north_south) * 2),
    );
    match channel {
        0 => f32::from((packed >> 11) & 0x1f) / 31.0,
        1 => f32::from((packed >> 5) & 0x3f) / 63.0,
        _ => f32::from(packed & 0x1f) / 31.0,
    }
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}
