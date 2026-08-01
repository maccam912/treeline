//! Fixed surveyed terrain used by the default Michigan player world.

use std::sync::OnceLock;

const ARTIFACT: &[u8] = include_bytes!("../assets/michigan_tile.tldem");
const COLOR_ARTIFACT: &[u8] = include_bytes!("../assets/michigan_tile.tlrgb");
const WATER_ARTIFACT: &[u8] = include_bytes!("../assets/michigan_tile.tlwater");
const CANOPY_ARTIFACT: &[u8] = include_bytes!("../assets/michigan_tile.tlcanopy");
const MAGIC: &[u8; 8] = b"TLDEM01\0";
const COLOR_MAGIC: &[u8; 8] = b"TLRGB01\0";
const WATER_MAGIC: &[u8; 8] = b"TLWTR01\0";
const CANOPY_MAGIC: &[u8; 8] = b"TLCAN01\0";
const HEADER_BYTES: usize = 40;
const WATER_HEADER_BYTES: usize = 42;
const QUANTIZATION_METERS: f64 = 0.1;
/// Extends mapped lake footprints into the surrounding shore by one water cell.
const SURVEYED_WATER_FOOTPRINT_EXPANSION_METERS: f64 = 4.0;

/// Versioned settings identity selecting the default surveyed-world bundle.
///
/// Any incompatible change to the embedded DEM, water, color, or canopy
/// contract must receive a new value so saved worlds cannot silently change.
pub const DEFAULT_SURVEYED_SETTINGS_HASH: u64 = 0x5355_5256_4559_0003;
/// Edge length of the default surveyed tile in local world meters.
pub const DEFAULT_SURVEYED_TILE_EDGE_METERS: f64 = 10_000.0;
/// Requested WGS84 position expressed in local world meters east of the tile edge.
pub const DEFAULT_SURVEYED_START_X: f64 = 6_737.563_408_352;
/// Requested WGS84 position expressed in local world meters south of the tile edge.
pub const DEFAULT_SURVEYED_START_Z: f64 = 7_211.701_769_280;

#[derive(Debug)]
struct SurveyedTileData {
    width: usize,
    height: usize,
    west_pixel_center_x: f64,
    north_pixel_center_z: f64,
    spacing_meters: f64,
    elevations_decimeters: Box<[i16]>,
}

static TILE: OnceLock<SurveyedTileData> = OnceLock::new();
static COLOR: OnceLock<SurveyedRasterHeader> = OnceLock::new();
static WATER: OnceLock<SurveyedWaterData> = OnceLock::new();
static CANOPY: OnceLock<SurveyedRasterHeader> = OnceLock::new();

/// Measured forest structure derived from terrain-normalized lidar returns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurveyedCanopySample {
    pub cover_fraction: f64,
    pub top_height_meters: f64,
}

#[derive(Clone, Copy, Debug)]
struct SurveyedRasterHeader {
    width: usize,
    height: usize,
    west_pixel_center_x: f64,
    north_pixel_center_z: f64,
    spacing_meters: f64,
}

#[derive(Debug)]
struct SurveyedWaterData {
    header: SurveyedRasterHeader,
    surfaces_decimeters: Box<[i16]>,
    lake_ids: Box<[u8]>,
}

