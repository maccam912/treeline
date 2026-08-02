//! Entropy for things that are meant to be different every time.
//!
//! World generation never calls this. Warp destinations are a player action,
//! not part of the world, so they are allowed to be unrepeatable — everything
//! the warp lands on is still fully determined by the destination.

/// Draws a value in `[0, 1)`.
#[cfg(target_arch = "wasm32")]
pub fn unit_interval() -> f64 {
    js_sys::Math::random()
}

/// Draws a value in `[0, 1)`.
///
/// Mixes wall-clock time with a monotonic counter, so repeated calls within one
/// clock tick still differ.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::cast_precision_loss)]
pub fn unit_interval() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use treeline_coordinates::stable_hash;

    static NONCE: AtomicU64 = AtomicU64::new(0);

    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let entropy = elapsed.as_secs() ^ u64::from(elapsed.subsec_nanos()).rotate_left(32);
    let mixed = stable_hash(&[entropy, NONCE.fetch_add(1, Ordering::Relaxed)]);
    ((mixed >> 11) as f64) / 9_007_199_254_740_992.0
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn draws_stay_in_the_unit_range() {
        for _ in 0..1_000 {
            assert!((0.0..1.0).contains(&unit_interval()));
        }
    }

    #[test]
    fn consecutive_draws_differ() {
        let draws = (0..64)
            .map(|_| unit_interval().to_bits())
            .collect::<Vec<_>>();
        let unique = draws.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), draws.len());
    }
}
