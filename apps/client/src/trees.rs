//! Streaming individual trees around the player.
//!
//! Trees stream on their own lattice rather than with terrain chunks, so
//! coarsening terrain never swaps a forest for a canopy surface. Tiles within
//! the simplified radius carry one batched draw of trunks and branch needle
//! masses; distant tiles keep only a silhouette. That keeps a forest visible to
//! the horizon at a workable vertex cost.

use std::collections::{BTreeMap, VecDeque};
use std::error::Error;

use bevy::prelude::{
    Assets, Commands, Entity, Mesh, Mesh3d, MeshMaterial3d, Name, Resource, Transform,
};
use treeline_coordinates::WorldPosition;
use treeline_ecology::TreeBounds;
use treeline_renderer::{TreeMeshDetail, WorldMaterials, WorldMeshOrigin, prepare_trees};
use treeline_terrain::SurfaceField;
use treeline_voxel::ChunkIndex;
use treeline_world::{ChunkStreamingConfig, WorldTerrain};

/// A tree tile spans four 32-meter chunks, or 128 meters.
const TILE_CHUNKS_PER_EDGE: u64 = 4;

/// Tree residency reach, as multiples of the near-terrain load radius.
///
/// Trees are visible far past the terrain the player can walk on, which is what
/// makes distance legible in a forest.
const RESIDENCY_MULTIPLIER: u64 = 20;
/// Tiles within this multiple of the chunk load radius carry branch geometry;
/// everything further keeps only a silhouette.
const SIMPLIFIED_MULTIPLIER: u64 = 10;

/// Stable identity of one tree tile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TreeTileIndex {
    x: i64,
    z: i64,
}

impl TreeTileIndex {
    pub fn containing(position: WorldPosition) -> Option<Self> {
        let chunk = ChunkIndex::containing(position)?;
        let per_edge = i64::try_from(TILE_CHUNKS_PER_EDGE).ok()?;
        Some(Self {
            x: chunk.x.div_euclid(per_edge),
            z: chunk.z.div_euclid(per_edge),
        })
    }

    pub fn chebyshev_distance(self, other: Self) -> u64 {
        self.x.abs_diff(other.x).max(self.z.abs_diff(other.z))
    }

    fn bounds(self) -> Option<TreeBounds> {
        let per_edge = i64::try_from(TILE_CHUNKS_PER_EDGE).ok()?;
        let chunk = ChunkIndex::new(self.x.checked_mul(per_edge)?, self.z.checked_mul(per_edge)?);
        let origin = chunk.sample_origin();
        let edge = ChunkIndex::edge_meters() * index_as_f64(per_edge);
        TreeBounds::new(origin.x, origin.z, origin.x + edge, origin.z + edge)
    }
}

/// One tree tile and the detail it was built at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TreeTileSpec {
    tile: TreeTileIndex,
    detail: TreeMeshDetail,
}

/// A built tree tile. Tiles over open ground hold no mesh at all.
#[derive(Debug)]
struct ResidentTreeTile {
    spec: TreeTileSpec,
    entity: Option<Entity>,
}

/// Every tree tile currently resident, plus the queue of tiles to build.
#[derive(Debug, Default, Resource)]
pub struct ResidentTrees {
    tiles: BTreeMap<TreeTileIndex, ResidentTreeTile>,
    pending: VecDeque<TreeTileSpec>,
}

impl ResidentTrees {
    pub fn clear(&mut self, commands: &mut Commands) {
        for resident in self.tiles.values() {
            if let Some(entity) = resident.entity {
                commands.entity(entity).despawn();
            }
        }
        self.tiles.clear();
        self.pending.clear();
    }

