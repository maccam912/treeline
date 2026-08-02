//! Renderer-facing terrain tiers and the first concrete wgpu terrain path.

use std::error::Error;
use std::fmt::{Display, Formatter};

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat4, Quat, Vec3};
use image::ImageFormat;
use image::imageops::{FilterType, resize};
use treeline_ecology::{
    BarkStyle, CrownShape, GroundCoverGroup, GroundPlant, ProceduralTree, RockForm, SurfaceRock,
    TreeCondition, TreeFunctionalGroup,
};
use treeline_mesher::Mesh;
use wgpu::util::DeviceExt;

const TERRAIN_SHADER: &str = include_str!("terrain.wgsl");
const SHADOW_SHADER: &str = include_str!("shadow.wgsl");
const SKY_SHADER: &str = include_str!("sky.wgsl");
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADOW_CASCADE_COUNT: usize = 3;
const SHADOW_MAP_SIZE: u32 = 1_024;
const SHADOW_CASCADE_SPLITS_METERS: [f32; SHADOW_CASCADE_COUNT] = [48.0, 140.0, 360.0];
const SHADOW_CASCADE_RADII_METERS: [f64; SHADOW_CASCADE_COUNT] = [56.0, 164.0, 424.0];
const SHADOW_DEPTH_METERS: f64 = 3_000.0;
const SURFACE_KIND_SOLID: f32 = 0.0;
const SURFACE_KIND_WATER: f32 = 1.0;
const SURFACE_KIND_PINE_BARK: f32 = 2.0;
const SURFACE_KIND_OAK_BARK: f32 = 3.0;
const MATERIAL_TEXTURE_EDGE: u32 = 512;
const MATERIAL_TEXTURE_LAYER_COUNT: u32 = 4;
const MATERIAL_TEXTURE_MIP_COUNT: u32 = 10;

const FOREST_FLOOR_DIFFUSE: &[u8] = include_bytes!("../assets/surfaces/forest_floor_diff_1k.jpg");
const FOREST_FLOOR_NORMAL: &[u8] = include_bytes!("../assets/surfaces/forest_floor_nor_gl_1k.jpg");
const FOREST_FLOOR_ARM: &[u8] = include_bytes!("../assets/surfaces/forest_floor_arm_1k.jpg");
const ROCK_FACE_DIFFUSE: &[u8] = include_bytes!("../assets/surfaces/rock_face_diff_1k.jpg");
const ROCK_FACE_NORMAL: &[u8] = include_bytes!("../assets/surfaces/rock_face_nor_gl_1k.jpg");
const ROCK_FACE_ARM: &[u8] = include_bytes!("../assets/surfaces/rock_face_arm_1k.jpg");
const PINE_BARK_DIFFUSE: &[u8] = include_bytes!("../assets/bark/pine_bark_diff_1k.jpg");
const PINE_BARK_NORMAL: &[u8] = include_bytes!("../assets/bark/pine_bark_nor_gl_1k.jpg");
const PINE_BARK_ARM: &[u8] = include_bytes!("../assets/bark/pine_bark_arm_1k.jpg");
const OAK_BARK_DIFFUSE: &[u8] = include_bytes!("../assets/bark/bark_brown_02_diff_1k.jpg");
const OAK_BARK_NORMAL: &[u8] = include_bytes!("../assets/bark/bark_brown_02_nor_gl_1k.jpg");
const OAK_BARK_ARM: &[u8] = include_bytes!("../assets/bark/bark_brown_02_arm_1k.jpg");

/// Maximum horizontal caster distance needed by the cascaded shadow maps.
pub const SHADOW_CASTER_DISTANCE_METERS: f64 = 480.0;

/// Curated daylight states that exercise the complete sky and sun model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeOfDay {
    Dawn,
    #[default]
    Noon,
    Dusk,
}

impl TimeOfDay {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Dawn => Self::Noon,
            Self::Noon => Self::Dusk,
            Self::Dusk => Self::Dawn,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dawn => "dawn",
            Self::Noon => "noon",
            Self::Dusk => "dusk",
        }
    }
}

/// Coherent sun, sky, and ambient-light inputs shared by every render path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingSettings {
    pub sun_direction: [f32; 3],
    pub sun_intensity: f32,
    pub sun_color: [f32; 3],
    pub sky_zenith: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub ground_ambient: [f32; 3],
}

