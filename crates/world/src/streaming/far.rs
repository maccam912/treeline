//! Distant terrain: one coarse height surface per 2 km tile.
//!
//! Far tiles exist so the horizon appears before near detail finishes. They are
//! a surface, not a volume, and they sample the same terrain field as near
//! chunks, so the two representations stay spatially aligned.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use treeline_coordinates::WorldPosition;
use treeline_mesher::{Mesh, MeshingError, SurfaceGridSpec, surface_grid};
use treeline_terrain::SurfaceField;
use treeline_voxel::ChunkIndex;

/// Stable identity of one far-terrain tile.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FarTileIndex {
    pub x: i64,
    pub z: i64,
}

impl FarTileIndex {
    /// A far tile spans sixty-four near chunks, or 2,048 meters.
    pub const CHUNKS_PER_EDGE: i64 = 64;
    /// Sixty-four-meter samples keep terrain silhouettes at vista distance.
    pub const CELLS_PER_EDGE: usize = 32;

    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    pub fn edge_meters() -> f64 {
        ChunkIndex::edge_meters() * i64_as_f64(Self::CHUNKS_PER_EDGE)
    }

    pub fn containing(position: WorldPosition) -> Option<Self> {
        let chunk = ChunkIndex::containing(position)?;
        Some(Self::new(
            chunk.x.div_euclid(Self::CHUNKS_PER_EDGE),
            chunk.z.div_euclid(Self::CHUNKS_PER_EDGE),
        ))
    }

    pub fn chebyshev_distance(self, other: Self) -> u64 {
        self.x.abs_diff(other.x).max(self.z.abs_diff(other.z))
    }

    fn origin(self) -> [f64; 2] {
        let edge = Self::edge_meters();
        [i64_as_f64(self.x) * edge, i64_as_f64(self.z) * edge]
    }
}

/// Everything needed to regenerate one far tile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FarTerrainMeshSpec {
    pub tile: FarTileIndex,
}

impl FarTerrainMeshSpec {
    pub(crate) fn surface_grid(self) -> SurfaceGridSpec {
        let origin = self.tile.origin();
        SurfaceGridSpec::new(
            origin[0],
            origin[1],
            [FarTileIndex::CELLS_PER_EDGE; 2],
            FarTileIndex::edge_meters() / usize_as_f64(FarTileIndex::CELLS_PER_EDGE),
        )
    }
}

/// Meshes one far tile as a height surface.
///
/// # Errors
///
/// Returns [`MeshingError`] when a surface sample is unavailable or the mesh
/// exceeds index capacity.
pub fn far_terrain_mesh(
    field: &impl SurfaceField,
    spec: FarTerrainMeshSpec,
) -> Result<Mesh, MeshingError> {
    surface_grid(field, spec.surface_grid())
}

/// The chunk rectangle that near terrain fully covers.
///
/// Far tiles cut this rectangle out of themselves so the two tiers do not
/// draw the same ground twice.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NearTerrainCutout {
    pub min: ChunkIndex,
    pub max_exclusive: ChunkIndex,
}

impl NearTerrainCutout {
    pub const fn new(min: ChunkIndex, max_exclusive: ChunkIndex) -> Option<Self> {
        if min.x >= max_exclusive.x || min.z >= max_exclusive.z {
            return None;
        }
        Some(Self { min, max_exclusive })
    }

    pub fn around(center: ChunkIndex, radius: u64) -> Option<Self> {
        let radius = i64::try_from(radius).ok()?;
        Some(Self {
            min: ChunkIndex::new(center.x.checked_sub(radius)?, center.z.checked_sub(radius)?),
            max_exclusive: ChunkIndex::new(
                center.x.checked_add(radius)?.checked_add(1)?,
                center.z.checked_add(radius)?.checked_add(1)?,
            ),
        })
    }
}

/// Far-terrain residency radii, in tiles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FarTerrainStreamingConfig {
    load_radius: u64,
    retain_radius: u64,
}

impl FarTerrainStreamingConfig {
    /// Rejects a retention radius inside the load radius, which would thrash.
    pub const fn new(load_radius: u64, retain_radius: u64) -> Option<Self> {
        if retain_radius < load_radius {
            return None;
        }
        Some(Self {
            load_radius,
            retain_radius,
        })
    }

    pub const fn load_radius(self) -> u64 {
        self.load_radius
    }
}

impl Default for FarTerrainStreamingConfig {
    fn default() -> Self {
        Self::new(10, 11).expect("the default far-terrain radii are valid")
    }
}

/// Which far tiles to build and which to drop.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FarTerrainStreamingPlan {
    pub center: FarTileIndex,
    pub load: Vec<FarTerrainMeshSpec>,
    pub unload: Vec<FarTileIndex>,
}

/// Plans far-terrain residency independently of near chunks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FarTerrainStreamer {
    config: FarTerrainStreamingConfig,
}

impl FarTerrainStreamer {
    pub const fn new(config: FarTerrainStreamingConfig) -> Self {
        Self { config }
    }

    pub const fn config(self) -> FarTerrainStreamingConfig {
        self.config
    }

