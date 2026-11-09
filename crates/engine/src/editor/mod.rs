//! The editor: the running game with panels on top, in three workspaces.
//! PROJECT is the start screen and the project's settings; BUILD edits a
//! scene (outliner on the left, inspector on the right, the 3D viewport in
//! the middle); LOGIC edits graphs. CTRL+K searches and runs everything.
//!
//! Every change goes through [`history::Edit`] so it can be undone. While a
//! value is being dragged or typed, the change is applied live but held as
//! *pending*; it becomes one undo step when the interaction ends.

mod add_dialog;
mod commands;
mod confirm;
mod conversation;
mod file_dialogs;
mod files;
pub mod gizmo;
pub mod graph_canvas;
mod graph_tab;
mod hierarchy;
pub mod history;
pub mod import;
pub mod inspector;
mod keys;
mod logic_docs;
pub mod markers;
mod menu_bar;
mod model_list;
mod name_dialog;
mod object_panel;
mod objects;
mod palette;
mod player_panel;
mod problems;
mod project_doc;
mod project_index;
mod project_page;
mod refactor;
mod script_tab;
mod selection;
mod settings_panel;
mod status_bar;
mod tabs;
mod viewport_tools;
mod widgets;

use crate::player::settings::PlayerSettings;
use crate::scene::{Object, Scene};
pub use commands::Workspace;
use confirm::Confirm;
use egui::{Rect, Ui};
use files::scene_files;
use gizmo::{TransformGizmo, View};
use glam::Vec3;
pub use graph_tab::GraphDoc;
use history::{Change, Edit, History};
pub use objects::{NEW_OBJECT_DISTANCE, placement};
pub use script_tab::ScriptDoc;
pub use selection::Pick;
pub use settings_panel::SettingsTarget;
use std::path::PathBuf;
use widgets::{Side, collapsed_panel, dialog_heading, dim, panel_header, section};

/// PLAY's controls: the game paused (and one step while paused) and its
/// speed. The editor owns them; the runtime reads them each frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayControls {
    pub paused: bool,
    /// Advance one frame while paused (STEP).
    pub step: bool,
    /// Game time per real time: ¼, ½, 1 or 2.
    pub speed: f32,
}

impl Default for PlayControls {
    fn default() -> Self {
        Self {
            paused: false,
            step: false,
            speed: 1.0,
        }
    }
}

