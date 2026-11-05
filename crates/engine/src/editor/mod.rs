//! The editor: the running game with panels on top. The scene hierarchy is
//! on the left (grouped by zone), the inspector on the right, and the 3D
//! viewport takes whatever space is left in the middle.
//!
//! Every change goes through [`history::Edit`] so it can be undone. While a
//! value is being dragged or typed, the change is applied live but held as
//! *pending*; it becomes one undo step when the interaction ends.

pub mod gizmo;
pub mod history;
pub mod import;
pub mod inspector;
pub mod markers;

use crate::render::mesh::Shape;
use crate::scene::{
    AcousticMaterial, Component, Door, Floating, Model, Object, PhysicsBody, Portal, Primitive,
    Scene, SoundSource, Switch, Transform, Zone, ZoneVolume,
};
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
    /// The scene is running in the viewport; editing is locked until Stop.
    pub playing: bool,
    /// A zone being renamed in the hierarchy: its name, and the new name
    /// typed so far.
    renaming: Option<(String, String)>,
    /// The reverb bake's state for the top bar (set by the runtime).
    pub bake_status: String,
    /// Where the loaded bake's probes are (set by the runtime).
    pub probes: Vec<Vec3>,
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
    /// Load this scene (relative to the project folder).
    pub open: Option<PathBuf>,
    /// The user confirmed closing the editor.
    pub exit: bool,
    /// The Play/Stop button was clicked.
    pub toggle_play: bool,
    /// BAKE ACOUSTICS was clicked.
    pub bake: bool,
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
            playing: false,
            renaming: None,
            bake_status: String::new(),
            probes: Vec::new(),
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
            toggle_play: false,
            bake: false,
        };
        egui::Panel::top("editor_top").show(ui, |ui| {
            self.top_bar(ui, scene, stats, blink, view, &mut output)
        });
        egui::Panel::left("editor_scene")
            .resizable(true)
            .default_size(260.0)
            .min_size(220.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(!self.playing, |ui| self.hierarchy(ui, scene, &mut output));
            });
        egui::Panel::right("editor_inspector")
            .resizable(true)
            .default_size(340.0)
            // Wide enough for its content, so selecting doesn't resize the viewport.
            .min_size(340.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(!self.playing, |ui| self.inspector(ui, scene, &mut output));
            });

        if let Some(index) = self
            .selected
            .filter(|&i| i < scene.objects.len() && !self.playing)
        {
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

        if !self.playing {
            markers::probes(ui, view, &self.probes);
            let starts = markers::player_starts(ui, view, scene, self.selected);
            let zones = markers::zones(ui, view, scene, self.selected, &stats.active_zones);
            for (clicked, hovered) in [starts, zones] {
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

        self.confirm_dialog(ui, scene, &mut output);

        output.viewport = ui.available_rect_before_wrap();
        hints(ui, output.viewport, self.playing);
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

    pub fn show_message(&mut self, text: String) {
        self.message = Some((text, false));
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
            Change::Modified(_) | Change::All => {}
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
        let half = shape.half_extents();
        let ahead = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        let object = Object {
            name: unique_name(scene, kind),
            zone: self.new_object_zone(scene)?,
            transform: Transform::at(placement((-half, half), view.aim, ahead)),
            components: vec![Component::Primitive(Primitive {
                shape,
                color: Vec3::splat(0.8),
                collision: true,
            })],
        };
        Some(self.append(scene, object))
    }

    /// Adds the PlayerStart where the camera looks, facing the way the
    /// camera faces (snapped to 15°).
    fn add_player_start(&mut self, scene: &mut Scene, view: &View) -> Option<Change> {
        let bounds = (
            Vec3::new(-0.3, 0.0, -0.3),
            Vec3::new(0.3, markers::POST_HEIGHT, 0.3),
        );
        let ahead = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        // A camera yaw turns right, an object's Y rotation turns left.
        let yaw = (-view.camera.yaw.to_degrees() / 15.0).round() * 15.0 + 0.0;
        let object = Object {
            name: unique_name(scene, "player start"),
            zone: self.new_object_zone(scene)?,
            transform: Transform {
                position: placement(bounds, view.aim, ahead),
                rotation: Vec3::new(0.0, yaw, 0.0),
                ..Default::default()
            },
            components: vec![Component::PlayerStart],
        };
        Some(self.append(scene, object))
    }

    /// Adds a zone volume (in the selection's zone) standing on the surface
    /// the camera looks at.
    fn add_zone_volume(&mut self, scene: &mut Scene, view: &View) -> Option<Change> {
        let volume = ZoneVolume::default();
        let half = volume.size / 2.0;
        let ahead = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        let zone = self.new_object_zone(scene)?;
        let object = Object {
            name: unique_name(scene, &format!("{zone} volume")),
            zone,
            transform: Transform::at(placement((-half, half), view.aim, ahead)),
            components: vec![Component::ZoneVolume(volume)],
        };
        Some(self.append(scene, object))
    }

    /// Adds a portal (from the selection's zone, to be pointed at another)
    /// on the surface the camera looks at, facing the camera.
    fn add_portal(&mut self, scene: &mut Scene, view: &View) -> Option<Change> {
        let half = markers::PORTAL_SIZE.x / 2.0;
        let bounds = (
            Vec3::new(-half, 0.0, -0.05),
            Vec3::new(half, markers::PORTAL_SIZE.y, 0.05),
        );
        let ahead = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        let yaw = (-view.camera.yaw.to_degrees() / 15.0).round() * 15.0 + 0.0;
        let zone = self.new_object_zone(scene)?;
        let object = Object {
            name: unique_name(scene, &format!("{zone} portal")),
            zone,
            transform: Transform {
                position: placement(bounds, view.aim, ahead),
                rotation: Vec3::new(0.0, yaw, 0.0),
                ..Default::default()
            },
            components: vec![Component::Portal(Portal::default())],
        };
        Some(self.append(scene, object))
    }

    /// Adds an empty zone named `zone`, `zone 2`... and starts renaming it.
    fn add_zone(&mut self, scene: &mut Scene) -> Change {
        let taken = |name: &str| scene.zones.iter().any(|z| z.name == name);
        let name = std::iter::once("zone".to_owned())
            .chain((2..).map(|n| format!("zone {n}")))
            .find(|name| !taken(name))
            .expect("some number is free");
        let mut after = scene.clone();
        after.zones.push(Zone { name: name.clone() });
        self.renaming = Some((name.clone(), name));
        self.push_scene(scene, after)
    }

    /// Renames a zone, and every object and portal that refers to it.
    /// `None` if the new name is empty, taken or unchanged.
    fn rename_zone(&mut self, scene: &mut Scene, old: &str, new: &str) -> Option<Change> {
        let new = new.trim();
        if new.is_empty() || scene.zones.iter().any(|z| z.name == new) {
            return None;
        }
        let mut after = scene.clone();
        for zone in after.zones.iter_mut().filter(|z| z.name == old) {
            zone.name = new.to_owned();
        }
        for object in &mut after.objects {
            if object.zone == old {
                object.zone = new.to_owned();
            }
            for component in &mut object.components {
                if let Component::Portal(portal) = component
                    && portal.to.0 == old
                {
                    portal.to.0 = new.to_owned();
                }
            }
        }
        Some(self.push_scene(scene, after))
    }

    /// Removes a zone with no objects in it.
    fn delete_zone(&mut self, scene: &mut Scene, name: &str) -> Option<Change> {
        if scene.objects.iter().any(|o| o.zone == name) {
            return None;
        }
        let mut after = scene.clone();
        after.zones.retain(|z| z.name != name);
        Some(self.push_scene(scene, after))
    }

    /// Replaces the whole scene as one undo step.
    fn push_scene(&mut self, scene: &mut Scene, after: Scene) -> Change {
        self.commit_pending(scene);
        let before = Box::new(scene.clone());
        self.push(
            scene,
            Edit::SetScene {
                before,
                after: Box::new(after),
            },
        )
    }

    /// Adds a model (path relative to the project) at `position`, in the
    /// selection's zone. `None` if the scene has no zones.
    pub fn add_model(&mut self, scene: &mut Scene, path: String, position: Vec3) -> Option<Change> {
        let stem = path.rsplit('/').next().unwrap_or(&path);
        let stem = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem);
        let object = Object {
            name: unique_name(scene, stem),
            zone: self.new_object_zone(scene)?,
            transform: Transform::at(position),
            components: vec![Component::Model(Model {
                path: path.clone(),
                color: Vec3::ONE,
                collision: true,
            })],
        };
        self.message = Some((format!("ADDED {path}"), false));
        Some(self.append(scene, object))
    }

    /// New objects go into the selection's zone, or the first zone.
    fn new_object_zone(&self, scene: &Scene) -> Option<String> {
        self.selected
            .and_then(|i| scene.objects.get(i))
            .map(|o| o.zone.clone())
            .or_else(|| scene.zones.first().map(|z| z.name.clone()))
    }

    fn append(&mut self, scene: &mut Scene, object: Object) -> Change {
        let index = scene.objects.len();
        self.push(
            scene,
            Edit::Add {
                index,
                object: Box::new(object),
            },
        )
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
            ui.add_space(12.0);
            let play = if self.playing {
                "[STOP F5]"
            } else {
                " PLAY F5 "
            };
            if ui.small_button(play).clicked() {
                output.toggle_play = true;
            }
            if self.playing {
                ui.label(RichText::new("[PLAYING]").color(theme::AMBER));
            }
            ui.add_space(12.0);
            // Editing waits until Stop: the scene is restored then.
            ui.add_enabled_ui(!self.playing, |ui| {
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
                        ui.separator();
                        let has_start = scene.player_start().is_some();
                        if ui
                            .add_enabled(!has_start, egui::Button::new("PLAYER START"))
                            .on_disabled_hover_text("THE SCENE ALREADY HAS ONE")
                            .clicked()
                            && let Some(change) = self.add_player_start(scene, view)
                        {
                            output.changed.push(change);
                            output.settled = true;
                        }
                        ui.separator();
                        if ui.button("ZONE VOLUME").clicked()
                            && let Some(change) = self.add_zone_volume(scene, view)
                        {
                            output.changed.push(change);
                            output.settled = true;
                        }
                        if ui.button("PORTAL").clicked()
                            && let Some(change) = self.add_portal(scene, view)
                        {
                            output.changed.push(change);
                            output.settled = true;
                        }
                    });
                });

                ui.add_space(12.0);
                let baking = self.bake_status.starts_with("BAKING");
                if ui
                    .add_enabled(!baking, egui::Button::new("BAKE ACOUSTICS").small())
                    .on_hover_text("REVERB PROBES IN EVERY ZONE VOLUME")
                    .clicked()
                {
                    output.bake = true;
                }
                let color = if self.bake_status == "BAKED" {
                    theme::AMBER_DIM
                } else {
                    theme::AMBER
                };
                ui.label(RichText::new(&self.bake_status).small().color(color));

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
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(format!("{} / {}", stats.drawn[0], stats.drawn[1]));
                ui.label(dim("DRAWN"));
                ui.add_space(12.0);
                let zone = stats.zone.as_deref().unwrap_or("-");
                ui.label(zone.to_uppercase()).on_hover_text(format!(
                    "ACTIVE: {}",
                    crate::runtime::active_text(&stats.active_zones)
                ));
                ui.label(dim("ZONE"));
                ui.add_space(12.0);
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

    fn hierarchy(&mut self, ui: &mut Ui, scene: &mut Scene, output: &mut EditorOutput) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("SCENE").strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("+ ZONE").clicked() {
                    output.changed.push(self.add_zone(scene));
                }
            });
        });
        ui.label(dim(&format!(
            "{} OBJECTS IN {} ZONES  (RIGHT-CLICK A ZONE)",
            scene.objects.len(),
            scene.zones.len()
        )));
        ui.add_space(4.0);
        let mut clicked = None;
        let mut zone_action = None;
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
                    if let Some((old, typed)) = &mut self.renaming
                        && *old == zone.name
                    {
                        let response = ui.text_edit_singleline(typed);
                        if !response.has_focus() && !response.lost_focus() {
                            response.request_focus();
                        }
                        if response.lost_focus() {
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            zone_action = Some(if enter {
                                ZoneAction::Rename(old.clone(), typed.clone())
                            } else {
                                ZoneAction::CancelRename
                            });
                        }
                        continue;
                    }
                    let empty = members.is_empty();
                    let response = egui::CollapsingHeader::new(header)
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
                    response.header_response.context_menu(|ui| {
                        if ui.button("RENAME").clicked() {
                            zone_action = Some(ZoneAction::StartRename(zone.name.clone()));
                            ui.close();
                        }
                        if ui
                            .add_enabled(empty, egui::Button::new("DELETE"))
                            .on_disabled_hover_text("ONLY EMPTY ZONES")
                            .clicked()
                        {
                            zone_action = Some(ZoneAction::Delete(zone.name.clone()));
                            ui.close();
                        }
                    });
                }
            });
        self.reveal_selection = false;
        if let Some(index) = clicked {
            self.select(scene, Some(index));
        }
        let change = match zone_action {
            Some(ZoneAction::StartRename(name)) => {
                self.renaming = Some((name.clone(), name));
                None
            }
            Some(ZoneAction::CancelRename) => {
                self.renaming = None;
                None
            }
            Some(ZoneAction::Rename(old, new)) => {
                self.renaming = None;
                if new.trim() != old {
                    let change = self.rename_zone(scene, &old, &new);
                    if change.is_none() {
                        self.show_error(format!("CAN'T RENAME TO \"{}\"", new.trim()));
                    }
                    change
                } else {
                    None
                }
            }
            Some(ZoneAction::Delete(name)) => self.delete_zone(scene, &name),
            None => None,
        };
        if let Some(change) = change {
            output.changed.push(change);
            output.settled = true;
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

        // For reference pickers.
        let mut objects: Vec<String> = scene.objects.iter().map(|o| o.name.clone()).collect();
        objects.sort();
        let names = inspector::Names {
            objects,
            zones: scene.zones.iter().map(|z| z.name.clone()).collect(),
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
                inspector::fields(ui, &mut object.transform, &names);

                let mut remove = None;
                for (i, component) in object.components.iter_mut().enumerate() {
                    if component_header(ui, &inspector::title(component)) {
                        remove = Some(i);
                    }
                    if let Component::Primitive(primitive) = component {
                        shape_kind(ui, i, &mut primitive.shape);
                    }
                    inspector::fields(ui, component, &names);
                }
                if let Some(i) = remove {
                    object.components.remove(i);
                }

                ui.add_space(10.0);
                ui.menu_button("ADD COMPONENT", |ui| {
                    for (name, component) in new_components() {
                        let kind = std::mem::discriminant(&component);
                        let present = object
                            .components
                            .iter()
                            .any(|c| std::mem::discriminant(c) == kind);
                        if ui
                            .add_enabled(!present, egui::Button::new(name.to_uppercase()))
                            .clicked()
                        {
                            object.components.push(component);
                        }
                    }
                });
            });

        if *object != before {
            self.begin_edit(index, &before);
            output.changed.push(Change::Modified(index));
        }
    }
}

