//! The editor: the running game with panels on top. The scene hierarchy is
//! on the left (grouped by zone), the inspector on the right, and the 3D
//! viewport takes whatever space is left in the middle.
//!
//! Every change goes through [`history::Edit`] so it can be undone. While a
//! value is being dragged or typed, the change is applied live but held as
//! *pending*; it becomes one undo step when the interaction ends.

pub mod gizmo;
pub mod history;
pub mod inspector;

use crate::scene::{Object, Scene};
use crate::ui::theme;
use egui::{Align, Layout, Rect, RichText, Ui};
use gizmo::{TransformGizmo, View};
use history::{Edit, History};
use std::path::PathBuf;
use transform_gizmo::config::GizmoModeKind;

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
    /// Last save/load result for the top bar; true = error.
    message: Option<(String, bool)>,
    /// Waiting for the user to decide about unsaved changes.
    confirm: Option<Confirm>,
    history: History,
    /// An edit in progress: the object's index and how it was before.
    pending: Option<(usize, Object)>,
    pub gizmo: TransformGizmo,
}

/// What to do once unsaved changes are dealt with.
#[derive(Debug, Clone)]
enum Confirm {
    Open(PathBuf),
    Close,
}

/// Numbers for the top bar.
pub struct EditorStats {
    pub fps: f32,
    pub frame_ms: f32,
    pub internal: [u32; 2],
    pub factor: u32,
}