pub struct Editor {
    /// PLAY's pause, step and speed.
    pub play_controls: PlayControls,
    /// Whether vsync is on (the runtime keeps it current), for VIEW.
    pub vsync: bool,
    /// `game.ron` and `flags.ron`, as one document with its own undo.
    project: Option<project_doc::ProjectDoc>,
    /// The FLAGS list: the flag whose name is being typed, and its old name.
    flag_rename_from: Option<(usize, String)>,
    /// A graph node copied in LOGIC (CTRL+C), to paste.
    node_clipboard: Option<crate::graph::NodeKind>,
    /// Box select being dragged in the viewport (points), from the runtime.
    pub box_rect: Option<egui::Rect>,
    /// PROBLEMS is open.
    problems_open: bool,
    /// How many problems there are, and when that was counted.
    problem_count: (usize, Option<std::time::Instant>),
    /// Objects copied (CTRL+C), to paste.
    clipboard: Vec<crate::scene::Object>,
    /// Put the keyboard in the inspector's name field (RENAME, F2).
    focus_name: bool,
    /// ADD's searchable dialog, while open.
    add_dialog: Option<add_dialog::AddDialog>,
    /// Open this graph at this node once Play has stopped (OPEN AT NODE in
    /// the test drawer).
    pub open_at_node: Option<(PathBuf, crate::graph::NodeId)>,
    /// The node to select when that graph's tab opens.
    select_on_open: Option<(PathBuf, crate::graph::NodeId)>,
    /// Player feel before and after it was changed while playing: KEEP or
    /// DISCARD is asked when Play stops.
    pub feel_changed: Option<(Box<PlayerSettings>, Box<PlayerSettings>)>,
    /// Commands from keys pressed this frame, run at the start of `ui`.
    queued: Vec<commands::Command>,
    /// Index into `Scene::objects`.
    pub selected: Option<usize>,
    /// Open the selection's zone and scroll to it in the hierarchy (set when
    /// it was picked in the viewport).
    pub reveal_selection: bool,
    /// The scene file being edited, relative to the project folder.
    pub scene_path: PathBuf,
    pub game_name: String,
    project_dir: PathBuf,
    /// History state at the last save (or load).
    saved_state: Option<u64>,
    /// The last message for the status bar; true = error.
    message: Option<(String, bool)>,
    /// Waiting for the user to decide about unsaved changes.
    confirm: Option<Confirm>,
    history: History,
    /// An edit in progress: the object's index and how it was before.
    pending: Option<(usize, Object)>,
    /// Whether the scene panel (left) and the inspector (right) are open.
    pub show_scene: bool,
    pub show_inspector: bool,
    /// Zone volume and portal markers in the viewport (H).
    pub show_zones: bool,
    /// Trigger boxes in the viewport.
    show_triggers: bool,
    /// Reverb probe dots in the viewport.
    show_probes: bool,
    /// The outliner's filter chip.
    outliner_filter: hierarchy::Filter,
    /// CTRL+K, while open.
    palette: Option<palette::Palette>,
    /// `?`: the shortcut list.
    shortcuts_open: bool,
    /// PLAY's test drawer (F3).
    pub drawer_open: bool,
    /// The PROJECT workspace, while open.
    project_page: Option<project_page::ProjectPage>,
    /// The key hints over the viewport.
    show_hints: bool,
    /// The frame stats in the status bar.
    show_stats: bool,
    /// Settings shown in the inspector instead of an object.
    pub settings: Option<SettingsTarget>,
    /// A zone or environment edit in progress: the scene before it.
    zone_pending: Option<Box<Scene>>,
    /// A player settings edit in progress: the settings before it and now.
    player_pending: Option<(Box<PlayerSettings>, Box<PlayerSettings>)>,
    pub gizmo: TransformGizmo,
    /// The scene is running in the viewport; editing is locked until Stop.
    pub playing: bool,
    /// A zone being renamed in the hierarchy: its name, and the new name
    /// typed so far.
    renaming: Option<(String, String)>,
    /// A zone with objects in it being deleted: its name, and the zone its
    /// objects would move to.
    zone_delete: Option<(String, String)>,
    /// Text typed in the hierarchy's search bar.
    search: String,
    /// The MODELS list under the scene panel.
    models: model_list::ModelList,
    /// The project's flags, graphs, scenes... for reference pickers.
    index: project_index::ProjectIndex,
    /// The reverb bake's state for the BAKE menu (set by the runtime).
    pub bake_status: String,
    /// The light bake's state for the BAKE menu (set by the runtime).
    pub light_status: String,
    /// Where the loaded bake's probes are (set by the runtime).
    pub probes: Vec<Vec3>,
    /// Open scenes, in tab order; the active one is `None` (its state is
    /// the live state above).
    tabs: Vec<Option<tabs::Parked>>,
    active_tab: usize,
    /// BUILD or LOGIC (PROJECT is `project_page`).
    workspace: Workspace,
    /// LOGIC's open graphs and scripts; the active one's slot is `None`
    /// (it is `graph` / `script`, at `logic_path`).
    logic_tabs: Vec<Option<logic_docs::LogicTab>>,
    logic_active: Option<usize>,
    /// The active LOGIC document's file, relative to the project.
    logic_path: PathBuf,
    /// NEW SCENE / SAVE AS: the name being typed.
    naming: Option<name_dialog::NameDialog>,
    /// The open graph, when the active tab is a graph tab.
    pub graph: Option<GraphDoc>,
    /// The open script, when the active tab is a script tab.
    pub script: Option<ScriptDoc>,
    /// The part of the selected model shown in the inspector, by name.
    pub selected_part: Option<String>,
    /// Picked with shift-click besides `selected` / `selected_part`.
    also: Vec<Pick>,
    /// Each model in the scene's parts and their centers, by path (kept
    /// up to date by the runtime).
    pub model_parts: ModelParts,
    /// The system's file dialog, while one is open.
    file_dialog: Option<file_dialogs::FileDialog>,
}

