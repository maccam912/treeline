//! Simulation settings that keep survival pressures independently adjustable.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pressure {
    Off,
    Gentle,
    Moderate,
    Demanding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurvivalSettings {
    pub hunger: Pressure,
    pub thirst: Pressure,
    pub temperature: Pressure,
    pub injuries: Pressure,
    pub weather: Pressure,
    pub wildlife: Pressure,
    pub navigation: Pressure,
}

impl Default for SurvivalSettings {
    fn default() -> Self {
        Self {
            hunger: Pressure::Gentle,
            thirst: Pressure::Moderate,
            temperature: Pressure::Moderate,
            injuries: Pressure::Gentle,
            weather: Pressure::Moderate,
            wildlife: Pressure::Gentle,
            navigation: Pressure::Gentle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survival_pressures_are_independent() {
        let settings = SurvivalSettings {
            hunger: Pressure::Off,
            navigation: Pressure::Demanding,
            ..SurvivalSettings::default()
        };
        assert_eq!(settings.hunger, Pressure::Off);
        assert_eq!(settings.navigation, Pressure::Demanding);
        assert_eq!(settings.weather, Pressure::Moderate);
    }
}
