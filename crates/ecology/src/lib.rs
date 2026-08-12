//! Forest structure and the individual trees that express it.
//!
//! Lidar tells us how much canopy stands where and how tall it is. It does not
//! tell us what species those trees are, how old they are, or which ones have
//! fallen. This crate keeps that split explicit:
//!
//! - [`Stand`] is the measurement: cover and canopy height over a patch.
//! - [`ForestComposition`] and [`TreeGenotype`] are the species grammar.
//! - [`Forest`] places [`ProceduralTree`] individuals, sized by the stand they
//!   grow in and shaped by their genotype.
//!
//! Measurements bound the result. A stand of six-meter regrowth cannot produce
//! a thirty-meter pine, whatever the genotype would grow to in the open.

mod individual;
mod placement;
mod random;
mod species;
mod stand;

pub use individual::{GrowthConditions, ProceduralTree, TreeCondition, grow as grow_tree};
pub use placement::{Forest, PLACEMENT_CELL_EDGE_METERS, TreeBounds};
pub use species::{BarkStyle, CrownShape, ForestComposition, TreeFunctionalGroup, TreeGenotype};
pub use stand::Stand;

/// Generator version that first sizes individuals from measured canopy.
///
/// Increment when a change makes the same world identity and the same measured
/// stand produce different trees.
pub const FOREST_GENERATOR_VERSION: u32 = 22;
