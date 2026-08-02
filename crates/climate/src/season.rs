//! The annual cycle, sampled at four representative points.

/// A representative quarter of the annual cycle.
///
/// Treeline has no wall clock in generation. A season is an explicit argument,
/// so every seasonal query is reproducible and can be inspected out of order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Season {
    #[default]
    Winter,
    Spring,
    Summer,
    Autumn,
}

impl Season {
    /// The four seasons in cycle order, starting from the annual temperature low.
    pub const ALL: [Self; 4] = [Self::Winter, Self::Spring, Self::Summer, Self::Autumn];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Winter => "winter",
            Self::Spring => "spring",
            Self::Summer => "summer",
            Self::Autumn => "autumn",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Winter => Self::Spring,
            Self::Spring => Self::Summer,
            Self::Summer => Self::Autumn,
            Self::Autumn => Self::Winter,
        }
    }

    /// Position in [`Self::ALL`], used to index per-season results.
    pub const fn index(self) -> usize {
        match self {
            Self::Winter => 0,
            Self::Spring => 1,
            Self::Summer => 2,
            Self::Autumn => 3,
        }
    }

    /// Offset from the annual mean, as a fraction of seasonal amplitude.
    ///
    /// Spring and autumn are not symmetric about the mean: the ground and lakes
    /// lag the sun, so spring runs colder than autumn at equal daylight.
    pub const fn temperature_offset_fraction(self) -> f64 {
        match self {
            Self::Winter => -0.85,
            Self::Spring => -0.15,
            Self::Summer => 0.85,
            Self::Autumn => 0.15,
        }
    }

    /// Share of annual precipitation falling in this season.
    ///
    /// The site's precipitation is spread through the year with a warm-season
    /// maximum; the four shares sum to one.
    pub const fn precipitation_share(self) -> f64 {
        match self {
            Self::Winter => 0.20,
            Self::Spring => 0.25,
            Self::Summer => 0.31,
            Self::Autumn => 0.24,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seasons_cycle_through_all_four_in_order() {
        let mut season = Season::Winter;
        for expected in Season::ALL.into_iter().skip(1).chain([Season::Winter]) {
            season = season.next();
            assert_eq!(season, expected);
        }
    }

    #[test]
    fn indices_address_all_and_precipitation_shares_sum_to_one() {
        for (index, season) in Season::ALL.into_iter().enumerate() {
            assert_eq!(season.index(), index);
        }
        let total: f64 = Season::ALL
            .into_iter()
            .map(Season::precipitation_share)
            .sum();
        assert!((total - 1.0).abs() < 1.0e-9);
    }
}
