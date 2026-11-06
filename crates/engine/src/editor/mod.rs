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
mod model_tab;
mod new_scene;
mod tabs;

use crate::player::settings::{Footsteps, PlayerSettings, RandomOrder};
use crate::player::tuning;
use crate::render::mesh::Shape;
use crate::scene::{
    AcousticMaterial, Component, Door, Floating, FogVolume, LightSource, Material, Model, Object,
    PhysicsBody, Portal, Primitive, Scene, SoundSource, Switch, Transform, Zone, ZoneVolume,
};
use crate::ui::theme;
use egui::{Align, Layout, Rect, RichText, Ui};
use gizmo::{TransformGizmo, View};
use glam::{Vec2, Vec3};
use history::{Change, Edit, History};
pub use model_tab::{ModelDoc, model_files, stage};
use std::path::{Path, PathBuf};
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
    /// Whether the scene panel (left) and the inspector (right) are open.
    pub show_scene: bool,
    pub show_inspector: bool,
    /// Zone volume and portal markers in the viewport (H).
    pub show_zones: bool,
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
    /// Text typed in the hierarchy's search bar.
    search: String,
    /// The reverb bake's state for the top bar (set by the runtime).
    pub bake_status: String,
    /// The light bake's state for the top bar (set by the runtime).
    pub light_status: String,
    /// Where the loaded bake's probes are (set by the runtime).
    pub probes: Vec<Vec3>,
    /// Open scenes, in tab order; the active one is `None` (its state is
    /// the live state above).
    tabs: Vec<Option<tabs::Parked>>,
    active_tab: usize,
    /// NEW SCENE / SAVE AS: the name being typed.
    naming: Option<new_scene::NameDialog>,
    /// The open model, when the active tab is a model tab.
    pub model: Option<ModelDoc>,
}

/// What to do once unsaved changes are dealt with.
#[derive(Debug, Clone)]
enum Confirm {
    Open(PathBuf),
    /// Close this tab.
    CloseTab(usize),
    /// Close the editor (every tab).
    Close,
}