/// How far in front of the camera new objects float when the camera isn't
/// looking at a surface, in meters.
pub const NEW_OBJECT_DISTANCE: f32 = 4.0;
/// The fine grid (same as Ctrl-snapping), in meters.
const SNAP: f32 = 0.25;

/// Where a new object goes: resting on `surface` (point, normal) if there
/// is one, otherwise at `fallback`. `bounds` is the object's bounding box
/// (min, max corner) around its origin. Snapped to the fine grid along the
/// surface (so snapped moves stay aligned) but not into it.
pub fn placement(bounds: (Vec3, Vec3), surface: Option<(Vec3, Vec3)>, fallback: Vec3) -> Vec3 {
    let Some((point, normal)) = surface else {
        return (fallback / SNAP).round() * SNAP;
    };
    // The box's corner furthest against the normal touches the surface.
    let (min, max) = bounds;
    let corner = Vec3::select(normal.cmpgt(Vec3::ZERO), min, max);
    let position = point - normal * normal.dot(corner);
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

/// Picks a primitive's shape type; the new shape keeps about the same size.
fn shape_kind(ui: &mut Ui, id: usize, shape: &mut Shape) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, "TYPE");
        egui::ComboBox::from_id_salt(("shape_kind", id))
            .selected_text(shape.kind().to_uppercase())
            .show_ui(ui, |ui| {
                for kind in Shape::KINDS {
                    if ui
                        .selectable_label(shape.kind() == kind, kind.to_uppercase())
                        .clicked()
                        && shape.kind() != kind
                    {
                        *shape = shape.with_kind(kind);
                    }
                }
            });
    });
}

