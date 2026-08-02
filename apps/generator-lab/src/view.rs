//! What each layer looks like when you look straight down at it.
//!
//! Every mode maps one measurement to color. The point is to see a layer as it
//! actually is — its gaps, its noise, its edges — so a mode never blends layers
//! together or smooths anything.

use treeline_climate::Season;
use treeline_ecology::TreeFunctionalGroup;
use treeline_world::WorldTerrain;

/// The tile's measured elevation range, which fixes the terrain color ramp.
///
/// A fixed range rather than a per-view stretch, so panning does not silently
/// rescale what the colors mean.
const MINIMUM_ELEVATION_METERS: f64 = 406.0;
const MAXIMUM_ELEVATION_METERS: f64 = 488.0;

/// Tallest canopy the height ramp resolves, in meters.
const MAXIMUM_CANOPY_HEIGHT_METERS: f64 = 40.0;

/// One layer of the bundle, or one thing derived from it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ViewMode {
    #[default]
    Elevation,
    Imagery,
    Water,
    CanopyCover,
    CanopyHeight,
    Forest,
    Snow,
}

impl ViewMode {
    pub const ALL: [Self; 7] = [
        Self::Elevation,
        Self::Imagery,
        Self::Water,
        Self::CanopyCover,
        Self::CanopyHeight,
        Self::Forest,
        Self::Snow,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Elevation => "Elevation",
            Self::Imagery => "Imagery",
            Self::Water => "Water",
            Self::CanopyCover => "Canopy cover",
            Self::CanopyHeight => "Canopy height",
            Self::Forest => "Forest species",
            Self::Snow => "Snow cover",
        }
    }

    /// What the mode measures, shown next to the view.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Elevation => "Bare-earth elevation, 406–488 m",
            Self::Imagery => "Natural-color aerial imagery",
            Self::Water => "Mapped lake footprints and levels",
            Self::CanopyCover => "Fraction of ground under canopy",
            Self::CanopyHeight => "Tallest lidar return, 0–40 m",
            Self::Forest => "Dominant growth strategy where forest stands",
            Self::Snow => "Seasonal snow retained by slope",
        }
    }

    /// Colors one position, or `None` where the mode has nothing to show.
    pub fn color_at(
        self,
        terrain: WorldTerrain,
        x: f64,
        z: f64,
        season: Season,
    ) -> Option<[f32; 4]> {
        match self {
            Self::Elevation => Some(elevation_color(terrain.surface_height_at(x, z)?)),
            Self::Imagery => terrain.surface_color_at(x, z),
            Self::Water => Some(water_color(terrain, x, z)),
            Self::CanopyCover => Some(ramp(
                terrain
                    .stand_at(x, z)
                    .map_or(0.0, treeline_ecology::Stand::canopy_cover_fraction),
                [0.42, 0.38, 0.30],
                [0.06, 0.34, 0.10],
            )),
            Self::CanopyHeight => Some(ramp(
                terrain.stand_at(x, z).map_or(0.0, |stand| {
                    stand.top_height_meters() / MAXIMUM_CANOPY_HEIGHT_METERS
                }),
                [0.10, 0.10, 0.14],
                [0.92, 0.86, 0.44],
            )),
            Self::Forest => Some(forest_color(terrain, x, z)),
            Self::Snow => Some(ramp(
                terrain.snow_cover_at(x, z, season)?.coverage_fraction,
                [0.20, 0.24, 0.20],
                [0.96, 0.97, 1.0],
            )),
        }
    }
}

/// A dark-to-light ramp across the tile's measured relief.
fn elevation_color(elevation_meters: f64) -> [f32; 4] {
    let span = MAXIMUM_ELEVATION_METERS - MINIMUM_ELEVATION_METERS;
    ramp(
        (elevation_meters - MINIMUM_ELEVATION_METERS) / span,
        [0.10, 0.14, 0.22],
        [0.94, 0.90, 0.78],
    )
}

