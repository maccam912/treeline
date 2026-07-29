//! Deterministic geological cave systems and their terrain-subtraction field.
//!
//! A cave region owns at most one compact graph. Systems are generated from
//! world identity, regional geology, climate, and the explicit surface field;
//! evaluating neighbouring regions never depends on generation order.

use std::collections::VecDeque;

use treeline_coordinates::{CellIndex, WorldIdentity, WorldPosition, stable_hash};
use treeline_geography::RegionalProfile;
use treeline_terrain::SurfaceField;

/// Generator version that first subtracts geological cave systems from terrain.
pub const CAVE_GENERATOR_VERSION: u32 = 16;

const DOMAIN_CAVE_SYSTEM: u64 = 0x4341_5645_5359_5354;
const DOMAIN_CAVE_GEOLOGY: u64 = 0x4341_5645_4745_4f4c;

/// Horizontal ownership cell for a cave system.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CaveRegionIndex {
    pub x: i64,
    pub z: i64,
}

impl CaveRegionIndex {
    pub const EDGE_METERS: f64 = 1_024.0;

    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    pub fn containing(x: f64, z: f64) -> Option<Self> {
        let cell = CellIndex::containing(x, z, 0, Self::EDGE_METERS)?;
        Some(Self::new(cell.x, cell.z))
    }

    pub fn center(self) -> [f64; 2] {
        [
            (index_as_f64(self.x) + 0.5) * Self::EDGE_METERS,
            (index_as_f64(self.z) + 0.5) * Self::EDGE_METERS,
        ]
    }

    pub fn generation_key(self, world: WorldIdentity) -> u64 {
        CellIndex::new(self.x, self.z, 0).generation_key(world, DOMAIN_CAVE_SYSTEM)
    }
}

/// Geological process responsible for a connected cave system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaveFamily {
    Karst,
    LavaTube,
    Fault,
    Sea,
    Talus,
    Glacial,
    Erosional,
}

impl CaveFamily {
    pub const ALL: [Self; 7] = [
        Self::Karst,
        Self::LavaTube,
        Self::Fault,
        Self::Sea,
        Self::Talus,
        Self::Glacial,
        Self::Erosional,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Karst => "karst",
            Self::LavaTube => "lava tube",
            Self::Fault => "fault",
            Self::Sea => "sea cave",
            Self::Talus => "talus",
            Self::Glacial => "glacial",
            Self::Erosional => "erosional",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaveNodeKind {
    Entrance,
    Sinkhole,
    Passage,
    Chamber,
    Shaft,
    Sump,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveNode {
    pub position: WorldPosition,
    pub kind: CaveNodeKind,
    pub radius_meters: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveEdge {
    pub from: usize,
    pub to: usize,
    pub radius_meters: f64,
}

/// One descending underground-water reach carried by a cave edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UndergroundRiver {
    pub edge_index: usize,
    pub flow_from: usize,
    pub flow_to: usize,
    pub discharge_cubic_meters_per_second: f64,
    pub surface_elevation_meters: f64,
    pub width_meters: f64,
}

/// A deterministic cave graph with explicit passage radii and water reaches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaveGraph {
    pub nodes: Vec<CaveNode>,
    pub edges: Vec<CaveEdge>,
    pub underground_rivers: Vec<UndergroundRiver>,
}

impl CaveGraph {
    pub fn has_valid_edges(&self) -> bool {
        self.edges.iter().all(|edge| {
            edge.from < self.nodes.len()
                && edge.to < self.nodes.len()
                && edge.from != edge.to
                && edge.radius_meters.is_finite()
                && edge.radius_meters > 0.0
        }) && self.underground_rivers.iter().all(|river| {
            let Some(edge) = self.edges.get(river.edge_index) else {
                return false;
            };
            ((river.flow_from == edge.from && river.flow_to == edge.to)
                || (river.flow_from == edge.to && river.flow_to == edge.from))
                && river.discharge_cubic_meters_per_second.is_finite()
                && river.discharge_cubic_meters_per_second > 0.0
                && river.width_meters.is_finite()
                && river.width_meters > 0.0
                && river.surface_elevation_meters.is_finite()
        })
    }

    pub fn is_connected(&self) -> bool {
        if self.nodes.is_empty() {
            return true;
        }
        let mut visited = vec![false; self.nodes.len()];
        let mut pending = VecDeque::from([0]);
        visited[0] = true;
        while let Some(node) = pending.pop_front() {
            for edge in &self.edges {
                let neighbour = if edge.from == node {
                    Some(edge.to)
                } else if edge.to == node {
                    Some(edge.from)
                } else {
                    None
                };
                if let Some(neighbour) = neighbour
                    && !visited[neighbour]
                {
                    visited[neighbour] = true;
                    pending.push_back(neighbour);
                }
            }
        }
        visited.into_iter().all(|node| node)
    }
}

/// Environmental controls that select and shape a cave family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveGeology {
    pub karst: f64,
    pub fracture: f64,
    pub permeability: f64,
    pub volcanism: f64,
    pub glacial_influence: f64,
    pub coastal_influence: f64,
    pub recharge: f64,
    pub surface_drainage_gradient: f64,
}

/// Axis-aligned extent of a cave subtraction field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveBounds {
    pub min: WorldPosition,
    pub max: WorldPosition,
}