/// Numbers for the top bar.
pub struct EditorStats {
    pub fps: f32,
    pub frame_ms: f32,
    /// GPU milliseconds per frame, if measured.
    pub gpu_ms: Option<f32>,
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
    /// BAKE LIGHTING was clicked.
    pub bake_lighting: bool,
    /// EXPORT PACKAGE was clicked.
    pub export_package: bool,
    /// Make this tab the active one.
    pub switch_tab: Option<usize>,
    /// Open this scene in a new tab.
    pub new_tab: Option<PathBuf>,
    /// Close this tab (unsaved changes already dealt with).
    pub close_tab: Option<usize>,
    /// Open this model (relative to the project) in a model tab.
    pub open_model: Option<PathBuf>,
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
            settings: None,
            zone_pending: None,
            player_pending: None,
            gizmo: TransformGizmo::default(),
            playing: false,
            renaming: None,
            search: String::new(),
            bake_status: String::new(),
            light_status: String::new(),
            probes: Vec::new(),
            tabs: vec![None],
            active_tab: 0,
            naming: None,
            model: None,
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
        let mut output = EditorOutput {
            viewport: Rect::NOTHING,
            changed: Vec::new(),
            settled: false,
            gizmo_hovered: false,
            open: None,
            exit: false,
            toggle_play: false,
            bake: false,
            bake_lighting: false,
            export_package: false,
            switch_tab: None,
            new_tab: None,
            close_tab: None,
            open_model: None,
        };
        if self.model.is_some() {
            self.model_ui(ui, scene, player, blink, view, &mut output);
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
        egui::Panel::top("editor_top").show(ui, |ui| {
            self.top_bar(ui, scene, player, stats, blink, view, &mut output)
        });
        if self.show_scene {
            egui::Panel::left("editor_scene")
                .resizable(true)
                .default_size(260.0)
                .min_size(220.0)
                .show(ui, |ui| {
                    ui.add_enabled_ui(!self.playing, |ui| self.hierarchy(ui, scene, &mut output));
                });
        }
        if self.show_inspector {
            egui::Panel::right("editor_inspector")
                .resizable(true)
                .default_size(340.0)
                // Wide enough for its content, so selecting doesn't resize the viewport.
                .min_size(340.0)
                .show(ui, |ui| {
                    ui.add_enabled_ui(!self.playing, |ui| {
                        self.inspector(ui, scene, player, &mut output)
                    });
                });
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
                self.begin_edit(index, &scene.objects[index]);
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
            let lights = markers::lights(ui, view, scene, self.selected);
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
        self.name_dialog(ui, scene, player, &mut output);

        output.viewport = ui.available_rect_before_wrap();
        hints(ui, output.viewport, self.playing);
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

    /// The window wants to close. Returns true if it may close now;
    /// otherwise asks about unsaved changes first.
    pub fn request_close(&mut self) -> bool {
        if self.any_dirty() {
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

    fn open_scene(&mut self, path: PathBuf, output: &mut EditorOutput) {
        if let Some(index) = self.tab_of(&path) {
            output.switch_tab = Some(index); // already open in another tab
        } else if self.is_dirty() {
            self.confirm = Some(Confirm::Open(path));
        } else {
            output.open = Some(path);
        }
    }

    /// Save / discard / cancel, when unsaved changes are in the way.
    fn confirm_dialog(
        &mut self,
        ui: &Ui,
        scene: &Scene,
        player: &PlayerSettings,
        output: &mut EditorOutput,
    ) {
        let Some(confirm) = self.confirm.clone() else {
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("unsaved_changes")).show(ui.ctx(), |ui| {
            ui.label(RichText::new("UNSAVED CHANGES").strong());
            let names = match &confirm {
                Confirm::Open(_) => self.scene_path.display().to_string().to_uppercase(),
                Confirm::CloseTab(index) => self.tab_label(*index),
                Confirm::Close => self.dirty_paths().join(", "),
            };
            let verb = if names.contains(", ") { "HAVE" } else { "HAS" };
            ui.label(dim(&format!("{names} {verb} CHANGES THAT AREN'T SAVED.")));
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
            let saved = match &confirm {
                Confirm::Open(_) => {
                    self.save_tab(self.active_tab, scene, player, &mut output.settled)
                }
                Confirm::CloseTab(index) => {
                    self.save_tab(*index, scene, player, &mut output.settled)
                }
                Confirm::Close => self.save_all(scene, player, &mut output.settled),
            };
            if !saved {
                return; // saving failed; the error is in the top bar
            }
        }
        match confirm {
            Confirm::Open(path) => output.open = Some(path),
            Confirm::CloseTab(index) => output.close_tab = Some(index),
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
                emissive: Vec3::ZERO,
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
        after.zones.push(Zone::new(name.clone()));
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
                emissive: Vec3::ZERO,
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
        if self.model.is_some() {
            return None;
        }
        let index = self.selected.filter(|&i| i < scene.objects.len())?;
        // Commit first so undo restores the object with its latest edit.
        self.commit_pending(scene);
        let object = Box::new(scene.objects[index].clone());
        Some(self.push(scene, Edit::Remove { index, object }))
    }

    /// Copies the selected object in place, right after it in the list, and
    /// selects the copy (move it with the gizmo).
    pub fn duplicate_selected(&mut self, scene: &mut Scene) -> Option<Change> {
        if self.model.is_some() {
            return None;
        }
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
        if let Some(model) = &mut self.model {
            model.clear_selection();
            return false;
        }
        let settled = self.commit_pending(scene);
        self.selected = selected;
        if selected.is_some() {
            self.settings = None;
        }
        settled
    }

    #[allow(clippy::too_many_arguments)]
    fn top_bar(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        stats: &EditorStats,
        blink: bool,
        view: &View,
        output: &mut EditorOutput,
    ) {
        ui.horizontal(|ui| {
            if ui
                .small_button(if self.show_scene { "[≡]" } else { " ≡ " })
                .on_hover_text("SHOW / HIDE THE SCENE PANEL")
                .clicked()
            {
                self.show_scene = !self.show_scene;
            }
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
                    output.settled |= self.save(scene, player);
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
                    && let Some(change) = self.undo(scene, player)
                {
                    output.changed.push(change);
                    output.settled = true;
                }
                let redo = format!("REDO [{}]", self.history.redo_len());
                if ui
                    .add_enabled(self.history.redo_len() > 0, egui::Button::new(redo).small())
                    .clicked()
                    && let Some(change) = self.redo(scene, player)
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
                // Both bakes in one menu; the bar only says something when
                // a bake is running or missing.
                let bakes = [
                    (
                        "ACOUSTICS",
                        "SOUND",
                        &self.bake_status,
                        "REVERB PROBES IN EVERY ZONE VOLUME",
                    ),
                    (
                        "LIGHTING",
                        "LIGHT",
                        &self.light_status,
                        "BOUNCE, SKY AND EMISSIVE LIGHT INTO LIGHTMAPS AND PROBES",
                    ),
                ];
                let mut clicked = [false; 2];
                ui.menu_button("BAKE", |ui| {
                    for (i, (name, _, status, hint)) in bakes.iter().enumerate() {
                        let busy = status.starts_with("BAKING");
                        if ui
                            .add_enabled(!busy, egui::Button::new(format!("{name}  {status}")))
                            .on_hover_text(*hint)
                            .clicked()
                        {
                            clicked[i] = true;
                            ui.close();
                        }
                    }
                });
                output.bake |= clicked[0];
                output.bake_lighting |= clicked[1];
                let mut naming = None;
                ui.menu_button("PROJECT", |ui| {
                    if ui.button("NEW SCENE").clicked() {
                        naming = Some(new_scene::Naming::New);
                        ui.close();
                    }
                    if ui.button("SAVE SCENE AS").clicked() {
                        naming = Some(new_scene::Naming::SaveAs);
                        ui.close();
                    }
                    ui.separator();
                    let hint = "THE WHOLE PROJECT FOLDER IN ONE .GAMEPACK FILE NEXT TO IT,                                 TO OPEN IN ANOTHER EDITOR (SAVED FILES ONLY)";
                    if ui.button("EXPORT PACKAGE").on_hover_text(hint).clicked() {
                        output.export_package = true;
                        ui.close();
                    }
                });
                let notes: Vec<String> = bakes
                    .iter()
                    .filter(|(_, _, status, _)| !status.is_empty() && status.as_str() != "BAKED")
                    .map(|(_, short_name, status, _)| {
                        let short = status.replace("BAKE OUTDATED", "OUTDATED");
                        format!("{short_name} {short}")
                    })
                    .collect();
                if !notes.is_empty() {
                    ui.label(RichText::new(notes.join("  ")).small().color(theme::AMBER));
                }
                if let Some(purpose) = naming {
                    self.ask_name(purpose);
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
            });

            // Stats on the right, as many as fit: the rest are dropped
            // rather than drawn over the buttons.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button(if self.show_inspector {
                        "[≡]"
                    } else {
                        " ≡ "
                    })
                    .on_hover_text("SHOW / HIDE THE INSPECTOR")
                    .clicked()
                {
                    self.show_inspector = !self.show_inspector;
                }
                ui.add_space(8.0);
                let room = ui.available_width();
                let zone = stats.zone.as_deref().unwrap_or("-").to_uppercase();
                let fps = format!("{:.0}  {:.2} MS", stats.fps, stats.frame_ms);
                if room > 620.0 {
                    ui.label(format!("{} / {}", stats.drawn[0], stats.drawn[1]));
                    ui.label(dim("DRAWN"));
                    ui.add_space(12.0);
                }
                if room > 460.0 {
                    ui.label(zone).on_hover_text(format!(
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
                }
                if room > 300.0
                    && let Some(gpu) = stats.gpu_ms
                {
                    ui.label(format!("{gpu:.2} MS"));
                    ui.label(dim("GPU"));
                    ui.add_space(12.0);
                }
                if room > 140.0 {
                    ui.label(fps);
                    ui.label(dim("FPS"));
                }
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
        // Right to left: the X first, the field fills what's left (never
        // negative: the panel can be very narrow on its first frame).
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!self.search.is_empty(), egui::Button::new("X").small())
                    .on_hover_text("CLEAR THE SEARCH")
                    .clicked()
                {
                    self.search.clear();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("SEARCH NAME, ZONE OR COMPONENT")
                        .desired_width(ui.available_width().max(0.0)),
                );
            });
        });
        let query = self.search.trim().to_lowercase();
        let searching = !query.is_empty();
        ui.add_space(4.0);
        let mut clicked = None;
        let mut zone_clicked = None;
        let mut zone_action = None;
        let environment = self.settings == Some(SettingsTarget::Environment);
        if ui
            .selectable_label(environment, picked("ENVIRONMENT", environment))
            .clicked()
        {
            zone_clicked = Some(SettingsTarget::Environment);
        }
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
                    // A zone whose name matches shows all its objects.
                    let zone_matches = zone.name.to_lowercase().contains(&query);
                    let total = members.len();
                    let members: Vec<(usize, &Object)> = if zone_matches {
                        members
                    } else {
                        members
                            .into_iter()
                            .filter(|(_, o)| matches_search(o, &query))
                            .collect()
                    };
                    if searching && members.is_empty() && !zone_matches {
                        continue;
                    }
                    let shown = if members.len() == total {
                        total.to_string()
                    } else {
                        format!("{}/{total}", members.len())
                    };
                    let header = format!("{} [{shown}]", zone.name.to_uppercase());
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
                    let empty = total == 0;
                    let chosen = self.settings == Some(SettingsTarget::Zone(zone.name.clone()));
                    let response = egui::CollapsingHeader::new(picked(&header, chosen))
                        .id_salt(&zone.name)
                        .open((reveal || searching).then_some(true))
                        .show(ui, |ui| {
                            for (index, object) in members {
                                let selected = self.selected == Some(index);
                                let response =
                                    ui.selectable_label(selected, picked(&object.name, selected));
                                if response.clicked() {
                                    clicked = Some(index);
                                }
                                if selected && reveal {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                            }
                        });
                    if response.header_response.clicked() {
                        zone_clicked = Some(SettingsTarget::Zone(zone.name.clone()));
                    }
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
                if searching
                    && !scene.zones.iter().any(|z| {
                        z.name.to_lowercase().contains(&query)
                            || scene
                                .objects
                                .iter()
                                .any(|o| o.zone == z.name && matches_search(o, &query))
                    })
                {
                    ui.label(dim("NOTHING MATCHES"));
                }
            });
        self.reveal_selection = false;
        if let Some(index) = clicked {
            self.select(scene, Some(index));
        }
        if let Some(target) = zone_clicked {
            self.select(scene, None);
            self.settings = Some(target);
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

    /// The selected zone's or the environment's settings. Returns false if
    /// neither is selected.
    fn settings_inspector(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        output: &mut EditorOutput,
    ) -> bool {
        let Some(target) = self.settings.clone() else {
            return false;
        };
        let names = inspector::Names {
            objects: Vec::new(),
            zones: Vec::new(),
        };
        let before = scene.clone();
        match target {
            SettingsTarget::Environment => {
                let env = &mut scene.environment;
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.label(RichText::new("ENVIRONMENT").strong());
                        ui.label(dim("THE WHOLE SCENE; ZONES CAN OVERRIDE FOG AND GRADE"));
                        section(ui, "SUN");
                        sun_angles(ui, &mut env.sun_direction);
                        section(ui, "LIGHT, FOG AND LOOK");
                        inspector::fields_except(ui, env, &names, &["sun_direction"]);
                    });
            }
            SettingsTarget::Zone(name) => {
                let Some(z) = scene.zones.iter().position(|z| z.name == name) else {
                    self.settings = None;
                    return false;
                };
                let env = scene.environment.clone();
                let zone = &mut scene.zones[z];
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            inspector::field_label(ui, "ZONE");
                            ui.label(zone.name.to_uppercase());
                        });
                        ui.label(dim("WITHOUT ITS OWN, A ZONE USES THE ENVIRONMENT'S"));

                        section(ui, "FOG");
                        own_toggle(ui, "OWN FOG", &mut zone.fog, &env.fog);
                        if let Some(fog) = &mut zone.fog {
                            inspector::fields(ui, fog, &names);
                        }

                        section(ui, "COLOR GRADE");
                        own_toggle(ui, "OWN GRADE", &mut zone.grade, &env.grade);
                        if let Some(grade) = &mut zone.grade {
                            inspector::fields(ui, grade, &names);
                        }
                    });
            }
        }
        if *scene != before {
            if self.zone_pending.is_none() {
                self.commit_pending(&before);
                self.zone_pending = Some(Box::new(before));
            }
            output.changed.push(Change::Settings);
        }
        true
    }

    fn inspector(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        output: &mut EditorOutput,
    ) {
        ui.add_space(4.0);
        ui.label(RichText::new("INSPECTOR").strong());
        ui.add_space(4.0);
        let Some(index) = self.selected.filter(|&i| i < scene.objects.len()) else {
            if !self.settings_inspector(ui, scene, output) {
                ui.label(dim("NOTHING SELECTED"));
            }
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
        let player_before = player.clone();
        let project_dir = self.project_dir.as_path();

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
                if object.components.contains(&Component::PlayerStart) {
                    player_inspector(ui, player, project_dir);
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
        if *player != player_before {
            self.begin_player_edit(player_before, player);
            output.changed.push(Change::Player);
        }
    }

    /// Starts (or continues) an edit of the player settings; `before` is
    /// how they were before this frame's change.
    fn begin_player_edit(&mut self, before: PlayerSettings, now: &PlayerSettings) {
        match &mut self.player_pending {
            Some((_, after)) => **after = now.clone(),
            None => self.player_pending = Some((Box::new(before), Box::new(now.clone()))),
        }
    }

    /// Whether the inspector shows the player settings (Player Start is selected).
    pub fn shows_player(&self, scene: &Scene) -> bool {
        self.selected
            .and_then(|i| scene.objects.get(i))
            .is_some_and(|o| o.components.contains(&Component::PlayerStart))
    }

    /// Adds a sound file to the footsteps (and turns the default steps
    /// off), as one undo step.
    pub fn add_footstep(&mut self, scene: &Scene, player: &mut PlayerSettings, path: String) {
        self.commit_pending(scene);
        let before = player.clone();
        player.footsteps.use_default = false;
        player.footsteps.clips.push(path);
        self.history.record(Edit::SetPlayer {
            before: Box::new(before),
            after: Box::new(player.clone()),
        });
    }
}

/// Whether an object's name or one of its components (e.g. "light",
/// "door") contains `query` (lowercase), ignoring case.
fn matches_search(object: &Object, query: &str) -> bool {
    object.name.to_lowercase().contains(query)
        || object
            .components
            .iter()
            .any(|c| inspector::title(c).to_lowercase().contains(query))
}

/// The game's player settings (its `player.ron`), shown under Player Start:
/// movement, jump, body and camera as in the F1 tuning panel, then the
/// footsteps.
fn player_inspector(ui: &mut Ui, player: &mut PlayerSettings, project_dir: &Path) {
    section(ui, "PLAYER");
    ui.label(dim(
        "THE GAME'S PLAYER.RON: SHARED BY EVERY SCENE, SAVED WITH THE SCENE",
    ));
    tuning::sections(ui, player);
    section(ui, "FOOTSTEPS");
    footsteps_inspector(ui, &mut player.footsteps, project_dir);
}

fn footsteps_inspector(ui: &mut Ui, steps: &mut Footsteps, project_dir: &Path) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, "VOLUME");
        ui.add(
            egui::DragValue::new(&mut steps.volume)
                .range(0.0..=2.0)
                .speed(0.01)
                .max_decimals(2),
        );
        ui.label(dim("1 = AS RECORDED"));
    });
    on_off(ui, "USE DEFAULT", &mut steps.use_default);
    if steps.use_default {
        ui.label(dim("THE ENGINE'S STEPS, ONE PER FLOOR MATERIAL"));
        return;
    }

    if steps.clips.is_empty() {
        ui.label(dim(
            "NO SOUNDS YET: ADD ONE, OR DROP .WAV FILES ON THE WINDOW",
        ));
        ui.label(dim("UNTIL THEN THE DEFAULT STEPS PLAY"));
    }
    enum ListAction {
        Up(usize),
        Down(usize),
        Remove(usize),
    }
    let mut action = None;
    let count = steps.clips.len();
    for (i, clip) in steps.clips.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(dim(&format!("{:>2}", i + 1)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("X").on_hover_text("REMOVE").clicked() {
                    action = Some(ListAction::Remove(i));
                }
                if ui
                    .add_enabled(i + 1 < count, egui::Button::new("↓").small())
                    .clicked()
                {
                    action = Some(ListAction::Down(i));
                }
                if ui
                    .add_enabled(i > 0, egui::Button::new("↑").small())
                    .clicked()
                {
                    action = Some(ListAction::Up(i));
                }
                let text = if project_dir.join(clip).is_file() {
                    RichText::new(clip)
                } else {
                    RichText::new(format!("{clip} (MISSING)")).color(theme::ALERT)
                };
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(text).truncate());
                });
            });
        });
    }
    match action {
        Some(ListAction::Up(i)) => steps.clips.swap(i, i - 1),
        Some(ListAction::Down(i)) => steps.clips.swap(i, i + 1),
        Some(ListAction::Remove(i)) => {
            steps.clips.remove(i);
        }
        None => {}
    }
    ui.menu_button("ADD SOUND", |ui| {
        let files = sound_files(project_dir);
        if files.is_empty() {
            ui.label(dim("NO .WAV FILES IN THE PROJECT"));
        }
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for file in files {
                    if ui.button(&file).clicked() {
                        steps.clips.push(file);
                        ui.close();
                    }
                }
            });
    });

    ui.add_space(6.0);
    on_off(ui, "RANDOMIZE", &mut steps.randomize);
    if !steps.randomize {
        ui.label(dim("IN LIST ORDER: 1, 2, 3, 1, 2, 3..."));
        return;
    }
    let name = |order| match order {
        RandomOrder::True => "TRUE RANDOM",
        RandomOrder::Smart => "SMART RANDOM",
    };
    ui.horizontal(|ui| {
        inspector::field_label(ui, "ORDER");
        egui::ComboBox::from_id_salt("footstep_order")
            .selected_text(name(steps.random))
            .show_ui(ui, |ui| {
                for order in [RandomOrder::True, RandomOrder::Smart] {
                    ui.selectable_value(&mut steps.random, order, name(order));
                }
            });
    });
    ui.label(dim(match steps.random {
        RandomOrder::True => "ANY SOUND EACH STEP; THE SAME ONE CAN REPEAT",
        RandomOrder::Smart => {
            "NO RECENT REPEATS, AND EACH STEP SLIGHTLY VARIED IN PITCH AND LOUDNESS"
        }
    }));
}

