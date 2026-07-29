//! Renderer-facing terrain tiers and the first concrete wgpu terrain path.

use std::error::Error;
use std::fmt::{Display, Formatter};

use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3};
use treeline_ecology::{
    BarkStyle, CrownShape, GroundCoverGroup, GroundPlant, ProceduralTree, RockForm, SurfaceRock,
    TreeCondition, TreeFunctionalGroup,
};
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

/// Geometry detail for one deterministic set of procedural tree individuals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeMeshDetail {
    /// Trunks, branches, damage, and species-specific crown clusters.
    Full,
    /// Trunks and one species-shaped crown, without individual branches.
    Simplified,
    /// A minimal trunk and crown silhouette for the outer individual-tree ring.
    Silhouette,
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
    position_high: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
    snow_coverage: f32,
    position_low: [f32; 3],
}

impl TerrainVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x4,
        3 => Float32,
        4 => Float32x3,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

fn terrain_vertex(
    position: [f64; 3],
    normal: [f32; 3],
    color: [f32; 4],
    snow_coverage: f32,
) -> TerrainVertex {
    let (position_high, position_low) = split_position(position);
    TerrainVertex {
        position_high,
        normal,
        color,
        snow_coverage,
        position_low,
    }
}

fn local_vertex(
    position: Vec3,
    normal: Vec3,
    color: [f32; 4],
    snow_coverage: f32,
) -> TerrainVertex {
    terrain_vertex(
        position.as_dvec3().to_array(),
        normal.to_array(),
        color,
        snow_coverage,
    )
}

fn split_position(position: [f64; 3]) -> ([f32; 3], [f32; 3]) {
    let split = position.map(split_f64);
    (
        [split[0][0], split[1][0], split[2][0]],
        [split[0][1], split[1][1], split[2][1]],
    )
}

fn split_f64(value: f64) -> [f32; 2] {
    let high = f64_as_f32(value);
    [high, f64_as_f32(value - f64::from(high))]
}

fn translate_local_vertices(vertices: &mut [TerrainVertex], origin: [f64; 3]) {
    for vertex in vertices {
        let local: [f64; 3] = std::array::from_fn(|axis| {
            f64::from(vertex.position_high[axis]) + f64::from(vertex.position_low[axis])
        });
        let (position_high, position_low) =
            split_position(std::array::from_fn(|axis| origin[axis] + local[axis]));
        vertex.position_high = position_high;
        vertex.position_low = position_low;
    }
}

const SNOW_GRID_SAMPLES_PER_EDGE: usize = 3;

#[derive(Clone, Copy, Debug)]
struct SnowDepthGrid {
    min_x: f64,
    min_z: f64,
    span_x: f64,
    span_z: f64,
    samples: [f64; SNOW_GRID_SAMPLES_PER_EDGE * SNOW_GRID_SAMPLES_PER_EDGE],
}

impl SnowDepthGrid {
    fn sample(mesh: &Mesh, mut snow_depth_at: impl FnMut(f64, f64) -> Option<f64>) -> Self {
        let Some((&first, remaining)) = mesh.positions.split_first() else {
            return Self {
                min_x: 0.0,
                min_z: 0.0,
                span_x: 0.0,
                span_z: 0.0,
                samples: [0.0; SNOW_GRID_SAMPLES_PER_EDGE * SNOW_GRID_SAMPLES_PER_EDGE],
            };
        };
        let ([min_x, min_z], [max_x, max_z]) = remaining.iter().fold(
            ([first[0], first[2]], [first[0], first[2]]),
            |(min, max), position| {
                let point = [position[0], position[2]];
                (
                    [min[0].min(point[0]), min[1].min(point[1])],
                    [max[0].max(point[0]), max[1].max(point[1])],
                )
            },
        );
        let span_x = max_x - min_x;
        let span_z = max_z - min_z;
        let samples = std::array::from_fn(|index| {
            let grid_x = index % SNOW_GRID_SAMPLES_PER_EDGE;
            let grid_z = index / SNOW_GRID_SAMPLES_PER_EDGE;
            let grid_offsets = [0.0, 0.5, 1.0];
            let x = min_x + (span_x * grid_offsets[grid_x]);
            let z = min_z + (span_z * grid_offsets[grid_z]);
            snow_depth_at(x, z).unwrap_or(0.0).clamp(0.0, 1.0)
        });

        Self {
            min_x,
            min_z,
            span_x,
            span_z,
            samples,
        }
    }

    fn coverage_at(self, position: [f64; 3]) -> f64 {
        let (cell_x, blend_x) = snow_grid_axis(position[0], self.min_x, self.span_x);
        let (cell_z, blend_z) = snow_grid_axis(position[2], self.min_z, self.span_z);
        let low = cell_z * SNOW_GRID_SAMPLES_PER_EDGE + cell_x;
        let bottom = lerp_f64(self.samples[low], self.samples[low + 1], blend_x);
        let top = lerp_f64(
            self.samples[low + SNOW_GRID_SAMPLES_PER_EDGE],
            self.samples[low + SNOW_GRID_SAMPLES_PER_EDGE + 1],
            blend_x,
        );
        lerp_f64(bottom, top, blend_z)
    }
}

fn snow_grid_axis(value: f64, minimum: f64, span: f64) -> (usize, f64) {
    let normalized = if span <= f64::EPSILON {
        0.0
    } else {
        ((value - minimum) / span).clamp(0.0, 1.0)
    };
    if normalized <= 0.5 {
        (0, normalized * 2.0)
    } else {
        (1, (normalized - 0.5) * 2.0)
    }
}