    /// Reconciles residency and builds at most one tile per frame.
    ///
    /// One tile per frame keeps a warp or a fast traversal from stalling the
    /// frame loop behind a burst of tree generation.
    ///
    /// # Errors
    ///
    /// Returns an error when the player leaves the representable range or a
    /// tree mesh cannot be uploaded.
    pub fn update(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        materials: &WorldMaterials,
        terrain: &WorldTerrain,
        config: ChunkStreamingConfig,
        player_position: WorldPosition,
    ) -> Result<(), Box<dyn Error>> {
        let center = TreeTileIndex::containing(player_position)
            .ok_or_else(|| std::io::Error::other("player position is outside tree tile range"))?;
        let desired = desired_tiles(center, config)?;
        let retain_radius = residency_radius(config).saturating_add(1);

        let removed = self
            .tiles
            .extract_if(.., |tile, _| {
                tile.chebyshev_distance(center) > retain_radius
            })
            .map(|(_, resident)| resident)
            .collect::<Vec<_>>();
        for resident in removed {
            if let Some(entity) = resident.entity {
                commands.entity(entity).despawn();
            }
        }
        self.pending
            .retain(|spec| desired.get(&spec.tile) == Some(&spec.detail));
        self.enqueue_missing(center, &desired);

        if let Some(spec) = self.pending.pop_front() {
            let entity = build_tile(commands, meshes, materials, terrain, spec)?;
            if let Some(previous) = self
                .tiles
                .insert(spec.tile, ResidentTreeTile { spec, entity })
                && let Some(entity) = previous.entity
            {
                commands.entity(entity).despawn();
            }
        }
        Ok(())
    }

    /// Queues tiles that are missing or built at the wrong detail, nearest first.
    fn enqueue_missing(
        &mut self,
        center: TreeTileIndex,
        desired: &BTreeMap<TreeTileIndex, TreeMeshDetail>,
    ) {
        let mut missing = desired
            .iter()
            .map(|(&tile, &detail)| TreeTileSpec { tile, detail })
            .filter(|spec| {
                self.tiles.get(&spec.tile).map(|resident| resident.spec) != Some(*spec)
                    && !self.pending.contains(spec)
            })
            .collect::<Vec<_>>();
        missing.sort_by_key(|spec| {
            (
                spec.tile.chebyshev_distance(center),
                spec.tile.z,
                spec.tile.x,
            )
        });
        self.pending.extend(missing);
    }
}

/// The tiles that should be resident, and at what detail.
fn desired_tiles(
    center: TreeTileIndex,
    config: ChunkStreamingConfig,
) -> Result<BTreeMap<TreeTileIndex, TreeMeshDetail>, Box<dyn Error>> {
    let radius = i64::try_from(residency_radius(config))?;
    let simplified = tile_radius(config, SIMPLIFIED_MULTIPLIER);

    let mut desired = BTreeMap::new();
    for z_offset in -radius..=radius {
        for x_offset in -radius..=radius {
            let tile = TreeTileIndex {
                x: center
                    .x
                    .checked_add(x_offset)
                    .ok_or_else(|| std::io::Error::other("tree tile x index overflow"))?,
                z: center
                    .z
                    .checked_add(z_offset)
                    .ok_or_else(|| std::io::Error::other("tree tile z index overflow"))?,
            };
            let detail = if tile.chebyshev_distance(center) <= simplified {
                TreeMeshDetail::Simplified
            } else {
                TreeMeshDetail::Silhouette
            };
            desired.insert(tile, detail);
        }
    }
    Ok(desired)
}

fn residency_radius(config: ChunkStreamingConfig) -> u64 {
    tile_radius(config, RESIDENCY_MULTIPLIER)
}

/// Converts a multiple of the chunk load radius into tree tiles.
fn tile_radius(config: ChunkStreamingConfig, multiplier: u64) -> u64 {
    config
        .load_radius()
        .saturating_mul(multiplier)
        .div_ceil(TILE_CHUNKS_PER_EDGE)
}

