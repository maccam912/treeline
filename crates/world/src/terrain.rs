//! The world the player stands in.
//!
//! [`WorldTerrain`] is the single place the measured layers, the site climate,
//! and the tree generator are composed into one thing the renderer and the
//! streamer can ask questions of. It owns no caches and no mutable state:
//! every query is a pure function of position, so two threads meshing adjacent
//! chunks cannot disagree.

use treeline_climate::{Season, SiteClimate};
use treeline_coordinates::{WorldIdentity, WorldPosition};
use treeline_ecology::{Forest, ForestComposition, ProceduralTree, Stand, TreeBounds};
use treeline_mesher::{Mesh, MeshingError, transvoxel_chunk};
use treeline_terrain::{DensityField, SurfaceField, SurveyedTerrain, TerrainSample};

use crate::mesh::TerrainMeshSpec;
use crate::streaming::far_terrain_mesh;
use crate::water;

/// Horizontal radius used to measure terrain slope for snow retention.
///
/// Fixed in world space rather than derived from mesh normals, so the same
/// ground receives the same snow at every terrain LOD.
const SNOW_SLOPE_SAMPLE_RADIUS_METERS: f64 = 16.0;

/// Mapped lake water above the measured surface at one position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LakeSurface {
    /// Stable per-tile identifier from the source hydrography.
    pub id: u8,
    pub surface_elevation_meters: f64,
    pub terrain_elevation_meters: f64,
    pub water_depth_meters: f64,
}

/// Seasonal snow lying on the terrain surface.
///
/// This is a render treatment. It does not change signed density, so it never
/// alters where the player can walk.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnowCover {
    pub season: Season,
    pub snowpack_water_equivalent_millimeters: f64,
    pub terrain_slope: f64,
    pub coverage_fraction: f64,
}

/// The composed world: measured terrain, its climate, and its forest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldTerrain {
    world: WorldIdentity,
    surveyed: SurveyedTerrain,
    climate: SiteClimate,
    composition: ForestComposition,
}

impl WorldTerrain {
    /// Composes the surveyed bundle for one world identity.
    ///
    /// The identity supplies stable per-individual variation; it does not
    /// change the measurements.
    pub const fn new(world: WorldIdentity) -> Self {
        Self {
            world,
            surveyed: SurveyedTerrain,
            climate: SiteClimate::SURVEYED_TILE,
            composition: ForestComposition::SURVEYED_TILE,
        }
    }

    pub const fn world(self) -> WorldIdentity {
        self.world
    }

    pub const fn climate(self) -> SiteClimate {
        self.climate
    }

    pub const fn composition(self) -> ForestComposition {
        self.composition
    }

    /// Bare-earth elevation, in meters above the bundle's vertical datum.
    ///
    /// The same value [`SurfaceField::surface_height`] reports, as an inherent
    /// method so callers do not need the trait in scope.
    pub fn surface_height_at(self, x: f64, z: f64) -> Option<f64> {
        self.surveyed.height_at(x, z)
    }

    /// Natural-color surface appearance from the bundle's aerial imagery.
    pub fn surface_color_at(self, x: f64, z: f64) -> Option<[f32; 4]> {
        self.surveyed.color_at(x, z)
    }

    /// Mapped lake water covering a position, if any.
    pub fn lake_at(self, x: f64, z: f64) -> Option<LakeSurface> {
        let lake = self.surveyed.lake_at(x, z)?;
        let terrain_elevation_meters = self.surveyed.height_at(x, z)?;
        Some(LakeSurface {
            id: lake.id,
            surface_elevation_meters: lake.surface_elevation_meters,
            terrain_elevation_meters,
            // The bundle carries a lake level, not a lake bottom. Where the
            // dilated footprint reaches above the recorded level, the sheet
            // still needs a visible film rather than negative depth.
            water_depth_meters: (lake.surface_elevation_meters - terrain_elevation_meters)
                .max(water::MINIMUM_VISIBLE_DEPTH_METERS),
        })
    }

    /// Measured forest structure over a position, or `None` on open ground.
    pub fn stand_at(self, x: f64, z: f64) -> Option<Stand> {
        let canopy = self.surveyed.canopy_at(x, z)?;
        Stand::measured(canopy.cover_fraction, canopy.top_height_meters)
    }

    /// Whether a surface feature can stand here: solid ground, and not a lake.
    pub fn has_dry_ground_at(self, x: f64, z: f64) -> bool {
        self.surveyed.height_at(x, z).is_some_and(|surface| {
            self.sample(WorldPosition::new(x, surface - 0.35, z))
                .is_solid()
        }) && self.lake_at(x, z).is_none()
    }