fn lerp_f64(start: f64, end: f64, amount: f64) -> f64 {
    start + ((end - start) * amount)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
    render_origin_high: [f32; 4],
    render_origin_low: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TerrainCutoutUniform {
    min_high: [f32; 2],
    min_low: [f32; 2],
    max_high: [f32; 2],
    max_low: [f32; 2],
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
            render_origin_high: [0.0; 4],
            render_origin_low: [0.0; 4],
        };
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform"),
            contents: bytemuck::bytes_of(&camera_uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let no_cutout = TerrainCutoutUniform {
            min_high: [0.0; 2],
            min_low: [0.0; 2],
            max_high: [0.0; 2],
            max_low: [0.0; 2],
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
                uniform_layout_entry(0, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                uniform_layout_entry(1, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
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
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                // Reverse-Z preserves precision from nearby ground cover through
                // the horizon instead of spending most depth values near 0.1 m.
                depth_compare: wgpu::CompareFunction::Greater,
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
            .map(|(index, (&position, &normal))| {
                terrain_vertex(
                    position,
                    normal,
                    mesh.colors
                        .get(index)
                        .copied()
                        .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                    0.0,
                )
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

    /// Uploads terrain with a deterministic snow-depth value interpolated
    /// across every surface vertex. The callback is evaluated on a bounded
    /// three-by-three lattice, keeping render-thread work independent of mesh
    /// density. The caller owns generation semantics; this keeps the renderer
    /// independent from world and climate crates.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::TooManyIndices`] when the mesh cannot be
    /// addressed using the renderer's `u32` draw count.
    pub fn upload_snowy_mesh(
        &self,
        device: &wgpu::Device,
        mesh: &Mesh,
        snow_depth_at: impl FnMut(f64, f64) -> Option<f64>,
    ) -> Result<TerrainMesh, RendererError> {
        let snow_depth = SnowDepthGrid::sample(mesh, snow_depth_at);
        let vertices = mesh
            .positions
            .iter()
            .zip(&mesh.normals)
            .enumerate()
            .map(|(index, (&position, &normal))| {
                terrain_vertex(
                    position,
                    normal,
                    mesh.colors
                        .get(index)
                        .copied()
                        .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                    f64_as_f32(snow_depth.coverage_at(position)),
                )
            })
            .collect::<Vec<_>>();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("snow-covered terrain vertices"),
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

    /// Builds and uploads procedural tree grammars as one independently owned mesh.
    ///
    /// Tree bases are resolved against the composed world surface supplied by
    /// the caller, keeping ecology independent from streaming-world artifacts.
    /// Individuals without a surface sample are omitted.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::TooManyIndices`] when the generated tree mesh
    /// exceeds `u32` vertex or draw addressing.
    pub fn upload_trees(
        &self,
        device: &wgpu::Device,
        trees: &[ProceduralTree],
        detail: TreeMeshDetail,
        surface_height: impl FnMut(f64, f64) -> Option<f64>,
    ) -> Result<TerrainMesh, RendererError> {
        let (vertices, indices) = procedural_tree_geometry(trees, detail, surface_height)?;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("procedural tree vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("procedural tree indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let index_count =
            u32::try_from(indices.len()).map_err(|_| RendererError::TooManyIndices)?;

        Ok(TerrainMesh {
            vertex_buffer,
            index_buffer,
            index_count,
        })
    }

    /// Builds and uploads deterministic surface rocks as one independently owned mesh.
    ///
    /// Rock bases are resolved against the composed world surface supplied by
    /// the caller. Individuals without a surface sample are omitted.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::TooManyIndices`] when the generated rock mesh
    /// exceeds `u32` vertex or draw addressing.
    pub fn upload_rocks(
        &self,
        device: &wgpu::Device,
        rocks: &[SurfaceRock],
        surface_height: impl FnMut(f64, f64) -> Option<f64>,
    ) -> Result<TerrainMesh, RendererError> {
        let (vertices, indices) = procedural_rock_geometry(rocks, surface_height)?;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("surface rock vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("surface rock indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let index_count =
            u32::try_from(indices.len()).map_err(|_| RendererError::TooManyIndices)?;

        Ok(TerrainMesh {
            vertex_buffer,
            index_buffer,
            index_count,
        })
    }

    /// Builds and uploads deterministic ground vegetation as one independently owned mesh.
    ///
    /// Plant bases are resolved against the composed world surface supplied by
    /// the caller. Individuals without a surface sample are omitted.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::TooManyIndices`] when the generated plant mesh
    /// exceeds `u32` vertex or draw addressing.
    pub fn upload_ground_vegetation(
        &self,
        device: &wgpu::Device,
        plants: &[GroundPlant],
        surface_height: impl FnMut(f64, f64) -> Option<f64>,
    ) -> Result<TerrainMesh, RendererError> {
        let (vertices, indices) = procedural_ground_vegetation_geometry(plants, surface_height)?;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ground vegetation vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ground vegetation indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let index_count =
            u32::try_from(indices.len()).map_err(|_| RendererError::TooManyIndices)?;

        Ok(TerrainMesh {
            vertex_buffer,
            index_buffer,
            index_count,
        })
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.depth = DepthTarget::new(device, width, height);
    }

    pub fn update_camera(
        &self,
        queue: &wgpu::Queue,
        view_projection: [[f32; 4]; 4],
        render_origin: [f64; 3],
    ) {
        let (render_origin_high, render_origin_low) = split_position(render_origin);
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&CameraUniform {
                view_projection,
                render_origin_high: [
                    render_origin_high[0],
                    render_origin_high[1],
                    render_origin_high[2],
                    0.0,
                ],
                render_origin_low: [
                    render_origin_low[0],
                    render_origin_low[1],
                    render_origin_low[2],
                    0.0,
                ],
            }),
        );
    }

    /// Updates the half-open world-space rectangle removed from coarse terrain.
    pub fn update_far_cutout(&self, queue: &wgpu::Queue, min_xz: [f64; 2], max_xz: [f64; 2]) {
        let ([min_high_x, min_low_x], [min_high_z, min_low_z]) =
            (split_f64(min_xz[0]), split_f64(min_xz[1]));
        let ([max_high_x, max_low_x], [max_high_z, max_low_z]) =
            (split_f64(max_xz[0]), split_f64(max_xz[1]));
        queue.write_buffer(
            &self.far_cutout_buffer,
            0,
            bytemuck::bytes_of(&TerrainCutoutUniform {
                min_high: [min_high_x, min_high_z],
                min_low: [min_low_x, min_low_z],
                max_high: [max_high_x, max_high_z],
                max_low: [max_low_x, max_low_z],
            }),
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
                    load: wgpu::LoadOp::Clear(0.0),
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

fn procedural_tree_geometry(
    trees: &[ProceduralTree],
    detail: TreeMeshDetail,
    mut surface_height: impl FnMut(f64, f64) -> Option<f64>,
) -> Result<(Vec<TerrainVertex>, Vec<u32>), RendererError> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for tree in trees {
        let Some(base_y) = surface_height(tree.x, tree.z) else {
            continue;
        };
        let first_vertex = vertices.len();
        append_tree(&mut vertices, &mut indices, *tree, detail, Vec3::ZERO)?;
        translate_local_vertices(&mut vertices[first_vertex..], [tree.x, base_y, tree.z]);
    }
    Ok((vertices, indices))
}

fn procedural_rock_geometry(
    rocks: &[SurfaceRock],
    mut surface_height: impl FnMut(f64, f64) -> Option<f64>,
) -> Result<(Vec<TerrainVertex>, Vec<u32>), RendererError> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for rock in rocks {
        let Some(surface_y) = surface_height(rock.x, rock.z) else {
            continue;
        };
        let first_vertex = vertices.len();
        append_surface_rock(&mut vertices, &mut indices, *rock)?;
        translate_local_vertices(&mut vertices[first_vertex..], [rock.x, surface_y, rock.z]);
    }
    Ok((vertices, indices))
}

fn procedural_ground_vegetation_geometry(
    plants: &[GroundPlant],
    mut surface_height: impl FnMut(f64, f64) -> Option<f64>,
) -> Result<(Vec<TerrainVertex>, Vec<u32>), RendererError> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for plant in plants {
        let Some(surface_y) = surface_height(plant.x, plant.z) else {
            continue;
        };
        let first_vertex = vertices.len();
        append_ground_plant(&mut vertices, &mut indices, *plant, Vec3::Y * 0.015)?;
        translate_local_vertices(&mut vertices[first_vertex..], [plant.x, surface_y, plant.z]);
    }
    Ok((vertices, indices))
}

fn append_ground_plant(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    plant: GroundPlant,
    base: Vec3,
) -> Result<(), RendererError> {
    match plant.genotype.group {
        GroundCoverGroup::Graminoid => append_graminoid(vertices, indices, plant, base),
        GroundCoverGroup::Forb => append_forb(vertices, indices, plant, base),
        GroundCoverGroup::Fern => append_fern(vertices, indices, plant, base),
        GroundCoverGroup::LowShrub => append_low_shrub(vertices, indices, plant, base),
        GroundCoverGroup::Moss => append_moss(vertices, indices, plant, base),
    }
}

fn append_graminoid(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    plant: GroundPlant,
    base: Vec3,
) -> Result<(), RendererError> {
    let count = usize::from(plant.genotype.leaf_count);
    let height = f64_as_f32(plant.height_meters);
    let radius = f64_as_f32(plant.radius_meters);
    let color = ground_plant_color(plant, 0);
    let lean = plant_lean(plant);
    for ordinal in 0..count {
        let radial = plant_radial(plant, ordinal, count);
        let height_scale = 0.68 + (hash_lane(plant.id.rotate_left(9), ordinal) * 0.40);
        let start = base + (radial * radius * 0.18);
        let end = start
            + (Vec3::Y * height * height_scale)
            + (lean * height * f64_as_f32(plant.lean_fraction))
            + (radial * radius * 0.18);
        let width =
            radius * (0.07 + ((1.0 - f64_as_f32(plant.genotype.slenderness_fraction)) * 0.08));
        append_leaf_blade(vertices, indices, start, end, width, color)?;
    }
    Ok(())
}

fn append_forb(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    plant: GroundPlant,
    base: Vec3,
) -> Result<(), RendererError> {
    let height = f64_as_f32(plant.height_meters);
    let radius = f64_as_f32(plant.radius_meters);
    let lean = plant_lean(plant);
    let top = base + (Vec3::Y * height) + (lean * height * f64_as_f32(plant.lean_fraction));
    append_tapered_cylinder(
        vertices,
        indices,
        CylinderSpec {
            start: base,
            end: top,
            start_radius: (height * 0.035).clamp(0.008, 0.025),
            end_radius: (height * 0.012).clamp(0.004, 0.012),
            sides: 4,
            color: ground_plant_color(plant, 0),
        },
    )?;
    let leaf_count = usize::from(plant.genotype.leaf_count).min(6);
    for ordinal in 0..leaf_count {
        let radial = plant_radial(plant, ordinal, leaf_count);
        let fraction = 0.22 + (usize_as_f32(ordinal) / usize_as_f32(leaf_count) * 0.55);
        let start = base + ((top - base) * fraction);
        let end =
            start + (radial * radius * (0.48 + (fraction * 0.24))) + (Vec3::Y * height * 0.06);
        append_leaf_blade(
            vertices,
            indices,
            start,
            end,
            radius * 0.16,
            ground_plant_color(plant, ordinal + 1),
        )?;
    }
    if plant.flowering_fraction > 0.08 {
        let flower_radius =
            (radius * (0.10 + (f64_as_f32(plant.flowering_fraction) * 0.13))).clamp(0.025, 0.12);
        append_octahedral_crown(
            vertices,
            indices,
            top,
            Vec3::new(flower_radius, flower_radius * 0.46, flower_radius),
            flower_color(plant),
        )?;
    }
    Ok(())
}

fn append_fern(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    plant: GroundPlant,
    base: Vec3,
) -> Result<(), RendererError> {
    let count = usize::from(plant.genotype.leaf_count);
    let height = f64_as_f32(plant.height_meters);
    let radius = f64_as_f32(plant.radius_meters);
    let lean = plant_lean(plant);
    for ordinal in 0..count {
        let radial = plant_radial(plant, ordinal, count);
        let scale = 0.72 + (hash_lane(plant.id.rotate_right(11), ordinal) * 0.34);
        let end = base
            + (radial * radius * scale)
            + (Vec3::Y * height * (0.62 + (scale * 0.18)))
            + (lean * height * f64_as_f32(plant.lean_fraction) * 0.42);
        append_leaf_blade(
            vertices,
            indices,
            base + (Vec3::Y * height * 0.04),
            end,
            radius * (0.16 + ((1.0 - scale) * 0.06)),
            ground_plant_color(plant, ordinal),
        )?;
    }
    Ok(())
}

fn append_low_shrub(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    plant: GroundPlant,
    base: Vec3,
) -> Result<(), RendererError> {
    let count = usize::from(plant.genotype.leaf_count).min(7);
    let height = f64_as_f32(plant.height_meters);
    let radius = f64_as_f32(plant.radius_meters);
    let woody_color = [0.25, 0.18, 0.09, 1.0];
    for ordinal in 0..count {
        let radial = plant_radial(plant, ordinal, count);
        let scale = 0.55 + (hash_lane(plant.id.rotate_left(17), ordinal) * 0.42);
        let end = base + (radial * radius * scale * 0.72) + (Vec3::Y * height * scale);
        append_tapered_cylinder(
            vertices,
            indices,
            CylinderSpec {
                start: base,
                end,
                start_radius: (height * 0.025).clamp(0.012, 0.04),
                end_radius: (height * 0.008).clamp(0.004, 0.015),
                sides: 4,
                color: woody_color,
            },
        )?;
        let cluster_radius = radius * (0.20 + (scale * 0.12));
        append_octahedral_crown(
            vertices,
            indices,
            end,
            Vec3::new(cluster_radius, cluster_radius * 0.72, cluster_radius),
            ground_plant_color(plant, ordinal),
        )?;
    }
    Ok(())
}

fn append_moss(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    plant: GroundPlant,
    base: Vec3,
) -> Result<(), RendererError> {
    let count = usize::from(plant.genotype.leaf_count).min(5);
    let height = f64_as_f32(plant.height_meters);
    let radius = f64_as_f32(plant.radius_meters);
    for ordinal in 0..count {
        let radial = plant_radial(plant, ordinal, count);
        let scale = 0.48 + (hash_lane(plant.id.rotate_right(21), ordinal) * 0.32);
        append_octahedral_crown(
            vertices,
            indices,
            base + (radial * radius * 0.38),
            Vec3::new(radius * scale, height * scale, radius * scale),
            ground_plant_color(plant, ordinal),
        )?;
    }
    Ok(())
}

fn append_leaf_blade(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    start: Vec3,
    end: Vec3,
    width: f32,
    color: [f32; 4],
) -> Result<(), RendererError> {
    let direction = (end - start).normalize_or_zero();
    if direction == Vec3::ZERO {
        return Ok(());
    }
    let tangent = direction.cross(Vec3::Y).normalize_or_zero();
    let tangent = if tangent == Vec3::ZERO {
        Vec3::X
    } else {
        tangent
    };
    let normal = tangent.cross(direction).normalize_or_zero();
    let base_index = u32::try_from(vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    for (position, normal) in [
        (start - (tangent * width), normal),
        (start + (tangent * width), normal),
        (end + (tangent * width * 0.12), normal),
        (end - (tangent * width * 0.12), normal),
    ] {
        vertices.push(local_vertex(position, normal, color, 0.0));
    }
    indices.extend_from_slice(&[
        base_index,
        base_index + 1,
        base_index + 2,
        base_index,
        base_index + 2,
        base_index + 3,
    ]);
    Ok(())
}

fn plant_radial(plant: GroundPlant, ordinal: usize, count: usize) -> Vec3 {
    let turn = f64_as_f32(plant.rotation_turns)
        + (usize_as_f32(ordinal) / usize_as_f32(count))
        + ((hash_lane(plant.id, ordinal) - 0.5) * 0.09);
    let angle = turn * std::f32::consts::TAU;
    Vec3::new(libm::cosf(angle), 0.0, libm::sinf(angle))
        * f64_as_f32(plant.genotype.spread_fraction)
}

fn plant_lean(plant: GroundPlant) -> Vec3 {
    Vec3::new(
        f64_as_f32(plant.lean_direction[0]),
        0.0,
        f64_as_f32(plant.lean_direction[1]),
    )
}

fn ground_plant_color(plant: GroundPlant, ordinal: usize) -> [f32; 4] {
    let base = match plant.genotype.group {
        GroundCoverGroup::Graminoid => [0.24, 0.46, 0.10],
        GroundCoverGroup::Forb => [0.16, 0.42, 0.09],
        GroundCoverGroup::Fern => [0.08, 0.34, 0.10],
        GroundCoverGroup::LowShrub => [0.13, 0.31, 0.075],
        GroundCoverGroup::Moss => [0.21, 0.38, 0.08],
    };
    let genotype_variation = f64_as_f32(plant.genotype.color_variation_fraction) - 0.5;
    let individual_variation = (hash_lane(plant.id.rotate_left(5), ordinal) - 0.5) * 0.08;
    [
        (base[0] + (genotype_variation * 0.07) + individual_variation).clamp(0.0, 1.0),
        (base[1] + (genotype_variation * 0.12) + individual_variation).clamp(0.0, 1.0),
        (base[2] + (genotype_variation * 0.04) + (individual_variation * 0.45)).clamp(0.0, 1.0),
        1.0,
    ]
}

fn flower_color(plant: GroundPlant) -> [f32; 4] {
    match plant.id % 4 {
        0 => [0.95, 0.72, 0.12, 1.0],
        1 => [0.76, 0.34, 0.72, 1.0],
        2 => [0.90, 0.88, 0.76, 1.0],
        _ => [0.38, 0.52, 0.88, 1.0],
    }
}

fn append_surface_rock(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    rock: SurfaceRock,
) -> Result<(), RendererError> {
    const SIDES: usize = 8;
    const RINGS: [(f32, f32); 3] = [(-0.62, 0.76), (-0.04, 1.0), (0.52, 0.70)];

    let radii = Vec3::new(
        f64_as_f32(rock.radii_meters[0]),
        f64_as_f32(rock.radii_meters[1]),
        f64_as_f32(rock.radii_meters[2]),
    );
    let center = Vec3::Y * (radii.y * (1.0 - f64_as_f32(rock.embedded_fraction)));
    let yaw = Quat::from_rotation_y(f64_as_f32(rock.rotation_turns) * std::f32::consts::TAU);
    let tilt_axis = Vec3::new(
        f64_as_f32(rock.tilt_direction[1]),
        0.0,
        -f64_as_f32(rock.tilt_direction[0]),
    )
    .normalize_or_zero();
    let tilt = Quat::from_axis_angle(
        tilt_axis,
        f64_as_f32(rock.tilt_fraction) * std::f32::consts::FRAC_PI_2,
    );
    let rotation = tilt * yaw;
    let base_index = u32::try_from(vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    push_rock_vertex(
        vertices,
        rock,
        center,
        rotation,
        Vec3::new(0.0, -radii.y, 0.0),
        0,
    );
    for (ring_index, &(height_fraction, radius_fraction)) in RINGS.iter().enumerate() {
        for side in 0..SIDES {
            let angle = usize_as_f32(side) / usize_as_f32(SIDES) * std::f32::consts::TAU;
            let irregularity = rock_irregularity(rock, ring_index, side);
            let horizontal_scale = radius_fraction * irregularity;
            let local = Vec3::new(
                libm::cosf(angle) * radii.x * horizontal_scale,
                radii.y * height_fraction,
                libm::sinf(angle) * radii.z * horizontal_scale,
            );
            push_rock_vertex(
                vertices,
                rock,
                center,
                rotation,
                local,
                1 + (ring_index * SIDES) + side,
            );
        }
    }
    let top_ordinal = 1 + (RINGS.len() * SIDES);
    push_rock_vertex(
        vertices,
        rock,
        center,
        rotation,
        Vec3::new(0.0, radii.y, 0.0),
        top_ordinal,
    );

    let first_ring = base_index + 1;
    for side in 0..SIDES {
        let next = (side + 1) % SIDES;
        indices.extend_from_slice(&[
            base_index,
            first_ring + usize_as_u32(side)?,
            first_ring + usize_as_u32(next)?,
        ]);
    }
    for ring in 0..(RINGS.len() - 1) {
        let lower = first_ring + usize_as_u32(ring * SIDES)?;
        let upper = lower + usize_as_u32(SIDES)?;
        for side in 0..SIDES {
            let next = (side + 1) % SIDES;
            let side = usize_as_u32(side)?;
            let next = usize_as_u32(next)?;
            indices.extend_from_slice(&[
                lower + side,
                upper + side,
                lower + next,
                lower + next,
                upper + side,
                upper + next,
            ]);
        }
    }
    let last_ring = first_ring + usize_as_u32((RINGS.len() - 1) * SIDES)?;
    let top = base_index + usize_as_u32(top_ordinal)?;
    for side in 0..SIDES {
        let next = (side + 1) % SIDES;
        indices.extend_from_slice(&[
            last_ring + usize_as_u32(side)?,
            top,
            last_ring + usize_as_u32(next)?,
        ]);
    }
    Ok(())
}

fn push_rock_vertex(
    vertices: &mut Vec<TerrainVertex>,
    rock: SurfaceRock,
    center: Vec3,
    rotation: Quat,
    local: Vec3,
    ordinal: usize,
) {
    let normalized = Vec3::new(
        local.x / f64_as_f32(rock.radii_meters[0]),
        local.y / f64_as_f32(rock.radii_meters[1]),
        local.z / f64_as_f32(rock.radii_meters[2]),
    )
    .normalize_or_zero();
    vertices.push(local_vertex(
        center + (rotation * local),
        rotation * normalized,
        rock_color(rock, ordinal),
        0.0,
    ));
}

fn rock_irregularity(rock: SurfaceRock, ring: usize, side: usize) -> f32 {
    let lane = ring * 3 + side;
    let variation = hash_lane(
        rock.id.rotate_left(u32::try_from(ring * 7).unwrap_or(0)),
        lane,
    ) - 0.5;
    let angularity = 1.0 - f64_as_f32(rock.genotype.roundness_fraction);
    let fracture = f64_as_f32(rock.genotype.fracture_fraction);
    1.0 + (variation * ((angularity * 0.34) + (fracture * 0.12)))
}

fn rock_color(rock: SurfaceRock, ordinal: usize) -> [f32; 4] {
    let hardness = f64_as_f32(rock.genotype.hardness_fraction);
    let carbonate = f64_as_f32(rock.genotype.carbonate_fraction);
    let weathering = f64_as_f32(rock.genotype.weathering_fraction);
    let moss = f64_as_f32(rock.moss_fraction);
    let form_tint = match rock.genotype.form {
        RockForm::RoundedBoulder => [0.02, 0.025, 0.02],
        RockForm::AngularBlock => [-0.025, -0.02, -0.01],
        RockForm::Slab => [0.055, 0.05, 0.035],
        RockForm::ScreeFragment => [-0.01, -0.005, 0.0],
    };
    let variation = (hash_lane(rock.id.rotate_right(13), ordinal) - 0.5) * 0.08;
    let mineral = [
        0.34 + (carbonate * 0.28) + (weathering * 0.10) + form_tint[0],
        0.35 + (carbonate * 0.26) + (weathering * 0.055) + form_tint[1],
        0.36 + (hardness * 0.12) + (carbonate * 0.19) - (weathering * 0.025) + form_tint[2],
    ];
    [
        (mineral[0] + variation - (moss * 0.15)).clamp(0.0, 1.0),
        (mineral[1] + variation + (moss * 0.035)).clamp(0.0, 1.0),
        (mineral[2] + variation - (moss * 0.12)).clamp(0.0, 1.0),
        1.0,
    ]
}

fn append_tree(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    detail: TreeMeshDetail,
    base: Vec3,
) -> Result<(), RendererError> {
    let height = f64_as_f32(tree.height_meters);
    let lean = Vec3::new(
        f64_as_f32(tree.lean_direction[0]),
        0.0,
        f64_as_f32(tree.lean_direction[1]),
    );
    let trunk_vector = if tree.condition == TreeCondition::Fallen {
        (lean * height * f64_as_f32(tree.lean_fraction)) + (Vec3::Y * height * 0.08)
    } else {
        (lean * height * f64_as_f32(tree.lean_fraction)) + (Vec3::Y * height)
    };
    let top = base + trunk_vector;
    let trunk_radius = f64_as_f32(tree.trunk_base_radius_meters);
    let top_radius = (trunk_radius
        * (1.0 - (f64_as_f32(tree.genotype.trunk_taper_fraction) * 0.88)))
        .max(trunk_radius * 0.08);
    let trunk_sides = match detail {
        TreeMeshDetail::Full => 7,
        TreeMeshDetail::Simplified => 5,
        TreeMeshDetail::Silhouette => 3,
    };
    append_tapered_cylinder(
        vertices,
        indices,
        CylinderSpec {
            start: base,
            end: top,
            start_radius: trunk_radius,
            end_radius: top_radius,
            sides: trunk_sides,
            color: bark_color(tree),
        },
    )?;

    if tree.condition == TreeCondition::Sapling {
        append_sapling_crown(vertices, indices, tree, base, top)?;
        return Ok(());
    }

    let frame = TreeFrame {
        base,
        top,
        trunk_vector,
        trunk_radius,
    };
    if detail == TreeMeshDetail::Full {
        append_tree_crown(vertices, indices, tree, frame)
    } else if tree_has_foliage(tree) {
        let crown_start = match tree.genotype.crown_shape {
            CrownShape::Conical => 0.24,
            CrownShape::Columnar => 0.38,
            CrownShape::Rounded => 0.46,
            CrownShape::Spreading => 0.34,
        };
        append_terminal_crown(
            vertices,
            indices,
            tree,
            frame,
            crown_start,
            f64_as_f32(tree.crown_radius_meters),
            foliage_color(tree),
        )
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct TreeFrame {
    base: Vec3,
    top: Vec3,
    trunk_vector: Vec3,
    trunk_radius: f32,
}

fn append_tree_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
) -> Result<(), RendererError> {
    let branch_count = branch_count(tree);
    let crown_start = match tree.genotype.crown_shape {
        CrownShape::Conical => 0.24,
        CrownShape::Columnar => 0.38,
        CrownShape::Rounded => 0.46,
        CrownShape::Spreading => 0.34,
    };
    let crown_radius = f64_as_f32(tree.crown_radius_meters);
    let foliage = foliage_color(tree);
    for branch_index in 0..branch_count {
        append_tree_branch(
            vertices,
            indices,
            tree,
            frame,
            crown_start,
            branch_index,
            branch_count,
        )?;
    }

    if !tree_has_foliage(tree) {
        return Ok(());
    }
    append_terminal_crown(
        vertices,
        indices,
        tree,
        frame,
        crown_start,
        crown_radius,
        foliage,
    )
}

fn append_tree_branch(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
    crown_start: f32,
    branch_index: usize,
    branch_count: usize,
) -> Result<(), RendererError> {
    let ordinal = usize_as_f32(branch_index);
    let count = usize_as_f32(branch_count);
    let branch_fraction = crown_start + ((ordinal + 1.0) / (count + 1.0) * (0.88 - crown_start));
    let start = frame.base + (frame.trunk_vector * branch_fraction);
    let turn = f64_as_f32(tree.rotation_turns)
        + (ordinal * 0.618_034)
        + (hash_lane(tree.id, branch_index) * 0.16);
    let (azimuth_sine, azimuth_cosine) = libm::sincosf(turn * std::f32::consts::TAU);
    let horizontal = Vec3::new(azimuth_cosine, 0.0, azimuth_sine);
    let branch_angle = f64_as_f32(tree.genotype.branching_angle_radians);
    let direction = (horizontal * libm::sinf(branch_angle)) + (Vec3::Y * libm::cosf(branch_angle));
    let height_taper = 1.0 - (branch_fraction * 0.52);
    let shape_scale = match tree.genotype.crown_shape {
        CrownShape::Conical => height_taper,
        CrownShape::Columnar => 0.58 + (height_taper * 0.20),
        CrownShape::Rounded => 0.76 + (height_taper * 0.16),
        CrownShape::Spreading => 0.92 + (height_taper * 0.18),
    };
    let damage_scale = 1.0 - (f64_as_f32(tree.damage_fraction) * 0.48);
    let crown_radius = f64_as_f32(tree.crown_radius_meters);
    let length = crown_radius * shape_scale * damage_scale;
    let end = start + (direction.normalize_or_zero() * length);
    append_tapered_cylinder(
        vertices,
        indices,
        CylinderSpec {
            start,
            end,
            start_radius: frame.trunk_radius * (0.20 * height_taper).max(0.07),
            end_radius: frame.trunk_radius * 0.045,
            sides: 4,
            color: bark_color(tree),
        },
    )?;
    if tree_has_foliage(tree) && tree.genotype.crown_shape != CrownShape::Conical {
        let cluster_radius = crown_radius
            * (0.22 + (f64_as_f32(tree.genotype.leaf_density_fraction) * 0.16))
            * damage_scale;
        let vertical_scale = match tree.genotype.crown_shape {
            CrownShape::Columnar => 1.35,
            CrownShape::Spreading => 0.72,
            CrownShape::Conical | CrownShape::Rounded => 1.0,
        };
        append_octahedral_crown(
            vertices,
            indices,
            end,
            Vec3::new(
                cluster_radius,
                cluster_radius * vertical_scale,
                cluster_radius,
            ),
            foliage_color(tree),
        )?;
    }
    Ok(())
}

fn append_terminal_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
    crown_start: f32,
    crown_radius: f32,
    foliage: [f32; 4],
) -> Result<(), RendererError> {
    match tree.genotype.crown_shape {
        CrownShape::Conical => append_conical_crown(
            vertices,
            indices,
            frame.base + (frame.trunk_vector * crown_start),
            frame.top + (Vec3::Y * crown_radius * 0.18),
            crown_radius,
            foliage,
        ),
        CrownShape::Columnar | CrownShape::Rounded | CrownShape::Spreading => {
            append_octahedral_crown(
                vertices,
                indices,
                frame.base + (frame.trunk_vector * 0.82),
                Vec3::new(
                    crown_radius * 0.72,
                    crown_radius
                        * if tree.genotype.crown_shape == CrownShape::Columnar {
                            1.25
                        } else {
                            0.82
                        },
                    crown_radius * 0.72,
                ),
                foliage,
            )
        }
    }
}

fn append_sapling_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    base: Vec3,
    top: Vec3,
) -> Result<(), RendererError> {
    if !tree_has_foliage(tree) {
        return Ok(());
    }
    let radius = f64_as_f32(tree.crown_radius_meters);
    if tree.genotype.crown_shape == CrownShape::Conical {
        append_conical_crown(
            vertices,
            indices,
            base + ((top - base) * 0.36),
            top,
            radius,
            foliage_color(tree),
        )
    } else {
        append_octahedral_crown(
            vertices,
            indices,
            base + ((top - base) * 0.72),
            Vec3::new(radius, radius * 1.15, radius),
            foliage_color(tree),
        )
    }
}

fn branch_count(tree: ProceduralTree) -> usize {
    let density = tree.genotype.branch_density_fraction * (1.0 - (tree.damage_fraction * 0.58));
    let mut count = 4_usize;
    for threshold in [0.18, 0.32, 0.46, 0.60, 0.74, 0.88] {
        if density >= threshold {
            count += 1;
        }
    }
    if tree.condition == TreeCondition::StormBroken {
        count.saturating_sub(2)
    } else {
        count
    }
}

fn tree_has_foliage(tree: ProceduralTree) -> bool {
    !matches!(
        tree.condition,
        TreeCondition::DeadStanding | TreeCondition::StormBroken
    )
}

fn bark_color(tree: ProceduralTree) -> [f32; 4] {
    let base = match tree.genotype.bark_style {
        BarkStyle::Scaly => [0.25, 0.18, 0.11],
        BarkStyle::Smooth => [0.43, 0.40, 0.33],
        BarkStyle::Furrowed => [0.27, 0.20, 0.14],
        BarkStyle::Plated => [0.36, 0.29, 0.18],
    };
    let bleaching = if tree.condition == TreeCondition::DeadStanding {
        0.46
    } else {
        f64_as_f32(tree.damage_fraction) * 0.12
    };
    [
        base[0] + bleaching,
        base[1] + bleaching,
        base[2] + (bleaching * 0.88),
        1.0,
    ]
}

fn foliage_color(tree: ProceduralTree) -> [f32; 4] {
    let base = match tree.genotype.functional_group {
        TreeFunctionalGroup::EvergreenNeedleleaf => [0.055, 0.24, 0.12],
        TreeFunctionalGroup::ColdDeciduous => [0.25, 0.43, 0.12],
        TreeFunctionalGroup::TemperateBroadleaf => [0.10, 0.36, 0.075],
        TreeFunctionalGroup::DryWoodland => [0.34, 0.40, 0.13],
    };
    let variation = (hash_lane(tree.id, 31) - 0.5) * 0.10;
    let damage = f64_as_f32(tree.damage_fraction);
    [
        (base[0] + variation + (damage * 0.12)).clamp(0.0, 1.0),
        (base[1] + variation - (damage * 0.10)).clamp(0.0, 1.0),
        (base[2] + (variation * 0.5) - (damage * 0.04)).clamp(0.0, 1.0),
        1.0,
    ]
}

#[derive(Clone, Copy, Debug)]
struct CylinderSpec {
    start: Vec3,
    end: Vec3,
    start_radius: f32,
    end_radius: f32,
    sides: usize,
    color: [f32; 4],
}

fn append_tapered_cylinder(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    spec: CylinderSpec,
) -> Result<(), RendererError> {
    let axis = (spec.end - spec.start).normalize_or_zero();
    if axis == Vec3::ZERO {
        return Ok(());
    }
    let reference = if axis.y.abs() < 0.92 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = axis.cross(reference).normalize_or_zero();
    let bitangent = axis.cross(tangent).normalize_or_zero();
    let base_index = u32::try_from(vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    for ring in 0..2 {
        let (center, radius) = if ring == 0 {
            (spec.start, spec.start_radius)
        } else {
            (spec.end, spec.end_radius)
        };
        for side in 0..spec.sides {
            let angle = usize_as_f32(side) / usize_as_f32(spec.sides) * std::f32::consts::TAU;
            let radial = (tangent * libm::cosf(angle)) + (bitangent * libm::sinf(angle));
            let position = center + (radial * radius);
            vertices.push(local_vertex(position, radial, spec.color, 0.0));
        }
    }
    for side in 0..spec.sides {
        let next = (side + 1) % spec.sides;
        let side = u32::try_from(side).map_err(|_| RendererError::TooManyIndices)?;
        let next = u32::try_from(next).map_err(|_| RendererError::TooManyIndices)?;
        let sides = u32::try_from(spec.sides).map_err(|_| RendererError::TooManyIndices)?;
        indices.extend_from_slice(&[
            base_index + side,
            base_index + next,
            base_index + sides + side,
            base_index + next,
            base_index + sides + next,
            base_index + sides + side,
        ]);
    }
    Ok(())
}

fn append_conical_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    base: Vec3,
    apex: Vec3,
    radius: f32,
    color: [f32; 4],
) -> Result<(), RendererError> {
    let sides = 9_usize;
    let base_index = u32::try_from(vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    let axis = (apex - base).normalize_or_zero();
    let reference = if axis.y.abs() < 0.92 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = axis.cross(reference).normalize_or_zero();
    let bitangent = axis.cross(tangent).normalize_or_zero();
    for side in 0..sides {
        let angle = usize_as_f32(side) / usize_as_f32(sides) * std::f32::consts::TAU;
        let radial = (tangent * libm::cosf(angle)) + (bitangent * libm::sinf(angle));
        vertices.push(local_vertex(
            base + (radial * radius),
            (radial + (axis * 0.35)).normalize_or_zero(),
            color,
            0.0,
        ));
    }
    vertices.push(local_vertex(apex, axis, color, 0.0));
    let apex_index =
        base_index + u32::try_from(sides).map_err(|_| RendererError::TooManyIndices)?;
    vertices.push(local_vertex(base, -axis, color, 0.0));
    let base_center_index = apex_index + 1;
    for side in 0..sides {
        let next = (side + 1) % sides;
        let side_index =
            base_index + u32::try_from(side).map_err(|_| RendererError::TooManyIndices)?;
        let next_index =
            base_index + u32::try_from(next).map_err(|_| RendererError::TooManyIndices)?;
        indices.extend_from_slice(&[side_index, next_index, apex_index]);
        indices.extend_from_slice(&[base_center_index, next_index, side_index]);
    }
    Ok(())
}

fn append_octahedral_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    center: Vec3,
    radius: Vec3,
    color: [f32; 4],
) -> Result<(), RendererError> {
    let base_index = u32::try_from(vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    let offsets = [
        Vec3::Y * radius.y,
        Vec3::X * radius.x,
        Vec3::Z * radius.z,
        -Vec3::X * radius.x,
        -Vec3::Z * radius.z,
        -Vec3::Y * radius.y,
    ];
    for offset in offsets {
        vertices.push(local_vertex(
            center + offset,
            offset.normalize_or_zero(),
            color,
            0.0,
        ));
    }
    for triangle in [
        [0, 2, 1],
        [0, 3, 2],
        [0, 4, 3],
        [0, 1, 4],
        [5, 1, 2],
        [5, 2, 3],
        [5, 3, 4],
        [5, 4, 1],
    ] {
        indices.extend(triangle.map(|index| base_index + index));
    }
    Ok(())
}

fn hash_lane(key: u64, lane: usize) -> f32 {
    let lane = u32::try_from(lane % 8).expect("hash lane is bounded");
    let byte = u8::try_from((key >> (lane * 8)) & 0xff).expect("masked hash lane fits u8");
    f32::from(byte) / 255.0
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f32(value: usize) -> f32 {
    value as f32
}

fn usize_as_u32(value: usize) -> Result<u32, RendererError> {
    u32::try_from(value).map_err(|_| RendererError::TooManyIndices)
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
    fn high_low_positions_preserve_submeter_camera_offsets_after_distant_warps() {
        let origin = [5_000_000.0, 800.0, -5_000_000.0];
        let position = [5_000_000.125, 799.9375, -4_999_999.875];
        let (origin_high, origin_low) = split_position(origin);
        let (position_high, position_low) = split_position(position);
        let relative: [f32; 3] = std::array::from_fn(|axis| {
            (position_high[axis] - origin_high[axis]) + (position_low[axis] - origin_low[axis])
        });

        assert!(
            relative
                .into_iter()
                .zip([0.125, -0.0625, 0.125])
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn distant_mountains_do_not_require_voxel_interiors() {
        assert_eq!(terrain_tier(30_000.0), TerrainRenderTier::Horizon);
    }

    #[test]
    fn snow_depth_uses_a_bounded_grid_independent_of_mesh_density() {
        let mesh = Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 0.0, 2.0],
                [0.0, 0.0, 2.0],
                [1.0, 0.0, 1.0],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 5],
            colors: Vec::new(),
            indices: vec![0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4],
        };
        let mut query_count = 0;
        let grid = SnowDepthGrid::sample(&mesh, |x, z| {
            query_count += 1;
            Some((x + z) * 0.25)
        });

        assert_eq!(query_count, 9);
        assert!((grid.coverage_at([1.0, 0.0, 1.0]) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn procedural_tree_grammars_build_well_formed_colored_geometry() {
        let trees = [
            tree_fixture(
                1,
                TreeFunctionalGroup::EvergreenNeedleleaf,
                CrownShape::Conical,
                TreeCondition::Mature,
            ),
            tree_fixture(
                2,
                TreeFunctionalGroup::ColdDeciduous,
                CrownShape::Columnar,
                TreeCondition::Sapling,
            ),
            tree_fixture(
                3,
                TreeFunctionalGroup::TemperateBroadleaf,
                CrownShape::Rounded,
                TreeCondition::Ancient,
            ),
            tree_fixture(
                4,
                TreeFunctionalGroup::DryWoodland,
                CrownShape::Spreading,
                TreeCondition::Fallen,
            ),
        ];
        let (vertices, indices) =
            procedural_tree_geometry(&trees, TreeMeshDetail::Full, |x, z| Some((x + z) * 0.01))
                .expect("tree geometry");

        assert!(!vertices.is_empty());
        assert!(!indices.is_empty());
        assert!(indices.len().is_multiple_of(3));
        assert!(
            indices
                .iter()
                .all(|&index| usize::try_from(index).is_ok_and(|index| index < vertices.len()))
        );
        assert!(vertices.iter().all(|vertex| {
            vertex.position_high.into_iter().all(f32::is_finite)
                && vertex.position_low.into_iter().all(f32::is_finite)
                && vertex.normal.into_iter().all(f32::is_finite)
                && (vertex.color[3] - 1.0).abs() < f32::EPSILON
        }));
        assert_front_facing_geometry(&vertices, &indices);
    }

    #[test]
    fn tree_lod_tiers_preserve_individuals_while_reducing_geometry() {
        let trees = [
            tree_fixture(
                11,
                TreeFunctionalGroup::EvergreenNeedleleaf,
                CrownShape::Conical,
                TreeCondition::Mature,
            ),
            tree_fixture(
                12,
                TreeFunctionalGroup::TemperateBroadleaf,
                CrownShape::Rounded,
                TreeCondition::Ancient,
            ),
            tree_fixture(
                13,
                TreeFunctionalGroup::DryWoodland,
                CrownShape::Spreading,
                TreeCondition::DeadStanding,
            ),
        ];
        let geometry = [
            TreeMeshDetail::Full,
            TreeMeshDetail::Simplified,
            TreeMeshDetail::Silhouette,
        ]
        .map(|detail| {
            procedural_tree_geometry(&trees, detail, |_, _| Some(42.0)).expect("tree LOD geometry")
        });

        assert!(geometry.iter().all(|(vertices, indices)| {
            !vertices.is_empty()
                && !indices.is_empty()
                && indices
                    .iter()
                    .all(|&index| usize::try_from(index).is_ok_and(|index| index < vertices.len()))
        }));
        assert!(geometry[0].0.len() > geometry[1].0.len());
        assert!(geometry[1].0.len() > geometry[2].0.len());
        assert!(geometry[0].1.len() > geometry[1].1.len());
        assert!(geometry[1].1.len() > geometry[2].1.len());
    }

    #[test]
    fn surface_rocks_build_well_formed_irregular_colored_geometry() {
        let rocks = [
            rock_fixture(10, RockForm::RoundedBoulder),
            rock_fixture(20, RockForm::AngularBlock),
            rock_fixture(30, RockForm::Slab),
            rock_fixture(40, RockForm::ScreeFragment),
        ];
        let (vertices, indices) =
            procedural_rock_geometry(&rocks, |x, z| Some((x - z) * 0.01)).expect("rock geometry");

        assert!(!vertices.is_empty());
        assert!(!indices.is_empty());
        assert!(indices.len().is_multiple_of(3));
        assert!(
            indices
                .iter()
                .all(|&index| usize::try_from(index).is_ok_and(|index| index < vertices.len()))
        );
        assert!(vertices.iter().all(|vertex| {
            vertex.position_high.into_iter().all(f32::is_finite)
                && vertex.position_low.into_iter().all(f32::is_finite)
                && vertex.normal.into_iter().all(f32::is_finite)
                && vertex.color[..3]
                    .iter()
                    .all(|channel| (0.0..=1.0).contains(channel))
                && (vertex.color[3] - 1.0).abs() < f32::EPSILON
        }));
        assert_front_facing_geometry(&vertices, &indices);
    }

    #[test]
    fn ground_vegetation_builds_well_formed_distinct_growth_forms() {
        let plants = [
            ground_plant_fixture(50, GroundCoverGroup::Graminoid),
            ground_plant_fixture(51, GroundCoverGroup::Forb),
            ground_plant_fixture(52, GroundCoverGroup::Fern),
            ground_plant_fixture(53, GroundCoverGroup::LowShrub),
            ground_plant_fixture(54, GroundCoverGroup::Moss),
        ];
        let (vertices, indices) =
            procedural_ground_vegetation_geometry(&plants, |x, z| Some((x + z) * 0.01))
                .expect("ground vegetation geometry");

        assert!(!vertices.is_empty());
        assert!(!indices.is_empty());
        assert!(indices.len().is_multiple_of(3));
        assert!(
            indices
                .iter()
                .all(|&index| usize::try_from(index).is_ok_and(|index| index < vertices.len()))
        );
        assert!(vertices.iter().all(|vertex| {
            vertex.position_high.into_iter().all(f32::is_finite)
                && vertex.position_low.into_iter().all(f32::is_finite)
                && vertex.normal.into_iter().all(f32::is_finite)
                && vertex.color[..3]
                    .iter()
                    .all(|channel| (0.0..=1.0).contains(channel))
                && (vertex.color[3] - 1.0).abs() < f32::EPSILON
        }));
        assert_front_facing_geometry(&vertices, &indices);
        assert!(
            vertices.iter().any(|vertex| {
                vertex.color[0] > vertex.color[1] * 1.35 || vertex.color[2] > vertex.color[1] * 1.35
            }),
            "flowering forbs should add a visible non-green accent"
        );
    }

    fn assert_front_facing_geometry(vertices: &[TerrainVertex], indices: &[u32]) {
        for (triangle_index, triangle) in indices.chunks_exact(3).enumerate() {
            let positions = [triangle[0], triangle[1], triangle[2]].map(|index| {
                let vertex = vertices[usize::try_from(index).expect("test index fits usize")];
                Vec3::from_array(vertex.position_high) + Vec3::from_array(vertex.position_low)
            });
            let geometric_normal = (positions[1] - positions[0]).cross(positions[2] - positions[0]);
            if geometric_normal.length_squared() <= f32::EPSILON {
                continue;
            }
            let vertex_normal = triangle
                .iter()
                .map(|&index| {
                    Vec3::from_array(
                        vertices[usize::try_from(index).expect("test index fits usize")].normal,
                    )
                })
                .sum::<Vec3>();
            assert!(
                geometric_normal.dot(vertex_normal) > 0.0,
                "triangle {triangle_index} faces away from its vertex normals"
            );
        }
    }

    fn ground_plant_fixture(id: u64, group: GroundCoverGroup) -> GroundPlant {
        GroundPlant {
            id,
            x: f64::from(u32::try_from(id).expect("small fixture id")) * 0.4,
            z: -6.0,
            height_meters: match group {
                GroundCoverGroup::Moss => 0.10,
                GroundCoverGroup::LowShrub => 1.0,
                _ => 0.62,
            },
            radius_meters: 0.38,
            rotation_turns: 0.31,
            lean_direction: [0.8, 0.6],
            lean_fraction: 0.14,
            flowering_fraction: if group == GroundCoverGroup::Forb {
                0.8
            } else {
                0.0
            },
            genotype: treeline_ecology::GroundPlantGenotype {
                group,
                leaf_count: 6,
                spread_fraction: 0.82,
                slenderness_fraction: 0.64,
                color_variation_fraction: 0.55,
            },
        }
    }

    fn rock_fixture(id: u64, form: RockForm) -> SurfaceRock {
        SurfaceRock {
            id,
            x: f64::from(u32::try_from(id).expect("small fixture id")),
            z: -8.0,
            radii_meters: [1.2, 0.8, 1.0],
            rotation_turns: 0.27,
            tilt_direction: [0.8, 0.6],
            tilt_fraction: 0.12,
            embedded_fraction: 0.22,
            moss_fraction: 0.34,
            genotype: treeline_ecology::RockGenotype {
                form,
                hardness_fraction: 0.72,
                weathering_fraction: 0.46,
                fracture_fraction: 0.58,
                roundness_fraction: if form == RockForm::RoundedBoulder {
                    0.88
                } else {
                    0.24
                },
                carbonate_fraction: 0.36,
            },
        }
    }

    fn tree_fixture(
        id: u64,
        group: TreeFunctionalGroup,
        crown_shape: CrownShape,
        condition: TreeCondition,
    ) -> ProceduralTree {
        ProceduralTree {
            id,
            x: f64::from(u32::try_from(id).expect("small fixture id")) * 8.0,
            z: -4.0,
            age_years: 120.0,
            height_meters: 18.0,
            trunk_base_radius_meters: 0.42,
            crown_radius_meters: 3.8,
            lean_direction: [0.8, 0.6],
            lean_fraction: if condition == TreeCondition::Fallen {
                0.92
            } else {
                0.08
            },
            damage_fraction: 0.18,
            rotation_turns: 0.37,
            condition,
            genotype: treeline_ecology::TreeGenotype {
                functional_group: group,
                mature_height_meters: 24.0,
                height_variation_fraction: 0.5,
                trunk_taper_fraction: 0.62,
                branching_angle_radians: 0.84,
                branch_density_fraction: 0.72,
                crown_shape,
                leaf_density_fraction: 0.78,
                bark_style: BarkStyle::Furrowed,
                slope_response_fraction: 0.5,
                wind_response_fraction: 0.6,
                competition_response_fraction: 0.7,
            },
        }
    }
}
