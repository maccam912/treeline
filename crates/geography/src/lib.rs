//! Deterministic, spatially continuous regional parameter fields.

use treeline_coordinates::{CellIndex, WorldIdentity};

const DOMAIN_UPLIFT: u64 = 0x5550_4c49_4654;
const DOMAIN_EROSION: u64 = 0x0045_524f_5349_4f4e;
const DOMAIN_ROCK: u64 = 0x524f_434b;
const DOMAIN_RAIN: u64 = 0x5241_494e;
const DOMAIN_TEMP: u64 = 0x5445_4d50;
const DOMAIN_KARST: u64 = 0x004b_4152_5354;

/// Coherent environmental parameters sampled at a horizontal world position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionalProfile {
    pub uplift: f64,
    pub erosion_age: f64,
    pub rock_hardness: f64,
    pub precipitation: f64,
    pub mean_temperature: f64,
    pub karst_probability: f64,
}

impl RegionalProfile {
    /// Samples correlated fields whose values remain in the inclusive range 0–1.
    pub fn sample(world: WorldIdentity, x: f64, z: f64) -> Option<Self> {
        const REGION_EDGE_METERS: f64 = 100_000.0;

        Some(Self {
            uplift: value_field(world, DOMAIN_UPLIFT, x, z, REGION_EDGE_METERS)?,
            erosion_age: value_field(world, DOMAIN_EROSION, x, z, REGION_EDGE_METERS)?,
            rock_hardness: value_field(world, DOMAIN_ROCK, x, z, REGION_EDGE_METERS)?,
            precipitation: value_field(world, DOMAIN_RAIN, x, z, REGION_EDGE_METERS)?,
            mean_temperature: value_field(world, DOMAIN_TEMP, x, z, REGION_EDGE_METERS)?,
            karst_probability: value_field(world, DOMAIN_KARST, x, z, REGION_EDGE_METERS)?,
        })
    }
}

fn value_field(world: WorldIdentity, domain: u64, x: f64, z: f64, edge: f64) -> Option<f64> {
    let cell = CellIndex::containing(x, z, 0, edge)?;
    let local_x = (x / edge) - index_as_f64(cell.x);
    let local_z = (z / edge) - index_as_f64(cell.z);
    let blend_x = smoothstep(local_x);
    let blend_z = smoothstep(local_z);

    let bottom_left = corner(world, domain, cell.x, cell.z);
    let bottom_right = corner(world, domain, cell.x + 1, cell.z);
    let top_left = corner(world, domain, cell.x, cell.z + 1);
    let top_right = corner(world, domain, cell.x + 1, cell.z + 1);

    let bottom = lerp(bottom_left, bottom_right, blend_x);
    let top = lerp(top_left, top_right, blend_x);
    Some(lerp(bottom, top, blend_z))
}

fn corner(world: WorldIdentity, domain: u64, x: i64, z: i64) -> f64 {
    let hash = CellIndex::new(x, z, 0).generation_key(world, domain);
    // Using the high 53 bits maps exactly into the precision available in f64.
    hash53_as_f64(hash >> 11) / 9_007_199_254_740_991.0
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: i64) -> f64 {
    index as f64
}

#[allow(clippy::cast_precision_loss)]
fn hash53_as_f64(hash: u64) -> f64 {
    hash as f64
}

fn smoothstep(value: f64) -> f64 {
    value * value * (3.0 - (2.0 * value))
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, 1, 0);

    #[test]
    fn samples_are_deterministic() {
        let first = RegionalProfile::sample(WORLD, 42_000.0, -17_000.0);
        let second = RegionalProfile::sample(WORLD, 42_000.0, -17_000.0);
        assert_eq!(first, second);
    }

    #[test]
    fn all_profile_values_are_normalized() {
        let profile = RegionalProfile::sample(WORLD, -42_000.0, 317_000.0)
            .expect("finite coordinates should sample");
        for value in [
            profile.uplift,
            profile.erosion_age,
            profile.rock_hardness,
            profile.precipitation,
            profile.mean_temperature,
            profile.karst_probability,
        ] {
            assert!((0.0..=1.0).contains(&value));
        }
    }

    #[test]
    fn a_shared_corner_has_one_value_from_every_neighbor() {
        let sampled =
            value_field(WORLD, DOMAIN_RAIN, 100_000.0, -200_000.0, 100_000.0).expect("finite");
        let shared_corner = corner(WORLD, DOMAIN_RAIN, 1, -2);
        assert!((sampled - shared_corner).abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_positions_do_not_generate_profiles() {
        assert!(RegionalProfile::sample(WORLD, f64::INFINITY, 0.0).is_none());
    }
}
