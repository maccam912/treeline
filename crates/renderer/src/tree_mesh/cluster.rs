//! The shoot cluster a conifer crown is massed from.
//!
//! Conifer foliage is not a surface. It is thousands of shoots, each a
//! fist-sized ball of needles, hung along branches in bunches — closer to a
//! bunch of grapes than to a cone. Drawing every needle is out of reach, but the
//! unit is not: one cluster here is one ball, and a crown is a few hundred of
//! them overlapping.
//!
//! A ball is drawn as nested shells rather than as one solid. The innermost is
//! the woody shoot; each one outside it stands where the needles growing off
//! that shoot reach, and the shader keeps only the slivers of a shell that a
//! needle actually occupies at that distance out. Stacked, they give a shoot
//! real depth: the eye sees between the needles, past the ones in front, to the
//! shaded ones behind.
//!
//! Two details do most of the rest. A vertex takes the ball's *undeformed*
//! outward direction as its normal, so an eight-triangle shell still shades as a
//! smooth round mass, and every shell along one ray shares that direction —
//! which is what lets the shader lay needles that stay put from one shell to the
//! next instead of swimming. And every ball is turned by a frame of its own, so
//! no two show the same profile and the union of them has an outline nothing
//! lathed could have.

use glam::Vec3;

use crate::RendererError;
use crate::tree_mesh::geometry::TreeGeometry;
use crate::vertex::{foliage_vertex, hash_fraction, usize_as_f32};

/// How far a ball's corners wander from its nominal radius, as a fraction.
const CLUSTER_DEFORM: f32 = 0.34;

/// How much of a ball is needles rather than shoot, as a fraction of its radius.
///
/// This is the depth the shells are spread through, and so the length of a
/// needle. It trades off against the shell count: what is left over is a solid
/// eight-sided core that the needles have to bury, but spread too few shells
/// too deep and each needle comes apart into the flakes its shells cut it into.
const NEEDLE_DEPTH: f32 = 0.82;

/// How the shells are spread through that depth.
///
/// Not evenly. A needle narrows on the way out, so the shells have to close up
/// on the way out too — a gap wider than the needle crossing it is a gap that
/// can be seen through, and a needle seen through in four places is four flakes.
/// Above one this bunches the shells toward the tips, which is where the needles
/// are thin and the gaps would show. Only a little, though: bunching the outside
/// thins the inside, and the gap the core shows through is the one that matters
/// most, because what is behind that one is a solid eight-sided block.
const SHELL_BUNCHING: f32 = 1.25;

/// The most a deformed corner can reach past the nominal radius: the hard bound
/// a crown sizes its margins against.
#[cfg(test)]
const CLUSTER_BULGE: f32 = 1.0 + (CLUSTER_DEFORM * 0.5);

/// How far a ball reaches in a direction that is not one of its corners.
///
/// A ball's six corners are the only points at its full radius; every direction
/// between them cuts inside. Seen from anywhere, the widest corner still on the
/// outline is at least `sqrt(2/3)` of the radius out and at most all of it, so a
/// ball looks about nine tenths the size it is declared.
///
/// Crowns place balls by this rather than by [`CLUSTER_BULGE`]: packing to the
/// worst case leaves the envelope visibly under-filled, and the corner that does
/// poke past is one corner of one ball.
pub(crate) const CLUSTER_REACH: f32 = 0.9;

/// One ball of needles: where it sits, how big it is, and how it is shaded.
pub(crate) struct Cluster {
    pub(crate) center: Vec3,
    pub(crate) radius: f32,
    pub(crate) color: [f32; 4],
    /// 0 deep in the crown's shade, 1 at an open branch tip.
    pub(crate) exposure: f32,
    /// Where up the crown the ball hangs, from 0 at the base to 1 at the apex.
    pub(crate) height: f32,
    /// Which ball this is, so its turn and its lumps are its own.
    pub(crate) seed: u64,
    /// How many nested shells to draw it as, counting the solid core.
    ///
    /// Two is the floor: a shoot needs an inside and an outside to have any
    /// depth at all.
    pub(crate) shells: usize,
}

