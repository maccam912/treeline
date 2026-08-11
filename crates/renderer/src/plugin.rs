//! Bevy plugin and material palette for the measured world.

use bevy::prelude::*;

#[derive(Resource, Debug)]
pub struct WorldMaterials {
    pub terrain: Handle<StandardMaterial>,
    pub water: Handle<StandardMaterial>,
    pub trees: Handle<StandardMaterial>,
}

#[derive(Debug, Default)]
pub struct TreelineRenderPlugin;

impl Plugin for TreelineRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, create_world_materials);
    }
}

fn create_world_materials(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    let terrain = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.92,
        reflectance: 0.18,
        ..default()
    });
    let water = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        metallic: 0.05,
        perceptual_roughness: 0.16,
        reflectance: 0.7,
        ..default()
    });
    let trees = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.88,
        reflectance: 0.12,
        ..default()
    });
    commands.insert_resource(WorldMaterials {
        terrain,
        water,
        trees,
    });
}