impl CaveBounds {
    pub fn intersects_horizontal(self, min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> bool {
        self.max.x >= min_x && self.min.x <= max_x && self.max.z >= min_z && self.min.z <= max_z
    }
}

/// Explainable result from sampling the cave subtraction field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveInfluence {
    /// Positive inside cave air and negative outside the cave boundary.
    pub void_density: f64,
    pub family: CaveFamily,
    pub nearest_kind: CaveNodeKind,
    pub system_key: u64,
}

/// A surface connection suitable for discovery, inspection, or player travel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveEntrance {
    pub position: WorldPosition,
    pub kind: CaveNodeKind,
    pub family: CaveFamily,
    pub system_key: u64,
}

/// One connected, geologically selected subterranean system.
#[derive(Clone, Debug, PartialEq)]
pub struct CaveSystem {
    pub region: CaveRegionIndex,
    pub system_key: u64,
    pub family: CaveFamily,
    pub geology: CaveGeology,
    pub water_table_elevation_meters: f64,
    pub graph: CaveGraph,
    pub bounds: CaveBounds,
}

impl CaveSystem {
    /// Generates the cave owned by `region`, if its correlated geological
    /// conditions produce one for this generator version.
    #[allow(clippy::too_many_lines)]
    pub fn generate(
        world: WorldIdentity,
        region: CaveRegionIndex,
        surface: &(impl SurfaceField + ?Sized),
    ) -> Option<Self> {
        if world.generator_version < CAVE_GENERATOR_VERSION {
            return None;
        }
        let key = region.generation_key(world);
        let center = region.center();
        let profile = RegionalProfile::sample(world, center[0], center[1])?;
        let center_surface = surface.surface_height(center[0], center[1])?;
        let mut geology = cave_geology(world, region, profile, center_surface);
        let family_weights = family_weights(profile, geology);
        let maximum_suitability = family_weights.iter().copied().fold(0.0_f64, f64::max);
        let occurrence_probability = 0.12 + (maximum_suitability * 0.48);
        if unit(key, 0) >= occurrence_probability {
            return None;
        }
        let family = select_family(family_weights, unit(key, 1));
        let drainage_sample_radius = 256.0;
        let west = surface
            .surface_height(center[0] - drainage_sample_radius, center[1])
            .unwrap_or(center_surface);
        let east = surface
            .surface_height(center[0] + drainage_sample_radius, center[1])
            .unwrap_or(center_surface);
        let north = surface
            .surface_height(center[0], center[1] - drainage_sample_radius)
            .unwrap_or(center_surface);
        let south = surface
            .surface_height(center[0], center[1] + drainage_sample_radius)
            .unwrap_or(center_surface);
        let sample_span = drainage_sample_radius * 2.0;
        let gradient = [(east - west) / sample_span, (south - north) / sample_span];
        geology.surface_drainage_gradient = libm::hypot(gradient[0], gradient[1]);

        let margin = 190.0;
        let span = CaveRegionIndex::EDGE_METERS - (margin * 2.0);
        let anchor_x = (index_as_f64(region.x) * CaveRegionIndex::EDGE_METERS)
            + margin
            + (unit(key, 2) * span);
        let anchor_z = (index_as_f64(region.z) * CaveRegionIndex::EDGE_METERS)
            + margin
            + (unit(key, 3) * span);
        let anchor_surface = surface.surface_height(anchor_x, anchor_z)?;
        let fracture_direction = unit(key, 4) * std::f64::consts::TAU;
        let direction = if geology.surface_drainage_gradient > 0.002 {
            libm::atan2(-gradient[1], -gradient[0])
                + ((unit(key, 52) - 0.5) * (1.4 - geology.fracture))
        } else {
            fracture_direction
        };
        let forward = [libm::cos(direction), libm::sin(direction)];
        let right = [-forward[1], forward[0]];
        let family_scale = family_scale(family);
        let base_radius = (2.6 + (unit(key, 5) * 3.8)) * family_scale;
        let initial_depth = family_base_depth(family)
            + (unit(key, 6) * family_depth_variation(family))
            + (geology.fracture * 12.0);
        let chain_step = (38.0 + (unit(key, 7) * 38.0)) * family_passage_length_scale(family);
        let descent_rate = family_descent_rate(family);
        let entrance_is_sinkhole = unit(key, 8) < 0.42;

        let entrance_offset = -chain_step * 0.85;
        let entrance_x = anchor_x + (forward[0] * entrance_offset);
        let entrance_z = anchor_z + (forward[1] * entrance_offset);
        let entrance_surface = surface
            .surface_height(entrance_x, entrance_z)
            .unwrap_or(anchor_surface);
        let mut nodes = vec![
            CaveNode {
                position: WorldPosition::new(entrance_x, entrance_surface + 0.6, entrance_z),
                kind: if entrance_is_sinkhole {
                    CaveNodeKind::Sinkhole
                } else {
                    CaveNodeKind::Entrance
                },
                radius_meters: base_radius * if entrance_is_sinkhole { 1.25 } else { 0.9 },
            },
            CaveNode {
                position: WorldPosition::new(anchor_x, anchor_surface - initial_depth, anchor_z),
                kind: CaveNodeKind::Passage,
                radius_meters: base_radius,
            },
        ];

        for ordinal in 0_u32..4 {
            let distance = chain_step * (f64::from(ordinal) + 1.0);
            let bend = (unit(key, 20 + u64::from(ordinal)) - 0.5) * chain_step * 0.7;
            let x = anchor_x + (forward[0] * distance) + (right[0] * bend);
            let z = anchor_z + (forward[1] * distance) + (right[1] * bend);
            let descent = initial_depth
                + (distance * (descent_rate + (geology.recharge * 0.045)))
                + (unit(key, 30 + u64::from(ordinal)) * 5.0);
            let local_surface = surface.surface_height(x, z).unwrap_or(anchor_surface);
            let kind = match ordinal {
                1 => CaveNodeKind::Chamber,
                3 => CaveNodeKind::Sump,
                _ => CaveNodeKind::Passage,
            };
            let radius = if kind == CaveNodeKind::Chamber {
                base_radius * (2.0 + unit(key, 40))
            } else if kind == CaveNodeKind::Sump {
                base_radius * 1.35
            } else {
                base_radius * (0.82 + (unit(key, 41 + u64::from(ordinal)) * 0.45))
            };
            nodes.push(CaveNode {
                position: WorldPosition::new(x, local_surface - descent, z),
                kind,
                radius_meters: radius,
            });
        }

        let sinkhole_distance = chain_step * 1.65;
        let sinkhole_x = anchor_x + (forward[0] * sinkhole_distance) + (right[0] * chain_step);
        let sinkhole_z = anchor_z + (forward[1] * sinkhole_distance) + (right[1] * chain_step);
        let sinkhole_surface = surface
            .surface_height(sinkhole_x, sinkhole_z)
            .unwrap_or(anchor_surface);
        nodes.push(CaveNode {
            position: WorldPosition::new(sinkhole_x, sinkhole_surface + 0.5, sinkhole_z),
            kind: if entrance_is_sinkhole {
                CaveNodeKind::Entrance
            } else {
                CaveNodeKind::Sinkhole
            },
            radius_meters: base_radius * 1.15,
        });

        let water_table_elevation_meters =
            anchor_surface - initial_depth - 12.0 - ((1.0 - geology.recharge) * 28.0);
        if let Some(sump) = nodes.get_mut(5) {
            sump.position.y = sump
                .position
                .y
                .min(water_table_elevation_meters + (sump.radius_meters * 0.45));
        }

        let shaft_parent = nodes[3];
        nodes.push(CaveNode {
            position: WorldPosition::new(
                shaft_parent.position.x + (right[0] * base_radius),
                shaft_parent.position.y - 18.0 - (unit(key, 50) * 28.0),
                shaft_parent.position.z + (right[1] * base_radius),
            ),
            kind: CaveNodeKind::Shaft,
            radius_meters: base_radius * 0.78,
        });

        let mut edges = vec![
            cave_edge(0, 1, base_radius * 0.82),
            cave_edge(1, 2, base_radius),
            cave_edge(2, 3, base_radius * 1.05),
            cave_edge(3, 4, base_radius),
            cave_edge(4, 5, base_radius * 0.9),
            cave_edge(6, 3, base_radius * 0.85),
            cave_edge(3, 7, base_radius * 0.7),
            cave_edge(7, 4, base_radius * 0.75),
        ];
        if matches!(family, CaveFamily::Karst | CaveFamily::Fault) && unit(key, 51) > 0.45 {
            edges.push(cave_edge(2, 4, base_radius * 0.72));
        }

        let mut underground_rivers = Vec::new();
        if geology.recharge > 0.28
            || matches!(
                family,
                CaveFamily::Karst | CaveFamily::Glacial | CaveFamily::Erosional
            )
        {
            for edge_index in [1_usize, 2, 3, 4] {
                let edge = edges[edge_index];
                let from = nodes[edge.from];
                let to = nodes[edge.to];
                let (flow_from, flow_to) = if from.position.y >= to.position.y {
                    (edge.from, edge.to)
                } else {
                    (edge.to, edge.from)
                };
                let lower = nodes[flow_to];
                underground_rivers.push(UndergroundRiver {
                    edge_index,
                    flow_from,
                    flow_to,
                    discharge_cubic_meters_per_second: 0.08
                        + (geology.recharge
                            * geology.permeability
                            * (0.65 + (geology.surface_drainage_gradient * 8.0).min(1.35))
                            * 8.0),
                    surface_elevation_meters: lower.position.y - (lower.radius_meters * 0.56),
                    width_meters: (0.7 + (geology.recharge * 2.8)).min(edge.radius_meters * 1.4),
                });
            }
        }

        let graph = CaveGraph {
            nodes,
            edges,
            underground_rivers,
        };
        if !graph.has_valid_edges() || !graph.is_connected() {
            return None;
        }
        let bounds = graph_bounds(&graph);
        Some(Self {
            region,
            system_key: key,
            family,
            geology,
            water_table_elevation_meters,
            graph,
            bounds,
        })
    }