impl LightingSettings {
    pub const fn for_time_of_day(time: TimeOfDay) -> Self {
        match time {
            TimeOfDay::Dawn => Self {
                sun_direction: [0.941, 0.224, 0.254],
                sun_intensity: 0.58,
                sun_color: [1.00, 0.47, 0.22],
                sky_zenith: [0.09, 0.18, 0.38],
                sky_horizon: [0.79, 0.38, 0.24],
                ground_ambient: [0.12, 0.08, 0.08],
            },
            TimeOfDay::Noon => Self {
                sun_direction: [0.457, 0.812, 0.355],
                sun_intensity: 0.88,
                sun_color: [1.00, 0.88, 0.70],
                sky_zenith: [0.16, 0.38, 0.73],
                sky_horizon: [0.42, 0.63, 0.85],
                ground_ambient: [0.13, 0.10, 0.07],
            },
            TimeOfDay::Dusk => Self {
                sun_direction: [-0.920, 0.207, -0.332],
                sun_intensity: 0.52,
                sun_color: [1.00, 0.39, 0.18],
                sky_zenith: [0.08, 0.12, 0.29],
                sky_horizon: [0.73, 0.30, 0.25],
                ground_ambient: [0.11, 0.07, 0.08],
            },
        }
    }
}

impl Default for LightingSettings {
    fn default() -> Self {
        Self::for_time_of_day(TimeOfDay::default())
    }
}

/// Renderer-facing atmosphere controls sampled from the world's local climate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtmosphereSettings {
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    pub moisture: f32,
    pub prevailing_wind: [f32; 2],
}

impl Default for AtmosphereSettings {
    fn default() -> Self {
        Self {
            fog_color: [0.39, 0.57, 0.72],
            fog_density: 1.0,
            moisture: 0.45,
            prevailing_wind: [0.8, 0.2],
        }
    }
}

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
    surface_kind: f32,
    material_uv: [f32; 2],
}

impl TerrainVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x4,
        3 => Float32,
        4 => Float32x3,
        5 => Float32,
        6 => Float32x2,
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
        surface_kind: SURFACE_KIND_SOLID,
        material_uv: [0.0; 2],
    }
}

