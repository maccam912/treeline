//! Scanned surface textures compiled into the binary.
//!
//! Four material layers share one array texture: ground, rock, and two barks.
//! Mip levels are generated on the CPU at load, which keeps the renderer free
//! of a compute pass purely for downsampling.

use image::ImageFormat;
use image::imageops::{FilterType, resize};

pub(crate) const MATERIAL_TEXTURE_EDGE: u32 = 512;
pub(crate) const MATERIAL_TEXTURE_LAYER_COUNT: u32 = 4;
pub(crate) const MATERIAL_TEXTURE_MIP_COUNT: u32 = 10;

pub(crate) const FOREST_FLOOR_DIFFUSE: &[u8] =
    include_bytes!("../assets/surfaces/forest_floor_diff_1k.jpg");
pub(crate) const FOREST_FLOOR_NORMAL: &[u8] =
    include_bytes!("../assets/surfaces/forest_floor_nor_gl_1k.jpg");
pub(crate) const FOREST_FLOOR_ARM: &[u8] =
    include_bytes!("../assets/surfaces/forest_floor_arm_1k.jpg");
pub(crate) const ROCK_FACE_DIFFUSE: &[u8] =
    include_bytes!("../assets/surfaces/rock_face_diff_1k.jpg");
pub(crate) const ROCK_FACE_NORMAL: &[u8] =
    include_bytes!("../assets/surfaces/rock_face_nor_gl_1k.jpg");
pub(crate) const ROCK_FACE_ARM: &[u8] = include_bytes!("../assets/surfaces/rock_face_arm_1k.jpg");
pub(crate) const PINE_BARK_DIFFUSE: &[u8] = include_bytes!("../assets/bark/pine_bark_diff_1k.jpg");
pub(crate) const PINE_BARK_NORMAL: &[u8] = include_bytes!("../assets/bark/pine_bark_nor_gl_1k.jpg");
pub(crate) const PINE_BARK_ARM: &[u8] = include_bytes!("../assets/bark/pine_bark_arm_1k.jpg");
pub(crate) const OAK_BARK_DIFFUSE: &[u8] =
    include_bytes!("../assets/bark/bark_brown_02_diff_1k.jpg");
pub(crate) const OAK_BARK_NORMAL: &[u8] =
    include_bytes!("../assets/bark/bark_brown_02_nor_gl_1k.jpg");
pub(crate) const OAK_BARK_ARM: &[u8] = include_bytes!("../assets/bark/bark_brown_02_arm_1k.jpg");

pub(crate) struct EmbeddedMaterial {
    diffuse: &'static [u8],
    normal: &'static [u8],
    arm: &'static [u8],
}

pub(crate) const EMBEDDED_MATERIALS: [EmbeddedMaterial; 4] = [
    EmbeddedMaterial {
        diffuse: FOREST_FLOOR_DIFFUSE,
        normal: FOREST_FLOOR_NORMAL,
        arm: FOREST_FLOOR_ARM,
    },
    EmbeddedMaterial {
        diffuse: ROCK_FACE_DIFFUSE,
        normal: ROCK_FACE_NORMAL,
        arm: ROCK_FACE_ARM,
    },
    EmbeddedMaterial {
        diffuse: PINE_BARK_DIFFUSE,
        normal: PINE_BARK_NORMAL,
        arm: PINE_BARK_ARM,
    },
    EmbeddedMaterial {
        diffuse: OAK_BARK_DIFFUSE,
        normal: OAK_BARK_NORMAL,
        arm: OAK_BARK_ARM,
    },
];

#[derive(Debug)]
pub(crate) struct MaterialTextures {
    pub(crate) _diffuse_texture: wgpu::Texture,
    pub(crate) diffuse_view: wgpu::TextureView,
    pub(crate) _normal_texture: wgpu::Texture,
    pub(crate) normal_view: wgpu::TextureView,
    pub(crate) _arm_texture: wgpu::Texture,
    pub(crate) arm_view: wgpu::TextureView,
    pub(crate) sampler: wgpu::Sampler,
}