/// Samples the lidar-derived bare-earth surface in local world meters.
///
/// The fixed bundle has no neighboring tiles yet. Samples beyond its 10 km
/// footprint clamp to the closest border so existing render residency can
/// finish without inventing a second terrain source. World X increases east
/// and world Z increases south, matching the renderer's right-handed ground
/// plane; source rasters instead store north at their first row.
pub fn michigan_surveyed_height_at(x: f64, z: f64) -> Option<f64> {
    if !x.is_finite() || !z.is_finite() {
        return None;
    }
    let tile = TILE.get_or_init(decode_embedded_tile);
    let grid_x = (x - tile.west_pixel_center_x) / tile.spacing_meters;
    let source_z = source_northing_from_world_z(tile.header(), z);
    let grid_z = (tile.north_pixel_center_z - source_z) / tile.spacing_meters;
    let maximum_x = usize_as_f64(tile.width - 1);
    let maximum_z = usize_as_f64(tile.height - 1);
    let clamped_x = grid_x.clamp(0.0, maximum_x);
    let clamped_z = grid_z.clamp(0.0, maximum_z);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let x0 = libm::floor(clamped_x) as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let z0 = libm::floor(clamped_z) as usize;
    let x1 = (x0 + 1).min(tile.width - 1);
    let z1 = (z0 + 1).min(tile.height - 1);
    let x_fraction = clamped_x - usize_as_f64(x0);
    let z_fraction = clamped_z - usize_as_f64(z0);
    let north_west = f64::from(tile.elevations_decimeters[z0 * tile.width + x0]);
    let north_east = f64::from(tile.elevations_decimeters[z0 * tile.width + x1]);
    let south_west = f64::from(tile.elevations_decimeters[z1 * tile.width + x0]);
    let south_east = f64::from(tile.elevations_decimeters[z1 * tile.width + x1]);
    let north = north_west + ((north_east - north_west) * x_fraction);
    let south = south_west + ((south_east - south_west) * x_fraction);
    Some((north + ((south - north) * z_fraction)) * QUANTIZATION_METERS)
}

/// Samples the fixed NAIP natural-color layer, graded for the terrain shader.
pub fn michigan_surveyed_color_at(x: f64, z: f64) -> Option<[f32; 4]> {
    if !x.is_finite() || !z.is_finite() {
        return None;
    }
    let header = COLOR.get_or_init(decode_color_header);
    let (x0, x1, z0, z1, x_fraction, z_fraction) = bilinear_sample(*header, x, z);
    let color_at = |sample_x: usize, sample_z: usize| {
        let offset = HEADER_BYTES + ((sample_z * header.width + sample_x) * 2);
        let packed = read_u16_from(COLOR_ARTIFACT, offset);
        let red = f32::from((packed >> 11) & 0x1f) / 31.0;
        let green = f32::from((packed >> 5) & 0x3f) / 63.0;
        let blue = f32::from(packed & 0x1f) / 31.0;
        [red, green, blue]
    };
    let north_west = color_at(x0, z0);
    let north_east = color_at(x1, z0);
    let south_west = color_at(x0, z1);
    let south_east = color_at(x1, z1);
    let x_fraction = f64_as_f32(x_fraction);
    let z_fraction = f64_as_f32(z_fraction);
    let mut color = [0.0; 4];
    for channel in 0..3 {
        let north =
            north_west[channel] + ((north_east[channel] - north_west[channel]) * x_fraction);
        let south =
            south_west[channel] + ((south_east[channel] - south_west[channel]) * x_fraction);
        color[channel] = (north + ((south - north) * z_fraction)) * 0.82;
    }
    color[3] = 0.90;
    Some(color)
}

