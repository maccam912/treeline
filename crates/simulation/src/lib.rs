//! Active-region simulation and independently adjustable survival pressures.

use std::collections::BTreeMap;

use treeline_hydrology::{ActiveWaterError, ActiveWaterRegion, FrozenWaterRegion, WaterStepReport};

/// Stable identity of one local simulation footprint.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActiveRegionId {
    pub x: i64,
    pub z: i64,
}

impl ActiveRegionId {
    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }
}

/// Lifecycle owner for deterministic active and frozen local water.
///
/// World generation supplies a regenerated topology on activation. The
/// simulation retains only compact changing state after a region leaves the
/// player bubble.
#[derive(Clone, Debug, Default)]
pub struct ActiveWaterSimulation {
    active: BTreeMap<ActiveRegionId, ActiveWaterRegion>,
    frozen: BTreeMap<ActiveRegionId, FrozenWaterRegion>,
}

impl ActiveWaterSimulation {
    pub fn active_region(&self, id: ActiveRegionId) -> Option<&ActiveWaterRegion> {
        self.active.get(&id)
    }

    pub fn active_region_mut(&mut self, id: ActiveRegionId) -> Option<&mut ActiveWaterRegion> {
        self.active.get_mut(&id)
    }

    pub fn frozen_region(&self, id: ActiveRegionId) -> Option<&FrozenWaterRegion> {
        self.frozen.get(&id)
    }

    /// Activates regenerated topology, restoring a previous compact summary
    /// when the region has already been visited.
    ///
    /// # Errors
    ///
    /// Returns an error when a retained summary does not match the regenerated
    /// cell topology.
    pub fn activate(
        &mut self,
        id: ActiveRegionId,
        regenerated: ActiveWaterRegion,
    ) -> Result<&ActiveWaterRegion, ActiveWaterError> {
        let water = if let Some(summary) = self.frozen.remove(&id) {
            ActiveWaterRegion::reconstruct(regenerated, &summary)?
        } else {
            regenerated
        };
        self.active.insert(id, water);
        Ok(&self.active[&id])
    }

    /// Freezes a region after it leaves every player's active bubble.
    pub fn freeze(&mut self, id: ActiveRegionId) -> Option<&FrozenWaterRegion> {
        let summary = self.active.remove(&id)?.freeze();
        self.frozen.insert(id, summary);
        self.frozen.get(&id)
    }

    /// Advances every active region in stable coordinate order.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested step is outside the active water
    /// model's finite, positive, at-most-60-second range.
    pub fn step(
        &mut self,
        delta_seconds: f64,
    ) -> Result<Vec<(ActiveRegionId, WaterStepReport)>, ActiveWaterError> {
        self.active
            .iter_mut()
            .map(|(&id, water)| water.step(delta_seconds).map(|report| (id, report)))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pressure {
    Off,
    Gentle,
    Moderate,
    Demanding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurvivalSettings {
    pub hunger: Pressure,
    pub thirst: Pressure,
    pub temperature: Pressure,
    pub injuries: Pressure,
    pub weather: Pressure,
    pub wildlife: Pressure,
    pub navigation: Pressure,
}

impl Default for SurvivalSettings {
    fn default() -> Self {
        Self {
            hunger: Pressure::Gentle,
            thirst: Pressure::Moderate,
            temperature: Pressure::Moderate,
            injuries: Pressure::Gentle,
            weather: Pressure::Moderate,
            wildlife: Pressure::Gentle,
            navigation: Pressure::Gentle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_hydrology::{WaterCell, WaterCellId, WaterCellKind, WaterConnection};

    #[test]
    fn survival_pressures_are_independent() {
        let settings = SurvivalSettings {
            hunger: Pressure::Off,
            navigation: Pressure::Demanding,
            ..SurvivalSettings::default()
        };
        assert_eq!(settings.hunger, Pressure::Off);
        assert_eq!(settings.navigation, Pressure::Demanding);
        assert_eq!(settings.weather, Pressure::Moderate);
    }

    fn generated_water(depth: f64) -> ActiveWaterRegion {
        ActiveWaterRegion::new(
            vec![WaterCell {
                id: WaterCellId(1),
                kind: WaterCellKind::Surface,
                bed_elevation_meters: 2.0,
                bank_elevation_meters: 4.0,
                area_square_meters: 100.0,
                water_depth_meters: depth,
                source_cubic_meters_per_second: 0.2,
                infiltration_cubic_meters_per_second: 0.0,
            }],
            vec![WaterConnection {
                from: WaterCellId(1),
                to: None,
                sill_elevation_meters: 3.0,
                width_meters: 1.0,
                conductance: 0.1,
            }],
        )
        .expect("generated water")
    }

    #[test]
    fn water_lifecycle_freezes_and_reconstructs_changing_state() {
        let id = ActiveRegionId::new(-2, 7);
        let mut simulation = ActiveWaterSimulation::default();
        simulation
            .activate(id, generated_water(1.2))
            .expect("activation");
        simulation.step(10.0).expect("step");
        let depth_before_freeze = simulation
            .active_region(id)
            .expect("active")
            .cell(WaterCellId(1))
            .expect("cell")
            .water_depth_meters;
        let frozen = simulation.freeze(id).expect("frozen").clone();
        assert!(simulation.active_region(id).is_none());

        simulation
            .activate(id, generated_water(0.0))
            .expect("reconstruction");
        let restored_depth = simulation
            .active_region(id)
            .expect("active")
            .cell(WaterCellId(1))
            .expect("cell")
            .water_depth_meters;
        assert!((restored_depth - depth_before_freeze).abs() <= 0.0005);
        assert_eq!(
            simulation.active_region(id).expect("active").freeze(),
            frozen
        );
    }

    #[test]
    fn active_regions_step_in_stable_coordinate_order() {
        let mut simulation = ActiveWaterSimulation::default();
        simulation
            .activate(ActiveRegionId::new(3, 0), generated_water(0.5))
            .expect("activation");
        simulation
            .activate(ActiveRegionId::new(-1, 4), generated_water(0.5))
            .expect("activation");
        let reports = simulation.step(1.0).expect("step");
        assert_eq!(reports[0].0, ActiveRegionId::new(-1, 4));
        assert_eq!(reports[1].0, ActiveRegionId::new(3, 0));
    }
}
