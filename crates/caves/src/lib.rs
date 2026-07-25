//! Geological cave-system topology before conversion into terrain subtraction.

use treeline_coordinates::WorldPosition;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaveNodeKind {
    Entrance,
    Passage,
    Chamber,
    Shaft,
    Sump,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaveNode {
    pub position: WorldPosition,
    pub kind: CaveNodeKind,
}

/// A deterministic cave graph with edges expressed as node-index pairs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaveGraph {
    pub nodes: Vec<CaveNode>,
    pub edges: Vec<(usize, usize)>,
}

impl CaveGraph {
    pub fn has_valid_edges(&self) -> bool {
        self.edges
            .iter()
            .all(|&(from, to)| from < self.nodes.len() && to < self.nodes.len() && from != to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_validation_rejects_missing_nodes() {
        let graph = CaveGraph {
            nodes: vec![CaveNode {
                position: WorldPosition::new(0.0, 0.0, 0.0),
                kind: CaveNodeKind::Entrance,
            }],
            edges: vec![(0, 1)],
        };
        assert!(!graph.has_valid_edges());
    }
}
