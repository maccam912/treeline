//! Renderer-facing terrain tiers and the first concrete wgpu terrain path.

use std::error::Error;
use std::fmt::{Display, Formatter};

use bytemuck::{Pod, Zeroable};
use treeline_mesher::Mesh;
use wgpu::util::DeviceExt;

const TERRAIN_SHADER: &str = include_str!("terrain.wgsl");
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerrainRenderTier {
    FullVoxel,
    VoxelLod,
    CoarseSurface,
    Horizon,
}

/// Selects a representation from horizontal distance in meters.
pub fn terrain_tier(distance_meters: f64) -> TerrainRenderTier {
    if distance_meters < 200.0 {
        TerrainRenderTier::FullVoxel
    } else if distance_meters < 2_000.0 {
        TerrainRenderTier::VoxelLod
    } else if distance_meters < 20_000.0 {
        TerrainRenderTier::CoarseSurface
    } else {
        TerrainRenderTier::Horizon
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TerrainVertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
}

impl TerrainVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TerrainCutoutUniform {
    min_xz: [f32; 2],
    max_xz: [f32; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererError {
    TooManyIndices,
}

impl Display for RendererError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyIndices => formatter.write_str("the terrain mesh has too many indices"),
        }
    }
}

impl Error for RendererError {}

/// GPU resources shared by all resident terrain chunks.
#[derive(Debug)]
pub struct TerrainRenderer {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    far_bind_group: wgpu::BindGroup,
    near_bind_group: wgpu::BindGroup,
    far_cutout_buffer: wgpu::Buffer,
    depth: DepthTarget,
}

struct TerrainBindings {
    layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    far_bind_group: wgpu::BindGroup,
    near_bind_group: wgpu::BindGroup,
    far_cutout_buffer: wgpu::Buffer,
}

impl TerrainBindings {
    fn new(device: &wgpu::Device) -> Self {
        let camera_uniform = CameraUniform {
            view_projection: [[0.0; 4]; 4],
        };
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform"),
            contents: bytemuck::bytes_of(&camera_uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let no_cutout = TerrainCutoutUniform {
            min_xz: [0.0; 2],
            max_xz: [0.0; 2],
        };
        let far_cutout_buffer = cutout_buffer(
            device,
            "far terrain cutout",
            &no_cutout,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let no_cutout_buffer = cutout_buffer(
            device,
            "near terrain no-cutout",
            &no_cutout,
            wgpu::BufferUsages::UNIFORM,
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bind group layout"),
            entries: &[
                uniform_layout_entry(0, wgpu::ShaderStages::VERTEX),
                uniform_layout_entry(1, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let far_bind_group = terrain_bind_group(
            device,
            &layout,
            &camera_buffer,
            &far_cutout_buffer,
            "far terrain bind group",
        );
        let near_bind_group = terrain_bind_group(
            device,
            &layout,
            &camera_buffer,
            &no_cutout_buffer,
            "near terrain bind group",
        );

        Self {
            layout,
            camera_buffer,
            far_bind_group,
            near_bind_group,
            far_cutout_buffer,
        }
    }
}

fn cutout_buffer(
    device: &wgpu::Device,
    label: &str,
    uniform: &TerrainCutoutUniform,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::bytes_of(uniform),
        usage,
    })
}

const fn uniform_layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn terrain_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera_buffer: &wgpu::Buffer,
    cutout_buffer: &wgpu::Buffer,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: cutout_buffer.as_entire_binding(),
            },
        ],
    })
}

/// GPU buffers for one independently loadable terrain chunk.
#[derive(Debug)]
pub struct TerrainMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl TerrainRenderer {
    /// Creates the shared lit terrain pipeline and camera/depth resources.
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let bindings = TerrainBindings::new(device);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain shader"),
            source: wgpu::ShaderSource::Wgsl(TERRAIN_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain pipeline layout"),
            bind_group_layouts: &[&bindings.layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[TerrainVertex::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            camera_buffer: bindings.camera_buffer,
            far_bind_group: bindings.far_bind_group,
            near_bind_group: bindings.near_bind_group,
            far_cutout_buffer: bindings.far_cutout_buffer,
            depth: DepthTarget::new(device, width, height),
        }
    }

    /// Uploads one independently owned terrain mesh.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::TooManyIndices`] when the mesh cannot be
    /// addressed using the renderer's `u32` draw count.
    pub fn upload_mesh(
        &self,
        device: &wgpu::Device,
        mesh: &Mesh,
    ) -> Result<TerrainMesh, RendererError> {
        let vertices = mesh
            .positions
            .iter()
            .zip(&mesh.normals)
            .enumerate()
            .map(|(index, (&position, &normal))| TerrainVertex {
                position,
                normal,
                color: mesh
                    .colors
                    .get(index)
                    .copied()
                    .unwrap_or([1.0, 1.0, 1.0, 0.0]),
            })
            .collect::<Vec<_>>();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let index_count =
            u32::try_from(mesh.indices.len()).map_err(|_| RendererError::TooManyIndices)?;

        Ok(TerrainMesh {
            vertex_buffer,
            index_buffer,
            index_count,
        })
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.depth = DepthTarget::new(device, width, height);
    }

    pub fn update_camera(&self, queue: &wgpu::Queue, view_projection: [[f32; 4]; 4]) {
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&CameraUniform { view_projection }),
        );
    }

    /// Updates the half-open world-space rectangle removed from coarse terrain.
    pub fn update_far_cutout(&self, queue: &wgpu::Queue, min_xz: [f32; 2], max_xz: [f32; 2]) {
        queue.write_buffer(
            &self.far_cutout_buffer,
            0,
            bytemuck::bytes_of(&TerrainCutoutUniform { min_xz, max_xz }),
        );
    }

    pub fn render<'far, 'near>(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        far_meshes: impl IntoIterator<Item = &'far TerrainMesh>,
        near_meshes: impl IntoIterator<Item = &'near TerrainMesh>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terrain render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.34,
                        g: 0.56,
                        b: 0.76,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.far_bind_group, &[]);
        for mesh in far_meshes {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
        pass.set_bind_group(0, &self.near_bind_group, &[]);
        for mesh in near_meshes {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }
}

#[derive(Debug)]
struct DepthTarget {
    view: wgpu::TextureView,
}

impl DepthTarget {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distant_mountains_do_not_require_voxel_interiors() {
        assert_eq!(terrain_tier(30_000.0), TerrainRenderTier::Horizon);
    }
}