/// Model paths → their parts' names and centers (in the model's space).
pub type ModelParts = std::collections::HashMap<String, Vec<(String, Vec3)>>;

/// Numbers for the status bar.
pub struct EditorStats {
    pub fps: f32,
    pub frame_ms: f32,
    /// GPU milliseconds per frame, if measured.
    pub gpu_ms: Option<f32>,
    pub internal: [u32; 2],
    pub factor: u32,
    /// The render scale set by hand (`[` / `]`, or the project's); `None`
    /// = automatic.
    pub scale_override: Option<u32>,
    /// The zone the camera is in.
    pub zone: Option<String>,
    /// The simulation radius: current zone and neighbours; empty = all.
    pub active_zones: Vec<String>,
    /// Draws in view, out of all draws.
    pub drawn: [usize; 2],
}

/// What the editor did this frame, for the runtime to act on.
pub struct EditorOutput {
    /// Space left for the 3D viewport, in points.
    pub viewport: Rect,
    /// What happened to the scene's objects (redraw them).
    pub changed: Vec<Change>,
    /// An edit was committed, undone or redone (rebuild collision).
    pub settled: bool,
    /// The pointer is on a gizmo handle or a marker, so a click there isn't
    /// a viewport pick.
    pub gizmo_hovered: bool,
    /// The user confirmed closing the editor.
    pub exit: bool,
    /// The Play/Stop button was clicked.
    pub toggle_play: bool,
    /// PLAY FROM HERE: play with the player where the editor camera is.
    pub play_from_here: bool,
    /// FREE CAMERA while playing (F2).
    pub free_camera: bool,
    /// VIEW › VSYNC.
    pub toggle_vsync: bool,
    /// The status bar's PROBLEMS was clicked.
    pub show_problems: bool,
    /// FRAME: look at this sphere (center, radius) with the editor camera.
    pub frame: Option<(glam::Vec3, f32)>,
    /// RESTART while playing: stop, then play from the start.
    pub restart_play: bool,
    /// Where PLAY's test drawer goes (the runtime fills it).
    pub drawer: Option<Rect>,
    /// CONTINUE: play from the saved game.
    pub resume_play: bool,
    /// Open the game in its own window, as a player gets it.
    pub play_window: bool,
    /// BAKE ACOUSTICS was clicked.
    pub bake: bool,
    /// BAKE LIGHTING was clicked.
    pub bake_lighting: bool,
    /// Write the project to a package file here.
    pub export_package: Option<PathBuf>,
    /// Make this tab the active one.
    pub switch_tab: Option<usize>,
    /// Open this scene in a new tab.
    pub new_tab: Option<PathBuf>,
    /// Close this tab (unsaved changes already dealt with).
    pub close_tab: Option<usize>,
    /// Open this graph (relative to the project) in a graph tab, or this
    /// script (`.rhai`) in a script tab.
    pub open_graph: Option<PathBuf>,
    /// Place this model (relative to the project) in the scene: where it
    /// was dropped on the scene image (points), or in front of the camera.
    pub place_model: Option<(PathBuf, Option<egui::Pos2>)>,
    /// Use this render scale now (`None` = automatic).
    pub render_scale: Option<Option<u32>>,
    /// The game was renamed (the window title follows).
    pub rename: Option<String>,
    /// Leave this project for the one in this folder (unsaved changes
    /// already dealt with).
    pub open_project: Option<PathBuf>,
}

impl EditorOutput {
    fn empty() -> Self {
        Self {
            viewport: Rect::NOTHING,
            changed: Vec::new(),
            settled: false,
            gizmo_hovered: false,
            exit: false,
            toggle_play: false,
            play_from_here: false,
            free_camera: false,
            toggle_vsync: false,
            frame: None,
            show_problems: false,
            restart_play: false,
            drawer: None,
            resume_play: false,
            play_window: false,
            bake: false,
            bake_lighting: false,
            export_package: None,
            switch_tab: None,
            new_tab: None,
            close_tab: None,
            open_graph: None,
            place_model: None,
            open_project: None,
            render_scale: None,
            rename: None,
        }
    }
}