    /// Samples the analytic union of node spheres and passage capsules.
    pub fn influence_at(&self, position: WorldPosition) -> CaveInfluence {
        const OUTSIDE_DENSITY_METERS: f64 = -4.0;
        let mut strongest = OUTSIDE_DENSITY_METERS;
        let mut nearest_kind = CaveNodeKind::Passage;
        for node in &self.graph.nodes {
            let reach = node.radius_meters - OUTSIDE_DENSITY_METERS;
            if (position.x - node.position.x).abs() > reach
                || (position.y - node.position.y).abs() > reach
                || (position.z - node.position.z).abs() > reach
            {
                continue;
            }
            let density = node.radius_meters - distance(position, node.position);
            if density > strongest {
                strongest = density;
                nearest_kind = node.kind;
            }
        }
        for edge in &self.graph.edges {
            let start = self.graph.nodes[edge.from];
            let end = self.graph.nodes[edge.to];
            let reach = edge.radius_meters - OUTSIDE_DENSITY_METERS;
            if position.x < start.position.x.min(end.position.x) - reach
                || position.x > start.position.x.max(end.position.x) + reach
                || position.y < start.position.y.min(end.position.y) - reach
                || position.y > start.position.y.max(end.position.y) + reach
                || position.z < start.position.z.min(end.position.z) - reach
                || position.z > start.position.z.max(end.position.z) + reach
            {
                continue;
            }
            let density =
                edge.radius_meters - distance_to_segment(position, start.position, end.position);
            if density > strongest {
                strongest = density;
                nearest_kind =
                    if start.kind == CaveNodeKind::Shaft || end.kind == CaveNodeKind::Shaft {
                        CaveNodeKind::Shaft
                    } else {
                        CaveNodeKind::Passage
                    };
            }
        }
        CaveInfluence {
            void_density: strongest,
            family: self.family,
            nearest_kind,
            system_key: self.system_key,
        }
    }

