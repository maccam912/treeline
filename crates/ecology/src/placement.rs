//! Where trees stand.
//!
//! A single global lattice owns every placement candidate. Each cell decides
//! its own stems from the stand measured at its center, so two overlapping
//! requests always agree on the trees in the overlap. Filtering to the caller's
//! bounds happens after generation, which is what makes the result independent
//! of request order, chunk size, and job completion order.

use treeline_coordinates::{CellIndex, WorldIdentity, stable_hash};

use crate::individual::{GrowthConditions, ProceduralTree, grow};
use crate::random::{fraction, lerp, stochastic_count};
use crate::species::ForestComposition;
use crate::stand::Stand;

/// Edge length of one placement cell, in meters.
///
/// This matches the canopy layer's spacing, so each cell reads exactly one
/// measured stand.
pub const PLACEMENT_CELL_EDGE_METERS: f64 = 6.0;

const DOMAIN_TREE_INDIVIDUALS: u64 = 0x5452_4545_5f49_4e44;
const LANE_COUNT: u64 = 0x434f_554e_545f_5f5f;
const LANE_JITTER_X: u64 = 0x585f_4a49_5454_4552;
const LANE_JITTER_Z: u64 = 0x5a5f_4a49_5454_4552;
/// Keeps jittered stems off the exact cell edge, where two cells would meet.
const JITTER_INSET: f64 = 0.06;

/// A half-open horizontal area to generate trees for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeBounds {
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
}

impl TreeBounds {
    /// Creates finite, non-empty bounds.
    pub fn new(min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> Option<Self> {
        ([min_x, min_z, max_x, max_z].into_iter().all(f64::is_finite)
            && min_x < max_x
            && min_z < max_z)
            .then_some(Self {
                min_x,
                min_z,
                max_x,
                max_z,
            })
    }

    fn contains(self, x: f64, z: f64) -> bool {
        x >= self.min_x && x < self.max_x && z >= self.min_z && z < self.max_z
    }

    /// The inclusive range of placement cells this area touches.
    fn cell_range(self) -> Option<(CellIndex, CellIndex)> {
        Some((
            CellIndex::containing(self.min_x, self.min_z, 0, PLACEMENT_CELL_EDGE_METERS)?,
            CellIndex::containing(self.max_x, self.max_z, 0, PLACEMENT_CELL_EDGE_METERS)?,
        ))
    }
}

/// Generates tree individuals from measured stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Forest {
    world: WorldIdentity,
}

impl Forest {
    pub const fn new(world: WorldIdentity) -> Self {
        Self { world }
    }

    /// Generates every tree standing inside `bounds`.
    ///
    /// `stand_at` reports the measured canopy over a cell center, or `None`
    /// where the ground is open. Cells without a stand contribute no trees.
    ///
    /// Returns `None` when the bounds fall outside the placement lattice's
    /// representable range.
    pub fn trees_in(
        self,
        bounds: TreeBounds,
        composition: ForestComposition,
        prevailing_wind: [f64; 2],
        mut stand_at: impl FnMut(f64, f64) -> Option<Stand>,
    ) -> Option<Vec<ProceduralTree>> {
        let (minimum, maximum) = bounds.cell_range()?;
        let mut trees = Vec::new();
        for cell_z in minimum.z..=maximum.z {
            for cell_x in minimum.x..=maximum.x {
                let origin = cell_origin(cell_x, cell_z);
                let center = [
                    origin[0] + (PLACEMENT_CELL_EDGE_METERS * 0.5),
                    origin[1] + (PLACEMENT_CELL_EDGE_METERS * 0.5),
                ];
                let Some(stand) = stand_at(center[0], center[1]) else {
                    continue;
                };
                let conditions = GrowthConditions {
                    stand,
                    composition,
                    prevailing_wind,
                };
                let cell_key = CellIndex::new(cell_x, cell_z, 0)
                    .generation_key(self.world, DOMAIN_TREE_INDIVIDUALS);
                for id in stem_ids(cell_key, stand) {
                    let [x, z] = jittered_position(id, origin);
                    if bounds.contains(x, z) {
                        trees.push(grow(id, x, z, conditions));
                    }
                }
            }
        }
        trees.sort_unstable_by_key(|tree| tree.id);
        Some(trees)
    }
}

/// Stable identities of the stems one cell carries.
fn stem_ids(cell_key: u64, stand: Stand) -> impl Iterator<Item = u64> {
    const SQUARE_METERS_PER_HECTARE: f64 = 10_000.0;

    let cell_area = PLACEMENT_CELL_EDGE_METERS * PLACEMENT_CELL_EDGE_METERS;
    let expected = stand.stems_per_hectare() * cell_area / SQUARE_METERS_PER_HECTARE;
    let count = stochastic_count(expected, fraction(cell_key, LANE_COUNT));
    (0..count).map(move |ordinal| stable_hash(&[cell_key, u64::from(ordinal)]))
}

