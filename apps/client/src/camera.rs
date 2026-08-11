//! Where the player is looking from, and how they move.

use bevy::prelude::Resource;
use glam::{DVec3, Vec2};
use treeline_coordinates::WorldPosition;
use treeline_terrain::{DensityField, SurfaceField};

use crate::input::InputState;

pub const EYE_HEIGHT: f64 = 1.72;
const WALK_SPEED: f64 = 1.4;
const SPRINT_SPEED: f64 = 4.5;
/// Height the aerial view holds above the ground it passes over.
const AERIAL_HEIGHT_METERS: f64 = 200.0;
const AERIAL_SPEED_MULTIPLIER: f64 = 10.0;
/// Keeps the view from tipping past vertical in either direction.
const MAX_PITCH_RADIANS: f64 = 1.5;

/// Ground level, or a survey view for finding your way around the tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraMode {
    Ground,
    Aerial,
}

impl CameraMode {
    const fn height_above_ground(self) -> f64 {
        match self {
            Self::Ground => EYE_HEIGHT,
            Self::Aerial => AERIAL_HEIGHT_METERS,
        }
    }

    const fn speed_multiplier(self) -> f64 {
        match self {
            Self::Ground => 1.0,
            Self::Aerial => AERIAL_SPEED_MULTIPLIER,
        }
    }
}

#[derive(Clone, Copy, Debug, Resource)]
pub struct Camera {
    pub position: DVec3,
    yaw: f64,
    pitch: f64,
    mode: CameraMode,
}

impl Camera {
    pub const fn new(position: DVec3, yaw: f64, pitch: f64) -> Self {
        Self {
            position,
            yaw,
            pitch,
            mode: CameraMode::Ground,
        }
    }

    pub fn world_position(self) -> WorldPosition {
        WorldPosition::new(self.position.x, self.position.y, self.position.z)
    }

    pub fn direction(self) -> DVec3 {
        let pitch_cosine = libm::cos(self.pitch);
        DVec3::new(
            libm::cos(self.yaw) * pitch_cosine,
            libm::sin(self.pitch),
            libm::sin(self.yaw) * pitch_cosine,
        )
        .normalize()
    }

    pub fn look(&mut self, delta_x: f64, delta_y: f64) {
        const SENSITIVITY: f64 = 0.002;
        self.yaw += delta_x * SENSITIVITY;
        self.pitch =
            (self.pitch - (delta_y * SENSITIVITY)).clamp(-MAX_PITCH_RADIANS, MAX_PITCH_RADIANS);
    }

    pub fn look_with_stick(&mut self, axis: Vec2, delta_seconds: f64) {
        const HORIZONTAL_SPEED: f64 = 2.4;
        const VERTICAL_SPEED: f64 = 1.8;
        self.yaw += f64::from(axis.x) * HORIZONTAL_SPEED * delta_seconds;
        self.pitch = (self.pitch + (f64::from(axis.y) * VERTICAL_SPEED * delta_seconds))
            .clamp(-MAX_PITCH_RADIANS, MAX_PITCH_RADIANS);
    }

    /// Turns to face a horizontal target, used after a warp.
    pub fn face(&mut self, target: [f64; 2]) {
        let delta_x = target[0] - self.position.x;
        let delta_z = target[1] - self.position.z;
        if delta_x != 0.0 || delta_z != 0.0 {
            self.yaw = libm::atan2(delta_z, delta_x);
            self.pitch = -0.08;
        }
    }

    /// Switches between ground and aerial view, returning whether it is aerial.
    pub fn toggle_aerial_mode(&mut self, terrain: &impl SurfaceField) -> bool {
        self.mode = match self.mode {
            CameraMode::Ground => CameraMode::Aerial,
            CameraMode::Aerial => CameraMode::Ground,
        };
        self.position.y = self.height_over(terrain, self.position.x, self.position.z);
        self.mode == CameraMode::Aerial
    }

    /// The height this camera would sit at over a horizontal position.
    pub fn height_over(self, terrain: &impl SurfaceField, x: f64, z: f64) -> f64 {
        surface_height(terrain, x, z) + self.mode.height_above_ground()
    }

    /// Advances one frame of movement, following the ground when on foot.
    ///
    /// A step that lands somewhere without a walkable floor — a cliff face, or
    /// water — is undone rather than allowed to clip through terrain.
    pub fn walk<T>(&mut self, input: &InputState, terrain: &T, delta_seconds: f64)
    where
        T: DensityField + SurfaceField,
    {
        let previous = self.position;
        let movement = self.movement(input);
        if movement.length_squared() > 0.0 {
            let base_speed = if input.sprint() {
                SPRINT_SPEED
            } else {
                WALK_SPEED
            };
            let intensity = movement.length().min(1.0);
            self.position += movement.normalize()
                * intensity
                * base_speed
                * self.mode.speed_multiplier()
                * delta_seconds;
        }

        if self.mode == CameraMode::Aerial {
            self.position.y = self.height_over(terrain, self.position.x, self.position.z);
            return;
        }
        self.position.y = self.follow_ground(terrain, previous) + EYE_HEIGHT;
    }

