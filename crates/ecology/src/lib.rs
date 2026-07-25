//! Ecosystem primitives built from environmental variables rather than biome IDs.

/// Local environmental conditions normalized to the inclusive range 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    pub temperature: f64,
    pub moisture: f64,
    pub soil_depth: f64,
    pub sunlight: f64,
    pub disturbance: f64,
}

/// Preferred environmental center and tolerance for a procedural species.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeciesPreference {
    pub temperature: f64,
    pub moisture: f64,
    pub tolerance: f64,
}

impl SpeciesPreference {
    /// Returns a normalized suitability score without assigning a biome label.
    pub fn suitability(self, environment: Environment) -> f64 {
        if self.tolerance <= 0.0 {
            return 0.0;
        }
        let temperature_delta = (environment.temperature - self.temperature).abs();
        let moisture_delta = (environment.moisture - self.moisture).abs();
        let distance = temperature_delta.hypot(moisture_delta);
        (1.0 - (distance / self.tolerance)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suitability_peaks_at_the_species_preference() {
        let preference = SpeciesPreference {
            temperature: 0.4,
            moisture: 0.8,
            tolerance: 0.5,
        };
        let environment = Environment {
            temperature: 0.4,
            moisture: 0.8,
            soil_depth: 0.7,
            sunlight: 0.5,
            disturbance: 0.1,
        };
        assert!((preference.suitability(environment) - 1.0).abs() < f64::EPSILON);
    }
}
