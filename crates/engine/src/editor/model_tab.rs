//! Model tabs: the modeler inside the editor. The tab's live "scene" is a
//! small stage (just the model) that the renderer draws as usual; the
//! model itself is an [`EditMesh`] edited here, its vertices, edges and
//! faces drawn and picked over the viewport. The controls follow Blender's.

use super::gizmo::View;
use super::markers::{project, project_segment};
use super::model_tools::Tool;
use super::model_transform::{Axis, Kind, Modal};
use super::model_view::Orbit;
use super::{Editor, EditorOutput, Side, collapsed_panel, dim, panel_header};
use crate::modeler::glb;
use crate::modeler::mesh::{Edge, EditMesh, edge};
use crate::scene::{Component, Model, Object, Scene, Transform, Zone};
use crate::ui::theme;
use egui::{Color32, Pos2, Stroke, Ui};
use glam::{Vec2, Vec3};

use std::collections::BTreeSet;
use std::path::PathBuf;

/// What clicks select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectMode {
    Vertex,
    Edge,
    Face,
}

/// One thing under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Element {
    Vertex(u32),
    Edge(Edge),
    Face(usize),
}

/// Screen distance (points) within which a vertex or edge is picked.
const PICK_RADIUS: f32 = 10.0;
/// How far (points) the pointer moves before a press becomes a box
/// selection instead of a click.
const BOX_THRESHOLD: f32 = 4.0;

/// An open model.
pub struct ModelDoc {
    pub mesh: EditMesh,
    /// The mesh as last saved (or opened).
    pub(super) saved: EditMesh,
    pub(super) undo: Vec<EditMesh>,
    pub(super) redo: Vec<EditMesh>,
    pub mode: SelectMode,
    pub(super) verts: BTreeSet<u32>,
    pub(super) edges: BTreeSet<Edge>,
    pub(super) faces: BTreeSet<usize>,
    /// A move / rotate / scale in progress (G, R, S).
    pub(super) modal: Option<Modal>,
    /// Where the camera turns around.
    pub orbit: Orbit,
    /// What the stage's model object is drawn from in the asset cache: a
    /// name of its own, so scenes using the file don't see unsaved edits.
    pub key: String,
    /// The mesh changed since it was last handed to the renderer.
    pub changed: bool,
    /// The tools' settings.
    pub(super) tools: super::model_tools::ToolSettings,
    /// Where a box selection started (egui points).
    box_start: Option<Pos2>,
    /// The PARTS panel's name field: which part, and the name typed.
    pub(super) part_name: Option<(u32, String)>,
    /// The mesh before a MATERIAL slider started moving (its undo step).
    pub(super) surface_before: Option<EditMesh>,
}

