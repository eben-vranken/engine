//! The launcher: a small window that asks which project to open, listing
//! the ones opened before (most recent first), with OPEN PROJECT… and NEW
//! PROJECT… for others. The chosen project opens in a new editor process
//! (as switching projects in the editor does) and the launcher closes.

use crate::project;
use crate::recent::{self, RecentProject};
use crate::render::RenderError;
use crate::render::quality::Quality;
use crate::runtime::RunError;
use crate::ui::{Overlay, theme};
use egui::{Align, Layout, RichText, Ui};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// What the editor it opens is started with.
pub struct LaunchConfig {
    pub quality: Quality,
    pub perf_log: bool,
}

pub fn run(config: LaunchConfig) -> Result<(), RunError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = Launcher {
        config,
        window: None,
        recent: recent::load(),
        message: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct Launcher {
    config: LaunchConfig,
    window: Option<LauncherWindow>,
    recent: Vec<RecentProject>,
    /// The last problem, shown under the buttons.
    message: Option<String>,
    error: Option<RunError>,
}

/// What a click asked for (done after the UI, as the file dialogs block).
enum Action {
    Open(PathBuf),
    Forget(PathBuf),
    Browse,
    New,
}

struct LauncherWindow {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    overlay: Overlay,
}

impl LauncherWindow {
    fn new(event_loop: &ActiveEventLoop) -> Result<Self, RunError> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(format!("{} — open a project", crate::NAME))
                    .with_inner_size(LogicalSize::new(640, 460))
                    .with_min_inner_size(LogicalSize::new(420, 300)),
            )?,
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(window.clone()),
        ));
        let surface = instance
            .create_surface(window.clone())
            .map_err(RenderError::from)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            // A few buttons: the integrated GPU is plenty.
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(RenderError::from)?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(RenderError::from)?;
        // egui writes display-encoded colors, as in the editor.
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or(RenderError::NoSurfaceFormat)?;
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let overlay = Overlay::new(&window, &device, format);
        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            overlay,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Draws `ui` on the background color.
    fn frame(&mut self, ui: impl FnMut(&mut Ui)) {
        let frame = self
            .overlay
            .run(&self.window, &self.device, &self.queue, ui);
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = texture.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let [r, g, b, _] = theme::BACKGROUND.to_normalized_gamma_f32();
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("launcher clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: r as f64,
                        g: g as f64,
                        b: b as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let size = [self.config.width, self.config.height];
        self.overlay
            .paint(frame, size, &self.device, &self.queue, &mut encoder, &view);
        self.queue.submit([encoder.finish()]);
        self.queue.present(texture);
    }
}

impl ApplicationHandler for Launcher {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match LauncherWindow::new(event_loop) {
            Ok(window) => self.window = Some(window),
            Err(err) => {
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(window) = &mut self.window else {
            return;
        };
        let _ = window.overlay.on_window_event(&window.window, &event);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => window.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                let mut action = None;
                let (recent, message) = (&self.recent, &self.message);
                window.frame(|ui| action = list_ui(ui, recent, message.as_deref()));
                if let Some(action) = action {
                    self.act(action, event_loop);
                }
            }
            _ => {}
        }
        // Redraw after every event: hovers and clicks show at once, and
        // nothing is drawn while nothing happens.
        if let Some(window) = &self.window {
            window.window.request_redraw();
        }
    }
}

impl Launcher {
    fn act(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        let start = self.start_dir();
        let chosen = match action {
            Action::Open(dir) => Some(Ok(dir)),
            Action::Forget(dir) => {
                recent::forget(&dir);
                self.recent = recent::load();
                None
            }
            Action::Browse => rfd::FileDialog::new()
                .set_title("Open project: pick its game.ron")
                .set_directory(&start)
                .add_filter("Game project", &["ron"])
                .pick_file()
                .map(|file| project::find(&file).map_err(|e| e.to_string())),
            Action::New => rfd::FileDialog::new()
                .set_title("New project: name the game's folder")
                .set_directory(&start)
                .set_file_name("new game")
                .save_file()
                .map(|dir| {
                    project::create(&dir)
                        .map(|_| dir)
                        .map_err(|e| e.to_string())
                }),
        };
        match chosen {
            Some(Ok(dir)) if !dir.join("game.ron").is_file() => {
                self.message = Some(format!("NO GAME.RON IN {}", dir.display()));
            }
            Some(Ok(dir)) => {
                match crate::runtime::start_editor(&dir, self.config.quality, self.config.perf_log)
                {
                    Ok(()) => event_loop.exit(),
                    Err(err) => self.message = Some(format!("COULD NOT START THE EDITOR: {err}")),
                }
            }
            Some(Err(err)) => self.message = Some(err.to_uppercase()),
            None => {}
        }
    }

