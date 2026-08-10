// What every surface in the world pass shares: the frame's uniforms, where a
// vertex lands, and the light and air that reach it.
//
// This is never a shader on its own. One fragment entry point is appended to it
// per pipeline — the near tier's ground or the far tier's cutout — so each
// surface kind compiles as its own small shader instead of as one branch of a
// large one. Two things come of that, and both matter more than they look.
//
// A shader that can `discard` cannot be depth-tested before it runs, because
// the test does not know yet whether the fragment survives. The far tier cuts
// the near tier's footprint out of itself; while one shader drew everything,
// that discard spent the early test on terrain, water, and bark as well. Split
// apart, only the surface that cuts holes in itself pays for doing so.
//
// And a fragment shader costs the registers of its worst path everywhere, so
// how many of them a machine can keep in flight is set by the heaviest surface
// in the file — which, when everything shared one, was the triplanar rock path.

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
var material_diffuse: texture_2d_array<f32>;

@group(0) @binding(5)
var material_normal: texture_2d_array<f32>;

@group(0) @binding(6)
var material_arm: texture_2d_array<f32>;

@group(0) @binding(7)
var material_sampler: sampler;

struct VertexInput {
    @location(0) position_high: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) snow_coverage: f32,
    @location(4) position_low: vec3<f32>,
    @location(5) surface_kind: f32,
    @location(6) material_uv: vec2<f32>,
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
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let normal = normalize(input.normal);
    let horizontal_normal = length(normal.xz);
    let terrain_slope = horizontal_normal / max(normal.y, 0.000001);
    let slope_retention = 1.0 - smoothstep(0.32, 1.15, terrain_slope);
    let render_position =
        (input.position_high - camera.render_origin_high.xyz)
        + (input.position_low - camera.render_origin_low.xyz);
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

fn sun_visibility(
    render_position: vec3<f32>,
    view_distance: f32,
    normal_dot_sun: f32,
) -> f32 {
    return shadow_visibility(render_position, view_distance, normal_dot_sun);
}

// The sun and the sky on one surface: direct light, skylight from above, and
// what the ground bounces back up.
//
// `diffuse_response` is how much of the sun the surface takes, which is the one
// term a surface kind is free to disagree about: a thin or massed surface may
// keep taking light past the terminator where a solid has already gone dark.
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