    /// Horizontal distance to the nearest passage footprint. Negative values
    /// are inside the top-down projection of the system.
    pub fn horizontal_distance_at(&self, x: f64, z: f64) -> f64 {
        let mut nearest = f64::INFINITY;
        for node in &self.graph.nodes {
            nearest = nearest
                .min(libm::hypot(x - node.position.x, z - node.position.z) - node.radius_meters);
        }
        for edge in &self.graph.edges {
            let start = self.graph.nodes[edge.from].position;
            let end = self.graph.nodes[edge.to].position;
            nearest = nearest.min(
                distance_to_segment_2d(x, z, start.x, start.z, end.x, end.z) - edge.radius_meters,
            );
        }
        nearest
    }

    pub fn entrances(&self) -> impl Iterator<Item = CaveEntrance> + '_ {
        self.graph.nodes.iter().filter_map(|node| {
            matches!(node.kind, CaveNodeKind::Entrance | CaveNodeKind::Sinkhole).then_some(
                CaveEntrance {
                    position: node.position,
                    kind: node.kind,
                    family: self.family,
                    system_key: self.system_key,
                },
            )
        })
    }

    /// Tight vertical extent of cave primitives that can affect one
    /// horizontal mesh footprint.
    pub fn vertical_bounds_in(
        &self,
        min_x: f64,
        min_z: f64,
        max_x: f64,
        max_z: f64,
    ) -> Option<(f64, f64)> {
        const SAMPLE_MARGIN_METERS: f64 = 4.0;
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for node in &self.graph.nodes {
            let nearest_x = node.position.x.clamp(min_x, max_x);
            let nearest_z = node.position.z.clamp(min_z, max_z);
            if libm::hypot(node.position.x - nearest_x, node.position.z - nearest_z)
                <= node.radius_meters + SAMPLE_MARGIN_METERS
            {
                minimum = minimum.min(node.position.y - node.radius_meters);
                maximum = maximum.max(node.position.y + node.radius_meters);
            }
        }
        for edge in &self.graph.edges {
            let start = self.graph.nodes[edge.from].position;
            let end = self.graph.nodes[edge.to].position;
            let reach = edge.radius_meters + SAMPLE_MARGIN_METERS;
            if start.x.max(end.x) + reach < min_x
                || start.x.min(end.x) - reach > max_x
                || start.z.max(end.z) + reach < min_z
                || start.z.min(end.z) - reach > max_z
            {
                continue;
            }
            minimum = minimum.min(start.y.min(end.y) - edge.radius_meters);
            maximum = maximum.max(start.y.max(end.y) + edge.radius_meters);
        }
        minimum.is_finite().then_some((minimum, maximum))
    }

    /// Stable regression fingerprint for the complete generated artifact.
    pub fn fingerprint(&self) -> u64 {
        let mut words = vec![
            self.system_key,
            self.family as u64,
            self.graph.nodes.len() as u64,
            self.graph.edges.len() as u64,
            self.graph.underground_rivers.len() as u64,
            self.water_table_elevation_meters.to_bits(),
        ];
        for node in &self.graph.nodes {
            words.extend([
                node.position.x.to_bits(),
                node.position.y.to_bits(),
                node.position.z.to_bits(),
                node.radius_meters.to_bits(),
                node.kind as u64,
            ]);
        }
        for edge in &self.graph.edges {
            words.extend([
                edge.from as u64,
                edge.to as u64,
                edge.radius_meters.to_bits(),
            ]);
        }
        for river in &self.graph.underground_rivers {
            words.extend([
                river.edge_index as u64,
                river.flow_from as u64,
                river.flow_to as u64,
                river.discharge_cubic_meters_per_second.to_bits(),
                river.surface_elevation_meters.to_bits(),
                river.width_meters.to_bits(),
            ]);
        }
        stable_hash(&words)
    }
}

