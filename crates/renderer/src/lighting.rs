//! Daylight, sky, and atmosphere as the rest of the engine sees them.
//!
//! These are the renderer's public lighting inputs. Their GPU byte layouts live
//! in [`crate::uniform`]; nothing here knows about wgpu.

/// Maximum horizontal caster distance needed by the cascaded shadow maps.
pub const SHADOW_CASTER_DISTANCE_METERS: f64 = 480.0;

/// Curated daylight states that exercise the complete sky and sun model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeOfDay {
    Dawn,
    #[default]
    Noon,
    Dusk,
}

impl TimeOfDay {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Dawn => Self::Noon,
            Self::Noon => Self::Dusk,
            Self::Dusk => Self::Dawn,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dawn => "dawn",
            Self::Noon => "noon",
            Self::Dusk => "dusk",
        }
    }
}

/// Coherent sun, sky, and ambient-light inputs shared by every render path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingSettings {
    pub sun_direction: [f32; 3],
    pub sun_intensity: f32,
    pub sun_color: [f32; 3],
    pub sky_zenith: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub ground_ambient: [f32; 3],
}

impl LightingSettings {
    pub const fn for_time_of_day(time: TimeOfDay) -> Self {
        match time {
            TimeOfDay::Dawn => Self {
                sun_direction: [0.941, 0.224, 0.254],
                sun_intensity: 0.58,
                sun_color: [1.00, 0.47, 0.22],
                sky_zenith: [0.09, 0.18, 0.38],
                sky_horizon: [0.79, 0.38, 0.24],
                ground_ambient: [0.12, 0.08, 0.08],
            },
            TimeOfDay::Noon => Self {
                sun_direction: [0.457, 0.812, 0.355],
                sun_intensity: 0.88,
                sun_color: [1.00, 0.88, 0.70],
                sky_zenith: [0.16, 0.38, 0.73],
                sky_horizon: [0.42, 0.63, 0.85],
                ground_ambient: [0.13, 0.10, 0.07],
            },
            TimeOfDay::Dusk => Self {
                sun_direction: [-0.920, 0.207, -0.332],
                sun_intensity: 0.52,
                sun_color: [1.00, 0.39, 0.18],
                sky_zenith: [0.08, 0.12, 0.29],
                sky_horizon: [0.73, 0.30, 0.25],
                ground_ambient: [0.11, 0.07, 0.08],
            },
        }
    }
}

impl Default for LightingSettings {
    fn default() -> Self {
        Self::for_time_of_day(TimeOfDay::default())
    }
}

/// Renderer-facing atmosphere controls sampled from the world's local climate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtmosphereSettings {
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    pub moisture: f32,
    pub prevailing_wind: [f32; 2],
}

impl Default for AtmosphereSettings {
    fn default() -> Self {
        Self {
            fog_color: [0.39, 0.57, 0.72],
            fog_density: 1.0,
            moisture: 0.45,
            prevailing_wind: [0.8, 0.2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daylight_presets_cycle_through_the_whole_day() {
        assert_eq!(TimeOfDay::Dawn.next(), TimeOfDay::Noon);
        assert_eq!(TimeOfDay::Noon.next(), TimeOfDay::Dusk);
        assert_eq!(TimeOfDay::Dusk.next(), TimeOfDay::Dawn);
    }

    #[test]
    fn the_sun_is_above_the_horizon_at_every_preset() {
        for time in [TimeOfDay::Dawn, TimeOfDay::Noon, TimeOfDay::Dusk] {
            let settings = LightingSettings::for_time_of_day(time);
            assert!(
                settings.sun_direction[1] > 0.0,
                "{} sun is below ground",
                time.label()
            );
            assert!(settings.sun_intensity > 0.0);
        }
    }

    #[test]
    fn noon_is_the_brightest_preset() {
        let noon = LightingSettings::for_time_of_day(TimeOfDay::Noon).sun_intensity;
        for time in [TimeOfDay::Dawn, TimeOfDay::Dusk] {
            assert!(LightingSettings::for_time_of_day(time).sun_intensity < noon);
        }
    }
}
