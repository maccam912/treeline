struct ShadowCamera {
    view_projection: mat4x4<f32>,
    render_origin_high: vec4<f32>,
    render_origin_low: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> shadow_camera: ShadowCamera;

struct VertexInput {
    @location(0) position_high: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) snow_coverage: f32,
    @location(4) position_low: vec3<f32>,
    @location(5) surface_kind: f32,
};

@vertex
fn vs_shadow(input: VertexInput) -> @builtin(position) vec4<f32> {
    let render_position =
        (input.position_high - shadow_camera.render_origin_high.xyz)
        + (input.position_low - shadow_camera.render_origin_low.xyz);
    return shadow_camera.view_projection * vec4<f32>(render_position, 1.0);
}
