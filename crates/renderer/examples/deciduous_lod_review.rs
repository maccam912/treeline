//! Reproducible broadleaf LOD review scene.
//!
//! `TREELINE_REVIEW_DETAIL=far` selects the silhouette tier and
//! `TREELINE_REVIEW_PATH=/tmp/far.png` chooses the capture path.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::window::WindowResolution;
use treeline_ecology::{
    ForestComposition, GrowthConditions, Stand, TreeCondition, TreeFunctionalGroup, grow_tree,
};
use treeline_renderer::{TreeMeshDetail, TreelineRenderPlugin, WorldMaterials, prepare_trees};

#[derive(Resource)]
struct CapturePath(PathBuf);

#[derive(Resource)]
struct ReviewDetail(TreeMeshDetail);

fn main() {
    let detail = match std::env::var("TREELINE_REVIEW_DETAIL").as_deref() {
        Ok("far") => TreeMeshDetail::Silhouette,
        _ => TreeMeshDetail::Simplified,
    };
    let path = std::env::var_os("TREELINE_REVIEW_PATH")
        .map_or_else(|| PathBuf::from("deciduous-lod-review.png"), PathBuf::from);
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Treeline deciduous LOD review".into(),
                resolution: WindowResolution::new(1_280, 720),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(TreelineRenderPlugin)
        .insert_resource(ClearColor(Color::srgb(0.48, 0.68, 0.76)))
        .insert_resource(CapturePath(path))
        .insert_resource(ReviewDetail(detail))
        .add_systems(Startup, setup)
        .add_systems(Update, capture_after_settle)
        .run();
}

#[allow(clippy::needless_pass_by_value)]
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world_materials: Res<WorldMaterials>,
    detail: Res<ReviewDetail>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(3_200.0, 3_200.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.19, 0.25, 0.13),
            perceptual_roughness: 1.0,
            ..default()
        })),
    ));

    let far = detail.0 == TreeMeshDetail::Silhouette;
    let default_distance = if far { 1_320.0 } else { 220.0 };
    let distance = std::env::var("TREELINE_REVIEW_DISTANCE")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(default_distance);
    let spacing = if far { 34.0 } else { 15.0 };
    let single = std::env::var_os("TREELINE_REVIEW_SINGLE").is_some();
    let grove = std::env::var_os("TREELINE_REVIEW_GROVE").is_some();
    let rows = if grove {
        (0..6)
            .map(|row| {
                let group = if row % 3 == 2 {
                    TreeFunctionalGroup::ColdDeciduous
                } else {
                    TreeFunctionalGroup::TemperateBroadleaf
                };
                (group, -(f64::from(row) * 13.0))
            })
            .collect::<Vec<_>>()
    } else {
        vec![
            (TreeFunctionalGroup::TemperateBroadleaf, 0.0),
            (TreeFunctionalGroup::ColdDeciduous, -22.0),
        ]
    };
    for (row, (group, z)) in rows.into_iter().enumerate() {
        if single && row > 0 {
            break;
        }
        let row_spacing = if grove { spacing * 0.72 } else { spacing };
        let id_start = 1 + (u64::try_from(row).expect("review row fits u64") * 4_096);
        let mut trees = review_row(group, row_spacing, z, id_start);
        if grove && row % 2 == 1 {
            for tree in &mut trees {
                tree.x += row_spacing * 0.5;
            }
        }
        if single {
            let mut tree = trees.remove(2);
            tree.x = 0.0;
            tree.z = 0.0;
            trees = vec![tree];
        }
        let prepared = prepare_trees(&trees, detail.0, |_, _| Some(0.0))
            .expect("review trees fit one mesh")
            .expect("the review row has geometry");
        commands.spawn((
            Mesh3d(meshes.add(prepared.mesh)),
            MeshMaterial3d(world_materials.trees.clone()),
            Transform::from_translation(Vec3::from_array(prepared.world_origin.map(f64_as_f32))),
        ));
    }

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-80.0, 90.0, 120.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Camera3d::default(),
        Msaa::Sample4,
        Transform::from_xyz(0.0, 8.5, distance).looking_at(Vec3::new(0.0, 10.5, 0.0), Vec3::Y),
    ));
}

fn review_row(
    group: TreeFunctionalGroup,
    spacing: f64,
    z: f64,
    id_start: u64,
) -> Vec<treeline_ecology::ProceduralTree> {
    let weights = match group {
        TreeFunctionalGroup::ColdDeciduous => [0.0, 1.0, 0.0],
        TreeFunctionalGroup::TemperateBroadleaf => [0.0, 0.0, 1.0],
        TreeFunctionalGroup::EvergreenNeedleleaf => unreachable!("broadleaf review row"),
    };
    let conditions = GrowthConditions {
        stand: Stand::measured(0.58, 27.0).expect("review stand"),
        composition: ForestComposition::new(weights).expect("one functional group"),
        prevailing_wind: [0.8, 0.6],
    };
    let mut trees = Vec::new();
    for id in id_start..id_start + 4_096 {
        let x = (f64::from(u32::try_from(trees.len()).expect("small review row")) - 2.0) * spacing;
        let tree = grow_tree(id, x, z, conditions);
        if matches!(
            tree.condition,
            TreeCondition::Mature | TreeCondition::Ancient | TreeCondition::WindDamaged
        ) && tree.height_meters >= 17.0
        {
            trees.push(tree);
            if trees.len() == 5 {
                break;
            }
        }
    }
    trees
}

#[allow(clippy::needless_pass_by_value)]
fn capture_after_settle(mut commands: Commands, path: Res<CapturePath>, mut frame: Local<u32>) {
    *frame += 1;
    if *frame != 24 {
        return;
    }
    let path = path.0.clone();
    commands.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
            save_to_disk(&path)(captured);
            exit.write(AppExit::Success);
        },
    );
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}
