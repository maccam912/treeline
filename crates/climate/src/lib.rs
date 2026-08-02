//! Seasonal climate for the surveyed site.
//!
//! The world is one measured 10 km square, so climate is a property of that
//! site rather than a synthesized global field. [`SiteClimate::SURVEYED_TILE`]
//! records the normals for the bundle's location in Michigan's Upper Peninsula;
//! [`SiteClimate::season`] turns them into the state of one season.
//!
//! Elevation is the only thing that varies within the tile, and it varies by
//! about eighty meters, so temperature is the only field that responds to
//! position at all. Nothing here is randomized: the same season and elevation
//! always produce the same result.

mod season;

pub use season::Season;

/// Climate normals for one location.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SiteClimate {
    pub mean_annual_temperature_celsius: f64,
    /// Half the difference between the warmest and coldest seasonal means.
    pub seasonal_temperature_amplitude_celsius: f64,
    pub annual_precipitation_millimeters: f64,
    /// Unit vector the wind blows toward, in world X and Z.
    pub prevailing_wind: [f64; 2],
    /// Elevation the temperature normals are stated at, in meters.
    pub reference_elevation_meters: f64,
}

/// Climate state during one season.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeasonalClimate {
    pub season: Season,
    pub mean_temperature_celsius: f64,
    pub precipitation_millimeters: f64,
    /// Precipitation arriving as snow rather than rain.
    pub snowfall_water_equivalent_millimeters: f64,
    /// Snow standing on the ground at the end of the season.
    pub snowpack_water_equivalent_millimeters: f64,
}

/// Temperature lost per meter of elevation gain, in degrees Celsius.
const LAPSE_RATE_CELSIUS_PER_METER: f64 = 0.0065;
/// Snow melted per season per degree Celsius above freezing, in millimeters.
const SEASONAL_MELT_PER_DEGREE_MILLIMETERS: f64 = 62.0;

impl SiteClimate {
    /// Normals for the bundle's location: 46.16 degrees north, humid continental.
    ///
    /// Long winters with lake-effect snow, warm humid summers, and precipitation
    /// spread through the year. The prevailing wind is the regional westerly,
    /// blowing east and slightly south in world axes.
    pub const SURVEYED_TILE: Self = Self {
        mean_annual_temperature_celsius: 4.8,
        seasonal_temperature_amplitude_celsius: 14.5,
        annual_precipitation_millimeters: 810.0,
        prevailing_wind: [0.958, 0.287],
        reference_elevation_meters: 440.0,
    };

    /// Resolves this season's temperature, precipitation, and standing snow.
    ///
    /// Returns `None` for a non-finite elevation.
    pub fn season(self, season: Season, elevation_meters: f64) -> Option<SeasonalClimate> {
        if !elevation_meters.is_finite() {
            return None;
        }
        let year = self.year(elevation_meters);
        Some(year[season.index()])
    }

    /// Resolves all four seasons at once, in [`Season::ALL`] order.
    ///
    /// Standing snow depends on what the previous seasons left behind, so the
    /// whole cycle is solved together rather than one season at a time.
    fn year(self, elevation_meters: f64) -> [SeasonalClimate; 4] {
        let cooling =
            (elevation_meters - self.reference_elevation_meters) * LAPSE_RATE_CELSIUS_PER_METER;
        let mut year = Season::ALL.map(|season| {
            let mean_temperature_celsius = self.mean_annual_temperature_celsius
                + (self.seasonal_temperature_amplitude_celsius
                    * season.temperature_offset_fraction())
                - cooling;
            let precipitation_millimeters =
                self.annual_precipitation_millimeters * season.precipitation_share();
            SeasonalClimate {
                season,
                mean_temperature_celsius,
                precipitation_millimeters,
                snowfall_water_equivalent_millimeters: precipitation_millimeters
                    * snowfall_fraction(mean_temperature_celsius),
                snowpack_water_equivalent_millimeters: 0.0,
            }
        });
        accumulate_snowpack(&mut year);
        year
    }
}

/// Share of precipitation falling as snow at a given mean temperature.
///
/// Snow and rain overlap across a few degrees around freezing rather than
/// switching at exactly zero.
fn snowfall_fraction(mean_temperature_celsius: f64) -> f64 {
    ((2.5 - mean_temperature_celsius) / 5.0).clamp(0.0, 1.0)
}

/// Carries snow between seasons until the annual cycle repeats.
///
/// Each season adds its snowfall and melts in proportion to how far it sits
/// above freezing. Two passes are enough for the carry-over to settle, because
/// the site melts out completely every summer.
fn accumulate_snowpack(year: &mut [SeasonalClimate; 4]) {
    let mut snowpack = 0.0;
    for _ in 0..2 {
        for season in year.iter_mut() {
            let melt_potential =
                (season.mean_temperature_celsius.max(0.0)) * SEASONAL_MELT_PER_DEGREE_MILLIMETERS;
            snowpack =
                (snowpack + season.snowfall_water_equivalent_millimeters - melt_potential).max(0.0);
            season.snowpack_water_equivalent_millimeters = snowpack;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: SiteClimate = SiteClimate::SURVEYED_TILE;
    const TILE_ELEVATION_METERS: f64 = 440.0;

    fn season(season: Season) -> SeasonalClimate {
        SITE.season(season, TILE_ELEVATION_METERS)
            .expect("finite elevation")
    }

    #[test]
    fn winter_is_below_freezing_and_summer_is_warm() {
        assert!(season(Season::Winter).mean_temperature_celsius < -5.0);
        assert!(season(Season::Summer).mean_temperature_celsius > 15.0);
    }

    #[test]
    fn snow_stands_in_winter_and_is_gone_by_summer() {
        assert!(season(Season::Winter).snowpack_water_equivalent_millimeters > 50.0);
        assert_eq!(
            season(Season::Summer)
                .snowpack_water_equivalent_millimeters
                .to_bits(),
            0.0_f64.to_bits()
        );
    }

    #[test]
    fn summer_precipitation_falls_as_rain() {
        assert_eq!(
            season(Season::Summer)
                .snowfall_water_equivalent_millimeters
                .to_bits(),
            0.0_f64.to_bits()
        );
        assert!(season(Season::Winter).snowfall_water_equivalent_millimeters > 100.0);
    }

    #[test]
    fn seasonal_precipitation_sums_to_the_annual_normal() {
        let total: f64 = Season::ALL
            .into_iter()
            .map(|value| season(value).precipitation_millimeters)
            .sum();
        assert!((total - SITE.annual_precipitation_millimeters).abs() < 1.0e-9);
    }

    #[test]
    fn higher_ground_is_colder_by_the_lapse_rate() {
        let low = SITE
            .season(Season::Summer, 406.0)
            .expect("finite elevation");
        let high = SITE
            .season(Season::Summer, 487.0)
            .expect("finite elevation");
        let expected = (487.0 - 406.0) * LAPSE_RATE_CELSIUS_PER_METER;

        assert!(
            (low.mean_temperature_celsius - high.mean_temperature_celsius - expected).abs()
                < 1.0e-9
        );
    }

    #[test]
    fn sampling_is_repeatable_and_order_independent() {
        let forward = Season::ALL.map(season);
        let mut reversed = Season::ALL;
        reversed.reverse();
        let mut backward = reversed.map(season);
        backward.reverse();

        assert_eq!(forward, backward);
    }

    #[test]
    fn non_finite_elevation_is_rejected() {
        assert_eq!(SITE.season(Season::Winter, f64::NAN), None);
    }
}