impl Editor {
    pub fn new(game_name: String, project_dir: PathBuf, scene_path: PathBuf) -> Self {
        let project = project_doc::ProjectDoc::load(&project_dir);
        Self {
            selected: None,
            reveal_selection: false,
            scene_path,
            game_name,
            project_dir,
            saved_state: None,
            message: None,
            confirm: None,
            history: History::default(),
            pending: None,
            show_scene: true,
            show_inspector: true,
            show_zones: true,
            show_triggers: true,
            show_probes: true,
            outliner_filter: Default::default(),
            palette: None,
            shortcuts_open: false,
            // Opened with F3 when wanted; the game gets the room first.
            drawer_open: false,
            project_page: None,
            show_hints: true,
            show_stats: true,
            settings: None,
            zone_pending: None,
            player_pending: None,
            gizmo: TransformGizmo::default(),
            queued: Vec::new(),
            play_controls: PlayControls::default(),
            add_dialog: None,
            clipboard: Vec::new(),
            problems_open: false,
            box_rect: None,
            problem_count: (0, None),
            node_clipboard: None,
            focus_name: false,
            flag_rename_from: None,
            project,
            vsync: true,
            open_at_node: None,
            select_on_open: None,
            feel_changed: None,
            playing: false,
            renaming: None,
            zone_delete: None,
            search: String::new(),
            models: Default::default(),
            index: Default::default(),
            bake_status: String::new(),
            light_status: String::new(),
            probes: Vec::new(),
            tabs: vec![None],
            workspace: Workspace::Build,
            logic_tabs: Vec::new(),
            logic_active: None,
            logic_path: PathBuf::new(),
            active_tab: 0,
            naming: None,
            graph: None,
            script: None,
            file_dialog: None,
            selected_part: None,
            also: Vec::new(),
            model_parts: ModelParts::new(),
        }
    }

