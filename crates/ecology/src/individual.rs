//! One tree: its life history, size, and lean.
//!
//! Height comes from the stand's measured canopy top rather than from the
//! genotype's open-grown potential, so a stand of six-meter regrowth produces
//! six-meter trees. The genotype decides proportions within that height, and
//! the individual's stable identity decides where in the stand it sits.

use crate::random::{fraction, lerp};
use crate::species::{ForestComposition, TreeGenotype};
use crate::stand::Stand;

/// Visible life history of one tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeCondition {
    Sapling,
    Mature,
    Ancient,
    WindDamaged,
    DeadStanding,
    StormBroken,
    Fallen,
}

impl TreeCondition {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sapling => "sapling",
            Self::Mature => "mature",
            Self::Ancient => "ancient",
            Self::WindDamaged => "wind-damaged",
            Self::DeadStanding => "dead standing",
            Self::StormBroken => "storm-broken",
            Self::Fallen => "fallen",
        }
    }

    /// Share of full trunk height this condition leaves standing.
    const fn standing_height_fraction(self) -> f64 {
        match self {
            Self::Sapling => 0.28,
            Self::StormBroken => 0.62,
            Self::Fallen => 0.88,
            Self::Mature | Self::Ancient | Self::WindDamaged | Self::DeadStanding => 1.0,
        }
    }
}

/// A tree individual placed on the global horizontal lattice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProceduralTree {
    pub id: u64,
    pub x: f64,
    pub z: f64,
    pub age_years: f64,
    pub height_meters: f64,
    pub trunk_base_radius_meters: f64,
    pub crown_radius_meters: f64,
    pub lean_direction: [f64; 2],
    pub lean_fraction: f64,
    pub damage_fraction: f64,
    pub rotation_turns: f64,
    pub condition: TreeCondition,
    pub genotype: TreeGenotype,
}

/// Everything a placed tree needs beyond its own identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrowthConditions {
    pub stand: Stand,
    pub composition: ForestComposition,
    /// Unit vector the wind blows toward, which trees lean away from.
    pub prevailing_wind: [f64; 2],
}

/// Grows one tree from its identity, position, and surroundings.
pub fn grow(id: u64, x: f64, z: f64, conditions: GrowthConditions) -> ProceduralTree {
    let genotype = TreeGenotype::new(conditions.composition.select(fraction(id, LANE_GROUP)), id);
    let age_years = age(id, conditions.stand);
    let damage_fraction = fraction(id, LANE_DAMAGE) * DAMAGE_CEILING;
    let condition = condition(id, age_years, damage_fraction);
    let height_meters = height(id, &genotype, conditions.stand, condition);
    let ancient_girth = if condition == TreeCondition::Ancient {
        ANCIENT_GIRTH_MULTIPLIER
    } else {
        1.0
    };

    ProceduralTree {
        id,
        x,
        z,
        age_years,
        height_meters,
        trunk_base_radius_meters: (height_meters
            * lerp(0.025, 0.043, 1.0 - genotype.trunk_taper_fraction)
            * ancient_girth)
            .max(0.045),
        crown_radius_meters: crown_radius(&genotype, conditions.stand, height_meters),
        lean_direction: lean_direction(id, conditions.prevailing_wind),
        lean_fraction: lean_fraction(&genotype, condition),
        damage_fraction,
        rotation_turns: fraction(id, LANE_ROTATION),
        condition,
        genotype,
    }
}

/// Estimates age from how tall the stand has already grown.
///
/// Canopy height is the only age evidence the measurements carry. A young
/// share of every stand is regeneration below the canopy.
fn age(id: u64, stand: Stand) -> f64 {
    let stand_age_years = (stand.top_height_meters() * YEARS_PER_CANOPY_METER).max(1.0);
    if fraction(id, LANE_REGENERATION) < REGENERATION_SHARE {
        return 1.0 + (fraction(id, LANE_YOUNG) * 13.0);
    }
    stand_age_years * lerp(0.55, 1.45, fraction(id, LANE_AGE))
}