/// Where one shell of a ball stands in the stack.
///
/// The hull is the outermost, and it encloses every shell inside it, so it is
/// the only one that has anything to say about the ball's outline — to the sun,
/// or to anything else that only wants to know how far the ball reaches.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Shell {
    Hull,
    Interior,
}

/// Appends one ball of needles to `geometry`.
///
/// Shells are emitted outermost first, so the hull is laid down before the
/// shells it covers.
pub(crate) fn append_cluster(
    geometry: &mut TreeGeometry,
    cluster: &Cluster,
) -> Result<(), RendererError> {
    if cluster.radius <= f32::EPSILON {
        return Ok(());
    }
    let frame = cluster_frame(cluster.seed);
    let shells = cluster.shells.max(2);
    for shell in (0..shells).rev() {
        let spread = usize_as_f32(shell) / usize_as_f32(shells - 1);
        let depth = 1.0 - libm::powf(1.0 - spread, SHELL_BUNCHING);
        let position = if shell == shells - 1 {
            Shell::Hull
        } else {
            Shell::Interior
        };
        append_shell(geometry, cluster, frame, depth, position)?;
    }
    Ok(())
}

/// Appends one shell of a ball, `depth` of the way out through its needles.
fn append_shell(
    geometry: &mut TreeGeometry,
    cluster: &Cluster,
    frame: [Vec3; 3],
    depth: f32,
    position: Shell,
) -> Result<(), RendererError> {
    // Which needles this shoot wears rather than the one beside it. Every shell
    // of a ball carries the same one, so its needles line up through the stack.
    let needle_seed = hash_fraction(cluster.seed, 3);
    let base_index = geometry.base_index()?;
    for (corner, lane) in CORNERS.into_iter().zip(0_u64..) {
        let direction = (frame[0] * corner.x) + (frame[1] * corner.y) + (frame[2] * corner.z);
        // Deformation is radial and always outward, so a lumpy ball stays
        // star-shaped about its center and its faces stay wound outward. It
        // scales the whole stack rather than one shell, so the shells stay
        // nested and the outermost keeps the silhouette a solid ball had.
        let reach = cluster.radius
            * ((1.0 - (CLUSTER_DEFORM * 0.5))
                + (hash_fraction(cluster.seed, lane + 8) * CLUSTER_DEFORM))
            * ((1.0 - NEEDLE_DEPTH) + (NEEDLE_DEPTH * depth));
        // Needles under a ball sit in the shade of the ones over them.
        let sunward = 0.55 + ((direction.y + 1.0) * 0.225);
        geometry.vertices.push(foliage_vertex(
            cluster.center + (direction * reach),
            direction,
            cluster.color,
            cluster.exposure * sunward,
            cluster.height,
            depth,
            needle_seed,
        ));
    }
    let indices = match position {
        Shell::Hull => &mut geometry.foliage_hull_indices,
        Shell::Interior => &mut geometry.foliage_interior_indices,
    };
    for face in FACES {
        indices.extend(face.map(|corner| base_index + corner));
    }
    Ok(())
}

/// The six corners of a ball, in its own frame.
const CORNERS: [Vec3; 6] = [
    Vec3::Y,
    Vec3::X,
    Vec3::Z,
    Vec3::NEG_X,
    Vec3::NEG_Z,
    Vec3::NEG_Y,
];

/// The eight faces over those corners, wound so every one turns outward.
const FACES: [[u32; 3]; 8] = [
    [0, 2, 1],
    [0, 3, 2],
    [0, 4, 3],
    [0, 1, 4],
    [5, 1, 2],
    [5, 2, 3],
    [5, 3, 4],
    [5, 4, 1],
];

