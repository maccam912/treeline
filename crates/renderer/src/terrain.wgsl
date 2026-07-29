struct Camera {
    view_projection: mat4x4<f32>,
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

struct VertexInput {
    @location(0) position_high: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) snow_coverage: f32,
    @location(4) position_low: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) elevation: f32,
    @location(2) color: vec4<f32>,
    @location(3) world_position: vec3<f32>,
    @location(4) snow_coverage: f32,
    @location(5) render_position: vec3<f32>,
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
    return output;
}

fn hash_2d(position: vec2<f32>) -> f32 {
    let projected = dot(position, vec2<f32>(127.1, 311.7));
    return fract(sin(projected) * 43758.5453);
}

fn value_noise(position: vec2<f32>) -> f32 {
    let cell = floor(position);
    let offset = fract(position);
    let blend = offset * offset * (3.0 - 2.0 * offset);
    let bottom = mix(hash_2d(cell), hash_2d(cell + vec2<f32>(1.0, 0.0)), blend.x);
    let top = mix(
        hash_2d(cell + vec2<f32>(0.0, 1.0)),
        hash_2d(cell + vec2<f32>(1.0, 1.0)),
        blend.x,
    );
    return mix(bottom, top, blend.y);
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let cutout_min =
        (terrain_cutout.min_high - camera.render_origin_high.xz)
        + (terrain_cutout.min_low - camera.render_origin_low.xz);
    let cutout_max =
        (terrain_cutout.max_high - camera.render_origin_high.xz)
        + (terrain_cutout.max_low - camera.render_origin_low.xz);
    if (
        input.render_position.x >= cutout_min.x
        && input.render_position.x < cutout_max.x
        && input.render_position.z >= cutout_min.y
        && input.render_position.z < cutout_max.y
    ) {
        discard;
    }

    let normal = normalize(input.world_normal);
    let slope = 1.0 - max(normal.y, 0.0);
    let grass = vec3<f32>(0.17, 0.34, 0.14);
    let stone = vec3<f32>(0.35, 0.34, 0.31);
    let snow = vec3<f32>(0.82, 0.86, 0.88);
    let alpine = smoothstep(550.0, 950.0, input.elevation);
    let stone_amount = clamp(slope * 1.7 + alpine * 0.7, 0.0, 1.0);
    let snow_amount = input.snow_coverage;
    let rock_base = mix(grass, stone, stone_amount);
    let untextured_base = mix(rock_base, snow, clamp(snow_amount, 0.0, 1.0));

    // World-space detail provides a fixed visual reference as the player moves.
    // Fade it when one pixel covers too much ground so distant terrain stays stable.
    let ground_position = input.world_position.xz;
    let footprint = max(length(dpdx(ground_position)), length(dpdy(ground_position)));
    let broad_detail = (value_noise(ground_position * 0.22) - 0.5) * 0.18;
    let fine_visibility = 1.0 - smoothstep(0.35, 1.8, footprint);
    let fine_detail = (value_noise(ground_position * 1.65) - 0.5) * 0.16 * fine_visibility;
    let soil_patch = smoothstep(0.64, 0.82, value_noise(ground_position * 0.43));
    let grass_amount = (1.0 - stone_amount) * (1.0 - snow_amount);
    let soil = vec3<f32>(0.23, 0.17, 0.10);
    let ground_base = mix(
        untextured_base,
        soil,
        soil_patch * grass_amount * fine_visibility * 0.42,
    );
    let base = ground_base * (1.0 + broad_detail + fine_detail);
    let visualized = mix(base, input.color.rgb, input.color.a);

    // A warm directional sun establishes form while cool skylight and a small
    // ground bounce keep shaded faces legible.
    let sun_direction = normalize(vec3<f32>(0.45, 0.80, 0.35));
    let sunlight = max(dot(normal, sun_direction), 0.0);
    let sky_exposure = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0);
    let ground_exposure = clamp(-normal.y, 0.0, 1.0);
    let skylight = mix(
        vec3<f32>(0.12, 0.15, 0.20),
        vec3<f32>(0.30, 0.37, 0.46),
        sky_exposure,
    );
    let direct_light = vec3<f32>(1.00, 0.88, 0.70) * sunlight * 0.86;
    let ground_bounce = vec3<f32>(0.13, 0.10, 0.07) * ground_exposure;
    let lit = visualized * (skylight + direct_light + ground_bounce);
    return vec4<f32>(lit, 1.0);
}