    /// Generates the trees standing inside a horizontal area.
    ///
    /// Every tree is sized by the stand measured at its own placement cell, so
    /// open ground stays open and short regrowth stays short.
    pub fn trees_in(self, bounds: TreeBounds) -> Option<Vec<ProceduralTree>> {
        let mut trees = Forest::new(self.world).trees_in(
            bounds,
            self.composition,
            self.climate.prevailing_wind,
            |x, z| self.stand_at(x, z),
        )?;
        trees.retain(|tree| self.has_dry_ground_at(tree.x, tree.z));
        Some(trees)
    }

    /// Samples seasonal snow using a fixed-scale slope measurement.
    pub fn snow_cover_at(self, x: f64, z: f64, season: Season) -> Option<SnowCover> {
        let radius = SNOW_SLOPE_SAMPLE_RADIUS_METERS;
        let left = self.surveyed.height_at(x - radius, z)?;
        let right = self.surveyed.height_at(x + radius, z)?;
        let north = self.surveyed.height_at(x, z - radius)?;
        let south = self.surveyed.height_at(x, z + radius)?;
        let span = radius * 2.0;
        let slope = libm::hypot((right - left) / span, (south - north) / span);
        self.snow_cover_for_slope(x, z, season, slope)
    }

    /// Samples seasonal snow using a slope the caller already computed.
    ///
    /// Mesh generation already produces surface normals; reusing that slope
    /// avoids four extra terrain queries per vertex.
    pub fn snow_cover_for_slope(
        self,
        x: f64,
        z: f64,
        season: Season,
        terrain_slope: f64,
    ) -> Option<SnowCover> {
        if !terrain_slope.is_finite() || terrain_slope < 0.0 {
            return None;
        }
        let elevation = self.surveyed.height_at(x, z)?;
        let snowpack = self
            .climate
            .season(season, elevation)?
            .snowpack_water_equivalent_millimeters;
        Some(SnowCover {
            season,
            snowpack_water_equivalent_millimeters: snowpack,
            terrain_slope,
            // Deep snow covers more ground; steep ground sheds it.
            coverage_fraction: (smoothstep(8.0, 240.0, snowpack)
                * (1.0 - smoothstep(0.32, 1.15, terrain_slope)))
            .clamp(0.0, 1.0),
        })
    }

    /// Builds the visible terrain surface for one near chunk or far tile.
    ///
    /// Trees are streamed separately, so coarsening terrain never replaces
    /// individual trees with a canopy surface.
    ///
    /// # Errors
    ///
    /// Returns [`MeshingError`] when the LOD is unsupported, a surface sample
    /// is unavailable, or the mesh exceeds index capacity.
    pub fn render_mesh(&self, spec: TerrainMeshSpec) -> Result<Mesh, MeshingError> {
        let mut mesh = match spec {
            TerrainMeshSpec::Near(spec) => {
                transvoxel_chunk(self, spec.chunk, spec.lod, spec.transition_faces)
            }
            TerrainMeshSpec::Far(spec) => far_terrain_mesh(self, spec),
        }?;
        self.apply_surface_colors(&mut mesh);
        Ok(mesh)
    }

    /// Builds the lake sheet aligned with one terrain mesh.
    ///
    /// # Errors
    ///
    /// Returns [`MeshingError`] when the LOD is unsupported, the surface grid
    /// is invalid, or the mesh exceeds index capacity.
    pub fn lake_surface_mesh(&self, spec: TerrainMeshSpec) -> Result<Mesh, MeshingError> {
        water::lake_sheet(*self, spec)
    }

    /// Paints mesh vertices with the bundle's aerial imagery.
    ///
    /// A mesh whose every vertex fell outside the imagery keeps no colors at
    /// all, which lets the renderer fall back to material shading rather than
    /// tinting terrain white.
    fn apply_surface_colors(self, mesh: &mut Mesh) {
        mesh.colors.clear();
        mesh.colors.reserve(mesh.positions.len());
        for position in &mesh.positions {
            mesh.colors.push(
                self.surface_color_at(position[0], position[2])
                    .unwrap_or(UNCOLORED),
            );
        }
        if mesh.colors.iter().all(|color| color[3] <= f32::EPSILON) {
            mesh.colors.clear();
        }
    }
}

/// A vertex the imagery does not cover: white with zero blend weight.
const UNCOLORED: [f32; 4] = [1.0, 1.0, 1.0, 0.0];

