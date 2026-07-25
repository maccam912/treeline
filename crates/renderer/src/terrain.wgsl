struct Camera {
    view_projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) elevation: f32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = camera.view_projection * vec4<f32>(input.position, 1.0);
    output.world_normal = normalize(input.normal);
    output.elevation = input.position.y;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let normal = normalize(input.world_normal);
    let sunlight = max(dot(normal, normalize(vec3<f32>(0.45, 0.8, 0.35))), 0.0);
    let slope = 1.0 - max(normal.y, 0.0);
    let grass = vec3<f32>(0.19, 0.38, 0.16);
    let stone = vec3<f32>(0.35, 0.34, 0.31);
    let highland = smoothstep(12.0, 18.0, input.elevation);
    let base = mix(grass, stone, clamp(slope * 1.5 + highland * 0.35, 0.0, 1.0));
    let lit = base * (0.38 + sunlight * 0.75);
    return vec4<f32>(lit, 1.0);
}
