// The shadow-free substitute appended to the scene shader when the backend
// does not support the shadow pipeline. Every surface stays fully lit by the
// sun rather than sampled against a shadow map that this backend cannot create.

fn shadow_visibility(
    render_position: vec3<f32>,
    view_distance: f32,
    normal_dot_sun: f32,
) -> f32 {
    return 1.0;
}
