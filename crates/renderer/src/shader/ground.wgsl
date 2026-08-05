// Everything solid: terrain, water, and bark.
//
// These three share a shader because they share a shape — a closed opaque
// surface that covers the pixels it is drawn over. None of them cuts holes in
// itself, so this entry point never discards, and the depth test can throw a
// fragment away before any of the work below runs. In a forest that is most of
// the frame: the hillside behind a stand is rejected rather than shaded.
//
// The far tier is the exception, and it lives in its own file so that this one
// can stay free of discards. See `far_ground.wgsl`.

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

fn ground_shading(input: VertexOutput, front_facing: bool) -> vec4<f32> {
    let geometric_normal = facing_normal(input.world_normal, front_facing);
    var normal = geometric_normal;
    let view_direction = normalize(-input.render_position);
    let view_distance = length(input.render_position);
    let position_dx = dpdx(input.render_position);
    let position_dy = dpdy(input.render_position);
    let ground_position = input.world_position.xz;

    // Every fragment here is one of the three kinds below, and each one assigns
    // its own color, so this stands in only for a surface kind that has been
    // tagged but not yet described.
    var visualized = input.color.rgb;
    var surface_ambient_occlusion = 1.0;
    var surface_roughness = 1.0;

    // Terrain materials stay fixed in world space. Horizontal surfaces use
    // forest litter, while steeper-than-45-degree faces blend to rock through
    // two side projections so cliffs do not stretch or expose UV seams.
    let is_solid = input.surface_kind < 0.5;
    if (is_solid) {
        let slope = 1.0 - max(geometric_normal.y, 0.0);
        let grass = vec3<f32>(0.17, 0.34, 0.14);
        let stone = vec3<f32>(0.35, 0.34, 0.31);
        let snow = vec3<f32>(0.82, 0.86, 0.88);
        let alpine = smoothstep(550.0, 950.0, input.elevation);
        let stone_amount = clamp(slope * 1.7 + alpine * 0.7, 0.0, 1.0);
        let snow_amount = input.snow_coverage;
        let rock_base = mix(grass, stone, stone_amount);
        let untextured_base = mix(rock_base, snow, clamp(snow_amount, 0.0, 1.0));

        // World-space detail provides a fixed visual reference as the player
        // moves. Fade it when one pixel covers too much ground so distant
        // terrain stays stable.
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
        let macro_base = mix(base, input.color.rgb, input.color.a);

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
        let macro_color = mix(macro_base, snow, snow_amount);
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
        let pine_amount = 1.0 - smoothstep(2.25, 2.75, input.surface_kind);
        let bark_layer = i32(clamp(input.surface_kind, 2.0, 3.0));
        let material_uv_dx = dpdx(input.material_uv);
        let material_uv_dy = dpdy(input.material_uv);
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
        let bark_frame = cotangent_frame(
            geometric_normal,
            position_dx,
            position_dy,
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

    let sunlight = max(dot(normal, lighting.sun_direction_intensity.xyz), 0.0);
    let shadow = sun_visibility(input.render_position, view_distance, sunlight);
    var lit = ambient_and_direct(
        visualized,
        normal,
        surface_ambient_occlusion,
        sunlight,
        shadow,
    );
    if (is_bark) {
        lit += sun_highlight(
            normal,
            view_direction,
            mix(72.0, 9.0, surface_roughness),
            mix(0.065, 0.012, surface_roughness),
            shadow,
        );
    }
    if (is_solid) {
        lit += sun_highlight(
            normal,
            view_direction,
            mix(92.0, 5.0, surface_roughness),
            mix(0.045, 0.003, surface_roughness),
            shadow,
        );
    }
    if (is_water) {
        lit += sun_highlight(normal, view_direction, 180.0, 1.8, shadow);
    }

    return vec4<f32>(aerial_perspective(lit, view_distance, input.elevation), 1.0);
}

@fragment
fn fs_ground(
    input: VertexOutput,
    @builtin(front_facing) front_facing: bool,
) -> @location(0) vec4<f32> {
    return ground_shading(input, front_facing);
}
