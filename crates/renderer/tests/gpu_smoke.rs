//! What only a real device can check.
//!
//! Vertex layouts, shader entry points, and bind groups are validated when a
//! pipeline is built and a pass is recorded, not when the crate compiles. The
//! WGSL has to agree with the vertex format and with the surface kinds the
//! geometry is tagged with, and nothing but a device says so.
//!
//! Machines without an adapter — headless CI among them — skip the test rather
//! than fail it.

use treeline_ecology::{ForestComposition, GrowthConditions, ProceduralTree, Stand, grow_tree};
use treeline_renderer::{TerrainRenderer, TreeMeshDetail};

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const TARGET_EDGE: u32 = 256;

#[test]
fn every_pipeline_builds_and_draws_a_stand_of_trees() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("renderer smoke test device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
        },
        None,
    ))
    .expect("a device on an adapter that exists");

    let renderer = TerrainRenderer::new(&device, &queue, TARGET_FORMAT, TARGET_EDGE, TARGET_EDGE);
    let trees = stand();
    let mesh = renderer
        .upload_trees(&device, &trees, TreeMeshDetail::Full, |_, _| Some(10.0))
        .expect("tree geometry uploads");

    let view = render_target(&device).create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("renderer smoke test encoder"),
    });
    // The same stand stands in for both tiers, so all three world pipelines
    // record a draw: the far tier's cutout, the near tier's ground, and the
    // foliage half of the near tier's trees.
    renderer.render(&mut encoder, &view, [&mesh], [&mesh], &[&mesh]);
    queue.submit(Some(encoder.finish()));
    device.poll(wgpu::Maintain::Wait);
}

fn stand() -> Vec<ProceduralTree> {
    (1..=64_u64)
        .map(|id| {
            grow_tree(
                id,
                f64::from(u32::try_from(id).expect("small fixture id")) * 8.0,
                -4.0,
                GrowthConditions {
                    stand: Stand::measured(0.8, 24.0).expect("measured stand"),
                    composition: ForestComposition::SURVEYED_TILE,
                    prevailing_wind: [0.8, 0.6],
                },
            )
        })
        .collect()
}

fn render_target(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("renderer smoke test target"),
        size: wgpu::Extent3d {
            width: TARGET_EDGE,
            height: TARGET_EDGE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}
