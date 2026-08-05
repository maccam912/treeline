// Conifer foliage: one closed cone per crown, ray-marched.
//
// The geometry is a cone in the shape of the crown's envelope, carrying its
// own definition (base, apex, radius, seed) flat on the vertices. A fragment
// here is on the cone's front surface, facing the camera. It marches the view
// ray back through the interior, sampling a needle field at a few steps, and
// accumulates how much crown the ray crossed. Where it crossed almost nothing —
// a rim where the ray grazes the surface, or a gap the needle field left thin —
// the fragment is discarded and the sky or hillside shows through. Where it
// crossed a real thickness it writes a crown that self-shadows by how deep it
// went and glows from the back where light passed through.
//
// This is the only surface in the world pass that cuts holes in itself, and so
// the only one that gives up an early depth test. It earns its own pipeline
// twice over: the ground no longer pays for a discard it never makes, and the
// crown — which covers more pixels than anything else in a forest — is no longer
// shaded once per shell. One draw call, one pass per pixel.

// How many steps the ray takes through a crown. Each step is one cheap density
// sample, so this is the whole per-pixel cost of a crown's interior; far fewer
// than the shells it replaced, and the steps that cross nothing cost almost
// nothing at all.
const MARCH_STEPS: i32 = 10;
// A fragment this close to empty coverage is a gap or a grazing rim, and is
// cut away so the crown keeps a ragged edge rather than a faceted cone.
const COVERAGE_MIN: f32 = 0.045;
// How much needle there is per meter inside a crown, before profile and noise.
const NEEDLE_DENSITY: f32 = 2.6;
// How steeply the interior darkens with the depth the ray crossed, for the
// self-shadow that reads as seeing past the needles in front to those behind.
const SELF_SHADOW: f32 = 0.7;
// How much of the light that reaches a crown passes out the far side.
const NEEDLE_TRANSLUCENCY: f32 = 0.9;
// How far past the terminator massed needles keep catching light.
const FOLIAGE_LIGHT_WRAP: f32 = 0.42;
// The scale of the needle field, in needles per meter.
const NEEDLE_FREQUENCY: f32 = 5.0;
// How far the field runs before it wraps, to keep the noise arithmetic inside
// what an `f32` can say for crowns far from the camera.
const FIELD_WRAP_METERS: f32 = 64.0;

// A cheap deterministic hash in `[0, 1)`, keyed by a position and a crown seed.
fn needle_hash(p: vec3<f32>, seed: f32) -> f32 {
    var q = vec3<f32>(
        dot(p, vec3<f32>(127.1, 311.7, 74.7)) + seed,
        dot(p, vec3<f32>(269.5, 183.3, 246.1)) + seed,
        dot(p, vec3<f32>(113.5, 271.9, 124.6)) + seed,
    );
    q = fract(q * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

// How much crown a cubic meter at `p` holds: the cone profile, thinning at the
// base and the tip, fading at the surface, and broken into needle clumps by a
// noise that makes the silhouette ragged rather than lathed.
fn cone_density(
    p: vec3<f32>,
    base: vec3<f32>,
    uhat: vec3<f32>,
    height: f32,
    radius: f32,
    seed: f32,
) -> f32 {
    let q = p - base;
    let axial = dot(q, uhat);
    if (axial < 0.0 || axial > height) {
        return 0.0;
    }
    let axial_frac = axial / height;
    let body = smoothstep(0.0, 0.3, axial_frac) * (1.0 - smoothstep(0.72, 1.0, axial_frac));
    let r_at = radius * (1.0 - axial_frac);
    let radial = q - uhat * axial;
    let perp = length(radial);
    if (perp > r_at) {
        return 0.0;
    }
    // The inner half of the crown is solid; the outer half fades to nothing at
    // the surface, which is what lets the noise break the rim into needles.
    let edge = 1.0 - smoothstep(0.55, 1.0, perp / max(r_at, 1.0e-4));
    let wrapped = q - floor(q / FIELD_WRAP_METERS) * FIELD_WRAP_METERS;
    let noise = 0.35 + needle_hash(wrapped * NEEDLE_FREQUENCY, seed) * 1.15;
    return NEEDLE_DENSITY * body * edge * noise;
}

@fragment
fn fs_foliage(input: VertexOutput) -> @location(0) vec4<f32> {
    let base = input.crown_a.xyz;
    let radius = input.crown_a.w;
    let apex = input.crown_b.xyz;
    let seed = input.crown_b.w;

    let origin = input.render_position;
    let direction = normalize(origin);
    let axis = apex - base;
    let height = length(axis);
    let uhat = axis / height;

    // Step through the crown along the view ray, accumulating how much of it the
    // ray crossed and how deep it went.
    let span = height + (radius * 2.0);
    let step_len = span / f32(MARCH_STEPS);
    var coverage = 0.0;
    var optical = 0.0;
    var t = 0.0;
    for (var step = 0; step < MARCH_STEPS; step += 1) {
        let density = cone_density(origin + direction * t, base, uhat, height, radius, seed);
        let slice = density * step_len;
        coverage += slice;
        optical += slice;
        t += step_len;
    }
    if (coverage < COVERAGE_MIN) {
        discard;
    }

    // A crown shades itself by how deep the ray went: the deeper, the darker,
    // which is the whole depth that the shells used to draw as stacked layers.
    let self_shadow = exp(-optical * SELF_SHADOW);

    // Needles are thin enough to light from behind, so foliage keeps taking
    // light past the terminator instead of falling to a hard shaded edge.
    let sun_direction = lighting.sun_direction_intensity.xyz;
    let view_direction = -direction;
    let normal = normalize(mix(view_direction, sun_direction, 0.45));
    let normal_dot_sun = dot(normal, sun_direction);
    let diffuse_response = max(
        (normal_dot_sun + FOLIAGE_LIGHT_WRAP) / (1.0 + FOLIAGE_LIGHT_WRAP),
        0.0,
    );
    let view_distance = length(origin);
    let shadow = sun_visibility(origin, view_distance, max(normal_dot_sun, 0.0));

    // Ambient occlusion: the interior, which the ray travelled furthest through,
    // is the part the crown keeps from the sky.
    let axial_frac = clamp(dot(origin - base, uhat) / height, 0.0, 1.0);
    let ambient_occlusion = exp(-optical * 0.5);

    // This year's growth, which is at the top of a crown, is lighter again.
    let new_growth = mix(0.94, 1.12, axial_frac);
    let albedo = input.color.rgb * new_growth;
    var lit = ambient_and_direct(
        albedo,
        normal,
        ambient_occlusion,
        diffuse_response,
        shadow,
    ) * self_shadow;

    // A crown with the sun behind it glows rather than silhouetting: the light
    // that crossed the needles comes out the far side.
    let transmission = pow(max(dot(-view_direction, sun_direction), 0.0), 3.0);
    let through = exp(-coverage * NEEDLE_TRANSLUCENCY);
    lit += albedo
        * lighting.sun_color.rgb
        * transmission
        * through
        * shadow
        * lighting.sun_direction_intensity.w;

    return vec4<f32>(aerial_perspective(lit, view_distance, input.elevation), 1.0);
}
