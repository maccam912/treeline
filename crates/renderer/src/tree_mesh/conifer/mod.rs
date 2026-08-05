//! Conifer crowns, massed from bunches of needle clusters.
//!
//! A conifer carries its foliage the way a vine carries grapes. Whorls of
//! branches ring the trunk, each ring shorter and drooping further than the one
//! above it, and along the outer half of every branch hangs a bunch of shoot
//! clusters — biggest at the tip, where the light is. The crown's outline is
//! then the union of a few hundred overlapping balls, broken at the scale a real
//! crown is broken at: about one shoot across.
//!
//! This file owns the crown's shape — where whorls sit and how far they reach.
//! What hangs off a branch is [`bunch`], how much of it each distance draws is
//! [`tier`], and one ball is [`crate::tree_mesh::cluster`].
//!
//! What this replaced was a solid skirt per whorl. That was cheaper, and correct
//! in outline from a distance, but up close a conifer became a faceted cone
//! wearing needle wallpaper — and nothing painted on a flat facet reads as
//! foliage, however good the paint. Clusters cost roughly three times the
//! triangles and buy a silhouette that is actually ragged. They are still
//! opaque, still one draw call, and still a small fraction of what a crown of
//! alpha-tested billboards spent.

mod bunch;
mod tier;
mod variation;

use glam::Vec3;
use treeline_ecology::ProceduralTree;

use crate::tree_mesh::cluster::{CLUSTER_REACH, Cluster, append_cluster};
use crate::tree_mesh::color::foliage_tone;
use crate::tree_mesh::geometry::TreeGeometry;
use crate::vertex::{f64_as_f32, usize_as_f32};
use crate::{RendererError, TreeMeshDetail};
use bunch::{Bunch, append_bunch};
use tier::{
    branches_per_whorl, cluster_span, clusters_per_branch, shells_per_cluster, whorl_count,
};
use variation::Variation;

/// Crown profile exponent. Just under one bows the cone out slightly, the way a
/// conifer's lower branches carry further than a straight taper would.
const CROWN_TAPER: f32 = 0.86;
/// How far a branch tip drops under its own weight, as a fraction of its reach.
const BRANCH_SAG: f32 = 0.32;
/// Turns each whorl is rotated from the one below it, so bunches interleave
/// instead of stacking into vertical ribs.
const WHORL_TURN: f32 = 0.618_034;

/// Draws a conifer crown between `crown_base` and `apex` into `geometry`.
pub(crate) fn append_conifer_crown(
    geometry: &mut TreeGeometry,
    tree: ProceduralTree,
    crown_base: Vec3,
    apex: Vec3,
    crown_radius: f32,
    foliage: [f32; 4],
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    let axis = apex - crown_base;
    let length = axis.length();
    if length <= f32::EPSILON || crown_radius <= f32::EPSILON {
        return Ok(());
    }
    let up = axis / length;
    let (tangent, bitangent) = crown_frame(up);
    let whorls = whorl_count(length, detail);
    let crown = Crown {
        base: crown_base,
        up,
        tangent,
        bitangent,
        length,
        radius: crown_radius,
        foliage,
        branches: branches_per_whorl(tree, detail),
        clusters: clusters_per_branch(detail),
        shells: shells_per_cluster(detail),
        span: cluster_span(detail),
        turn: f64_as_f32(tree.rotation_turns),
        thinning: 1.0 - (f64_as_f32(tree.damage_fraction) * 0.42),
    };
    let mut variation = Variation::new(tree.id);
    for whorl in 0..whorls {
        append_whorl(geometry, &crown, &mut variation, whorl, whorls)?;
    }
    append_leader(geometry, &crown, &mut variation, apex)
}

/// One tree's crown, as every whorl in it sees it.
pub(super) struct Crown {
    base: Vec3,
    pub(super) up: Vec3,
    pub(super) tangent: Vec3,
    bitangent: Vec3,
    length: f32,
    radius: f32,
    pub(super) foliage: [f32; 4],
    /// Branches around one ring.
    branches: usize,
    /// Balls strung along one branch.
    pub(super) clusters: usize,
    /// Nested shells one ball is drawn as.
    pub(super) shells: usize,
    /// A tip ball's radius, as a fraction of the branch it hangs off.
    pub(super) span: f32,
    /// Turn of the lowest whorl's first branch, in turns.
    turn: f32,
    /// How much of its reach a damaged crown keeps.
    thinning: f32,
}

/// Appends one whorl: a ring of branches, each carrying a bunch.
fn append_whorl(
    geometry: &mut TreeGeometry,
    crown: &Crown,
    variation: &mut Variation,
    whorl: usize,
    whorls: usize,
) -> Result<(), RendererError> {
    let height = (usize_as_f32(whorl) + 0.5) / usize_as_f32(whorls);
    let spacing = crown.length / usize_as_f32(whorls);
    let reach = (crown.radius
        * libm::powf(1.0 - height, CROWN_TAPER)
        * shaded_out(height)
        * crown.thinning
        * (0.80 + (variation.next() * 0.22)))
        .max(crown.radius * 0.10);
    // Old wood carries years of its own weight; this year's growth sweeps up.
    let sag = BRANCH_SAG * (1.0 - (height * 0.62));
    let center =
        crown.base + (crown.up * ((crown.length * height) + (variation.signed() * spacing * 0.3)));

    for branch in 0..crown.branches {
        let around = usize_as_f32(branch) / usize_as_f32(crown.branches);
        let turn =
            crown.turn + (usize_as_f32(whorl) * WHORL_TURN) + around + (variation.signed() * 0.06);
        let (azimuth_sine, azimuth_cosine) = libm::sincosf(turn * std::f32::consts::TAU);
        let radial = (crown.tangent * azimuth_cosine) + (crown.bitangent * azimuth_sine);
        // No two branches of a whorl leave the trunk at the same height or carry
        // the same weight, so a ring is never the flat disc a lathe would give.
        let root = center + (crown.up * (variation.signed() * spacing * 0.8));
        let branch_reach = reach * (0.84 + (variation.next() * 0.22));
        append_bunch(
            geometry,
            crown,
            variation,
            &Bunch {
                root,
                radial,
                reach: branch_reach,
                sag,
                height,
            },
        )?;
    }
    Ok(())
}

/// Appends the leader: the shoot at the very top, which is what makes a conifer
/// a spire rather than a cone with the point cut off.
fn append_leader(
    geometry: &mut TreeGeometry,
    crown: &Crown,
    variation: &mut Variation,
    apex: Vec3,
) -> Result<(), RendererError> {
    let radius = crown.radius * crown.span * 0.55;
    append_cluster(
        geometry,
        &Cluster {
            center: apex - (crown.up * radius * CLUSTER_REACH),
            radius,
            color: foliage_tone(crown.foliage, variation.signed()),
            exposure: 1.0,
            height: 1.0,
            seed: variation.seed(),
            shells: crown.shells,
        },
    )
}

/// How much of its reach a whorl gives up to the crown above it.
///
/// A conifer in a stand loses its lowest branches to the shade of its own crown,
/// so the widest whorls sit a little above the bottom rather than at it. Without
/// this a crown is a plain cone standing on its widest ring, which is the shape
/// nothing in a forest actually has.
fn shaded_out(height: f32) -> f32 {
    (0.62 + (height * 3.2)).min(1.0)
}

/// A frame perpendicular to a crown's axis, for placing branches around it.
fn crown_frame(axis: Vec3) -> (Vec3, Vec3) {
    let reference = if axis.y.abs() < 0.92 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = axis.cross(reference).normalize_or_zero();
    (tangent, axis.cross(tangent).normalize_or_zero())
}
