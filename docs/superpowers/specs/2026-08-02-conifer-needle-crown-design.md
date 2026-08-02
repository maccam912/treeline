# Conifer needle-puff crowns

Date: 2026-08-02

## Goal

Replace the flat green cone that conifers (`CrownShape::Conical`) draw today with a
crown made of many small needle billboards. The result should read as a real conifer —
dense needle clusters you can see through in places — without expensive per-tree
geometry, and it must stay within the renderer crate and keep the existing LOD behavior.

## Decisions

- **Geometry-first, crossed static planes.** Each "puff" is 2–3 vertical quads crossed
  at fixed angles and built entirely on the CPU. No camera-facing shader billboards, no
  vertex-format changes. With enough puffs at independent azimuths, only a few are ever
  near edge-on at once, and the sparse needle texture hides the rest.
- **Procedural needle texture.** The needle pattern is generated at load by a small pure,
  seeded function and added as a 5th layer to the material array. No new binary assets,
  no keying problems, bit-stable.
- **Kill the cone at every tier.** `append_conical_crown` is deleted. `Full`, `Simplified`,
  and `Silhouette` all render puffs, at decreasing count and increasing quad size.
- **Alpha test, not alpha blend.** `discard` in the fragment shader (the mechanism the
  terrain cutout already uses) gives hard-edged see-through without a transparent pass
  or sort order.

## Crown geometry

`append_conical_crown` is replaced by a puff placement + quad generator.

### Envelope

Puffs fill the same envelope the cone did, so near and far tiers stay aligned:

- Apex at `top + crown_radius * 0.18` (matching today's terminal cone).
- Base at `crown_start` (0.24 of the trunk vector) with radius `crown_radius`.
- Linear taper to a point at the apex: radius at height fraction `t` is
  `crown_radius * (1 - t)`.

### Placement

Puffs spiral up the envelope. For `i in 0..count`:

- `t = (i + 0.5) / count`
- azimuth `= i * golden_angle + hash(tree.id, i) * jitter`
- radius `= envelope_radius(t) * (0.55 + hash * 0.45)` so some puffs sit inside the
  envelope and some poke past the edge
- a small vertical jitter from the same hash
- one apex puff at the tip

Each puff gets its own rotation from the hash so the crossed planes are not aligned
between puffs.

### Puff shape

A puff is `planes` vertical quads crossed at `180/planes` degrees. Each quad is 4
vertices / 2 triangles, with the needle texture mapped across it and
`surface_kind = SURFACE_KIND_NEEDLE_FOLIAGE`. Back-face culling drops the far-facing
half of each crossed plane automatically.

### Counts by tier

Count derives from the genotype's `branch_density_fraction * leaf_density_fraction`,
scaled by tier:

| Tier | Planes per puff | Puffs (scaled by density) |
|------|-----------------|---------------------------|
| `Full` | 3 | ~14–24 |
| `Simplified` | 2 | ~8–12 |
| `Silhouette` | 2 | ~4–6, larger quads |
| Sapling | 2 | 2–4, small quads |

## Procedural needle texture

- Material layer count grows 4 → 5. Layer 5 is a generated RGBA needle-fan texture at
  512² with the same 10-level mip chain as the rest.
- A small pure function paints one fan: needle blades radiating from the quad center
  with slight droop/curve, alpha = blade presence, two-tone green, per-blade jitter,
  darker near the center and lighter at the tips. Seeded and bit-stable.
- A matching normal map and ARM (AO near the fan base, high roughness) are generated
  from the same blade mask, so the needle path reuses the material machinery.
- `upload_material_layers` gains a path for generated RGBA layers; the existing JPEG
  path is unchanged.

## Shader changes

- New constant `SURFACE_KIND_NEEDLE_FOLIAGE: f32 = 4.0` in `vertex.rs` (current band is
  0–3).
- `is_bark` is narrowed from `surface_kind > 1.5` to `surface_kind > 1.5 &&
  surface_kind < 4.0` so needle puffs do not sample bark.
- New `is_needle` branch (`surface_kind` in (3.5, 4.5)) in `terrain.wgsl`:
  sample diffuse layer 4, `discard` where alpha < ~0.35, tint by vertex color, then
  enter the shared sun/sky lighting path so needles light and shadow like other geometry.
- Per-vertex color is `foliage_color(tree)` plus a per-puff jitter from the id hash, so
  adjacent puffs differ slightly.

## Shadow pass

Unchanged. The shadow shader is depth-only and binds no material textures, so needle
discard in shadow would require a fragment stage plus material bindings. A conifer casts
a dense canopy shadow anyway, so solid puffs reading as a dense crown in shadow is
realistic. Adding alpha discard to the shadow pass is a possible follow-up.

## Tests

Texture:

- determinism: two generation runs are bit-identical
- the alpha mask is non-empty and within texture bounds
- material layer count is 5

Geometry:

- a stand builds well-formed colored geometry
- tiers strictly shed vertices (`Full > Simplified > Silhouette`)
- envelope alignment: every puff at every tier stays inside the cone envelope (the
  "distant tiers stay aligned" invariant)
- per-tree determinism and generation-order independence
- saplings get puffs
- every tier keeps at least one puff

Existing tests asserting monotonic vertex counts across tiers must still pass.

## Files touched

- `crates/renderer/src/tree_mesh/shape.rs` — crossed-quad puff generator; delete
  `append_conical_crown`
- `crates/renderer/src/tree_mesh/mod.rs` — puff placement; route both Conical call
  sites (terminal crown, sapling crown) to it
- `crates/renderer/src/tree_mesh/color.rs` — per-puff foliage tint helper
- `crates/renderer/src/vertex.rs` — `SURFACE_KIND_NEEDLE_FOLIAGE`
- `crates/renderer/src/material.rs` + new `needle_texture.rs` — 5th layer, procedural
  generator, RGBA upload path
- `crates/renderer/src/terrain.wgsl` — `is_needle` branch, `is_bark` scoping

All changes live in the renderer crate. No generation crates, ecosystem, or protocol
are touched, and world output is unchanged — this is purely a rendering change.

## Out of scope

- Wind sway on puffs
- Alpha discard in the shadow pass
- Texturing the broadleaf octahedra
- Any camera-facing billboard machinery