/// A zone edit picked in the hierarchy, applied after drawing it.
enum ZoneAction {
    StartRename(String),
    CancelRename,
    /// Old name, new name.
    Rename(String, String),
    Delete(String),
}

/// Components the inspector can add (one of each per object).
fn new_components() -> [(&'static str, Component); 8] {
    [
        ("floating", Component::Floating(Floating::default())),
        ("door", Component::Door(Door::default())),
        ("switch", Component::Switch(Switch::default())),
        (
            "physics body",
            Component::PhysicsBody(PhysicsBody::default()),
        ),
        (
            "sound source",
            Component::SoundSource(SoundSource::default()),
        ),
        (
            "acoustic material",
            Component::AcousticMaterial(AcousticMaterial::default()),
        ),
        ("zone volume", Component::ZoneVolume(ZoneVolume::default())),
        ("portal", Component::Portal(Portal::default())),
    ]
}

/// A component's title with a REMOVE button; returns true if it was clicked.
fn component_header(ui: &mut Ui, title: &str) -> bool {
    ui.add_space(10.0);
    let removed = ui
        .horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.small_button("REMOVE").clicked()
            })
            .inner
        })
        .inner;
    ui.separator();
    removed
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
fn hints(ui: &Ui, viewport: Rect, playing: bool) {
    let text = if playing {
        "F5 STOP  E USE  ESC FREE MOUSE  CLICK CAPTURE MOUSE  R RESPAWN  F1 TUNING  F2 FLY"
    } else {
        "CLICK SELECT  W/E/R GIZMO  CTRL SNAP  RMB+WASD FLY  CTRL+D DUPLICATE  DEL DELETE  CTRL+Z/Y UNDO/REDO  CTRL+S SAVE  F5 PLAY"
    };
    egui::Area::new(egui::Id::unique("editor_hints"))
        // Anchored at the bottom, so wrapped lines grow upward.
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(viewport.left_bottom() + egui::vec2(10.0, -8.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_width(viewport.width() - 20.0);
            ui.label(RichText::new(text).small().color(theme::AMBER_DIM));
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
    fn renaming_a_zone_moves_its_objects_and_portals_and_undoes() {
        let mut scene = boxes(2);
        scene.zones.push(crate::scene::Zone {
            name: "hall".into(),
        });
        scene.objects[1].components = vec![Component::Portal(Portal {
            to: crate::scene::ZoneRef("test".into()),
        })];
        scene.objects[1].zone = "hall".into();
        let original = scene.clone();
        let mut editor = editor();

        assert!(
            editor.rename_zone(&mut scene, "test", "hall").is_none(),
            "taken"
        );
        assert_eq!(
            editor.rename_zone(&mut scene, "test", " cellar "),
            Some(Change::All)
        );
        assert_eq!(scene.zones[0].name, "cellar");
        assert_eq!(scene.objects[0].zone, "cellar");
        assert_eq!(
            scene.objects[1].components[0],
            Component::Portal(Portal {
                to: crate::scene::ZoneRef("cellar".into())
            })
        );
        assert!(
            editor.delete_zone(&mut scene, "cellar").is_none(),
            "not empty"
        );
        editor.undo(&mut scene);
        assert_eq!(scene, original);

        editor.add_zone(&mut scene);
        assert_eq!(scene.zones.last().unwrap().name, "zone");
        assert!(editor.delete_zone(&mut scene, "zone").is_some());
        assert_eq!(scene, original);
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
    fn player_start_stands_on_the_floor_facing_like_the_camera() {
        let mut scene = boxes(1);
        let mut editor = editor();
        let camera = Camera {
            position: Vec3::new(0.0, 1.7, 0.0),
            // Looking toward +X (yaw turns right), slightly off the 15° grid.
            yaw: 92f32.to_radians(),
            pitch: -0.4,
            fov_y: 70.0,
            near: 0.1,
            far: 100.0,
        };
        let view = View {
            camera: &camera,
            image: Rect::NOTHING,
            looking: false,
            aim: Some((Vec3::new(3.0, 0.0, 0.1), Vec3::Y)),
        };
        editor.add_player_start(&mut scene, &view);
        let start = scene.player_start().expect("added");
        assert_eq!(start.position, Vec3::new(3.0, 0.0, 0.0));
        let facing = start.quat() * Vec3::NEG_Z;
        assert!((facing - Vec3::X).length() < 1e-5, "faces +X, got {facing}");
    }

    #[test]
    fn placement_rests_on_the_surface() {
        let half = primitives()[3].1.half_extents();
        let cylinder = (-half, half);
        let floor = Some((Vec3::new(0.3, 0.1, -2.0), Vec3::Y));
        // On a floor at y = 0.1: bottom touches it, x/z on the grid.
        assert_eq!(
            placement(cylinder, floor, Vec3::ZERO),
            Vec3::new(0.25, 0.6, -2.0),
            "half height 0.5 above the floor"
        );
        // Against a wall facing +X at x = 1.1.
        let wall = Some((Vec3::new(1.1, 0.4, -2.0), Vec3::X));
        assert_eq!(
            placement(cylinder, wall, Vec3::ZERO),
            Vec3::new(1.6, 0.5, -2.0)
        );
        // A model with its origin at its base (as exported from Blender)
        // sits with the origin on the floor.
        let model = (Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 3.0, 1.0));
        assert_eq!(
            placement(model, floor, Vec3::ZERO),
            Vec3::new(0.25, 0.1, -2.0)
        );
        // Nothing hit: the fallback, on the grid.
        assert_eq!(
            placement(model, None, Vec3::new(0.3, 1.0, -4.1)),
            Vec3::new(0.25, 1.0, -4.0)
        );
    }
}
