use treeline_coordinates::{WorldIdentity, WorldPosition};
use treeline_geography::RegionalProfile;
use treeline_terrain::{DensityField, GroundPlane, Material};

fn main() {
    let world = WorldIdentity::new(0x5eed, 1, 0);
    let position = WorldPosition::new(803_431.2, 77.4, -59_201.9);
    let profile = RegionalProfile::sample(world, position.x, position.z)
        .expect("the built-in inspection coordinate is finite");
    let prototype = GroundPlane {
        surface_height: 80.0,
        material: Material::Rock,
    };
    let terrain = prototype.sample(position);

    println!("Generator Lab scaffold");
    println!("coordinate: {position:?}");
    println!("regional profile: {profile:#?}");
    println!("prototype terrain sample: {terrain:?}");
}
