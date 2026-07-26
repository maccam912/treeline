//! Deterministic river networks and water-state primitives.

use std::collections::{BTreeMap, BTreeSet};

use treeline_coordinates::{WorldIdentity, WorldPosition};
use treeline_geography::{
    DRAINAGE_CELL_EDGE_METERS, DrainageCellIndex, MacroElevation, RegionalProfile, WatershedRegion,
    WatershedRegionIndex,
};

const SECONDS_PER_YEAR: f64 = 31_556_952.0;
const MIN_CHANNEL_CATCHMENT_CELLS: u64 = 8;

/// A directed segment in a generated river graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiverSegment {
    pub source_cell: DrainageCellIndex,
    pub mouth_cell: DrainageCellIndex,
    pub source: WorldPosition,
    pub mouth: WorldPosition,
    pub drainage_area_square_kilometers: f64,
    pub discharge_cubic_meters_per_second: f64,
}

impl RiverSegment {
    /// Rivers may be level locally, but cannot spontaneously flow uphill.
    pub fn descends_or_is_level(self) -> bool {
        self.source.y >= self.mouth.y
    }
}

/// River channels derived from one deterministic regional drainage artifact.
///
/// Rainfall becomes local runoff in every drainage cell, then accumulates
/// downstream through the region's flow graph. A channel begins once at least
/// 32 km² (eight coarse cells) contribute to it. Cross-region segments retain
/// an explicit mouth, while discharge includes only catchment represented by
/// this artifact; larger multi-region accumulation belongs to a later
/// hierarchical drainage layer.
#[derive(Clone, Debug, PartialEq)]
pub struct RiverNetwork {
    pub world: WorldIdentity,
    pub region: WatershedRegionIndex,
    segments: Vec<RiverSegment>,
}

impl RiverNetwork {
    pub fn generate(world: WorldIdentity, region: WatershedRegionIndex) -> Option<Self> {
        let watershed = WatershedRegion::generate(world, region)?;
        Self::from_watershed(&watershed)
    }

    /// Derives channels without regenerating an already-cached watershed.
    pub fn from_watershed(watershed: &WatershedRegion) -> Option<Self> {
        let cells = watershed.cells();
        let slots = cells
            .iter()
            .enumerate()
            .map(|(slot, cell)| (cell.index, slot))
            .collect::<BTreeMap<_, _>>();
        let mut incoming = vec![0_usize; cells.len()];
        let mut discharge = Vec::with_capacity(cells.len());

        for cell in cells {
            discharge.push(local_runoff(watershed.world, cell.index)?);
            if let Some(target) = cell.flow_to.and_then(|target| slots.get(&target)) {
                incoming[*target] = incoming[*target].checked_add(1)?;
            }
        }

        let mut ready = incoming
            .iter()
            .enumerate()
            .filter_map(|(slot, &count)| (count == 0).then_some(slot))
            .collect::<BTreeSet<_>>();
        let mut processed = 0_usize;
        while let Some(slot) = ready.pop_first() {
            processed = processed.checked_add(1)?;
            let Some(target) = cells[slot].flow_to.and_then(|target| slots.get(&target)) else {
                continue;
            };
            discharge[*target] += discharge[slot];
            incoming[*target] = incoming[*target].checked_sub(1)?;
            if incoming[*target] == 0 {
                ready.insert(*target);
            }
        }
        if processed != cells.len() {
            return None;
        }

        let terrain = MacroElevation::new(watershed.world);
        let cell_area_square_kilometers =
            DRAINAGE_CELL_EDGE_METERS * DRAINAGE_CELL_EDGE_METERS / 1_000_000.0;
        let mut segments = Vec::new();
        for (slot, cell) in cells.iter().enumerate() {
            if cell.flow_accumulation_cells < MIN_CHANNEL_CATCHMENT_CELLS {
                continue;
            }
            let Some(mouth_cell) = cell.flow_to else {
                continue;
            };
            let [source_x, source_z] = cell.index.center();
            let source_y = cell.filled_elevation_meters;
            let [mouth_x, mouth_z] = mouth_cell.center();
            let mouth_y = if let Some(&mouth_slot) = slots.get(&mouth_cell) {
                cells[mouth_slot].filled_elevation_meters
            } else {
                terrain.sample(mouth_x, mouth_z)?.elevation_meters
            };
            let segment = RiverSegment {
                source_cell: cell.index,
                mouth_cell,
                source: WorldPosition::new(source_x, source_y, source_z),
                mouth: WorldPosition::new(mouth_x, mouth_y, mouth_z),
                drainage_area_square_kilometers: u64_as_f64(cell.flow_accumulation_cells)
                    * cell_area_square_kilometers,
                discharge_cubic_meters_per_second: discharge[slot],
            };
            if !segment.descends_or_is_level() {
                return None;
            }
            segments.push(segment);
        }
        segments.sort_by_key(|segment| segment.source_cell);

        Some(Self {
            world: watershed.world,
            region: watershed.index,
            segments,
        })
    }

