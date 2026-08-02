//! Grid addressing and byte decoding shared by every surveyed bundle layer.
//!
//! All four layers cover the same footprint at different spacings, so they
//! share one header format and one rule for turning world meters into raster
//! cells. Only that rule lives here; each layer decodes its own payload.

/// Header common to every layer of a surveyed bundle.
///
/// Positions are the centers of the north-west cell, in local world meters.
#[derive(Clone, Copy, Debug)]
pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub west_pixel_center_x: f64,
    pub north_pixel_center_z: f64,
    pub spacing_meters: f64,
}

/// Bytes preceding the payload in every layer artifact.
pub const HEADER_BYTES: usize = 40;

/// Decimeter quantization shared by the elevation and water-level encodings.
pub const QUANTIZATION_METERS: f64 = 0.1;

/// The four cells and interpolation weights surrounding a world position.
#[derive(Clone, Copy, Debug)]
pub struct BilinearCell {
    pub west: usize,
    pub east: usize,
    pub north: usize,
    pub south: usize,
    pub x_fraction: f64,
    pub z_fraction: f64,
}

impl Raster {
    /// Reads and validates a layer header, rejecting a foreign magic number.
    ///
    /// # Panics
    ///
    /// Panics when the artifact is truncated, carries the wrong magic, or
    /// declares dimensions or spacing outside the bundle contract. These are
    /// build-time asserts over `include_bytes!` data, not runtime inputs.
    pub fn decode(data: &[u8], magic: [u8; 8]) -> Self {
        assert!(data.len() >= HEADER_BYTES, "surveyed layer is truncated");
        assert_eq!(data[..8], magic, "surveyed layer magic is invalid");
        let raster = Self {
            width: usize::try_from(read_u32(data, 8)).expect("raster width is representable"),
            height: usize::try_from(read_u32(data, 12)).expect("raster height is representable"),
            west_pixel_center_x: read_f64(data, 16),
            north_pixel_center_z: read_f64(data, 24),
            spacing_meters: read_f64(data, 32),
        };
        assert!(
            raster.width > 1 && raster.height > 1,
            "surveyed layer dimensions are valid"
        );
        assert!(
            raster.spacing_meters.is_finite() && raster.spacing_meters > 0.0,
            "surveyed layer spacing is valid"
        );
        raster
    }

    /// Total cells in the layer.
    ///
    /// # Panics
    ///
    /// Panics when the declared dimensions overflow `usize`.
    pub fn cell_count(self) -> usize {
        self.width
            .checked_mul(self.height)
            .expect("surveyed layer dimensions are representable")
    }

    /// Row-major payload slot of one cell.
    pub const fn slot(self, west_east: usize, north_south: usize) -> usize {
        (north_south * self.width) + west_east
    }

    /// Continuous cell coordinates of a world position, north-west cell at 0.
    ///
    /// World Z increases south while source raster rows run north to south, so
    /// the vertical axis is mirrored through the layer's north edge.
    fn cell_coordinates(self, x: f64, z: f64) -> (f64, f64) {
        let north_edge_z = self.north_pixel_center_z + (self.spacing_meters * 0.5);
        let source_northing = north_edge_z - z;
        (
            (x - self.west_pixel_center_x) / self.spacing_meters,
            (self.north_pixel_center_z - source_northing) / self.spacing_meters,
        )
    }

    /// Locates the cell containing a world position, or `None` when outside.
    ///
    /// `margin_cells` admits positions that far beyond the mapped footprint,
    /// which layers with a dilated footprint need in order to look at their
    /// neighbours.
    pub fn containing_cell(self, x: f64, z: f64, margin_cells: f64) -> Option<(isize, isize)> {
        let (grid_x, grid_z) = self.cell_coordinates(x, z);
        let (grid_x, grid_z) = (grid_x + 0.5, grid_z + 0.5);
        (grid_x >= -margin_cells
            && grid_z >= -margin_cells
            && grid_x < usize_as_f64(self.width) + margin_cells
            && grid_z < usize_as_f64(self.height) + margin_cells)
            .then(|| (floor_as_isize(grid_x), floor_as_isize(grid_z)))
    }

