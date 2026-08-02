//! The renderer itself: what it owns, and what a frame does.
//!
//! One pipeline draws every surface. A frame renders the sun's shadow cascades
//! first, then the sky backdrop, then far terrain, then near terrain — far
//! before near so the near tier's depth wins wherever the two overlap.

use glam::Mat4;
use treeline_ecology::ProceduralTree;
use treeline_mesher::Mesh;
use wgpu::util::DeviceExt;

use crate::gpu::{
    DepthTarget, ShadowBindings, ShadowMap, TerrainBindings, TerrainMesh, create_shadow_pipeline,
    create_sky_pipeline, create_terrain_pipeline,
};
use crate::lighting::{AtmosphereSettings, LightingSettings};
use crate::material::MaterialTextures;
use crate::snow::SnowDepthGrid;
use crate::tree_mesh::procedural_tree_geometry;
use crate::uniform::{
    CameraUniform, SHADOW_CASCADE_COUNT, ShadowCameraUniform, TerrainCutoutUniform,
    atmosphere_uniform, lighting_uniform,
};
use crate::vertex::{
    SURFACE_KIND_SOLID, SURFACE_KIND_WATER, f64_as_f32, mesh_vertices, split_f64, split_position,
    terrain_vertex,
};
use crate::{RendererError, TreeMeshDetail};

#[derive(Debug)]
pub struct TerrainRenderer {
    pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    far_bind_group: wgpu::BindGroup,
    near_bind_group: wgpu::BindGroup,
    far_cutout_buffer: wgpu::Buffer,
    atmosphere_buffer: wgpu::Buffer,
    lighting_buffer: wgpu::Buffer,
    shadow_map: ShadowMap,
    shadow_camera_buffers: [wgpu::Buffer; SHADOW_CASCADE_COUNT],
    shadow_bind_groups: [wgpu::BindGroup; SHADOW_CASCADE_COUNT],
    _material_textures: MaterialTextures,
    water_animation_seconds: f64,
    depth: DepthTarget,
}

impl TerrainRenderer {
    /// Creates the shared lit terrain pipeline and camera/depth resources.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let shadow_map = ShadowMap::new(device);
        let material_textures = MaterialTextures::new(device, queue);
        let bindings = TerrainBindings::new(device, &shadow_map, &material_textures);
        let shadow_bindings = ShadowBindings::new(device);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain pipeline layout"),
            bind_group_layouts: &[&bindings.layout],
            push_constant_ranges: &[],
        });
        let pipeline = create_terrain_pipeline(device, &pipeline_layout, surface_format);
        let sky_pipeline = create_sky_pipeline(device, &pipeline_layout, surface_format);
        let shadow_pipeline = create_shadow_pipeline(device, &shadow_bindings.layout);

        Self {
            pipeline,
            sky_pipeline,
            shadow_pipeline,
            camera_buffer: bindings.camera_buffer,
            far_bind_group: bindings.far_bind_group,
            near_bind_group: bindings.near_bind_group,
            far_cutout_buffer: bindings.far_cutout_buffer,
            atmosphere_buffer: bindings.atmosphere_buffer,
            lighting_buffer: bindings.lighting_buffer,
            shadow_map,
            shadow_camera_buffers: shadow_bindings.camera_buffers,
            shadow_bind_groups: shadow_bindings.bind_groups,
            _material_textures: material_textures,
            water_animation_seconds: 0.0,
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
        Self::upload_mesh_with_kind(device, mesh, SURFACE_KIND_SOLID, "terrain vertices")
    }

    /// Uploads a dedicated water sheet. Water vertices use the supplied
    /// hydrology color while the shader adds surface reflection, glints, and
    /// distance-aware aerial perspective without affecting solid terrain.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::TooManyIndices`] when the mesh cannot be
    /// addressed using the renderer's `u32` draw count.
    pub fn upload_water_mesh(
        &self,
        device: &wgpu::Device,
        mesh: &Mesh,
    ) -> Result<TerrainMesh, RendererError> {
        Self::upload_mesh_with_kind(device, mesh, SURFACE_KIND_WATER, "water vertices")
    }

    pub(crate) fn upload_mesh_with_kind(
        device: &wgpu::Device,
        mesh: &Mesh,
        surface_kind: f32,
        vertex_label: &str,
    ) -> Result<TerrainMesh, RendererError> {
        let vertices = mesh_vertices(mesh, surface_kind);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(vertex_label),
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

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.depth = DepthTarget::new(device, width, height);
    }

    pub fn update_camera(
        &self,
        queue: &wgpu::Queue,
        view_projection: [[f32; 4]; 4],
        render_origin: [f64; 3],
        view_direction: [f32; 3],
        lighting: LightingSettings,
    ) {
        let (render_origin_high, render_origin_low) = split_position(render_origin);
        let inverse_view_projection = Mat4::from_cols_array_2d(&view_projection)
            .inverse()
            .to_cols_array_2d();
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&CameraUniform {
                view_projection,
                inverse_view_projection,
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
        let lighting = lighting_uniform(lighting, render_origin, view_direction);
        queue.write_buffer(&self.lighting_buffer, 0, bytemuck::bytes_of(&lighting));
        for (cascade, buffer) in self.shadow_camera_buffers.iter().enumerate() {
            queue.write_buffer(
                buffer,
                0,
                bytemuck::bytes_of(&ShadowCameraUniform {
                    view_projection: lighting.shadow_view_projection[cascade],
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
    }

    /// Updates climate-derived fog and wind controls without rebuilding any
    /// resident terrain or render pipeline resources.
    pub fn update_atmosphere(&self, queue: &wgpu::Queue, settings: AtmosphereSettings) {
        queue.write_buffer(
            &self.atmosphere_buffer,
            0,
            bytemuck::bytes_of(&atmosphere_uniform(settings)),
        );
    }

    /// Advances the visual water waves without changing deterministic water
    /// simulation state or rebuilding any material resources.
    pub fn advance_water_time(&mut self, queue: &wgpu::Queue, delta_seconds: f64) {
        self.water_animation_seconds += delta_seconds.max(0.0);
        let elapsed_seconds = f64_as_f32(self.water_animation_seconds);
        queue.write_buffer(
            &self.atmosphere_buffer,
            28,
            bytemuck::bytes_of(&elapsed_seconds),
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
        shadow_meshes: &[&TerrainMesh],
    ) {
        for cascade in 0..SHADOW_CASCADE_COUNT {
            let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sun shadow cascade pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_map.layer_views[cascade],
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            shadow_pass.set_pipeline(&self.shadow_pipeline);
            shadow_pass.set_bind_group(0, &self.shadow_bind_groups[cascade], &[]);
            for mesh in shadow_meshes {
                shadow_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                shadow_pass
                    .set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                shadow_pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terrain render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
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
        pass.set_pipeline(&self.sky_pipeline);
        pass.set_bind_group(0, &self.near_bind_group, &[]);
        pass.draw(0..3, 0..1);
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
