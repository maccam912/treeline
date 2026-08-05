//! One branch's worth of foliage: the bunch of balls strung along it.
//!
//! This is where the crown stops being a shape and starts being a plant. A
//! branch does not carry an even coat of needles — it carries shoots, clustered
//! toward the outer end where the light is and thinning to bare wood against the
//! trunk. Drawing that literally is what makes a conifer read as a bunch of
//! grapes rather than a cone with a texture on it.

use glam::Vec3;

use crate::RendererError;
use crate::tree_mesh::cluster::{CLUSTER_REACH, Cluster, append_cluster};
use crate::tree_mesh::color::foliage_tone;
use crate::tree_mesh::conifer::Crown;
use crate::tree_mesh::conifer::variation::Variation;
use crate::tree_mesh::geometry::TreeGeometry;
use crate::vertex::usize_as_f32;

/// How far out a branch its foliage starts. The inner stretch is bare wood.
const BUNCH_START: f32 = 0.34;
/// How much smaller the innermost ball of a bunch is than the one at the tip.
const BUNCH_TAPER: f32 = 0.55;

/// One branch, as the bunch hanging off it sees it.
pub(super) struct Bunch {
    /// Where the branch leaves the trunk.
    pub(super) root: Vec3,
    /// Which way it heads, perpendicular to the crown axis.
    pub(super) radial: Vec3,
    /// How far it reaches before its foliage ends.
    pub(super) reach: f32,
    /// How far its tip drops, as a fraction of that reach.
    pub(super) sag: f32,
    /// Where up the crown it sits.
    pub(super) height: f32,
}

/// Appends the balls strung along one branch, biggest at the tip.
///
/// Every ball is pulled in from where it would sit by how far it reaches, so a
/// bunch ends at the branch tip rather than spilling past the crown.
pub(super) fn append_bunch(
    geometry: &mut TreeGeometry,
    crown: &Crown,
    variation: &mut Variation,
    bunch: &Bunch,
) -> Result<(), RendererError> {
    let tip_radius = bunch.reach * crown.span;
    let sideways = crown.up.cross(bunch.radial).normalize_or(crown.tangent);
    for step in 0..crown.clusters {
        let along = (usize_as_f32(step) + 1.0) / usize_as_f32(crown.clusters);
        let radius = tip_radius * (BUNCH_TAPER + ((1.0 - BUNCH_TAPER) * along));
        let distance = ((bunch.reach - (radius * CLUSTER_REACH)) * along).max(0.0);
        // Weight hangs from the branch, not from the crown's axis, so a leaning
        // tree's bunches still droop toward the ground.
        let drop = distance * bunch.sag * (0.7 + (variation.next() * 0.5));
        let center = bunch.root + (bunch.radial * distance) - (Vec3::Y * drop)
            + (sideways * (variation.signed() * radius * 0.6))
            + (crown.up * (variation.signed() * radius * 0.7));
        append_cluster(
            geometry,
            &Cluster {
                center,
                radius,
                color: foliage_tone(crown.foliage, variation.signed()),
                // A ball out at the tip stands in the open; one back against the
                // trunk sits in the shade of the whole crown above it.
                exposure: ((BUNCH_START + (along * 0.66)) * (0.82 + (bunch.height * 0.18)))
                    .clamp(0.0, 1.0),
                height: bunch.height,
                seed: variation.seed(),
                shells: crown.shells,
            },
        )?;
    }
    Ok(())
}