fn cave_geology(
    world: WorldIdentity,
    region: CaveRegionIndex,
    profile: RegionalProfile,
    surface_elevation: f64,
) -> CaveGeology {
    let key = CellIndex::new(region.x, region.z, 0).generation_key(world, DOMAIN_CAVE_GEOLOGY);
    let fracture = unit(key, 0);
    let permeability = (profile.karst_probability * 0.55 + unit(key, 1) * 0.45).clamp(0.0, 1.0);
    let volcanism = (unit(key, 2) * (0.35 + ((1.0 - profile.erosion_age) * 0.65))).clamp(0.0, 1.0);
    let glacial_influence = ((1.0 - profile.mean_temperature) * profile.uplift).clamp(0.0, 1.0);
    let coastal_influence =
        (1.0 - (surface_elevation.abs() / 180.0).clamp(0.0, 1.0)) * profile.precipitation;
    let recharge = (profile.precipitation * (0.35 + (permeability * 0.65))).clamp(0.0, 1.0);
    CaveGeology {
        karst: profile.karst_probability,
        fracture,
        permeability,
        volcanism,
        glacial_influence,
        coastal_influence,
        recharge,
        surface_drainage_gradient: 0.0,
    }
}

fn family_weights(profile: RegionalProfile, geology: CaveGeology) -> [f64; 7] {
    [
        geology.karst * geology.permeability * (0.4 + (geology.recharge * 0.6)),
        geology.volcanism * (0.45 + ((1.0 - profile.erosion_age) * 0.55)),
        geology.fracture * (0.35 + (profile.rock_hardness * 0.65)),
        geology.coastal_influence,
        profile.uplift * profile.erosion_age * (0.3 + (profile.rock_hardness * 0.7)),
        geology.glacial_influence,
        profile.precipitation * (0.35 + ((1.0 - profile.rock_hardness) * 0.65)),
    ]
}