/// Returns the mapped lake identifier and representative surface elevation.
pub fn michigan_surveyed_lake_at(x: f64, z: f64) -> Option<(u8, f64)> {
    if !x.is_finite() || !z.is_finite() {
        return None;
    }
    let water = WATER.get_or_init(decode_water);
    let grid_x = ((x - water.header.west_pixel_center_x) / water.header.spacing_meters) + 0.5;
    let source_z = source_northing_from_world_z(water.header, z);
    let grid_z =
        ((water.header.north_pixel_center_z - source_z) / water.header.spacing_meters) + 0.5;
    let expansion_cells = SURVEYED_WATER_FOOTPRINT_EXPANSION_METERS / water.header.spacing_meters;
    debug_assert_eq!(expansion_cells.to_bits(), 1.0_f64.to_bits());
    if grid_x < -expansion_cells
        || grid_z < -expansion_cells
        || grid_x >= usize_as_f64(water.header.width) + expansion_cells
        || grid_z >= usize_as_f64(water.header.height) + expansion_cells
    {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let sample_x = libm::floor(grid_x) as isize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let sample_z = libm::floor(grid_z) as isize;
    let lake_id_at = |sample_x: isize, sample_z: isize| {
        let sample_x = usize::try_from(sample_x).ok()?;
        let sample_z = usize::try_from(sample_z).ok()?;
        (sample_x < water.header.width && sample_z < water.header.height)
            .then(|| water.lake_ids[sample_z * water.header.width + sample_x])
    };
    let direct_lake_id = lake_id_at(sample_x, sample_z).unwrap_or(0);
    let lake_id = if direct_lake_id != 0 {
        direct_lake_id
    } else {
        // A one-cell square dilation extends every mapped shore by four meters.
        // Lowest ID wins the rare overlap so the result is stable across visits.
        (-1..=1)
            .flat_map(|offset_z| {
                (-1..=1).filter_map(move |offset_x| {
                    lake_id_at(sample_x + offset_x, sample_z + offset_z)
                })
            })
            .filter(|lake_id| *lake_id != 0)
            .min()
            .unwrap_or(0)
    };
    if lake_id == 0 {
        return None;
    }
    let surface = water.surfaces_decimeters[usize::from(lake_id - 1)];
    Some((lake_id, f64::from(surface) * QUANTIZATION_METERS))
}

/// Samples local lidar-derived canopy cover and height in world meters.
pub fn michigan_surveyed_canopy_at(x: f64, z: f64) -> Option<SurveyedCanopySample> {
    if !x.is_finite() || !z.is_finite() {
        return None;
    }
    let header = CANOPY.get_or_init(decode_canopy_header);
    let grid_x = ((x - header.west_pixel_center_x) / header.spacing_meters) + 0.5;
    let source_z = source_northing_from_world_z(*header, z);
    let grid_z = ((header.north_pixel_center_z - source_z) / header.spacing_meters) + 0.5;
    if grid_x < 0.0
        || grid_z < 0.0
        || grid_x >= usize_as_f64(header.width)
        || grid_z >= usize_as_f64(header.height)
    {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let sample_x = libm::floor(grid_x) as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let sample_z = libm::floor(grid_z) as usize;
    let offset = HEADER_BYTES + ((sample_z * header.width + sample_x) * 2);
    Some(SurveyedCanopySample {
        cover_fraction: f64::from(read_byte_from(CANOPY_ARTIFACT, offset)) / 255.0,
        top_height_meters: f64::from(read_byte_from(CANOPY_ARTIFACT, offset + 1)) * 0.5,
    })
}

fn bilinear_sample(
    header: SurveyedRasterHeader,
    x: f64,
    z: f64,
) -> (usize, usize, usize, usize, f64, f64) {
    let grid_x = (x - header.west_pixel_center_x) / header.spacing_meters;
    let source_z = source_northing_from_world_z(header, z);
    let grid_z = (header.north_pixel_center_z - source_z) / header.spacing_meters;
    let maximum_x = usize_as_f64(header.width - 1);
    let maximum_z = usize_as_f64(header.height - 1);
    let clamped_x = grid_x.clamp(0.0, maximum_x);
    let clamped_z = grid_z.clamp(0.0, maximum_z);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let x0 = libm::floor(clamped_x) as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let z0 = libm::floor(clamped_z) as usize;
    let x1 = (x0 + 1).min(header.width - 1);
    let z1 = (z0 + 1).min(header.height - 1);
    (
        x0,
        x1,
        z0,
        z1,
        clamped_x - usize_as_f64(x0),
        clamped_z - usize_as_f64(z0),
    )
}

impl SurveyedTileData {
    const fn header(&self) -> SurveyedRasterHeader {
        SurveyedRasterHeader {
            width: self.width,
            height: self.height,
            west_pixel_center_x: self.west_pixel_center_x,
            north_pixel_center_z: self.north_pixel_center_z,
            spacing_meters: self.spacing_meters,
        }
    }
}

fn source_northing_from_world_z(header: SurveyedRasterHeader, world_z: f64) -> f64 {
    let north_edge_z = header.north_pixel_center_z + (header.spacing_meters * 0.5);
    north_edge_z - world_z
}

fn decode_embedded_tile() -> SurveyedTileData {
    assert!(
        ARTIFACT.len() >= HEADER_BYTES,
        "surveyed tile header is truncated"
    );
    assert_eq!(&ARTIFACT[..8], MAGIC, "surveyed tile magic is invalid");
    let width = usize::try_from(read_u32(8)).expect("surveyed tile width is representable");
    let height = usize::try_from(read_u32(12)).expect("surveyed tile height is representable");
    let west_pixel_center_x = read_f64(16);
    let north_pixel_center_z = read_f64(24);
    let spacing_meters = read_f64(32);
    assert!(
        width > 1 && height > 1,
        "surveyed tile dimensions are valid"
    );
    assert!(
        spacing_meters.is_finite() && spacing_meters > 0.0,
        "surveyed tile spacing is valid"
    );

    let sample_count = width
        .checked_mul(height)
        .expect("surveyed tile sample count is representable");
    let mut elevations = Vec::with_capacity(sample_count);
    let mut cursor = HEADER_BYTES;
    for _ in 0..height {
        let mut elevation = read_i16_at(&mut cursor);
        elevations.push(elevation);
        for _ in 1..width {
            let marker = i8::from_le_bytes([read_byte_at(&mut cursor)]);
            let delta = if marker == i8::MIN {
                read_i16_at(&mut cursor)
            } else {
                i16::from(marker)
            };
            elevation = elevation
                .checked_add(delta)
                .expect("surveyed tile elevation delta is valid");
            elevations.push(elevation);
        }
    }
    assert_eq!(cursor, ARTIFACT.len(), "surveyed tile has trailing bytes");
    assert_eq!(elevations.len(), sample_count);
    SurveyedTileData {
        width,
        height,
        west_pixel_center_x,
        north_pixel_center_z,
        spacing_meters,
        elevations_decimeters: elevations.into_boxed_slice(),
    }
}

fn decode_color_header() -> SurveyedRasterHeader {
    assert!(
        COLOR_ARTIFACT.len() >= HEADER_BYTES,
        "surveyed color header is truncated"
    );
    assert_eq!(&COLOR_ARTIFACT[..8], COLOR_MAGIC);
    let header = read_raster_header(COLOR_ARTIFACT);
    let expected_bytes = HEADER_BYTES
        + header
            .width
            .checked_mul(header.height)
            .and_then(|samples| samples.checked_mul(2))
            .expect("surveyed color dimensions are representable");
    assert_eq!(COLOR_ARTIFACT.len(), expected_bytes);
    header
}

fn decode_canopy_header() -> SurveyedRasterHeader {
    assert!(
        CANOPY_ARTIFACT.len() >= HEADER_BYTES,
        "surveyed canopy header is truncated"
    );
    assert_eq!(&CANOPY_ARTIFACT[..8], CANOPY_MAGIC);
    let header = read_raster_header(CANOPY_ARTIFACT);
    let expected_bytes = HEADER_BYTES
        + header
            .width
            .checked_mul(header.height)
            .and_then(|samples| samples.checked_mul(2))
            .expect("surveyed canopy dimensions are representable");
    assert_eq!(CANOPY_ARTIFACT.len(), expected_bytes);
    header
}

fn decode_water() -> SurveyedWaterData {
    assert!(
        WATER_ARTIFACT.len() >= WATER_HEADER_BYTES,
        "surveyed water header is truncated"
    );
    assert_eq!(&WATER_ARTIFACT[..8], WATER_MAGIC);
    let header = read_raster_header(WATER_ARTIFACT);
    let lake_count = usize::from(read_u16_from(WATER_ARTIFACT, HEADER_BYTES));
    let mut cursor = WATER_HEADER_BYTES;
    let mut surfaces = Vec::with_capacity(lake_count);
    for _ in 0..lake_count {
        surfaces.push(read_i16_from_at(WATER_ARTIFACT, &mut cursor));
    }
    let sample_count = header
        .width
        .checked_mul(header.height)
        .expect("surveyed water dimensions are representable");
    let mut lake_ids = Vec::with_capacity(sample_count);
    for _ in 0..header.height {
        let row_start = lake_ids.len();
        while lake_ids.len() - row_start < header.width {
            let lake_id = read_byte_from_at(WATER_ARTIFACT, &mut cursor);
            let run = usize::from(read_u16_from_at(WATER_ARTIFACT, &mut cursor));
            assert!(run > 0, "surveyed water runs are non-empty");
            assert!(
                usize::from(lake_id) <= lake_count,
                "surveyed water identifier has a surface"
            );
            assert!(
                lake_ids.len() - row_start + run <= header.width,
                "surveyed water run remains inside its row"
            );
            lake_ids.extend(std::iter::repeat_n(lake_id, run));
        }
    }
    assert_eq!(lake_ids.len(), sample_count);
    assert_eq!(cursor, WATER_ARTIFACT.len());
    SurveyedWaterData {
        header,
        surfaces_decimeters: surfaces.into_boxed_slice(),
        lake_ids: lake_ids.into_boxed_slice(),
    }
}

fn read_raster_header(data: &[u8]) -> SurveyedRasterHeader {
    let width = usize::try_from(read_u32_from(data, 8)).expect("raster width is representable");
    let height = usize::try_from(read_u32_from(data, 12)).expect("raster height is representable");
    let header = SurveyedRasterHeader {
        width,
        height,
        west_pixel_center_x: read_f64_from(data, 16),
        north_pixel_center_z: read_f64_from(data, 24),
        spacing_meters: read_f64_from(data, 32),
    };
    assert!(header.width > 1 && header.height > 1);
    assert!(header.spacing_meters.is_finite() && header.spacing_meters > 0.0);
    header
}

fn read_u32(offset: usize) -> u32 {
    read_u32_from(ARTIFACT, offset)
}

fn read_u32_from(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        data[offset..offset + 4]
            .try_into()
            .expect("surveyed artifact u32 is present"),
    )
}

fn read_f64(offset: usize) -> f64 {
    read_f64_from(ARTIFACT, offset)
}

fn read_f64_from(data: &[u8], offset: usize) -> f64 {
    f64::from_le_bytes(
        data[offset..offset + 8]
            .try_into()
            .expect("surveyed artifact f64 is present"),
    )
}

fn read_u16_from(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        data[offset..offset + 2]
            .try_into()
            .expect("surveyed artifact u16 is present"),
    )
}

