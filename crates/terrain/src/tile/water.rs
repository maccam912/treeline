//! Mapped lake footprints decoded from the bundle's `.tlwater` layer.

use super::raster::{Cursor, HEADER_BYTES, QUANTIZATION_METERS, Raster};

const ARTIFACT: &[u8] = include_bytes!("../../assets/michigan_tile.tlwater");
const MAGIC: &[u8; 8] = b"TLWTR01\0";

/// Horizontal resolution of the measured footprint mask.
pub const WATER_MASK_SPACING_METERS: f64 = 4.0;

/// Versioned horizontal expansion of every mapped footprint, in meters.
///
/// Three water cells of dilation let the horizontal sheet pass beneath the
/// surrounding shore instead of ending at the polygon edge. It is a rendering
/// decision recorded in bundle metadata, not measured lake extent.
pub const FOOTPRINT_EXPANSION_METERS: f64 = 12.0;

const FOOTPRINT_EXPANSION_CELLS: isize = 3;

/// One mapped lake at a horizontal position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LakeSample {
    /// Stable per-tile feature identifier from the source hydrography.
    pub id: u8,
    /// Representative source level, not a measured lake bottom.
    pub surface_elevation_meters: f64,
}

#[derive(Debug)]
pub struct Water {
    raster: Raster,
    surfaces_decimeters: Box<[i16]>,
    lake_ids: Box<[u8]>,
}

/// Decodes per-lake surface levels and the run-length encoded footprint mask.
///
/// # Panics
///
/// Panics when the embedded artifact does not match the bundle contract.
pub fn decode() -> Water {
    let raster = Raster::decode(ARTIFACT, *MAGIC);
    assert_eq!(
        raster.spacing_meters.to_bits(),
        WATER_MASK_SPACING_METERS.to_bits(),
        "surveyed water spacing matches the sampling contract"
    );
    let mut cursor = Cursor::new(ARTIFACT, HEADER_BYTES);
    let lake_count = usize::from(cursor.u16());
    let surfaces_decimeters = (0..lake_count).map(|_| cursor.i16()).collect::<Vec<_>>();
    let mut lake_ids = Vec::with_capacity(raster.cell_count());
    for _ in 0..raster.height {
        let row_start = lake_ids.len();
        while lake_ids.len() - row_start < raster.width {
            let lake_id = cursor.u8();
            let run = usize::from(cursor.u16());
            assert!(run > 0, "surveyed water runs are non-empty");
            assert!(
                usize::from(lake_id) <= lake_count,
                "surveyed water identifier has a surface"
            );
            assert!(
                lake_ids.len() - row_start + run <= raster.width,
                "surveyed water run remains inside its row"
            );
            lake_ids.extend(std::iter::repeat_n(lake_id, run));
        }
    }
    assert_eq!(lake_ids.len(), raster.cell_count());
    assert!(cursor.is_at_end(), "surveyed water has trailing bytes");
    Water {
        raster,
        surfaces_decimeters: surfaces_decimeters.into_boxed_slice(),
        lake_ids: lake_ids.into_boxed_slice(),
    }
}

impl Water {
    /// Returns the lake covering a horizontal position, if any.
    pub fn lake_at(&self, x: f64, z: f64) -> Option<LakeSample> {
        let expansion_cells = FOOTPRINT_EXPANSION_METERS / self.raster.spacing_meters;
        debug_assert_eq!(expansion_cells.to_bits(), 3.0_f64.to_bits());
        let (cell_x, cell_z) = self.raster.containing_cell(x, z, expansion_cells)?;
        let id = match self.mapped_id(cell_x, cell_z) {
            Some(id) => id,
            None => self.dilated_id(cell_x, cell_z, FOOTPRINT_EXPANSION_CELLS)?,
        };
        Some(LakeSample {
            id,
            surface_elevation_meters: f64::from(self.surfaces_decimeters[usize::from(id - 1)])
                * QUANTIZATION_METERS,
        })
    }

    /// Reads the mapped identifier of one cell, treating 0 and off-grid as dry.
    fn mapped_id(&self, cell_x: isize, cell_z: isize) -> Option<u8> {
        let cell_x = usize::try_from(cell_x).ok()?;
        let cell_z = usize::try_from(cell_z).ok()?;
        (cell_x < self.raster.width && cell_z < self.raster.height)
            .then(|| self.lake_ids[self.raster.slot(cell_x, cell_z)])
            .filter(|id| *id != 0)
    }

    /// Extends every mapped shore by the configured cell radius.
    ///
    /// The lowest identifier wins the rare overlap, so a shore cell resolves to
    /// the same lake on every visit.
    fn dilated_id(&self, cell_x: isize, cell_z: isize, radius: isize) -> Option<u8> {
        (-radius..=radius)
            .flat_map(|offset_z| {
                (-radius..=radius).map(move |offset_x| (cell_x + offset_x, cell_z + offset_z))
            })
            .filter_map(|(x, z)| self.mapped_id(x, z))
            .min()
    }

    /// Finds a mapped cell whose eastern neighbour is dry, for dilation tests.
    #[cfg(test)]
    pub fn east_facing_shore(&self) -> Option<(f64, f64, u8)> {
        use super::raster::usize_as_f64;

        (0..self.raster.height).find_map(|cell_z| {
            (0..self.raster.width - 1).find_map(|cell_x| {
                let id = self.lake_ids[self.raster.slot(cell_x, cell_z)];
                let east = self.lake_ids[self.raster.slot(cell_x + 1, cell_z)];
                (id != 0 && east == 0).then(|| {
                    (
                        self.raster.west_pixel_center_x
                            + (usize_as_f64(cell_x + 1) * self.raster.spacing_meters),
                        (usize_as_f64(cell_z) + 0.5) * self.raster.spacing_meters,
                        id,
                    )
                })
            })
        })
    }
}
