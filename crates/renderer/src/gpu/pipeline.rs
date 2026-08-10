//! Render pipelines: lit ground, the sky backdrop, and shadow depth.
//!
//! Terrain, water, and trunks are one vertex format and one bind group — they
//! are different surface kinds, not different materials. What they do not share
//! is a fragment shader. Each world pipeline is [`SCENE_SHADER`] with one entry
//! point appended to it, so a surface kind is compiled on its own: it costs the
//! registers its own path needs, and it gives up an early depth test only if it
//! is a kind that discards.

use crate::gpu::DEPTH_FORMAT;
use crate::vertex::TerrainVertex;

/// Uniforms, the shared vertex shader, and the light and air every surface is
/// drawn through. Never compiled alone: an entry point is appended to it.
pub(crate) const SCENE_SHADER: &str = include_str!("../shader/scene.wgsl");
pub(crate) const GROUND_SHADER: &str = include_str!("../shader/ground.wgsl");
pub(crate) const FAR_GROUND_SHADER: &str = include_str!("../shader/far_ground.wgsl");
pub(crate) const SHADOW_SHADER: &str = include_str!("../shader/shadow.wgsl");
pub(crate) const SKY_SHADER: &str = include_str!("../shader/sky.wgsl");
pub(crate) const SHADOWS_ENABLED: &str = include_str!("../shader/shadows_enabled.wgsl");
pub(crate) const SHADOWS_DISABLED: &str = include_str!("../shader/shadows_disabled.wgsl");

/// The two pipelines a frame draws the world with.
///
/// They differ only in their fragment entry point. Splitting them is what lets
/// the near tier — most of the pixels on screen — keep the early depth test
/// that the far tier's cutout has to give up.
#[derive(Debug)]
pub(crate) struct WorldPipelines {
    /// Coarse terrain, cut away where the near tier covers it.
    pub(crate) far_ground: wgpu::RenderPipeline,
    /// Near terrain, water, and bark. Opaque throughout, and never discards.
    pub(crate) near_ground: wgpu::RenderPipeline,
}

pub(crate) fn create_world_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    surface_format: wgpu::TextureFormat,
    shadows: bool,
) -> WorldPipelines {
    let shadow_stage = if shadows {
        SHADOWS_ENABLED
    } else {
        SHADOWS_DISABLED
    };
    let ground = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ground shader"),
        source: wgpu::ShaderSource::Wgsl(
            format!("{SCENE_SHADER}\n{shadow_stage}\n{GROUND_SHADER}\n{FAR_GROUND_SHADER}").into(),
        ),
    });
    WorldPipelines {
        far_ground: create_surface_pipeline(
            device,
            layout,
            surface_format,
            "far ground pipeline",
            &ground,
            "fs_far_ground",
        ),
        near_ground: create_surface_pipeline(
            device,
            layout,
            surface_format,
            "near ground pipeline",
            &ground,
            "fs_ground",
        ),
    }
}

/// One world pipeline: the shared vertex shader, one fragment entry point, and
/// the reverse-Z depth state every opaque surface in the frame agrees on.
fn create_surface_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    surface_format: wgpu::TextureFormat,
    label: &str,
    module: &wgpu::ShaderModule,
    fragment_entry: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[TerrainVertex::layout()],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            // Reverse-Z preserves precision from ground cover through the horizon.
            depth_compare: wgpu::CompareFunction::Greater,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

pub(crate) fn create_sky_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    surface_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky shader"),
        source: wgpu::ShaderSource::Wgsl(SKY_SHADER.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("physical sky pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_sky"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_sky"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

pub(crate) fn create_shadow_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadow shader"),
        source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadow pipeline layout"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cascaded sun shadow pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_shadow"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[TerrainVertex::layout()],
        },
        fragment: None,
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 1.8,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fragment shader that can discard cannot be depth-tested before it
    /// runs, so a discard anywhere in the near tier's shader is paid for by
    /// every hillside the world draws in front of.
    ///
    /// One discard earns its place, and it is quarantined in the pipeline that
    /// needs it: the far tier's cutout, drawn first into a depth buffer holding
    /// nothing but sky, so it has no early rejection to lose. Anything else
    /// means a surface kind has gone to alpha testing — or, worse, has dragged
    /// the ground down with it.
    ///
    /// Statements are counted, not the word: prose about discarding is fine.
    #[test]
    fn only_the_shaders_that_cut_holes_in_themselves_discard() {
        for (name, source) in [("scene", SCENE_SHADER), ("ground", GROUND_SHADER)] {
            assert_eq!(
                source.matches("discard;").count(),
                0,
                "the {name} shader discards, costing every opaque surface its early depth test"
            );
        }
        assert_eq!(
            FAR_GROUND_SHADER.matches("discard;").count(),
            1,
            "the far ground shader no longer cuts itself out the way its pipeline assumes"
        );
    }

    /// The scene half declares the bindings and the vertex stage; an entry
    /// point half supplies only a fragment stage. Neither is a shader alone,
    /// and a stage drifting into the wrong half would compile here and fail at
    /// pipeline creation on a machine, not in the gate.
    #[test]
    fn every_world_pipeline_is_the_scene_plus_one_fragment_entry_point() {
        assert!(SCENE_SHADER.contains("@vertex"));
        for source in [GROUND_SHADER, FAR_GROUND_SHADER] {
            assert!(!source.contains("@vertex"));
            assert!(!source.contains("@group("));
        }
        assert_eq!(SCENE_SHADER.matches("@fragment").count(), 0);
        assert_eq!(GROUND_SHADER.matches("@fragment").count(), 1);
        assert_eq!(FAR_GROUND_SHADER.matches("@fragment").count(), 1);
    }
}
