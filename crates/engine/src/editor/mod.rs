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

use crate::render::mesh::Shape;
use crate::scene::{Component, Object, Primitive, Scene, Transform};
use crate::ui::theme;
use egui::{Align, Layout, Rect, RichText, Ui};
use gizmo::{TransformGizmo, View};
use glam::{Vec2, Vec3};
use history::{Change, Edit, History};
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
    /// What happened to the scene's objects (redraw them).
    pub changed: Vec<Change>,
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
        egui::Panel::top("editor_top").show(ui, |ui| {
            self.top_bar(ui, scene, stats, blink, view, &mut output)
        });
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
                output.changed.push(Change::Modified(index));
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

    /// Undoes the last edit.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<Change> {
        self.commit_pending(scene);
        let change = self.history.undo(scene)?;
        self.follow(change);
        Some(change)
    }

    /// Redoes the last undone edit.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<Change> {
        self.commit_pending(scene);
        let change = self.history.redo(scene)?;
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
            Change::Modified(_) => {}
            Change::Added(index) => {
                self.selected = Some(index);
                self.reveal_selection = true;
            }
            Change::Removed(index) => {
                self.selected = match self.selected {
                    Some(s) if s == index => None,
                    Some(s) if s > index => Some(s - 1),
                    s => s,
                }
            }
        }
    }

    /// Adds a primitive in front of the camera, in the selection's zone (or
    /// the first zone). `None` if the scene has no zones.
    fn add_primitive(
        &mut self,
        scene: &mut Scene,
        kind: &str,
        shape: Shape,
        view: &View,
    ) -> Option<Change> {
        let zone = self
            .selected
            .and_then(|i| scene.objects.get(i))
            .map(|o| o.zone.clone())
            .or_else(|| scene.zones.first().map(|z| z.name.clone()))?;
        let object = Object {
            name: unique_name(scene, kind),
            zone,
            transform: Transform::at(placement(shape, view)),
            components: vec![Component::Primitive(Primitive {
                shape,
                color: Vec3::splat(0.8),
            })],
        };
        let index = scene.objects.len();
        Some(self.push(
            scene,
            Edit::Add {
                index,
                object: Box::new(object),
            },
        ))
    }

    /// Removes the selected object.
    pub fn delete_selected(&mut self, scene: &mut Scene) -> Option<Change> {
        let index = self.selected.filter(|&i| i < scene.objects.len())?;
        // Commit first so undo restores the object with its latest edit.
        self.commit_pending(scene);
        let object = Box::new(scene.objects[index].clone());
        Some(self.push(scene, Edit::Remove { index, object }))
    }

    /// Copies the selected object in place, right after it in the list, and
    /// selects the copy (move it with the gizmo).
    pub fn duplicate_selected(&mut self, scene: &mut Scene) -> Option<Change> {
        let index = self.selected.filter(|&i| i < scene.objects.len())?;
        let mut object = scene.objects[index].clone();
        object.name = unique_name(scene, &object.name);
        Some(self.push(
            scene,
            Edit::Add {
                index: index + 1,
                object: Box::new(object),
            },
        ))
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
        view: &View,
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
                && let Some(change) = self.undo(scene)
            {
                output.changed.push(change);
                output.settled = true;
            }
            let redo = format!("REDO [{}]", self.history.redo_len());
            if ui
                .add_enabled(self.history.redo_len() > 0, egui::Button::new(redo).small())
                .clicked()
                && let Some(change) = self.redo(scene)
            {
                output.changed.push(change);
                output.settled = true;
            }

            ui.add_space(12.0);
            ui.add_enabled_ui(!scene.zones.is_empty(), |ui| {
                ui.menu_button("ADD", |ui| {
                    for (kind, shape) in primitives() {
                        if ui.button(kind.to_uppercase()).clicked()
                            && let Some(change) = self.add_primitive(scene, kind, shape, view)
                        {
                            output.changed.push(change);
                            output.settled = true;
                        }
                    }
                });
            });

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

        let mut structural = None;
        ui.horizontal(|ui| {
            if ui.small_button("DUPLICATE CTRL+D").clicked() {
                structural = self.duplicate_selected(scene);
            }
            if ui.small_button("DELETE DEL").clicked() {
                structural = self.delete_selected(scene);
            }
        });
        if let Some(change) = structural {
            output.changed.push(change);
            output.settled = true;
            return;
        }
        ui.add_space(4.0);

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
            output.changed.push(Change::Modified(index));
        }
    }
}

/// How far in front of the camera new objects float when the camera isn't
/// looking at a surface, in meters.
const NEW_OBJECT_DISTANCE: f32 = 4.0;
/// The fine grid (same as Ctrl-snapping), in meters.
const SNAP: f32 = 0.25;

/// Where a new primitive goes: resting on the surface the camera looks at,
/// or floating in front of it. Snapped to the fine grid along the surface
/// (so snapped moves stay aligned) but not into it.
fn placement(shape: Shape, view: &View) -> Vec3 {
    let Some((point, normal)) = view.aim else {
        let position = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        return (position / SNAP).round() * SNAP;
    };
    // Push the shape out along the normal until its bounding box touches.
    let position = point + normal * normal.abs().dot(shape.half_extents());
    let snapped = (position / SNAP).round() * SNAP;
    Vec3::select(normal.abs().cmpgt(Vec3::splat(0.5)), position, snapped)
}