/// What the editor did this frame, for the runtime to act on.
pub struct EditorOutput {
    /// Space left for the 3D viewport, in points.
    pub viewport: Rect,
    /// Objects whose data changed (redraw them).
    pub changed: Vec<usize>,
    /// An edit was committed, undone or redone (rebuild collision).
    pub settled: bool,
    /// The pointer is on a gizmo handle, so a click there isn't a selection.
    pub gizmo_hovered: bool,
    /// Load this scene (relative to the project folder).
    pub open: Option<PathBuf>,
    /// The user confirmed closing the editor.
    pub exit: bool,
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
            gizmo: TransformGizmo::default(),
        }
    }

    /// Draws the panels, applies inspector edits to `scene`, and reports
    /// what changed.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        stats: &EditorStats,
        blink: bool,
        view: &View,
    ) -> EditorOutput {
        let mut output = EditorOutput {
            viewport: Rect::NOTHING,
            changed: Vec::new(),
            settled: false,
            gizmo_hovered: false,
            open: None,
            exit: false,
        };
        egui::Panel::top("editor_top")
            .show(ui, |ui| self.top_bar(ui, scene, stats, blink, &mut output));
        egui::Panel::left("editor_scene")
            .resizable(true)
            .default_size(260.0)
            .min_size(220.0)
            .show(ui, |ui| self.hierarchy(ui, scene));
        egui::Panel::right("editor_inspector")
            .resizable(true)
            .default_size(340.0)
            // Wide enough for its content, so selecting doesn't resize the viewport.
            .min_size(340.0)
            .show(ui, |ui| self.inspector(ui, scene, &mut output));

        if let Some(index) = self.selected.filter(|&i| i < scene.objects.len()) {
            let current = scene.objects[index].transform;
            if let Some(moved) = self.gizmo.ui(ui, view, &current)
                && moved != current
            {
                self.begin_edit(index, &scene.objects[index]);
                scene.objects[index].transform = moved;
                output.changed.push(index);
            }
            output.gizmo_hovered = self.gizmo.hovered() || self.gizmo.dragging();
        }

        // A drag or text entry has ended: the pending change becomes one undo step.
        let interacting = self.gizmo.dragging()
            || ui.ctx().egui_is_using_pointer()
            || ui.ctx().memory(|m| m.focused().is_some());
        if !interacting {
            output.settled |= self.commit_pending(scene);
        }

        self.confirm_dialog(ui, scene, &mut output);

        output.viewport = ui.available_rect_before_wrap();
        hints(ui, output.viewport);
        output
    }

    /// Unsaved changes, including an edit still in progress.
    pub fn is_dirty(&self) -> bool {
        self.pending.is_some() || self.history.state() != self.saved_state
    }

    /// Writes the scene to its file. Returns true if a pending edit was
    /// committed first (collision needs a rebuild).
    pub fn save(&mut self, scene: &Scene) -> bool {
        let settled = self.commit_pending(scene);
        let path = self.project_dir.join(&self.scene_path);
        match scene.save(&path) {
            Ok(()) => {
                self.saved_state = self.history.state();
                self.message = Some((format!("SAVED {}", self.scene_path.display()), false));
                log::info!("saved {}", path.display());
            }
            Err(err) => {
                log::error!("{err}");
                self.message = Some((err.to_string(), true));
            }
        }
        settled
    }

    /// The window wants to close. Returns true if it may close now;
    /// otherwise asks about unsaved changes first.
    pub fn request_close(&mut self) -> bool {
        if self.is_dirty() {
            self.confirm = Some(Confirm::Close);
            false
        } else {
            true
        }
    }

    /// A new scene was loaded: start fresh.
    pub fn scene_loaded(&mut self, scene_path: PathBuf) {
        self.message = Some((format!("OPENED {}", scene_path.display()), false));
        self.scene_path = scene_path;
        self.selected = None;
        self.pending = None;
        self.history = History::default();
        self.saved_state = None;
    }

    pub fn show_error(&mut self, error: String) {
        self.message = Some((error, true));
    }

    fn open_scene(&mut self, path: PathBuf, output: &mut EditorOutput) {
        if self.is_dirty() {
            self.confirm = Some(Confirm::Open(path));
        } else {
            output.open = Some(path);
        }
    }

    /// Save / discard / cancel, when unsaved changes are in the way.
    fn confirm_dialog(&mut self, ui: &Ui, scene: &Scene, output: &mut EditorOutput) {
        let Some(confirm) = self.confirm.clone() else {
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("unsaved_changes")).show(ui.ctx(), |ui| {
            ui.label(RichText::new("UNSAVED CHANGES").strong());
            ui.label(dim(&format!(
                "{} HAS CHANGES THAT AREN'T SAVED.",
                self.scene_path.display().to_string().to_uppercase()
            )));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("SAVE").clicked() {
                    decision = Some(true);
                }
                if ui.button("DISCARD").clicked() {
                    decision = Some(false);
                }
                if ui.button("CANCEL").clicked() {
                    self.confirm = None;
                }
            });
        });
        let Some(save) = decision else {
            return;
        };
        self.confirm = None;
        if save {
            output.settled |= self.save(scene);
            if self.is_dirty() {
                return; // saving failed; the error is in the top bar
            }
        }
        match confirm {
            Confirm::Open(path) => output.open = Some(path),
            Confirm::Close => output.exit = true,
        }
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

    /// Undoes the last edit; returns the object it changed.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<usize> {
        self.commit_pending(scene);
        self.history.undo(scene)
    }

    /// Redoes the last undone edit; returns the object it changed.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<usize> {
        self.commit_pending(scene);
        self.history.redo(scene)
    }

    /// Changes the selection, finishing any edit on the previous one.
    pub fn select(&mut self, scene: &Scene, selected: Option<usize>) -> bool {
        let settled = self.commit_pending(scene);
        self.selected = selected;
        settled
    }

    fn top_bar(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        stats: &EditorStats,
        blink: bool,
        output: &mut EditorOutput,
    ) {
        ui.horizontal(|ui| {
            let cursor = if blink { "_" } else { " " };
            ui.label(RichText::new(format!("ENGINE // EDITOR{cursor}")).strong());
            ui.add_space(12.0);
            ui.label(self.game_name.to_uppercase());
            egui::ComboBox::from_id_salt("scene_picker")
                .selected_text(self.scene_path.display().to_string().to_uppercase())
                .show_ui(ui, |ui| {
                    for path in scene_files(&self.project_dir) {
                        let current = path == self.scene_path;
                        let text = path.display().to_string().to_uppercase();
                        if ui.selectable_label(current, text).clicked() && !current {
                            self.open_scene(path, output);
                        }
                    }
                });
            if self.is_dirty() {
                ui.label(RichText::new("[MODIFIED]").color(theme::AMBER));
            }
            if ui.small_button("SAVE").clicked() {
                output.settled |= self.save(scene);
            }
            if let Some((text, error)) = &self.message {
                let color = if *error {
                    theme::ALERT
                } else {
                    theme::AMBER_DIM
                };
                ui.label(RichText::new(text.to_uppercase()).small().color(color));
            }
            ui.add_space(12.0);

            let undo = format!("UNDO [{}]", self.history.undo_len());
            if ui
                .add_enabled(self.history.undo_len() > 0, egui::Button::new(undo).small())
                .clicked()
                && let Some(index) = self.undo(scene)
            {
                output.changed.push(index);
                output.settled = true;
            }
            let redo = format!("REDO [{}]", self.history.redo_len());
            if ui
                .add_enabled(self.history.redo_len() > 0, egui::Button::new(redo).small())
                .clicked()
                && let Some(index) = self.redo(scene)
            {
                output.changed.push(index);
                output.settled = true;
            }

            ui.add_space(12.0);
            for (kind, text) in [
                (GizmoModeKind::Translate, "MOVE W"),
                (GizmoModeKind::Rotate, "ROTATE E"),
                (GizmoModeKind::Scale, "SCALE R"),
            ] {
                let label = if self.gizmo.kind == kind {
                    format!("[{text}]")
                } else {
                    format!(" {text} ")
                };
                if ui.small_button(label).clicked() {
                    self.gizmo.kind = kind;
                }
            }
            ui.label(dim("CTRL SNAP"));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(format!(
                    "{}×{} ×{}",
                    stats.internal[0], stats.internal[1], stats.factor
                ));
                ui.label(dim("INTERNAL"));
                ui.add_space(12.0);
                ui.label(format!("{:.0}  {:.2} MS", stats.fps, stats.frame_ms));
                ui.label(dim("FPS"));
            });
        });
    }

    fn hierarchy(&mut self, ui: &mut Ui, scene: &Scene) {
        ui.add_space(4.0);
        ui.label(RichText::new("SCENE").strong());
        ui.label(dim(&format!(
            "{} OBJECTS IN {} ZONES",
            scene.objects.len(),
            scene.zones.len()
        )));
        ui.add_space(4.0);
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for zone in &scene.zones {
                    let members: Vec<(usize, &Object)> = scene
                        .objects
                        .iter()
                        .enumerate()
                        .filter(|(_, o)| o.zone == zone.name)
                        .collect();
                    let header = format!("{} [{}]", zone.name.to_uppercase(), members.len());
                    let reveal = self.reveal_selection
                        && members.iter().any(|(i, _)| Some(*i) == self.selected);
                    egui::CollapsingHeader::new(header)
                        .id_salt(&zone.name)
                        .open(reveal.then_some(true))
                        .show(ui, |ui| {
                            for (index, object) in members {
                                let selected = self.selected == Some(index);
                                let response = ui.selectable_label(selected, &object.name);
                                if response.clicked() {
                                    clicked = Some(index);
                                }
                                if selected && reveal {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                            }
                        });
                }
            });
        self.reveal_selection = false;
        if let Some(index) = clicked {
            self.select(scene, Some(index));
        }
    }

    fn inspector(&mut self, ui: &mut Ui, scene: &mut Scene, output: &mut EditorOutput) {
        ui.add_space(4.0);
        ui.label(RichText::new("INSPECTOR").strong());
        ui.add_space(4.0);
        let Some(index) = self.selected.filter(|&i| i < scene.objects.len()) else {
            ui.label(dim("NOTHING SELECTED"));
            return;
        };

        let zones = &scene.zones;
        let object = &mut scene.objects[index];
        let before = object.clone();

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    inspector::field_label(ui, "NAME");
                    ui.text_edit_singleline(&mut object.name);
                });
                ui.horizontal(|ui| {
                    inspector::field_label(ui, "ZONE");
                    egui::ComboBox::from_id_salt("inspector_zone")
                        .selected_text(object.zone.to_uppercase())
                        .show_ui(ui, |ui| {
                            for zone in zones {
                                ui.selectable_value(
                                    &mut object.zone,
                                    zone.name.clone(),
                                    zone.name.to_uppercase(),
                                );
                            }
                        });
                });

                section(ui, "TRANSFORM");
                inspector::fields(ui, &mut object.transform);

                for component in &mut object.components {
                    section(ui, &inspector::title(component));
                    inspector::fields(ui, component);
                }
            });

        if *object != before {
            self.begin_edit(index, &before);
            output.changed.push(index);
        }
    }
}

/// Scene files in the project's `scenes/` folder, relative to the project.
fn scene_files(project_dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(project_dir.join("scenes")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ron"))
        .filter_map(|p| p.strip_prefix(project_dir).ok().map(PathBuf::from))
        .collect();
    files.sort();
    files
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong());
    ui.separator();
}

fn dim(text: &str) -> RichText {
    RichText::new(text).color(theme::AMBER_DIM)
}

/// Controls, in the viewport's bottom-left corner.
fn hints(ui: &Ui, viewport: Rect) {
    egui::Area::new(egui::Id::unique("editor_hints"))
        .fixed_pos(viewport.left_bottom() + egui::vec2(10.0, -26.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.label(
                RichText::new(
                    "CLICK SELECT  W/E/R GIZMO  CTRL SNAP  RMB+WASD FLY  CTRL+Z/Y UNDO/REDO  CTRL+S SAVE",
                )
                .small()
                .color(theme::AMBER_DIM),
            );
        });
}