impl MaterialTextures {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let diffuse_texture = create_material_texture(
            device,
            "Poly Haven material diffuse array",
            wgpu::TextureFormat::Rgba8UnormSrgb,
        );
        let normal_texture = create_material_texture(
            device,
            "Poly Haven material normal array",
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let arm_texture = create_material_texture(
            device,
            "Poly Haven material AO roughness metalness array",
            wgpu::TextureFormat::Rgba8Unorm,
        );
        upload_material_layers(
            queue,
            &diffuse_texture,
            EMBEDDED_MATERIALS.map(|material| material.diffuse),
        );
        upload_material_layers(
            queue,
            &normal_texture,
            EMBEDDED_MATERIALS.map(|material| material.normal),
        );
        upload_material_layers(
            queue,
            &arm_texture,
            EMBEDDED_MATERIALS.map(|material| material.arm),
        );
        let array_view = |texture: &wgpu::Texture, label| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(label),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let diffuse_view = array_view(&diffuse_texture, "Poly Haven material diffuse array view");
        let normal_view = array_view(&normal_texture, "Poly Haven material normal array view");
        let arm_view = array_view(&arm_texture, "Poly Haven material ARM array view");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("surface material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 4,
            ..Default::default()
        });
        Self {
            _diffuse_texture: diffuse_texture,
            diffuse_view,
            _normal_texture: normal_texture,
            normal_view,
            _arm_texture: arm_texture,
            arm_view,
            sampler,
        }
    }
}

pub(crate) fn create_material_texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: MATERIAL_TEXTURE_EDGE,
            height: MATERIAL_TEXTURE_EDGE,
            depth_or_array_layers: MATERIAL_TEXTURE_LAYER_COUNT,
        },
        mip_level_count: MATERIAL_TEXTURE_MIP_COUNT,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

pub(crate) fn upload_material_layers(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    encoded_layers: [&[u8]; 4],
) {
    for (layer, encoded) in encoded_layers.into_iter().enumerate() {
        let decoded = image::load_from_memory_with_format(encoded, ImageFormat::Jpeg)
            .expect("embedded Poly Haven material JPEG must decode")
            .to_rgba8();
        assert_eq!(
            decoded.dimensions(),
            (1_024, 1_024),
            "embedded Poly Haven material maps must retain their source dimensions"
        );
        let mut mip = resize(
            &decoded,
            MATERIAL_TEXTURE_EDGE,
            MATERIAL_TEXTURE_EDGE,
            FilterType::Triangle,
        );
        for mip_level in 0..MATERIAL_TEXTURE_MIP_COUNT {
            let (width, height) = mip.dimensions();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: u32::try_from(layer).expect("material layer count fits u32"),
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                mip.as_raw(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            if width > 1 {
                mip = resize(&mip, width / 2, height / 2, FilterType::Triangle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_map_decodes_at_the_expected_resolution() {
        for material in EMBEDDED_MATERIALS {
            for encoded in [material.diffuse, material.normal, material.arm] {
                let image = image::load_from_memory_with_format(encoded, ImageFormat::Jpeg)
                    .expect("embedded material map");
                assert_eq!((image.width(), image.height()), (1_024, 1_024));
            }
        }
    }

    #[test]
    fn the_mip_chain_reaches_a_single_pixel() {
        assert_eq!(
            MATERIAL_TEXTURE_MIP_COUNT,
            MATERIAL_TEXTURE_EDGE.ilog2() + 1
        );
    }

    #[test]
    fn the_array_texture_has_one_layer_per_material() {
        assert_eq!(
            usize::try_from(MATERIAL_TEXTURE_LAYER_COUNT).expect("layer count fits usize"),
            EMBEDDED_MATERIALS.len()
        );
    }
}
