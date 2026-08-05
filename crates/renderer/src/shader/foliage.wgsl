// Conifer foliage: nested shells of needles standing off a shoot, with the
// needles themselves grown here rather than sampled from a map.
//
// What the geometry supplies is where a shoot sits, which way it faces, and how
// far out through its needles a fragment stands. Everything finer than that is
// grown below.
//
// This is the only surface in the world pass that cuts holes in itself, and so
// the only one that gives up an early depth test. It earns keeping its own
// pipeline twice over: the ground no longer pays for a discard it never makes,
// and a crown — which covers more pixels than anything else in a forest and
// samples no textures at all — is no longer compiled against the register
// budget of the triplanar rock path it used to share a shader with.
const FOLIAGE_SURFACE_KIND: f32 = 4.0;
// How many needle strands stand across one unit of a shoot's chart, which is
// most of the way around it. Needles then scale with the shoot they grow on,
// which is what a big shoot and a small one actually differ by.
const NEEDLE_CELLS_PER_SHOOT: f32 = 9.0;
// How thick a strand is against the shoot and again at its tips, in strand
// spacings. The first has to cover its shell nearly whole, or the core shows
// through; the second has to stay well clear of zero, or the outermost shell
// draws nothing and the crown ends on the drawn edge of the one below it.
const NEEDLE_ROOT: f32 = 0.54;
const NEEDLE_TIP: f32 = 0.26;
// How sharply a strand narrows between the two. Below one it gives its width up
// early and then holds it, which is the shape of a needle rather than a spike.
const NEEDLE_TAPER: f32 = 0.75;
// How much of a strand's own lean shows in the way it catches light.
const NEEDLE_TILT: f32 = 0.55;
// How dark the inside of a shoot is against needles standing in the open.
const NEEDLE_SHADE: f32 = 0.55;
// How steeply that shade lifts on the way out through the shells.
const NEEDLE_SHADE_FALLOFF: f32 = 1.0;
// A strand this thick covers its cell well enough to stand in for a solid one
// once the cells fall under a pixel. Thinner shells are dropped whole instead.
const NEEDLE_SOLID_AT: f32 = 0.40;
// How far past the terminator massed needles keep catching light. Thin foliage
// scatters through itself, so a crown never has the hard edge a solid does.
const FOLIAGE_LIGHT_WRAP: f32 = 0.42;
// How much light a backlit needle passes through itself.
const NEEDLE_TRANSLUCENCY: f32 = 0.85;
// How far a shoot's needles slide through the field for a seed of one. Enough
// that two shoots side by side land in unrelated country.
const NEEDLE_SEED_SLIDE: f32 = 64.0;
// Where in its cell a strand may stand: the middle half of it, from a quarter
// in to three quarters across.
//
// This is the hot constant in the whole crown. A strand kept off its cell walls
// cannot be the nearest one to a point two cells away, so the search below only
// has to look at four cells rather than nine — and the search runs once per
// fragment, on the surface that covers more of a forest than anything else.
//
// What it costs is a little of the scatter. Needles come out more evenly spaced
// than they were, which is closer to how a shoot actually carries them than the
// clumping a free jitter gives, but it is a visible change and this is the
// number to walk back if a crown starts reading as woven.
const NEEDLE_JITTER_ORIGIN: f32 = 0.25;
const NEEDLE_JITTER_SPAN: f32 = 0.5;

// Where on a shoot a fragment stands, as a flat chart of the direction it faces.
//
// A ball has no coordinate that survives being pushed outward except the
// direction itself: every shell along one ray faces the same way, so anything
// built out of that direction alone holds still through the whole stack. That is
// what makes a strand a needle standing out of the shoot rather than unrelated
// speckle on each shell in turn.
//
// The octahedral projection is the cheap way to flatten a direction without a
// pole or a seam to comb needles around, and its distortion — about two to one
// at worst — is nothing a needle shows.
fn needle_chart(direction: vec3<f32>) -> vec2<f32> {
    let folded = direction / (abs(direction.x) + abs(direction.y) + abs(direction.z));
    if (folded.y >= 0.0) {
        return folded.xz;
    }
    return (1.0 - abs(folded.zx))
        * vec2<f32>(
            select(-1.0, 1.0, folded.x >= 0.0),
            select(-1.0, 1.0, folded.z >= 0.0),
        );
}

// Two independent values for one cell of the needle field.
fn needle_hash(cell: vec2<f32>) -> vec2<f32> {
    var mixed = fract(cell.xyx * vec3<f32>(0.1031, 0.1030, 0.0973));
    mixed += dot(mixed, mixed.yzx + 33.33);
    return fract((mixed.xx + mixed.yz) * mixed.zy);
}