/// [ON]/[OFF] row for a setting.
fn on_off(ui: &mut Ui, label: &str, value: &mut bool) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, label);
        let text = if *value { "[ON] " } else { "[OFF]" };
        if ui.small_button(text).clicked() {
            *value = !*value;
        }
    });
}

/// Every `.wav` in the project, as paths relative to it with `/`
/// separators, sorted. Hidden folders are skipped.
fn sound_files(project_dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut folders = vec![project_dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if path.is_dir() {
                if !hidden {
                    folders.push(path);
                }
            } else if import::is_sound(&path)
                && let Ok(relative) = path.strip_prefix(project_dir)
            {
                let parts: Vec<_> = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect();
                found.push(parts.join("/"));
            }
        }
    }
    found.sort();
    found
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
fn new_components() -> [(&'static str, Component); 11] {
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
        (
            "light source",
            Component::LightSource(LightSource::default()),
        ),
        ("fog volume", Component::FogVolume(FogVolume::default())),
        ("material", Component::Material(Material::default())),
    ]
}

/// A row's text in the hierarchy: inverted (amber background, text in the
/// background color) when it's the one being inspected.
fn picked(text: &str, selected: bool) -> RichText {
    let text = RichText::new(text);
    if selected {
        text.color(theme::BACKGROUND).background_color(theme::AMBER)
    } else {
        text
    }
}