    /// Where the file dialogs start: next to the latest project.
    fn start_dir(&self) -> PathBuf {
        self.recent
            .first()
            .and_then(|p| p.dir.parent())
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_default()
    }
}

/// The window's contents. Returns what was clicked.
fn list_ui(ui: &mut Ui, recent: &[RecentProject], message: Option<&str>) -> Option<Action> {
    let mut action = None;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().inner_margin(20.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("{}_", crate::NAME.to_uppercase()))
                    .heading()
                    .strong(),
            );
            ui.label(
                RichText::new(format!("V{}", crate::VERSION))
                    .small()
                    .color(theme::AMBER_DIM),
            );
            ui.add_space(16.0);

            egui::Panel::bottom("launcher_buttons")
                .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 8)))
                .show(ui, |ui| {
                    if let Some(message) = message {
                        ui.label(RichText::new(message).color(theme::ALERT));
                        ui.add_space(6.0);
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .button("OPEN PROJECT…")
                            .on_hover_text("ANOTHER GAME: PICK ITS GAME.RON")
                            .clicked()
                        {
                            action = Some(Action::Browse);
                        }
                        if ui
                            .button("NEW PROJECT…")
                            .on_hover_text(
                                "A NEW, EMPTY GAME: PICK WHERE ITS FOLDER GOES AND NAME IT",
                            )
                            .clicked()
                        {
                            action = Some(Action::New);
                        }
                    });
                });

            ui.label(RichText::new("RECENT PROJECTS").color(theme::AMBER_DIM));
            ui.separator();
            if recent.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "NONE YET. OPEN OR MAKE A PROJECT BELOW; NEXT TIME IT'S LISTED HERE.",
                    )
                    .color(theme::AMBER_DIM),
                );
            }
            let now = recent::now();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for project in recent {
                        if let Some(clicked) = row(ui, project, now) {
                            action = Some(clicked);
                        }
                    }
                });
        });
    action
}

/// One recent project: click to open it, X to take it off the list.
fn row(ui: &mut Ui, project: &RecentProject, now: u64) -> Option<Action> {
    /// The age and the X, right of the row.
    const SIDE: f32 = 130.0;
    const HEIGHT: f32 = 40.0;
    let mut action = None;
    let exists = project.dir.join("game.ron").is_file();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEIGHT),
        egui::Sense::hover(),
    );
    let main = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x - SIDE, rect.max.y));
    let side = egui::Rect::from_min_max(egui::pos2(main.max.x + 8.0, rect.min.y), rect.max);

    let response = ui
        .interact(
            main,
            egui::Id::unique(("launcher_row", &project.dir)),
            egui::Sense::click(),
        )
        .on_hover_text(if exists {
            "OPEN IN THE EDITOR"
        } else {
            "THE FOLDER OR ITS GAME.RON IS GONE"
        });
    let hot = exists && response.hovered();
    let (text, frame) = match (exists, hot) {
        (false, _) => (theme::AMBER_DIM, theme::AMBER_DIM.gamma_multiply(0.5)),
        (true, true) => (theme::AMBER, theme::AMBER),
        (true, false) => (theme::AMBER, theme::AMBER_DIM),
    };
    let painter = ui.painter();
    if hot {
        painter.rect_filled(main, 0.0, theme::AMBER.gamma_multiply(0.08));
    }
    painter.rect_stroke(
        main,
        0.0,
        egui::Stroke::new(1.0, frame),
        egui::StrokeKind::Inside,
    );
    let left = main.left_top() + egui::vec2(10.0, 5.0);
    painter.text(
        left,
        egui::Align2::LEFT_TOP,
        project.name.to_uppercase(),
        egui::FontId::monospace(13.0),
        text,
    );
    painter.text(
        left + egui::vec2(0.0, 17.0),
        egui::Align2::LEFT_TOP,
        project.dir.display().to_string(),
        egui::FontId::monospace(11.0),
        theme::AMBER_DIM,
    );
    if exists && response.clicked() {
        action = Some(Action::Open(project.dir.clone()));
    }

    ui.scope_builder(egui::UiBuilder::new().max_rect(side), |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .small_button("X")
                .on_hover_text("TAKE OFF THE LIST (THE FILES STAY)")
                .clicked()
            {
                action = Some(Action::Forget(project.dir.clone()));
            }
            let when = if exists {
                recent::ago(project.opened, now)
            } else {
                "MISSING".into()
            };
            ui.label(RichText::new(when).small().color(theme::AMBER_DIM));
        });
    });
    ui.add_space(6.0);
    action
}