    /// Plans the horizon first, so a broad landscape appears quickly.
    ///
    /// Returns `None` only when the player cannot map to the tile lattice.
    pub fn plan(
        self,
        player_position: WorldPosition,
        loaded: &BTreeMap<FarTileIndex, FarTerrainMeshSpec>,
    ) -> Option<FarTerrainStreamingPlan> {
        let center = FarTileIndex::containing(player_position)?;
        let load_radius = i64::try_from(self.config.load_radius).ok()?;
        let mut desired = BTreeMap::new();
        for z_offset in -load_radius..=load_radius {
            for x_offset in -load_radius..=load_radius {
                let tile = FarTileIndex::new(
                    center.x.checked_add(x_offset)?,
                    center.z.checked_add(z_offset)?,
                );
                desired.insert(tile, FarTerrainMeshSpec { tile });
            }
        }
        // A hysteresis band keeps recently passed tiles resident, so stepping
        // back and forth across a tile edge does not rebuild them.
        for &tile in loaded.keys() {
            if tile.chebyshev_distance(center) <= self.config.retain_radius {
                desired.entry(tile).or_insert(FarTerrainMeshSpec { tile });
            }
        }

        let mut load = desired
            .values()
            .copied()
            .filter(|spec| loaded.get(&spec.tile) != Some(spec))
            .collect::<Vec<_>>();
        load.sort_by_key(|spec| {
            (
                Reverse(spec.tile.chebyshev_distance(center)),
                spec.tile.z,
                spec.tile.x,
            )
        });

        Some(FarTerrainStreamingPlan {
            center,
            load,
            unload: loaded
                .keys()
                .copied()
                .filter(|tile| !desired.contains_key(tile))
                .collect(),
        })
    }
}

#[allow(clippy::cast_precision_loss)]
fn i64_as_f64(value: i64) -> f64 {
    value as f64
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streamer(load: u64, retain: u64) -> FarTerrainStreamer {
        FarTerrainStreamer::new(FarTerrainStreamingConfig::new(load, retain).expect("valid radii"))
    }

    fn plan(loaded: &[FarTileIndex]) -> FarTerrainStreamingPlan {
        let loaded = loaded
            .iter()
            .map(|&tile| (tile, FarTerrainMeshSpec { tile }))
            .collect();
        streamer(1, 2)
            .plan(WorldPosition::new(0.0, 0.0, 0.0), &loaded)
            .expect("origin is inside tile range")
    }

    #[test]
    fn a_retention_radius_inside_the_load_radius_is_rejected() {
        assert_eq!(FarTerrainStreamingConfig::new(4, 3), None);
        assert!(FarTerrainStreamingConfig::new(4, 4).is_some());
    }

    #[test]
    fn tiles_align_with_chunks_and_the_origin() {
        assert_eq!(
            FarTileIndex::edge_meters().to_bits(),
            (ChunkIndex::edge_meters() * 64.0).to_bits()
        );
        assert_eq!(
            FarTileIndex::containing(WorldPosition::new(0.0, 0.0, 0.0)),
            Some(FarTileIndex::new(0, 0))
        );
    }

    #[test]
    fn negative_coordinates_align_downward() {
        let edge = FarTileIndex::edge_meters();
        assert_eq!(
            FarTileIndex::containing(WorldPosition::new(-1.0, 0.0, -1.0)),
            Some(FarTileIndex::new(-1, -1))
        );
        assert_eq!(
            FarTileIndex::containing(WorldPosition::new(-edge, 0.0, -edge)),
            Some(FarTileIndex::new(-1, -1))
        );
    }

    #[test]
    fn an_empty_world_loads_the_whole_disc_horizon_first() {
        let plan = plan(&[]);
        assert_eq!(plan.load.len(), 9);
        assert!(plan.unload.is_empty());

        let first = plan.load[0].tile.chebyshev_distance(plan.center);
        let last = plan.load[plan.load.len() - 1]
            .tile
            .chebyshev_distance(plan.center);
        assert!(first > last, "the horizon must be queued before the center");
    }

    #[test]
    fn tiles_inside_the_retention_band_stay_resident() {
        let plan = plan(&[FarTileIndex::new(2, 0)]);
        assert!(plan.unload.is_empty());
    }

    #[test]
    fn tiles_beyond_the_retention_band_are_dropped() {
        let plan = plan(&[FarTileIndex::new(3, 0)]);
        assert_eq!(plan.unload, vec![FarTileIndex::new(3, 0)]);
    }

    #[test]
    fn planning_is_idempotent() {
        let loaded = plan(&[])
            .load
            .into_iter()
            .map(|spec| (spec.tile, spec))
            .collect();
        let settled = streamer(1, 2)
            .plan(WorldPosition::new(0.0, 0.0, 0.0), &loaded)
            .expect("origin is inside tile range");

        assert!(settled.load.is_empty());
        assert!(settled.unload.is_empty());
    }

    #[test]
    fn cutouts_reject_empty_rectangles() {
        assert_eq!(
            NearTerrainCutout::new(ChunkIndex::new(0, 0), ChunkIndex::new(0, 1)),
            None
        );
        let cutout = NearTerrainCutout::around(ChunkIndex::new(0, 0), 2).expect("valid cutout");
        assert_eq!(cutout.min, ChunkIndex::new(-2, -2));
        assert_eq!(cutout.max_exclusive, ChunkIndex::new(3, 3));
    }
}
