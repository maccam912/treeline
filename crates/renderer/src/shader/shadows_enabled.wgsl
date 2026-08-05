// The cascaded sun shadow bindings and sampling, appended to the scene shader
// only when the backend supports shadows.
//
// Each cascade lives in its own 2D depth texture rather than in an array,
// because WebGL2 has no depth array textures. The three textures share one
// comparison sampler.

@group(0) @binding(8)
var shadow_map_0: texture_depth_2d;

@group(0) @binding(9)
var shadow_map_1: texture_depth_2d;

@group(0) @binding(10)
var shadow_map_2: texture_depth_2d;

@group(0) @binding(11)
var shadow_sampler: sampler_comparison;

fn sample_shadow(cascade: u32, uv: vec2<f32>, reference_depth: f32) -> f32 {
    if (cascade == 0u) {
        return textureSampleCompareLevel(shadow_map_0, shadow_sampler, uv, reference_depth);
    } else if (cascade == 1u) {
        return textureSampleCompareLevel(shadow_map_1, shadow_sampler, uv, reference_depth);
    } else {
        return textureSampleCompareLevel(shadow_map_2, shadow_sampler, uv, reference_depth);
    }
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
            visibility += sample_shadow(cascade, uv + offset, reference_depth);
        }
    }
    return visibility / 9.0;
}

fn shadow_visibility(
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
