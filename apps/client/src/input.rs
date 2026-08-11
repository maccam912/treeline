//! Keyboard, mouse, and touch, reduced to movement and look axes.
//!
//! The game only ever asks for axes, never for devices, so a touch drag and a
//! key press reach movement through the same path.

use std::collections::HashSet;

use bevy::input::keyboard::KeyCode;
use bevy::prelude::Resource;
use glam::Vec2;

#[derive(Debug, Default, Resource)]
pub struct InputState {
    pressed: HashSet<KeyCode>,
    sticks: VirtualSticks,
}

impl InputState {
    pub fn set_key(&mut self, code: KeyCode, pressed: bool) {
        if pressed {
            self.pressed.insert(code);
        } else {
            self.pressed.remove(&code);
        }
    }

    pub fn forward_axis(&self) -> f32 {
        (self.key_axis(
            [KeyCode::KeyW, KeyCode::ArrowUp],
            [KeyCode::KeyS, KeyCode::ArrowDown],
        ) + self.sticks.movement_axis().y)
            .clamp(-1.0, 1.0)
    }

    pub fn right_axis(&self) -> f32 {
        (self.key_axis(
            [KeyCode::KeyD, KeyCode::ArrowRight],
            [KeyCode::KeyA, KeyCode::ArrowLeft],
        ) + self.sticks.movement_axis().x)
            .clamp(-1.0, 1.0)
    }

    pub fn look_axis(&self) -> Vec2 {
        self.sticks.look_axis()
    }

    /// Sprinting is either shift, or pushing the movement stick to its edge.
    pub fn sprint(&self) -> bool {
        self.is_down(KeyCode::ShiftLeft)
            || self.is_down(KeyCode::ShiftRight)
            || self.sticks.movement_axis().length() > 0.85
    }

    pub fn clear(&mut self) {
        self.pressed.clear();
        self.sticks.clear();
    }

    pub fn begin_touch(&mut self, id: u64, position: Vec2, viewport_width: f32, radius: f32) {
        self.sticks.begin(id, position, viewport_width);
        self.sticks.set_radius(radius);
    }

    pub fn move_touch(&mut self, id: u64, position: Vec2) {
        self.sticks.update(id, position);
    }

    pub fn end_touch(&mut self, id: u64) {
        self.sticks.end(id);
    }

    fn key_axis(&self, positive: [KeyCode; 2], negative: [KeyCode; 2]) -> f32 {
        let held =
            |keys: [KeyCode; 2]| f32::from(u8::from(keys.iter().any(|&key| self.is_down(key))));
        held(positive) - held(negative)
    }

    fn is_down(&self, code: KeyCode) -> bool {
        self.pressed.contains(&code)
    }
}

/// One finger held down, tracked from where it first touched.
#[derive(Clone, Copy, Debug)]
struct StickTouch {
    id: u64,
    origin: Vec2,
    current: Vec2,
}

impl StickTouch {
    const fn new(id: u64, position: Vec2) -> Self {
        Self {
            id,
            origin: position,
            current: position,
        }
    }

    /// Displacement from the touch origin, as a unit-clamped axis.
    fn axis(self, radius: f32) -> Vec2 {
        Vec2::new(
            self.current.x - self.origin.x,
            self.origin.y - self.current.y,
        )
        .clamp_length_max(radius.max(1.0))
            / radius.max(1.0)
    }
}

/// Two thumbsticks: the left half of the screen moves, the right half looks.
#[derive(Debug)]
struct VirtualSticks {
    movement: Option<StickTouch>,
    look: Option<StickTouch>,
    radius: f32,
}

impl Default for VirtualSticks {
    fn default() -> Self {
        Self {
            movement: None,
            look: None,
            radius: 64.0,
        }
    }
}

impl VirtualSticks {
    /// Claims a stick for a new touch, ignoring a second finger on the same half.
    fn begin(&mut self, id: u64, position: Vec2, viewport_width: f32) {
        let target = if position.x < viewport_width * 0.5 {
            &mut self.movement
        } else {
            &mut self.look
        };
        if target.is_none() {
            *target = Some(StickTouch::new(id, position));
        }
    }

    fn update(&mut self, id: u64, position: Vec2) {
        for stick in [&mut self.movement, &mut self.look].into_iter().flatten() {
            if stick.id == id {
                stick.current = position;
                break;
            }
        }
    }

    fn end(&mut self, id: u64) {
        if self.movement.is_some_and(|stick| stick.id == id) {
            self.movement = None;
        }
        if self.look.is_some_and(|stick| stick.id == id) {
            self.look = None;
        }
    }

    fn set_radius(&mut self, radius: f32) {
        self.radius = radius.max(1.0);
    }

    fn movement_axis(&self) -> Vec2 {
        self.movement
            .map_or(Vec2::ZERO, |stick| stick.axis(self.radius))
    }

    fn look_axis(&self) -> Vec2 {
        self.look
            .map_or(Vec2::ZERO, |stick| stick.axis(self.radius))
    }

    fn clear(&mut self) {
        self.movement = None;
        self.look = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_keys_cancel_and_arrows_match_the_letters() {
        let mut input = InputState::default();
        input.set_key(KeyCode::KeyW, true);
        assert!((input.forward_axis() - 1.0).abs() < f32::EPSILON);

        input.set_key(KeyCode::KeyS, true);
        assert!(input.forward_axis().abs() < f32::EPSILON);

        input.clear();
        input.set_key(KeyCode::ArrowUp, true);
        assert!((input.forward_axis() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn releasing_a_key_stops_the_movement() {
        let mut input = InputState::default();
        input.set_key(KeyCode::KeyD, true);
        input.set_key(KeyCode::KeyD, false);
        assert!(input.right_axis().abs() < f32::EPSILON);
    }

    #[test]
    fn the_left_half_of_the_screen_moves_and_the_right_half_looks() {
        let mut input = InputState::default();
        input.begin_touch(1, Vec2::new(50.0, 300.0), 800.0, 64.0);
        input.begin_touch(2, Vec2::new(600.0, 300.0), 800.0, 64.0);
        input.move_touch(1, Vec2::new(50.0, 236.0));
        input.move_touch(2, Vec2::new(664.0, 300.0));

        assert!((input.forward_axis() - 1.0).abs() < f32::EPSILON);
        assert!((input.look_axis().x - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_stick_axis_never_exceeds_one() {
        let mut input = InputState::default();
        input.begin_touch(1, Vec2::new(50.0, 300.0), 800.0, 64.0);
        input.move_touch(1, Vec2::new(50.0, -10_000.0));

        assert!(input.forward_axis() <= 1.0);
        assert!(input.look_axis().length() <= 1.0);
    }

    #[test]
    fn a_fully_pushed_movement_stick_sprints() {
        let mut input = InputState::default();
        assert!(!input.sprint());
        input.begin_touch(1, Vec2::new(50.0, 300.0), 800.0, 64.0);
        input.move_touch(1, Vec2::new(50.0, 200.0));
        assert!(input.sprint());
    }

    #[test]
    fn ending_a_touch_releases_only_its_own_stick() {
        let mut input = InputState::default();
        input.begin_touch(1, Vec2::new(50.0, 300.0), 800.0, 64.0);
        input.begin_touch(2, Vec2::new(600.0, 300.0), 800.0, 64.0);
        input.move_touch(1, Vec2::new(50.0, 236.0));
        input.move_touch(2, Vec2::new(664.0, 300.0));
        input.end_touch(1);

        assert!(input.forward_axis().abs() < f32::EPSILON);
        assert!((input.look_axis().x - 1.0).abs() < f32::EPSILON);
    }
}
