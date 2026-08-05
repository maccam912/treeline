//! The deterministic randomness one crown is built from.

use crate::vertex::hash_fraction;

/// Per-crown variation, one lane per draw.
///
/// A crown draws a few thousand values, so lanes are just a running count: the
/// same tree always draws the same sequence, and no two trees share one. That is
/// what lets a stand be meshed in any order, or one tree at a time, and come out
/// bit-for-bit the same.
pub(super) struct Variation {
    id: u64,
    lane: u64,
}

impl Variation {
    pub(super) const fn new(id: u64) -> Self {
        Self { id, lane: 0 }
    }

    /// The next value in `[0, 1)`.
    pub(super) fn next(&mut self) -> f32 {
        self.lane += 1;
        hash_fraction(self.id, self.lane)
    }

    /// The next value in `[-0.5, 0.5)`.
    pub(super) fn signed(&mut self) -> f32 {
        self.next() - 0.5
    }

    /// A seed of its own for one ball, drawn from the same sequence.
    pub(super) fn seed(&mut self) -> u64 {
        self.lane += 1;
        self.id
            .rotate_left(17)
            .wrapping_add(self.lane.wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }
}
