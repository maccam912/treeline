//! What kinds of tree grow here, and what shape each one takes.
//!
//! Lidar measures how much forest stands somewhere and how tall it is, but not
//! what species it is. Species therefore stays procedural: a site-level mix of
//! growth strategies, plus a per-individual genotype that varies architecture
//! within a strategy. The renderer consumes the genotype as a grammar rather
//! than selecting from a library of tree models.

use crate::random::{fraction, lerp};

/// A tree growth strategy, used as a continuous mixture rather than a biome.
///
/// The surveyed site is northern mixed forest, so these are the three
/// strategies that actually occur there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeFunctionalGroup {
    EvergreenNeedleleaf,
    ColdDeciduous,
    TemperateBroadleaf,
}

impl TreeFunctionalGroup {
    pub const ALL: [Self; 3] = [
        Self::EvergreenNeedleleaf,
        Self::ColdDeciduous,
        Self::TemperateBroadleaf,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::EvergreenNeedleleaf => "evergreen needleleaf",
            Self::ColdDeciduous => "cold deciduous",
            Self::TemperateBroadleaf => "temperate broadleaf",
        }
    }
}

/// Relative abundance of each growth strategy.
///
/// Fractions are normalized on construction, so a mixture is always a
/// probability distribution over [`TreeFunctionalGroup::ALL`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForestComposition {
    fractions: [f64; 3],
}

impl ForestComposition {
    /// The surveyed site's mixture: northern hardwood over conifer.
    ///
    /// Michigan's Upper Peninsula carries sugar maple and beech with hemlock,
    /// white pine, and spruce, plus aspen and birch on disturbed ground.
    pub const SURVEYED_TILE: Self = Self {
        fractions: [0.38, 0.24, 0.38],
    };

    /// Normalizes a mixture, rejecting negative or all-zero weights.
    pub fn new(weights: [f64; 3]) -> Option<Self> {
        let total: f64 = weights.iter().sum();
        (weights
            .iter()
            .all(|weight| weight.is_finite() && *weight >= 0.0)
            && total > 0.0)
            .then(|| Self {
                fractions: weights.map(|weight| weight / total),
            })
    }

    pub fn fraction(self, group: TreeFunctionalGroup) -> f64 {
        self.fractions[Self::slot(group)]
    }

    pub fn dominant(self) -> TreeFunctionalGroup {
        TreeFunctionalGroup::ALL
            .into_iter()
            .reduce(|dominant, group| {
                if self.fraction(group) > self.fraction(dominant) {
                    group
                } else {
                    dominant
                }
            })
            .unwrap_or(TreeFunctionalGroup::EvergreenNeedleleaf)
    }

    /// Picks a strategy from a uniform draw, weighted by abundance.
    pub fn select(self, selection: f64) -> TreeFunctionalGroup {
        let mut cumulative = 0.0;
        for group in TreeFunctionalGroup::ALL {
            cumulative += self.fraction(group);
            if selection <= cumulative {
                return group;
            }
        }
        TreeFunctionalGroup::TemperateBroadleaf
    }

    const fn slot(group: TreeFunctionalGroup) -> usize {
        match group {
            TreeFunctionalGroup::EvergreenNeedleleaf => 0,
            TreeFunctionalGroup::ColdDeciduous => 1,
            TreeFunctionalGroup::TemperateBroadleaf => 2,
        }
    }
}

/// Broad crown architecture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrownShape {
    Conical,
    Columnar,
    Rounded,
}

/// Bark architecture, which drives trunk color and surface treatment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarkStyle {
    Scaly,
    Smooth,
    Furrowed,
}

/// Architecture and environmental responses of one tree individual.
///
/// Proportions are ratios rather than absolute sizes: measured canopy height
/// sets how tall a tree actually grows, and these values decide its shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeGenotype {
    pub functional_group: TreeFunctionalGroup,
    /// Height this strategy reaches at maturity in the open, in meters.
    pub mature_height_meters: f64,
    pub height_variation_fraction: f64,
    pub trunk_taper_fraction: f64,
    pub branching_angle_radians: f64,
    pub branch_density_fraction: f64,
    pub crown_shape: CrownShape,
    pub leaf_density_fraction: f64,
    pub bark_style: BarkStyle,
    pub wind_response_fraction: f64,
    /// How strongly a closed canopy suppresses this individual's growth.
    pub competition_response_fraction: f64,
}