/// Places one tree in the stand's height profile.
///
/// The measured canopy top is a ceiling, not a target: most trees sit below it,
/// and only the oldest reach it. Suppression by a closed canopy pushes the rest
/// further down.
fn height(id: u64, genotype: &TreeGenotype, stand: Stand, condition: TreeCondition) -> f64 {
    let rank = lerp(0.42, 1.0, fraction(id, LANE_RANK));
    let suppression =
        stand.canopy_cover_fraction() * genotype.competition_response_fraction * SUPPRESSION;
    (stand.top_height_meters() * rank * (1.0 - suppression) * condition.standing_height_fraction())
        .max(0.8)
}

fn crown_radius(genotype: &TreeGenotype, stand: Stand, height_meters: f64) -> f64 {
    let crowding = stand.canopy_cover_fraction() * genotype.competition_response_fraction;
    (height_meters
        * genotype.crown_radius_fraction()
        * lerp(0.72, 1.12, genotype.leaf_density_fraction)
        * (1.0 - (crowding * 0.24)))
        .max(0.25)
}

/// Chooses a life stage from age and accumulated damage.
///
/// Order matters: the rarest outcomes are tested first so a single draw can
/// select among them without one outcome masking another.
fn condition(id: u64, age_years: f64, damage_fraction: f64) -> TreeCondition {
    if age_years < 15.0 {
        return TreeCondition::Sapling;
    }
    let event = fraction(id, LANE_EVENT);
    // Fallen trees stay dormant until their geometry can follow the ground;
    // a rigid, nearly horizontal trunk visibly floats above sloping terrain.
    if event < 0.05 {
        return TreeCondition::DeadStanding;
    }
    if event < 0.08 {
        return TreeCondition::StormBroken;
    }
    if age_years > 240.0 && event > 0.36 {
        return TreeCondition::Ancient;
    }
    if damage_fraction > 0.42 {
        return TreeCondition::WindDamaged;
    }
    TreeCondition::Mature
}

/// Leans a tree downwind, with enough scatter that a stand is not a comb.
fn lean_direction(id: u64, prevailing_wind: [f64; 2]) -> [f64; 2] {
    let jitter = [
        (fraction(id, LANE_LEAN_X) - 0.5) * 0.42,
        (fraction(id, LANE_LEAN_Z) - 0.5) * 0.42,
    ];
    let direction = [
        prevailing_wind[0] + jitter[0],
        prevailing_wind[1] + jitter[1],
    ];
    let length = libm::hypot(direction[0], direction[1]);
    if length > f64::EPSILON {
        [direction[0] / length, direction[1] / length]
    } else {
        [1.0, 0.0]
    }
}

fn lean_fraction(genotype: &TreeGenotype, condition: TreeCondition) -> f64 {
    let ordinary = genotype.wind_response_fraction * 0.04;
    match condition {
        TreeCondition::Fallen => 0.92,
        TreeCondition::WindDamaged => ordinary + 0.12,
        _ => ordinary,
    }
    .clamp(0.0, 0.96)
}

/// Years of growth represented by one meter of canopy height.
const YEARS_PER_CANOPY_METER: f64 = 4.5;
/// Share of stems that are regeneration below the measured canopy.
const REGENERATION_SHARE: f64 = 0.14;
/// How far a fully closed canopy holds back a maximally responsive individual.
const SUPPRESSION: f64 = 0.22;
const DAMAGE_CEILING: f64 = 0.55;
const ANCIENT_GIRTH_MULTIPLIER: f64 = 1.28;

