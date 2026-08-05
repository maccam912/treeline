//! How much of a crown each detail tier draws.
//!
//! Every tier fills the same envelope, so a crown does not change shape when a
//! tile crosses a detail boundary — it only loses the grain of itself. What a
//! coarser tier gives up in ball count it takes back in ball size, which is why
//! these numbers have to move together.
//!
//! Shelling a ball multiplies everything below by the shell count, so the near
//! tier spends its budget on depth within a shoot rather than on more shoots:
//! it draws fewer, larger balls than it did when a ball was one solid. Needles
//! that stand out of a ball fill the space the extra balls used to fake.

use treeline_ecology::ProceduralTree;

use crate::TreeMeshDetail;
use crate::vertex::{f64_as_f32, usize_as_f32};

/// How many whorls one crown carries.
///
/// Whorls thin with distance rather than shrinking, so a crown keeps the tiering
/// that makes it read as a conifer for as long as it is worth drawing at all.
pub(super) fn whorl_count(crown_length: f32, detail: TreeMeshDetail) -> usize {
    let (spacing, limit) = match detail {
        TreeMeshDetail::Full => (1.7, 9),
        TreeMeshDetail::Simplified => (2.4, 7),
        TreeMeshDetail::Silhouette => (4.2, 4),
    };
    round_as_usize(libm::roundf(crown_length / spacing).clamp(2.0, usize_as_f32(limit)))
}

/// How many branches one whorl rings the trunk with.
///
/// This is what decides whether a ring closes. Too few and the bunches on it read
/// as separate lobes with sky between them from every angle; the balls then have
/// to grow to cover the gap, and a crown of big balls is a crown of boulders.
pub(super) fn branches_per_whorl(tree: ProceduralTree, detail: TreeMeshDetail) -> usize {
    let density = f64_as_f32(tree.genotype.branch_density_fraction).clamp(0.0, 1.0);
    match detail {
        TreeMeshDetail::Full => 6 + round_as_usize(libm::roundf(density)),
        TreeMeshDetail::Simplified => 4,
        TreeMeshDetail::Silhouette => 3,
    }
}

/// How many balls one branch strings out.
///
/// Two is what makes a branch read as a bunch rather than a blob on a stick.
/// A third ball used to be what broke up the run between them; needles standing
/// out of the two now do that, and for a fifth of the cost.
pub(super) const fn clusters_per_branch(detail: TreeMeshDetail) -> usize {
    match detail {
        TreeMeshDetail::Full | TreeMeshDetail::Simplified => 2,
        TreeMeshDetail::Silhouette => 1,
    }
}

/// How big a tip ball is against the branch it hangs off.
///
/// This is the counterweight to the three counts above: it is what holds the
/// envelope full as a coarser tier draws fewer balls into it.
pub(super) const fn cluster_span(detail: TreeMeshDetail) -> f32 {
    match detail {
        TreeMeshDetail::Full => 0.52,
        TreeMeshDetail::Simplified => 0.48,
        TreeMeshDetail::Silhouette => 0.58,
    }
}

/// How many nested shells one ball of needles is drawn as.
///
/// Every shell is another pass over the same pixels, so this is the most
/// expensive number here and the first one to spend on the near tier alone.
/// Two is the floor and gives a shoot a bare inside and outside; five is enough
/// that a needle reads as a needle rather than as a step.
pub(super) const fn shells_per_cluster(detail: TreeMeshDetail) -> usize {
    match detail {
        TreeMeshDetail::Full => 5,
        TreeMeshDetail::Simplified => 3,
        TreeMeshDetail::Silhouette => 2,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round_as_usize(value: f32) -> usize {
    value as usize
}