// The nearest needle strand: how far off its axis a point lies, in strand
// spacings, and two numbers belonging to that strand alone.
//
// Cellular noise, one jittered strand per cell, over the four cells nearest the
// point. That is fewer than a cellular noise usually needs, and it is exact
// here rather than approximate. A strand held to the middle half of its cell
// stands at least three quarters of a spacing from anything outside these four,
// and the widest a strand is ever drawn is [`NEEDLE_ROOT`] — so a strand this
// search cannot see is a strand that could not have covered the point anyway.
//
// Which is only true while `NEEDLE_ROOT` stays under 0.75. Widen a needle past
// that and the four cells stop being enough: crowns pick up seams where a
// strand that should have been found was not.
fn needle_strand(field: vec2<f32>) -> vec3<f32> {
    let corner = floor(field - 0.5);
    var nearest = vec3<f32>(2.0, 0.0, 0.0);
    for (var y = 0; y <= 1; y += 1) {
        for (var x = 0; x <= 1; x += 1) {
            let cell = corner + vec2<f32>(f32(x), f32(y));
            let jitter = needle_hash(cell);
            let strand = cell + NEEDLE_JITTER_ORIGIN + (jitter * NEEDLE_JITTER_SPAN);
            let reach = length(strand - field);
            if (reach < nearest.x) {
                nearest = vec3<f32>(reach, jitter);
            }
        }
    }
    return nearest;
}

@fragment
fn fs_foliage(
    input: VertexOutput,
    @builtin(front_facing) front_facing: bool,
) -> @location(0) vec4<f32> {
    let geometric_normal = facing_normal(input.world_normal, front_facing);
    let depth = input.needle_depth;
    let chart = needle_chart(geometric_normal);
    let field = (chart * NEEDLE_CELLS_PER_SHOOT)
        + (needle_hash(vec2<f32>(input.needle_seed, 0.5)) * NEEDLE_SEED_SLIDE);
    let strand = needle_strand(field);
    // A needle is thickest against the shoot and gives its width up on the way
    // out, so each shell keeps less of itself than the one under it. This is the
    // whole silhouette: no alpha, no sorting, just the shells running out of
    // needle. The core keeps everything, so however far into a crown you look
    // there is always wood behind the gaps.
    let thickness = mix(NEEDLE_ROOT, NEEDLE_TIP, pow(depth, NEEDLE_TAPER));
    // Once a strand falls under a pixel the cut would alias into crawling
    // speckle, so past that the shell gives up on strands: a dense one becomes
    // solid, a sparse one is dropped whole. That is the trade a mip makes, and
    // it leaves a distant crown reading as the mass it is.
    //
    // The bias is nothing at all until the strands are nearly that small. Fading
    // it in any sooner inflates every crown in the middle distance back into the
    // ring of solid facets the shells exist to break up.
    let strand_footprint = max(length(dpdx(field)), length(dpdy(field)));
    let unresolved = smoothstep(0.8, 1.5, strand_footprint)
        * select(-2.0, 2.0, thickness >= NEEDLE_SOLID_AT);
    if (depth > 0.0 && strand.x > thickness + unresolved) {
        discard;
    }

    // Every strand leans its own way, which is what keeps a shell of them from
    // shading as the one smooth ball underneath. The lean is taken square to the
    // shell, so it tilts a needle without burying it.
    let wander =
        vec3<f32>(strand.y, strand.z, fract(strand.y + strand.z)) - vec3<f32>(0.5);
    let lean = wander - (geometric_normal * dot(wander, geometric_normal));
    let normal = normalize(geometric_normal + (lean * NEEDLE_TILT * depth));

    // A crown shades itself twice over: almost no sky reaches the wood at the
    // middle of a shoot, and a shoot set back along a branch sits in the dark of
    // the whole mass above it.
    let exposure = input.material_uv.x;
    let ambient_occlusion =
        mix(NEEDLE_SHADE, 1.0, pow(depth, NEEDLE_SHADE_FALLOFF))
        * mix(0.55, 1.0, exposure);

    // Needles run dark at the woody base and light at the tips, and this year's
    // growth, which is at the top of a crown, is lighter again.
    let tone = input.color.rgb;
    let new_growth = mix(0.94, 1.12, input.material_uv.y);
    let visualized = mix(tone * 0.82, tone * new_growth * 1.35, pow(depth, 0.8))
        * (0.90 + (strand.y * 0.20));

    // Needles are thin enough to light from behind, so foliage keeps taking
    // light past the terminator instead of falling to a hard shaded edge.
    let sun_direction = lighting.sun_direction_intensity.xyz;
    let normal_dot_sun = dot(normal, sun_direction);
    let diffuse_response = max(
        (normal_dot_sun + FOLIAGE_LIGHT_WRAP) / (1.0 + FOLIAGE_LIGHT_WRAP),
        0.0,
    );
    let view_direction = normalize(-input.render_position);
    let view_distance = length(input.render_position);
    let shadow = sun_visibility(
        input.render_position,
        view_distance,
        max(normal_dot_sun, 0.0),
    );
    var lit = ambient_and_direct(
        visualized,
        normal,
        ambient_occlusion,
        diffuse_response,
        shadow,
    );

    // A crown with the sun behind it glows rather than silhouetting: the light
    // that made it through a needle comes out the far side. Only the needles
    // standing clear of the shoot are thin enough to pass it, so this rides on
    // depth rather than on the shading the shells got.
    let transmission = pow(max(dot(-view_direction, sun_direction), 0.0), 3.0);
    lit += visualized
        * lighting.sun_color.rgb
        * transmission
        * NEEDLE_TRANSLUCENCY
        * depth
        * shadow
        * lighting.sun_direction_intensity.w;

    return vec4<f32>(aerial_perspective(lit, view_distance, input.elevation), 1.0);
}