    /// Selects the four cells around a world position, clamped to the edge.
    ///
    /// The bundle has no neighbouring tiles yet. Clamping lets mesh residency
    /// finish at the finite border instead of inventing a second source; it is
    /// not a gap-filling policy.
    pub fn bilinear_cell(self, x: f64, z: f64) -> BilinearCell {
        let (grid_x, grid_z) = self.cell_coordinates(x, z);
        let clamped_x = grid_x.clamp(0.0, usize_as_f64(self.width - 1));
        let clamped_z = grid_z.clamp(0.0, usize_as_f64(self.height - 1));
        let west = floor_as_usize(clamped_x);
        let north = floor_as_usize(clamped_z);
        BilinearCell {
            west,
            east: (west + 1).min(self.width - 1),
            north,
            south: (north + 1).min(self.height - 1),
            x_fraction: clamped_x - usize_as_f64(west),
            z_fraction: clamped_z - usize_as_f64(north),
        }
    }
}

impl BilinearCell {
    /// Interpolates four corner values sampled through `corner(west_east, north_south)`.
    pub fn interpolate(self, mut corner: impl FnMut(usize, usize) -> f64) -> f64 {
        let north = lerp(
            corner(self.west, self.north),
            corner(self.east, self.north),
            self.x_fraction,
        );
        let south = lerp(
            corner(self.west, self.south),
            corner(self.east, self.south),
            self.x_fraction,
        );
        lerp(north, south, self.z_fraction)
    }
}

/// Sequential reader over an artifact payload.
pub struct Cursor<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    pub const fn new(data: &'a [u8], offset: usize) -> Self {
        Self { data, offset }
    }

    /// # Panics
    ///
    /// Panics when the artifact ends before the requested byte.
    pub fn u8(&mut self) -> u8 {
        let value = *self
            .data
            .get(self.offset)
            .expect("surveyed payload byte is present");
        self.offset += 1;
        value
    }

    /// # Panics
    ///
    /// Panics when the artifact ends before the requested value.
    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }

    /// # Panics
    ///
    /// Panics when the artifact ends before the requested value.
    pub fn i16(&mut self) -> i16 {
        i16::from_le_bytes(self.take())
    }

    pub const fn is_at_end(&self) -> bool {
        self.offset == self.data.len()
    }

    fn take<const BYTES: usize>(&mut self) -> [u8; BYTES] {
        let end = self
            .offset
            .checked_add(BYTES)
            .expect("surveyed payload cursor is representable");
        let value = self.data[self.offset..end]
            .try_into()
            .expect("surveyed payload value is present");
        self.offset = end;
        value
    }
}

/// # Panics
///
/// Panics when the artifact ends before the requested byte.
pub fn read_u8(data: &[u8], offset: usize) -> u8 {
    *data.get(offset).expect("surveyed payload byte is present")
}

/// # Panics
///
/// Panics when the artifact ends before the requested value.
pub fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        data[offset..offset + 2]
            .try_into()
            .expect("surveyed payload u16 is present"),
    )
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        data[offset..offset + 4]
            .try_into()
            .expect("surveyed header u32 is present"),
    )
}

fn read_f64(data: &[u8], offset: usize) -> f64 {
    f64::from_le_bytes(
        data[offset..offset + 8]
            .try_into()
            .expect("surveyed header f64 is present"),
    )
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

#[allow(clippy::cast_precision_loss)]
pub fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn floor_as_usize(value: f64) -> usize {
    libm::floor(value) as usize
}

#[allow(clippy::cast_possible_truncation)]
fn floor_as_isize(value: f64) -> isize {
    libm::floor(value) as isize
}
