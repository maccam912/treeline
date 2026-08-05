// The coarse terrain tier, which is ground with a hole cut in it.
//
// Far terrain is drawn under the near tier and has to give way where the two
// overlap, and the shape it gives way to is a rectangle the camera moves
// through — nothing a mesh could be built around. So it is cut per fragment.
//
// That cut is a `discard`, which costs this pipeline its early depth test, and
// it is why the entry point lives here rather than beside `fs_ground`. Far
// terrain can afford it: it is drawn first, into a depth buffer holding nothing
// but sky, so there is no early rejection for it to lose. The near tier, drawn
// after it into a frame it occludes most of, keeps its own.

// Whether a fragment falls inside the half-open rectangle the near tier owns.
fn inside_near_tier(render_position: vec3<f32>) -> bool {
    let cutout_min =
        (terrain_cutout.min_high - camera.render_origin_high.xz)
        + (terrain_cutout.min_low - camera.render_origin_low.xz);
    let cutout_max =
        (terrain_cutout.max_high - camera.render_origin_high.xz)
        + (terrain_cutout.max_low - camera.render_origin_low.xz);
    return render_position.x >= cutout_min.x
        && render_position.x < cutout_max.x
        && render_position.z >= cutout_min.y
        && render_position.z < cutout_max.y;
}

@fragment
fn fs_far_ground(
    input: VertexOutput,
    @builtin(front_facing) front_facing: bool,
) -> @location(0) vec4<f32> {
    if (inside_near_tier(input.render_position)) {
        discard;
    }
    return ground_shading(input, front_facing);
}
