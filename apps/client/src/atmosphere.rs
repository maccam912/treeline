//! Turning the site's climate into fog and haze.
//!
//! The tile is one climate, so this is computed once at startup rather than
//! resampled as the player moves.

use treeline_climate::{Season, SiteClimate};
use treeline_renderer::AtmosphereSettings;

/// Season the haze is tuned for, matching the terrain's snow treatment.
const SEASON: Season = Season::Winter;

/// Elevation the atmosphere is evaluated at: the tile's mid-range.
const REFERENCE_ELEVATION_METERS: f64 = 440.0;

/// Derives fog color and density from climate normals.
///
/// Cold air reads bluer and holds less haze; damp air reads greyer and thicker.
/// Both are presentation, not simulation.
pub fn settings_for(climate: SiteClimate) -> Option<AtmosphereSettings> {
    let season = climate.season(SEASON, REFERENCE_ELEVATION_METERS)?;
    let warmth = ((season.mean_temperature_celsius + 20.0) / 55.0).clamp(0.0, 1.0);
    let moisture = (season.precipitation_millimeters / 320.0).clamp(0.0, 1.0);

    Some(AtmosphereSettings {
        fog_color: [
            f64_as_f32(0.36 + (warmth * 0.07) + ((1.0 - moisture) * 0.03)),
            f64_as_f32(0.52 + (warmth * 0.04) + (moisture * 0.05)),
            f64_as_f32(0.66 + ((1.0 - warmth) * 0.07) + (moisture * 0.03)),
        ],
        fog_density: f64_as_f32(0.58 + (moisture * 0.92)),
    })
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_surveyed_site_produces_usable_atmosphere() {
        let settings = settings_for(SiteClimate::SURVEYED_TILE).expect("the site has climate");

        assert!(
            settings
                .fog_color
                .into_iter()
                .all(|channel| (0.0..=1.0).contains(&channel))
        );
        assert!(settings.fog_density > 0.0);
    }

    #[test]
    fn a_wetter_climate_reads_hazier() {
        let dry = settings_for(SiteClimate {
            annual_precipitation_millimeters: 200.0,
            ..SiteClimate::SURVEYED_TILE
        })
        .expect("valid climate");
        let wet = settings_for(SiteClimate {
            annual_precipitation_millimeters: 1_600.0,
            ..SiteClimate::SURVEYED_TILE
        })
        .expect("valid climate");

        assert!(wet.fog_density > dry.fog_density);
    }
}