    pub fn segments(&self) -> &[RiverSegment] {
        &self.segments
    }

    pub fn segment_from(&self, source: DrainageCellIndex) -> Option<&RiverSegment> {
        self.segments
            .binary_search_by_key(&source, |segment| segment.source_cell)
            .ok()
            .map(|slot| &self.segments[slot])
    }
}

fn local_runoff(world: WorldIdentity, cell: DrainageCellIndex) -> Option<f64> {
    let [x, z] = cell.center();
    let profile = RegionalProfile::sample(world, x, z)?;
    let annual_precipitation_meters = 0.25 + (profile.precipitation * 2.75);
    let runoff_fraction = (0.75 - (profile.mean_temperature * 0.5)).clamp(0.15, 0.75);
    let cell_area_square_meters = DRAINAGE_CELL_EDGE_METERS * DRAINAGE_CELL_EDGE_METERS;
    Some(annual_precipitation_meters * cell_area_square_meters * runoff_fraction / SECONDS_PER_YEAR)
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

#[allow(clippy::cast_precision_loss)]
fn u64_as_f64(value: u64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_coordinates::stable_hash;

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, 2, 0);

    #[test]
    fn river_direction_rejects_uphill_segments() {
        let river = RiverSegment {
            source_cell: DrainageCellIndex::new(0, 0),
            mouth_cell: DrainageCellIndex::new(1, 0),
            source: WorldPosition::new(0.0, 100.0, 0.0),
            mouth: WorldPosition::new(10.0, 101.0, 0.0),
            drainage_area_square_kilometers: 32.0,
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

    #[test]
    fn generated_rivers_are_connected_downhill_channels() {
        let network =
            RiverNetwork::generate(WORLD, WatershedRegionIndex::new(-1, 0)).expect("network");
        assert!(!network.segments().is_empty());
        for segment in network.segments() {
            assert!(segment.descends_or_is_level());
            assert!(segment.discharge_cubic_meters_per_second > 0.0);
            assert!(
                segment.source_cell.x.abs_diff(segment.mouth_cell.x) <= 1
                    && segment.source_cell.z.abs_diff(segment.mouth_cell.z) <= 1
            );
            assert!(segment.drainage_area_square_kilometers >= 32.0);
        }
    }

    #[test]
    fn discharge_does_not_decrease_at_internal_confluences() {
        let network =
            RiverNetwork::generate(WORLD, WatershedRegionIndex::new(0, 0)).expect("network");
        for segment in network.segments() {
            if let Some(downstream) = network.segment_from(segment.mouth_cell) {
                assert!(
                    downstream.discharge_cubic_meters_per_second
                        >= segment.discharge_cubic_meters_per_second
                );
            }
        }
    }

    #[test]
    fn river_generation_is_order_independent() {
        let first_index = WatershedRegionIndex::new(-2, 1);
        let second_index = WatershedRegionIndex::new(-1, 1);
        let first = RiverNetwork::generate(WORLD, first_index).expect("first");
        let second = RiverNetwork::generate(WORLD, second_index).expect("second");
        let second_again = RiverNetwork::generate(WORLD, second_index).expect("second again");
        let first_again = RiverNetwork::generate(WORLD, first_index).expect("first again");

        assert_eq!(first, first_again);
        assert_eq!(second, second_again);
    }

    #[test]
    fn river_network_has_a_golden_fingerprint() {
        let network =
            RiverNetwork::generate(WORLD, WatershedRegionIndex::new(-1, 2)).expect("network");
        let mut words = Vec::new();
        for segment in network.segments().iter().step_by(17) {
            words.extend([
                u64::from_le_bytes(segment.source_cell.x.to_le_bytes()),
                u64::from_le_bytes(segment.source_cell.z.to_le_bytes()),
                u64::from_le_bytes(segment.mouth_cell.x.to_le_bytes()),
                u64::from_le_bytes(segment.mouth_cell.z.to_le_bytes()),
                segment.source.y.to_bits(),
                segment.mouth.y.to_bits(),
                segment.discharge_cubic_meters_per_second.to_bits(),
            ]);
        }
        assert_eq!(
            stable_hash(&words),
            13_240_554_273_518_066_894,
            "changing this value changes generated regional rivers"
        );
    }
}