/// A turned frame for one ball, as `[tangent, up, cotangent]`.
///
/// The turn is what keeps a crown from reading as a heap of identical diamonds.
/// It is right-handed, so the faces stay wound the way [`FACES`] declares them.
fn cluster_frame(seed: u64) -> [Vec3; 3] {
    let azimuth = hash_fraction(seed, 1) * std::f32::consts::TAU;
    let tilt = (hash_fraction(seed, 2) * 2.0) - 1.0;
    let ring = libm::sqrtf((1.0 - (tilt * tilt)).max(0.0));
    let (azimuth_sine, azimuth_cosine) = libm::sincosf(azimuth);
    let up = Vec3::new(ring * azimuth_cosine, tilt, ring * azimuth_sine).normalize_or(Vec3::Y);
    let reference = if up.y.abs() < 0.92 { Vec3::Y } else { Vec3::X };
    let tangent = up.cross(reference).normalize_or(Vec3::X);
    [tangent, up, tangent.cross(up)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHELLS: usize = 5;

    fn ball(seed: u64) -> TreeGeometry {
        let mut geometry = TreeGeometry::default();
        append_cluster(
            &mut geometry,
            &Cluster {
                center: Vec3::new(3.0, 11.0, -2.0),
                radius: 0.7,
                color: [0.06, 0.24, 0.12, 1.0],
                exposure: 0.8,
                height: 0.4,
                seed,
                shells: SHELLS,
            },
        )
        .expect("one ball fits u32 addressing");
        geometry
    }

    fn position(geometry: &TreeGeometry, index: u32) -> Vec3 {
        let index = usize::try_from(index).expect("an addressable vertex");
        let vertex = &geometry.vertices[index];
        Vec3::from(vertex.position_high) + Vec3::from(vertex.position_low)
    }

    /// A ball is opaque and back-face culled, so a face wound inward is a hole
    /// straight through the crown.
    #[test]
    fn every_face_of_a_ball_turns_away_from_its_center() {
        let center = Vec3::new(3.0, 11.0, -2.0);
        for seed in 0..64 {
            let geometry = ball(seed);
            let indices = geometry.all_indices().collect::<Vec<_>>();
            for triangle in indices.chunks_exact(3) {
                let corners = [0, 1, 2].map(|corner| position(&geometry, triangle[corner]));
                let winding = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
                let centroid = ((corners[0] + corners[1] + corners[2]) / 3.0) - center;
                assert!(
                    winding.dot(centroid) > 0.0,
                    "seed {seed} wound a face into the ball"
                );
                assert!(winding.length() > 1.0e-4, "seed {seed} drew a sliver");
            }
        }
    }

    /// The sun sees a ball through its hull alone, so the hull has to be one
    /// shell — the outermost — and every other shell has to sit inside it. Get
    /// this wrong and a crown's shadow shrinks to the shape of a shell that was
    /// never meant to be seen from outside.
    #[test]
    fn a_balls_hull_is_the_one_shell_that_encloses_the_rest() {
        let center = Vec3::new(3.0, 11.0, -2.0);
        for seed in 0..64 {
            let geometry = ball(seed);
            assert_eq!(geometry.foliage_hull_indices.len(), FACES.len() * 3);
            assert_eq!(
                geometry.foliage_interior_indices.len(),
                FACES.len() * 3 * (SHELLS - 1)
            );

            // Corners are pushed six at a time and the hull goes first, so one
            // corner of the hull is vertex `lane`, and the same corner of the
            // shell `shell` steps in is six vertices further along for each.
            let reach = |vertex: usize| {
                let vertex = u32::try_from(vertex).expect("one ball");
                (position(&geometry, vertex) - center).length()
            };
            for lane in 0..CORNERS.len() {
                for shell in 1..SHELLS {
                    assert!(
                        reach(lane) > reach((shell * CORNERS.len()) + lane),
                        "seed {seed} let shell {shell} out past the hull at corner {lane}"
                    );
                }
            }
        }
    }

    /// Shading a lumpy solid as a smooth one is the whole trick, and it only
    /// works if the normal ignores the lump the position took.
    #[test]
    fn a_balls_normals_stay_spherical_however_it_is_deformed() {
        let center = Vec3::new(3.0, 11.0, -2.0);
        let geometry = ball(7);
        for (index, vertex) in geometry.vertices.iter().enumerate() {
            let normal = Vec3::from(vertex.normal);
            assert!((normal.length() - 1.0).abs() < 1.0e-5);
            let offset = position(&geometry, u32::try_from(index).expect("six corners")) - center;
            // Same direction as the offset, but not the same length: the
            // position carries the deformation and the normal does not.
            assert!(normal.dot(offset.normalize()) > 0.9999);
        }
    }

    /// Callers butt balls up against the edge of a crown, so the bound they are
    /// promised has to hold for every seed — and shelling a ball must not have
    /// grown it, since the shells are spread through its radius, not past it.
    #[test]
    fn no_corner_reaches_past_the_declared_bulge() {
        let center = Vec3::new(3.0, 11.0, -2.0);
        for seed in 0..256 {
            let geometry = ball(seed);
            for index in 0..u32::try_from(geometry.vertices.len()).expect("one ball") {
                let reach = (position(&geometry, index) - center).length();
                assert!(reach <= 0.7 * CLUSTER_BULGE + 1.0e-5, "seed {seed} bulged");
            }
        }
    }

    /// The whole point of the stack: shells nest, closing up toward the needle
    /// tips, with the outermost holding the radius the ball declares. Shells
    /// that crossed would show needles growing inward; shells spread evenly
    /// would leave gaps out at the tips wider than the needles crossing them.
    #[test]
    fn shells_nest_and_close_up_toward_the_needle_tips() {
        let center = Vec3::new(3.0, 11.0, -2.0);
        let geometry = ball(19);
        assert_eq!(geometry.vertices.len(), 6 * SHELLS);

        // The first shell emitted is the outermost, so one corner's reach falls
        // through the stack, by a step that widens the further in it goes.
        let corner = |shell: usize| {
            let index = u32::try_from(shell * 6).expect("one ball");
            (position(&geometry, index) - center).length()
        };
        let steps: Vec<f32> = (1..SHELLS)
            .map(|shell| corner(shell - 1) - corner(shell))
            .collect();
        for (step, inner) in steps.iter().zip(&steps[1..]) {
            assert!(*step > 0.0, "a shell fell inside the one under it");
            assert!(inner > step, "the shells did not close up toward the tips");
        }
        let core = corner(SHELLS - 1);
        assert!((core - (corner(0) * (1.0 - NEEDLE_DEPTH))).abs() < 1.0e-5);
    }

    /// Depth is what tells the shader how far out through a shoot's needles a
    /// fragment sits, so it has to span the whole range whatever the count.
    #[test]
    fn every_shell_carries_its_own_depth() {
        let geometry = ball(3);
        let mut depths: Vec<f32> = geometry
            .vertices
            .iter()
            .map(|vertex| vertex.needle_depth)
            .collect();
        depths.sort_by(f32::total_cmp);
        depths.dedup_by(|left, right| (*left - *right).abs() < 1.0e-6);
        assert_eq!(depths.len(), SHELLS);
        assert!(depths[0].abs() < 1.0e-6);
        assert!((depths[SHELLS - 1] - 1.0).abs() < 1.0e-6);
    }

    /// A shoot with one shell has no inside and no outside, so it has no depth
    /// to shade. Asking for one gets the floor rather than a degenerate ball.
    #[test]
    fn a_ball_always_gets_an_inside_and_an_outside() {
        let mut geometry = TreeGeometry::default();
        append_cluster(
            &mut geometry,
            &Cluster {
                center: Vec3::ZERO,
                radius: 0.4,
                color: [0.0; 4],
                exposure: 0.0,
                height: 0.0,
                seed: 1,
                shells: 0,
            },
        )
        .expect("a floored ball");
        assert_eq!(geometry.vertices.len(), 12);
    }

    #[test]
    fn a_ball_with_no_radius_costs_nothing() {
        let mut geometry = TreeGeometry::default();
        append_cluster(
            &mut geometry,
            &Cluster {
                center: Vec3::ZERO,
                radius: 0.0,
                color: [0.0; 4],
                exposure: 0.0,
                height: 0.0,
                seed: 1,
                shells: 5,
            },
        )
        .expect("an empty ball");
        assert!(geometry.vertices.is_empty());
        assert_eq!(geometry.all_indices().count(), 0);
    }
}
