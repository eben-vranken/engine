//! Window, event loop and per-frame update. `run` opens a window and plays a
//! scene in first person, with a free-fly debug camera, the debug HUD and the
//! player tuning panel.

use crate::camera::FlyCamera;
use crate::editor::{Editor, EditorStats};
use crate::input::Input;
use crate::physics::PhysicsWorld;
use crate::player::settings::{PlayerSettings, SettingsFile};
use crate::player::tuning::{self, TuningAction};
use crate::player::{Player, Spawn};
use crate::render::{RenderError, Renderer, scale};
use crate::scene::Scene;
use crate::ui::{Overlay, theme};
use egui::{Align2, RichText};
use glam::{Vec2, Vec3};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// How often `player.ron` is checked for changes.
const RELOAD_INTERVAL: Duration = Duration::from_millis(250);

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
    /// The game project folder; `player.ron` lives here.
    pub game_dir: PathBuf,
    /// Must contain a `PlayerStart`; the player spawns at the origin otherwise.
    pub scene: Scene,
    /// The file `scene` came from.
    pub scene_path: PathBuf,
    pub game_name: String,
    /// Open the editor instead of playing.
    pub editor: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CameraMode {
    Player,
    Fly,
}

struct Running {
    window: Arc<Window>,
    renderer: Renderer,
    overlay: Overlay,
    physics: PhysicsWorld,
    player: Player,
    settings: PlayerSettings,
    settings_file: SettingsFile,
    settings_status: String,
    settings_error: Option<String>,
    last_reload_check: Instant,
    fly: FlyCamera,
    mode: CameraMode,
    tuning_open: bool,
    input: Input,
    focused: bool,
    /// Player mode: Esc gave the mouse back; a click takes it again.
    mouse_released: bool,
    /// Fly mode: right mouse button held (and pressed outside the UI).
    rmb_look: bool,
    last_frame: Instant,
    fps: FpsCounter,
    scene: Scene,
    /// Present when running the editor.
    editor: Option<Editor>,
    /// Object currently highlighted in the viewport.
    highlighted: Option<usize>,
    /// Mouse position in window pixels.
    cursor: Vec2,
    /// Where the scene is drawn, in window pixels (the editor's viewport).
    viewport: Option<egui::Rect>,
    /// Editor: a left click in the viewport to resolve this frame, in window pixels.
    pending_pick: Option<Vec2>,
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
            WindowEvent::Focused(focused) => {
                app.focused = focused;
                if !focused {
                    app.input.release_all();
                    app.rmb_look = false;
                    app.mouse_released = true;
                }
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
            WindowEvent::CursorMoved { position, .. } => {
                app.cursor = Vec2::new(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { button, state, .. } if app.editor.is_some() => {
                let in_viewport = app
                    .viewport
                    .is_some_and(|v| v.contains(egui::pos2(app.cursor.x, app.cursor.y)))
                    && !app.overlay.wants_pointer();
                match button {
                    MouseButton::Right => app.rmb_look = state.is_pressed() && in_viewport,
                    MouseButton::Left if state.is_pressed() && in_viewport && !app.rmb_look => {
                        app.pending_pick = Some(app.cursor);
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let over_ui = app.overlay.wants_pointer();
                match button {
                    MouseButton::Left if state.is_pressed() && !over_ui => {
                        app.mouse_released = false;
                    }
                    MouseButton::Right => app.rmb_look = state.is_pressed() && !over_ui,
                    _ => {}
                }
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

        let settings_file = SettingsFile::new(config.game_dir.join("player.ron"));
        let (settings, settings_error) = match settings_file.load() {
            Ok(settings) => (settings, None),
            Err(err) => {
                log::error!("{err}; using defaults");
                (PlayerSettings::default(), Some(err.to_string()))
            }
        };
        let spawn = config.scene.player_start().map_or(
            Spawn {
                position: Vec3::ZERO,
                yaw: 0.0,
            },
            Spawn::from_transform,
        );
        if config.scene.player_start().is_none() {
            log::warn!("the scene has no PlayerStart; spawning at the origin");
        }
        let player = Player::new(spawn, &settings);
        let start = player.camera(&settings);
        let mut fly = FlyCamera::new(start.position, start.yaw, start.pitch);
        fly.camera.fov_y = start.fov_y;
        let editor = config
            .editor
            .then(|| Editor::new(config.game_name.clone(), config.scene_path.clone()));
        if editor.is_some() {
            fly.move_requires_look = true;
        }

        Ok(Self {
            window,
            renderer,
            overlay,
            physics: PhysicsWorld::from_scene(&config.scene),
            player,
            settings,
            settings_file,
            settings_status: String::new(),
            settings_error,
            last_reload_check: Instant::now(),
            fly,
            mode: if editor.is_some() {
                CameraMode::Fly
            } else {
                CameraMode::Player
            },
            tuning_open: false,
            input: Input::default(),
            focused: true,
            mouse_released: false,
            rmb_look: false,
            last_frame: Instant::now(),
            fps: FpsCounter::default(),
            scene: config.scene.clone(),
            editor,
            highlighted: None,
            cursor: Vec2::ZERO,
            viewport: None,
            pending_pick: None,
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

    fn update_mouse_capture(&mut self) {
        let wanted = self.focused
            && match self.mode {
                CameraMode::Player => !self.tuning_open && !self.mouse_released,
                CameraMode::Fly => self.rmb_look,
            };
        self.set_looking(wanted);
    }

    fn handle_keys(&mut self, actions: &mut HudActions) {
        let input = &self.input;
        let factor = self.renderer.layout().factor;
        if input.pressed(KeyCode::BracketLeft) {
            actions.scale = Some(Some(factor.saturating_sub(1).max(1)));
        }
        if input.pressed(KeyCode::BracketRight) {
            actions.scale = Some(Some(factor + 1));
        }
        if input.pressed(KeyCode::Digit0) {
            actions.scale = Some(None);
        }
        if input.pressed(KeyCode::KeyV) {
            actions.vsync = Some(!self.renderer.vsync());
        }
        if let Some(editor) = &mut self.editor {
            if input.pressed(KeyCode::Escape) {
                editor.selected = None;
            }
            return;
        }
        if input.pressed(KeyCode::Escape) {
            self.mouse_released = true;
        }
        if input.pressed(KeyCode::F1) {
            self.tuning_open = !self.tuning_open;
        }
        if input.pressed(KeyCode::F2) {
            self.mode = match self.mode {
                CameraMode::Player => {
                    // Start flying from where the player is looking.
                    let view = self.player.camera(&self.settings);
                    self.fly.camera = view;
                    CameraMode::Fly
                }
                CameraMode::Fly => CameraMode::Player,
            };
        }
        if input.pressed(KeyCode::KeyR) {
            self.player.respawn(&self.settings);
        }
    }

    fn poll_settings_file(&mut self) {
        if self.last_reload_check.elapsed() < RELOAD_INTERVAL {
            return;
        }
        self.last_reload_check = Instant::now();
        match self.settings_file.poll() {
            Some(Ok(settings)) => {
                self.settings = settings;
                self.settings_error = None;
                self.settings_status = "RELOADED FROM DISK".into();
                log::info!("reloaded {}", self.settings_file.path.display());
            }
            Some(Err(err)) => {
                log::error!("{err}");
                self.settings_error = Some(err.to_string());
            }
            None => {}
        }
    }

    fn apply_tuning(&mut self, action: TuningAction) {
        match action {
            TuningAction::Save => match self.settings_file.save(&self.settings) {
                Ok(()) => {
                    self.settings_error = None;
                    self.settings_status = format!(
                        "SAVED TO {}",
                        self.settings_file.path.display().to_string().to_uppercase()
                    );
                }
                Err(err) => self.settings_error = Some(err.to_string()),
            },
            TuningAction::ResetToDefaults => {
                self.settings = PlayerSettings::default();
                self.settings_status = "DEFAULTS RESTORED (NOT SAVED)".into();
            }
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.fps.tick(dt);

        let mut actions = HudActions::default();
        self.handle_keys(&mut actions);
        self.poll_settings_file();
        self.update_mouse_capture();

        let camera = match self.mode {
            CameraMode::Player => {
                self.player
                    .update(&self.input, &self.settings, &self.physics, dt);
                self.player.camera(&self.settings)
            }
            CameraMode::Fly => {
                self.fly.update(&self.input, dt);
                self.fly.camera.clone()
            }
        };

        if let (Some(editor), Some(cursor)) = (&mut self.editor, self.pending_pick.take()) {
            let [x, y, w, h] = self.renderer.image_rect();
            let ndc = Vec2::new(
                (cursor.x - x) / w * 2.0 - 1.0,
                1.0 - (cursor.y - y) / h * 2.0,
            );
            if ndc.abs().max_element() <= 1.0 {
                let (origin, direction) = camera.ray(ndc, w / h);
                editor.selected = self
                    .physics
                    .pick(origin, direction, camera.far)
                    .map(|(object, _)| object);
                editor.reveal_selection = true;
            }
        }

        let hud = HudInfo {
            fps: self.fps.fps,
            frame_ms: self.fps.frame_ms,
            gpu: self.renderer.adapter_name().to_uppercase(),
            output: self.renderer.output_size(),
            internal: self.renderer.layout().internal,
            factor: self.renderer.layout().factor,
            auto: self.renderer.render_scale_override().is_none(),
            vsync: self.renderer.vsync(),
            mode: self.mode,
            fly_speed: self.fly.speed,
            player: PlayerReadout {
                speed: self.player.horizontal_speed(),
                velocity: self.player.velocity,
                grounded: self.player.grounded,
                air_time: self.player.time_since_grounded,
                crouching: self.player.crouching,
            },
            mouse_free: !self.input.looking && self.mode == CameraMode::Player && !self.tuning_open,
            cursor_on: self.overlay.time().fract() < 0.5,
        };
        let file_name = self
            .settings_file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let editor_stats = EditorStats {
            fps: hud.fps,
            frame_ms: hud.frame_ms,
            internal: hud.internal,
            factor: hud.factor,
        };
        let blink = hud.cursor_on;
        let tuning_open = self.tuning_open;
        let settings = &mut self.settings;
        let (status, error) = (&self.settings_status, self.settings_error.as_deref());
        let mut tuning_action = None;
        let editor = &mut self.editor;
        let scene = &self.scene;
        let mut viewport = None;
        let ui_frame = self.overlay.run(
            &self.window,
            self.renderer.device(),
            self.renderer.queue(),
            |ui| match editor {
                Some(editor) => viewport = Some(editor.ui(ui, scene, &editor_stats, blink)),
                None => {
                    draw_hud(ui, &hud, &mut actions);
                    if tuning_open {
                        tuning_action = tuning::panel(ui, settings, &file_name, status, error);
                    }
                }
            },
        );
        if let Some(action) = tuning_action {
            self.apply_tuning(action);
        }

        // The editor's viewport, from egui points to window pixels.
        let ppp = ui_frame.pixels_per_point();
        self.viewport = viewport.map(|rect: egui::Rect| {
            egui::Rect::from_min_max((rect.min * ppp).round(), (rect.max * ppp).round())
        });
        self.renderer.set_viewport(self.viewport.map(|v| {
            [
                v.min.x.max(0.0) as u32,
                v.min.y.max(0.0) as u32,
                v.width().max(1.0) as u32,
                v.height().max(1.0) as u32,
            ]
        }));
        let selected = self.editor.as_ref().and_then(|e| e.selected);
        if selected != self.highlighted {
            self.renderer.set_highlight(selected);
            self.highlighted = selected;
        }

        if let Some(scale) = actions.scale {
            self.renderer.set_render_scale(scale);
        }
        if let Some(vsync) = actions.vsync {
            self.renderer.set_vsync(vsync);
        }

        let size = self.renderer.output_size();
        let overlay = &mut self.overlay;
        self.renderer
            .render(&camera, |device, queue, encoder, target| {
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

struct PlayerReadout {
    speed: f32,
    velocity: Vec3,
    grounded: bool,
    air_time: f32,
    crouching: bool,
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
    mode: CameraMode,
    fly_speed: f32,
    player: PlayerReadout,
    mouse_free: bool,
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
                        });

                    ui.add_space(6.0);
                    egui::Grid::new("hud_player")
                        .num_columns(2)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| match hud.mode {
                            CameraMode::Player => {
                                let p = &hud.player;
                                ui.label(dim("MODE"));
                                ui.label(if p.crouching {
                                    "PLAYER [CROUCH]"
                                } else {
                                    "PLAYER"
                                });
                                ui.end_row();

                                ui.label(dim("SPEED"));
                                ui.label(format!("{:>5.2} M/S", p.speed));
                                ui.end_row();

                                ui.label(dim("VELOCITY"));
                                ui.label(format!(
                                    "{:>6.2} {:>6.2} {:>6.2}",
                                    p.velocity.x, p.velocity.y, p.velocity.z
                                ));
                                ui.end_row();

                                ui.label(dim("GROUNDED"));
                                ui.label(if p.grounded { "[YES]" } else { "[NO] " });
                                ui.end_row();

                                ui.label(dim("AIR TIME"));
                                ui.label(format!("{:>5.2} S", p.air_time));
                                ui.end_row();
                            }
                            CameraMode::Fly => {
                                ui.label(dim("MODE"));
                                ui.label("FLY");
                                ui.end_row();

                                ui.label(dim("CAMERA"));
                                ui.label(format!("{:.1} M/S", hud.fly_speed));
                                ui.end_row();
                            }
                        });

                    ui.add_space(6.0);
                    let hints: &[&str] = match hud.mode {
                        CameraMode::Player => &[
                            "WASD MOVE  SPACE JUMP  SHIFT SPRINT",
                            "CTRL/C CROUCH  R RESPAWN",
                            "F1 TUNING  F2 FLY  ESC FREE MOUSE",
                        ],
                        CameraMode::Fly => &[
                            "RMB LOOK  WASD MOVE  Q/E DOWN/UP",
                            "SHIFT FAST  WHEEL SPEED  F2 PLAYER",
                        ],
                    };
                    for line in hints.iter().chain(&["[ ] SCALE  0 AUTO  V VSYNC"]) {
                        ui.label(RichText::new(*line).small().color(theme::AMBER_DIM));
                    }
                    if hud.mouse_free {
                        ui.label(
                            RichText::new("CLICK TO CAPTURE MOUSE")
                                .small()
                                .color(theme::AMBER),
                        );
                    }
                });
        });
}