fn select_family(weights: [f64; 7], selection: f64) -> CaveFamily {
    let total = weights.iter().sum::<f64>();
    if total <= f64::EPSILON {
        return CaveFamily::Erosional;
    }
    let target = selection * total;
    let mut accumulated = 0.0;
    for (family, weight) in CaveFamily::ALL.into_iter().zip(weights) {
        accumulated += weight;
        if target <= accumulated {
            return family;
        }
    }
    CaveFamily::Erosional
}

const fn family_scale(family: CaveFamily) -> f64 {
    match family {
        CaveFamily::Karst => 1.25,
        CaveFamily::LavaTube => 1.15,
        CaveFamily::Fault => 0.78,
        CaveFamily::Sea => 1.3,
        CaveFamily::Talus => 0.72,
        CaveFamily::Glacial => 1.18,
        CaveFamily::Erosional => 1.0,
    }
}

const fn family_base_depth(family: CaveFamily) -> f64 {
    match family {
        CaveFamily::Karst => 28.0,
        CaveFamily::LavaTube => 20.0,
        CaveFamily::Fault => 34.0,
        CaveFamily::Sea => 9.0,
        CaveFamily::Talus => 11.0,
        CaveFamily::Glacial => 14.0,
        CaveFamily::Erosional => 22.0,
    }
}

const fn family_depth_variation(family: CaveFamily) -> f64 {
    match family {
        CaveFamily::Karst | CaveFamily::Fault => 34.0,
        CaveFamily::LavaTube | CaveFamily::Erosional => 22.0,
        CaveFamily::Sea | CaveFamily::Talus | CaveFamily::Glacial => 13.0,
    }
}

const fn family_passage_length_scale(family: CaveFamily) -> f64 {
    match family {
        CaveFamily::Karst => 1.15,
        CaveFamily::LavaTube => 1.42,
        CaveFamily::Fault => 0.86,
        CaveFamily::Sea => 0.72,
        CaveFamily::Talus => 0.58,
        CaveFamily::Glacial => 0.82,
        CaveFamily::Erosional => 1.0,
    }
}

