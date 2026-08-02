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

/// Deterministic xorshift generator so the texture is identical every launch.
struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// A value in `[0, 1)`.
    #[allow(clippy::cast_precision_loss)]
    fn next(&mut self) -> f64 {
        let mut state = self.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.0 = state;
        (state >> 11) as f64 / (u64::MAX >> 11) as f64
    }
}

/// Paints one needle fan and derives the matching normal and ARM maps.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn generate_needle_maps() -> NeedleMaps {
    const BLADE_COUNT: usize = 54;
    let edge = NEEDLE_TEXTURE_EDGE;
    let mut diffuse = RgbaImage::from_pixel(edge, edge, image::Rgba([0, 0, 0, 0]));
    let mut rng = Xorshift64::new(0x8AC5_2E6C_D96A_4B3F);
    let center = [f64::from(edge) * 0.5, f64::from(edge) * 0.48];
    let max_length = f64::from(edge) * 0.44;
    for blade in 0..BLADE_COUNT {
        const SEGMENTS: usize = 8;
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
            draw_needle_segment(
                &mut diffuse,
                previous,
                point,
                width * (1.0 - 0.45 * t),
                color,
            );
            previous = point;
        }
    }
    let normal = normal_from_alpha(&diffuse);
    let arm = arm_from_alpha(&diffuse);
    NeedleMaps {
        diffuse,
        normal,
        arm,
    }
}

/// Draws a tapered disc-spine segment of one needle blade.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
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
                    image.put_pixel(
                        x,
                        y,
                        image::Rgba([color[0] as u8, color[1] as u8, color[2] as u8, 255]),
                    );
                }
            }
        }
    }
}

/// Out-of-plane normals with gentle relief where blades rise off the fan.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
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
    let (dx, dy) = if along_x {
        (1_i64, 0_i64)
    } else {
        (0_i64, 1_i64)
    };
    (sample(i64::from(x) + dx, i64::from(y) + dy) - sample(i64::from(x) - dx, i64::from(y) - dy))
        / 510.0
}

/// AO darkens toward the dense fan base; roughness stays high and diffuse.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
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
                image::Rgba([
                    ((0.45 + 0.55 * falloff) * 255.0).clamp(0.0, 255.0) as u8,
                    217,
                    0,
                    255,
                ]),
            );
        }
    }
    arm
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
        assert_eq!(
            maps.diffuse.dimensions(),
            (NEEDLE_TEXTURE_EDGE, NEEDLE_TEXTURE_EDGE)
        );
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