const LANE_GROUP: u64 = 0x4752_4f55_505f_5f5f;
const LANE_AGE: u64 = 0x4147_455f_5641_5259;
const LANE_YOUNG: u64 = 0x594f_554e_475f_5f5f;
const LANE_REGENERATION: u64 = 0x5245_4745_4e45_5241;
const LANE_RANK: u64 = 0x4841_4e4b_5f52_414e;
const LANE_EVENT: u64 = 0x4556_454e_545f_5f5f;
const LANE_DAMAGE: u64 = 0x4441_4d41_4745_5f5f;
const LANE_ROTATION: u64 = 0x524f_5441_5449_4f4e;
const LANE_LEAN_X: u64 = 0x4c45_414e_5f58_5f5f;
const LANE_LEAN_Z: u64 = 0x4c45_414e_5f5a_5f5f;

#[cfg(test)]
mod tests {
    use super::*;

    fn conditions(cover: f64, top_height: f64) -> GrowthConditions {
        GrowthConditions {
            stand: Stand::measured(cover, top_height).expect("measured stand"),
            composition: ForestComposition::SURVEYED_TILE,
            prevailing_wind: [1.0, 0.0],
        }
    }

    #[test]
    fn no_tree_grows_taller_than_the_measured_canopy() {
        let conditions = conditions(0.8, 12.0);
        for id in 0..2_000_u64 {
            let tree = grow(id, 0.0, 0.0, conditions);
            assert!(
                tree.height_meters <= conditions.stand.top_height_meters(),
                "tree {id} exceeded the measured canopy"
            );
        }
    }

    #[test]
    fn short_regrowth_produces_short_trees() {
        let tall = grow(0x5eed, 0.0, 0.0, conditions(0.9, 34.0));
        let short = grow(0x5eed, 0.0, 0.0, conditions(0.9, 5.5));
        assert!(short.height_meters < 5.5);
        assert!(tall.height_meters > short.height_meters * 3.0);
    }

    #[test]
    fn a_tree_is_identical_for_one_identity_and_stand() {
        let conditions = conditions(0.7, 22.0);
        assert_eq!(
            grow(0x5eed, 12.0, -4.0, conditions),
            grow(0x5eed, 12.0, -4.0, conditions)
        );
    }

    #[test]
    fn a_closed_canopy_suppresses_growth() {
        let open = grow(0x5eed, 0.0, 0.0, conditions(0.05, 30.0));
        let closed = grow(0x5eed, 0.0, 0.0, conditions(1.0, 30.0));
        assert!(closed.height_meters < open.height_meters);
        assert!(closed.crown_radius_meters < open.crown_radius_meters);
    }

    #[test]
    fn a_stand_contains_a_range_of_life_stages_but_no_fallen_trees() {
        let conditions = conditions(0.8, 30.0);
        let mut seen = Vec::new();
        for id in 0..4_000_u64 {
            let condition = grow(id, 0.0, 0.0, conditions).condition;
            if !seen.contains(&condition) {
                seen.push(condition);
            }
        }
        assert!(seen.contains(&TreeCondition::Mature));
        assert!(seen.contains(&TreeCondition::Sapling));
        assert!(!seen.contains(&TreeCondition::Fallen));
        assert!(seen.contains(&TreeCondition::DeadStanding));
    }

    #[test]
    fn trees_lean_downwind() {
        let conditions = GrowthConditions {
            prevailing_wind: [0.0, 1.0],
            ..conditions(0.8, 24.0)
        };
        let downwind = (0..256_u64)
            .map(|id| grow(id, 0.0, 0.0, conditions).lean_direction[1])
            .filter(|component| *component > 0.0)
            .count();
        assert_eq!(downwind, 256);
    }

    #[test]
    fn proportions_stay_physically_plausible() {
        let conditions = conditions(0.6, 28.0);
        for id in 0..500_u64 {
            let tree = grow(id, 0.0, 0.0, conditions);
            assert!(tree.trunk_base_radius_meters > 0.0);
            assert!(tree.trunk_base_radius_meters < tree.height_meters);
            assert!(tree.crown_radius_meters > 0.0);
            assert!(tree.crown_radius_meters < tree.height_meters);
            assert!((0.0..=1.0).contains(&tree.rotation_turns));
        }
    }
}
