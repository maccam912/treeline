//! Bare-earth elevation decoded from the bundle's `.tldem` layer.

use super::raster::{Cursor, HEADER_BYTES, QUANTIZATION_METERS, Raster};

const ARTIFACT: &[u8] = include_bytes!("../../assets/michigan_tile.tldem");
const MAGIC: &[u8; 8] = b"TLDEM01\0";

#[derive(Debug)]
pub struct Elevation {
    raster: Raster,
    decimeters: Box<[i16]>,
}

/// Decodes the row-delta encoded elevation stream.
///
/// Each row stores an absolute first sample followed by signed deltas: one
/// byte when it fits, otherwise an escape byte and a 16-bit delta.
///
/// # Panics
///
/// Panics when the embedded artifact does not match the bundle contract.
pub fn decode() -> Elevation {
    let raster = Raster::decode(ARTIFACT, *MAGIC);
    let mut decimeters = Vec::with_capacity(raster.cell_count());
    let mut cursor = Cursor::new(ARTIFACT, HEADER_BYTES);
    for _ in 0..raster.height {
        let mut elevation = cursor.i16();
        decimeters.push(elevation);
        for _ in 1..raster.width {
            let marker = i8::from_le_bytes([cursor.u8()]);
            let delta = if marker == i8::MIN {
                cursor.i16()
            } else {
                i16::from(marker)
            };
            elevation = elevation
                .checked_add(delta)
                .expect("surveyed elevation delta is valid");
            decimeters.push(elevation);
        }
    }
    assert!(cursor.is_at_end(), "surveyed elevation has trailing bytes");
    assert_eq!(decimeters.len(), raster.cell_count());
    Elevation {
        raster,
        decimeters: decimeters.into_boxed_slice(),
    }
}

impl Elevation {
    /// Bilinearly samples the bare-earth surface in meters above the vertical datum.
    pub fn height_at(&self, x: f64, z: f64) -> f64 {
        self.raster
            .bilinear_cell(x, z)
            .interpolate(|west_east, north_south| {
                f64::from(self.decimeters[self.raster.slot(west_east, north_south)])
            })
            * QUANTIZATION_METERS
    }

    #[cfg(test)]
    pub const fn raster(&self) -> Raster {
        self.raster
    }

    #[cfg(test)]
    pub fn decimeters(&self) -> &[i16] {
        &self.decimeters
    }
}
