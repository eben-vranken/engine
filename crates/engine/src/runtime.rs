//! Window, event loop and per-frame update. `run` opens a window and draws
//! a scene with a free-fly camera and the debug HUD.

use crate::camera::FlyCamera;
use crate::input::Input;
use crate::render::{RenderError, Renderer, scale};
use crate::scene::Scene;
use crate::ui::{Overlay, theme};
use egui::{Align2, RichText};
use glam::Vec2;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("event loop: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),
    #[error("could not create the window: {0}")]
    Window(#[from] winit::error::OsError),
    #[error(transparent)]
    Render(#[from] RenderError),
}

pub struct RunConfig {
    pub title: String,
    pub scene: Scene,
    pub camera: FlyCamera,
}

pub fn run(config: RunConfig) -> Result<(), RunError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        config,
        running: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    config: RunConfig,
    running: Option<Running>,
    error: Option<RunError>,
}

struct Running {
    window: Arc<Window>,
    renderer: Renderer,
    overlay: Overlay,
    camera: FlyCamera,
    input: Input,
    last_frame: Instant,
    fps: FpsCounter,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match Running::new(event_loop, &self.config) {
            Ok(running) => self.running = Some(running),
            Err(err) => {
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(app) = &mut self.running else {
            return;
        };
        let ui_consumed = app.overlay.on_window_event(&app.window, &event);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => app.renderer.resize(size.width, size.height),
            WindowEvent::Focused(false) => {
                app.input.release_all();
                app.set_looking(false);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if !event.state.is_pressed() {
                        app.input.key_up(code);
                    } else if !ui_consumed {
                        app.input.key_down(code);
                    }
                }
            }
            WindowEvent::MouseInput {
                button: MouseButton::Right,
                state,
                ..
            } => {
                let look = state.is_pressed() && !app.overlay.wants_pointer();
                app.set_looking(look);
            }
            WindowEvent::MouseWheel { delta, .. } if !ui_consumed => {
                app.input.scroll += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
            }
            WindowEvent::RedrawRequested => app.frame(),
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let (Some(app), DeviceEvent::MouseMotion { delta }) = (&mut self.running, event)
            && app.input.looking
        {
            app.input.mouse_delta += Vec2::new(delta.0 as f32, delta.1 as f32);
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(app) = &self.running {
            app.window.request_redraw();
        }
    }
}

impl Running {
    fn new(event_loop: &ActiveEventLoop, config: &RunConfig) -> Result<Self, RunError> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(&config.title)
                    .with_inner_size(LogicalSize::new(1600, 900)),
            )?,
        );
        let mut renderer = Renderer::new(window.clone())?;
        renderer.set_scene(&config.scene);
        let overlay = Overlay::new(&window, renderer.device(), renderer.surface_format());
        Ok(Self {
            window,
            renderer,
            overlay,
            camera: config.camera.clone(),
            input: Input::default(),
            last_frame: Instant::now(),
            fps: FpsCounter::default(),
        })
    }

    /// Mouse look grabs and hides the cursor; releasing gives it back.
    fn set_looking(&mut self, looking: bool) {
        if looking == self.input.looking {
            return;
        }
        self.input.looking = looking;
        if looking {
            // Locked is best; Windows only supports Confined.
            let _ = self
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
        }
        self.window.set_cursor_visible(!looking);
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.fps.tick(dt);

        let mut actions = HudActions::default();
        let factor = self.renderer.layout().factor;
        if self.input.pressed(KeyCode::BracketLeft) {
            actions.scale = Some(Some(factor.saturating_sub(1).max(1)));
        }
        if self.input.pressed(KeyCode::BracketRight) {
            actions.scale = Some(Some(factor + 1));
        }
        if self.input.pressed(KeyCode::Digit0) {
            actions.scale = Some(None);
        }
        if self.input.pressed(KeyCode::KeyV) {
            actions.vsync = Some(!self.renderer.vsync());
        }

        self.camera.update(&self.input, dt);

        let hud = HudInfo {
            fps: self.fps.fps,
            frame_ms: self.fps.frame_ms,
            gpu: self.renderer.adapter_name().to_uppercase(),
            output: self.renderer.output_size(),
            internal: self.renderer.layout().internal,
            factor,
            auto: self.renderer.render_scale_override().is_none(),
            vsync: self.renderer.vsync(),
            speed: self.camera.speed,
            cursor_on: self.overlay.time().fract() < 0.5,
        };
        let ui_frame = self.overlay.run(
            &self.window,
            self.renderer.device(),
            self.renderer.queue(),
            |ui| draw_hud(ui, &hud, &mut actions),
        );

        if let Some(scale) = actions.scale {
            self.renderer.set_render_scale(scale);
        }
        if let Some(vsync) = actions.vsync {
            self.renderer.set_vsync(vsync);
        }

        let size = self.renderer.output_size();
        let overlay = &mut self.overlay;
        self.renderer
            .render(&self.camera.camera, |device, queue, encoder, target| {
                overlay.paint(ui_frame, size, device, queue, encoder, target);
            });
        self.input.end_frame();
    }
}