const fn family_descent_rate(family: CaveFamily) -> f64 {
    match family {
        CaveFamily::Karst => 0.045,
        CaveFamily::LavaTube => 0.012,
        CaveFamily::Fault => 0.12,
        CaveFamily::Sea => 0.008,
        CaveFamily::Talus => 0.07,
        CaveFamily::Glacial => 0.025,
        CaveFamily::Erosional => 0.055,
    }
}

const fn cave_edge(from: usize, to: usize, radius_meters: f64) -> CaveEdge {
    CaveEdge {
        from,
        to,
        radius_meters,
    }
}

fn graph_bounds(graph: &CaveGraph) -> CaveBounds {
    let mut min = WorldPosition::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = WorldPosition::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for node in &graph.nodes {
        min.x = min.x.min(node.position.x - node.radius_meters);
        min.y = min.y.min(node.position.y - node.radius_meters);
        min.z = min.z.min(node.position.z - node.radius_meters);
        max.x = max.x.max(node.position.x + node.radius_meters);
        max.y = max.y.max(node.position.y + node.radius_meters);
        max.z = max.z.max(node.position.z + node.radius_meters);
    }
    CaveBounds { min, max }
}

fn distance(point: WorldPosition, other: WorldPosition) -> f64 {
    libm::hypot(
        libm::hypot(point.x - other.x, point.y - other.y),
        point.z - other.z,
    )
}

fn distance_to_segment(point: WorldPosition, start: WorldPosition, end: WorldPosition) -> f64 {
    let segment = [end.x - start.x, end.y - start.y, end.z - start.z];
    let offset = [point.x - start.x, point.y - start.y, point.z - start.z];
    let length_squared =
        (segment[0] * segment[0]) + (segment[1] * segment[1]) + (segment[2] * segment[2]);
    if length_squared <= f64::EPSILON {
        return distance(point, start);
    }
    let amount = ((offset[0] * segment[0] + offset[1] * segment[1] + offset[2] * segment[2])
        / length_squared)
        .clamp(0.0, 1.0);
    distance(
        point,
        WorldPosition::new(
            start.x + (segment[0] * amount),
            start.y + (segment[1] * amount),
            start.z + (segment[2] * amount),
        ),
    )
}

fn distance_to_segment_2d(
    x: f64,
    z: f64,
    start_x: f64,
    start_z: f64,
    end_x: f64,
    end_z: f64,
) -> f64 {
    let segment = [end_x - start_x, end_z - start_z];
    let offset = [x - start_x, z - start_z];
    let length_squared = (segment[0] * segment[0]) + (segment[1] * segment[1]);
    if length_squared <= f64::EPSILON {
        return libm::hypot(offset[0], offset[1]);
    }
    let amount =
        ((offset[0] * segment[0] + offset[1] * segment[1]) / length_squared).clamp(0.0, 1.0);
    libm::hypot(
        x - (start_x + (segment[0] * amount)),
        z - (start_z + (segment[1] * amount)),
    )
}

fn unit(key: u64, ordinal: u64) -> f64 {
    let hash = stable_hash(&[key, ordinal]);
    hash53_as_f64(hash >> 11) / 9_007_199_254_740_991.0
}

#[allow(clippy::cast_precision_loss)]
fn index_as_f64(index: i64) -> f64 {
    index as f64
}

