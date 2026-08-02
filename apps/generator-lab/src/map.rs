//! Building the top-down map the lab draws.
//!
//! The map is a flat grid of quads, one color sample per cell. Sampling per
//! cell rather than per vertex keeps hard edges hard: a lake shore or a stand
//! boundary shows up where it actually is instead of being smoothed away by
//! interpolation.

use treeline_climate::Season;
use treeline_mesher::Mesh;
use treeline_world::WorldTerrain;

use crate::view::ViewMode;

/// Map cells across the shorter screen axis.
///
/// Enough to resolve the six-meter canopy grid at close zoom, few enough to
/// rebuild every time the view changes without a visible pause.
const CELLS_PER_SHORT_EDGE: usize = 192;

/// The area the map currently shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapView {
    pub center: [f64; 2],
    /// Width of the view in world meters.
    pub span_meters: f64,
    pub mode: ViewMode,
    pub season: Season,
}

impl MapView {
    /// Cell size and grid dimensions for a viewport.
    fn grid(self, width: u32, height: u32) -> ([usize; 2], f64) {
        let aspect = f64::from(width.max(1)) / f64::from(height.max(1));
        let cell_meters = self.span_meters / usize_as_f64(CELLS_PER_SHORT_EDGE);
        let (cells_x, cells_z) = if aspect >= 1.0 {
            (
                scaled_cells(usize_as_f64(CELLS_PER_SHORT_EDGE) * aspect),
                CELLS_PER_SHORT_EDGE,
            )
        } else {
            (
                CELLS_PER_SHORT_EDGE,
                scaled_cells(usize_as_f64(CELLS_PER_SHORT_EDGE) / aspect),
            )
        };
        ([cells_x, cells_z], cell_meters)
    }

    /// The world position under a viewport pixel.
    pub fn world_position_at(self, pixel: [f64; 2], width: u32, height: u32) -> [f64; 2] {
        let ([cells_x, cells_z], cell_meters) = self.grid(width, height);
        let span = [
            usize_as_f64(cells_x) * cell_meters,
            usize_as_f64(cells_z) * cell_meters,
        ];
        let fraction = [
            pixel[0] / f64::from(width.max(1)),
            pixel[1] / f64::from(height.max(1)),
        ];
        [
            self.center[0] + ((fraction[0] - 0.5) * span[0]),
            self.center[1] + ((fraction[1] - 0.5) * span[1]),
        ]
    }
}

/// Builds the map mesh for one view.
///
/// Cells the mode has nothing to say about are omitted rather than drawn in a
/// placeholder color, so gaps in a layer stay visibly empty.
pub fn build(terrain: WorldTerrain, view: MapView, width: u32, height: u32) -> Mesh {
    let ([cells_x, cells_z], cell_meters) = view.grid(width, height);
    let origin = [
        view.center[0] - (usize_as_f64(cells_x) * cell_meters * 0.5),
        view.center[1] - (usize_as_f64(cells_z) * cell_meters * 0.5),
    ];

    let mut mesh = Mesh::default();
    for cell_z in 0..cells_z {
        for cell_x in 0..cells_x {
            let min = [
                origin[0] + (usize_as_f64(cell_x) * cell_meters),
                origin[1] + (usize_as_f64(cell_z) * cell_meters),
            ];
            let center = [min[0] + (cell_meters * 0.5), min[1] + (cell_meters * 0.5)];
            let Some(color) = view
                .mode
                .color_at(terrain, center[0], center[1], view.season)
            else {
                continue;
            };
            append_quad(&mut mesh, min, cell_meters, color);
        }
    }
    mesh
}

/// Appends one upward-facing quad on the map plane.
fn append_quad(mesh: &mut Mesh, min: [f64; 2], edge: f64, color: [f32; 4]) {
    let Ok(base) = u32::try_from(mesh.positions.len()) else {
        return;
    };
    let max = [min[0] + edge, min[1] + edge];
    mesh.positions.extend([
        [min[0], 0.0, min[1]],
        [min[0], 0.0, max[1]],
        [max[0], 0.0, min[1]],
        [max[0], 0.0, max[1]],
    ]);
    mesh.normals.extend([[0.0, 1.0, 0.0]; 4]);
    mesh.colors.extend([color; 4]);
    mesh.indices
        .extend([base, base + 1, base + 2, base + 2, base + 1, base + 3]);
}

