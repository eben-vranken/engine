//! egui on top of the rendered frame, at native resolution.

pub mod theme;

use std::time::Instant;
use winit::window::Window;

pub struct Overlay {
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    /// Textures egui released last frame; freed once that frame is done.
    to_free: Vec<egui::TextureId>,
    start: Instant,
}

/// One frame's worth of UI, ready to paint.
pub struct UiFrame {
    primitives: Vec<egui::ClippedPrimitive>,
    pixels_per_point: f32,
}

impl UiFrame {
    /// Physical pixels per egui point (the display scale factor).
    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }
}

impl Overlay {
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let state = egui_winit::State::new(
            ctx,
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let renderer = egui_wgpu::Renderer::new(device, format, Default::default());
        Self {
            state,
            renderer,
            to_free: Vec::new(),
            start: Instant::now(),
        }
    }

    /// Returns true if egui used the event (e.g. a click on a button).
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// True while a text field (or other widget) takes keyboard input.
    pub fn wants_keyboard(&self) -> bool {
        self.state.egui_ctx().egui_wants_keyboard_input()
    }

    /// True while the pointer is over a UI element.
    pub fn wants_pointer(&self) -> bool {
        self.state.egui_ctx().egui_wants_pointer_input()
    }

    /// Seconds since the overlay started, for blinking cursors and the like.
    pub fn time(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }

    /// Runs the UI code and uploads any new textures (fonts etc.), so they
    /// are in place even if this frame never gets painted.
    pub fn run(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        build: impl FnMut(&mut egui::Ui),
    ) -> UiFrame {
        for id in self.to_free.drain(..) {
            self.renderer.free_texture(&id);
        }
        let input = self.state.take_egui_input(window);
        let mut output = self.state.egui_ctx().run_ui(input, build);
        self.state
            .handle_platform_output(window, output.platform_output);
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                self.renderer.update_texture(device, queue, id, &delta);
            }
        }
        self.to_free.extend(output.textures_delta.free.drain());
        let primitives = self
            .state
            .egui_ctx()
            .tessellate(output.shapes, output.pixels_per_point);
        UiFrame {
            primitives,
            pixels_per_point: output.pixels_per_point,
        }
    }

    pub fn paint(
        &mut self,
        frame: UiFrame,
        size: [u32; 2],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: frame.pixels_per_point,
        };
        let extra =
            self.renderer
                .update_buffers(device, queue, encoder, &frame.primitives, &screen);
        if !extra.is_empty() {
            queue.submit(extra);
        }
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &frame.primitives, &screen);
        }
    }
}