impl ModelDoc {
    pub fn new(mesh: EditMesh, key: String) -> Self {
        Self {
            saved: mesh.clone(),
            mesh,
            undo: Vec::new(),
            redo: Vec::new(),
            mode: SelectMode::Face,
            verts: BTreeSet::new(),
            edges: BTreeSet::new(),
            faces: BTreeSet::new(),
            modal: None,
            orbit: Orbit::default(),
            key,
            changed: true,
            tools: Default::default(),
            box_start: None,
            part_name: None,
            surface_before: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.modal.is_some() || self.mesh != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.mesh.clone();
    }

    /// Records `before` as an undo step (the mesh has already changed).
    pub(super) fn record(&mut self, before: EditMesh) {
        if before != self.mesh {
            self.undo.push(before);
            self.redo.clear();
            self.changed = true;
        }
    }

    pub fn undo(&mut self) {
        self.cancel_transform();
        if let Some(before) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.mesh, before));
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        self.cancel_transform();
        if let Some(after) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.mesh, after));
            self.after_history();
        }
    }

    /// The mesh was swapped for another version: drop selections that no
    /// longer exist.
    pub(super) fn after_history(&mut self) {
        let (verts, faces) = (self.mesh.positions.len() as u32, self.mesh.faces.len());
        let edges: BTreeSet<Edge> = self.mesh.edges().into_iter().collect();
        self.verts.retain(|&v| v < verts);
        self.faces.retain(|&f| f < faces);
        self.edges.retain(|e| edges.contains(e));
        self.changed = true;
    }

    /// Keeps a transform in progress, as one undo step.
    pub(super) fn finish_drag(&mut self) {
        if let Some(modal) = self.modal.take() {
            self.record(modal.before);
        }
    }

    /// Puts back what a transform in progress changed.
    pub(super) fn cancel_transform(&mut self) {
        if let Some(modal) = self.modal.take() {
            self.mesh = modal.before;
            self.after_history();
        }
    }

    /// A move / rotate / scale is in progress (so Esc and number keys
    /// belong to it).
    pub fn transforming(&self) -> bool {
        self.modal.is_some()
    }

    /// Starts moving, turning or scaling the selection from `pointer`.
    fn start_transform(&mut self, kind: Kind, pointer: Pos2) {
        let verts = self.selected_verts();
        if !verts.is_empty() {
            self.modal = Some(Modal::new(
                kind,
                self.mesh.clone(),
                &self.mesh,
                &verts,
                pointer,
            ));
        }
    }

    /// E: extrudes the selected faces and moves the new ones along their
    /// normal; cancelling takes the extrusion back too.
    fn extrude_and_move(&mut self, pointer: Pos2) {
        if !self.can(&Tool::Extrude) {
            return;
        }
        let before = self.mesh.clone();
        let normal: Vec3 = self.faces.iter().map(|&f| self.mesh.face_normal(f)).sum();
        self.mesh.extrude(&self.faces, 0.0);
        self.changed = true;
        let verts = self.selected_verts();
        let mut modal = Modal::new(Kind::Move, before, &self.mesh, &verts, pointer);
        if let Some(normal) = normal.try_normalize() {
            modal.axis = Some(Axis::Normal(normal));
        }
        self.modal = Some(modal);
    }

    pub(super) fn clear_selection(&mut self) {
        self.verts.clear();
        self.edges.clear();
        self.faces.clear();
    }

    pub fn set_mode(&mut self, mode: SelectMode) {
        if mode != self.mode {
            self.mode = mode;
            self.clear_selection();
        }
    }

    pub(super) fn selection_count(&self) -> usize {
        match self.mode {
            SelectMode::Vertex => self.verts.len(),
            SelectMode::Edge => self.edges.len(),
            SelectMode::Face => self.faces.len(),
        }
    }

    /// Selects everything (in the current mode).
    pub(super) fn select_all(&mut self) {
        self.clear_selection();
        match self.mode {
            SelectMode::Vertex => self.verts = (0..self.mesh.positions.len() as u32).collect(),
            SelectMode::Edge => self.edges = self.mesh.edges().into_iter().collect(),
            SelectMode::Face => self.faces = (0..self.mesh.faces.len()).collect(),
        }
    }

    /// Selects `element` (only it, or added/removed with `toggle`); `None`
    /// clears the selection unless toggling.
    fn click(&mut self, element: Option<Element>, toggle: bool) {
        if !toggle {
            self.clear_selection();
        }
        fn flip<T: Ord>(set: &mut BTreeSet<T>, item: T) {
            if !set.remove(&item) {
                set.insert(item);
            }
        }
        match element {
            Some(Element::Vertex(v)) => flip(&mut self.verts, v),
            Some(Element::Edge(e)) => flip(&mut self.edges, e),
            Some(Element::Face(f)) => flip(&mut self.faces, f),
            None => {}
        }
    }

    /// The corners the selection moves.
    pub(super) fn selected_verts(&self) -> BTreeSet<u32> {
        match self.mode {
            SelectMode::Vertex => self.verts.clone(),
            SelectMode::Edge => self.edges.iter().flatten().copied().collect(),
            SelectMode::Face => self
                .faces
                .iter()
                .flat_map(|&f| self.mesh.faces[f].verts.iter().copied())
                .collect(),
        }
    }

    /// Whether `point` on the mesh can be seen from the camera (no face in
    /// front of it).
    pub(super) fn visible(&self, view: &View, point: Vec3) -> bool {
        let (eye, direction, distance) = view.camera.ray_to(point);
        match self.mesh.pick_face(eye, direction) {
            Some((_, t)) => t >= distance * 0.999 - 1e-4,
            None => true,
        }
    }

    /// The vertex, edge or face (by mode) under the pointer.
    fn pick(&self, view: &View, pointer: Pos2) -> Option<Element> {
        match self.mode {
            SelectMode::Vertex => {
                let mut best = None;
                for (i, &p) in self.mesh.positions.iter().enumerate() {
                    if let Some(at) = project(view, p) {
                        let d = at.distance(pointer);
                        if d < PICK_RADIUS
                            && best.is_none_or(|(_, b)| d < b)
                            && self.visible(view, p)
                        {
                            best = Some((i as u32, d));
                        }
                    }
                }
                best.map(|(v, _)| Element::Vertex(v))
            }
            SelectMode::Edge => {
                let mut best = None;
                for e in self.mesh.edges() {
                    let (a, b) = (
                        self.mesh.positions[e[0] as usize],
                        self.mesh.positions[e[1] as usize],
                    );
                    if let Some([sa, sb]) = project_segment(view, a, b) {
                        let d = segment_distance(pointer, sa, sb);
                        if d < PICK_RADIUS
                            && best.is_none_or(|(_, b)| d < b)
                            && self.visible(view, (a + b) / 2.0)
                        {
                            best = Some((e, d));
                        }
                    }
                }
                best.map(|(e, _)| Element::Edge(e))
            }
            SelectMode::Face => {
                let (origin, direction) = ray(view, pointer)?;
                self.mesh
                    .pick_face(origin, direction)
                    .map(|(f, _)| Element::Face(f))
            }
        }
    }

    /// Selects (or with `add`, adds) what lies inside `rect` and can be
    /// seen.
    fn box_select(&mut self, view: &View, rect: egui::Rect, add: bool) {
        if !add {
            self.clear_selection();
        }
        let inside = |p: Vec3| project(view, p).is_some_and(|at| rect.contains(at));
        match self.mode {
            SelectMode::Vertex => {
                for (i, &p) in self.mesh.positions.iter().enumerate() {
                    if inside(p) && self.visible(view, p) {
                        self.verts.insert(i as u32);
                    }
                }
            }
            SelectMode::Edge => {
                for e in self.mesh.edges() {
                    let (a, b) = (
                        self.mesh.positions[e[0] as usize],
                        self.mesh.positions[e[1] as usize],
                    );
                    if inside(a) && inside(b) && self.visible(view, (a + b) / 2.0) {
                        self.edges.insert(e);
                    }
                }
            }
            SelectMode::Face => {
                for f in 0..self.mesh.faces.len() {
                    // Just off the surface, so the face doesn't hide itself.
                    let center = self.mesh.face_center(f) + self.mesh.face_normal(f) * 1e-3;
                    if inside(center) && self.visible(view, center) {
                        self.faces.insert(f);
                    }
                }
            }
        }
    }

    /// Edges (and in vertex mode, vertices) over the viewport; the
    /// selection bright, what's under the pointer brighter.
    fn draw(&self, ui: &Ui, view: &View, hovered: Option<Element>) {
        let painter = ui.painter().with_clip_rect(view.image);
        let selected_fill = Color32::from_rgba_unmultiplied(255, 158, 44, 70);
        let hover_fill = Color32::from_rgba_unmultiplied(255, 158, 44, 35);
        for f in 0..self.mesh.faces.len() {
            let fill = if self.faces.contains(&f) {
                selected_fill
            } else if hovered == Some(Element::Face(f)) {
                hover_fill
            } else {
                continue;
            };
            let points: Option<Vec<Pos2>> =
                self.mesh.corners(f).map(|p| project(view, p)).collect();
            if let Some(points) = points {
                painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
            }
        }
        let face_edges: BTreeSet<Edge> = self
            .faces
            .iter()
            .flat_map(|&f| {
                let v = &self.mesh.faces[f].verts;
                (0..v.len()).map(move |i| edge(v[i], v[(i + 1) % v.len()]))
            })
            .collect();
        for e in self.mesh.edges() {
            let (a, b) = (
                self.mesh.positions[e[0] as usize],
                self.mesh.positions[e[1] as usize],
            );
            let Some(segment) = project_segment(view, a, b) else {
                continue;
            };
            let selected = self.edges.contains(&e)
                || face_edges.contains(&e)
                || (self.mode == SelectMode::Vertex
                    && self.verts.contains(&e[0])
                    && self.verts.contains(&e[1]));
            let stroke = if selected || hovered == Some(Element::Edge(e)) {
                Stroke::new(2.0, theme::AMBER)
            } else if self.visible(view, (a + b) / 2.0) {
                Stroke::new(1.0, theme::AMBER_DIM)
            } else {
                continue; // hidden behind the model
            };
            painter.line_segment(segment, stroke);
        }
        if self.mode == SelectMode::Vertex {
            for (i, &p) in self.mesh.positions.iter().enumerate() {
                let i = i as u32;
                let selected = self.verts.contains(&i);
                let hover = hovered == Some(Element::Vertex(i));
                if !(selected || hover || self.visible(view, p)) {
                    continue;
                }
                if let Some(at) = project(view, p) {
                    let (radius, color) = if selected || hover {
                        (4.0, theme::AMBER)
                    } else {
                        (2.5, theme::AMBER_DIM)
                    };
                    painter.circle_filled(at, radius, color);
                }
            }
        }
    }
}

