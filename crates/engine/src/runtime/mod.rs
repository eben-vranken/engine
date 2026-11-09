//! Window, event loop and per-frame update. `run` opens a window and plays a
//! scene in first person, with a free-fly debug camera, the debug HUD and the
//! player tuning panel.

mod bakes;
mod game;
mod hot_reload;
mod hud;
mod keys;
mod map;
mod project;
mod story;
mod test_drawer;
pub(crate) use project::start_editor;
mod tabs;

use crate::assets::Assets;
use crate::audio::Audio;
use crate::audio::bake::BakeJob;
use crate::behaviors::Behaviors;
use crate::camera::{Camera, FlyCamera};
use crate::editor::{Editor, EditorStats};
use crate::input::Input;
use crate::lightmap;
use crate::perf;
use crate::physics::PhysicsWorld;
use crate::player::Player;
use crate::player::settings::{PlayerSettings, SettingsFile};
use crate::player::tuning;
use crate::render::{RenderError, Renderer};
use crate::scene::Scene;
use crate::ui::Overlay;
use crate::zones::{Active, ZoneMap, ZoneTracker};
use game::spawn_point;
use glam::Vec2;
pub(crate) use hud::active_text;
use hud::{
    FpsCounter, HudActions, HudInfo, PlayerReadout, draw_audio_panel, draw_hud, draw_prompt,
    draw_toast,
};
use std::path::PathBuf;
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
    /// The render scale to start at (`game.ron`); `None` = automatic.
    pub render_scale: Option<u32>,
    /// Log FPS and GPU times every couple of seconds (for measuring on
    /// machines you can't watch).
    pub perf_log: bool,
    /// Play mode: start from the beginning even if there is a saved game.
    pub new_game: bool,
    /// Play mode as a player gets it: no tuning panel, test drawer, debug
    /// readout or debug keys.
    pub player: bool,
    /// Play mode: walk this route instead of reading the keyboard, then quit.
    pub bench: Option<crate::bench::Route>,
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
    /// Playing as a player (`--player`): only the game, no debug tools.
    player_view: bool,
    input: Input,
    focused: bool,
    /// Player mode: Esc gave the mouse back; a click takes it again.
    mouse_released: bool,
    /// Fly mode: the right mouse button held (pressed outside the UI).
    rmb_look: bool,
    last_frame: Instant,
    fps: FpsCounter,
    /// `--bench`: the route being walked.
    bench: Option<crate::bench::Bench>,
    /// `--perf-log`: log the numbers below every `perf::INTERVAL`.
    perf_log: bool,
    /// Frame times, summarised every `perf::INTERVAL` (`perf_rolled`).
    frame_times: perf::FrameTimes,
    perf_rolled: Instant,
    /// The last interval's frame times and timers, for the log and STATS.
    frame_summary: perf::FrameSummary,
    perf_stats: Vec<perf::Stat>,
    scene: Scene,
    /// Present when running the editor.
    editor: Option<Editor>,
    /// Objects currently highlighted in the viewport.
    highlighted: Vec<usize>,
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
    /// The render quality asked for (passed on when the editor reopens).
    quality: crate::render::quality::Quality,
    /// Editor: playing in place; what Stop restores.
    play: Option<PlaySnapshot>,
    /// Running behaviors: always in play mode, in the editor while playing.
    behaviors: Option<Behaviors>,
    /// A text prompt for what the player is looking at.
    prompt: Option<String>,
    /// The game's graphs, dialogue and world state while it runs.
    story: Option<story::Story>,
    /// A scene a graph loaded while playing (the editor's own comes back
    /// on Stop).
    play_scene: Option<PathBuf>,
    /// The test drawer asked to bring the looping sound to the player.
    move_sound: bool,
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
        // Tab: in the editor, the left panel (Shift+Tab the inspector). Handled before egui, which would
        // otherwise move keyboard focus between buttons; left alone while
        // typing in a field.
        if let WindowEvent::KeyboardInput { event: key, .. } = &event
            && key.physical_key == PhysicalKey::Code(KeyCode::Tab)
            && !app.overlay.wants_keyboard()
        {
            // While the game runs Tab does nothing (the map is on M).
            if key.state.is_pressed()
                && !key.repeat
                && app.story.is_none()
                && let Some(editor) = &mut app.editor
            {
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
                    MouseButton::Right => {
                        app.rmb_look = state.is_pressed() && in_viewport;
                    }
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
        if !config.editor {
            running.start_story(!config.new_game);
        }
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
        renderer.set_render_scale(config.render_scale);
        log::info!("present mode: {:?}", renderer.present_mode());
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
            player_view: config.player && !config.editor,
            input: Input::default(),
            focused: true,
            mouse_released: false,
            rmb_look: false,
            last_frame: Instant::now(),
            fps: FpsCounter::default(),
            bench: config
                .bench
                .clone()
                .filter(|_| !config.editor)
                .map(crate::bench::Bench::new),
            perf_log: config.perf_log,
            frame_times: perf::FrameTimes::default(),
            perf_rolled: Instant::now(),
            frame_summary: perf::FrameSummary::default(),
            perf_stats: Vec::new(),
            scene: config.scene.clone(),
            editor,
            highlighted: Vec::new(),
            cursor: Vec2::ZERO,
            viewport: None,
            pending_pick: None,
            dropped: Vec::new(),
            game_dir: config.game_dir.clone(),
            exit_requested: false,
            quality: config.quality,
            play: None,
            behaviors: (!config.editor).then(|| Behaviors::start(&config.scene)),
            prompt: None,
            story: None,
            play_scene: None,
            move_sound: false,
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

    /// The refresh interval of the window's monitor in milliseconds (60 Hz
    /// if it can't be told).
    fn refresh_ms(&self) -> f32 {
        self.window
            .current_monitor()
            .and_then(|m| m.refresh_rate_millihertz())
            .map_or(1000.0 / 60.0, |mhz| 1_000_000.0 / mhz as f32)
    }

    /// Ends a `perf::INTERVAL`: summarises it for STATS and `--perf-log`.
    fn roll_perf(&mut self) {
        perf::roll();
        self.frame_summary = self.frame_times.summary(self.refresh_ms());
        self.perf_stats = perf::last();
        if !self.perf_log {
            return;
        }
        let f = self.frame_summary;
        log::info!(
            "perf: frames {} | p50 {:.2} ms | p99 {:.2} ms | max {:.2} ms | {} long (> {:.1} ms) | present {:?}",
            f.frames,
            f.p50,
            f.p99,
            f.max,
            f.long,
            f.refresh_ms * perf::LONG_FRAME,
            self.renderer.present_mode(),
        );
        if !self.perf_stats.is_empty() {
            let stats: Vec<String> = self
                .perf_stats
                .iter()
                .map(|s| format!("{} {}", s.name, s.text()).to_lowercase())
                .collect();
            log::info!("perf: {}", stats.join(" | "));
        }
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
        if let Some(audio) = &self.audio {
            let t = audio.take_timing();
            log::info!(
                "perf: audio buffer {:.1} ms | slowest fill {:.2} ms | longest gap {:.2} ms | {} underruns",
                t.buffer,
                t.slowest_fill,
                t.longest_gap,
                t.underruns,
            );
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        // Unclamped for the stats: a long frame is what we want to see.
        let frame_ms = (now - self.last_frame).as_secs_f32() * 1000.0;
        let dt = (frame_ms / 1000.0).min(0.1);
        self.last_frame = now;
        self.fps.tick(dt);
        self.frame_times.push(frame_ms);
        if self.perf_rolled.elapsed().as_secs_f32() >= perf::INTERVAL {
            self.perf_rolled = Instant::now();
            self.roll_perf();
        }

        if let Some(bench) = &mut self.bench {
            let eye = self.player.camera(&self.settings).position.y - self.player.feet.y;
            match bench.drive(&mut self.player, eye, dt) {
                Some(input) => self.input = input,
                None => {
                    self.bench = None;
                    self.exit_requested = true;
                }
            }
        }

        let mut actions = HudActions::default();
        self.handle_keys(&mut actions);
        self.poll_files();
        self.update_model_parts();
        self.update_mouse_capture();
        self.run_game(dt);

        match self.mode {
            CameraMode::Player => {
                // In dialogue or on the map the player stands still.
                let busy = self.story.as_ref().is_some_and(story::Story::busy);
                let idle = Input::default();
                let input = if busy { &idle } else { &self.input };
                self.player.update(input, &self.settings, &self.physics, dt);
            }
            CameraMode::Fly => self.fly.update(&self.input, dt),
        }
        // Scripts after the controller: they see whether it jumped.
        let busy = self.story.as_ref().is_some_and(story::Story::busy);
        let input = if busy || self.mode == CameraMode::Fly {
            Input::default()
        } else {
            self.input.clone()
        };
        self.run_scripts(&input, dt);
        let camera = match self.mode {
            CameraMode::Player => self.player.camera(&self.settings),
            CameraMode::Fly => self.fly.camera.clone(),
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

        // The story's parts on screen: the line, the map, a toast, F3.
        let line = self.story.as_ref().and_then(|s| s.line.as_ref()).map(|l| {
            (
                self.speaker_name(&l.view.speaker),
                l.view.text.clone(),
                l.time,
                l.view.replies.clone(),
                l.choice,
            )
        });
        let scene_key = self.scene_key();
        let map = self.story.as_ref().map(|s| {
            map::map_data(
                &self.scene,
                s.world.visited.get(&scene_key),
                zone.as_deref(),
                self.player.feet,
                self.player.yaw,
            )
        });
        let toast = self
            .story
            .as_ref()
            .and_then(|s| s.toast.as_ref().map(|t| t.0.clone()));
        let style = self.story.as_ref().map(|s| s.style.clone());
        let use_font = self.story.as_ref().and_then(|s| s.custom_font) == Some(true);
        let load_font = self.story.as_ref().is_some_and(|s| s.custom_font.is_none());
        let mut font_loaded = None;
        let map_open = self.story.as_ref().is_some_and(|s| s.map_open);
        let game_drawer =
            self.editor.is_none() && self.story.as_ref().is_some_and(|s| s.debug_open);
        let game_dir = self.game_dir.clone();
        let f = self.frame_summary;
        let mut drawer_stats: Vec<(&'static str, String)> = vec![
            (
                "FPS",
                format!("{:.0}  {:.2} MS", self.fps.fps, self.fps.frame_ms),
            ),
            (
                "FRAMES",
                format!("P50 {:.2}  P99 {:.2}  MAX {:.2} MS", f.p50, f.p99, f.max),
            ),
            (
                "LONG",
                format!(
                    "{} OF {} (> {:.1} MS)",
                    f.long,
                    f.frames,
                    f.refresh_ms * perf::LONG_FRAME
                ),
            ),
            (
                "PRESENT",
                format!("{:?}", self.renderer.present_mode()).to_uppercase(),
            ),
            (
                "GPU",
                self.renderer
                    .gpu_times()
                    .map_or("-".into(), |t| format!("{:.2} MS", t.iter().sum::<f32>())),
            ),
            (
                "INTERNAL",
                format!(
                    "{}×{} ×{}",
                    self.renderer.layout().internal[0],
                    self.renderer.layout().internal[1],
                    self.renderer.layout().factor
                ),
            ),
            ("ZONE", zone.clone().unwrap_or("-".into()).to_uppercase()),
            ("ACTIVE", active_text(&active_zones)),
            (
                "DRAWN",
                format!(
                    "{} / {}",
                    self.renderer.drawn()[0],
                    self.renderer.drawn()[1]
                ),
            ),
            (
                "SPEED",
                format!("{:.2} M/S", self.player.horizontal_speed()),
            ),
            (
                "GROUNDED",
                if self.player.grounded {
                    "YES".into()
                } else {
                    "NO".into()
                },
            ),
        ];
        // The last interval's timers and counters, in name order.
        drawer_stats.extend(self.perf_stats.iter().map(|s| (s.name, s.text())));
        let mut drawer_actions = None;
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
            scale_override: self.renderer.render_scale_override(),
            zone,
            active_zones,
            drawn: hud.drawn,
        };
        let blink = hud.cursor_on;
        let tuning_open = self.tuning_open;
        let player_view = self.player_view;
        let settings = &mut self.settings;
        let (status, error) = (&self.settings_status, self.settings_error.as_deref());
        let mut tuning_action = None;
        // Only over the 3D view: not over LOGIC's canvas or the PROJECT page.
        let view_shown = self.editor.as_ref().is_none_or(Editor::shows_view);
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
        let prompt = self.prompt.clone();
        let toggles = self.audio.as_ref().map(|a| a.toggles);
        let game_running = self.behaviors.is_some();
        let story_slot = &mut self.story;
        let mut audio_toggles = self.audio.as_mut().map(|a| &mut a.toggles);
        let ui_frame = self.overlay.run(
            &self.window,
            self.renderer.device(),
            self.renderer.queue(),
            |ui| {
                match editor {
                    Some(editor) => {
                        let output = editor.ui(ui, scene, settings, &editor_stats, blink, &view);
                        // PLAY's test drawer, in the space the editor kept for it.
                        if let (Some(rect), Some(story)) = (output.drawer, story_slot.as_mut()) {
                            let mut child = ui.new_child(
                                egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(8.0, 4.0))),
                            );
                            let inputs = test_drawer::Inputs {
                                toggles: audio_toggles.as_deref_mut(),
                                settings,
                                map: map.as_ref(),
                                stats: &drawer_stats,
                            };
                            drawer_actions = Some(test_drawer::drawer(&mut child, story, inputs));
                        }
                        editor_output = Some(output);
                        if editor.playing && tuning_open {
                            tuning_action = tuning::panel(ui, settings, &file_name, status, error);
                        }
                    }
                    None => {
                        if game_drawer && let Some(story) = story_slot.as_mut() {
                            egui::Panel::right("test_drawer")
                                .resizable(true)
                                .default_size(430.0)
                                .show(ui, |ui| {
                                    let inputs = test_drawer::Inputs {
                                        toggles: audio_toggles.as_deref_mut(),
                                        settings,
                                        map: map.as_ref(),
                                        stats: &drawer_stats,
                                    };
                                    drawer_actions = Some(test_drawer::drawer(ui, story, inputs));
                                });
                        } else if !player_view {
                            draw_hud(ui, &hud, &mut actions);
                        }
                        if tuning_open {
                            tuning_action = tuning::panel(ui, settings, &file_name, status, error);
                        }
                    }
                }
                if let Some(text) = &prompt {
                    draw_prompt(ui, view.image, text);
                }
                if load_font && let Some(style) = &style {
                    // Usable from the next frame on.
                    font_loaded = Some(crate::dialogue::load_font(ui.ctx(), &game_dir, style));
                }
                if let (Some((speaker, text, time, replies, choice)), Some(style)) = (&line, &style)
                {
                    let (shown, complete) = crate::dialogue::typed(text, *time, style.typing_speed);
                    let shown = crate::dialogue::Shown {
                        speaker,
                        text: shown,
                        complete,
                        replies,
                        choice: *choice,
                    };
                    crate::dialogue::draw(ui, view.image, style, use_font, &shown);
                }
                if let Some(map) = map.as_ref().filter(|_| map_open) {
                    map::draw_map(ui, view.image, map);
                }
                if let Some(text) = &toast {
                    draw_toast(ui, view.image, text);
                }
                // The audio switches' corner readout, unless the drawer shows them.
                if let Some(toggles) =
                    toggles.filter(|_| drawer_actions.is_none() && view_shown && !player_view)
                {
                    draw_audio_panel(ui, view.image, &toggles, game_running);
                }
            },
        );
        if let Some(action) = tuning_action {
            self.apply_tuning(action);
        }
        if let Some(story) = &mut self.story
            && font_loaded.is_some()
        {
            story.custom_font = font_loaded;
        }
        if let Some(drawer) = drawer_actions {
            if let Some(action) = drawer.tuning {
                self.apply_tuning(action);
            }
            if drawer.clap
                && let Some(audio) = &mut self.audio
            {
                audio.clap(camera.position);
            }
            self.move_sound |= drawer.move_sound;
        }

        // A click in the viewport selects what's under it, unless it was on a gizmo handle.
        let on_gizmo = editor_output.as_ref().is_some_and(|o| o.gizmo_hovered);
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
                let shift =
                    self.input.held(KeyCode::ShiftLeft) || self.input.held(KeyCode::ShiftRight);
                let settled = match hit {
                    // Shift-click adds or removes an object.
                    Some(object) if shift => {
                        editor.toggle_pick(&self.scene, crate::editor::Pick { object, part: None })
                    }
                    _ if shift => false,
                    _ => editor.select(&self.scene, hit),
                };
                if settled {
                    self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
                }
                editor.reveal_selection = true;
            }
        }

        // A model from the MODELS list: placed like a file dropped there
        // (next frame), at the drop point or the middle of the view.
        if let Some((path, at)) = editor_output.as_mut().and_then(|o| o.place_model.take()) {
            let cursor = at.map_or(Vec2::splat(-1.0), |p| Vec2::new(p.x, p.y) * ppp);
            self.dropped.push((self.game_dir.join(path), cursor));
        }
        self.import_dropped(&camera);

        if let Some(output) = &editor_output {
            if output.toggle_play {
                self.toggle_play();
            }
            if output.restart_play {
                self.stop_play();
                self.start_play(false);
            }
            if output.resume_play && self.play.is_none() {
                self.start_play(true);
            }
            if output.play_window {
                self.play_in_window();
            }
            if output.exit {
                self.exit_requested = true;
            }
            if output.bake {
                self.start_bake();
            }
            if output.bake_lighting {
                self.start_light_bake();
            }
            if let Some(out) = &output.export_package {
                self.export_package(out);
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
            if let Some(path) = &output.open_graph {
                self.open_graph(path.clone());
            }
            if let Some(index) = output.switch_tab {
                self.switch_tab(index);
            }
            if let Some(index) = output.close_tab {
                self.close_tab(index);
            }
            if let Some(scale) = output.render_scale {
                self.renderer.set_render_scale(scale);
            }
            if let Some(name) = &output.rename {
                self.window
                    .set_title(&format!("{name} — {} editor", crate::NAME));
            }
            if let Some(dir) = &output.open_project {
                self.open_project(dir);
            }
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
        let selected = match &self.editor {
            Some(editor) if self.play.is_none() => editor.selected_objects(),
            _ => Vec::new(),
        };
        if selected != self.highlighted {
            self.renderer.set_highlight(&selected);
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

/// Editor: new objects go on surfaces up to this far ahead (meters).
const AIM_DISTANCE: f32 = 20.0;