    /// Resolves the floor under the camera, backing out of an illegal step.
    fn follow_ground<T>(&mut self, terrain: &T, previous: DVec3) -> f64
    where
        T: DensityField + SurfaceField,
    {
        let current_floor = self.position.y - EYE_HEIGHT;
        if let Some(floor) =
            walkable_floor_height(terrain, self.position.x, self.position.z, current_floor)
        {
            return floor;
        }
        self.position.x = previous.x;
        self.position.z = previous.z;
        walkable_floor_height(
            terrain,
            self.position.x,
            self.position.z,
            previous.y - EYE_HEIGHT,
        )
        .unwrap_or_else(|| surface_height(terrain, self.position.x, self.position.z))
    }

    fn movement(self, input: &InputState) -> DVec3 {
        let forward = DVec3::new(libm::cos(self.yaw), 0.0, libm::sin(self.yaw));
        let right = forward.cross(DVec3::Y);
        (forward * f64::from(input.forward_axis())) + (right * f64::from(input.right_axis()))
    }

    /// The horizontal heading the player is moving in, for terrain prefetching.
    pub fn travel_direction(self, input: &InputState) -> [f64; 2] {
        let movement = self.movement(input);
        if movement.length_squared() <= f64::EPSILON {
            return [0.0, 0.0];
        }
        let direction = movement.normalize();
        [direction.x, direction.z]
    }
}

/// Surface elevation at a position the player can actually be at.
///
/// # Panics
///
/// Panics for a non-finite position, which would mean movement itself is
/// broken rather than the terrain.
pub fn surface_height(terrain: &impl SurfaceField, x: f64, z: f64) -> f64 {
    terrain
        .surface_height(x, z)
        .expect("finite player positions must have terrain")
}