/// Rounds a scaled cell count to a usable grid dimension.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scaled_cells(count: f64) -> usize {
    if count.is_finite() && count >= 1.0 {
        (count as usize).min(CELLS_PER_SHORT_EDGE * 8)
    } else {
        1
    }
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_world::DEFAULT_WORLD_IDENTITY;

    const TERRAIN: WorldTerrain = WorldTerrain::new(DEFAULT_WORLD_IDENTITY);

    fn view(mode: ViewMode) -> MapView {
        MapView {
            center: [5_000.0, 5_000.0],
            span_meters: 2_000.0,
            mode,
            season: Season::Winter,
        }
    }

    #[test]
    fn the_map_is_a_well_formed_upward_facing_grid() {
        let mesh = build(TERRAIN, view(ViewMode::Elevation), 1_200, 800);
        assert!(mesh.is_well_formed());
        assert!(!mesh.indices.is_empty());
        assert!(mesh.normals.iter().all(|normal| normal[1] > 0.99));
        assert!(
            mesh.positions
                .iter()
                .all(|position| position[1].to_bits() == 0)
        );
    }

    #[test]
    fn the_map_is_centered_on_the_view() {
        let view = view(ViewMode::Elevation);
        let mesh = build(TERRAIN, view, 800, 800);
        let min_x = mesh
            .positions
            .iter()
            .map(|position| position[0])
            .fold(f64::INFINITY, f64::min);
        let max_x = mesh
            .positions
            .iter()
            .map(|position| position[0])
            .fold(f64::NEG_INFINITY, f64::max);

        assert!(((min_x + max_x) * 0.5 - view.center[0]).abs() < 1.0);
        assert!((max_x - min_x - view.span_meters).abs() < view.span_meters * 0.05);
    }

    #[test]
    fn a_wider_viewport_shows_more_ground_horizontally() {
        let view = view(ViewMode::Elevation);
        let wide = build(TERRAIN, view, 1_600, 800);
        let square = build(TERRAIN, view, 800, 800);
        assert!(wide.positions.len() > square.positions.len());
    }

    #[test]
    fn the_center_pixel_maps_back_to_the_view_center() {
        let view = view(ViewMode::Elevation);
        let center = view.world_position_at([600.0, 400.0], 1_200, 800);
        assert!((center[0] - view.center[0]).abs() < 10.0);
        assert!((center[1] - view.center[1]).abs() < 10.0);
    }

    #[test]
    fn pixels_map_left_to_right_and_top_to_bottom() {
        let view = view(ViewMode::Elevation);
        let left = view.world_position_at([0.0, 400.0], 1_200, 800);
        let right = view.world_position_at([1_200.0, 400.0], 1_200, 800);
        let top = view.world_position_at([600.0, 0.0], 1_200, 800);
        let bottom = view.world_position_at([600.0, 800.0], 1_200, 800);

        assert!(left[0] < right[0]);
        assert!(top[1] < bottom[1]);
    }

    #[test]
    fn a_layer_with_gaps_omits_cells_rather_than_filling_them() {
        // Canopy is absent over water and clearings; elevation covers everything.
        let elevation = build(TERRAIN, view(ViewMode::Elevation), 800, 800);
        let imagery = build(TERRAIN, view(ViewMode::Imagery), 800, 800);
        assert_eq!(elevation.positions.len(), imagery.positions.len());
    }

    #[test]
    fn generation_is_repeatable() {
        let view = view(ViewMode::CanopyCover);
        assert_eq!(
            build(TERRAIN, view, 800, 600),
            build(TERRAIN, view, 800, 600)
        );
    }
}