    /// Draws the panels, applies inspector edits to `scene`, and reports
    /// what changed.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        stats: &EditorStats,
        blink: bool,
        view: &View,
    ) -> EditorOutput {
        let mut output = EditorOutput::empty();
        self.poll_file_dialog(scene, player, &mut output);
        if !self.playing
            && let Some(request) = self.open_at_node.take()
        {
            output.open_graph = Some(request.0.clone());
            self.select_on_open = Some(request);
        }
        self.keep_feel_dialog(ui, player);
        // A finished change to the project's settings is one undo step.
        if !ui.ctx().memory(|m| m.focused().is_some())
            && !ui.ctx().input(|i| i.pointer.any_down())
            && let Some(doc) = &mut self.project
        {
            doc.settle();
        }
        // Keys pressed this frame (`press_keys`), run like menu items.
        for command in std::mem::take(&mut self.queued) {
            self.run(command, scene, player, view, &mut output);
        }
        let mut bar = egui::Frame::side_top_panel(ui.style());
        if self.playing {
            bar = bar.fill(crate::ui::theme::PLAYING);
        }
        egui::Panel::top("editor_menu").frame(bar).show(ui, |ui| {
            self.top_bar(ui, scene, player, blink, view, &mut output)
        });
        egui::Panel::bottom("editor_status").show(ui, |ui| self.status_bar(ui, stats, &mut output));
        self.global_keys(ui, scene, player, view, &mut output);
        self.palette_ui(ui, scene, player, view, &mut output);
        if self
            .problem_count
            .1
            .is_none_or(|t| t.elapsed().as_secs_f32() > 0.5)
        {
            // Flags and graphs must be known, or everything looks unknown.
            self.index.refresh(&self.project_dir, false);
            self.problem_count = (
                self.all_problems(scene).len(),
                Some(std::time::Instant::now()),
            );
        }
        if std::mem::take(&mut output.show_problems) {
            self.problems_open = true;
        }
        self.problems_ui(ui, scene, &mut output);
        if let Some(rect) = self.box_rect {
            ui.ctx()
                .layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::unique("box_select"),
                ))
                .rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, crate::ui::theme::AMBER),
                    egui::StrokeKind::Inside,
                );
        }
        let mut picked = None;
        self.add_dialog_ui(ui, scene, &mut picked, &mut output);
        if let Some(command) = picked {
            self.run(command, scene, player, view, &mut output);
        }
        self.shortcuts_ui(ui);
        self.confirm_dialog(ui, scene, player, &mut output);
        self.name_dialog(ui, &mut output);

        if self.playing {
            // The game fills the window; the test drawer sits on the right.
            if self.drawer_open {
                let panel = egui::Panel::right("test_drawer")
                    .resizable(true)
                    .default_size(430.0)
                    .min_size(340.0)
                    .show(ui, |ui| ui.allocate_space(ui.available_size()));
                output.drawer = Some(panel.response.rect);
            }
            output.viewport = ui.available_rect_before_wrap();
            self.hints(ui, output.viewport);
            return output;
        }
        match self.workspace() {
            Workspace::Project => {
                self.project_page_ui(ui, player, stats, &mut output);
                output.viewport = egui::Rect::from_min_size(
                    ui.available_rect_before_wrap().max,
                    egui::vec2(1.0, 1.0),
                );
                return output;
            }
            Workspace::Logic => {
                if self.script.is_some() {
                    self.script_ui(ui, &mut output);
                } else {
                    self.graph_ui(ui, &mut output);
                }
                return output;
            }
            Workspace::Build => {}
        }
        if self.show_scene {
            egui::Panel::left("editor_scene")
                .resizable(true)
                .default_size(260.0)
                .min_size(220.0)
                .show(ui, |ui| {
                    let playing = self.playing;
                    let mut run = None;
                    self.show_scene = !panel_header(ui, "OUTLINER", Side::Left, "TAB", |ui| {
                        if ui
                            .button("+ ADD")
                            .on_hover_text("SEARCHABLE: SHAPES, MODELS, BEHAVIORS, ZONES  (A)")
                            .clicked()
                        {
                            run = Some(commands::Command::AddDialog);
                        }
                    });
                    if let Some(command) = run {
                        self.run(command, scene, player, view, &mut output);
                    }
                    // The panel's lower part: the project's models.
                    let max = (ui.available_height() - 200.0).max(120.0);
                    egui::Panel::bottom("editor_models")
                        .resizable(true)
                        .default_size(240.0f32.min(max))
                        .min_size(120.0)
                        .max_size(max)
                        .show(ui, |ui| {
                            ui.add_enabled_ui(!playing, |ui| self.model_list(ui, &mut output));
                        });
                    ui.add_enabled_ui(!playing, |ui| self.hierarchy(ui, scene, &mut output));
                });
        } else {
            self.show_scene =
                collapsed_panel(ui, "editor_scene_strip", Side::Left, "OUTLINER", "TAB");
        }
        if self.show_inspector {
            egui::Panel::right("editor_inspector")
                .resizable(true)
                .default_size(340.0)
                // Wide enough for its content, so selecting doesn't resize the viewport.
                .min_size(340.0)
                .show(ui, |ui| {
                    self.show_inspector =
                        !panel_header(ui, "INSPECTOR", Side::Right, "SHIFT+TAB", |_| {});
                    ui.add_enabled_ui(!self.playing, |ui| {
                        self.inspector(ui, scene, player, &mut output)
                    });
                });
        } else {
            self.show_inspector = collapsed_panel(
                ui,
                "editor_inspector_strip",
                Side::Right,
                "INSPECTOR",
                "SHIFT+TAB",
            );
        }
        egui::Panel::top("editor_tabs").show(ui, |ui| self.tab_strip(ui, &mut output));
        if !self.playing {
            self.drop_model(ui, view.image, &mut output);
        }

        if let Some(index) = self
            .selected
            .filter(|&i| i < scene.objects.len() && !self.playing)
        {
            let current = scene.objects[index].transform;
            if let Some(moved) = self.gizmo.ui(ui, view, &current)
                && moved != current
            {
                let others: Vec<usize> = self
                    .selected_objects()
                    .into_iter()
                    .filter(|&i| i != index && i < scene.objects.len())
                    .collect();
                if others.is_empty() {
                    self.begin_edit(index, &scene.objects[index]);
                } else {
                    self.begin_group_move(scene);
                }
                // The others move, turn and scale with it, about it.
                let delta = moved.matrix() * current.matrix().inverse();
                for i in others {
                    let t = &mut scene.objects[i].transform;
                    let (scale, rotation, position) =
                        (delta * t.matrix()).to_scale_rotation_translation();
                    t.position = position;
                    t.set_quat(rotation);
                    t.scale = scale;
                    output.changed.push(Change::Modified(i));
                }
                scene.objects[index].transform = moved;
                output.changed.push(Change::Modified(index));
            }
            output.gizmo_hovered = self.gizmo.hovered() || self.gizmo.dragging();
        }

        if !self.playing {
            if self.show_probes {
                markers::probes(ui, view, &self.probes);
            }
            let starts = markers::player_starts(ui, view, scene, self.selected);
            let zones = if self.show_zones || self.show_triggers {
                markers::zones(
                    ui,
                    view,
                    scene,
                    self.selected,
                    &stats.active_zones,
                    (self.show_zones, self.show_triggers),
                )
            } else {
                (None, false)
            };
            let lights = markers::lights(ui, view, scene, self.selected, &self.model_parts);
            for (clicked, hovered) in [starts, zones, lights] {
                output.gizmo_hovered |= hovered;
                if let Some(index) = clicked {
                    output.settled |= self.select(scene, Some(index));
                    self.reveal_selection = true;
                }
            }
        }

        // A drag or text entry has ended: the pending change becomes one undo step.
        let interacting = self.gizmo.dragging()
            || ui.ctx().egui_is_using_pointer()
            || ui.ctx().memory(|m| m.focused().is_some());
        if !interacting {
            output.settled |= self.commit_pending(scene);
        }

        output.viewport = ui.available_rect_before_wrap();
        self.viewport_tools(ui, output.viewport, &mut output);
        self.hints(ui, output.viewport);
        output
    }

    /// Keys egui sees first: CTRL+K, `?`, H (not while typing).
    fn global_keys(
        &mut self,
        ui: &Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        view: &View,
        output: &mut EditorOutput,
    ) {
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        // `?` is read as text, so it works on any keyboard layout; the
        // other keys come from the binding table (`keys`).
        let help = ui.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Text(t) if t == "?"))
        });
        if help && !typing && self.palette.is_none() {
            self.shortcuts_open = true;
        }
        let _ = (scene, player, view, output);
    }

    /// Unsaved changes, including an edit still in progress.
    /// The active scene has unsaved changes (LOGIC's documents count apart).
    pub fn is_dirty(&self) -> bool {
        self.pending.is_some()
            || self.zone_pending.is_some()
            || self.player_pending.is_some()
            || self.history.state() != self.saved_state
    }

    /// Writes the scene to its file, and the player settings to the
    /// project's `player.ron` if they differ from it. Returns true if a
    /// pending edit was committed first (collision needs a rebuild).
    pub fn save(&mut self, scene: &Scene, player: &PlayerSettings) -> bool {
        if self.graph.is_some() {
            self.save_graph();
            return false;
        }
        if self.script.is_some() {
            self.save_script();
            return false;
        }
        let settled = self.commit_pending(scene);
        let path = self.project_dir.join(&self.scene_path);
        let saved = scene
            .save(&path)
            .map_err(|e| e.to_string())
            .and_then(|()| self.save_player(player));
        match saved {
            Ok(player_saved) => {
                self.saved_state = self.history.state();
                let also = if player_saved { " AND PLAYER.RON" } else { "" };
                self.message = Some((format!("SAVED {}{also}", self.scene_path.display()), false));
                log::info!("saved {}", path.display());
            }
            Err(err) => {
                log::error!("{err}");
                self.message = Some((err, true));
            }
        }
        settled
    }

    /// Writes `player.ron` if it differs from `player`. Returns whether it
    /// wrote. A file that doesn't parse is left alone (it may hold hand
    /// edits in progress); a missing one is only created for non-defaults.
    fn save_player(&self, player: &PlayerSettings) -> Result<bool, String> {
        let path = self.project_dir.join("player.ron");
        let unchanged = if path.exists() {
            let on_disk = PlayerSettings::load(&path)
                .map_err(|e| format!("{e} (player settings not saved)"))?;
            on_disk == *player
        } else {
            *player == PlayerSettings::default()
        };
        if unchanged {
            return Ok(false);
        }
        player.save(&path).map_err(|e| e.to_string())?;
        log::info!("saved {}", path.display());
        Ok(true)
    }

    /// A new scene was loaded: start fresh.
    pub fn scene_loaded(&mut self, scene_path: PathBuf) {
        self.message = Some((format!("OPENED {}", scene_path.display()), false));
        self.scene_path = scene_path;
        self.selected = None;
        self.settings = None;
        self.pending = None;
        self.history = History::default();
        self.saved_state = None;
    }

    pub fn show_error(&mut self, error: String) {
        self.message = Some((error, true));
    }

    pub fn show_message(&mut self, text: String) {
        self.message = Some((text, false));
    }

    /// Starts (or continues) an edit of one object; `object` is its state
    /// before this frame's change.
    fn begin_edit(&mut self, index: usize, object: &Object) {
        if self.pending.as_ref().is_none_or(|(i, _)| *i != index) {
            self.pending = Some((index, object.clone()));
        }
    }

    /// Records the edit in progress, if any. Returns true if one was recorded.
    pub fn commit_pending(&mut self, scene: &Scene) -> bool {
        if let Some((before, after)) = self.player_pending.take()
            && before != after
        {
            self.history.record(Edit::SetPlayer { before, after });
            return true;
        }
        if let Some(before) = self.zone_pending.take()
            && *before != *scene
        {
            self.history.record(Edit::SetScene {
                before,
                after: Box::new(scene.clone()),
            });
            return true;
        }
        let Some((index, before)) = self.pending.take() else {
            return false;
        };
        let after = &scene.objects[index];
        if *after == before {
            return false;
        }
        self.history.record(Edit::SetObject {
            index,
            before: Box::new(before),
            after: Box::new(after.clone()),
        });
        true
    }

    /// Undoes the last edit.
    pub fn undo(&mut self, scene: &mut Scene, player: &mut PlayerSettings) -> Option<Change> {
        if let Some(doc) = &mut self.graph {
            doc.undo();
            return None;
        }
        self.commit_pending(scene);
        let change = self.history.undo(scene, player)?;
        self.follow(change);
        Some(change)
    }

    /// Redoes the last undone edit.
    pub fn redo(&mut self, scene: &mut Scene, player: &mut PlayerSettings) -> Option<Change> {
        if let Some(doc) = &mut self.graph {
            doc.redo();
            return None;
        }
        self.commit_pending(scene);
        let change = self.history.redo(scene, player)?;
        self.follow(change);
        Some(change)
    }

    /// Applies a new edit (not a live one) and records it.
    fn push(&mut self, scene: &mut Scene, edit: Edit) -> Change {
        self.commit_pending(scene);
        let change = self.history.push(scene, edit);
        self.follow(change);
        change
    }

    /// Keeps the selection on the same object when indices shift; a newly
    /// added (or restored) object becomes the selection.
    fn follow(&mut self, change: Change) {
        match change {
            Change::Modified(_) | Change::All | Change::Settings | Change::Player => {}
            Change::Added(index) => {
                self.clear_selection();
                self.selected = Some(index);
                self.reveal_selection = true;
            }
            Change::Removed(index) => {
                self.follow_removed(index);
                self.selected = match self.selected {
                    Some(s) if s == index => None,
                    Some(s) if s > index => Some(s - 1),
                    s => s,
                }
            }
        }
    }

    /// Changes the selection, finishing any edit on the previous one.
    pub fn select(&mut self, scene: &Scene, selected: Option<usize>) -> bool {
        if let Some(doc) = &mut self.graph {
            doc.clear_selection();
            return false;
        }
        let settled = self.commit_pending(scene);
        self.selected = selected;
        self.selected_part = None;
        self.also.clear();
        if selected.is_some() {
            self.settings = None;
        }
        settled
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::test_scene;

    pub fn editor() -> Editor {
        Editor::new("test".into(), PathBuf::new(), PathBuf::new())
    }

    pub fn output() -> EditorOutput {
        EditorOutput::empty()
    }

    pub fn boxes(count: usize) -> Scene {
        test_scene(
            (0..count)
                .map(|i| (Shape::Box { size: Vec3::ONE }, Vec3::X * i as f32))
                .collect(),
        )
    }
}
