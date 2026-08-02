//! Cascaded shadow maps for the sun.

use wgpu::util::DeviceExt;

use crate::gpu::{DEPTH_FORMAT, uniform_layout_entry};
use crate::uniform::{SHADOW_CASCADE_COUNT, SHADOW_MAP_SIZE, ShadowCameraUniform};

#[derive(Debug)]
pub(crate) struct ShadowMap {
    pub(crate) _texture: wgpu::Texture,
    pub(crate) sampling_view: wgpu::TextureView,
    pub(crate) layer_views: [wgpu::TextureView; SHADOW_CASCADE_COUNT],
    pub(crate) sampler: wgpu::Sampler,
}

impl ShadowMap {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cascaded sun shadow maps"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_SIZE,
                height: SHADOW_MAP_SIZE,
                depth_or_array_layers: u32::try_from(SHADOW_CASCADE_COUNT)
                    .expect("shadow cascade count fits u32"),
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let sampling_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("sun shadow map array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layer_views = std::array::from_fn(|cascade| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("sun shadow cascade"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: u32::try_from(cascade).expect("cascade index fits u32"),
                array_layer_count: Some(1),
                ..Default::default()
            })
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sun shadow comparison sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Self {
            _texture: texture,
            sampling_view,
            layer_views,
            sampler,
        }
    }
}

pub(crate) struct ShadowBindings {
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) camera_buffers: [wgpu::Buffer; SHADOW_CASCADE_COUNT],
    pub(crate) bind_groups: [wgpu::BindGroup; SHADOW_CASCADE_COUNT],
}

impl ShadowBindings {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow camera bind group layout"),
            entries: &[uniform_layout_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let empty = ShadowCameraUniform {
            view_projection: [[0.0; 4]; 4],
            render_origin_high: [0.0; 4],
            render_origin_low: [0.0; 4],
        };
        let camera_buffers = std::array::from_fn(|_| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shadow camera uniform"),
                contents: bytemuck::bytes_of(&empty),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        });
        let bind_groups = std::array::from_fn(|cascade| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shadow camera bind group"),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffers[cascade].as_entire_binding(),
                }],
            })
        });
        Self {
            layout,
            camera_buffers,
            bind_groups,
        }
    }
}
