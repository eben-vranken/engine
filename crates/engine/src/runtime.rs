//! Window, event loop and per-frame update. `run` opens a window and plays a
//! scene in first person, with a free-fly debug camera, the debug HUD and the
//! player tuning panel.

use crate::assets::Assets;
use crate::audio::bake::{self, BakeJob};
use crate::audio::{Audio, Toggles};
use crate::behaviors::{Behaviors, Interaction, is_usable_switch};
use crate::camera::{Camera, FlyCamera};
use crate::editor::history::Change;
use crate::editor::import;
use crate::editor::{Editor, EditorStats};
use crate::input::Input;
use crate::lightmap;
use crate::physics::PhysicsWorld;
use crate::player::settings::{PlayerSettings, SettingsFile};
use crate::player::tuning::{self, TuningAction};
use crate::player::{Player, Spawn};
use crate::render::{RenderError, Renderer, scale};
use crate::scene::{Component, Scene};
use crate::ui::{Overlay, theme};
use crate::zones::{Active, ZoneMap, ZoneTracker};
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
    pub quality: crate::render::quality::Quality,
    /// Log FPS and GPU times every couple of seconds (for measuring on
    /// machines you can't watch).
    pub perf_log: bool,
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

impl App {
    fn close(&mut self, event_loop: &ActiveEventLoop) {
        // Never offer to save a scene mid-play.
        if let Some(running) = self.running.as_mut() {
            running.stop_play();
        }
        let may_close = match self.running.as_mut().and_then(|r| r.editor.as_mut()) {
            Some(editor) => editor.request_close(),
            None => true,
        };
        if may_close {
            event_loop.exit();
        }
    }
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
    /// `--perf-log`: when FPS and GPU times were last logged.
    perf_log: Option<Instant>,
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
    /// Files dropped on the window and where (window pixels); editor only.
    dropped: Vec<(PathBuf, Vec2)>,
    game_dir: PathBuf,
    assets: Assets,
    /// The editor confirmed closing; the event loop exits next.
    exit_requested: bool,
    /// Editor: playing in place; what Stop restores.
    play: Option<PlaySnapshot>,
    /// Running behaviors: always in play mode, in the editor while playing.
    behaviors: Option<Behaviors>,
    /// A text prompt for what the player is looking at.
    prompt: Option<&'static str>,
    /// Whether the player was on the ground last frame (landing makes a
    /// footstep).
    was_grounded: bool,
    /// `None` without an output device (the game runs silent).
    audio: Option<Audio>,
    /// The zone the active camera is in.
    zone: ZoneTracker,
    /// The simulation radius: what runs (updated every frame from `zone`).
    active: Active,
    /// The scene file, relative to the project (play mode; the editor
    /// tracks its own).
    scene_path: PathBuf,
    /// A reverb bake running in the background.
    bake_job: Option<BakeJob>,
    /// The loaded bake's signature, to tell when the scene has changed
    /// since (`None` = no bake).
    baked_signature: Option<u64>,
    /// A light bake running in the background.
    light_job: Option<lightmap::BakeJob>,
    /// Models opened in model tabs so far (names their cache keys).
    models_opened: usize,
    /// The loaded light bake's signature (`None` = no bake).
    light_signature: Option<u64>,
}

/// The editor's state when Play was pressed.
struct PlaySnapshot {
    scene: Scene,
    camera: Camera,
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
        // Editor: Tab toggles the scene panel, Shift+Tab the inspector.
        // Handled before egui, which would otherwise move keyboard focus
        // between buttons; left alone while typing in a field.
        if let (Some(editor), WindowEvent::KeyboardInput { event: key, .. }) =
            (&mut app.editor, &event)
            && key.physical_key == PhysicalKey::Code(KeyCode::Tab)
            && !app.overlay.wants_keyboard()
        {
            if key.state.is_pressed() && !key.repeat {
                let shift =
                    app.input.held(KeyCode::ShiftLeft) || app.input.held(KeyCode::ShiftRight);
                if shift {
                    editor.show_inspector = !editor.show_inspector;
                } else {
                    editor.show_scene = !editor.show_scene;
                }
            }
            return;
        }
        let ui_consumed = app.overlay.on_window_event(&app.window, &event);
        match event {
            WindowEvent::CloseRequested => self.close(event_loop),
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
                        if app.play.is_some() {
                            app.mouse_released = false;
                        } else {
                            app.pending_pick = Some(app.cursor);
                        }
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
            WindowEvent::DroppedFile(_) if app.play.is_some() => {
                if let Some(editor) = &mut app.editor {
                    editor.show_error("STOP PLAYING TO IMPORT FILES".into());
                }
            }
            WindowEvent::DroppedFile(path) if app.editor.is_some() => {
                // The window got no cursor events during the drag; ask the OS.
                if let Some(cursor) = crate::cursor::position_in(&app.window) {
                    app.cursor = cursor;
                }
                app.dropped.push((path, app.cursor));
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(app) = &self.running {
            if app.exit_requested {
                event_loop.exit();
            }
            app.window.request_redraw();
        }
    }
}

impl Running {
    fn new(event_loop: &ActiveEventLoop, config: &RunConfig) -> Result<Self, RunError> {
        let mut running = Self::open(event_loop, config)?;
        running.load_bake();
        running.load_lightmaps();
        Ok(running)
    }