#[allow(clippy::cast_precision_loss)]
fn hash53_as_f64(hash: u64) -> f64 {
    hash as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use treeline_terrain::{GroundPlane, Material};

    const WORLD: WorldIdentity = WorldIdentity::new(0x5eed, CAVE_GENERATOR_VERSION, 0);
    const SURFACE: GroundPlane = GroundPlane {
        surface_height: 120.0,
        material: Material::Rock,
    };

    #[test]
    fn edge_validation_rejects_missing_nodes() {
        let graph = CaveGraph {
            nodes: vec![CaveNode {
                position: WorldPosition::new(0.0, 0.0, 0.0),
                kind: CaveNodeKind::Entrance,
                radius_meters: 3.0,
            }],
            edges: vec![cave_edge(0, 1, 2.0)],
            underground_rivers: Vec::new(),
        };
        assert!(!graph.has_valid_edges());
    }

    #[test]
    fn generated_graphs_are_connected_and_include_surface_connections_and_shafts() {
        let system = first_system(WORLD);
        assert!(system.graph.has_valid_edges());
        assert!(system.graph.is_connected());
        assert!(
            system
                .graph
                .nodes
                .iter()
                .any(|node| node.kind == CaveNodeKind::Entrance)
        );
        assert!(
            system
                .graph
                .nodes
                .iter()
                .any(|node| node.kind == CaveNodeKind::Sinkhole)
        );
        assert!(
            system
                .graph
                .nodes
                .iter()
                .any(|node| node.kind == CaveNodeKind::Shaft)
        );
    }

    #[test]
    fn generation_is_deterministic_and_order_independent() {
        let index = first_system(WORLD).region;
        let expected = CaveSystem::generate(WORLD, index, &SURFACE).expect("known cave");
        let neighbour = CaveRegionIndex::new(index.x + 1, index.z - 1);
        let _ = CaveSystem::generate(WORLD, neighbour, &SURFACE);
        let repeated = CaveSystem::generate(WORLD, index, &SURFACE).expect("same cave");
        assert_eq!(expected, repeated);
        assert_eq!(expected.fingerprint(), repeated.fingerprint());
    }

    #[test]
    fn negative_region_boundaries_are_half_open() {
        assert_eq!(
            CaveRegionIndex::containing(-0.01, -1_024.0),
            Some(CaveRegionIndex::new(-1, -1))
        );
        assert_eq!(
            CaveRegionIndex::containing(1_024.0, 1_023.99),
            Some(CaveRegionIndex::new(1, 0))
        );
    }

    #[test]
    fn old_worlds_have_no_caves() {
        let old = WorldIdentity::new(0x5eed, CAVE_GENERATOR_VERSION - 1, 0);
        assert!(CaveSystem::generate(old, CaveRegionIndex::new(0, 0), &SURFACE).is_none());
    }

    #[test]
    fn cave_void_is_positive_at_nodes_and_negative_outside_bounds() {
        let system = first_system(WORLD);
        let node = system.graph.nodes[1];
        assert!(system.influence_at(node.position).void_density > 0.0);
        let outside = WorldPosition::new(
            system.bounds.max.x + 100.0,
            system.bounds.max.y + 100.0,
            system.bounds.max.z + 100.0,
        );
        assert!(system.influence_at(outside).void_density < 0.0);
    }

    #[test]
    fn underground_rivers_follow_graph_edges_downhill() {
        let system = (-12..=12)
            .flat_map(|z| (-12..=12).map(move |x| CaveRegionIndex::new(x, z)))
            .filter_map(|index| CaveSystem::generate(WORLD, index, &SURFACE))
            .find(|system| !system.graph.underground_rivers.is_empty())
            .expect("test area contains a wet cave");
        for river in &system.graph.underground_rivers {
            let from = system.graph.nodes[river.flow_from];
            let to = system.graph.nodes[river.flow_to];
            assert!(from.position.y >= to.position.y);
            assert!(river.surface_elevation_meters < to.position.y);
        }
    }

    #[test]
    fn geological_family_selector_can_reach_every_supported_family() {
        for (index, expected) in CaveFamily::ALL.into_iter().enumerate() {
            let mut weights = [0.0; 7];
            weights[index] = 1.0;
            assert_eq!(select_family(weights, 0.5), expected);
        }
    }

    #[test]
    fn generated_world_area_contains_every_geological_family() {
        let mut found = [false; 7];
        'search: for z in -64..=64 {
            for x in -64..=64 {
                if let Some(system) =
                    CaveSystem::generate(WORLD, CaveRegionIndex::new(x, z), &SURFACE)
                {
                    found[system.family as usize] = true;
                    if found.into_iter().all(|family| family) {
                        break 'search;
                    }
                }
            }
        }
        assert!(
            found.into_iter().all(|family| family),
            "the regression area should exercise every cave family: {found:?}"
        );
    }

    fn first_system(world: WorldIdentity) -> CaveSystem {
        for radius in 0_i64..24 {
            for z in -radius..=radius {
                for x in -radius..=radius {
                    if let Some(system) =
                        CaveSystem::generate(world, CaveRegionIndex::new(x, z), &SURFACE)
                    {
                        return system;
                    }
                }
            }
        }
        panic!("test world should generate a cave");
    }
}
