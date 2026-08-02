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

fn cotangent_frame(
    geometric_normal: vec3<f32>,
    position_dx: vec3<f32>,
    position_dy: vec3<f32>,
    uv_dx: vec2<f32>,
    uv_dy: vec2<f32>,
) -> mat3x3<f32> {
    let position_dy_perpendicular = cross(position_dy, geometric_normal);
    let position_dx_perpendicular = cross(geometric_normal, position_dx);
    let tangent = position_dy_perpendicular * uv_dx.x
        + position_dx_perpendicular * uv_dy.x;
    let bitangent = position_dy_perpendicular * uv_dx.y
        + position_dx_perpendicular * uv_dy.y;
    let inverse_scale = inverseSqrt(max(max(dot(tangent, tangent), dot(bitangent, bitangent)), 0.00000001));
    return mat3x3<f32>(
        tangent * inverse_scale,
        bitangent * inverse_scale,
        geometric_normal,
    );
}

fn wave_gradient(
    position: vec2<f32>,
    direction: vec2<f32>,
    frequency: f32,
    amplitude: f32,
    phase: f32,
) -> vec2<f32> {
    return direction
        * (cos(dot(position, direction) * frequency + phase) * amplitude * frequency);
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
    sky += lighting.sun_color.rgb * solar_haze * 0.22;
    return sky;
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

    var normal = normalize(input.world_normal);
    let geometric_normal = normal;
    let position_dx = dpdx(input.render_position);
    let position_dy = dpdy(input.render_position);
    let material_uv_dx = dpdx(input.material_uv);
    let material_uv_dy = dpdy(input.material_uv);
    let bark_frame = cotangent_frame(
        normal,
        position_dx,
        position_dy,
        material_uv_dx,
        material_uv_dy,
    );
    let pine_amount = 1.0 - smoothstep(2.25, 2.75, input.surface_kind);
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
    var visualized = mix(base, input.color.rgb, input.color.a);
    var surface_ambient_occlusion = 1.0;
    var surface_roughness = 1.0;

    // Terrain materials stay fixed in world space. Horizontal surfaces use
    // forest litter, while steeper-than-45-degree faces blend to rock through
    // two side projections so cliffs do not stretch or expose UV seams.
    let is_solid = input.surface_kind < 0.5;
    if (is_solid) {
        let forest_uv = input.world_position.xz / 2.1;
        let rock_x_uv = vec2<f32>(input.world_position.z, -input.world_position.y) / 2.4;
        let rock_z_uv = vec2<f32>(input.world_position.x, -input.world_position.y) / 2.4;
        let forest_uv_dx = dpdx(forest_uv);
        let forest_uv_dy = dpdy(forest_uv);
        let rock_x_uv_dx = dpdx(rock_x_uv);
        let rock_x_uv_dy = dpdy(rock_x_uv);
        let rock_z_uv_dx = dpdx(rock_z_uv);
        let rock_z_uv_dy = dpdy(rock_z_uv);

        let forest_diffuse = textureSampleGrad(
            material_diffuse,
            material_sampler,
            forest_uv,
            0,
            forest_uv_dx,
            forest_uv_dy,
        );
        let forest_normal_map = textureSampleGrad(
            material_normal,
            material_sampler,
            forest_uv,
            0,
            forest_uv_dx,
            forest_uv_dy,
        );
        let forest_arm = textureSampleGrad(
            material_arm,
            material_sampler,
            forest_uv,
            0,
            forest_uv_dx,
            forest_uv_dy,
        );
        let rock_x_diffuse = textureSampleGrad(
            material_diffuse,
            material_sampler,
            rock_x_uv,
            1,
            rock_x_uv_dx,
            rock_x_uv_dy,
        );
        let rock_z_diffuse = textureSampleGrad(
            material_diffuse,
            material_sampler,
            rock_z_uv,
            1,
            rock_z_uv_dx,
            rock_z_uv_dy,
        );
        let rock_x_normal_map = textureSampleGrad(
            material_normal,
            material_sampler,
            rock_x_uv,
            1,
            rock_x_uv_dx,
            rock_x_uv_dy,
        );
        let rock_z_normal_map = textureSampleGrad(
            material_normal,
            material_sampler,
            rock_z_uv,
            1,
            rock_z_uv_dx,
            rock_z_uv_dy,
        );
        let rock_x_arm = textureSampleGrad(
            material_arm,
            material_sampler,
            rock_x_uv,
            1,
            rock_x_uv_dx,
            rock_x_uv_dy,
        );
        let rock_z_arm = textureSampleGrad(
            material_arm,
            material_sampler,
            rock_z_uv,
            1,
            rock_z_uv_dx,
            rock_z_uv_dy,
        );

        var rock_axis_weights = pow(abs(geometric_normal.xz), vec2<f32>(8.0));
        rock_axis_weights /= max(rock_axis_weights.x + rock_axis_weights.y, 0.0001);
        let rock_diffuse = mix(rock_z_diffuse, rock_x_diffuse, rock_axis_weights.x);
        let rock_arm = mix(rock_z_arm, rock_x_arm, rock_axis_weights.x);
        let rock_amount = 1.0 - smoothstep(0.62, 0.78, abs(geometric_normal.y));
        let sampled_diffuse = mix(forest_diffuse.rgb, rock_diffuse.rgb, rock_amount);
        let sampled_arm = mix(forest_arm, rock_arm, rock_amount);

        let forest_frame = cotangent_frame(
            geometric_normal,
            position_dx,
            position_dy,
            forest_uv_dx,
            forest_uv_dy,
        );
        let rock_x_frame = cotangent_frame(
            geometric_normal,
            position_dx,
            position_dy,
            rock_x_uv_dx,
            rock_x_uv_dy,
        );
        let rock_z_frame = cotangent_frame(
            geometric_normal,
            position_dx,
            position_dy,
            rock_z_uv_dx,
            rock_z_uv_dy,
        );
        let forest_tangent_normal = forest_normal_map.xyz * 2.0 - 1.0;
        let rock_x_tangent_normal = rock_x_normal_map.xyz * 2.0 - 1.0;
        let rock_z_tangent_normal = rock_z_normal_map.xyz * 2.0 - 1.0;
        let forest_surface_normal = normalize(
            forest_frame * normalize(vec3<f32>(forest_tangent_normal.xy * 0.52, max(forest_tangent_normal.z, 0.1)))
        );
        let rock_x_surface_normal = normalize(
            rock_x_frame * normalize(vec3<f32>(rock_x_tangent_normal.xy * 0.72, max(rock_x_tangent_normal.z, 0.1)))
        );
        let rock_z_surface_normal = normalize(
            rock_z_frame * normalize(vec3<f32>(rock_z_tangent_normal.xy * 0.72, max(rock_z_tangent_normal.z, 0.1)))
        );
        let rock_surface_normal = normalize(
            mix(rock_z_surface_normal, rock_x_surface_normal, rock_axis_weights.x)
        );
        let terrain_surface_normal = normalize(
            mix(forest_surface_normal, rock_surface_normal, rock_amount)
        );
        normal = normalize(mix(terrain_surface_normal, geometric_normal, snow_amount));

        // Preserve measured imagery and geography color at broad scales while
        // allowing the scans to contribute real albedo variation up close.
        let macro_color = mix(visualized, snow, snow_amount);
        let forest_reference = vec3<f32>(0.22, 0.16, 0.085);
        let rock_reference = vec3<f32>(0.36, 0.27, 0.22);
        let reference_color = mix(forest_reference, rock_reference, rock_amount);
        let material_detail = clamp(
            sampled_diffuse / reference_color,
            vec3<f32>(0.48),
            vec3<f32>(1.58),
        );
        let texture_visibility = fine_visibility * (1.0 - snow_amount);
        visualized = macro_color * mix(vec3<f32>(1.0), material_detail, texture_visibility);
        surface_ambient_occlusion = mix(1.0, sampled_arm.r, texture_visibility * 0.72);
        surface_roughness = sampled_arm.g;
    }

    let is_bark = input.surface_kind > 1.5;
    if (is_bark) {
        let bark_layer = i32(clamp(input.surface_kind, 2.0, 3.0));
        let diffuse_sample = textureSampleGrad(
            material_diffuse,
            material_sampler,
            input.material_uv,
            bark_layer,
            material_uv_dx,
            material_uv_dy,
        );
        let normal_sample = textureSampleGrad(
            material_normal,
            material_sampler,
            input.material_uv,
            bark_layer,
            material_uv_dx,
            material_uv_dy,
        );
        let arm_sample = textureSampleGrad(
            material_arm,
            material_sampler,
            input.material_uv,
            bark_layer,
            material_uv_dx,
            material_uv_dy,
        );
        let tangent_normal = normal_sample.xyz * 2.0 - 1.0;
        let normal_strength = mix(0.88, 0.72, pine_amount);
        normal = normalize(
            bark_frame
            * normalize(vec3<f32>(
                tangent_normal.xy * normal_strength,
                max(tangent_normal.z, 0.08),
            ))
        );
        surface_ambient_occlusion = arm_sample.r;
        surface_roughness = arm_sample.g;
        let pine_reference = vec3<f32>(0.25, 0.18, 0.11);
        let oak_reference = vec3<f32>(0.27, 0.20, 0.14);
        let reference_color = mix(oak_reference, pine_reference, pine_amount);
        let individual_tint = clamp(
            input.color.rgb / reference_color,
            vec3<f32>(0.72),
            vec3<f32>(1.65),
        );
        visualized = diffuse_sample.rgb
            * individual_tint
            * mix(0.62, 1.0, surface_ambient_occlusion);
    }

    // Dedicated hydrology sheets retain their generated ocean/lake/wetland
    // color but read as reflective water instead of flat colored terrain.
    let is_water = input.surface_kind > 0.5 && input.surface_kind < 1.5;
    let view_direction = normalize(-input.render_position);
    if (is_water) {
        let wind = normalize(atmosphere.wind_moisture.xy + vec2<f32>(0.0001, 0.0001));
        let cross_wind = vec2<f32>(-wind.y, wind.x);
        let time = atmosphere.wind_moisture.w;
        let diagonal_wind = normalize((wind * 0.72) + (cross_wind * 0.69));
        let moisture = clamp(atmosphere.wind_moisture.z, 0.0, 1.0);
        let wave_scale = mix(0.72, 1.18, moisture);
        var gradient = wave_gradient(ground_position, wind, 0.31, 0.23 * wave_scale, time * 0.82);
        gradient += wave_gradient(
            ground_position,
            diagonal_wind,
            0.73,
            0.075 * wave_scale,
            time * 1.27 + 1.8,
        );
        gradient += wave_gradient(
            ground_position,
            cross_wind,
            2.35,
            0.012 * wave_scale,
            time * 2.15 + 4.1,
        );
        normal = normalize(vec3<f32>(-gradient.x, 1.0, -gradient.y));
        let facing = clamp(dot(normal, view_direction), 0.0, 1.0);
        let fresnel = 0.025 + (0.72 * pow(1.0 - facing, 5.0));
        let reflection_direction = reflect(-view_direction, normal);
        let reflected_sky = sky_radiance(reflection_direction);
        let water_body_color = input.color.rgb * mix(0.58, 0.76, facing);
        visualized = mix(water_body_color, reflected_sky, fresnel);
    }

    // The sky, direct light, reflections, and shadow maps share one sun model.
    let sun_direction = lighting.sun_direction_intensity.xyz;
    let sunlight = max(dot(normal, sun_direction), 0.0);
    let view_distance = length(input.render_position);
    let shadow = sun_visibility(input.render_position, view_distance, sunlight);
    let sky_exposure = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0);
    let ground_exposure = clamp(-normal.y, 0.0, 1.0);
    let skylight = mix(
        lighting.ground_ambient.rgb * 0.42,
        lighting.sky_zenith.rgb * 0.48,
        sky_exposure,
    );
    let direct_light = lighting.sun_color.rgb
        * sunlight
        * shadow
        * lighting.sun_direction_intensity.w;
    let ground_bounce = lighting.ground_ambient.rgb * ground_exposure;
    var lit = visualized * (
        skylight * surface_ambient_occlusion
        + direct_light
        + ground_bounce * surface_ambient_occlusion
    );
    if (is_bark) {
        let half_direction = normalize(sun_direction + view_direction);
        let highlight_power = mix(72.0, 9.0, surface_roughness);
        let highlight_strength = mix(0.065, 0.012, surface_roughness);
        let bark_highlight = pow(max(dot(normal, half_direction), 0.0), highlight_power);
        lit += lighting.sun_color.rgb
            * bark_highlight
            * highlight_strength
            * shadow
            * lighting.sun_direction_intensity.w;
    }
    if (is_solid) {
        let half_direction = normalize(sun_direction + view_direction);
        let highlight_power = mix(92.0, 5.0, surface_roughness);
        let highlight_strength = mix(0.045, 0.003, surface_roughness);
        let material_highlight = pow(
            max(dot(normal, half_direction), 0.0),
            highlight_power,
        );
        lit += lighting.sun_color.rgb
            * material_highlight
            * highlight_strength
            * shadow
            * lighting.sun_direction_intensity.w;
    }
    if (is_water) {
        let half_direction = normalize(sun_direction + view_direction);
        let sun_glint = pow(max(dot(normal, half_direction), 0.0), 180.0);
        lit += lighting.sun_color.rgb
            * sun_glint
            * shadow
            * lighting.sun_direction_intensity.w
            * 1.8;
    }

    // Exponential aerial perspective keeps the full 100 km horizon legible
    // without a hard fog wall. Low terrain carries a little more suspended
    // moisture than ridges, producing visible valley haze.
    let fog_density = max(atmosphere.fog_color_density.w, 0.0);
    let moisture = clamp(atmosphere.wind_moisture.z, 0.0, 1.0);
    let distance_haze = 1.0 - exp(-(view_distance / 22000.0) * fog_density);
    let lowland_haze = exp(-max(input.elevation, 0.0) / 700.0)
        * smoothstep(900.0, 9000.0, view_distance)
        * mix(0.06, 0.28, moisture);
    let haze = clamp(distance_haze + lowland_haze, 0.0, 0.92);
    let horizon_color = mix(
        lighting.sky_horizon.rgb,
        atmosphere.fog_color_density.rgb,
        0.55,
    );
    lit = mix(lit, horizon_color, haze);
    return vec4<f32>(lit, 1.0);
}
