//! Stable identities and spatial primitives shared by deterministic generation.

const I64_MIN_AS_F64: f64 = -9_223_372_036_854_775_808.0;
const I64_MAX_EXCLUSIVE_AS_F64: f64 = 9_223_372_036_854_775_808.0;

/// Identifies a procedural world and the algorithm contract used to generate it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorldIdentity {
    /// User- or server-selected world seed.
    pub seed: u64,
    /// Version of generation algorithms and their deterministic outputs.
    pub generator_version: u32,
    /// Hash of generation settings that affect pristine terrain.
    pub settings_hash: u64,
}

impl WorldIdentity {
    /// Creates a world identity with explicit versioned inputs.
    pub const fn new(seed: u64, generator_version: u32, settings_hash: u64) -> Self {
        Self {
            seed,
            generator_version,
            settings_hash,
        }
    }

    /// Returns a stable key suitable for seeding a specific generation domain.
    pub fn domain_key(self, domain: u64) -> u64 {
        stable_hash(&[
            self.seed,
            u64::from(self.generator_version),
            self.settings_hash,
            domain,
        ])
    }
}

/// A position in meters in the effectively infinite world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl WorldPosition {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

/// Integer coordinates of a cell in a hierarchical spatial grid.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CellIndex {
    pub x: i64,
    pub z: i64,
    pub level: u8,
}

impl CellIndex {
    pub const fn new(x: i64, z: i64, level: u8) -> Self {
        Self { x, z, level }
    }

    /// Resolves a world-space horizontal position into a cell.
    ///
    /// `base_edge_meters` is the edge length at level zero. Every successive
    /// level doubles it.
    pub fn containing(x: f64, z: f64, level: u8, base_edge_meters: f64) -> Option<Self> {
        if !x.is_finite()
            || !z.is_finite()
            || !base_edge_meters.is_finite()
            || base_edge_meters <= 0.0
        {
            return None;
        }

        let edge = base_edge_meters * 2.0_f64.powi(i32::from(level));
        let cell_x = (x / edge).floor();
        let cell_z = (z / edge).floor();
        let valid_index_range = I64_MIN_AS_F64..I64_MAX_EXCLUSIVE_AS_F64;
        if !valid_index_range.contains(&cell_x) || !valid_index_range.contains(&cell_z) {
            return None;
        }

        #[allow(clippy::cast_possible_truncation)]
        Some(Self::new(cell_x as i64, cell_z as i64, level))
    }

    /// Produces a stable key for this cell within a named generation domain.
    pub fn generation_key(self, world: WorldIdentity, domain: u64) -> u64 {
        stable_hash(&[
            world.domain_key(domain),
            zigzag(self.x),
            zigzag(self.z),
            u64::from(self.level),
        ])
    }
}

/// A small, specified hash used only for deterministic generation.
///
/// This deliberately avoids process-randomized standard-library hashers. Its
/// output is part of the generator contract and must not change accidentally.
pub fn stable_hash(words: &[u64]) -> u64 {
    words.iter().fold(0x6a09_e667_f3bc_c909, |state, word| {
        splitmix64(state ^ splitmix64(*word))
    })
}

const fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn zigzag(value: i64) -> u64 {
    value
        .unsigned_abs()
        .wrapping_mul(2)
        .wrapping_sub(u64::from(value.is_negative()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_positions_floor_into_the_previous_cell() {
        assert_eq!(
            CellIndex::containing(-0.01, -128.0, 0, 128.0),
            Some(CellIndex::new(-1, -1, 0))
        );
    }

    #[test]
    fn cell_boundaries_are_half_open() {
        assert_eq!(
            CellIndex::containing(128.0, 255.99, 0, 128.0),
            Some(CellIndex::new(1, 1, 0))
        );
    }

    #[test]
    fn invalid_coordinates_are_rejected() {
        assert!(CellIndex::containing(f64::NAN, 0.0, 0, 1.0).is_none());
        assert!(CellIndex::containing(0.0, 0.0, 0, 0.0).is_none());
    }

    #[test]
    fn stable_hash_has_a_golden_value() {
        assert_eq!(
            stable_hash(&[1, 2, 3, 4]),
            0x2268_a524_e18c_9723,
            "changing this value changes generated worlds"
        );
    }

    #[test]
    fn world_version_is_part_of_the_key() {
        let old = WorldIdentity::new(42, 1, 0);
        let new = WorldIdentity::new(42, 2, 0);
        let cell = CellIndex::new(4, -7, 3);
        assert_ne!(cell.generation_key(old, 12), cell.generation_key(new, 12));
    }
}
