# Conifer Needle-Puff Crowns Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat green cone that conifers draw with a crown built from crossed-quad needle billboards carrying a procedural needle texture, so crowns read as dense, see-through needle clusters.

**Architecture:** A conifer crown becomes a set of "puffs," each a few crossed vertical quads. Puffs are placed deterministically inside the old cone envelope, so LOD tiers stay aligned. The needle pattern is a procedurally generated 5th layer in the existing material array, selected by a new `surface_kind` band and clipped with `discard` in the fragment shader. All changes live in `crates/renderer`.

**Tech Stack:** Rust, wgpu (WGSL), the `image` crate for texture generation, `libm` for deterministic trig.

**Working convention:** AGENTS.md says work directly on `main` (no feature branch). Commit after each task with `git`.

---

## Task 1: Add the needle-foliage surface kind

**Files:**
- Modify: `crates/renderer/src/vertex.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block at the bottom of `crates/renderer/src/vertex.rs`:

```rust
    #[test]
    fn every_surface_kind_occupies_a_distinct_band() {
        let mut kinds = vec![
            SURFACE_KIND_SOLID,
            SURFACE_KIND_WATER,
            SURFACE_KIND_PINE_BARK,
            SURFACE_KIND_OAK_BARK,
            SURFACE_KIND_NEEDLE_FOLIAGE,
        ];
        kinds.sort_by(f32::total_cmp);
        kinds.dedup();
        assert_eq!(kinds.len(), 5);
        assert!(SURFACE_KIND_NEEDLE_FOLIAGE > SURFACE_KIND_OAK_BARK);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p treeline-renderer every_surface_kind_occupies_a_distinct_band`
Expected: FAIL — `SURFACE_KIND_NEEDLE_FOLIAGE` is not defined.

- [ ] **Step 3: Add the constant**

In `crates/renderer/src/vertex.rs`, after line 17 (`pub(crate) const SURFACE_KIND_OAK_BARK: f32 = 3.0;`):

```rust
pub(crate) const SURFACE_KIND_NEEDLE_FOLIAGE: f32 = 4.0;
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p treeline-renderer every_surface_kind_occupies_a_distinct_band`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/renderer/src/vertex.rs
git commit -m "Add needle foliage surface kind"
```

---

## Task 2: Procedural needle-fan texture generator

**Files:**
- Create: `crates/renderer/src/needle_texture.rs`
- Modify: `crates/renderer/src/lib.rs` (declare the module)

- [ ] **Step 1: Write the failing test**

Create `crates/renderer/src/needle_texture.rs` with only the test module first, plus the module declaration in `lib.rs`:

In `crates/renderer/src/lib.rs`, after line 18 (`mod material;`), add:

```rust
mod needle_texture;
```

In `crates/renderer/src/needle_texture.rs`:

```rust
//! Procedural needle-fan texture for conifer crowns.
//!
//! One needle fan is painted into an RGBA image: blades radiating from a
//! center, alpha-carrying so the shader can discard the gaps. The normal and
//! ARM maps are derived from the same mask so the three array layers stay in
//! register with each other.

use image::RgbaImage;

pub(crate) const NEEDLE_TEXTURE_EDGE: u32 = 1024;

pub(crate) struct NeedleMaps {
    pub(crate) diffuse: RgbaImage,
    pub(crate) normal: RgbaImage,
    pub(crate) arm: RgbaImage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_needle_maps_are_bit_stable() {
        let first = generate_needle_maps();
        let second = generate_needle_maps();
        assert_eq!(first.diffuse.as_raw(), second.diffuse.as_raw());
        assert_eq!(first.normal.as_raw(), second.normal.as_raw());
        assert_eq!(first.arm.as_raw(), second.arm.as_raw());
    }

    #[test]
    fn the_needle_fan_has_transparent_gaps_and_opaque_blades() {
        let maps = generate_needle_maps();
        assert_eq!(maps.diffuse.dimensions(), (NEEDLE_TEXTURE_EDGE, NEEDLE_TEXTURE_EDGE));
        let mut transparent = 0_u64;
        let mut opaque = 0_u64;
        for pixel in maps.diffuse.pixels() {
            if pixel[3] == 0 {
                transparent += 1;
            } else {
                opaque += 1;
            }
        }
        assert!(transparent > 0, "the fan must leave transparent gaps");
        assert!(opaque > 0, "the fan must paint blades");
    }

    #[test]
    fn the_normal_and_arm_maps_match_the_diffuse_dimensions() {
        let maps = generate_needle_maps();
        assert_eq!(maps.normal.dimensions(), maps.diffuse.dimensions());
        assert_eq!(maps.arm.dimensions(), maps.diffuse.dimensions());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p treeline-renderer the_needle_fan_has_transparent_gaps_and_opaque_blades`
Expected: FAIL — `generate_needle_maps` is not defined.

- [ ] **Step 3: Implement the generator**

Add the following to `crates/renderer/src/needle_texture.rs`, between the `NeedleMaps` struct and the `#[cfg(test)]` module:

```rust
/// Deterministic xorshift generator so the texture is identical every launch.
struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// A value in `[0, 1)`.
    fn next(&mut self) -> f64 {
        let mut state = self.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.0 = state;
        f64::from(state >> 11) / f64::from(u64::MAX >> 11)
    }
}

/// Paints one needle fan and derives the matching normal and ARM maps.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub(crate) fn generate_needle_maps() -> NeedleMaps {
    let edge = NEEDLE_TEXTURE_EDGE;
    let mut diffuse = RgbaImage::from_pixel(edge, edge, image::Rgba([0, 0, 0, 0]));
    let mut rng = Xorshift64::new(0x8AC5_2E6C_D96A_4B3F);
    let center = [f64::from(edge) * 0.5, f64::from(edge) * 0.48];
    let max_length = f64::from(edge) * 0.44;
    const BLADE_COUNT: usize = 54;
    for blade in 0..BLADE_COUNT {
        let angle = f64::from(blade as u32) / BLADE_COUNT as f64 * std::f64::consts::TAU
            + rng.next() * 0.32;
        let length = max_length * (0.30 + rng.next() * 0.70);
        let droop = 0.06 + rng.next() * 0.12;
        let width = 3.0 + rng.next() * 3.0;
        let tone = rng.next();
        let base_green = [46.0 + tone * 34.0, 108.0 + tone * 46.0, 52.0 + tone * 26.0];
        let tip_green = [76.0 + tone * 30.0, 150.0 + tone * 42.0, 70.0 + tone * 26.0];
        let direction = [libm::cos(angle), -libm::sin(angle)];
        let mut previous = center;
        const SEGMENTS: usize = 8;
        for segment in 1..=SEGMENTS {
            let t = segment as f64 / SEGMENTS as f64;
            let point = [
                center[0] + direction[0] * length * t,
                center[1] + direction[1] * length * t + droop * length * t * t,
            ];
            let color = [
                base_green[0] + (tip_green[0] - base_green[0]) * t,
                base_green[1] + (tip_green[1] - base_green[1]) * t,
                base_green[2] + (tip_green[2] - base_green[2]) * t,
            ];
            draw_needle_segment(&mut diffuse, previous, point, width * (1.0 - 0.45 * t), color);
            previous = point;
        }
    }
    let normal = normal_from_alpha(&diffuse);
    let arm = arm_from_alpha(&diffuse);
    NeedleMaps { diffuse, normal, arm }
}

/// Draws a tapered disc-spine segment of one needle blade.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn draw_needle_segment(
    image: &mut RgbaImage,
    start: [f64; 2],
    end: [f64; 2],
    width: f64,
    color: [f64; 3],
) {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    let distance = libm::hypot(dx, dy);
    if distance < f64::EPSILON {
        return;
    }
    let radius = width * 0.5;
    let radius_squared = radius * radius;
    let steps = libm::ceil(distance / 1.5).max(1.0) as u64;
    for step in 0..=steps {
        let t = step as f64 / steps as f64;
        let center_x = start[0] + dx * t;
        let center_y = start[1] + dy * t;
        let min_x = (center_x - radius).floor().max(0.0) as u32;
        let max_x = (center_x + radius)
            .ceil()
            .min(f64::from(image.width().saturating_sub(1))) as u32;
        let min_y = (center_y - radius).floor().max(0.0) as u32;
        let max_y = (center_y + radius)
            .ceil()
            .min(f64::from(image.height().saturating_sub(1))) as u32;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let offset_x = f64::from(x) - center_x;
                let offset_y = f64::from(y) - center_y;
                if offset_x * offset_x + offset_y * offset_y <= radius_squared {
                    image.put_pixel(x, y, image::Rgba([color[0] as u8, color[1] as u8, color[2] as u8, 255]));
                }
            }
        }
    }
}

/// Out-of-plane normals with gentle relief where blades rise off the fan.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn normal_from_alpha(alpha: &RgbaImage) -> RgbaImage {
    let (width, height) = alpha.dimensions();
    let mut normal = RgbaImage::from_pixel(width, height, image::Rgba([128, 128, 255, 255]));
    for y in 0..height {
        for x in 0..width {
            let gradient_x = alpha_gradient(alpha, x, y, true);
            let gradient_y = alpha_gradient(alpha, x, y, false);
            let normal_z = 1.0_f64;
            let length = libm::hypot(libm::hypot(gradient_x * 0.7, gradient_y * 0.7), normal_z);
            normal.put_pixel(
                x,
                y,
                image::Rgba([
                    (gradient_x * 0.7 / length * 127.0 + 128.0).clamp(0.0, 255.0) as u8,
                    (gradient_y * 0.7 / length * 127.0 + 128.0).clamp(0.0, 255.0) as u8,
                    (normal_z / length * 127.0 + 128.0).clamp(0.0, 255.0) as u8,
                    255,
                ]),
            );
        }
    }
    normal
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn alpha_gradient(alpha: &RgbaImage, x: u32, y: u32, along_x: bool) -> f64 {
    let (width, height) = alpha.dimensions();
    let sample = |u: i64, v: i64| {
        let u = u.clamp(0, i64::from(width) - 1) as u32;
        let v = v.clamp(0, i64::from(height) - 1) as u32;
        f64::from(alpha.get_pixel(u, v)[3])
    };
    let (dx, dy) = if along_x { (1_i64, 0_i64) } else { (0_i64, 1_i64) };
    (sample(i64::from(x) + dx, i64::from(y) + dy) - sample(i64::from(x) - dx, i64::from(y) - dy))
        / 510.0
}

/// AO darkens toward the dense fan base; roughness stays high and diffuse.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn arm_from_alpha(diffuse: &RgbaImage) -> RgbaImage {
    let (width, height) = diffuse.dimensions();
    let center = [f64::from(width) * 0.5, f64::from(height) * 0.48];
    let max_radius = libm::hypot(center[0], center[1]);
    let mut arm = RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 0]));
    for y in 0..height {
        for x in 0..width {
            if diffuse.get_pixel(x, y)[3] == 0 {
                continue;
            }
            let distance = libm::hypot(f64::from(x) - center[0], f64::from(y) - center[1]);
            let falloff = (distance / max_radius).clamp(0.0, 1.0);
            arm.put_pixel(
                x,
                y,
                image::Rgba([((0.45 + 0.55 * falloff) * 255.0).clamp(0.0, 255.0) as u8, 217, 0, 255]),
            );
        }
    }
    arm
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p treeline-renderer needle_texture`
Expected: all three PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/renderer/src/lib.rs crates/renderer/src/needle_texture.rs
git commit -m "Add procedural needle-fan texture generator"
```

---

## Task 3: Add the needle layer to the material array

**Files:**
- Modify: `crates/renderer/src/material.rs`

- [ ] **Step 1: Write the failing test**

Change the existing test `the_array_texture_has_one_layer_per_material` in `crates/renderer/src/material.rs` to:

```rust
    #[test]
    fn the_array_texture_has_one_layer_per_material() {
        assert_eq!(
            usize::try_from(MATERIAL_TEXTURE_LAYER_COUNT).expect("layer count fits usize"),
            EMBEDDED_MATERIALS.len() + 1
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p treeline-renderer the_array_texture_has_one_layer_per_material`
Expected: FAIL — layer count is 4 but `EMBEDDED_MATERIALS.len() + 1` is 5.

- [ ] **Step 3: Bump the layer count**

In `crates/renderer/src/material.rs`, change line 11:

```rust
pub(crate) const MATERIAL_TEXTURE_LAYER_COUNT: u32 = 4;
```

to:

```rust
pub(crate) const MATERIAL_TEXTURE_LAYER_COUNT: u32 = 5;
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p treeline-renderer the_array_texture_has_one_layer_per_material`
Expected: PASS

- [ ] **Step 5: Generate the layer and upload it**

Rewrite the imports at the top of `crates/renderer/src/material.rs` (line 7) to:

```rust
use image::ImageFormat;
use image::imageops::{FilterType, resize};
use image::RgbaImage;

use crate::needle_texture::generate_needle_maps;
```

Replace the body of `MaterialTextures::new` (currently lines 75–136) so it builds five layers from the four embedded JPEGs plus the generated needle maps, and replace `upload_material_layers` (currently lines 160–210) so it accepts `RgbaImage` layers. Add a `material_layers` helper. The full new `new`, helper, and `upload_material_layers`:

```rust
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let needle = generate_needle_maps();
        let diffuse_layers =
            material_layers(EMBEDDED_MATERIALS.map(|material| material.diffuse), needle.diffuse);
        let normal_layers =
            material_layers(EMBEDDED_MATERIALS.map(|material| material.normal), needle.normal);
        let arm_layers =
            material_layers(EMBEDDED_MATERIALS.map(|material| material.arm), needle.arm);
        let diffuse_texture = create_material_texture(
            device,
            "Poly Haven material diffuse array",
            wgpu::TextureFormat::Rgba8UnormSrgb,
        );
        let normal_texture = create_material_texture(
            device,
            "Poly Haven material normal array",
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let arm_texture = create_material_texture(
            device,
            "Poly Haven material AO roughness metalness array",
            wgpu::TextureFormat::Rgba8Unorm,
        );
        upload_material_layers(queue, &diffuse_texture, &diffuse_layers);
        upload_material_layers(queue, &normal_texture, &normal_layers);
        upload_material_layers(queue, &arm_texture, &arm_layers);
        let array_view = |texture: &wgpu::Texture, label| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(label),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let diffuse_view = array_view(&diffuse_texture, "Poly Haven material diffuse array view");
        let normal_view = array_view(&normal_texture, "Poly Haven material normal array view");
        let arm_view = array_view(&arm_texture, "Poly Haven material ARM array view");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("surface material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 4,
            ..Default::default()
        });
        Self {
            _diffuse_texture: diffuse_texture,
            diffuse_view,
            _normal_texture: normal_texture,
            normal_view,
            _arm_texture: arm_texture,
            arm_view,
            sampler,
        }
    }
```

Add this free function directly above `create_material_texture`:

```rust
/// Decodes the four embedded scans and appends the generated needle layer.
fn material_layers(embedded: [&[u8]; 4], generated: RgbaImage) -> Vec<RgbaImage> {
    let mut layers = embedded
        .map(|encoded| {
            let decoded = image::load_from_memory_with_format(encoded, ImageFormat::Jpeg)
                .expect("embedded Poly Haven material JPEG must decode")
                .to_rgba8();
            assert_eq!(
                decoded.dimensions(),
                (1_024, 1_024),
                "embedded Poly Haven material maps must retain their source dimensions"
            );
            decoded
        })
        .to_vec();
    layers.push(generated);
    layers
}
```

Replace `upload_material_layers` entirely with:

```rust
pub(crate) fn upload_material_layers(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    layers: &[RgbaImage],
) {
    for (layer, decoded) in layers.iter().enumerate() {
        let mut mip = resize(
            decoded,
            MATERIAL_TEXTURE_EDGE,
            MATERIAL_TEXTURE_EDGE,
            FilterType::Triangle,
        );
        for mip_level in 0..MATERIAL_TEXTURE_MIP_COUNT {
            let (width, height) = mip.dimensions();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: u32::try_from(layer).expect("material layer count fits u32"),
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                mip.as_raw(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            if width > 1 {
                mip = resize(&mip, width / 2, height / 2, FilterType::Triangle);
            }
        }
    }
}
```

- [ ] **Step 6: Run the full renderer test suite**

Run: `cargo test -p treeline-renderer`
Expected: all PASS (including `every_embedded_map_decodes_at_the_expected_resolution`, which still reads the 1024×1024 embedded JPEGs).

- [ ] **Step 7: Commit**

```bash
git add crates/renderer/src/material.rs
git commit -m "Add needle foliage layer to material array"
```

---

## Task 4: Crossed-quad needle puff geometry

**Files:**
- Modify: `crates/renderer/src/tree_mesh/shape.rs`
- Modify: `crates/renderer/src/tree_mesh/mod.rs`

- [ ] **Step 1: Write the failing test**

In `crates/renderer/src/tree_mesh/mod.rs`, inside `mod tests` (after the `assert_well_formed` helper), add:

```rust
    #[test]
    fn a_needle_puff_builds_crossed_front_facing_quads() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        append_needle_puff(
            &mut vertices,
            &mut indices,
            Vec3::ZERO,
            1.0,
            2,
            0.0,
            [0.3, 0.5, 0.3, 1.0],
        )
        .expect("puff geometry");
        assert_eq!(vertices.len(), 2 * 4);
        assert_eq!(indices.len(), 2 * 2 * 3);
        assert_well_formed(&vertices, &indices);
        assert!(
            vertices
                .iter()
                .all(|vertex| vertex.surface_kind == SURFACE_KIND_NEEDLE_FOLIAGE)
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p treeline-renderer a_needle_puff_builds_crossed_front_facing_quads`
Expected: FAIL — `append_needle_puff` is not defined.

- [ ] **Step 3: Implement the puff geometry**

In `crates/renderer/src/tree_mesh/shape.rs`, update the `use` list (lines 11–14) to import `SURFACE_KIND_NEEDLE_FOLIAGE`:

```rust
use crate::vertex::{
    SURFACE_KIND_NEEDLE_FOLIAGE, SURFACE_KIND_OAK_BARK, SURFACE_KIND_PINE_BARK, TerrainVertex,
    local_vertex, material_vertex, usize_as_f32,
};
```

Add this function to `crates/renderer/src/tree_mesh/shape.rs` (it can replace `append_conical_crown` in place, lines 103–146, but leave `append_conical_crown` in place for now — it is removed in Task 5):

```rust
pub(crate) fn append_needle_puff(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    center: Vec3,
    half_extent: f32,
    planes: usize,
    rotation_radians: f32,
    color: [f32; 4],
) -> Result<(), RendererError> {
    let base_index = u32::try_from(vertices.len()).map_err(|_| RendererError::TooManyIndices)?;
    for plane in 0..planes {
        let angle =
            rotation_radians + usize_as_f32(plane) / usize_as_f32(planes) * std::f32::consts::PI;
        let normal = Vec3::new(libm::cosf(angle), 0.0, libm::sinf(angle));
        let width = normal.cross(Vec3::Y);
        let corner = |sign_x: f32, sign_y: f32| {
            center + (width * (sign_x * half_extent)) + (Vec3::Y * (sign_y * half_extent))
        };
        let corners = [
            corner(-1.0, 1.0),
            corner(1.0, 1.0),
            corner(1.0, -1.0),
            corner(-1.0, -1.0),
        ];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for (corner, uv) in corners.into_iter().zip(uvs) {
            vertices.push(material_vertex(
                corner,
                normal,
                color,
                SURFACE_KIND_NEEDLE_FOLIAGE,
                uv,
            ));
        }
        let plane_base =
            base_index + u32::try_from(plane * 4).map_err(|_| RendererError::TooManyIndices)?;
        indices.extend_from_slice(&[
            plane_base,
            plane_base + 1,
            plane_base + 2,
            plane_base,
            plane_base + 2,
            plane_base + 3,
        ]);
    }
    Ok(())
}
```

In `crates/renderer/src/tree_mesh/mod.rs`, add `append_needle_puff` to the `use shape::{...}` list (line 19), keeping `append_conical_crown` for now (it is removed in Task 5), and import the new constant (line 14):

```rust
use crate::vertex::{SURFACE_KIND_NEEDLE_FOLIAGE, TerrainVertex, f64_as_f32, hash_lane, translate_local_vertices, usize_as_f32};
```

```rust
use shape::{CylinderSpec, append_conical_crown, append_needle_puff, append_octahedral_crown, append_tapered_cylinder};
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p treeline-renderer a_needle_puff_builds_crossed_front_facing_quads`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/renderer/src/tree_mesh/shape.rs crates/renderer/src/tree_mesh/mod.rs
git commit -m "Add crossed-quad needle puff geometry"
```

---

## Task 5: Build conifer crowns from needle puffs

**Files:**
- Modify: `crates/renderer/src/tree_mesh/mod.rs`
- Modify: `crates/renderer/src/tree_mesh/color.rs`
- Modify: `crates/renderer/src/tree_mesh/shape.rs` (delete `append_conical_crown`)

- [ ] **Step 1: Write the failing tests**

Add these tests to `mod tests` in `crates/renderer/src/tree_mesh/mod.rs` (after the tests from Task 4). Note `CrownShape` is already imported in the test module's `use` (line 318). You will also need `bytemuck` in the test module:

```rust
    fn conifer() -> ProceduralTree {
        stand()
            .into_iter()
            .find(|tree| tree.genotype.crown_shape == CrownShape::Conical)
            .expect("a conifer in the mixture")
    }

    fn sapling() -> ProceduralTree {
        (0..10_000_u64)
            .map(tree)
            .find(|tree| tree.condition == TreeCondition::Sapling)
            .expect("a sapling in the population")
    }

    /// Every tier places its puffs inside the same cone envelope, so distant
    /// crowns stay spatially aligned with near ones.
    #[test]
    fn needle_puffs_stay_within_the_crown_envelope_at_every_tier() {
        let tree = conifer();
        let crown_base = Vec3::ZERO;
        let apex = Vec3::new(0.0, 20.0, 0.0);
        let crown_radius = 4.0;
        let half_extent = crown_radius
            * (0.15 + f64_as_f32(tree.genotype.leaf_density_fraction) * 0.10);
        let margin = half_extent * 1.6 + 0.05;
        let axis = apex - crown_base;
        let axis_length = axis.length();
        let axis_dir = axis / axis_length;
        for detail in [
            TreeMeshDetail::Full,
            TreeMeshDetail::Simplified,
            TreeMeshDetail::Silhouette,
        ] {
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            append_needle_crown(
                &mut vertices,
                &mut indices,
                tree,
                crown_base,
                apex,
                crown_radius,
                [0.3, 0.5, 0.3, 1.0],
                detail,
            )
            .expect("crown geometry");
            assert!(!vertices.is_empty(), "{detail:?} produced no puffs");
            for vertex in &vertices {
                let position = Vec3::new(
                    vertex.position_high[0] + vertex.position_low[0],
                    vertex.position_high[1] + vertex.position_low[1],
                    vertex.position_high[2] + vertex.position_low[2],
                );
                let relative = position - crown_base;
                let along = relative.dot(axis_dir);
                let t = along / axis_length;
                assert!(t > -0.01 && t < 1.01, "{detail:?} puff at t={t}");
                let lateral = relative - (axis_dir * along);
                let distance = lateral.length();
                let envelope = crown_radius * (1.0 - t).max(0.0);
                assert!(
                    distance <= envelope + margin,
                    "{detail:?} puff escaped the envelope: {distance} > {envelope} + {margin}"
                );
            }
        }
    }

    #[test]
    fn every_tier_keeps_needle_puffs() {
        let stand = stand();
        for detail in [
            TreeMeshDetail::Full,
            TreeMeshDetail::Simplified,
            TreeMeshDetail::Silhouette,
        ] {
            let (vertices, _) = procedural_tree_geometry(&stand, detail, |_, _| Some(42.0))
                .expect("tree geometry");
            assert!(
                vertices
                    .iter()
                    .any(|vertex| vertex.surface_kind == SURFACE_KIND_NEEDLE_FOLIAGE),
                "{detail:?} lost its needle puffs"
            );
        }
    }

    #[test]
    fn saplings_render_needle_puffs() {
        let (vertices, _) =
            procedural_tree_geometry(&[sapling()], TreeMeshDetail::Full, |_, _| Some(42.0))
                .expect("tree geometry");
        assert!(
            vertices
                .iter()
                .any(|vertex| vertex.surface_kind == SURFACE_KIND_NEEDLE_FOLIAGE)
        );
    }

    /// Geometry must be bit-stable for one input and identical whether trees
    /// are meshed together or one at a time.
    #[test]
    fn a_trees_geometry_is_bit_stable_and_neighbor_independent() {
        let stand = stand();
        let batch = procedural_tree_geometry(&stand, TreeMeshDetail::Full, |_, _| Some(42.0))
            .expect("tree geometry");
        let again = procedural_tree_geometry(&stand, TreeMeshDetail::Full, |_, _| Some(42.0))
            .expect("tree geometry");
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&batch.0),
            bytemuck::cast_slice::<_, u8>(&again.0)
        );
        assert_eq!(batch.1, again.1);

        let mut concatenated_vertices = Vec::new();
        let mut concatenated_indices = Vec::new();
        for tree in &stand {
            let (mut vertices, mut indices) =
                procedural_tree_geometry(&[*tree], TreeMeshDetail::Full, |_, _| Some(42.0))
                    .expect("tree geometry");
            let base = u32::try_from(concatenated_vertices.len()).expect("vertex count fits u32");
            for index in &mut indices {
                *index += base;
            }
            concatenated_vertices.append(&mut vertices);
            concatenated_indices.append(&mut indices);
        }
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&batch.0),
            bytemuck::cast_slice::<_, u8>(&concatenated_vertices)
        );
        assert_eq!(batch.1, concatenated_indices);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p treeline-renderer needle_puffs_stay_within_the_crown_envelope_at_every_tier`
Expected: FAIL — `append_needle_crown` is not defined.

- [ ] **Step 3: Add the per-puff tint helper**

In `crates/renderer/src/tree_mesh/color.rs`, add after `foliage_color` (line 49):

```rust
pub(crate) fn puff_color(tree: ProceduralTree, foliage: [f32; 4], lane: usize) -> [f32; 4] {
    let jitter = (hash_lane(tree.id, lane + 24) - 0.5) * 0.14;
    [
        (foliage[0] + jitter).clamp(0.0, 1.0),
        (foliage[1] + jitter).clamp(0.0, 1.0),
        (foliage[2] + (jitter * 0.5)).clamp(0.0, 1.0),
        1.0,
    ]
}
```

- [ ] **Step 4: Implement the crown placement and rewire the call sites**

In `crates/renderer/src/tree_mesh/mod.rs`:

1. Update the `use color::{...}` list (line 16) to add `puff_color`:

```rust
use color::{
    CylinderMaterial, bark_color, bark_cylinder_material, foliage_color, puff_color, tree_has_foliage,
};
```

2. Add `append_needle_crown` after `append_tree_branch` (after line 230). This needs `libm` for `roundf`:

```rust
/// A conifer crown as a cloud of crossed-quad needle puffs.
///
/// Puffs spiral up the same envelope the old cone used, so every detail tier
/// keeps the crown silhouette aligned. Puff count scales with the genotype's
/// combined branch and leaf density; quad size stays constant so coarser tiers
/// never poke past the fuller one.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(crate) fn append_needle_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    crown_base: Vec3,
    apex: Vec3,
    crown_radius: f32,
    foliage: [f32; 4],
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    let axis = apex - crown_base;
    let axis_length = axis.length();
    if axis_length <= f32::EPSILON {
        return Ok(());
    }
    let direction = axis / axis_length;
    let density =
        f64_as_f32(tree.genotype.branch_density_fraction * tree.genotype.leaf_density_fraction);
    let (planes, base_count) = match detail {
        TreeMeshDetail::Full => (3, 12.0 + density * 12.0),
        TreeMeshDetail::Simplified => (2, 6.0 + density * 6.0),
        TreeMeshDetail::Silhouette => (2, 4.0 + density * 3.0),
    };
    let count = usize::try_from(libm::roundf(base_count.clamp(1.0, 48.0)) as i32)
        .expect("puff count fits usize");
    let half_extent = crown_radius
        * (0.15 + f64_as_f32(tree.genotype.leaf_density_fraction) * 0.10);
    let reference = if direction.y.abs() < 0.92 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = direction.cross(reference).normalize_or_zero();
    let bitangent = direction.cross(tangent).normalize_or_zero();
    let golden = 0.618_034_f32;
    for index in 0..count {
        let t = usize_as_f32(index + 1) / usize_as_f32(count + 1);
        let azimuth =
            (golden * usize_as_f32(index) + hash_lane(tree.id, index) * 0.5) * std::f32::consts::TAU;
        let radial = (tangent * libm::cosf(azimuth)) + (bitangent * libm::sinf(azimuth));
        let envelope_radius =
            crown_radius * (1.0 - t) * (0.55 + hash_lane(tree.id, index + 8) * 0.45);
        let position = (crown_base + (direction * (axis_length * t))) + (radial * envelope_radius);
        let rotation = hash_lane(tree.id, index + 16) * std::f32::consts::TAU;
        append_needle_puff(
            vertices,
            indices,
            position,
            half_extent,
            planes,
            rotation,
            puff_color(tree, foliage, index),
        )?;
    }
    append_needle_puff(
        vertices,
        indices,
        apex,
        half_extent * 0.7,
        planes,
        hash_lane(tree.id, 31) * std::f32::consts::TAU,
        puff_color(tree, foliage, count),
    )?;
    Ok(())
}
```

3. Give `append_tree_crown` and `append_terminal_crown` a `detail: TreeMeshDetail` parameter and route the Conical branches to `append_needle_crown`:

Replace `append_tree_crown` (lines 126–164) with:

```rust
pub(crate) fn append_tree_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    let branch_count = branch_count(tree);
    let crown_start = match tree.genotype.crown_shape {
        CrownShape::Conical => 0.24,
        CrownShape::Columnar => 0.38,
        CrownShape::Rounded => 0.46,
    };
    let crown_radius = f64_as_f32(tree.crown_radius_meters);
    let foliage = foliage_color(tree);
    for branch_index in 0..branch_count {
        append_tree_branch(
            vertices,
            indices,
            tree,
            frame,
            crown_start,
            branch_index,
            branch_count,
        )?;
    }

    if !tree_has_foliage(tree) {
        return Ok(());
    }
    append_terminal_crown(
        vertices,
        indices,
        tree,
        frame,
        crown_start,
        crown_radius,
        foliage,
        detail,
    )
}
```

Replace the Conical arm of `append_terminal_crown` (currently lines 241–249) with a call to `append_needle_crown`, and add the `detail` parameter to its signature (line 232–240):

```rust
pub(crate) fn append_terminal_crown(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    tree: ProceduralTree,
    frame: TreeFrame,
    crown_start: f32,
    crown_radius: f32,
    foliage: [f32; 4],
    detail: TreeMeshDetail,
) -> Result<(), RendererError> {
    match tree.genotype.crown_shape {
        CrownShape::Conical => append_needle_crown(
            vertices,
            indices,
            tree,
            frame.base + (frame.trunk_vector * crown_start),
            frame.top + (Vec3::Y * crown_radius * 0.18),
            crown_radius,
            foliage,
            detail,
        ),
        CrownShape::Columnar | CrownShape::Rounded => append_octahedral_crown(
            vertices,
            indices,
            frame.base + (frame.trunk_vector * 0.82),
            Vec3::new(
                crown_radius * 0.72,
                crown_radius
                    * if tree.genotype.crown_shape == CrownShape::Columnar {
                        1.25
                    } else {
                        0.82
                    },
                crown_radius * 0.72,
            ),
            foliage,
        ),
    }
}
```

4. Update the two call sites. In `append_tree`, change the dispatch so both the Full path and the coarser path forward `detail`:

```rust
    if detail == TreeMeshDetail::Full {
        append_tree_crown(vertices, indices, tree, frame, detail)
    } else if tree_has_foliage(tree) {
        let crown_start = match tree.genotype.crown_shape {
            CrownShape::Conical => 0.24,
            CrownShape::Columnar => 0.38,
            CrownShape::Rounded => 0.46,
        };
        append_terminal_crown(
            vertices,
            indices,
            tree,
            frame,
            crown_start,
            f64_as_f32(tree.crown_radius_meters),
            foliage_color(tree),
            detail,
        )
    } else {
        Ok(())
    }
```

5. Replace the Conical arm of `append_sapling_crown` (currently lines 280–288) with:

```rust
    if tree.genotype.crown_shape == CrownShape::Conical {
        append_needle_crown(
            vertices,
            indices,
            tree,
            base + ((top - base) * 0.36),
            top,
            radius,
            foliage_color(tree),
            TreeMeshDetail::Simplified,
        )
    } else {
```

Note: saplings use the `Simplified` tier, so they get 6–12 small puffs scaled down by their small `crown_radius` rather than the spec table's nominal 2–4 — the smaller envelope keeps them proportionate, and this avoids inventing a fourth `TreeMeshDetail` variant.

6. Remove `append_conical_crown` from `crates/renderer/src/tree_mesh/shape.rs` (lines 103–146) and drop it from the `use shape::{...}` list in `mod.rs`:

```rust
use shape::{CylinderSpec, append_needle_puff, append_octahedral_crown, append_tapered_cylinder};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p treeline-renderer tree_mesh`
Expected: all PASS, including the existing `coarser_detail_sheds_geometry_without_dropping_trees` (puff counts strictly shrink: Full 12–24 puffs × 3 planes > Simplified 6–12 × 2 > Silhouette 4–7 × 2).

- [ ] **Step 6: Commit**

```bash
git add crates/renderer/src/tree_mesh/
git commit -m "Build conifer crowns from needle puffs"
```

---

## Task 6: Shade and clip needle puffs in the shader

**Files:**
- Modify: `crates/renderer/src/terrain.wgsl`

- [ ] **Step 1: Narrow the bark band so needles do not sample bark**

In `crates/renderer/src/terrain.wgsl`, change line 448:

```wgsl
    let is_bark = input.surface_kind > 1.5;
```

to:

```wgsl
    let is_bark = input.surface_kind > 1.5 && input.surface_kind < 4.0;
```

- [ ] **Step 2: Add the needle branch**

Insert the following block immediately after the `is_bark` block closes (after line 497, just before the `// Dedicated hydrology sheets...` comment at line 499):

```wgsl
    let is_needle = input.surface_kind > 3.5 && input.surface_kind < 4.5;
    if (is_needle) {
        let needle_diffuse = textureSampleGrad(
            material_diffuse,
            material_sampler,
            input.material_uv,
            4,
            material_uv_dx,
            material_uv_dy,
        );
        if (needle_diffuse.a < 0.35) {
            discard;
        }
        let needle_normal_map = textureSampleGrad(
            material_normal,
            material_sampler,
            input.material_uv,
            4,
            material_uv_dx,
            material_uv_dy,
        );
        let needle_arm = textureSampleGrad(
            material_arm,
            material_sampler,
            input.material_uv,
            4,
            material_uv_dx,
            material_uv_dy,
        );
        let tangent_normal = needle_normal_map.xyz * 2.0 - 1.0;
        normal = normalize(
            bark_frame
            * normalize(vec3<f32>(
                tangent_normal.xy * 0.6,
                max(tangent_normal.z, 0.1),
            ))
        );
        surface_ambient_occlusion = needle_arm.r;
        surface_roughness = needle_arm.g;
        visualized = needle_diffuse.rgb * input.color.rgb;
    }
```

- [ ] **Step 3: Verify the workspace still builds**

Run: `cargo build -p treeline-renderer`
Expected: SUCCESS. The WGSL is validated by wgpu at pipeline creation, which happens at runtime; the build confirms the Rust side compiles. After the full gate in Task 7, launch the client (`cargo run -p client`) or `generator-lab` and confirm conifer crowns show needle clumps with see-through gaps instead of a smooth green cone.

- [ ] **Step 4: Commit**

```bash
git add crates/renderer/src/terrain.wgsl
git commit -m "Shade and clip needle puffs in the terrain shader"
```

---

## Task 7: Full gate

- [ ] **Step 1: Run the full local gate**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Expected: all pass. The pedantic clippy group is enabled at the workspace level, so any `cast_possible_truncation`, `cast_sign_loss`, or `cast_precision_loss` outside the scoped `#[allow(...)]` attributes will fail here.

- [ ] **Step 2: Check the wasm gate**

Run: `cargo clippy -p client --target wasm32-unknown-unknown --all-targets -- -D warnings`
Expected: PASS (the renderer compiles for wasm; `image` and `libm` are already used there).

- [ ] **Step 3: Visual confirmation**

Run: `cargo run -p client` (or `cargo run -p generator-lab`)
Expected: conifer crowns are clouds of needle puffs with visible gaps to the background; distant trees keep a matching silhouette; no green cones remain.

- [ ] **Step 4: Update DESIGN.md if the roadmap tracker mentions tree rendering**

Check `DESIGN.md` for a roadmap item covering tree/crown rendering. If it claims "conical crown" behavior or has a tracker entry affected by this change, update it in the same change per AGENTS.md. Commit if so.
