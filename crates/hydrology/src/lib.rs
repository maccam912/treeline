//! Hydrology data structures with explicit physical invariants.

use treeline_coordinates::WorldPosition;

/// A directed segment in a generated river graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiverSegment {
    pub source: WorldPosition,
    pub mouth: WorldPosition,
    pub discharge_cubic_meters_per_second: f64,
}

impl RiverSegment {
    /// Rivers may be level locally, but cannot spontaneously flow uphill.
    pub fn descends_or_is_level(self) -> bool {
        self.source.y >= self.mouth.y
    }
}

/// A terrain basin filled to a single equilibrium surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basin {
    pub bottom_elevation: f64,
    pub spill_elevation: f64,
}

impl Basin {
    pub fn water_depth_at_spill(self) -> Option<f64> {
        (self.spill_elevation >= self.bottom_elevation)
            .then_some(self.spill_elevation - self.bottom_elevation)
    }
}

/// Compact persistent state used when a local water simulation is frozen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrozenWaterState {
    pub volume_cubic_meters: f64,
    pub surface_elevation: f64,
    pub outflow_cubic_meters_per_second: f64,
}

impl FrozenWaterState {
    pub fn is_physical(self) -> bool {
        self.volume_cubic_meters >= 0.0 && self.outflow_cubic_meters_per_second >= 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn river_direction_rejects_uphill_segments() {
        let river = RiverSegment {
            source: WorldPosition::new(0.0, 100.0, 0.0),
            mouth: WorldPosition::new(10.0, 101.0, 0.0),
            discharge_cubic_meters_per_second: 2.0,
        };
        assert!(!river.descends_or_is_level());
    }

    #[test]
    fn basin_depth_is_derived_from_its_spill_point() {
        let basin = Basin {
            bottom_elevation: 70.0,
            spill_elevation: 82.5,
        };
        let depth = basin.water_depth_at_spill().expect("valid basin");
        assert!((depth - 12.5).abs() < f64::EPSILON);
    }

    #[test]
    fn impossible_basins_are_rejected() {
        let basin = Basin {
            bottom_elevation: 10.0,
            spill_elevation: 9.0,
        };
        assert_eq!(basin.water_depth_at_spill(), None);
    }
}