/// Finds the floor the player can stand on at a horizontal position.
///
/// Scans downward from just above the current floor for the first air-to-solid
/// crossing with headroom, then refines it. Returning `None` means there is
/// nowhere legal to stand — too high a step up, or no headroom.
pub fn walkable_floor_height(
    terrain: &impl DensityField,
    x: f64,
    z: f64,
    current_floor: f64,
) -> Option<f64> {
    const MAX_STEP_UP_METERS: f64 = 1.5;
    const MAX_DESCENT_METERS: f64 = 160.0;
    const SCAN_STEP_METERS: f64 = 0.5;
    const REFINEMENT_STEPS: usize = 9;

    if !x.is_finite() || !z.is_finite() || !current_floor.is_finite() {
        return None;
    }
    let mut air_y = current_floor + MAX_STEP_UP_METERS;
    let mut air_density = terrain.sample(WorldPosition::new(x, air_y, z)).density;
    let minimum_y = current_floor - MAX_DESCENT_METERS;
    let mut sample_y = air_y - SCAN_STEP_METERS;
    while sample_y >= minimum_y {
        let density = terrain.sample(WorldPosition::new(x, sample_y, z)).density;
        if air_density > 0.0 && density <= 0.0 {
            let mut solid_y = sample_y;
            for _ in 0..REFINEMENT_STEPS {
                let midpoint = (solid_y + air_y) * 0.5;
                if terrain.sample(WorldPosition::new(x, midpoint, z)).density <= 0.0 {
                    solid_y = midpoint;
                } else {
                    air_y = midpoint;
                }
            }
            let floor = (solid_y + air_y) * 0.5;
            let has_headroom = terrain
                .sample(WorldPosition::new(x, floor + EYE_HEIGHT, z))
                .density
                > 0.0;
            if has_headroom {
                return Some(floor);
            }
        }
        air_y = sample_y;
        air_density = density;
        sample_y -= SCAN_STEP_METERS;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::keyboard::KeyCode;
    use treeline_terrain::{Material, TerrainSample};

    #[derive(Clone, Copy, Debug)]
    struct SlopedGround;

    impl SlopedGround {
        fn height_at(x: f64, z: f64) -> f64 {
            (x * 0.25) - (z * 0.125)
        }
    }

    impl DensityField for SlopedGround {
        fn sample(&self, position: WorldPosition) -> TerrainSample {
            let density = position.y - Self::height_at(position.x, position.z);
            TerrainSample::new(
                density,
                if density > 0.0 {
                    Material::Air
                } else {
                    Material::Rock
                },
            )
        }
    }

    impl SurfaceField for SlopedGround {
        fn surface_height(&self, x: f64, z: f64) -> Option<f64> {
            (x.is_finite() && z.is_finite()).then(|| Self::height_at(x, z))
        }
    }

    /// A sphere of air below a flat surface, reachable through an open mouth.
    #[derive(Clone, Copy, Debug)]
    struct OpenCave;

    impl DensityField for OpenCave {
        fn sample(&self, position: WorldPosition) -> TerrainSample {
            let void = 5.5 - libm::hypot(libm::hypot(position.x, position.y + 5.0), position.z);
            let density = position.y.max(void);
            TerrainSample::new(
                density,
                if density > 0.0 {
                    Material::Air
                } else {
                    Material::Rock
                },
            )
        }
    }

    #[test]
    fn walking_follows_the_ground_at_a_steady_speed() {
        let mut input = InputState::default();
        input.set_key(KeyCode::KeyW, true);
        let mut camera = Camera::new(DVec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);

        camera.walk(&input, &SlopedGround, 1.0 / 60.0);

        assert!((camera.position.x - WALK_SPEED / 60.0).abs() < 1.0e-9);
        // The floor comes from a bisection search, so it is exact only to
        // the scan's refinement resolution.
        assert!(
            (camera.position.y - SlopedGround::height_at(camera.position.x, 0.0) - EYE_HEIGHT)
                .abs()
                < 0.01
        );
    }

    #[test]
    fn sprinting_is_faster_than_walking() {
        let mut walk = InputState::default();
        walk.set_key(KeyCode::KeyW, true);
        let mut sprint = InputState::default();
        sprint.set_key(KeyCode::KeyW, true);
        sprint.set_key(KeyCode::ShiftLeft, true);

        let mut walker = Camera::new(DVec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);
        let mut sprinter = walker;
        walker.walk(&walk, &SlopedGround, 0.1);
        sprinter.walk(&sprint, &SlopedGround, 0.1);

        assert!(sprinter.position.x > walker.position.x * 2.0);
    }

    #[test]
    fn aerial_mode_holds_a_fixed_clearance_and_keeps_the_view_direction() {
        let (x, z, yaw, pitch) = (24.0, -12.0, 0.75, -0.2);
        let surface = SlopedGround::height_at(x, z);
        let mut camera = Camera::new(DVec3::new(x, surface + EYE_HEIGHT, z), yaw, pitch);

        assert!(camera.toggle_aerial_mode(&SlopedGround));
        assert_eq!(camera.position, DVec3::new(x, surface + 200.0, z));

        assert!(!camera.toggle_aerial_mode(&SlopedGround));
        assert_eq!(camera.position, DVec3::new(x, surface + EYE_HEIGHT, z));
        assert!((camera.yaw - yaw).abs() < f64::EPSILON);
        assert!((camera.pitch - pitch).abs() < f64::EPSILON);
    }

    #[test]
    fn destination_height_follows_the_current_mode() {
        let mut camera = Camera::new(DVec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);
        let surface = SlopedGround::height_at(80.0, -40.0);

        assert!(
            (camera.height_over(&SlopedGround, 80.0, -40.0) - surface - EYE_HEIGHT).abs() < 1.0e-9
        );
        camera.toggle_aerial_mode(&SlopedGround);
        assert!((camera.height_over(&SlopedGround, 80.0, -40.0) - surface - 200.0).abs() < 1.0e-9);
    }

    #[test]
    fn pitch_cannot_tip_past_vertical() {
        let mut camera = Camera::new(DVec3::ZERO, 0.0, 0.0);
        camera.look(0.0, -10_000.0);
        assert!(camera.pitch <= MAX_PITCH_RADIANS);
        camera.look(0.0, 20_000.0);
        assert!(camera.pitch >= -MAX_PITCH_RADIANS);
    }

    #[test]
    fn facing_a_target_points_the_camera_at_it() {
        let mut camera = Camera::new(DVec3::new(0.0, 0.0, 0.0), 0.0, 0.0);
        camera.face([0.0, 10.0]);
        let direction = camera.direction();

        assert!(direction.z > 0.9);
        assert!(direction.x.abs() < 0.1);
    }

    #[test]
    fn a_walkable_floor_is_found_through_an_open_cave_mouth() {
        let floor = walkable_floor_height(&OpenCave, 0.0, 0.0, 0.0).expect("cave floor");
        assert!((floor + 10.5).abs() < 0.01);
    }

    #[test]
    fn solid_rock_has_no_walkable_floor() {
        assert!(walkable_floor_height(&OpenCave, 8.0, 0.0, -10.5).is_none());
    }

    #[test]
    fn non_finite_positions_have_no_floor() {
        assert!(walkable_floor_height(&SlopedGround, f64::NAN, 0.0, 0.0).is_none());
        assert!(walkable_floor_height(&SlopedGround, 0.0, 0.0, f64::INFINITY).is_none());
    }
}
