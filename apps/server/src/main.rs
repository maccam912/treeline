use treeline_platform::PlatformKind;
use treeline_protocol::{PROTOCOL_VERSION, accepts};
use treeline_world::RegionState;

fn main() {
    let platform = PlatformKind::Headless;
    let initial_region_state = RegionState::Ungenerated;
    assert!(accepts(PROTOCOL_VERSION));
    println!("Treeline server scaffold: {platform:?}, regions begin {initial_region_state:?}");
}
