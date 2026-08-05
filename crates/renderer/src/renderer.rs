//! The renderer itself: what it owns, and what a frame does.
//!
//! A frame renders the sun's shadow cascades first, then the sky backdrop, then
//! far terrain, then near terrain — far before near so the near tier's depth
//! wins wherever the two overlap — and foliage last of all.
//!
//! Foliage goes last because it is the one surface that cuts holes in itself,
//! and so the one that cannot be depth-tested before it shades. Every solid
//! thing in the frame has written its depth by the time a crown is drawn, which
//! is the most the depth buffer can do for a pass that has to run to find out
//! whether it had anything to draw.

use glam::Mat4;
use treeline_ecology::ProceduralTree;
use treeline_mesher::Mesh;
use wgpu::util::DeviceExt;

use crate::gpu::{
    DepthTarget, ShadowBindings, ShadowMap, TerrainBindings, TerrainMesh, WorldPipelines,
    create_shadow_pipeline, create_sky_pipeline, create_world_pipelines,
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
    world: WorldPipelines,
    sky_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: Option<wgpu::RenderPipeline>,
    camera_buffer: wgpu::Buffer,
    far_bind_group: wgpu::BindGroup,
    near_bind_group: wgpu::BindGroup,
    far_cutout_buffer: wgpu::Buffer,
    atmosphere_buffer: wgpu::Buffer,
    lighting_buffer: wgpu::Buffer,
    shadow_map: Option<ShadowMap>,
    shadow_camera_buffers: Option<[wgpu::Buffer; SHADOW_CASCADE_COUNT]>,
    shadow_bind_groups: Option<[wgpu::BindGroup; SHADOW_CASCADE_COUNT]>,
    _material_textures: MaterialTextures,
    water_animation_seconds: f64,
    depth: DepthTarget,
}