/// FPS averaged over half-second windows so the number is readable.
#[derive(Default)]
struct FpsCounter {
    frames: u32,
    elapsed: f32,
    fps: f32,
    frame_ms: f32,
}

impl FpsCounter {
    fn tick(&mut self, dt: f32) {
        self.frames += 1;
        self.elapsed += dt;
        if self.elapsed >= 0.5 {
            self.fps = self.frames as f32 / self.elapsed;
            self.frame_ms = self.elapsed * 1000.0 / self.frames as f32;
            self.frames = 0;
            self.elapsed = 0.0;
        }
    }
}

struct HudInfo {
    fps: f32,
    frame_ms: f32,
    gpu: String,
    output: [u32; 2],
    internal: [u32; 2],
    factor: u32,
    auto: bool,
    vsync: bool,
    speed: f32,
    cursor_on: bool,
}

#[derive(Default)]
struct HudActions {
    /// `Some(None)` = back to automatic.
    scale: Option<Option<u32>>,
    vsync: Option<bool>,
}

fn draw_hud(ui: &mut egui::Ui, hud: &HudInfo, actions: &mut HudActions) {
    let dim = |text: &str| RichText::new(text).color(theme::AMBER_DIM);
    egui::Area::new(egui::Id::unique("hud"))
        .anchor(Align2::LEFT_TOP, [12.0, 12.0])
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(theme::BACKGROUND.gamma_multiply(0.85))
                .stroke(egui::Stroke::new(1.0, theme::AMBER_DIM))
                .inner_margin(10)
                .show(ui, |ui| {
                    let cursor = if hud.cursor_on { "_" } else { " " };
                    ui.label(RichText::new(format!("ENGINE // RENDER{cursor}")).strong());
                    ui.label(RichText::new(&hud.gpu).small().color(theme::AMBER_DIM));
                    ui.add_space(6.0);

                    egui::Grid::new("hud_stats")
                        .num_columns(2)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(dim("FPS"));
                            ui.label(format!("{:>5.0}  {:>5.2} MS", hud.fps, hud.frame_ms));
                            ui.end_row();

                            ui.label(dim("OUTPUT"));
                            ui.label(format!("{}×{}", hud.output[0], hud.output[1]));
                            ui.end_row();

                            ui.label(dim("INTERNAL"));
                            ui.label(format!("{}×{}", hud.internal[0], hud.internal[1]));
                            ui.end_row();

                            ui.label(dim("SCALE"));
                            ui.horizontal(|ui| {
                                if ui.small_button("-").clicked() {
                                    actions.scale = Some(Some(hud.factor.saturating_sub(1).max(1)));
                                }
                                ui.label(format!("×{}", hud.factor));
                                if ui.small_button("+").clicked() {
                                    actions.scale =
                                        Some(Some((hud.factor + 1).min(scale::MAX_FACTOR)));
                                }
                                let auto = if hud.auto { "[AUTO]" } else { " AUTO " };
                                if ui.small_button(auto).clicked() {
                                    actions.scale = Some(None);
                                }
                            });
                            ui.end_row();

                            ui.label(dim("VSYNC"));
                            let vsync = if hud.vsync { "[ON] " } else { "[OFF]" };
                            if ui.small_button(vsync).clicked() {
                                actions.vsync = Some(!hud.vsync);
                            }
                            ui.end_row();

                            ui.label(dim("CAMERA"));
                            ui.label(format!("{:.1} M/S", hud.speed));
                            ui.end_row();
                        });

                    ui.add_space(6.0);
                    for line in [
                        "RMB LOOK  WASD MOVE  Q/E DOWN/UP",
                        "SHIFT FAST  WHEEL SPEED",
                        "[ ] SCALE  0 AUTO  V VSYNC",
                    ] {
                        ui.label(RichText::new(line).small().color(theme::AMBER_DIM));
                    }
                });
        });
}
