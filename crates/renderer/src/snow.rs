//! Seasonal snow sampled onto terrain vertices.
//!
//! Snow coverage is evaluated on a small fixed lattice over each mesh and
//! interpolated, so render-thread cost stays independent of mesh density and
//! the same ground gets the same snow at every LOD.

use treeline_mesher::Mesh;

pub(crate) const SNOW_GRID_SAMPLES_PER_EDGE: usize = 3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct SnowDepthGrid {
    min_x: f64,
    min_z: f64,
    span_x: f64,
    span_z: f64,
    samples: [f64; SNOW_GRID_SAMPLES_PER_EDGE * SNOW_GRID_SAMPLES_PER_EDGE],
}

impl SnowDepthGrid {
    pub(crate) fn sample(
        mesh: &Mesh,
        mut snow_depth_at: impl FnMut(f64, f64) -> Option<f64>,
    ) -> Self {
        let Some((&first, remaining)) = mesh.positions.split_first() else {
            return Self {
                min_x: 0.0,
                min_z: 0.0,
                span_x: 0.0,
                span_z: 0.0,
                samples: [0.0; SNOW_GRID_SAMPLES_PER_EDGE * SNOW_GRID_SAMPLES_PER_EDGE],
            };
        };
        let ([min_x, min_z], [max_x, max_z]) = remaining.iter().fold(
            ([first[0], first[2]], [first[0], first[2]]),
            |(min, max), position| {
                let point = [position[0], position[2]];
                (
                    [min[0].min(point[0]), min[1].min(point[1])],
                    [max[0].max(point[0]), max[1].max(point[1])],
                )
            },
        );
        let span_x = max_x - min_x;
        let span_z = max_z - min_z;
        let samples = std::array::from_fn(|index| {
            let grid_x = index % SNOW_GRID_SAMPLES_PER_EDGE;
            let grid_z = index / SNOW_GRID_SAMPLES_PER_EDGE;
            let grid_offsets = [0.0, 0.5, 1.0];
            let x = min_x + (span_x * grid_offsets[grid_x]);
            let z = min_z + (span_z * grid_offsets[grid_z]);
            snow_depth_at(x, z).unwrap_or(0.0).clamp(0.0, 1.0)
        });

        Self {
            min_x,
            min_z,
            span_x,
            span_z,
            samples,
        }
    }

    pub(crate) fn coverage_at(self, position: [f64; 3]) -> f64 {
        let (cell_x, blend_x) = snow_grid_axis(position[0], self.min_x, self.span_x);
        let (cell_z, blend_z) = snow_grid_axis(position[2], self.min_z, self.span_z);
        let low = cell_z * SNOW_GRID_SAMPLES_PER_EDGE + cell_x;
        let bottom = lerp_f64(self.samples[low], self.samples[low + 1], blend_x);
        let top = lerp_f64(
            self.samples[low + SNOW_GRID_SAMPLES_PER_EDGE],
            self.samples[low + SNOW_GRID_SAMPLES_PER_EDGE + 1],
            blend_x,
        );
        lerp_f64(bottom, top, blend_z)
    }
}

pub(crate) fn snow_grid_axis(value: f64, minimum: f64, span: f64) -> (usize, f64) {
    let normalized = if span <= f64::EPSILON {
        0.0
    } else {
        ((value - minimum) / span).clamp(0.0, 1.0)
    };
    if normalized <= 0.5 {
        (0, normalized * 2.0)
    } else {
        (1, (normalized - 0.5) * 2.0)
    }
}

pub(crate) fn lerp_f64(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of the grid is bounded cost: a mesh with more vertices must
    /// not cause more snow queries.
    #[test]
    fn snow_is_sampled_on_a_fixed_lattice_regardless_of_mesh_density() {
        let mesh = Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 0.0, 2.0],
                [0.0, 0.0, 2.0],
                [1.0, 0.0, 1.0],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 5],
            colors: Vec::new(),
            indices: vec![0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4],
        };
        let mut queries = 0;
        let grid = SnowDepthGrid::sample(&mesh, |x, z| {
            queries += 1;
            Some((x + z) * 0.25)
        });

        assert_eq!(
            queries,
            SNOW_GRID_SAMPLES_PER_EDGE * SNOW_GRID_SAMPLES_PER_EDGE
        );
        assert!((grid.coverage_at([1.0, 0.0, 1.0]) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn coverage_interpolates_between_lattice_samples() {
        let mesh = Mesh {
            positions: vec![[0.0; 3], [4.0, 0.0, 4.0]],
            normals: vec![[0.0, 1.0, 0.0]; 2],
            colors: Vec::new(),
            indices: Vec::new(),
        };
        let grid = SnowDepthGrid::sample(&mesh, |x, _| Some(x / 4.0));

        assert!(grid.coverage_at([0.0, 0.0, 0.0]) < grid.coverage_at([2.0, 0.0, 0.0]));
        assert!(grid.coverage_at([2.0, 0.0, 0.0]) < grid.coverage_at([4.0, 0.0, 0.0]));
    }
}
