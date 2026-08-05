// What every surface in the world pass shares: the frame's uniforms, where a
// vertex lands, and the light and air that reach it.
//
// This is never a shader on its own. One fragment entry point is appended to it
// per pipeline — ground, the far tier's cutout, or foliage — so each surface
// kind compiles as its own small shader instead of as one branch of a large
// one. Two things come of that, and both matter more than they look.
//
// A shader that can `discard` cannot be depth-tested before it runs, because
// the test does not know yet whether the fragment survives. While one shader
// drew everything, the needle cutout in the foliage branch spent that early
// test on terrain, water, and bark as well: a forest shaded the hillside behind
// it in full and then threw it away. Split apart, only foliage pays.
//
// And a fragment shader costs the registers of its worst path everywhere, so
// how many of them a machine can keep in flight is set by the heaviest surface
// in the file. Foliage, which covers the most pixels and samples no textures at
// all, was being held to what the triplanar rock path needs.

struct Camera {
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
    render_origin_high: vec4<f32>,
    render_origin_low: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct TerrainCutout {
    min_high: vec2<f32>,
    min_low: vec2<f32>,
    max_high: vec2<f32>,
    max_low: vec2<f32>,
};

@group(0) @binding(1)
var<uniform> terrain_cutout: TerrainCutout;

struct Atmosphere {
    fog_color_density: vec4<f32>,
    wind_moisture: vec4<f32>,
};

@group(0) @binding(2)
var<uniform> atmosphere: Atmosphere;

struct Lighting {
    sun_direction_intensity: vec4<f32>,
    sun_color: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    ground_ambient: vec4<f32>,
    cascade_splits: vec4<f32>,
    shadow_view_projection: array<mat4x4<f32>, 3>,
};

@group(0) @binding(3)
var<uniform> lighting: Lighting;

@group(0) @binding(4)
var shadow_map: texture_depth_2d_array;

@group(0) @binding(5)
var shadow_sampler: sampler_comparison;

@group(0) @binding(6)
var material_diffuse: texture_2d_array<f32>;

@group(0) @binding(7)
var material_normal: texture_2d_array<f32>;

@group(0) @binding(8)
var material_arm: texture_2d_array<f32>;

@group(0) @binding(9)
var material_sampler: sampler;

struct VertexInput {
    @location(0) position_high: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) snow_coverage: f32,
    @location(4) position_low: vec3<f32>,
    @location(5) surface_kind: f32,
    @location(6) material_uv: vec2<f32>,
    @location(7) needle_depth: f32,
    @location(8) needle_seed: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) elevation: f32,
    @location(2) color: vec4<f32>,
    @location(3) world_position: vec3<f32>,
    @location(4) snow_coverage: f32,
    @location(5) render_position: vec3<f32>,
    @location(6) @interpolate(flat) surface_kind: f32,
    @location(7) material_uv: vec2<f32>,
    @location(8) needle_position: vec3<f32>,
    @location(9) @interpolate(flat) needle_depth: f32,
    @location(10) @interpolate(flat) needle_seed: f32,
};

// How far needle tips travel in the wind, in meters, and how fast.
const NEEDLE_SWAY_METERS: f32 = 0.05;
const NEEDLE_SWAY_RATE: f32 = 1.7;
// How far the wind's own phase runs before it repeats, in meters. Far larger
// than anything one crown spans, so no two trees in a stand stir together.
const NEEDLE_WRAP_METERS: f32 = 4096.0;

// Where a vertex stands in the needle field.
//
// World coordinates run to six figures and needles are cut at centimeters, so
// the field is wrapped to keep the arithmetic inside what an `f32` can say. The
// high half of a position carries its magnitude and the low half its detail, so
// wrapping the high half alone costs nothing: the sum stays exact to well under
// a millimeter anywhere in the world.
fn needle_position(position_high: vec3<f32>, position_low: vec3<f32>) -> vec3<f32> {
    let wrapped =
        position_high - floor(position_high / NEEDLE_WRAP_METERS) * NEEDLE_WRAP_METERS;
    return wrapped + position_low;
}

// How far the wind has carried a needle tip.
//
// Only where a vertex is drawn moves. Where its needles are sampled does not,
// so a crown stirs without its needles swimming through it. Depth is zero on
// everything that is not a needle shell, which is what keeps trunks and terrain
// still without asking what surface they are.
//
// The shadow pass does not sway with it. A needle tip travels a few centimeters,
// which is well inside what a cascade can resolve, and matching it there would
// mean handing the wind to a pass that otherwise needs nothing but a camera.
fn needle_sway(field_position: vec3<f32>, depth: f32) -> vec3<f32> {
    let wind = atmosphere.wind_moisture.xy;
    let phase = (atmosphere.wind_moisture.w * NEEDLE_SWAY_RATE)
        + dot(field_position.xz, vec2<f32>(0.31, 0.27))
        + (field_position.y * 0.19);
    return vec3<f32>(wind.x, 0.0, wind.y) * (sin(phase) * NEEDLE_SWAY_METERS * depth);
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let normal = normalize(input.normal);
    let horizontal_normal = length(normal.xz);
    let terrain_slope = horizontal_normal / max(normal.y, 0.000001);
    let slope_retention = 1.0 - smoothstep(0.32, 1.15, terrain_slope);
    let field_position = needle_position(input.position_high, input.position_low);
    let render_position =
        (input.position_high - camera.render_origin_high.xyz)
        + (input.position_low - camera.render_origin_low.xyz)
        + needle_sway(field_position, input.needle_depth);
    let world_position = input.position_high + input.position_low;
    output.clip_position = camera.view_projection * vec4<f32>(render_position, 1.0);
    output.world_normal = normal;
    output.elevation = world_position.y;
    output.color = input.color;
    output.world_position = world_position;
    output.snow_coverage = input.snow_coverage * slope_retention;
    output.render_position = render_position;
    output.surface_kind = input.surface_kind;
    output.material_uv = input.material_uv;
    output.needle_position = field_position;
    output.needle_depth = input.needle_depth;
    output.needle_seed = input.needle_seed;
    return output;
}

