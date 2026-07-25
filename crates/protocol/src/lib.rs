//! Network contracts shared by clients and authoritative hosts.

use treeline_coordinates::{WorldIdentity, WorldPosition};

/// Increment when the wire contract changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClientMessage {
    Join { protocol_version: u32 },
    Move { position: WorldPosition },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ServerMessage {
    Welcome {
        protocol_version: u32,
        world: WorldIdentity,
    },
    RejectVersion {
        supported: u32,
    },
}

pub const fn accepts(version: u32) -> bool {
    version == PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incompatible_protocol_versions_are_rejected() {
        assert!(accepts(PROTOCOL_VERSION));
        assert!(!accepts(PROTOCOL_VERSION + 1));
    }
}