/// Water blue over dry grey, deepening with the water's own depth.
fn water_color(terrain: WorldTerrain, x: f64, z: f64) -> [f32; 4] {
    let Some(lake) = terrain.lake_at(x, z) else {
        return [0.24, 0.24, 0.26, 1.0];
    };
    ramp(
        (lake.water_depth_meters / 6.0).clamp(0.0, 1.0),
        [0.30, 0.62, 0.82],
        [0.02, 0.16, 0.42],
    )
}

/// The species most likely to dominate a stand, or bare ground where none does.
fn forest_color(terrain: WorldTerrain, x: f64, z: f64) -> [f32; 4] {
    let Some(stand) = terrain.stand_at(x, z) else {
        return [0.34, 0.30, 0.24, 1.0];
    };
    let base = match terrain.composition().dominant() {
        TreeFunctionalGroup::EvergreenNeedleleaf => [0.06, 0.28, 0.14],
        TreeFunctionalGroup::ColdDeciduous => [0.42, 0.56, 0.16],
        TreeFunctionalGroup::TemperateBroadleaf => [0.14, 0.44, 0.10],
    };
    // Cover modulates brightness, so a sparse stand reads as sparse.
    let cover = f64_as_f32(0.35 + (stand.canopy_cover_fraction() * 0.65));
    [base[0] * cover, base[1] * cover, base[2] * cover, 1.0]
}

/// Interpolates between two colors, clamping the amount to the unit range.
fn ramp(amount: f64, low: [f32; 3], high: [f32; 3]) -> [f32; 4] {
    let amount = f64_as_f32(amount.clamp(0.0, 1.0));
    [
        low[0] + ((high[0] - low[0]) * amount),
        low[1] + ((high[1] - low[1]) * amount),
        low[2] + ((high[2] - low[2]) * amount),
        1.0,
    ]
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_world::DEFAULT_WORLD_IDENTITY;

    const TERRAIN: WorldTerrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);
    const SPAWN: [f64; 2] = [6_737.5, 7_211.7];
    const LAKE: [f64; 2] = [7_364.0, 6_894.0];

    #[test]
    fn every_mode_colors_the_spawn_inside_the_unit_range() {
        for mode in ViewMode::ALL {
            let color = mode
                .color_at(TERRAIN, SPAWN[0], SPAWN[1], Season::Winter)
                .unwrap_or_else(|| panic!("{} has no color at spawn", mode.label()));
            assert!(
                color
                    .into_iter()
                    .all(|channel| (0.0..=1.0).contains(&channel)),
                "{} produced an out-of-range color",
                mode.label()
            );
        }
    }

    #[test]
    fn elevation_spans_the_tiles_measured_relief() {
        let low = elevation_color(MINIMUM_ELEVATION_METERS);
        let high = elevation_color(MAXIMUM_ELEVATION_METERS);
        assert!(high[0] > low[0]);
        // Values outside the range clamp rather than wrapping around.
        assert_eq!(
            elevation_color(0.0).map(f32::to_bits),
            low.map(f32::to_bits)
        );
        assert_eq!(
            elevation_color(10_000.0).map(f32::to_bits),
            high.map(f32::to_bits)
        );
    }

    #[test]
    fn water_reads_differently_from_dry_ground() {
        let wet = water_color(TERRAIN, LAKE[0], LAKE[1]);
        let dry = water_color(TERRAIN, SPAWN[0], SPAWN[1]);
        assert_ne!(wet.map(f32::to_bits), dry.map(f32::to_bits));
        assert!(wet[2] > wet[0], "water should read blue");
    }

    #[test]
    fn every_mode_has_a_distinct_label_and_a_description() {
        let mut labels = ViewMode::ALL.map(ViewMode::label).to_vec();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), ViewMode::ALL.len());
        assert!(
            ViewMode::ALL
                .into_iter()
                .all(|mode| !mode.description().is_empty())
        );
    }

    #[test]
    fn winter_shows_more_snow_than_summer() {
        let winter = ViewMode::Snow
            .color_at(TERRAIN, SPAWN[0], SPAWN[1], Season::Winter)
            .expect("spawn has climate");
        let summer = ViewMode::Snow
            .color_at(TERRAIN, SPAWN[0], SPAWN[1], Season::Summer)
            .expect("spawn has climate");
        assert!(winter[0] > summer[0]);
    }
}