impl DensityField for WorldTerrain {
    fn sample(&self, position: WorldPosition) -> TerrainSample {
        self.surveyed.sample(position)
    }
}

impl SurfaceField for WorldTerrain {
    fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
        self.surveyed.surface_height(x, z)
    }

    fn volume_bounds(&self, min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> Option<(f64, f64)> {
        self.surveyed.volume_bounds(min_x, min_z, max_x, max_z)
    }
}

fn smoothstep(edge0: f64, edge1: f64, value: f64) -> f64 {
    let amount = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    amount * amount * (3.0 - (2.0 * amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEFAULT_WORLD_IDENTITY;
    use treeline_terrain::{SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z};

    const WORLD: WorldTerrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);
    /// Interior of Upper Holmes Lake, the largest mapped waterbody in the tile.
    const LAKE_INTERIOR: [f64; 2] = [7_364.0, 6_894.0];

    #[test]
    fn the_spawn_stands_on_dry_measured_ground() {
        assert!(WORLD.has_dry_ground_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z));
        assert!(
            WORLD
                .surface_color_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z)
                .is_some()
        );
    }

    #[test]
    fn mapped_lakes_are_wet_and_have_depth() {
        let lake = WORLD
            .lake_at(LAKE_INTERIOR[0], LAKE_INTERIOR[1])
            .expect("Upper Holmes Lake is mapped");
        assert_eq!(lake.id, 19);
        assert!(lake.water_depth_meters > 0.0);
        assert!(!WORLD.has_dry_ground_at(LAKE_INTERIOR[0], LAKE_INTERIOR[1]));
    }

    #[test]
    fn winter_lays_more_snow_than_summer() {
        let winter = WORLD
            .snow_cover_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z, Season::Winter)
            .expect("spawn has climate");
        let summer = WORLD
            .snow_cover_at(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z, Season::Summer)
            .expect("spawn has climate");

        assert!(winter.coverage_fraction > 0.5);
        assert_eq!(summer.coverage_fraction.to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn steep_ground_sheds_snow() {
        let flat = WORLD
            .snow_cover_for_slope(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z, Season::Winter, 0.0)
            .expect("spawn has climate");
        let steep = WORLD
            .snow_cover_for_slope(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z, Season::Winter, 1.4)
            .expect("spawn has climate");

        assert!(flat.coverage_fraction > steep.coverage_fraction);
        assert_eq!(steep.coverage_fraction.to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn non_finite_slopes_are_rejected() {
        assert_eq!(
            WORLD.snow_cover_for_slope(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z, Season::Winter, -1.0),
            None
        );
        assert_eq!(
            WORLD.snow_cover_for_slope(
                SURVEYED_SPAWN_X,
                SURVEYED_SPAWN_Z,
                Season::Winter,
                f64::NAN
            ),
            None
        );
    }

    #[test]
    fn measured_canopy_produces_trees_only_where_forest_stands() {
        let bounds = TreeBounds::new(
            SURVEYED_SPAWN_X - 48.0,
            SURVEYED_SPAWN_Z - 48.0,
            SURVEYED_SPAWN_X + 48.0,
            SURVEYED_SPAWN_Z + 48.0,
        )
        .expect("valid bounds");
        let trees = WORLD.trees_in(bounds).expect("bounds are representable");
        assert!(!trees.is_empty());
        for tree in trees {
            let stand = WORLD
                .stand_at(tree.x, tree.z)
                .expect("tree stands in forest");
            assert!(tree.height_meters <= stand.top_height_meters());
            assert!(WORLD.has_dry_ground_at(tree.x, tree.z));
        }
    }

    #[test]
    fn no_trees_grow_in_a_lake() {
        let bounds = TreeBounds::new(
            LAKE_INTERIOR[0] - 24.0,
            LAKE_INTERIOR[1] - 24.0,
            LAKE_INTERIOR[0] + 24.0,
            LAKE_INTERIOR[1] + 24.0,
        )
        .expect("valid bounds");
        assert!(
            WORLD
                .trees_in(bounds)
                .expect("bounds are representable")
                .is_empty()
        );
    }

    #[test]
    fn sampling_is_order_independent() {
        let first = WORLD.surface_height(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z);
        let _elsewhere = WORLD.surface_height(LAKE_INTERIOR[0], LAKE_INTERIOR[1]);
        assert_eq!(
            WORLD.surface_height(SURVEYED_SPAWN_X, SURVEYED_SPAWN_Z),
            first
        );
    }
}