/// What the inspector shows when it isn't showing an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsTarget {
    Environment,
    /// A zone, by name.
    Zone(String),
}

/// The sun's direction as compass angle and height above the horizon
/// (degrees), easier to aim than a vector.
fn sun_angles(ui: &mut Ui, direction: &mut Vec3) {
    // The direction the light travels; the sun sits the other way.
    let to_sun = -direction.normalize_or(Vec3::NEG_Y);
    let mut azimuth = to_sun.x.atan2(-to_sun.z).to_degrees();
    let mut elevation = to_sun.y.clamp(-1.0, 1.0).asin().to_degrees();
    let mut changed = false;
    ui.horizontal(|ui| {
        inspector::field_label(ui, "AZIMUTH");
        changed |= inspector::drag(ui, &mut azimuth, 0.5, 5.0);
    });
    ui.horizontal(|ui| {
        inspector::field_label(ui, "ELEVATION");
        changed |= inspector::drag(ui, &mut elevation, 0.25, 5.0);
    });
    if changed {
        let (az, el) = (
            azimuth.to_radians(),
            elevation.clamp(-89.0, 89.0).to_radians(),
        );
        let to_sun = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
        *direction = -to_sun;
    }
}

/// [ON]/[OFF] for a zone setting: on starts from the environment's value.
fn own_toggle<T: Clone>(ui: &mut Ui, label: &str, value: &mut Option<T>, fallback: &T) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, label);
        let text = if value.is_some() { "[ON] " } else { "[OFF]" };
        if ui.small_button(text).clicked() {
            *value = match value {
                Some(_) => None,
                None => Some(fallback.clone()),
            };
        }
    });
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
        "CLICK SELECT  W/E/R GIZMO  CTRL SNAP  RMB+WASD FLY  CTRL+D DUPLICATE  DEL DELETE  CTRL+Z/Y UNDO/REDO  CTRL+S SAVE  F5 PLAY  TAB/SHIFT+TAB PANELS  H ZONES"
    };
    hint_line(ui, viewport, text);
}

