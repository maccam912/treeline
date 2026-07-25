use treeline_renderer::terrain_tier;
use treeline_voxel::LodLevel;
use treeline_world::GenerationPriority;

fn main() {
    let near_spacing = LodLevel::new(0).spacing_meters();
    let horizon_tier = terrain_tier(30_000.0);
    let first_job = GenerationPriority::Horizon;
    println!(
        "World viewer scaffold: {near_spacing}m near samples, {horizon_tier:?}, first job {first_job:?}"
    );
}
