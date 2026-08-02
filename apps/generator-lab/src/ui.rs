//! The lab's overlay, and the camera that makes the map read as a map.

use std::sync::Arc;

use glam::Mat4;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::inspect::Inspection;
use crate::map::MapView;
use crate::view::ViewMode;

/// Builds a top-down orthographic view of a horizontal span.
///
/// Orthographic rather than perspective: a map has no vanishing point, and a
/// cell's size on screen should not depend on where it sits in the view.
/// Reverse-Z matches what the terrain pipeline expects.
pub fn top_down_view_projection(span_meters: f64, width: u32, height: u32) -> [[f32; 4]; 4] {
    let aspect = f64::from(width.max(1)) / f64::from(height.max(1));
    let half_width = f64_as_f32(span_meters * 0.5);
    let half_height = f64_as_f32(span_meters * 0.5 / aspect);
    let projection = Mat4::orthographic_rh(
        -half_width,
        half_width,
        -half_height,
        half_height,
        4_000.0,
        1.0,
    );
    // The camera sits above the plane looking down; world Z increases south, so
    // "up" on screen is negative Z.
    let view = Mat4::look_to_rh(glam::Vec3::ZERO, -glam::Vec3::Y, -glam::Vec3::Z);
    (projection * view).to_cols_array_2d()
}

/// The egui overlay.
pub struct Egui {
    context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
}

impl Egui {
    pub fn new(window: &Arc<Window>, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let context = egui::Context::default();
        let state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(f64_as_f32(window.scale_factor())),
            window.theme(),
            usize::try_from(device.limits().max_texture_dimension_2d).ok(),
        );
        Self {
            context,
            state,
            renderer: egui_wgpu::Renderer::new(device, format, None, 1, false),
        }
    }

    /// Returns whether the UI consumed the event, and whether to repaint.
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> (bool, bool) {
        let response = self.state.on_window_event(window, event);
        (response.consumed, response.repaint)
    }

    /// Draws the overlay into the frame.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        surface_config: &wgpu::SurfaceConfiguration,
        view: MapView,
        inspection: Option<&Inspection>,
    ) {
        let input = self.state.take_egui_input(window);
        let output = self
            .context
            .run(input, |context| draw_panel(context, view, inspection));
        self.state
            .handle_platform_output(window, output.platform_output);

        let primitives = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        for (id, delta) in &output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [surface_config.width, surface_config.height],
            pixels_per_point: output.pixels_per_point,
        };
        self.renderer
            .update_buffers(device, queue, encoder, &primitives, &descriptor);

        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Generator Lab UI pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            })
            .forget_lifetime();
        self.renderer.render(&mut pass, &primitives, &descriptor);
        drop(pass);

        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}

/// The side panel: which layer is shown, where, and what is under the cursor.
fn draw_panel(context: &egui::Context, view: MapView, inspection: Option<&Inspection>) {
    egui::SidePanel::left("layers")
        .default_width(340.0)
        .show(context, |ui| {
            ui.heading(view.mode.label());
            ui.label(view.mode.description());
            ui.separator();

            ui.label(format!(
                "center  {:.0}, {:.0}    span  {:.0} m    season  {}",
                view.center[0],
                view.center[1],
                view.span_meters,
                view.season.label()
            ));
            ui.separator();

            ui.label("layers");
            for (index, mode) in ViewMode::ALL.into_iter().enumerate() {
                let marker = if mode == view.mode { "▸" } else { " " };
                ui.monospace(format!("{marker} {}  {}", index + 1, mode.label()));
            }
            ui.separator();

            ui.label("keys");
            for line in [
                "WASD / arrows   pan",
                "+ / -           zoom",
                "C               cycle season",
                "left click      inspect",
                "right click     recenter",
                "Escape          quit",
            ] {
                ui.monospace(line);
            }
            ui.separator();

            match inspection {
                Some(inspection) => {
                    ui.label(format!(
                        "inspection at {:.1}, {:.1}",
                        inspection.position[0], inspection.position[1]
                    ));
                    for line in &inspection.lines {
                        ui.monospace(line);
                    }
                }
                None => {
                    ui.label("click the map to inspect a position");
                }
            }
        });
}

#[allow(clippy::cast_possible_truncation)]
fn f64_as_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map has no vanishing point: equal spans must project to equal sizes.
    #[test]
    fn the_map_projection_is_uniform_across_the_view() {
        let matrix = Mat4::from_cols_array_2d(&top_down_view_projection(1_000.0, 800, 800));
        let project = |x: f32, z: f32| {
            let clip = matrix * glam::Vec4::new(x, 0.0, z, 1.0);
            clip.truncate() / clip.w
        };
        let near_step = project(100.0, 0.0).x - project(0.0, 0.0).x;
        let far_step = project(400.0, 0.0).x - project(300.0, 0.0).x;

        assert!((near_step - far_step).abs() < 1.0e-5);
    }

    #[test]
    fn the_span_fills_the_viewport_width() {
        let matrix = Mat4::from_cols_array_2d(&top_down_view_projection(1_000.0, 800, 800));
        let edge = matrix * glam::Vec4::new(500.0, 0.0, 0.0, 1.0);
        assert!((edge.x / edge.w - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn north_is_up_on_screen() {
        let matrix = Mat4::from_cols_array_2d(&top_down_view_projection(1_000.0, 800, 800));
        let north = matrix * glam::Vec4::new(0.0, 0.0, -100.0, 1.0);
        let south = matrix * glam::Vec4::new(0.0, 0.0, 100.0, 1.0);
        assert!(north.y / north.w > south.y / south.w);
    }

    #[test]
    fn a_wider_viewport_keeps_cells_square() {
        let matrix = Mat4::from_cols_array_2d(&top_down_view_projection(1_000.0, 1_600, 800));
        let across = matrix * glam::Vec4::new(500.0, 0.0, 0.0, 1.0);
        let down = matrix * glam::Vec4::new(0.0, 0.0, 250.0, 1.0);

        assert!((across.x / across.w - 1.0).abs() < 1.0e-5);
        assert!((down.y / down.w + 1.0).abs() < 1.0e-5);
    }
}
