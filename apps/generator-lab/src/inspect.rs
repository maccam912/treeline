//! Reporting everything known about one position.
//!
//! Clicking asks the same questions the game asks while streaming, and prints
//! the answers verbatim. When something looks wrong on the map, this is how you
//! find out which layer disagrees.

use treeline_climate::Season;
use treeline_ecology::TreeBounds;
use treeline_world::WorldTerrain;

/// Radius of the tree sample taken around a click, in meters.
const TREE_SAMPLE_RADIUS_METERS: f64 = 32.0;

/// Everything the world reports at one horizontal position.
#[derive(Clone, Debug, PartialEq)]
pub struct Inspection {
    pub position: [f64; 2],
    pub lines: Vec<String>,
}

/// Samples every layer at a position.
pub fn at(terrain: WorldTerrain, x: f64, z: f64, season: Season) -> Inspection {
    let mut lines = Vec::new();

    match terrain.surface_height_at(x, z) {
        Some(elevation) => lines.push(format!("elevation  {elevation:.2} m")),
        None => lines.push("elevation  outside the measured tile".to_owned()),
    }

    lines.push(match terrain.lake_at(x, z) {
        Some(lake) => format!(
            "water      lake {} at {:.1} m, {:.2} m deep",
            lake.id, lake.surface_elevation_meters, lake.water_depth_meters
        ),
        None => "water      dry".to_owned(),
    });

    lines.push(match terrain.stand_at(x, z) {
        Some(stand) => format!(
            "canopy     {:.0}% cover, {:.1} m tall, ~{:.0} stems/ha",
            stand.canopy_cover_fraction() * 100.0,
            stand.top_height_meters(),
            stand.stems_per_hectare()
        ),
        None => "canopy     open ground".to_owned(),
    });

    if let Some(snow) = terrain.snow_cover_at(x, z, season) {
        lines.push(format!(
            "snow       {:.0}% cover in {}, {:.0} mm pack, slope {:.2}",
            snow.coverage_fraction * 100.0,
            season.label(),
            snow.snowpack_water_equivalent_millimeters,
            snow.terrain_slope
        ));
    }

    lines.push(describe_trees(terrain, x, z));
    lines.push(format!(
        "ground     {}",
        if terrain.has_dry_ground_at(x, z) {
            "walkable"
        } else {
            "not walkable"
        }
    ));

    Inspection {
        position: [x, z],
        lines,
    }
}

/// Summarizes the trees standing near a position.
fn describe_trees(terrain: WorldTerrain, x: f64, z: f64) -> String {
    let radius = TREE_SAMPLE_RADIUS_METERS;
    let Some(bounds) = TreeBounds::new(x - radius, z - radius, x + radius, z + radius) else {
        return "trees      unavailable".to_owned();
    };
    let Some(trees) = terrain.trees_in(bounds) else {
        return "trees      unavailable".to_owned();
    };
    let Some(tallest) = trees
        .iter()
        .max_by(|left, right| left.height_meters.total_cmp(&right.height_meters))
    else {
        return format!("trees      none within {radius:.0} m");
    };
    format!(
        "trees      {} within {:.0} m; tallest {:.1} m {} {}",
        trees.len(),
        radius,
        tallest.height_meters,
        tallest.genotype.functional_group.label(),
        tallest.condition.label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_world::DEFAULT_WORLD_IDENTITY;

    const TERRAIN: WorldTerrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);
    const SPAWN: [f64; 2] = [6_737.5, 7_211.7];
    const LAKE: [f64; 2] = [7_364.0, 6_894.0];

    #[test]
    fn every_layer_reports_something_at_the_spawn() {
        let inspection = at(TERRAIN, SPAWN[0], SPAWN[1], Season::Winter);
        assert_eq!(
            inspection.position.map(f64::to_bits),
            SPAWN.map(f64::to_bits)
        );
        for layer in ["elevation", "water", "canopy", "snow", "trees", "ground"] {
            assert!(
                inspection.lines.iter().any(|line| line.starts_with(layer)),
                "no {layer} line in {:?}",
                inspection.lines
            );
        }
    }

    #[test]
    fn a_lake_reports_its_identity_and_is_not_walkable() {
        let inspection = at(TERRAIN, LAKE[0], LAKE[1], Season::Winter);
        assert!(inspection.lines.iter().any(|line| line.contains("lake 19")));
        assert!(
            inspection
                .lines
                .iter()
                .any(|line| line.contains("not walkable"))
        );
    }

    #[test]
    fn dry_ground_reports_as_dry_and_walkable() {
        let inspection = at(TERRAIN, SPAWN[0], SPAWN[1], Season::Winter);
        assert!(inspection.lines.iter().any(|line| line.contains("dry")));
        assert!(
            inspection
                .lines
                .iter()
                .any(|line| line.ends_with("walkable") && !line.contains("not"))
        );
    }

    #[test]
    fn inspecting_outside_the_tile_reports_rather_than_panicking() {
        let inspection = at(TERRAIN, 1.0e9, -1.0e9, Season::Winter);
        assert!(!inspection.lines.is_empty());
    }

    #[test]
    fn inspection_is_repeatable() {
        assert_eq!(
            at(TERRAIN, SPAWN[0], SPAWN[1], Season::Summer),
            at(TERRAIN, SPAWN[0], SPAWN[1], Season::Summer)
        );
    }
}