fn read_u16_from_at(data: &[u8], cursor: &mut usize) -> u16 {
    let value = read_u16_from(data, *cursor);
    *cursor += 2;
    value
}

fn read_byte_from_at(data: &[u8], cursor: &mut usize) -> u8 {
    let value = *data
        .get(*cursor)
        .expect("surveyed artifact byte is present");
    *cursor += 1;
    value
}

fn read_byte_from(data: &[u8], offset: usize) -> u8 {
    *data.get(offset).expect("surveyed artifact byte is present")
}

fn read_i16_from_at(data: &[u8], cursor: &mut usize) -> i16 {
    let end = cursor
        .checked_add(2)
        .expect("surveyed artifact cursor is representable");
    let value = i16::from_le_bytes(
        data[*cursor..end]
            .try_into()
            .expect("surveyed artifact i16 is present"),
    );
    *cursor = end;
    value
}

fn read_byte_at(cursor: &mut usize) -> u8 {
    let value = *ARTIFACT
        .get(*cursor)
        .expect("surveyed tile elevation stream is complete");
    *cursor += 1;
    value
}

fn read_i16_at(cursor: &mut usize) -> i16 {
    let end = cursor
        .checked_add(2)
        .expect("surveyed tile cursor is representable");
    let value = i16::from_le_bytes(
        ARTIFACT[*cursor..end]
            .try_into()
            .expect("surveyed tile i16 is present"),
    );
    *cursor = end;
    value
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tile_decodes_with_expected_contract() {
        assert!(
            include_str!("../assets/michigan_tile.json")
                .contains("\"settings_identity\": \"0x5355525645590003\"")
        );
        assert_eq!(DEFAULT_SURVEYED_SETTINGS_HASH, 0x5355_5256_4559_0003);
        let tile = decode_embedded_tile();
        assert_eq!((tile.width, tile.height), (5_000, 5_000));
        assert_eq!(tile.spacing_meters.to_bits(), 2.0_f64.to_bits());
        assert_eq!(tile.west_pixel_center_x.to_bits(), 1.0_f64.to_bits());
        assert_eq!(tile.north_pixel_center_z.to_bits(), 9_999.0_f64.to_bits());
        assert_eq!(
            tile.elevations_decimeters.iter().copied().min(),
            Some(4_061)
        );
        assert_eq!(
            tile.elevations_decimeters.iter().copied().max(),
            Some(4_874)
        );
    }

    #[test]
    fn requested_location_is_inside_tile_and_has_realistic_elevation() {
        let height =
            michigan_surveyed_height_at(DEFAULT_SURVEYED_START_X, DEFAULT_SURVEYED_START_Z)
                .unwrap();
        assert!((406.0..=488.0).contains(&height));
    }

    #[test]
    fn world_z_increases_south_through_source_raster_rows() {
        let tile = decode_embedded_tile();
        let north_west = f64::from(tile.elevations_decimeters[0]) * QUANTIZATION_METERS;
        let south_west = f64::from(tile.elevations_decimeters[(tile.height - 1) * tile.width])
            * QUANTIZATION_METERS;

        assert_eq!(michigan_surveyed_height_at(1.0, 1.0), Some(north_west));
        assert_eq!(michigan_surveyed_height_at(1.0, 9_999.0), Some(south_west));
    }

    #[test]
    fn color_and_water_artifacts_match_the_fixed_footprint() {
        let color =
            michigan_surveyed_color_at(DEFAULT_SURVEYED_START_X, DEFAULT_SURVEYED_START_Z).unwrap();
        assert!(
            color
                .into_iter()
                .all(|channel| (0.0..=1.0).contains(&channel))
        );

        let upper_holmes_lake = michigan_surveyed_lake_at(7_364.0, 6_894.0).unwrap();
        assert_eq!(upper_holmes_lake.0, 19);
        assert!((upper_holmes_lake.1 - 415.5).abs() < f64::EPSILON);
        assert!(
            michigan_surveyed_lake_at(DEFAULT_SURVEYED_START_X, DEFAULT_SURVEYED_START_Z).is_none()
        );
    }

    #[test]
    fn surveyed_water_footprint_expands_one_cell_beyond_the_mapped_shore() {
        let water = decode_water();
        let (mapped_x, mapped_z, lake_id) = (0..water.header.height)
            .find_map(|sample_z| {
                (0..water.header.width.saturating_sub(1)).find_map(|sample_x| {
                    let lake_id = water.lake_ids[sample_z * water.header.width + sample_x];
                    let east_id = water.lake_ids[sample_z * water.header.width + sample_x + 1];
                    (lake_id != 0 && east_id == 0).then_some((sample_x, sample_z, lake_id))
                })
            })
            .expect("surveyed water contains an east-facing shore");
        let expanded_x = water.header.west_pixel_center_x
            + (usize_as_f64(mapped_x + 1) * water.header.spacing_meters);
        let expanded_z = (usize_as_f64(mapped_z) + 0.5) * water.header.spacing_meters;

        assert_eq!(
            michigan_surveyed_lake_at(expanded_x, expanded_z).map(|sample| sample.0),
            Some(lake_id)
        );
    }

    #[test]
    fn canopy_artifact_varies_cover_and_matches_spawn_height() {
        let header = decode_canopy_header();
        assert_eq!((header.width, header.height), (1_667, 1_667));
        assert_eq!(header.spacing_meters.to_bits(), 6.0_f64.to_bits());
        let samples = CANOPY_ARTIFACT[HEADER_BYTES..]
            .chunks_exact(2)
            .map(|sample| (sample[0], sample[1]))
            .collect::<Vec<_>>();
        assert!(samples.iter().any(|&(cover, _)| cover == 0));
        assert!(samples.iter().any(|&(cover, _)| cover == u8::MAX));
        assert!(samples.iter().any(|&(_, height)| height >= 50));

        let spawn = michigan_surveyed_canopy_at(DEFAULT_SURVEYED_START_X, DEFAULT_SURVEYED_START_Z)
            .unwrap();
        assert_eq!(spawn.cover_fraction.to_bits(), 1.0_f64.to_bits());
        assert_eq!(spawn.top_height_meters.to_bits(), 5.5_f64.to_bits());
    }

    #[test]
    fn non_finite_samples_are_rejected() {
        assert_eq!(michigan_surveyed_height_at(f64::NAN, 0.0), None);
        assert_eq!(michigan_surveyed_height_at(0.0, f64::INFINITY), None);
        assert_eq!(michigan_surveyed_canopy_at(f64::NAN, 0.0), None);
    }
}
