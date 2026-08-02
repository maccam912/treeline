//! Deterministic per-individual variation.
//!
//! Every value here is a pure function of a stable identity and a lane
//! constant, so an individual keeps the same traits no matter which chunk
//! asked for it, in what order, or on which machine.

use treeline_coordinates::stable_hash;

/// Draws a value in `[0, 1)` for one identity and one independent lane.
///
/// Lanes are distinct constants so two traits of the same individual never
/// correlate. Only the top 53 bits are used, which is the range `f64` can
/// represent exactly.
pub fn fraction(id: u64, lane: u64) -> f64 {
    hash53_as_f64(stable_hash(&[id, lane]))
}

pub fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

/// Rounds a fractional expected count to a whole number of individuals.
///
/// A cell expecting 2.4 stems yields three stems 40% of the time and two
/// otherwise, so density is preserved in aggregate without every cell rounding
/// the same way. The ceiling keeps one pathological cell from dominating a
/// chunk's vertex budget.
pub fn stochastic_count(expected: f64, rounding_fraction: f64) -> u8 {
    const MAX_PER_CELL: u8 = 16;

    let mut count = 0_u8;
    let mut remainder = expected.max(0.0);
    while remainder >= 1.0 && count < MAX_PER_CELL {
        count += 1;
        remainder -= 1.0;
    }
    count
        .saturating_add(u8::from(rounding_fraction < remainder))
        .min(MAX_PER_CELL)
}

#[allow(clippy::cast_precision_loss)]
fn hash53_as_f64(hash: u64) -> f64 {
    ((hash >> 11) as f64) / 9_007_199_254_740_992.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_stay_in_the_unit_range_and_differ_between_lanes() {
        for id in 0..64_u64 {
            let first = fraction(id, 0x01);
            let second = fraction(id, 0x02);
            assert!((0.0..1.0).contains(&first));
            assert!((0.0..1.0).contains(&second));
            assert!((first - second).abs() > f64::EPSILON);
        }
    }

    #[test]
    fn fractions_are_repeatable() {
        assert_eq!(
            fraction(0x5eed, 0x42).to_bits(),
            fraction(0x5eed, 0x42).to_bits()
        );
    }

    #[test]
    fn stochastic_counts_average_out_to_the_expected_density() {
        let total: u32 = (0..1_000)
            .map(|sample| u32::from(stochastic_count(2.4, fraction(sample, 0x99))))
            .sum();
        assert!((2_300..2_500).contains(&total));
    }

    #[test]
    fn stochastic_counts_are_bounded() {
        assert_eq!(stochastic_count(f64::MAX, 0.0), 16);
        assert_eq!(stochastic_count(-5.0, 0.5), 0);
    }
}