/// The camera ray through a screen point (egui points).
fn ray(view: &View, pointer: Pos2) -> Option<(Vec3, Vec3)> {
    let image = view.image;
    if !image.contains(pointer) {
        return None;
    }
    let ndc = Vec2::new(
        (pointer.x - image.min.x) / image.width() * 2.0 - 1.0,
        1.0 - (pointer.y - image.min.y) / image.height() * 2.0,
    );
    Some(
        view.camera
            .ray(ndc, image.width() / image.height().max(1.0)),
    )
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let (ab, ap) = (b - a, p - a);
    let t = (ap.dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// The stage a model tab shows: the model at the origin, nothing else
/// (the axes are drawn over it).
pub fn stage(key: &str) -> Scene {
    let zone = "model".to_string();
    let mut scene = Scene {
        zones: vec![Zone::new(zone.clone())],
        objects: vec![Object {
            name: "model".into(),
            zone: zone.clone(),
            transform: Transform::default(),
            components: vec![Component::Model(Model {
                path: key.into(),
                color: Vec3::ONE,
                emissive: Vec3::ZERO,
                collision: false,
                parts: Vec::new(),
            })],
        }],
        ..Default::default()
    };
    scene.environment.fog.density = 0.0;
    scene
}

impl Editor {
    /// Opens `doc` (read from `path`) in a new model tab showing `stage`.
    pub fn open_model_tab(
        &mut self,
        path: PathBuf,
        doc: ModelDoc,
        stage: Scene,
        scene: &mut Scene,
        camera: &crate::camera::FlyCamera,
    ) {
        self.open_tab(path, stage, scene, camera);
        self.model = Some(doc);
    }

    /// The editor while a model tab is active.
    pub(super) fn model_ui(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        player: &crate::player::settings::PlayerSettings,
        view: &View,
        output: &mut EditorOutput,
    ) {
        // Keys. 1/2/3 (select modes) and the numpad (views) come from the
        // runtime, which can tell the number row from the numpad.
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        let modal_open = self.confirm.is_some() || self.naming.is_some();
        let pointer = ui
            .input(|i| i.pointer.latest_pos())
            .filter(|&p| view.image.contains(p));
        let mut run = None;
        if let Some(model) = &mut self.model
            && !typing
            && !view.looking
            && !modal_open
        {
            ui.input_mut(|i| {
                if let Some(modal) = &mut model.modal {
                    for (key, axis) in [(egui::Key::X, 0), (egui::Key::Y, 1), (egui::Key::Z, 2)] {
                        if i.consume_key(egui::Modifiers::NONE, key) {
                            modal.lock(axis);
                        }
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace) {
                        modal.backspace();
                    }
                    for event in &i.events {
                        if let egui::Event::Text(text) = event {
                            modal.type_text(text);
                        }
                    }
                    return;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::A) {
                    model.select_all();
                }
                if i.consume_key(egui::Modifiers::ALT, egui::Key::A) {
                    model.clear_selection();
                }
                if let Some(pointer) = pointer {
                    for (key, kind) in [
                        (egui::Key::G, Kind::Move),
                        (egui::Key::R, Kind::Rotate),
                        (egui::Key::S, Kind::Scale),
                    ] {
                        if i.consume_key(egui::Modifiers::NONE, key) {
                            model.start_transform(kind, pointer);
                        }
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::E) {
                        model.extrude_and_move(pointer);
                    }
                }
                for (modifiers, key, tool) in [
                    (egui::Modifiers::NONE, egui::Key::I, Tool::Inset),
                    (egui::Modifiers::NONE, egui::Key::X, Tool::Delete),
                    (egui::Modifiers::NONE, egui::Key::M, Tool::Merge),
                    (egui::Modifiers::CTRL, egui::Key::R, Tool::LoopCut),
                ] {
                    if i.consume_key(modifiers, key) {
                        run = Some(tool);
                    }
                }
            });
        }

        if self.show_scene {
            egui::Panel::left("editor_scene")
                .resizable(true)
                .default_size(220.0)
                .min_size(200.0)
                .show(ui, |ui| {
                    self.show_scene = !panel_header(ui, "MODEL", Side::Left, "TAB", |_| {});
                    self.model_panel(ui);
                });
        } else {
            self.show_scene =
                collapsed_panel(ui, "editor_scene_strip", Side::Left, "MODEL PANEL", "TAB");
        }
        if !self.show_inspector {
            self.show_inspector = collapsed_panel(
                ui,
                "editor_inspector_strip",
                Side::Right,
                "TOOLS PANEL",
                "SHIFT+TAB",
            );
        } else if let Some(model) = &mut self.model {
            let project_dir = &self.project_dir;
            let show = &mut self.show_inspector;
            egui::Panel::right("editor_inspector")
                .resizable(true)
                .default_size(300.0)
                .min_size(280.0)
                .show(ui, |ui| {
                    *show = !panel_header(ui, "TOOLS", Side::Right, "SHIFT+TAB", |_| {});
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let (tool, _) = model.tools_panel(ui, project_dir);
                        run = tool.or(run.take());
                    });
                });
        }
        if let Some(tool) = run
            && let Some(model) = &mut self.model
            && let Some(message) = model.apply(tool)
        {
            self.message = Some((message, false));
        }
        egui::Panel::top("editor_tabs").show(ui, |ui| self.tab_strip(ui, output));

        let viewport = ui.available_rect_before_wrap();
        let modal = self.confirm.is_some() || self.naming.is_some();
        let Some(model) = &mut self.model else {
            return;
        };

        model.draw_axes(ui, view);
        let on_gizmo = !modal && model.view_gizmo(ui, view);

        // A move / rotate / scale in progress: the selection follows the
        // pointer; click or Enter keeps it, right-click or Esc puts it back.
        let pointer = ui.input(|i| i.pointer.latest_pos());
        if let Some(modal) = &mut model.modal {
            let (snap, confirm, cancel) = ui.input_mut(|i| {
                let confirm = i.pointer.primary_pressed()
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                let cancel = i.pointer.secondary_pressed()
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                (i.modifiers.ctrl, confirm, cancel)
            });
            if let Some(pointer) = pointer {
                modal.update(&mut model.mesh, view, pointer, snap);
                model.changed = true;
            }
            modal.draw(ui, view, pointer);
            if cancel {
                model.cancel_transform();
            } else if confirm {
                model.finish_drag();
            }
            model.draw(ui, view, None);
            model.box_start = None;
            self.confirm_dialog(ui, scene, player, output);
            self.name_dialog(ui, output);
            output.viewport = viewport;
            self.hints(ui, viewport);
            return;
        }

        let over_viewport = pointer.is_some_and(|p| viewport.contains(p) && view.image.contains(p));
        let hovered = pointer
            .filter(|_| over_viewport && !modal && !view.looking && !on_gizmo)
            .and_then(|p| model.pick(view, p));
        model.draw(ui, view, hovered);

        // Click selects what's under the pointer; dragging from empty space
        // or anything selects what's inside the box.
        let free = over_viewport && !modal && !view.looking && !on_gizmo;
        let (pressed, released, shift) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.modifiers.shift,
            )
        });
        if pressed && free {
            model.box_start = pointer;
        }
        if let (Some(start), Some(now)) = (model.box_start, pointer) {
            let rect = egui::Rect::from_two_pos(start, now);
            let dragged = rect.size().max_elem() > BOX_THRESHOLD;
            if dragged {
                let painter = ui.painter().with_clip_rect(view.image);
                painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(255, 158, 44, 20));
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0, theme::AMBER),
                    egui::StrokeKind::Inside,
                );
            }
            if released {
                model.box_start = None;
                if dragged {
                    model.box_select(view, rect, shift);
                } else {
                    model.click(hovered, shift);
                }
            }
        }

        self.confirm_dialog(ui, scene, player, output);
        self.name_dialog(ui, output);
        output.viewport = viewport;
        self.hints(ui, viewport);
    }

    /// The left panel: what's selected and how to select.
    fn model_panel(&mut self, ui: &mut Ui) {
        let Some(model) = &mut self.model else {
            return;
        };
        super::section(ui, "SELECT");
        ui.horizontal(|ui| {
            for (label, mode) in [
                ("VERTEX 1", SelectMode::Vertex),
                ("EDGE 2", SelectMode::Edge),
                ("FACE 3", SelectMode::Face),
            ] {
                if ui.selectable_label(model.mode == mode, label).clicked() {
                    model.set_mode(mode);
                }
            }
        });
        ui.label(dim(&format!("{} SELECTED", model.selection_count())));
        ui.add_space(8.0);
        super::section(ui, "MESH");
        ui.label(dim(&format!(
            "{} VERTICES  {} EDGES  {} FACES",
            model.mesh.positions.len(),
            model.mesh.edges().len(),
            model.mesh.faces.len()
        )));
        let triangles: usize = model
            .mesh
            .faces
            .iter()
            .map(|f| f.verts.len().saturating_sub(2))
            .sum();
        ui.label(dim(&format!("{triangles} TRIANGLES")));
        model.parts_panel(ui);
    }

    /// Saves the active model tab. Returns whether it saved.
    pub(super) fn save_model(&mut self) -> bool {
        let Some(model) = &mut self.model else {
            return false;
        };
        model.finish_drag();
        let path = self.project_dir.join(&self.scene_path);
        match glb::save(&path, &model.mesh, &self.project_dir) {
            Ok(()) => {
                model.mark_saved();
                log::info!("saved {}", path.display());
                self.message = Some((format!("SAVED {}", self.scene_path.display()), false));
                true
            }
            Err(err) => {
                log::error!("{err}");
                self.message = Some((err.to_string(), true));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_undo_and_dirty_state() {
        let mut doc = ModelDoc::new(EditMesh::cube(Vec3::ONE), "test".into());
        assert!(!doc.is_dirty());
        doc.click(Some(Element::Face(3)), false);
        doc.click(Some(Element::Face(1)), true);
        assert_eq!(doc.selected_verts().len(), 6, "two faces sharing an edge");
        doc.click(Some(Element::Face(1)), true);
        assert_eq!(doc.selection_count(), 1);

        // Lift the top face, as a move would.
        let before = doc.mesh.clone();
        for v in doc.selected_verts() {
            doc.mesh.positions[v as usize].y += 1.0;
        }
        doc.record(before);
        assert!(doc.is_dirty());
        doc.undo();
        assert!(!doc.is_dirty());
        doc.redo();
        assert!(doc.mesh.positions.iter().any(|p| p.y > 1.0));
        doc.mark_saved();
        assert!(!doc.is_dirty());

        doc.select_all();
        assert_eq!(doc.selection_count(), 6);
        doc.clear_selection();
        assert_eq!(doc.selection_count(), 0);
    }
}
