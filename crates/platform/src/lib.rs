//! Platform boundaries kept outside deterministic simulation and generation.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformKind {
    Desktop,
    Mobile,
    Web,
    Headless,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum JobPriority {
    Critical,
    Visible,
    Background,
}