impl TerrainRenderer {
    /// Creates the shared lit terrain pipeline and camera/depth resources.
    ///
    /// When `shadows` is false, no shadow map, shadow pipeline, or shadow
    /// bindings are created and every surface is lit as fully visible to the
    /// sun. Backends without depth texture support (WebGL2) pass false so the
    /// pipeline compiles at all.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        shadows: bool,
    ) -> Self {
        let material_textures = MaterialTextures::new(device, queue);
        let shadow_map = shadows.then(|| ShadowMap::new(device));
        let bindings = TerrainBindings::new(device, shadow_map.as_ref(), &material_textures);
        let (shadow_bind_groups, shadow_camera_buffers, shadow_pipeline) = if shadows {
            let shadow_bindings = ShadowBindings::new(device);
            let shadow_pipeline = create_shadow_pipeline(device, &shadow_bindings.layout);
            (
                Some(shadow_bindings.bind_groups),
                Some(shadow_bindings.camera_buffers),
                Some(shadow_pipeline),
            )
        } else {
            (None, None, None)
        };

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain pipeline layout"),
            bind_group_layouts: &[&bindings.layout],
            push_constant_ranges: &[],
        });
        let world = create_world_pipelines(
            device,
            &pipeline_layout,
            surface_format,
            shadow_map.is_some(),
        );
        let sky_pipeline = create_sky_pipeline(device, &pipeline_layout, surface_format);

        Self {
            world,
            sky_pipeline,
            shadow_pipeline,
            camera_buffer: bindings.camera_buffer,
            far_bind_group: bindings.far_bind_group,
            near_bind_group: bindings.near_bind_group,
            far_cutout_buffer: bindings.far_cutout_buffer,
            atmosphere_buffer: bindings.atmosphere_buffer,
            lighting_buffer: bindings.lighting_buffer,
            shadow_map,
            shadow_camera_buffers,
            shadow_bind_groups,
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
        Ok(TerrainMesh {
            vertex_buffer,
            index_buffer,
            opaque_index_count: u32::try_from(mesh.indices.len())
                .map_err(|_| RendererError::TooManyIndices)?,
            foliage_hull_index_count: 0,
            foliage_interior_index_count: 0,
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
        Ok(TerrainMesh {
            vertex_buffer,
            index_buffer,
            opaque_index_count: u32::try_from(mesh.indices.len())
                .map_err(|_| RendererError::TooManyIndices)?,
            foliage_hull_index_count: 0,
            foliage_interior_index_count: 0,
        })
    }

    /// Builds and uploads procedural tree grammars as one independently owned mesh.
    ///
    /// Tree bases are resolved against the composed world surface supplied by
    /// the caller, keeping ecology independent from streaming-world artifacts.
    /// Individuals without a surface sample are omitted.
    ///
    /// Trunks, branches, and crowns are all triangles in one buffer, so a whole
    /// tile of trees is one upload. It draws in two, because a needle shell
    /// cuts its own silhouette and a trunk does not: the opaque half goes down
    /// first, and the foliage half follows behind it in its own pipeline.
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
        let geometry = procedural_tree_geometry(trees, detail, surface_height)?;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("procedural tree vertices"),
            contents: bytemuck::cast_slice(&geometry.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        // Opaque, then needle hulls, then the shells behind them: one buffer,
        // three ranges, so no pass that wants a different run of it pays for a
        // binding change or a second upload.
        let indices = geometry.all_indices().collect::<Vec<_>>();
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("procedural tree indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Ok(TerrainMesh {
            vertex_buffer,
            index_buffer,
            opaque_index_count: u32::try_from(geometry.indices.len())
                .map_err(|_| RendererError::TooManyIndices)?,
            foliage_hull_index_count: u32::try_from(geometry.foliage_hull_indices.len())
                .map_err(|_| RendererError::TooManyIndices)?,
            foliage_interior_index_count: u32::try_from(geometry.foliage_interior_indices.len())
                .map_err(|_| RendererError::TooManyIndices)?,
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
        if let Some(shadow_camera_buffers) = &self.shadow_camera_buffers {
            for (cascade, buffer) in shadow_camera_buffers.iter().enumerate() {
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
        // Each tier is walked as a list rather than a stream because the far
        // and near bind groups differ, and switching those per mesh would cost
        // far more than collecting the two.
        let far_meshes = far_meshes.into_iter().collect::<Vec<_>>();
        let near_meshes = near_meshes.into_iter().collect::<Vec<_>>();

        if let (Some(shadow_map), Some(shadow_pipeline), Some(shadow_bind_groups)) = (
            &self.shadow_map,
            &self.shadow_pipeline,
            &self.shadow_bind_groups,
        ) {
            for (cascade, bind_group) in shadow_bind_groups.iter().enumerate() {
                let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("sun shadow cascade pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &shadow_map.layer_views[cascade],
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                shadow_pass.set_pipeline(shadow_pipeline);
                shadow_pass.set_bind_group(0, bind_group, &[]);
                draw_meshes(&mut shadow_pass, shadow_meshes, TerrainMesh::shadow_indices);
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
        pass.set_pipeline(&self.world.far_ground);
        pass.set_bind_group(0, &self.far_bind_group, &[]);
        draw_meshes(&mut pass, &far_meshes, TerrainMesh::opaque_indices);
        pass.set_pipeline(&self.world.near_ground);
        pass.set_bind_group(0, &self.near_bind_group, &[]);
        draw_meshes(&mut pass, &near_meshes, TerrainMesh::opaque_indices);
        // Foliage last, over a depth buffer every solid thing has already
        // written to. It is the one pass that shades before it is depth-tested,
        // so it is the one that gains most from having nothing left to occlude
        // it. Crowns live in the near tier, so the bind group already stands.
        pass.set_pipeline(&self.world.foliage);
        draw_meshes(&mut pass, &near_meshes, TerrainMesh::foliage_indices);
    }
}

/// Draws one range of each mesh, skipping the meshes that have none.
fn draw_meshes(
    pass: &mut wgpu::RenderPass<'_>,
    meshes: &[&TerrainMesh],
    range: fn(&TerrainMesh) -> std::ops::Range<u32>,
) {
    for mesh in meshes {
        let indices = range(mesh);
        if indices.is_empty() {
            continue;
        }
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(indices, 0, 0..1);
    }
}