/// A line of key hints along the bottom of the viewport.
fn hint_line(ui: &Ui, viewport: Rect, text: &str) {
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
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(editor.selected, Some(0), "restored object is selected");
        assert_eq!(scene.objects[2].name, "object 2");

        editor.redo(&mut scene, &mut PlayerSettings::default());
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

        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);
        assert_eq!(editor.selected, None);
    }

    #[test]
    fn sun_angles_round_trip() {
        let dir = Vec3::new(0.62, -0.62, 0.3).normalize();
        let to_sun = -dir;
        let az = to_sun.x.atan2(-to_sun.z);
        let el = to_sun.y.asin();
        let back = -Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
        assert!((back - dir).length() < 1e-5);
    }

    #[test]
    fn search_matches_names_and_components_ignoring_case() {
        let scene = boxes(1);
        let object = &scene.objects[0];
        assert!(matches_search(object, &object.name.to_lowercase()));
        assert!(matches_search(object, "primitive"));
        assert!(!matches_search(object, "no such thing"));
    }

    #[test]
    fn a_player_settings_drag_is_one_undo_step() {
        let scene = boxes(1);
        let mut scene_after = scene.clone();
        let mut editor = editor();
        let mut player = PlayerSettings::default();
        // As the inspector does on each frame of a drag.
        for speed in [5.0, 6.0, 7.0] {
            let before = player.clone();
            player.movement.walk_speed = speed;
            editor.begin_player_edit(before, &player);
        }
        assert!(editor.is_dirty());
        assert!(editor.commit_pending(&scene));
        editor.add_footstep(&scene, &mut player, "audio/steps/a.wav".into());
        assert!(!player.footsteps.use_default);

        editor.undo(&mut scene_after, &mut player);
        assert!(player.footsteps.use_default && player.footsteps.clips.is_empty());
        assert_eq!(player.movement.walk_speed, 7.0);
        editor.undo(&mut scene_after, &mut player);
        assert_eq!(player, PlayerSettings::default());
        assert_eq!(scene_after, scene);
    }

    #[test]
    fn a_zone_fog_edit_is_one_undo_step() {
        let mut scene = boxes(1);
        let mut editor = editor();
        editor.settings = Some(SettingsTarget::Zone("test".into()));
        // As the zone inspector does: remember the scene, then edit live.
        editor.zone_pending = Some(Box::new(scene.clone()));
        scene.zones[0].fog = Some(crate::scene::Fog::default());
        scene.zones[0].fog.as_mut().unwrap().density = 0.3;
        assert!(editor.is_dirty());
        assert!(editor.commit_pending(&scene));
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene.zones[0].fog, None);
        // Selecting an object leaves the zone.
        editor.select(&scene, Some(0));
        assert_eq!(editor.settings, None);
    }

    #[test]
    fn renaming_a_zone_moves_its_objects_and_portals_and_undoes() {
        let mut scene = boxes(2);
        scene.zones.push(crate::scene::Zone::new("hall"));
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
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);

        editor.add_zone(&mut scene);
        assert_eq!(scene.zones.last().unwrap().name, "zone");
        assert!(editor.delete_zone(&mut scene, "zone").is_some());
        assert_eq!(scene, original);
    }

    #[test]
    fn added_primitive_lands_in_the_selected_zone_on_the_grid() {
        let mut scene = boxes(1);
        scene.zones.push(crate::scene::Zone::new("other"));
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