/// The Add menu: each primitive with a starting size of about a meter.
fn primitives() -> [(&'static str, Shape); 5] {
    [
        ("box", Shape::Box { size: Vec3::ONE }),
        (
            "plane",
            Shape::Plane {
                size: Vec2::splat(4.0),
            },
        ),
        (
            "wedge",
            Shape::Wedge {
                size: Vec3::new(1.0, 1.0, 2.0),
            },
        ),
        (
            "cylinder",
            Shape::Cylinder {
                radius: 0.5,
                height: 1.0,
                segments: 12,
            },
        ),
        (
            "sphere",
            Shape::Sphere {
                radius: 0.5,
                segments: 12,
                rings: 8,
            },
        ),
    ]
}

/// `base`, or `base 2`, `base 3`... if that name is taken. A trailing number
/// on `base` is replaced, so copies of "box 2" are "box 3", not "box 2 2".
fn unique_name(scene: &Scene, base: &str) -> String {
    let taken = |name: &str| scene.objects.iter().any(|o| o.name == name);
    if !taken(base) {
        return base.to_owned();
    }
    let stem = match base.rsplit_once(' ') {
        Some((stem, n)) if n.parse::<u32>().is_ok() => stem,
        _ => base,
    };
    (2..)
        .map(|n| format!("{stem} {n}"))
        .find(|name| !taken(name))
        .expect("some number is free")
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
        // Anchored at the bottom, so wrapped lines grow upward.
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(viewport.left_bottom() + egui::vec2(10.0, -8.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_width(viewport.width() - 20.0);
            ui.label(
                RichText::new(
                    "CLICK SELECT  W/E/R GIZMO  CTRL SNAP  RMB+WASD FLY  CTRL+D DUPLICATE  DEL DELETE  CTRL+Z/Y UNDO/REDO  CTRL+S SAVE",
                )
                .small()
                .color(theme::AMBER_DIM),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::scene::test_scene;

    fn editor() -> Editor {
        Editor::new("test".into(), PathBuf::new(), PathBuf::new())
    }

    fn boxes(count: usize) -> Scene {
        test_scene(
            (0..count)
                .map(|i| (Shape::Box { size: Vec3::ONE }, Vec3::X * i as f32))
                .collect(),
        )
    }

    #[test]
    fn unique_names_count_up() {
        let mut scene = boxes(1);
        scene.objects[0].name = "box".into();
        assert_eq!(unique_name(&scene, "lamp"), "lamp");
        assert_eq!(unique_name(&scene, "box"), "box 2");
        scene.objects[0].name = "box 2".into();
        assert_eq!(unique_name(&scene, "box 2"), "box 3");
    }

    #[test]
    fn selection_follows_deletes_and_undo() {
        let mut scene = boxes(3);
        let mut editor = editor();

        // Deleting an earlier object keeps the same object selected.
        editor.selected = Some(0);
        editor.delete_selected(&mut scene);
        assert_eq!(editor.selected, None);
        editor.selected = Some(1);
        editor.undo(&mut scene);
        assert_eq!(editor.selected, Some(0), "restored object is selected");
        assert_eq!(scene.objects[2].name, "object 2");

        editor.redo(&mut scene);
        assert_eq!(editor.selected, None);
        assert_eq!(scene.objects.len(), 2);
    }

    #[test]
    fn duplicate_goes_after_the_original_and_undoes() {
        let mut scene = boxes(2);
        let original = scene.clone();
        let mut editor = editor();
        editor.selected = Some(0);
        assert_eq!(
            editor.duplicate_selected(&mut scene),
            Some(Change::Added(1))
        );
        assert_eq!(editor.selected, Some(1));
        assert_eq!(scene.objects[1].name, "object 2");
        assert_eq!(scene.objects[1].transform, scene.objects[0].transform);

        editor.undo(&mut scene);
        assert_eq!(scene, original);
        assert_eq!(editor.selected, None);
    }

    #[test]
    fn added_primitive_lands_in_the_selected_zone_on_the_grid() {
        let mut scene = boxes(1);
        scene.zones.push(crate::scene::Zone {
            name: "other".into(),
        });
        scene.objects[0].zone = "other".into();
        let mut editor = editor();
        editor.selected = Some(0);
        let camera = Camera {
            position: Vec3::new(0.1, 1.7, 0.0),
            yaw: 0.3,
            pitch: -0.2,
            fov_y: 70.0,
            near: 0.1,
            far: 100.0,
        };
        let view = View {
            camera: &camera,
            image: Rect::NOTHING,
            looking: false,
            aim: None,
        };
        let (kind, shape) = primitives()[0];
        let change = editor.add_primitive(&mut scene, kind, shape, &view);
        assert_eq!(change, Some(Change::Added(1)));
        let added = &scene.objects[1];
        assert_eq!(added.zone, "other");
        let p = added.transform.position;
        assert_eq!(p, (p / SNAP).round() * SNAP);
        assert!((p - camera.position).length() > 3.0);
    }

    #[test]
    fn placement_rests_on_the_aimed_surface() {
        let camera = Camera {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 70.0,
            near: 0.1,
            far: 100.0,
        };
        let mut view = View {
            camera: &camera,
            image: Rect::NOTHING,
            looking: false,
            aim: Some((Vec3::new(0.3, 0.1, -2.0), Vec3::Y)),
        };
        let cylinder = primitives()[3].1;
        // On a floor at y = 0.1: bottom touches it, x/z on the grid.
        assert_eq!(
            placement(cylinder, &view),
            Vec3::new(0.25, 0.6, -2.0),
            "half height 0.5 above the floor"
        );
        // Against a wall facing +X at x = 1.1.
        view.aim = Some((Vec3::new(1.1, 0.4, -2.0), Vec3::X));
        assert_eq!(placement(cylinder, &view), Vec3::new(1.6, 0.5, -2.0));
    }
}
