use treeline_platform::PlatformKind;
use treeline_renderer::terrain_tier;
use treeline_simulation::SurvivalSettings;

fn main() {
    let platform = PlatformKind::Desktop;
    let survival = SurvivalSettings::default();
    let starting_tier = terrain_tier(0.0);
    println!("Treeline client scaffold: {platform:?}, {starting_tier:?}, survival {survival:?}");
}
