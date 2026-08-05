//! Bind groups wiring uniforms, shadow maps, and material textures together.
//!
//! Near and far terrain share one layout but need different cutout rectangles,
//! so each gets its own bind group over the same buffers.

use wgpu::util::DeviceExt;

use crate::gpu::shadow::ShadowMap;
use crate::gpu::{
    comparison_sampler_layout_entry, depth_texture_layout_entry, filtering_sampler_layout_entry,
    sampled_texture_array_layout_entry, uniform_layout_entry,
};
use crate::lighting::{AtmosphereSettings, LightingSettings};
use crate::material::MaterialTextures;
use crate::uniform::{CameraUniform, TerrainCutoutUniform, atmosphere_uniform, lighting_uniform};

pub(crate) struct TerrainBindings {
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) camera_buffer: wgpu::Buffer,
    pub(crate) far_bind_group: wgpu::BindGroup,
    pub(crate) near_bind_group: wgpu::BindGroup,
    pub(crate) far_cutout_buffer: wgpu::Buffer,
    pub(crate) atmosphere_buffer: wgpu::Buffer,
    pub(crate) lighting_buffer: wgpu::Buffer,
}

impl TerrainBindings {
    pub(crate) fn new(
        device: &wgpu::Device,
        shadow_map: Option<&ShadowMap>,
        material_textures: &MaterialTextures,
    ) -> Self {
        let camera_uniform = CameraUniform {
            view_projection: [[0.0; 4]; 4],
            inverse_view_projection: [[0.0; 4]; 4],
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

        // Materials live at bindings 4-7 always. Shadows, when the backend
        // supports them, take bindings 8-11; on backends without shadow maps
        // (WebGL2) those bindings and the shader that samples them are omitted
        // entirely rather than bound to a texture that cannot exist.
        let mut layout_entries = vec![
            uniform_layout_entry(0, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
            uniform_layout_entry(1, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
            // Wind reaches the vertex stage too: needle shells sway on it.
            uniform_layout_entry(2, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
            uniform_layout_entry(3, wgpu::ShaderStages::FRAGMENT),
            sampled_texture_array_layout_entry(4),
            sampled_texture_array_layout_entry(5),
            sampled_texture_array_layout_entry(6),
            filtering_sampler_layout_entry(7),
        ];
        if shadow_map.is_some() {
            layout_entries.extend([
                depth_texture_layout_entry(8),
                depth_texture_layout_entry(9),
                depth_texture_layout_entry(10),
                comparison_sampler_layout_entry(11),
            ]);
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bind group layout"),
            entries: &layout_entries,
        });
        let atmosphere = AtmosphereSettings::default();
        let atmosphere_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("atmosphere uniform"),
            contents: bytemuck::bytes_of(&atmosphere_uniform(atmosphere)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let lighting = lighting_uniform(LightingSettings::default(), [0.0; 3], [0.0, 0.0, -1.0]);
        let lighting_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("lighting uniform"),
            contents: bytemuck::bytes_of(&lighting),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let resources = |cutout_buffer| TerrainBindGroupResources {
            camera_buffer: &camera_buffer,
            cutout_buffer,
            atmosphere_buffer: &atmosphere_buffer,
            lighting_buffer: &lighting_buffer,
            shadow_map,
            material_textures,
        };
        let far_bind_group = terrain_bind_group(
            device,
            &layout,
            resources(&far_cutout_buffer),
            "far terrain bind group",
        );
        let near_bind_group = terrain_bind_group(
            device,
            &layout,
            resources(&no_cutout_buffer),
            "near terrain bind group",
        );

        Self {
            layout,
            camera_buffer,
            far_bind_group,
            near_bind_group,
            far_cutout_buffer,
            atmosphere_buffer,
            lighting_buffer,
        }
    }
}

pub(crate) fn cutout_buffer(
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

#[derive(Clone, Copy)]
pub(crate) struct TerrainBindGroupResources<'a> {
    pub(crate) camera_buffer: &'a wgpu::Buffer,
    pub(crate) cutout_buffer: &'a wgpu::Buffer,
    pub(crate) atmosphere_buffer: &'a wgpu::Buffer,
    pub(crate) lighting_buffer: &'a wgpu::Buffer,
    pub(crate) shadow_map: Option<&'a ShadowMap>,
    pub(crate) material_textures: &'a MaterialTextures,
}

pub(crate) fn terrain_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    resources: TerrainBindGroupResources<'_>,
    label: &str,
) -> wgpu::BindGroup {
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: resources.camera_buffer.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: resources.cutout_buffer.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: resources.atmosphere_buffer.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 3,
            resource: resources.lighting_buffer.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::TextureView(&resources.material_textures.diffuse_view),
        },
        wgpu::BindGroupEntry {
            binding: 5,
            resource: wgpu::BindingResource::TextureView(&resources.material_textures.normal_view),
        },
        wgpu::BindGroupEntry {
            binding: 6,
            resource: wgpu::BindingResource::TextureView(&resources.material_textures.arm_view),
        },
        wgpu::BindGroupEntry {
            binding: 7,
            resource: wgpu::BindingResource::Sampler(&resources.material_textures.sampler),
        },
    ];
    if let Some(shadow_map) = resources.shadow_map {
        entries.extend([
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&shadow_map.layer_views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(&shadow_map.layer_views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&shadow_map.layer_views[2]),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::Sampler(&shadow_map.sampler),
            },
        ]);
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}
