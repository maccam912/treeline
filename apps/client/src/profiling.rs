//! Lightweight frame diagnostics and the entry point for full trace captures.
//!
//! The overlay stays hidden until F3 is pressed. Building the client with its
//! `profiling` feature additionally sends Bevy's schedule, render, and custom
//! application spans to Tracy.

use std::time::Duration;

use bevy::dev_tools::fps_overlay::{
    FpsOverlayConfig, FpsOverlayPlugin, FpsOverlaySystems, FrameTimeGraphConfig,
};
use bevy::prelude::*;
use bevy::text::TextFont;

const TOGGLE_KEY: KeyCode = KeyCode::F3;

/// Installs an on-demand frame-time graph with negligible cost while hidden.
#[derive(Debug, Default)]
pub struct FrameProfilerPlugin;

impl Plugin for FrameProfilerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FpsOverlayPlugin {
            config: overlay_config(),
        })
        .add_systems(Update, toggle_overlay.before(FpsOverlaySystems::Customize));
    }
}

fn overlay_config() -> FpsOverlayConfig {
    FpsOverlayConfig {
        text_config: TextFont::from_font_size(18.0),
        text_color: Color::srgb(0.92, 0.96, 0.90),
        enabled: false,
        refresh_interval: Duration::from_millis(100),
        frame_time_graph_config: FrameTimeGraphConfig {
            enabled: false,
            min_fps: 30.0,
            target_fps: 60.0,
        },
    }
}

fn toggle_overlay(keys: Res<ButtonInput<KeyCode>>, mut overlay: ResMut<FpsOverlayConfig>) {
    if keys.just_pressed(TOGGLE_KEY) {
        let enabled = !overlay.enabled;
        overlay.enabled = enabled;
        overlay.frame_time_graph_config.enabled = enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_starts_fully_hidden() {
        let config = overlay_config();

        assert!(!config.enabled);
        assert!(!config.frame_time_graph_config.enabled);
        assert!((config.frame_time_graph_config.min_fps - 30.0).abs() < f32::EPSILON);
        assert!((config.frame_time_graph_config.target_fps - 60.0).abs() < f32::EPSILON);
    }
}
