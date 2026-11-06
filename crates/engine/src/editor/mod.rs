//! The editor: the running game with panels on top. The scene hierarchy is
//! on the left (grouped by zone), the inspector on the right, and the 3D
//! viewport takes whatever space is left in the middle.
//!
//! Every change goes through [`history::Edit`] so it can be undone. While a
//! value is being dragged or typed, the change is applied live but held as
//! *pending*; it becomes one undo step when the interaction ends.

mod confirm;
mod file_dialogs;
mod files;
pub mod gizmo;
mod hierarchy;
pub mod history;
pub mod import;
pub mod inspector;
pub mod markers;
mod menu_bar;
mod model_parts;
mod model_tab;
mod model_tools;
mod name_dialog;
mod object_panel;
mod objects;
mod player_panel;
mod project_settings;
mod selection;
mod settings_panel;
mod status_bar;
mod tabs;
mod widgets;

use crate::player::settings::PlayerSettings;
use crate::scene::{Object, Scene};
use confirm::Confirm;
use egui::{Rect, Ui};
use files::{model_files, scene_files};
use gizmo::{TransformGizmo, View};
use glam::Vec3;
use history::{Change, Edit, History};
pub use model_tab::{ModelDoc, stage};
pub use objects::{NEW_OBJECT_DISTANCE, placement};
pub use selection::Pick;
pub use settings_panel::SettingsTarget;
use std::path::PathBuf;
use widgets::{Side, collapsed_panel, dialog_heading, dim, panel_header, section};

pub struct Editor {
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
    /// NEW SCENE / SAVE AS: the name being typed.
    naming: Option<name_dialog::NameDialog>,
    /// The open model, when the active tab is a model tab.
    pub model: Option<ModelDoc>,
    /// The part of the selected model shown in the inspector, by name.
    pub selected_part: Option<String>,
    /// Picked with shift-click besides `selected` / `selected_part`.
    also: Vec<Pick>,
    /// Each model in the scene's parts and their centers, by path (kept
    /// up to date by the runtime).
    pub model_parts: ModelParts,
    /// PROJECT SETTINGS, while open.
    project_settings: Option<project_settings::SettingsDialog>,
    /// PROJECT SETTINGS was picked in the menu (opens next).
    want_project_settings: bool,
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
    /// Open this model (relative to the project) in a model tab.
    pub open_model: Option<PathBuf>,
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
            bake: false,
            bake_lighting: false,
            export_package: None,
            switch_tab: None,
            new_tab: None,
            close_tab: None,
            open_model: None,
            open_project: None,
            render_scale: None,
            rename: None,
        }
    }
}

impl Editor {
    pub fn new(game_name: String, project_dir: PathBuf, scene_path: PathBuf) -> Self {
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
            show_hints: true,
            show_stats: true,
            settings: None,
            zone_pending: None,
            player_pending: None,
            gizmo: TransformGizmo::default(),
            playing: false,
            renaming: None,
            zone_delete: None,
            search: String::new(),
            bake_status: String::new(),
            light_status: String::new(),
            probes: Vec::new(),
            tabs: vec![None],
            active_tab: 0,
            naming: None,
            model: None,
            file_dialog: None,
            project_settings: None,
            want_project_settings: false,
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
        egui::Panel::top("editor_menu").show(ui, |ui| {
            self.menu_bar(ui, scene, player, blink, view, &mut output)
        });
        egui::Panel::bottom("editor_status").show(ui, |ui| self.status_bar(ui, stats));
        self.project_settings_dialog(ui, stats, &mut output);
        if self.model.is_some() {
            self.model_ui(ui, scene, player, view, &mut output);
            return output;
        }
        // H: zone markers (not while typing in a field). Tab and Shift+Tab
        // (the side panels) are handled by the runtime, before egui.
        if !ui.ctx().memory(|m| m.focused().is_some()) {
            ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::NONE, egui::Key::H) {
                    self.show_zones = !self.show_zones;
                }
            });
        }
        if self.show_scene {
            egui::Panel::left("editor_scene")
                .resizable(true)
                .default_size(260.0)
                .min_size(220.0)
                .show(ui, |ui| {
                    let playing = self.playing;
                    let mut add_zone = false;
                    self.show_scene = !panel_header(ui, "SCENE", Side::Left, "TAB", |ui| {
                        add_zone = ui
                            .add_enabled(!playing, egui::Button::new("+ ZONE").small())
                            .clicked();
                    });
                    if add_zone {
                        output.changed.push(self.add_zone(scene));
                    }
                    ui.add_enabled_ui(!playing, |ui| self.hierarchy(ui, scene, &mut output));
                });
        } else {
            self.show_scene =
                collapsed_panel(ui, "editor_scene_strip", Side::Left, "SCENE PANEL", "TAB");
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
            markers::probes(ui, view, &self.probes);
            let starts = markers::player_starts(ui, view, scene, self.selected);
            let zones = if self.show_zones {
                markers::zones(ui, view, scene, self.selected, &stats.active_zones)
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

        self.confirm_dialog(ui, scene, player, &mut output);
        self.name_dialog(ui, &mut output);

        output.viewport = ui.available_rect_before_wrap();
        self.hints(ui, output.viewport);
        output
    }

    /// Unsaved changes, including an edit still in progress.
    pub fn is_dirty(&self) -> bool {
        self.model.as_ref().is_some_and(|m| m.is_dirty())
            || self.pending.is_some()
            || self.zone_pending.is_some()
            || self.player_pending.is_some()
            || self.history.state() != self.saved_state
    }

    /// Writes the scene to its file, and the player settings to the
    /// project's `player.ron` if they differ from it. Returns true if a
    /// pending edit was committed first (collision needs a rebuild).
    pub fn save(&mut self, scene: &Scene, player: &PlayerSettings) -> bool {
        if self.model.is_some() {
            self.save_model();
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
        if let Some(model) = &mut self.model {
            model.undo();
            return None;
        }
        self.commit_pending(scene);
        let change = self.history.undo(scene, player)?;
        self.follow(change);
        Some(change)
    }

    /// Redoes the last undone edit.
    pub fn redo(&mut self, scene: &mut Scene, player: &mut PlayerSettings) -> Option<Change> {
        if let Some(model) = &mut self.model {
            model.redo();
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
        if let Some(model) = &mut self.model {
            model.clear_selection();
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
