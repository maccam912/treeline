//! Generator Lab — a top-down inspector for the surveyed world.
//!
//! The lab exists to answer one question quickly: what does the world actually
//! say at this position? It draws one layer at a time as a map, and reports
//! every layer at whatever you click.
//!
//! It samples the same [`treeline_world`] API the game does. If the lab shows
//! it, the game sees it.

mod inspect;
mod lab;
mod map;
mod ui;
mod view;

use std::error::Error;
use std::sync::Arc;

use lab::Lab;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Scroll steps zoom by the same factor the keyboard does.
const SCROLL_ZOOM_IN: f64 = 0.7;
const SCROLL_ZOOM_OUT: f64 = 1.4;

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    // The lab is idle between inputs, so it waits rather than polling.
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut LabApp::default())?;
    Ok(())
}

#[derive(Default)]
struct LabApp {
    lab: Option<Lab>,
}

impl ApplicationHandler for LabApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.lab.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Treeline Generator Lab")
            .with_inner_size(LogicalSize::new(1_200, 800));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create the Generator Lab window: {error}");
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(Lab::new(window)) {
            Ok(lab) => self.lab = Some(lab),
            Err(error) => {
                eprintln!("failed to start the Generator Lab: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(lab) = self.lab.as_mut() else {
            return;
        };
        if window_id != lab.window().id() {
            return;
        }
        // The UI sees every event first and may claim it.
        let consumed = lab.handle_ui_event(&event);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                report(lab.resize(size.width, size.height), "resize");
            }
            WindowEvent::RedrawRequested => match lab.render() {
                Ok(()) | Err(wgpu::SurfaceError::Timeout) => {}
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    lab.reconfigure_surface();
                }
                Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                Err(error) => eprintln!("Generator Lab render failed: {error}"),
            },
            WindowEvent::CursorMoved { position, .. } => lab.set_cursor(position),
            WindowEvent::MouseWheel { delta, .. } if !consumed => {
                let direction = match delta {
                    MouseScrollDelta::LineDelta(_, vertical) => f64::from(vertical),
                    MouseScrollDelta::PixelDelta(position) => position.y.signum(),
                };
                if direction != 0.0 {
                    let factor = if direction > 0.0 {
                        SCROLL_ZOOM_IN
                    } else {
                        SCROLL_ZOOM_OUT
                    };
                    report(lab.zoom_by(factor), "zoom");
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if !consumed => match button {
                MouseButton::Left => lab.inspect_cursor(),
                MouseButton::Right => report(lab.recenter_on_cursor(), "recenter"),
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape {
                        event_loop.exit();
                    } else if !consumed {
                        report(lab.handle_key(code), "update the view");
                    }
                }
            }
            _ => {}
        }
    }
}

/// Reports a failed command without taking the lab down with it.
fn report(result: Result<(), Box<dyn Error>>, action: &str) {
    if let Err(error) = result {
        eprintln!("failed to {action}: {error}");
    }
}