// The surface normal a fragment shades with, turned to face the viewer.
//
// Every surface here is closed and back-face culled, so the flip only ever
// catches a mesh whose winding disagrees with its normals.
fn facing_normal(world_normal: vec3<f32>, front_facing: bool) -> vec3<f32> {
    let normal = normalize(world_normal);
    return select(-normal, normal, front_facing);
}

fn cascade_shadow(
    cascade: u32,
    render_position: vec3<f32>,
    normal_dot_sun: f32,
) -> f32 {
    let shadow_clip = lighting.shadow_view_projection[cascade]
        * vec4<f32>(render_position, 1.0);
    let projected = shadow_clip.xyz / shadow_clip.w;
    let uv = projected.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if (
        projected.z <= 0.0
        || projected.z >= 1.0
        || any(uv < vec2<f32>(0.0))
        || any(uv > vec2<f32>(1.0))
    ) {
        return 1.0;
    }
    let texel = 1.0 / 1024.0;
    let bias = 0.00018 + ((1.0 - normal_dot_sun) * 0.00062);
    let reference_depth = projected.z - bias;
    var visibility = 0.0;
    for (var z = -1; z <= 1; z += 1) {
        for (var x = -1; x <= 1; x += 1) {
            let offset = vec2<f32>(f32(x), f32(z)) * texel;
            visibility += textureSampleCompareLevel(
                shadow_map,
                shadow_sampler,
                uv + offset,
                i32(cascade),
                reference_depth,
            );
        }
    }
    return visibility / 9.0;
}

fn sun_visibility(
    render_position: vec3<f32>,
    view_distance: f32,
    normal_dot_sun: f32,
) -> f32 {
    var cascade = 2u;
    var previous_split = lighting.cascade_splits.y;
    if (view_distance < lighting.cascade_splits.x) {
        cascade = 0u;
        previous_split = 0.0;
    } else if (view_distance < lighting.cascade_splits.y) {
        cascade = 1u;
        previous_split = lighting.cascade_splits.x;
    } else if (view_distance >= lighting.cascade_splits.z) {
        return 1.0;
    }
    let current = cascade_shadow(cascade, render_position, normal_dot_sun);
    if (cascade >= 2u) {
        return current;
    }
    let split = lighting.cascade_splits[cascade];
    let blend_start = mix(previous_split, split, 0.88);
    let blend = smoothstep(blend_start, split, view_distance);
    let next = cascade_shadow(cascade + 1u, render_position, normal_dot_sun);
    return mix(current, next, blend);
}

// The sun and the sky on one surface: direct light, skylight from above, and
// what the ground bounces back up.
//
// `diffuse_response` is how much of the sun the surface takes, which is the one
// term a surface kind is free to disagree about — foliage keeps taking light
// past the terminator where a solid has already gone dark.
fn ambient_and_direct(
    albedo: vec3<f32>,
    normal: vec3<f32>,
    ambient_occlusion: f32,
    diffuse_response: f32,
    shadow: f32,
) -> vec3<f32> {
    let sky_exposure = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0);
    let ground_exposure = clamp(-normal.y, 0.0, 1.0);
    let skylight = mix(
        lighting.ground_ambient.rgb * 0.42,
        lighting.sky_zenith.rgb * 0.48,
        sky_exposure,
    );
    let direct_light = lighting.sun_color.rgb
        * diffuse_response
        * shadow
        * lighting.sun_direction_intensity.w;
    let ground_bounce = lighting.ground_ambient.rgb * ground_exposure;
    return albedo * (
        skylight * ambient_occlusion
        + direct_light
        + ground_bounce * ambient_occlusion
    );
}

// The sun's own reflection off a surface, as a Blinn-Phong lobe.
fn sun_highlight(
    normal: vec3<f32>,
    view_direction: vec3<f32>,
    power: f32,
    strength: f32,
    shadow: f32,
) -> vec3<f32> {
    let half_direction = normalize(lighting.sun_direction_intensity.xyz + view_direction);
    return lighting.sun_color.rgb
        * pow(max(dot(normal, half_direction), 0.0), power)
        * strength
        * shadow
        * lighting.sun_direction_intensity.w;
}

// Exponential aerial perspective keeps the full 100 km horizon legible without
// a hard fog wall. Low terrain carries a little more suspended moisture than
// ridges, producing visible valley haze.
fn aerial_perspective(lit: vec3<f32>, view_distance: f32, elevation: f32) -> vec3<f32> {
    let fog_density = max(atmosphere.fog_color_density.w, 0.0);
    let moisture = clamp(atmosphere.wind_moisture.z, 0.0, 1.0);
    let distance_haze = 1.0 - exp(-(view_distance / 22000.0) * fog_density);
    let lowland_haze = exp(-max(elevation, 0.0) / 700.0)
        * smoothstep(900.0, 9000.0, view_distance)
        * mix(0.06, 0.28, moisture);
    let haze = clamp(distance_haze + lowland_haze, 0.0, 0.92);
    let horizon_color = mix(
        lighting.sky_horizon.rgb,
        atmosphere.fog_color_density.rgb,
        0.55,
    );
    return mix(lit, horizon_color, haze);
}
