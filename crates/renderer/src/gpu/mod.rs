//! wgpu resources: layouts, pipelines, bind groups, and render targets.

mod bindings;
mod pipeline;
mod shadow;

pub(crate) use bindings::TerrainBindings;
pub(crate) use pipeline::{
    WorldPipelines, create_shadow_pipeline, create_sky_pipeline, create_world_pipelines,
};
pub(crate) use shadow::{ShadowBindings, ShadowMap};

pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

pub(crate) const fn uniform_layout_entry(
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

pub(crate) const fn depth_texture_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(crate) const fn comparison_sampler_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
        count: None,
    }
}

pub(crate) const fn sampled_texture_array_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}

pub(crate) const fn filtering_sampler_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// One independently owned mesh: terrain, water, or a tile's whole stand of
/// trees, trunks and foliage alike.
///
/// A stand's indices arrive sorted into three runs, because three passes each
/// want a different prefix or suffix of them: everything opaque first, then the
/// outermost shell of every ball of needles, then the shells nested inside
/// those. Terrain and water are all opaque and leave the other two empty.
#[derive(Debug)]
pub struct TerrainMesh {
    pub(crate) vertex_buffer: wgpu::Buffer,
    pub(crate) index_buffer: wgpu::Buffer,
    pub(crate) opaque_index_count: u32,
    pub(crate) foliage_hull_index_count: u32,
    pub(crate) foliage_interior_index_count: u32,
}

impl TerrainMesh {
    /// The indices the ground pipeline draws.
    pub(crate) fn opaque_indices(&self) -> std::ops::Range<u32> {
        0..self.opaque_index_count
    }

    /// The indices the foliage pipeline draws: every shell of every ball.
    pub(crate) fn foliage_indices(&self) -> std::ops::Range<u32> {
        self.opaque_index_count..self.all_index_count()
    }

    /// The indices a shadow cascade draws.
    ///
    /// Everything solid, and then the hull of each ball of needles but nothing
    /// behind it. A ball's outermost shell encloses the rest, so the sun sees
    /// the same crown either way — and the shells it no longer rasterizes are
    /// four fifths of the foliage in the frame, three cascades over.
    pub(crate) fn shadow_indices(&self) -> std::ops::Range<u32> {
        0..(self.opaque_index_count + self.foliage_hull_index_count)
    }

    fn all_index_count(&self) -> u32 {
        self.opaque_index_count + self.foliage_hull_index_count + self.foliage_interior_index_count
    }
}

#[derive(Debug)]
pub(crate) struct DepthTarget {
    pub(crate) view: wgpu::TextureView,
}

impl DepthTarget {
    pub(crate) fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
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
