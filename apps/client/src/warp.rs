//! Jumping to somewhere else in the tile.
//!
//! Warping exists to make the whole surveyed square reachable while there is no
//! travel gameplay yet. Both warps land on dry, walkable ground, because the
//! alternative is dropping the player into a lake or a cliff face.

use treeline_terrain::SURVEYED_TILE_EDGE_METERS;
use treeline_world::WorldTerrain;

/// Keeps warp destinations clear of the tile's clamped outer edge.
const BORDER_CLEARANCE_METERS: f64 = 64.0;
/// How many random positions to try before giving up on finding dry ground.
const SITE_ATTEMPTS: usize = 64;

/// Directions probed outward from open water when looking for a shore.
const SHORE_DIRECTIONS: u32 = 16;
/// How far a shore search walks before abandoning a direction.
const MAX_SHORE_DISTANCE_METERS: f64 = 2_000.0;
/// How far back from the waterline the player is placed.
const SHORE_CLEARANCE_METERS: f64 = 8.0;
/// How close the refined waterline must be before a shore counts as found.
const SHORE_PRECISION_METERS: f64 = 32.0;

/// Interior of Upper Holmes Lake, the largest mapped waterbody in the tile.
const LAKE_INTERIOR: [f64; 2] = [7_364.0, 6_894.0];

/// A place to stand, and optionally something to look at from there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpSite {
    pub destination: [f64; 2],
    pub face: Option<[f64; 2]>,
}

/// Finds dry ground somewhere in the tile.
///
/// Returns `None` when repeated attempts all land in water, which would mean
/// the tile has almost no dry ground left.
pub fn random_site(terrain: &WorldTerrain, random: impl FnMut() -> f64) -> Option<WarpSite> {
    let mut random = random;
    for _ in 0..SITE_ATTEMPTS {
        let destination = position_in_tile(random(), random());
        if terrain.has_dry_ground_at(destination[0], destination[1]) {
            return Some(WarpSite {
                destination,
                face: None,
            });
        }
    }
    None
}

/// Finds a lake shore to stand on, looking out over the water.
///
/// Returns `None` when no reachable shore is found around the mapped lake.
pub fn lake_shore_site(terrain: &WorldTerrain, direction_fraction: f64) -> Option<WarpSite> {
    let (destination, water) = dry_shore(terrain, LAKE_INTERIOR, direction_fraction)?;
    Some(WarpSite {
        destination,
        face: Some(water),
    })
}

/// Maps two unit fractions onto a position inside the tile's usable interior.
fn position_in_tile(x_fraction: f64, z_fraction: f64) -> [f64; 2] {
    let usable_edge = SURVEYED_TILE_EDGE_METERS - (BORDER_CLEARANCE_METERS * 2.0);
    [x_fraction, z_fraction]
        .map(|fraction| BORDER_CLEARANCE_METERS + (fraction.clamp(0.0, 1.0) * usable_edge))
}

/// Walks outward from open water until it reaches land.
///
/// Each direction doubles its stride while still over water, then hands the
/// bracketing pair to a refinement step. Returns the standing position and the
/// last water position, which is what the camera turns to face.
fn dry_shore(
    terrain: &WorldTerrain,
    water: [f64; 2],
    direction_fraction: f64,
) -> Option<([f64; 2], [f64; 2])> {
    for index in 0..SHORE_DIRECTIONS {
        let fraction =
            (direction_fraction + (f64::from(index) / f64::from(SHORE_DIRECTIONS))).fract();
        let angle = fraction * std::f64::consts::TAU;
        let direction = [libm::cos(angle), libm::sin(angle)];

        let mut water_side = water;
        let mut distance = 8.0;
        while distance <= MAX_SHORE_DISTANCE_METERS {
            let candidate = [
                water[0] + (direction[0] * distance),
                water[1] + (direction[1] * distance),
            ];
            if terrain.lake_at(candidate[0], candidate[1]).is_some() {
                water_side = candidate;
                distance *= 2.0;
                continue;
            }
            if terrain.has_dry_ground_at(candidate[0], candidate[1]) {
                return refine_shore(terrain, water_side, candidate, direction);
            }
            break;
        }
    }
    None
}

