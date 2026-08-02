//! Measured forest structure, as the tree generator consumes it.
//!
//! A [`Stand`] is what lidar actually reports about a patch of forest: how much
//! of it is covered by canopy, and how tall the tallest return is. Everything
//! about individual trees is derived from those two numbers plus species
//! grammar, so the measurements bound the forest instead of decorating it.

/// Measured canopy structure over one patch of ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stand {
    canopy_cover_fraction: f64,
    top_height_meters: f64,
}

/// Tallest plausible canopy return, in meters.
///
/// The preparation pipeline already rejects returns above this height as
/// artifacts; the same ceiling applies here so a corrupt sample cannot produce
/// an implausible tree.
const MAX_CANOPY_HEIGHT_METERS: f64 = 60.0;

/// Stems per hectare in a fully closed stand of short, dense regrowth.
const CLOSED_REGROWTH_STEMS_PER_HECTARE: f64 = 1_400.0;
/// Stems per hectare in a fully closed stand of mature, tall trees.
const CLOSED_MATURE_STEMS_PER_HECTARE: f64 = 400.0;
/// Canopy height at which a stand counts as fully mature, in meters.
const MATURE_CANOPY_HEIGHT_METERS: f64 = 35.0;

impl Stand {
    /// Accepts one canopy measurement, rejecting values outside the contract.
    ///
    /// Returns `None` for a non-finite, negative, or implausible sample, and
    /// for a cell with no canopy at all — open ground is the absence of a
    /// stand, not a stand with zero trees.
    pub fn measured(canopy_cover_fraction: f64, top_height_meters: f64) -> Option<Self> {
        (canopy_cover_fraction.is_finite()
            && top_height_meters.is_finite()
            && canopy_cover_fraction > 0.0
            && top_height_meters > 0.0
            && top_height_meters <= MAX_CANOPY_HEIGHT_METERS)
            .then_some(Self {
                canopy_cover_fraction: canopy_cover_fraction.min(1.0),
                top_height_meters,
            })
    }

    /// Fraction of the patch under canopy.
    pub const fn canopy_cover_fraction(self) -> f64 {
        self.canopy_cover_fraction
    }

    /// Height of the tallest measured return, in meters.
    pub const fn top_height_meters(self) -> f64 {
        self.top_height_meters
    }

    /// Estimates stem density from cover and height.
    ///
    /// Cover scales density directly. Height sets what a closed canopy means:
    /// dense young regrowth packs many small stems into the same cover that a
    /// mature stand fills with a few large crowns.
    pub fn stems_per_hectare(self) -> f64 {
        let maturity = (self.top_height_meters / MATURE_CANOPY_HEIGHT_METERS).clamp(0.0, 1.0);
        let closed_density = CLOSED_MATURE_STEMS_PER_HECTARE
            + ((CLOSED_REGROWTH_STEMS_PER_HECTARE - CLOSED_MATURE_STEMS_PER_HECTARE)
                * (1.0 - maturity));
        self.canopy_cover_fraction * closed_density
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_ground_and_implausible_samples_are_not_stands() {
        assert_eq!(Stand::measured(0.0, 12.0), None);
        assert_eq!(Stand::measured(0.6, 0.0), None);
        assert_eq!(Stand::measured(0.6, 80.0), None);
        assert_eq!(Stand::measured(f64::NAN, 12.0), None);
        assert_eq!(Stand::measured(-0.2, 12.0), None);
    }

    #[test]
    fn cover_above_one_is_clamped() {
        let stand = Stand::measured(1.4, 20.0).expect("measured stand");
        assert_eq!(stand.canopy_cover_fraction().to_bits(), 1.0_f64.to_bits());
    }

    #[test]
    fn denser_cover_carries_more_stems() {
        let sparse = Stand::measured(0.25, 20.0).expect("measured stand");
        let dense = Stand::measured(0.95, 20.0).expect("measured stand");
        assert!(dense.stems_per_hectare() > sparse.stems_per_hectare() * 3.0);
    }

    #[test]
    fn young_regrowth_is_denser_than_mature_forest_at_equal_cover() {
        let regrowth = Stand::measured(1.0, 4.0).expect("measured stand");
        let mature = Stand::measured(1.0, 34.0).expect("measured stand");
        assert!(regrowth.stems_per_hectare() > mature.stems_per_hectare() * 2.0);
        assert!(mature.stems_per_hectare() >= CLOSED_MATURE_STEMS_PER_HECTARE);
    }
}