    fn open(event_loop: &ActiveEventLoop, config: &RunConfig) -> Result<Self, RunError> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(&config.title)
                    .with_inner_size(LogicalSize::new(1600, 900)),
            )?,
        );
        let mut renderer = Renderer::new(window.clone(), config.quality)?;
        let mut assets = Assets::new(config.game_dir.clone());
        renderer.set_scene(&config.scene, &mut assets);
        let overlay = Overlay::new(&window, renderer.device(), renderer.surface_format());

        let settings_file = SettingsFile::new(config.game_dir.join("player.ron"));
        let (settings, settings_error) = match settings_file.load() {
            Ok(settings) => (settings, None),
            Err(err) => {
                log::error!("{err}; using defaults");
                (PlayerSettings::default(), Some(err.to_string()))
            }
        };
        let player = Player::new(spawn_point(&config.scene), &settings);
        let start = player.camera(&settings);
        let mut fly = FlyCamera::new(start.position, start.yaw, start.pitch);
        fly.camera.fov_y = start.fov_y;
        let editor = config.editor.then(|| {
            Editor::new(
                config.game_name.clone(),
                config.game_dir.clone(),
                config.scene_path.clone(),
            )
        });
        if editor.is_some() {
            fly.move_requires_look = true;
        }

        Ok(Self {
            window,
            renderer,
            overlay,
            physics: if config.editor {
                PhysicsWorld::from_scene(&config.scene, &mut assets)
            } else {
                PhysicsWorld::for_game(&config.scene, &mut assets)
            },
            assets,
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
            perf_log: config.perf_log.then(Instant::now),
            scene: config.scene.clone(),
            editor,
            highlighted: None,
            cursor: Vec2::ZERO,
            viewport: None,
            pending_pick: None,
            dropped: Vec::new(),
            game_dir: config.game_dir.clone(),
            exit_requested: false,
            play: None,
            behaviors: (!config.editor).then(|| Behaviors::start(&config.scene)),
            prompt: None,
            was_grounded: true,
            audio: Audio::new(config.game_dir.clone())
                .inspect_err(|err| log::error!("{err}; running without sound"))
                .ok(),
            zone: ZoneTracker::default(),
            active: Active::default(),
            scene_path: config.scene_path.clone(),
            bake_job: None,
            baked_signature: None,
            light_job: None,
            light_signature: None,
            models_opened: 0,
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
        if self.editor.is_some() && self.input.pressed(KeyCode::F5) {
            self.toggle_play();
        }
        // A/B comparison of the acoustic simulation, anywhere.
        if let Some(audio) = &mut self.audio {
            let toggles = &mut audio.toggles;
            for (key, flag) in [
                (KeyCode::F6, &mut toggles.occlusion),
                (KeyCode::F7, &mut toggles.reflections),
                (KeyCode::F8, &mut toggles.hrtf),
                (KeyCode::F9, &mut toggles.air_absorption),
                (KeyCode::F10, &mut toggles.baked),
            ] {
                if self.input.pressed(key) {
                    *flag = !*flag;
                }
            }
        }
        // While playing in the editor, the game's keys apply (below).
        let playing = self.play.is_some();
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
        if !playing && let Some(editor) = &mut self.editor {
            let ctrl = input.held(KeyCode::ControlLeft) || input.held(KeyCode::ControlRight);
            let shift = input.held(KeyCode::ShiftLeft) || input.held(KeyCode::ShiftRight);
            let mut settled = false;
            let mut changed = None;
            if input.pressed(KeyCode::Escape) {
                settled = editor.select(&self.scene, None);
            }
            // Gizmo modes; while flying these keys move the camera instead.
            if !input.looking && !ctrl {
                for (key, kind) in [
                    (
                        KeyCode::KeyW,
                        transform_gizmo::config::GizmoModeKind::Translate,
                    ),
                    (
                        KeyCode::KeyE,
                        transform_gizmo::config::GizmoModeKind::Rotate,
                    ),
                    (KeyCode::KeyR, transform_gizmo::config::GizmoModeKind::Scale),
                ] {
                    if input.pressed(key) {
                        editor.gizmo.kind = kind;
                    }
                }
            }
            if ctrl && input.pressed(KeyCode::KeyZ) {
                changed = if shift {
                    editor.redo(&mut self.scene, &mut self.settings)
                } else {
                    editor.undo(&mut self.scene, &mut self.settings)
                };
            }
            if ctrl && input.pressed(KeyCode::KeyY) {
                changed = editor.redo(&mut self.scene, &mut self.settings);
            }
            if ctrl && input.pressed(KeyCode::KeyS) {
                settled |= editor.save(&self.scene, &self.settings);
            }
            if input.pressed(KeyCode::Delete) {
                changed = editor.delete_selected(&mut self.scene);
            }
            // Not while flying, where D strafes.
            if ctrl && !input.looking && input.pressed(KeyCode::KeyD) {
                changed = editor.duplicate_selected(&mut self.scene);
            }
            if let Some(change) = changed {
                self.show_change(change);
                settled = true;
            }
            if settled {
                self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
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

    /// Behaviors act on the scene; moved objects are redrawn and their
    /// colliders follow.
    fn run_game(&mut self, dt: f32) {
        if self.behaviors.is_none() {
            return;
        }
        self.game_sounds();
        let interaction = self.interaction();
        let Some(behaviors) = &mut self.behaviors else {
            return;
        };
        let moved = behaviors.update(&mut self.scene, dt, &interaction, &self.active);
        for &index in &moved {
            self.renderer
                .update_object(&self.scene, index, &mut self.assets);
            self.physics
                .move_object(index, &self.scene.objects[index].transform);
        }

        if self.mode == CameraMode::Player {
            let body = &self.settings.body;
            let height = if self.player.crouching {
                body.crouch_height
            } else {
                body.height
            };
            // The capsule floats a step above the feet.
            let bottom = self.player.feet + Vec3::Y * body.step_height;
            self.physics.push_bodies(
                bottom,
                height - body.step_height,
                body.radius,
                self.player.velocity,
            );
        }
        self.physics.set_active(&self.active);
        self.physics.advance(dt);
        let poses: Vec<_> = self.physics.body_poses().collect();
        for (index, position, rotation) in poses {
            let transform = &mut self.scene.objects[index].transform;
            transform.position = position;
            transform.set_quat(rotation);
            self.renderer
                .update_object(&self.scene, index, &mut self.assets);
        }
    }

    /// Sounds the player makes: claps (Q), footsteps by floor material, and
    /// M to bring the looping sound to where the player stands.
    fn game_sounds(&mut self) {
        let eye = match self.mode {
            CameraMode::Player => self.player.camera(&self.settings),
            CameraMode::Fly => self.fly.camera.clone(),
        };
        if self.input.pressed(KeyCode::KeyM) {
            let ahead = eye.forward().with_y(0.0).normalize_or(Vec3::NEG_Z);
            let looping = self.scene.objects.iter().position(|o| {
                o.components
                    .iter()
                    .any(|c| matches!(c, Component::SoundSource(s) if s.looping))
            });
            if let Some(index) = looping {
                self.scene.objects[index].transform.position = eye.position + ahead * 1.5;
                self.renderer
                    .update_object(&self.scene, index, &mut self.assets);
                self.rebuild_physics();
            }
        }
        let Some(audio) = &mut self.audio else {
            return;
        };
        if self.input.pressed(KeyCode::KeyQ) {
            audio.clap(eye.position);
        }
        if self.mode != CameraMode::Player {
            return;
        }
        let grounded = self.player.grounded;
        let landed = grounded && !self.was_grounded;
        self.was_grounded = grounded;
        // In step with the head bob: one footstep per bob, plus one on landing.
        if landed || self.player.stepped {
            let feet = self.player.feet;
            let floor = self.physics.object_below(feet + Vec3::Y * 0.1, 0.4);
            let material = floor
                .and_then(|i| self.scene.objects.get(i))
                .and_then(|o| {
                    o.components.iter().find_map(|c| match c {
                        Component::AcousticMaterial(m) => Some(m.preset),
                        _ => None,
                    })
                })
                .unwrap_or_default();
            if floor.is_some() {
                // Just above the floor, so the step comes from the surface.
                audio.footstep(material, feet + Vec3::Y * 0.15, &self.settings.footsteps);
            }
        }
    }

    /// What the player does that behaviors react to: pressing E at a switch
    /// in reach (a prompt shows while one is aimed at), standing on things.
    fn interaction(&mut self) -> Interaction {
        self.prompt = None;
        if self.mode != CameraMode::Player {
            return Interaction::default();
        }
        let eye = self.player.camera(&self.settings);
        let aimed = self
            .physics
            .pick(eye.position, eye.forward(), REACH)
            .map(|(object, _)| object)
            .filter(|&object| is_usable_switch(&self.scene, object));
        if aimed.is_some() {
            self.prompt = Some("E  USE");
        }
        Interaction {
            used: aimed.filter(|_| self.input.pressed(KeyCode::KeyE)),
            standing_on: self
                .physics
                .object_below(self.player.feet + Vec3::Y * 0.1, 0.4),
        }
    }

    /// The physics world for the current state: dynamic bodies only while
    /// the game runs.
    fn rebuild_physics(&mut self) {
        self.physics = if self.behaviors.is_some() {
            PhysicsWorld::for_game(&self.scene, &mut self.assets)
        } else {
            PhysicsWorld::from_scene(&self.scene, &mut self.assets)
        };
    }

    /// Editor: Play runs the scene from its PlayerStart; Stop puts back the
    /// scene and the editor camera as they were.
    fn toggle_play(&mut self) {
        if self.editor.as_ref().is_some_and(|e| e.model.is_some()) {
            return; // nothing to play in a model tab
        }
        if self.play.is_some() {
            self.stop_play();
        } else {
            self.start_play();
        }
    }

    fn start_play(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.commit_pending(&self.scene);
        editor.playing = true;
        self.play = Some(PlaySnapshot {
            scene: self.scene.clone(),
            camera: self.fly.camera.clone(),
        });
        self.behaviors = Some(Behaviors::start(&self.scene));
        self.rebuild_physics();
        self.player = Player::new(spawn_point(&self.scene), &self.settings);
        self.zone.reset();
        self.mode = CameraMode::Player;
        self.mouse_released = false;
        self.tuning_open = false;
    }

    fn stop_play(&mut self) {
        let Some(snapshot) = self.play.take() else {
            return;
        };
        self.scene = snapshot.scene;
        self.fly.camera = snapshot.camera;
        self.behaviors = None;
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
        self.mode = CameraMode::Fly;
        self.tuning_open = false;
        self.zone.reset();
        if let Some(editor) = &mut self.editor {
            editor.playing = false;
        }
    }

    /// Editor: redraws what an edit changed. Adding or removing objects
    /// shifts the indices the draws refer to, so those rebuild all draws.
    fn show_change(&mut self, change: Change) {
        match change {
            Change::Modified(index) => {
                self.renderer
                    .update_object(&self.scene, index, &mut self.assets);
            }
            Change::Settings => self.renderer.set_zones(&self.scene, &self.assets),
            Change::Player => {}
            Change::Added(_) | Change::Removed(_) | Change::All => {
                let selected = self.editor.as_ref().and_then(|e| e.selected);
                self.renderer.set_highlight(selected);
                self.highlighted = selected;
                self.renderer.set_scene(&self.scene, &mut self.assets);
            }
        }
    }

    /// Editor: copies dropped model files into the project and places each
    /// on the surface under the drop point. Dropped WAV files are copied to
    /// `audio/steps/` and, while Player Start is selected, added to the
    /// footsteps.
    fn import_dropped(&mut self, camera: &Camera) {
        for (file, cursor) in std::mem::take(&mut self.dropped) {
            let Some(editor) = &mut self.editor else {
                return;
            };
            if crate::package::is_package(&file) {
                open_package_window(&file, editor);
                continue;
            }
            if import::is_sound(&file) {
                match import::copy_sound_into_project(&file, &self.game_dir) {
                    Ok(path) if editor.shows_player(&self.scene) => {
                        editor.show_message(format!("ADDED {path} TO THE FOOTSTEPS"));
                        editor.add_footstep(&self.scene, &mut self.settings, path);
                    }
                    Ok(path) => editor.show_message(format!(
                        "COPIED TO {path}; SELECT PLAYER START TO USE IT FOR FOOTSTEPS"
                    )),
                    Err(err) => {
                        log::error!("{err}");
                        editor.show_error(err.to_string());
                    }
                }
                continue;
            }
            let path = match import::copy_into_project(&file, &self.game_dir) {
                Ok(path) => path,
                Err(err) => {
                    log::error!("{err}");
                    editor.show_error(err.to_string());
                    continue;
                }
            };
            // It may have been cached as missing before the copy.
            self.assets.forget(&path);
            let bounds = self.assets.model(&path).map_or(
                (Vec3::splat(-0.25), Vec3::splat(0.25)), // the placeholder box
                |model| model.mesh.bounds(),
            );
            // Dropped on a panel: aim through the middle of the view.
            let [x, y, w, h] = self.renderer.image_rect();
            let ndc = Vec2::new(
                (cursor.x - x) / w * 2.0 - 1.0,
                1.0 - (cursor.y - y) / h * 2.0,
            );
            let ndc = if ndc.abs().max_element() <= 1.0 {
                ndc
            } else {
                Vec2::ZERO
            };
            let (origin, direction) = camera.ray(ndc, w / h);
            let surface = self.physics.surface(origin, direction, camera.far);
            let ahead = origin + direction * crate::editor::NEW_OBJECT_DISTANCE;
            let position = crate::editor::placement(bounds, surface, ahead);
            if let Some(change) = editor.add_model(&mut self.scene, path, position) {
                self.show_change(change);
                self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
            }
        }
    }

    /// Editor: replace the scene with another file from the project.
    fn open_scene(&mut self, path: PathBuf) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match Scene::load(&self.game_dir.join(&path)) {
            Ok(scene) => {
                self.scene = scene;
                editor.scene_loaded(path);
                self.scene_swapped();
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string());
            }
        }
    }

    /// Editor: opens a scene from the project in a new tab.
    fn open_tab(&mut self, path: PathBuf) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match Scene::load(&self.game_dir.join(&path)) {
            Ok(loaded) => {
                editor.open_tab(path, loaded, &mut self.scene, &self.fly);
                self.scene_swapped();
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string());
            }
        }
    }

    /// Editor: opens a model from the project in a new model tab.
    fn open_model(&mut self, path: PathBuf) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if let Some(index) = editor.tab_of(&path) {
            self.switch_tab(index);
            return;
        }
        match crate::modeler::glb::load(&self.game_dir.join(&path)) {
            Ok(mesh) => {
                self.models_opened += 1;
                // A cache key that is no file's path: the model as edited.
                let key = format!("<modeler {}>", self.models_opened);
                let data = crate::modeler::model_data(&mesh, &mut self.assets);
                self.assets.insert_model(&key, data);
                let doc = crate::editor::ModelDoc::new(mesh, key.clone());
                let stage = crate::editor::stage(&key);
                editor.open_model_tab(path, doc, stage, &mut self.scene, &self.fly);
                // Look at the model from the front, a little above.
                let eye = Vec3::new(1.8, 1.6, 2.6);
                let forward = -eye.normalize();
                let camera = &mut self.fly.camera;
                camera.position = eye;
                camera.yaw = forward.x.atan2(-forward.z);
                camera.pitch = forward.y.asin();
                self.scene_swapped();
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string().to_uppercase());
            }
        }
    }

    /// Editor: makes another tab the active one.
    fn switch_tab(&mut self, index: usize) {
        if let Some(editor) = &mut self.editor
            && editor.switch_tab(index, &mut self.scene, &mut self.fly)
        {
            self.scene_swapped();
        }
    }

    /// Editor: closes a tab (its unsaved changes were already dealt with);
    /// closing the active one moves to its neighbour first.
    fn close_tab(&mut self, index: usize) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.tab_count() < 2 {
            return;
        }
        if index == editor.active_tab() {
            let next = if index + 1 < editor.tab_count() {
                index + 1
            } else {
                index - 1
            };
            self.switch_tab(next);
        }
        if let Some(editor) = &mut self.editor {
            editor.remove_tab(index);
        }
    }

    /// Editor: writes the project folder to a package next to it.
    fn export_package(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let out = crate::package::export_path(&self.game_dir);
        match crate::package::export(&self.game_dir, &editor.game_name, &out) {
            Ok(count) => {
                log::info!("exported {} ({count} files)", out.display());
                let unsaved = if editor.any_dirty() {
                    "; UNSAVED CHANGES ARE NOT IN IT"
                } else {
                    ""
                };
                editor.show_message(format!(
                    "EXPORTED {} ({count} FILES){unsaved}",
                    out.display()
                ));
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string());
            }
        }
    }

    /// A different scene is now in `self.scene`: rebuild what is made from
    /// it and load its bakes.
    fn scene_swapped(&mut self) {
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
        self.zone.reset();
        self.highlighted = None;
        self.renderer.set_highlight(None);
        self.load_bake();
        self.load_lightmaps();
    }

    /// Hot reload: picks up `player.ron` and model files changed on disk.
    fn poll_files(&mut self) {
        if self.last_reload_check.elapsed() < RELOAD_INTERVAL {
            return;
        }
        self.last_reload_check = Instant::now();
        self.poll_settings_file();
        self.poll_model_files();
        self.check_bake();
        self.check_light_bake();
    }

    /// The current scene's file, in the project.
    fn scene_file(&self) -> PathBuf {
        let relative = self
            .editor
            .as_ref()
            .map_or(&self.scene_path, |e| &e.scene_path);
        self.game_dir.join(relative)
    }

    /// Loads the scene's reverb bake (if it has one) into the audio, and
    /// its probes into the editor.
    fn load_bake(&mut self) {
        let file = bake::path_for(&self.scene_file());
        let Some(audio) = &mut self.audio else {
            return;
        };
        let (signature, probes) = match audio.load_bake(&file) {
            Ok(Some((signature, probes))) => (Some(signature), probes),
            Ok(None) => (None, Vec::new()),
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err.to_string());
                }
                (None, Vec::new())
            }
        };
        self.baked_signature = signature;
        if let Some(editor) = &mut self.editor {
            editor.probes = probes;
        }
        self.check_bake();
    }

    /// Editor: starts baking the scene's reverb.
    fn start_bake(&mut self) {
        if self.bake_job.is_some() {
            return;
        }
        let file = bake::path_for(&self.scene_file());
        let result = match &self.audio {
            Some(audio) => audio
                .start_bake(&self.scene, &mut self.assets, file)
                .map_err(|e| e.to_string()),
            None => Err("NO AUDIO DEVICE: CAN'T BAKE".into()),
        };
        match result {
            Ok(job) => self.bake_job = Some(job),
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err);
                }
            }
        }
    }

    /// Editor: the bake's progress, or whether the loaded bake still
    /// matches the scene; loads a bake that just finished.
    fn check_bake(&mut self) {
        if let Some(job) = &mut self.bake_job {
            match job.finished() {
                None => {
                    let p = job.progress();
                    if let Some(editor) = &mut self.editor {
                        editor.bake_status = format!(
                            "BAKING {} {}/{} {:.0}%",
                            p.zone.to_uppercase(),
                            p.index,
                            p.count,
                            p.fraction * 100.0
                        );
                    }
                    return;
                }
                Some(result) => {
                    self.bake_job = None;
                    match result {
                        Ok(file) => {
                            log::info!("baked {}", file.display());
                            self.load_bake();
                            if let Some(editor) = &mut self.editor {
                                editor.show_message("REVERB BAKED".into());
                            }
                        }
                        Err(err) => {
                            log::error!("{err}");
                            if let Some(editor) = &mut self.editor {
                                editor.show_error(err.to_string());
                            }
                        }
                    }
                }
            }
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.bake_status = match self.baked_signature {
            None => "NOT BAKED".into(),
            // Compare against the scene as edited, not as played.
            Some(baked) => {
                let scene = self.play.as_ref().map_or(&self.scene, |p| &p.scene);
                if bake::signature(scene) == baked {
                    "BAKED".into()
                } else {
                    "BAKE OUTDATED".into()
                }
            }
        };
    }

    /// Loads the scene's light bake (if any) into the renderer.
    fn load_lightmaps(&mut self) {
        let file = lightmap::path_for(&self.scene_file());
        let baked = match lightmap::load(&file) {
            Ok(baked) => baked,
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err.to_string());
                }
                None
            }
        };
        self.light_signature = baked.as_ref().map(|b| b.signature);
        self.renderer.set_lightmaps(baked.map(std::sync::Arc::new));
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.check_light_bake();
    }

    /// Editor: starts baking the scene's lighting.
    fn start_light_bake(&mut self) {
        if self.light_job.is_some() {
            return;
        }
        // Bake the scene as edited, not as played.
        let scene = self.play.as_ref().map_or(&self.scene, |p| &p.scene);
        let file = lightmap::path_for(&self.scene_file());
        match lightmap::start(scene, &mut self.assets, file) {
            Ok(job) => self.light_job = Some(job),
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err.to_string().to_uppercase());
                }
            }
        }
    }

    /// Editor: the light bake's progress or whether the loaded one still
    /// matches the scene; loads a bake that just finished.
    fn check_light_bake(&mut self) {
        if let Some(job) = &mut self.light_job {
            match job.finished() {
                None => {
                    let p = job.progress();
                    if let Some(editor) = &mut self.editor {
                        editor.light_status =
                            format!("BAKING {} {:.0}%", p.stage, p.fraction * 100.0);
                    }
                    return;
                }
                Some(result) => {
                    self.light_job = None;
                    match result {
                        Ok(file) => {
                            log::info!("baked {}", file.display());
                            self.load_lightmaps();
                            if let Some(editor) = &mut self.editor {
                                editor.show_message("LIGHTING BAKED".into());
                            }
                        }
                        Err(err) => {
                            log::error!("{err}");
                            if let Some(editor) = &mut self.editor {
                                editor.show_error(err.to_string().to_uppercase());
                            }
                        }
                    }
                }
            }
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.light_status = match self.light_signature {
            None => "NOT BAKED".into(),
            Some(baked) => {
                let scene = self.play.as_ref().map_or(&self.scene, |p| &p.scene);
                if lightmap::signature(scene) == baked {
                    "BAKED".into()
                } else {
                    "BAKE OUTDATED".into()
                }
            }
        };
    }

    /// Models re-exported over their file: redraw and re-collide everything.
    fn poll_model_files(&mut self) {
        let changed = self.assets.changed_models();
        if changed.is_empty() {
            return;
        }
        log::info!("reloaded {}", changed.join(", "));
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.rebuild_physics();
        if let Some(editor) = &mut self.editor {
            editor.show_message(format!("RELOADED {}", changed.join(", ")));
        }
    }

    fn poll_settings_file(&mut self) {
        match self.settings_file.poll() {
            // The editor saving the same values isn't news.
            Some(Ok(settings)) if settings == self.settings => self.settings_error = None,
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
        if let Some(last) = self.perf_log
            && last.elapsed().as_secs_f32() >= 2.0
        {
            self.perf_log = Some(Instant::now());
            let [w, h] = self.renderer.layout().internal;
            let gpu = match self.renderer.gpu_times() {
                Some(times) => {
                    let stages: Vec<String> = crate::render::timings::STAGES
                        .iter()
                        .zip(times)
                        .map(|(name, ms)| format!("{} {ms:.2}", name.to_lowercase()))
                        .collect();
                    format!(
                        "gpu {:.2} ms ({})",
                        times.iter().sum::<f32>(),
                        stages.join(", ")
                    )
                }
                None => "gpu times unavailable".into(),
            };
            log::info!(
                "perf: {:.0} fps, {:.2} ms | {w}x{h} x{} {} | {gpu}",
                self.fps.fps,
                self.fps.frame_ms,
                self.renderer.layout().factor,
                self.renderer.quality().name(),
            );
        }

        let mut actions = HudActions::default();
        self.handle_keys(&mut actions);
        self.poll_files();
        self.update_mouse_capture();
        self.run_game(dt);

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
        let zones = ZoneMap::new(&self.scene);
        let current = self.zone.update(&self.scene, &zones, camera.position);
        self.active = Active::new(&self.scene, &zones, current);
        self.renderer.set_zone(current);
        let zone = current.map(|z| self.scene.zones[z].name.clone());
        let active_zones = self.active.zone_names(&self.scene);
        if let Some(audio) = &mut self.audio {
            audio.update(
                &self.scene,
                &mut self.assets,
                &camera,
                &self.active,
                zone.as_deref(),
            );
        }

        let hud = HudInfo {
            fps: self.fps.fps,
            frame_ms: self.fps.frame_ms,
            gpu_times: self.renderer.gpu_times(),
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
            zone: zone.clone(),
            active_zones: active_zones.clone(),
            drawn: self.renderer.drawn(),
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
            gpu_ms: hud.gpu_times.map(|t| t.iter().sum()),
            internal: hud.internal,
            factor: hud.factor,
            zone,
            active_zones,
            drawn: hud.drawn,
        };
        let blink = hud.cursor_on;
        let tuning_open = self.tuning_open;
        let settings = &mut self.settings;
        let (status, error) = (&self.settings_status, self.settings_error.as_deref());
        let mut tuning_action = None;
        let editor = &mut self.editor;
        let scene = &mut self.scene;
        let mut editor_output = None;
        let [ix, iy, iw, ih] = self.renderer.image_rect();
        let ppp = self.window.scale_factor() as f32;
        let view = crate::editor::gizmo::View {
            camera: &camera,
            image: egui::Rect::from_min_size(
                egui::pos2(ix / ppp, iy / ppp),
                egui::vec2(iw / ppp, ih / ppp),
            ),
            looking: self.input.looking,
            aim: if editor.is_some() {
                self.physics
                    .surface(camera.position, camera.forward(), AIM_DISTANCE)
            } else {
                None
            },
        };
        let prompt = self.prompt;
        let toggles = self.audio.as_ref().map(|a| a.toggles);
        let game_running = self.behaviors.is_some();
        let ui_frame = self.overlay.run(
            &self.window,
            self.renderer.device(),
            self.renderer.queue(),
            |ui| {
                match editor {
                    Some(editor) => {
                        editor_output =
                            Some(editor.ui(ui, scene, settings, &editor_stats, blink, &view));
                        if editor.playing && tuning_open {
                            tuning_action = tuning::panel(ui, settings, &file_name, status, error);
                        }
                    }
                    None => {
                        draw_hud(ui, &hud, &mut actions);
                        if tuning_open {
                            tuning_action = tuning::panel(ui, settings, &file_name, status, error);
                        }
                    }
                }
                if let Some(text) = prompt {
                    draw_prompt(ui, view.image, text);
                }
                if let Some(toggles) = toggles {
                    draw_audio_panel(ui, view.image, &toggles, game_running);
                }
            },
        );
        if let Some(action) = tuning_action {
            self.apply_tuning(action);
        }

        // A click in the viewport selects what's under it, unless it was on a gizmo handle.
        // (Model tabs pick their own vertices, edges and faces.)
        let on_gizmo = editor_output.as_ref().is_some_and(|o| o.gizmo_hovered)
            || self.editor.as_ref().is_some_and(|e| e.model.is_some());
        if let (Some(editor), Some(cursor)) = (&mut self.editor, self.pending_pick.take())
            && !on_gizmo
        {
            let [x, y, w, h] = self.renderer.image_rect();
            let ndc = Vec2::new(
                (cursor.x - x) / w * 2.0 - 1.0,
                1.0 - (cursor.y - y) / h * 2.0,
            );
            if ndc.abs().max_element() <= 1.0 {
                let (origin, direction) = camera.ray(ndc, w / h);
                let hit = self
                    .physics
                    .pick(origin, direction, camera.far)
                    .map(|(object, _)| object);
                if editor.select(&self.scene, hit) {
                    self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
                }
                editor.reveal_selection = true;
            }
        }

        self.import_dropped(&camera);

        if let Some(output) = &editor_output {
            if output.toggle_play {
                self.toggle_play();
            }
            if output.exit {
                self.exit_requested = true;
            }
            if let Some(path) = &output.open {
                self.open_scene(path.clone());
            }
            if output.bake {
                self.start_bake();
            }
            if output.bake_lighting {
                self.start_light_bake();
            }
            if output.export_package {
                self.export_package();
            }
            for &change in &output.changed {
                self.show_change(change);
            }
            if output.settled {
                self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
            }
            // Last: these swap in another scene.
            if let Some(path) = &output.new_tab {
                self.open_tab(path.clone());
            }
            if let Some(index) = output.switch_tab {
                self.switch_tab(index);
            }
            if let Some(index) = output.close_tab {
                self.close_tab(index);
            }
            if let Some(path) = &output.open_model {
                self.open_model(path.clone());
            }
        }
        // A model tab's mesh was edited: redraw it.
        if let Some(model) = self.editor.as_mut().and_then(|e| e.model.as_mut())
            && model.changed
        {
            model.changed = false;
            let data = crate::modeler::model_data(&model.mesh, &mut self.assets);
            self.assets.insert_model(&model.key, data);
            self.renderer
                .reload_object(&self.scene, 0, &mut self.assets);
        }

        // The editor's viewport, from egui points to window pixels.
        let ppp = ui_frame.pixels_per_point();
        self.viewport = editor_output.map(|output| {
            let rect = output.viewport;
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
        // No selection highlight in the running game.
        let selected = self
            .editor
            .as_ref()
            .and_then(|e| e.selected)
            .filter(|_| self.play.is_none());
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

/// A package dropped on the editor: unpack it next to itself and open it in
/// a second editor window (this one keeps its project).
fn open_package_window(file: &std::path::Path, editor: &mut Editor) {
    let opened = crate::package::unpack(file)
        .map_err(|e| e.to_string())
        .and_then(|(dir, _)| {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            std::process::Command::new(exe)
                .arg("--editor")
                .arg("--game")
                .arg(&dir)
                .spawn()
                .map_err(|e| e.to_string())?;
            Ok(dir)
        });
    match opened {
        Ok(dir) => editor.show_message(format!("UNPACKED INTO {}; OPENING IT", dir.display())),
        Err(err) => {
            log::error!("{err}");
            editor.show_error(err);
        }
    }
}

/// Where the player starts: the scene's PlayerStart, or the origin.
fn spawn_point(scene: &Scene) -> Spawn {
    scene.player_start().map_or_else(
        || {
            log::warn!("the scene has no PlayerStart; spawning at the origin");
            Spawn {
                position: Vec3::ZERO,
                yaw: 0.0,
            }
        },
        Spawn::from_transform,
    )
}

/// How far the player reaches to use a switch (meters).
const REACH: f32 = 2.5;

/// A quiet text prompt just below the middle of the view.
/// The acoustic simulation's switches, top right of the view.
fn draw_audio_panel(ui: &egui::Ui, image: egui::Rect, toggles: &Toggles, game_running: bool) {
    let state = |on: bool| if on { "[ON] " } else { "[OFF]" };
    let mut lines = vec![
        format!("F6 OCCLUSION   {}", state(toggles.occlusion)),
        format!("F7 REFLECTIONS {}", state(toggles.reflections)),
        format!("F8 HRTF        {}", state(toggles.hrtf)),
        format!("F9 AIR         {}", state(toggles.air_absorption)),
        format!("F10 BAKED      {}", state(toggles.baked)),
    ];
    if game_running {
        lines.push("Q CLAP   M MOVE SOUND HERE".into());
    }
    egui::Area::new(egui::Id::unique("audio_panel"))
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(image.right_top() + egui::vec2(-10.0, 10.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            for line in lines {
                let color = if line.contains("[OFF]") {
                    crate::ui::theme::ALERT
                } else {
                    crate::ui::theme::AMBER_DIM
                };
                ui.label(egui::RichText::new(line).small().color(color));
            }
        });
}

fn draw_prompt(ui: &egui::Ui, image: egui::Rect, text: &str) {
    egui::Area::new(egui::Id::unique("prompt"))
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(image.center() + egui::vec2(0.0, 28.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.label(egui::RichText::new(text).color(crate::ui::theme::AMBER));
        });
}

/// The simulation radius for the HUD: the active zones, or ALL.
pub(crate) fn active_text(zones: &[String]) -> String {
    if zones.is_empty() {
        "ALL".into()
    } else {
        zones.join(", ").to_uppercase()
    }
}

/// Editor: new objects go on surfaces up to this far ahead (meters).
const AIM_DISTANCE: f32 = 20.0;

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
    /// GPU milliseconds per stage, if measured.
    gpu_times: Option<[f32; crate::render::timings::STAGES.len()]>,
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
    zone: Option<String>,
    /// Current zone and neighbours; empty = everything runs.
    active_zones: Vec<String>,
    /// Draws in view, out of all draws.
    drawn: [usize; 2],
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

                            if let Some(times) = hud.gpu_times {
                                ui.label(dim("GPU"));
                                ui.label(format!("{:>5.2} MS", times.iter().sum::<f32>()));
                                ui.end_row();
                                for (name, ms) in crate::render::timings::STAGES.iter().zip(times) {
                                    ui.label(dim(&format!("  {name}")));
                                    ui.label(format!("{ms:>5.2} MS"));
                                    ui.end_row();
                                }
                            }

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
                        .show(ui, |ui| {
                            ui.label(dim("ZONE"));
                            ui.label(hud.zone.as_deref().unwrap_or("-").to_uppercase());
                            ui.end_row();
                            ui.label(dim("ACTIVE"));
                            ui.label(active_text(&hud.active_zones));
                            ui.end_row();
                            ui.label(dim("DRAWN"));
                            ui.label(format!("{} / {}", hud.drawn[0], hud.drawn[1]));
                            ui.end_row();
                            match hud.mode {
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