fn mesh_vertices(mesh: &Mesh, surface_kind: f32) -> Vec<TerrainVertex> {
    mesh.positions
        .iter()
        .zip(&mesh.normals)
        .enumerate()
        .map(|(index, (&position, &normal))| {
            let mut vertex = terrain_vertex(
                position,
                normal,
                mesh.colors
                    .get(index)
                    .copied()
                    .unwrap_or([1.0, 1.0, 1.0, 0.0]),
                0.0,
            );
            vertex.surface_kind = surface_kind;
            vertex
        })
        .collect()
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

fn material_vertex(
    position: Vec3,
    normal: Vec3,
    color: [f32; 4],
    surface_kind: f32,
    material_uv: [f32; 2],
) -> TerrainVertex {
    let mut vertex = local_vertex(position, normal, color, 0.0);
    vertex.surface_kind = surface_kind;
    vertex.material_uv = material_uv;
    vertex
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
    inverse_view_projection: [[f32; 4]; 4],
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct AtmosphereUniform {
    fog_color_density: [f32; 4],
    wind_moisture: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct LightingUniform {
    sun_direction_intensity: [f32; 4],
    sun_color: [f32; 4],
    sky_zenith: [f32; 4],
    sky_horizon: [f32; 4],
    ground_ambient: [f32; 4],
    cascade_splits: [f32; 4],
    shadow_view_projection: [[[f32; 4]; 4]; SHADOW_CASCADE_COUNT],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ShadowCameraUniform {
    view_projection: [[f32; 4]; 4],
    render_origin_high: [f32; 4],
    render_origin_low: [f32; 4],
}

fn atmosphere_uniform(settings: AtmosphereSettings) -> AtmosphereUniform {
    AtmosphereUniform {
        fog_color_density: [
            settings.fog_color[0],
            settings.fog_color[1],
            settings.fog_color[2],
            settings.fog_density.max(0.0),
        ],
        wind_moisture: [
            settings.prevailing_wind[0],
            settings.prevailing_wind[1],
            settings.moisture.clamp(0.0, 1.0),
            0.0,
        ],
    }
}

fn normalized_sun_direction(settings: LightingSettings) -> Vec3 {
    Vec3::from_array(settings.sun_direction).normalize_or(Vec3::Y)
}

fn lighting_uniform(
    settings: LightingSettings,
    render_origin: [f64; 3],
    view_direction: [f32; 3],
) -> LightingUniform {
    let sun_direction = normalized_sun_direction(settings);
    LightingUniform {
        sun_direction_intensity: [
            sun_direction.x,
            sun_direction.y,
            sun_direction.z,
            settings.sun_intensity.max(0.0),
        ],
        sun_color: [
            settings.sun_color[0].max(0.0),
            settings.sun_color[1].max(0.0),
            settings.sun_color[2].max(0.0),
            0.0,
        ],
        sky_zenith: [
            settings.sky_zenith[0].max(0.0),
            settings.sky_zenith[1].max(0.0),
            settings.sky_zenith[2].max(0.0),
            0.0,
        ],
        sky_horizon: [
            settings.sky_horizon[0].max(0.0),
            settings.sky_horizon[1].max(0.0),
            settings.sky_horizon[2].max(0.0),
            0.0,
        ],
        ground_ambient: [
            settings.ground_ambient[0].max(0.0),
            settings.ground_ambient[1].max(0.0),
            settings.ground_ambient[2].max(0.0),
            0.0,
        ],
        cascade_splits: [
            SHADOW_CASCADE_SPLITS_METERS[0],
            SHADOW_CASCADE_SPLITS_METERS[1],
            SHADOW_CASCADE_SPLITS_METERS[2],
            0.0,
        ],
        shadow_view_projection: shadow_view_projections(
            render_origin,
            view_direction,
            sun_direction,
        ),
    }
}

fn shadow_view_projections(
    render_origin: [f64; 3],
    view_direction: [f32; 3],
    sun_direction: Vec3,
) -> [[[f32; 4]; 4]; SHADOW_CASCADE_COUNT] {
    let origin = DVec3::from_array(render_origin);
    let view_direction = DVec3::new(
        f64::from(view_direction[0]),
        0.0,
        f64::from(view_direction[2]),
    )
    .normalize_or(DVec3::Z);
    let sun_direction = sun_direction.as_dvec3();
    let light_forward = -sun_direction;
    let provisional_up = if light_forward.y.abs() > 0.98 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let light_right = light_forward.cross(provisional_up).normalize();
    let light_up = light_right.cross(light_forward).normalize();

    std::array::from_fn(|cascade| {
        let radius = SHADOW_CASCADE_RADII_METERS[cascade];
        let desired_center = origin + (view_direction * radius * 0.35);
        let texel_size = (radius * 2.0) / f64::from(SHADOW_MAP_SIZE);
        let snapped_right = libm::round(desired_center.dot(light_right) / texel_size) * texel_size;
        let snapped_up = libm::round(desired_center.dot(light_up) / texel_size) * texel_size;
        let snapped_center = (light_right * snapped_right)
            + (light_up * snapped_up)
            + (light_forward * desired_center.dot(light_forward));
        let relative_center = snapped_center - origin;
        let eye = relative_center + (sun_direction * (SHADOW_DEPTH_METERS * 0.5));
        let view = Mat4::look_at_rh(eye.as_vec3(), relative_center.as_vec3(), light_up.as_vec3());
        let radius = f64_as_f32(radius);
        let projection = Mat4::orthographic_rh(
            -radius,
            radius,
            -radius,
            radius,
            0.0,
            f64_as_f32(SHADOW_DEPTH_METERS),
        );
        (projection * view).to_cols_array_2d()
    })
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

struct TerrainBindings {
    layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    far_bind_group: wgpu::BindGroup,
    near_bind_group: wgpu::BindGroup,
    far_cutout_buffer: wgpu::Buffer,
    atmosphere_buffer: wgpu::Buffer,
    lighting_buffer: wgpu::Buffer,
}

#[derive(Debug)]
struct ShadowMap {
    _texture: wgpu::Texture,
    sampling_view: wgpu::TextureView,
    layer_views: [wgpu::TextureView; SHADOW_CASCADE_COUNT],
    sampler: wgpu::Sampler,
}

#[derive(Clone, Copy)]
struct EmbeddedMaterial {
    diffuse: &'static [u8],
    normal: &'static [u8],
    arm: &'static [u8],
}

const EMBEDDED_MATERIALS: [EmbeddedMaterial; 4] = [
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
struct MaterialTextures {
    _diffuse_texture: wgpu::Texture,
    diffuse_view: wgpu::TextureView,
    _normal_texture: wgpu::Texture,
    normal_view: wgpu::TextureView,
    _arm_texture: wgpu::Texture,
    arm_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

impl MaterialTextures {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
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

fn create_material_texture(
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

fn upload_material_layers(
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

impl ShadowMap {
    fn new(device: &wgpu::Device) -> Self {
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

struct ShadowBindings {
    layout: wgpu::BindGroupLayout,
    camera_buffers: [wgpu::Buffer; SHADOW_CASCADE_COUNT],
    bind_groups: [wgpu::BindGroup; SHADOW_CASCADE_COUNT],
}

impl ShadowBindings {
    fn new(device: &wgpu::Device) -> Self {
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

impl TerrainBindings {
    fn new(
        device: &wgpu::Device,
        shadow_map: &ShadowMap,
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bind group layout"),
            entries: &[
                uniform_layout_entry(0, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                uniform_layout_entry(1, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                uniform_layout_entry(2, wgpu::ShaderStages::FRAGMENT),
                uniform_layout_entry(3, wgpu::ShaderStages::FRAGMENT),
                depth_texture_layout_entry(4),
                comparison_sampler_layout_entry(5),
                sampled_texture_array_layout_entry(6),
                sampled_texture_array_layout_entry(7),
                sampled_texture_array_layout_entry(8),
                filtering_sampler_layout_entry(9),
            ],
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

const fn depth_texture_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}

const fn comparison_sampler_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
        count: None,
    }
}

const fn sampled_texture_array_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

const fn filtering_sampler_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

#[derive(Clone, Copy)]
struct TerrainBindGroupResources<'a> {
    camera_buffer: &'a wgpu::Buffer,
    cutout_buffer: &'a wgpu::Buffer,
    atmosphere_buffer: &'a wgpu::Buffer,
    lighting_buffer: &'a wgpu::Buffer,
    shadow_map: &'a ShadowMap,
    material_textures: &'a MaterialTextures,
}

fn terrain_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    resources: TerrainBindGroupResources<'_>,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
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
                resource: wgpu::BindingResource::TextureView(&resources.shadow_map.sampling_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&resources.shadow_map.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(
                    &resources.material_textures.diffuse_view,
                ),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(
                    &resources.material_textures.normal_view,
                ),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&resources.material_textures.arm_view),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::Sampler(&resources.material_textures.sampler),
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

fn create_terrain_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    surface_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain shader"),
        source: wgpu::ShaderSource::Wgsl(TERRAIN_SHADER.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("terrain pipeline"),
        layout: Some(layout),
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

fn create_sky_pipeline(
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

fn create_shadow_pipeline(
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

    fn upload_mesh_with_kind(
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
            material: CylinderMaterial::UNTEXTURED,
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
                material: CylinderMaterial::UNTEXTURED,
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
            material: if detail == TreeMeshDetail::Silhouette {
                CylinderMaterial::UNTEXTURED
            } else {
                bark_cylinder_material(tree, 0)
            },
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
            material: bark_cylinder_material(tree, branch_index + 1),
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
struct CylinderMaterial {
    surface_kind: f32,
    seed: f32,
}

impl CylinderMaterial {
    const UNTEXTURED: Self = Self {
        surface_kind: SURFACE_KIND_SOLID,
        seed: 0.0,
    };
}

fn bark_cylinder_material(tree: ProceduralTree, lane: usize) -> CylinderMaterial {
    let surface_kind = match tree.genotype.functional_group {
        TreeFunctionalGroup::EvergreenNeedleleaf => SURFACE_KIND_PINE_BARK,
        TreeFunctionalGroup::ColdDeciduous
        | TreeFunctionalGroup::TemperateBroadleaf
        | TreeFunctionalGroup::DryWoodland => SURFACE_KIND_OAK_BARK,
    };
    CylinderMaterial {
        surface_kind,
        seed: hash_lane(tree.id.rotate_left(29), lane + 41),
    }
}

#[derive(Clone, Copy, Debug)]
struct CylinderSpec {
    start: Vec3,
    end: Vec3,
    start_radius: f32,
    end_radius: f32,
    sides: usize,
    color: [f32; 4],
    material: CylinderMaterial,
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
    let is_bark = spec.material.surface_kind >= SURFACE_KIND_PINE_BARK;
    let vertices_per_ring = if is_bark { spec.sides + 1 } else { spec.sides };
    let average_radius = (spec.start_radius + spec.end_radius) * 0.5;
    let is_pine_bark = spec.material.surface_kind < SURFACE_KIND_OAK_BARK;
    let repeat_width_meters = if is_pine_bark { 2.0 } else { 1.0 };
    let around_repeats =
        libm::roundf((std::f32::consts::TAU * average_radius / repeat_width_meters).max(1.0))
            .clamp(1.0, 12.0);
    let axial_repeats_per_meter = if is_pine_bark { 0.5 } else { 1.0 };
    let axis_length = (spec.end - spec.start).length();
    let u_offset = spec.material.seed * 7.0;
    let v_offset = (spec.material.seed * 17.0).fract();
    for ring in 0..2 {
        let (center, radius) = if ring == 0 {
            (spec.start, spec.start_radius)
        } else {
            (spec.end, spec.end_radius)
        };
        for side in 0..vertices_per_ring {
            let angle = usize_as_f32(side) / usize_as_f32(spec.sides) * std::f32::consts::TAU;
            let radial = (tangent * libm::cosf(angle)) + (bitangent * libm::sinf(angle));
            let position = center + (radial * radius);
            if is_bark {
                vertices.push(material_vertex(
                    position,
                    radial,
                    spec.color,
                    spec.material.surface_kind,
                    [
                        u_offset + (usize_as_f32(side) / usize_as_f32(spec.sides) * around_repeats),
                        v_offset + (usize_as_f32(ring) * axis_length * axial_repeats_per_meter),
                    ],
                ));
            } else {
                vertices.push(local_vertex(position, radial, spec.color, 0.0));
            }
        }
    }
    for side in 0..spec.sides {
        let next = if is_bark {
            side + 1
        } else {
            (side + 1) % spec.sides
        };
        let side = u32::try_from(side).map_err(|_| RendererError::TooManyIndices)?;
        let next = u32::try_from(next).map_err(|_| RendererError::TooManyIndices)?;
        let ring_stride =
            u32::try_from(vertices_per_ring).map_err(|_| RendererError::TooManyIndices)?;
        indices.extend_from_slice(&[
            base_index + side,
            base_index + next,
            base_index + ring_stride + side,
            base_index + next,
            base_index + ring_stride + next,
            base_index + ring_stride + side,
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
    fn daylight_presets_cycle_and_share_normalized_sun_directions() {
        assert_eq!(TimeOfDay::Dawn.next(), TimeOfDay::Noon);
        assert_eq!(TimeOfDay::Noon.next(), TimeOfDay::Dusk);
        assert_eq!(TimeOfDay::Dusk.next(), TimeOfDay::Dawn);

        for time in [TimeOfDay::Dawn, TimeOfDay::Noon, TimeOfDay::Dusk] {
            let settings = LightingSettings::for_time_of_day(time);
            let uniform = lighting_uniform(settings, [0.0; 3], [0.0, 0.0, -1.0]);
            let direction = Vec3::from_array([
                uniform.sun_direction_intensity[0],
                uniform.sun_direction_intensity[1],
                uniform.sun_direction_intensity[2],
            ]);
            assert!((direction.length() - 1.0).abs() < 1.0e-6);
            assert!(direction.y > 0.0);
            assert!(uniform.sun_direction_intensity[3] > 0.0);
        }
    }

    #[test]
    fn shadow_cascades_are_ordered_finite_and_texel_stabilized() {
        assert!(
            SHADOW_CASCADE_SPLITS_METERS
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        assert!(
            SHADOW_CASCADE_RADII_METERS
                .iter()
                .zip(SHADOW_CASCADE_SPLITS_METERS)
                .all(|(&radius, split)| radius > f64::from(split))
        );

        let sun = normalized_sun_direction(LightingSettings::default());
        let first_origin = [1_000_000.0, 410.0, -1_000_000.0];
        let moved_origin = [1_000_000.01, 410.0, -999_999.99];
        let direction = [0.35, -0.12, -0.93];
        let world_point = DVec3::new(1_000_012.0, 402.0, -1_000_018.0);
        let first = shadow_view_projections(first_origin, direction, sun);
        let moved = shadow_view_projections(moved_origin, direction, sun);

        for cascade in 0..SHADOW_CASCADE_COUNT {
            assert!(
                first[cascade]
                    .iter()
                    .flatten()
                    .all(|value| value.is_finite())
            );
            let projected = |matrix: [[f32; 4]; 4], origin: [f64; 3]| {
                let relative = (world_point - DVec3::from_array(origin)).as_vec3();
                let clip = Mat4::from_cols_array_2d(&matrix) * relative.extend(1.0);
                clip.truncate() / clip.w
            };
            let first_position = projected(first[cascade], first_origin);
            let moved_position = projected(moved[cascade], moved_origin);
            let maximum_texel_step = 2.0 / f64_as_f32(f64::from(SHADOW_MAP_SIZE)) + 1.0e-6;
            assert!((first_position.x - moved_position.x).abs() <= maximum_texel_step);
            assert!((first_position.y - moved_position.y).abs() <= maximum_texel_step);
        }
    }

    #[test]
    fn gpu_uniform_layouts_match_wgsl_alignment() {
        assert_eq!(std::mem::size_of::<CameraUniform>(), 160);
        assert_eq!(std::mem::size_of::<ShadowCameraUniform>(), 96);
        assert_eq!(std::mem::size_of::<LightingUniform>(), 288);
    }

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
    fn water_upload_vertices_are_distinct_from_solid_terrain() {
        let mesh = Mesh {
            positions: vec![[0.0, 4.0, 0.0]],
            normals: vec![[0.0, 1.0, 0.0]],
            colors: vec![[0.04, 0.34, 0.58, 1.0]],
            indices: Vec::new(),
        };
        let terrain = mesh_vertices(&mesh, 0.0);
        let water = mesh_vertices(&mesh, 1.0);

        assert!(terrain[0].surface_kind.abs() < f32::EPSILON);
        assert!((water[0].surface_kind - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            terrain[0].color.map(f32::to_bits),
            water[0].color.map(f32::to_bits)
        );
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
    fn pine_and_oak_bark_cylinders_have_distinct_seam_safe_material_coordinates() {
        let pine = tree_fixture(
            21,
            TreeFunctionalGroup::EvergreenNeedleleaf,
            CrownShape::Conical,
            TreeCondition::Mature,
        );
        let oak = tree_fixture(
            22,
            TreeFunctionalGroup::TemperateBroadleaf,
            CrownShape::Rounded,
            TreeCondition::Mature,
        );
        let pine_material = bark_cylinder_material(pine, 0);
        let oak_material = bark_cylinder_material(oak, 0);

        assert!((pine_material.surface_kind - SURFACE_KIND_PINE_BARK).abs() < f32::EPSILON);
        assert!((oak_material.surface_kind - SURFACE_KIND_OAK_BARK).abs() < f32::EPSILON);

        for material in [pine_material, oak_material] {
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            append_tapered_cylinder(
                &mut vertices,
                &mut indices,
                CylinderSpec {
                    start: Vec3::ZERO,
                    end: Vec3::Y * 4.0,
                    start_radius: 0.42,
                    end_radius: 0.28,
                    sides: 7,
                    color: [0.3, 0.2, 0.1, 1.0],
                    material,
                },
            )
            .expect("bark cylinder");

            assert_eq!(vertices.len(), 16);
            assert_eq!(indices.len(), 42);
            assert!(vertices.iter().all(|vertex| {
                (vertex.surface_kind - material.surface_kind).abs() < f32::EPSILON
                    && vertex.material_uv.into_iter().all(f32::is_finite)
            }));

            let seam_start = vertices[0];
            let seam_end = vertices[7];
            let start_position = Vec3::from_array(seam_start.position_high)
                + Vec3::from_array(seam_start.position_low);
            let end_position =
                Vec3::from_array(seam_end.position_high) + Vec3::from_array(seam_end.position_low);
            let repeat_span = seam_end.material_uv[0] - seam_start.material_uv[0];
            assert!((start_position - end_position).length() < 0.000_01);
            assert!(repeat_span >= 1.0);
            assert!((repeat_span - libm::roundf(repeat_span)).abs() < f32::EPSILON);
            assert!(vertices[8].material_uv[1] > vertices[0].material_uv[1]);
        }
    }

    #[test]
    fn embedded_poly_haven_material_maps_decode_with_complete_mip_coverage() {
        for material in EMBEDDED_MATERIALS {
            for encoded in [material.diffuse, material.normal, material.arm] {
                let image = image::load_from_memory_with_format(encoded, ImageFormat::Jpeg)
                    .expect("embedded material map");
                assert_eq!(image.width(), 1_024);
                assert_eq!(image.height(), 1_024);
            }
        }
        assert_eq!(
            MATERIAL_TEXTURE_MIP_COUNT,
            MATERIAL_TEXTURE_EDGE.ilog2() + 1,
            "the mip chain must reach a one-pixel final level"
        );
        assert_eq!(
            MATERIAL_TEXTURE_LAYER_COUNT as usize,
            EMBEDDED_MATERIALS.len()
        );
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