/// Bisects between a known water point and a known dry point to find the waterline.
fn refine_shore(
    terrain: &WorldTerrain,
    mut water_side: [f64; 2],
    mut dry_side: [f64; 2],
    direction: [f64; 2],
) -> Option<([f64; 2], [f64; 2])> {
    for _ in 0..16 {
        let midpoint = [
            (water_side[0] + dry_side[0]) * 0.5,
            (water_side[1] + dry_side[1]) * 0.5,
        ];
        if terrain.lake_at(midpoint[0], midpoint[1]).is_some() {
            water_side = midpoint;
        } else if terrain.has_dry_ground_at(midpoint[0], midpoint[1]) {
            dry_side = midpoint;
        } else {
            break;
        }
    }
    // A wide gap means the bisection never converged on a real waterline —
    // usually a cliff between the two samples rather than a shore.
    if (dry_side[0] - water_side[0]).hypot(dry_side[1] - water_side[1]) > SHORE_PRECISION_METERS {
        return None;
    }

    let standing = [
        dry_side[0] + (direction[0] * SHORE_CLEARANCE_METERS),
        dry_side[1] + (direction[1] * SHORE_CLEARANCE_METERS),
    ];
    if terrain.has_dry_ground_at(standing[0], standing[1]) {
        return Some((standing, water_side));
    }
    terrain
        .has_dry_ground_at(dry_side[0], dry_side[1])
        .then_some((dry_side, water_side))
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_world::DEFAULT_WORLD_IDENTITY;

    const TERRAIN: WorldTerrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);

    /// A deterministic stand-in for the runtime's entropy source.
    fn sequence(values: Vec<f64>) -> impl FnMut() -> f64 {
        let mut index = 0;
        move || {
            let value = values[index % values.len()];
            index += 1;
            value
        }
    }

    #[test]
    fn random_warps_stay_inside_the_tile_and_land_on_dry_ground() {
        let site = random_site(&TERRAIN, sequence(vec![0.31, 0.72, 0.18, 0.44, 0.86, 0.09]))
            .expect("the tile has dry ground");
        for coordinate in site.destination {
            assert!(coordinate >= BORDER_CLEARANCE_METERS);
            assert!(coordinate <= SURVEYED_TILE_EDGE_METERS - BORDER_CLEARANCE_METERS);
        }
        assert!(TERRAIN.has_dry_ground_at(site.destination[0], site.destination[1]));
    }

    #[test]
    fn a_random_warp_is_repeatable_for_the_same_draws() {
        let draws = vec![0.31, 0.72, 0.18, 0.44];
        assert_eq!(
            random_site(&TERRAIN, sequence(draws.clone())),
            random_site(&TERRAIN, sequence(draws))
        );
    }

    #[test]
    fn the_tile_corners_are_reachable() {
        let far_edge = SURVEYED_TILE_EDGE_METERS - BORDER_CLEARANCE_METERS;
        let close = |actual: [f64; 2], expected: [f64; 2]| {
            actual
                .into_iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-9)
        };

        assert!(close(
            position_in_tile(0.0, 0.0),
            [BORDER_CLEARANCE_METERS; 2]
        ));
        assert!(close(position_in_tile(1.0, 1.0), [far_edge; 2]));
        // Fractions outside the unit range clamp rather than leaving the tile.
        assert!(close(
            position_in_tile(-5.0, 9.0),
            [BORDER_CLEARANCE_METERS, far_edge]
        ));
    }

    #[test]
    fn a_lake_warp_lands_on_dry_ground_facing_water() {
        let site = lake_shore_site(&TERRAIN, 0.25).expect("Upper Holmes Lake has a shore");
        let water = site.face.expect("a lake warp faces water");

        assert!(TERRAIN.has_dry_ground_at(site.destination[0], site.destination[1]));
        assert!(TERRAIN.lake_at(water[0], water[1]).is_some());
    }

    #[test]
    fn a_lake_warp_stands_close_to_the_waterline() {
        let site = lake_shore_site(&TERRAIN, 0.25).expect("Upper Holmes Lake has a shore");
        let water = site.face.expect("a lake warp faces water");
        let distance = (site.destination[0] - water[0]).hypot(site.destination[1] - water[1]);

        assert!(distance <= SHORE_PRECISION_METERS + SHORE_CLEARANCE_METERS);
    }

    #[test]
    fn every_search_direction_finds_the_same_lake() {
        for step in 0..8 {
            let site = lake_shore_site(&TERRAIN, f64::from(step) / 8.0)
                .expect("Upper Holmes Lake has a shore in every direction");
            let water = site.face.expect("a lake warp faces water");
            assert_eq!(
                TERRAIN.lake_at(water[0], water[1]).map(|lake| lake.id),
                Some(19)
            );
        }
    }
}