impl TreeGenotype {
    /// Derives one individual's architecture from its strategy and identity.
    pub fn new(group: TreeFunctionalGroup, id: u64) -> Self {
        let architecture = fraction(id, LANE_ARCHITECTURE);
        let foliage = fraction(id, LANE_FOLIAGE);
        let response = fraction(id, LANE_RESPONSE);
        match group {
            TreeFunctionalGroup::EvergreenNeedleleaf => Self {
                functional_group: group,
                mature_height_meters: lerp(23.0, 38.0, architecture),
                height_variation_fraction: fraction(id, LANE_HEIGHT),
                trunk_taper_fraction: lerp(0.64, 0.88, architecture),
                branching_angle_radians: lerp(0.82, 1.28, foliage),
                branch_density_fraction: lerp(0.70, 1.0, foliage),
                crown_shape: CrownShape::Conical,
                leaf_density_fraction: lerp(0.72, 1.0, foliage),
                bark_style: BarkStyle::Scaly,
                wind_response_fraction: lerp(0.52, 0.82, response),
                competition_response_fraction: lerp(0.66, 0.90, architecture),
            },
            TreeFunctionalGroup::ColdDeciduous => Self {
                functional_group: group,
                mature_height_meters: lerp(17.0, 29.0, architecture),
                height_variation_fraction: fraction(id, LANE_HEIGHT),
                trunk_taper_fraction: lerp(0.48, 0.72, architecture),
                branching_angle_radians: lerp(0.70, 1.16, foliage),
                branch_density_fraction: lerp(0.48, 0.82, foliage),
                crown_shape: CrownShape::Columnar,
                leaf_density_fraction: lerp(0.50, 0.86, foliage),
                bark_style: BarkStyle::Smooth,
                wind_response_fraction: lerp(0.58, 0.88, response),
                competition_response_fraction: lerp(0.58, 0.84, architecture),
            },
            TreeFunctionalGroup::TemperateBroadleaf => Self {
                functional_group: group,
                mature_height_meters: lerp(21.0, 35.0, architecture),
                height_variation_fraction: fraction(id, LANE_HEIGHT),
                trunk_taper_fraction: lerp(0.38, 0.64, architecture),
                branching_angle_radians: lerp(0.62, 1.08, foliage),
                branch_density_fraction: lerp(0.56, 0.92, foliage),
                crown_shape: CrownShape::Rounded,
                leaf_density_fraction: lerp(0.62, 1.0, foliage),
                bark_style: BarkStyle::Furrowed,
                wind_response_fraction: lerp(0.36, 0.68, response),
                competition_response_fraction: lerp(0.62, 0.92, architecture),
            },
        }
    }

    /// Crown radius as a fraction of trunk height, before competition.
    pub const fn crown_radius_fraction(self) -> f64 {
        match self.crown_shape {
            CrownShape::Conical => 0.18,
            CrownShape::Columnar => 0.20,
            CrownShape::Rounded => 0.26,
        }
    }
}

const LANE_ARCHITECTURE: u64 = 0x4152_4348_4954_4543;
const LANE_FOLIAGE: u64 = 0x464f_4c49_4147_455f;
const LANE_RESPONSE: u64 = 0x5245_5350_4f4e_5345;
const LANE_HEIGHT: u64 = 0x4845_4947_4854_5f56;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compositions_normalize_and_reject_degenerate_weights() {
        let composition = ForestComposition::new([2.0, 1.0, 1.0]).expect("positive weights");
        assert!(
            (composition.fraction(TreeFunctionalGroup::EvergreenNeedleleaf) - 0.5).abs() < 1.0e-9
        );
        assert_eq!(ForestComposition::new([0.0; 3]), None);
        assert_eq!(ForestComposition::new([1.0, -1.0, 1.0]), None);
    }

    #[test]
    fn the_site_mixture_sums_to_one() {
        let total: f64 = TreeFunctionalGroup::ALL
            .into_iter()
            .map(|group| ForestComposition::SURVEYED_TILE.fraction(group))
            .sum();
        assert!((total - 1.0).abs() < 1.0e-9);
    }

    #[test]
    fn selection_covers_every_group_in_proportion() {
        let composition = ForestComposition::SURVEYED_TILE;
        let mut counts = [0_u32; 3];
        for sample in 0..3_000_u64 {
            let group = composition.select(fraction(sample, 0x11));
            counts[TreeFunctionalGroup::ALL
                .iter()
                .position(|candidate| *candidate == group)
                .expect("group is enumerated")] += 1;
        }
        assert!(counts.iter().all(|count| *count > 500));
    }

    #[test]
    fn a_genotype_is_stable_for_one_identity() {
        let group = TreeFunctionalGroup::TemperateBroadleaf;
        assert_eq!(
            TreeGenotype::new(group, 0x5eed),
            TreeGenotype::new(group, 0x5eed)
        );
    }

    #[test]
    fn needleleaf_bark_and_crown_stay_conifer_shaped() {
        let genotype = TreeGenotype::new(TreeFunctionalGroup::EvergreenNeedleleaf, 7);
        assert_eq!(genotype.crown_shape, CrownShape::Conical);
        assert_eq!(genotype.bark_style, BarkStyle::Scaly);
    }
}
