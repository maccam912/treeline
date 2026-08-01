struct Camera {
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
    render_origin_high: vec4<f32>,
    render_origin_low: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

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

struct SkyOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) clip_position: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vertex_index: u32) -> SkyOutput {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var output: SkyOutput;
    output.clip_position = positions[vertex_index];
    output.position = vec4<f32>(output.clip_position, 0.0, 1.0);
    return output;
}

fn sky_radiance(direction: vec3<f32>) -> vec3<f32> {
    let elevation = clamp(direction.y, 0.0, 1.0);
    let horizon_blend = pow(elevation, 0.38);
    let climate_horizon = mix(
        lighting.sky_horizon.rgb,
        atmosphere.fog_color_density.rgb,
        0.38,
    );
    var sky = mix(climate_horizon, lighting.sky_zenith.rgb, horizon_blend);
    let sun_alignment = max(dot(direction, lighting.sun_direction_intensity.xyz), 0.0);
    let solar_haze = pow(sun_alignment, 24.0) * (1.0 - smoothstep(0.0, 0.55, elevation));
    let sun_disc = smoothstep(0.99972, 0.99991, sun_alignment);
    sky += lighting.sun_color.rgb * solar_haze * 0.22;
    sky += lighting.sun_color.rgb * sun_disc * lighting.sun_direction_intensity.w * 3.5;
    return sky;
}

@fragment
fn fs_sky(input: SkyOutput) -> @location(0) vec4<f32> {
    let world = camera.inverse_view_projection
        * vec4<f32>(input.clip_position, 0.0001, 1.0);
    let direction = normalize(world.xyz / max(abs(world.w), 0.000001));
    return vec4<f32>(sky_radiance(direction), 1.0);
}
