//! Exact byte layouts the shaders read, and the math that fills them.
//!
//! Every struct here is `#[repr(C)]` and must stay in step with the matching
//! declaration in the WGSL sources. Positions arrive as camera-relative pairs
//! of `f32` because a single `f32` cannot hold world coordinates at this scale.

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat4, Vec3};

use crate::lighting::{AtmosphereSettings, LightingSettings};
use crate::vertex::f64_as_f32;

pub(crate) const SHADOW_CASCADE_COUNT: usize = 3;
pub(crate) const SHADOW_MAP_SIZE: u32 = 1_024;
pub(crate) const SHADOW_CASCADE_SPLITS_METERS: [f32; SHADOW_CASCADE_COUNT] = [48.0, 140.0, 360.0];
pub(crate) const SHADOW_CASCADE_RADII_METERS: [f64; SHADOW_CASCADE_COUNT] = [56.0, 164.0, 424.0];
pub(crate) const SHADOW_DEPTH_METERS: f64 = 3_000.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct CameraUniform {
    pub(crate) view_projection: [[f32; 4]; 4],
    pub(crate) inverse_view_projection: [[f32; 4]; 4],
    pub(crate) render_origin_high: [f32; 4],
    pub(crate) render_origin_low: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct TerrainCutoutUniform {
    pub(crate) min_high: [f32; 2],
    pub(crate) min_low: [f32; 2],
    pub(crate) max_high: [f32; 2],
    pub(crate) max_low: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct AtmosphereUniform {
    pub(crate) fog_color_density: [f32; 4],
    pub(crate) wind_moisture: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct LightingUniform {
    pub(crate) sun_direction_intensity: [f32; 4],
    pub(crate) sun_color: [f32; 4],
    pub(crate) sky_zenith: [f32; 4],
    pub(crate) sky_horizon: [f32; 4],
    pub(crate) ground_ambient: [f32; 4],
    pub(crate) cascade_splits: [f32; 4],
    pub(crate) shadow_view_projection: [[[f32; 4]; 4]; SHADOW_CASCADE_COUNT],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct ShadowCameraUniform {
    pub(crate) view_projection: [[f32; 4]; 4],
    pub(crate) render_origin_high: [f32; 4],
    pub(crate) render_origin_low: [f32; 4],
}

pub(crate) fn atmosphere_uniform(settings: AtmosphereSettings) -> AtmosphereUniform {
    AtmosphereUniform {
        fog_color_density: [
            settings.fog_color[0],
            settings.fog_color[1],
            settings.fog_color[2],
            settings.fog_density.max(0.0),
        ],
        wind_moisture: [
            settings.prevailing_wind[0],
            settings.prevailing_wind[1],
            settings.moisture.clamp(0.0, 1.0),
            0.0,
        ],
    }
}

pub(crate) fn normalized_sun_direction(settings: LightingSettings) -> Vec3 {
    Vec3::from_array(settings.sun_direction).normalize_or(Vec3::Y)
}

pub(crate) fn lighting_uniform(
    settings: LightingSettings,
    render_origin: [f64; 3],
    view_direction: [f32; 3],
) -> LightingUniform {
    let sun_direction = normalized_sun_direction(settings);
    LightingUniform {
        sun_direction_intensity: [
            sun_direction.x,
            sun_direction.y,
            sun_direction.z,
            settings.sun_intensity.max(0.0),
        ],
        sun_color: [
            settings.sun_color[0].max(0.0),
            settings.sun_color[1].max(0.0),
            settings.sun_color[2].max(0.0),
            0.0,
        ],
        sky_zenith: [
            settings.sky_zenith[0].max(0.0),
            settings.sky_zenith[1].max(0.0),
            settings.sky_zenith[2].max(0.0),
            0.0,
        ],
        sky_horizon: [
            settings.sky_horizon[0].max(0.0),
            settings.sky_horizon[1].max(0.0),
            settings.sky_horizon[2].max(0.0),
            0.0,
        ],
        ground_ambient: [
            settings.ground_ambient[0].max(0.0),
            settings.ground_ambient[1].max(0.0),
            settings.ground_ambient[2].max(0.0),
            0.0,
        ],
        cascade_splits: [
            SHADOW_CASCADE_SPLITS_METERS[0],
            SHADOW_CASCADE_SPLITS_METERS[1],
            SHADOW_CASCADE_SPLITS_METERS[2],
            0.0,
        ],
        shadow_view_projection: shadow_view_projections(
            render_origin,
            view_direction,
            sun_direction,
        ),
    }
}

pub(crate) fn shadow_view_projections(
    render_origin: [f64; 3],
    view_direction: [f32; 3],
    sun_direction: Vec3,
) -> [[[f32; 4]; 4]; SHADOW_CASCADE_COUNT] {
    let origin = DVec3::from_array(render_origin);
    let view_direction = DVec3::new(
        f64::from(view_direction[0]),
        0.0,
        f64::from(view_direction[2]),
    )
    .normalize_or(DVec3::Z);
    let sun_direction = sun_direction.as_dvec3();
    let light_forward = -sun_direction;
    let provisional_up = if light_forward.y.abs() > 0.98 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let light_right = light_forward.cross(provisional_up).normalize();
    let light_up = light_right.cross(light_forward).normalize();

    std::array::from_fn(|cascade| {
        let radius = SHADOW_CASCADE_RADII_METERS[cascade];
        let desired_center = origin + (view_direction * radius * 0.35);
        let texel_size = (radius * 2.0) / f64::from(SHADOW_MAP_SIZE);
        let snapped_right = libm::round(desired_center.dot(light_right) / texel_size) * texel_size;
        let snapped_up = libm::round(desired_center.dot(light_up) / texel_size) * texel_size;
        let snapped_center = (light_right * snapped_right)
            + (light_up * snapped_up)
            + (light_forward * desired_center.dot(light_forward));
        let relative_center = snapped_center - origin;
        let eye = relative_center + (sun_direction * (SHADOW_DEPTH_METERS * 0.5));
        let view = Mat4::look_at_rh(eye.as_vec3(), relative_center.as_vec3(), light_up.as_vec3());
        let radius = f64_as_f32(radius);
        let projection = Mat4::orthographic_rh(
            -radius,
            radius,
            -radius,
            radius,
            0.0,
            f64_as_f32(SHADOW_DEPTH_METERS),
        );
        (projection * view).to_cols_array_2d()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::TimeOfDay;

    #[test]
    fn uniform_sizes_match_the_layouts_the_shaders_declare() {
        assert_eq!(std::mem::size_of::<CameraUniform>(), 160);
        assert_eq!(std::mem::size_of::<ShadowCameraUniform>(), 96);
        assert_eq!(std::mem::size_of::<LightingUniform>(), 288);
    }

    #[test]
    fn sun_directions_reach_the_shader_normalized() {
        for time in [TimeOfDay::Dawn, TimeOfDay::Noon, TimeOfDay::Dusk] {
            let uniform = lighting_uniform(
                LightingSettings::for_time_of_day(time),
                [0.0; 3],
                [0.0, 0.0, -1.0],
            );
            let direction = Vec3::from_slice(&uniform.sun_direction_intensity[..3]);
            assert!((direction.length() - 1.0).abs() < 1.0e-6);
            assert!(uniform.sun_direction_intensity[3] > 0.0);
        }
    }

    #[test]
    fn shadow_cascades_grow_outward_and_cover_their_splits() {
        assert!(
            SHADOW_CASCADE_SPLITS_METERS
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        assert!(
            SHADOW_CASCADE_RADII_METERS
                .iter()
                .zip(SHADOW_CASCADE_SPLITS_METERS)
                .all(|(&radius, split)| radius > f64::from(split))
        );
    }

    /// Cascades snap to their own texel grid, so a small camera move must not
    /// shift shadows: without this, edges crawl as the player walks.
    #[test]
    fn cascades_stay_texel_stable_under_small_camera_movement() {
        let sun = normalized_sun_direction(LightingSettings::default());
        let first_origin = [1_000_000.0, 410.0, -1_000_000.0];
        let moved_origin = [1_000_000.01, 410.0, -999_999.99];
        let direction = [0.35, -0.12, -0.93];
        let world_point = DVec3::new(1_000_012.0, 402.0, -1_000_018.0);
        let first = shadow_view_projections(first_origin, direction, sun);
        let moved = shadow_view_projections(moved_origin, direction, sun);

        for cascade in 0..SHADOW_CASCADE_COUNT {
            assert!(
                first[cascade]
                    .iter()
                    .flatten()
                    .all(|value| value.is_finite())
            );
            let projected = |matrix: [[f32; 4]; 4], origin: [f64; 3]| {
                let relative = (world_point - DVec3::from_array(origin)).as_vec3();
                let clip = Mat4::from_cols_array_2d(&matrix) * relative.extend(1.0);
                clip.truncate() / clip.w
            };
            let before = projected(first[cascade], first_origin);
            let after = projected(moved[cascade], moved_origin);
            let one_texel = 2.0 / f64_as_f32(f64::from(SHADOW_MAP_SIZE)) + 1.0e-6;
            assert!((before.x - after.x).abs() <= one_texel);
            assert!((before.y - after.y).abs() <= one_texel);
        }
    }
}