/// Builds one tile's trees, or nothing when its ground carries no forest.
fn build_tile(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &WorldMaterials,
    terrain: &WorldTerrain,
    spec: TreeTileSpec,
) -> Result<Option<Entity>, Box<dyn Error>> {
    let bounds = spec
        .tile
        .bounds()
        .ok_or_else(|| std::io::Error::other("tree tile bounds are invalid"))?;
    let trees = terrain
        .trees_in(bounds)
        .ok_or_else(|| std::io::Error::other("tree generation is unavailable"))?;
    if trees.is_empty() {
        return Ok(None);
    }
    let Some(prepared) = prepare_trees(&trees, spec.detail, |x, z| terrain.surface_height(x, z))?
    else {
        return Ok(None);
    };
    Ok(Some(
        commands
            .spawn((
                Name::new("tree tile"),
                Mesh3d(meshes.add(prepared.mesh)),
                MeshMaterial3d(materials.trees.clone()),
                Transform::default(),
                WorldMeshOrigin(prepared.world_origin),
            ))
            .id(),
    ))
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(value: i64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: ChunkStreamingConfig = match ChunkStreamingConfig::new(4, 5) {
        Some(config) => config,
        None => panic!("valid radii"),
    };

    fn center() -> TreeTileIndex {
        TreeTileIndex::containing(WorldPosition::new(0.0, 0.0, 0.0)).expect("origin is in range")
    }

    #[test]
    fn tiles_align_with_chunks_and_the_origin() {
        assert_eq!(center(), TreeTileIndex { x: 0, z: 0 });
        let edge = ChunkIndex::edge_meters() * 4.0;
        assert_eq!(
            TreeTileIndex::containing(WorldPosition::new(edge, 0.0, 0.0)),
            Some(TreeTileIndex { x: 1, z: 0 })
        );
    }

    #[test]
    fn negative_coordinates_align_downward() {
        assert_eq!(
            TreeTileIndex::containing(WorldPosition::new(-1.0, 0.0, -1.0)),
            Some(TreeTileIndex { x: -1, z: -1 })
        );
    }

    #[test]
    fn a_tile_covers_its_own_bounds_exactly() {
        let bounds = TreeTileIndex { x: 1, z: -2 }
            .bounds()
            .expect("valid tile bounds");
        let edge = ChunkIndex::edge_meters() * 4.0;
        let expected = TreeBounds::new(edge, -2.0 * edge, 2.0 * edge, -edge).expect("valid bounds");
        assert_eq!(bounds, expected);
    }

    #[test]
    fn detail_coarsens_with_distance_and_covers_the_whole_disc() {
        let desired = desired_tiles(center(), CONFIG).expect("tiles are representable");
        let radius = residency_radius(CONFIG);
        let expected_tiles = usize::try_from((radius * 2 + 1).pow(2)).expect("count fits usize");
        assert_eq!(desired.len(), expected_tiles);

        assert_eq!(desired[&center()], TreeMeshDetail::Simplified);
        let edge_tile = TreeTileIndex {
            x: i64::try_from(radius).expect("radius fits i64"),
            z: 0,
        };
        assert_eq!(desired[&edge_tile], TreeMeshDetail::Silhouette);
    }

    #[test]
    fn detail_never_gets_finer_with_distance() {
        let desired = desired_tiles(center(), CONFIG).expect("tiles are representable");
        let rank = |detail| match detail {
            TreeMeshDetail::Simplified => 0,
            TreeMeshDetail::Silhouette => 1,
        };
        for (tile, &detail) in &desired {
            for (other, &other_detail) in &desired {
                if tile.chebyshev_distance(center()) < other.chebyshev_distance(center()) {
                    assert!(rank(detail) <= rank(other_detail));
                }
            }
        }
    }

    #[test]
    fn branch_geometry_stays_inside_the_residency_disc() {
        let desired = desired_tiles(center(), CONFIG).expect("tiles are representable");
        let simplified_tiles = desired
            .values()
            .filter(|detail| **detail == TreeMeshDetail::Simplified)
            .count();

        let simplified_radius = tile_radius(CONFIG, SIMPLIFIED_MULTIPLIER);
        let expected_tiles =
            usize::try_from((simplified_radius * 2 + 1).pow(2)).expect("count fits usize");
        assert_eq!(simplified_tiles, expected_tiles);
        assert_eq!(TILE_CHUNKS_PER_EDGE, 4);
        assert_eq!(tile_radius(CONFIG, SIMPLIFIED_MULTIPLIER), 10);
        assert!(
            tile_radius(CONFIG, SIMPLIFIED_MULTIPLIER) < tile_radius(CONFIG, RESIDENCY_MULTIPLIER)
        );
    }

    #[test]
    fn trees_reach_much_further_than_walkable_terrain() {
        assert!(residency_radius(CONFIG) * TILE_CHUNKS_PER_EDGE > CONFIG.load_radius() * 4);
    }

    #[test]
    fn queued_tiles_are_ordered_nearest_first() {
        let mut resident = ResidentTrees::default();
        let desired = desired_tiles(center(), CONFIG).expect("tiles are representable");
        resident.enqueue_missing(center(), &desired);

        let distances = resident
            .pending
            .iter()
            .map(|spec| spec.tile.chebyshev_distance(center()))
            .collect::<Vec<_>>();
        assert!(distances.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(distances.first(), Some(&0));
    }

    #[test]
    fn a_queued_tile_is_not_queued_twice() {
        let mut resident = ResidentTrees::default();
        let desired = desired_tiles(center(), CONFIG).expect("tiles are representable");
        resident.enqueue_missing(center(), &desired);
        let queued = resident.pending.len();
        resident.enqueue_missing(center(), &desired);

        assert_eq!(resident.pending.len(), queued);
    }

    #[test]
    fn a_dense_surveyed_tile_has_a_bounded_simplified_mesh() {
        let terrain = WorldTerrain::new(treeline_world::DEFAULT_WORLD_IDENTITY);
        let bounds = TreeBounds::new(5_760.0, 5_888.0, 5_888.0, 6_016.0).expect("tile bounds");
        let trees = terrain.trees_in(bounds).expect("tree generation");
        assert!(trees.len() > 1_500, "the fixture must remain a dense tile");

        let simplified = prepare_trees(&trees, TreeMeshDetail::Simplified, |x, z| {
            terrain.surface_height(x, z)
        })
        .expect("simplified tree mesh")
        .expect("the dense tile has geometry");
        let (vertices, indices) = mesh_counts(&simplified);

        eprintln!("dense simplified tile: {vertices} vertices, {indices} indices");

        assert!(vertices <= 250_000, "dense tile has {vertices} vertices");
        assert!(indices <= 1_250_000, "dense tile has {indices} indices");
    }

    #[test]
    fn four_simplified_tiles_at_a_dense_boundary_have_a_bounded_total_mesh() {
        let terrain = WorldTerrain::new(treeline_world::DEFAULT_WORLD_IDENTITY);
        let mut vertices = 0;
        let mut indices = 0;
        for tile in [
            TreeTileIndex { x: 45, z: 46 },
            TreeTileIndex { x: 46, z: 46 },
            TreeTileIndex { x: 45, z: 47 },
            TreeTileIndex { x: 46, z: 47 },
        ] {
            let bounds = tile.bounds().expect("simplified tile bounds");
            let trees = terrain.trees_in(bounds).expect("tree generation");
            let Some(prepared) = prepare_trees(&trees, TreeMeshDetail::Simplified, |x, z| {
                terrain.surface_height(x, z)
            })
            .expect("simplified tree mesh") else {
                continue;
            };
            let counts = mesh_counts(&prepared);
            vertices += counts.0;
            indices += counts.1;
        }

        eprintln!("four simplified tiles: {vertices} vertices, {indices} indices");
        assert!(
            vertices <= 700_000,
            "four simplified tiles have {vertices} vertices"
        );
        assert!(
            indices <= 3_500_000,
            "four simplified tiles have {indices} indices"
        );
    }

    fn mesh_counts(prepared: &treeline_renderer::PreparedMesh) -> (usize, usize) {
        (
            prepared.mesh.count_vertices(),
            prepared
                .mesh
                .indices()
                .expect("tree surfaces are indexed")
                .len(),
        )
    }
}