/// Scatters one stem inside its cell.
fn jittered_position(id: u64, origin: [f64; 2]) -> [f64; 2] {
    let offset = |lane| {
        PLACEMENT_CELL_EDGE_METERS * lerp(JITTER_INSET, 1.0 - JITTER_INSET, fraction(id, lane))
    };
    [
        origin[0] + offset(LANE_JITTER_X),
        origin[1] + offset(LANE_JITTER_Z),
    ]
}

fn cell_origin(cell_x: i64, cell_z: i64) -> [f64; 2] {
    [cell_x, cell_z].map(|index| index_as_f64(index) * PLACEMENT_CELL_EDGE_METERS)
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: i64) -> f64 {
    index as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, 1, 0);
    const WIND: [f64; 2] = [1.0, 0.0];

    fn uniform_stand(_x: f64, _z: f64) -> Option<Stand> {
        Stand::measured(0.85, 24.0)
    }

    fn trees_in(min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> Vec<ProceduralTree> {
        Forest::new(WORLD)
            .trees_in(
                TreeBounds::new(min_x, min_z, max_x, max_z).expect("valid bounds"),
                ForestComposition::SURVEYED_TILE,
                WIND,
                uniform_stand,
            )
            .expect("bounds are representable")
    }

    #[test]
    fn bounds_reject_empty_and_non_finite_areas() {
        assert_eq!(TreeBounds::new(0.0, 0.0, 0.0, 10.0), None);
        assert_eq!(TreeBounds::new(0.0, 0.0, f64::NAN, 10.0), None);
        assert!(TreeBounds::new(-10.0, -10.0, 10.0, 10.0).is_some());
    }

    #[test]
    fn every_tree_lands_inside_the_requested_bounds() {
        for tree in trees_in(-64.0, 32.0, 64.0, 160.0) {
            assert!(tree.x >= -64.0 && tree.x < 64.0);
            assert!(tree.z >= 32.0 && tree.z < 160.0);
        }
    }

    #[test]
    fn a_measured_stand_actually_produces_trees() {
        assert!(!trees_in(0.0, 0.0, 96.0, 96.0).is_empty());
    }

    #[test]
    fn open_ground_produces_no_trees() {
        let trees = Forest::new(WORLD)
            .trees_in(
                TreeBounds::new(0.0, 0.0, 96.0, 96.0).expect("valid bounds"),
                ForestComposition::SURVEYED_TILE,
                WIND,
                |_, _| None,
            )
            .expect("bounds are representable");
        assert!(trees.is_empty());
    }

    #[test]
    fn adjacent_requests_agree_on_the_trees_they_share() {
        let whole = trees_in(0.0, 0.0, 128.0, 64.0);
        let mut halves = trees_in(0.0, 0.0, 64.0, 64.0);
        halves.extend(trees_in(64.0, 0.0, 128.0, 64.0));
        halves.sort_unstable_by_key(|tree| tree.id);

        assert_eq!(whole, halves);
    }

    #[test]
    fn a_larger_request_contains_every_tree_of_a_smaller_one() {
        let inner = trees_in(0.0, 0.0, 48.0, 48.0);
        let outer = trees_in(-96.0, -96.0, 192.0, 192.0);

        assert!(!inner.is_empty());
        for tree in inner {
            assert!(outer.contains(&tree));
        }
    }

    #[test]
    fn generation_is_order_independent() {
        let forward = trees_in(0.0, 0.0, 64.0, 64.0);
        let _elsewhere = trees_in(4_096.0, -2_048.0, 4_160.0, -1_984.0);
        let again = trees_in(0.0, 0.0, 64.0, 64.0);

        assert_eq!(forward, again);
    }

    #[test]
    fn negative_coordinates_generate_the_same_way() {
        let trees = trees_in(-256.0, -256.0, -192.0, -192.0);
        assert!(!trees.is_empty());
        for tree in trees {
            assert!(tree.x < -192.0 && tree.z < -192.0);
        }
    }

    #[test]
    fn different_worlds_place_different_trees() {
        let other = Forest::new(WorldIdentity::new(0xd1ce, 1, 0))
            .trees_in(
                TreeBounds::new(0.0, 0.0, 64.0, 64.0).expect("valid bounds"),
                ForestComposition::SURVEYED_TILE,
                WIND,
                uniform_stand,
            )
            .expect("bounds are representable");

        assert_ne!(trees_in(0.0, 0.0, 64.0, 64.0), other);
    }

    #[test]
    fn denser_stands_carry_more_trees() {
        let bounds = TreeBounds::new(0.0, 0.0, 96.0, 96.0).expect("valid bounds");
        let count = |cover| {
            Forest::new(WORLD)
                .trees_in(bounds, ForestComposition::SURVEYED_TILE, WIND, |_, _| {
                    Stand::measured(cover, 24.0)
                })
                .expect("bounds are representable")
                .len()
        };

        assert!(count(0.9) > count(0.2) * 2);
    }
}
