//! Graph tabs: a gameplay graph open next to the scenes. The canvas fills
//! the middle, the project's flags are on the left and the selected node's
//! settings on the right (generated from reflection, like components).
//! Every change is an undo step of the tab's own.

use super::graph_canvas::{self, CanvasView, Marks};
use super::widgets::{Side, collapsed_panel, panel_header};
use super::{Editor, EditorOutput, dim, inspector, section};
use crate::camera::FlyCamera;
use crate::graph::flags::{FlagDef, FlagValue};
use crate::graph::{Graph, NodeId};
use crate::scene::Scene;
use crate::ui::theme;
use egui::{RichText, Ui};
use std::path::PathBuf;

/// An open graph and its editing state.
pub struct GraphDoc {
    pub graph: Graph,
    /// As last saved (or loaded).
    saved: Graph,
    undo: Vec<Graph>,
    redo: Vec<Graph>,
    /// The graph before the edit in progress.
    before: Option<Graph>,
    pub view: CanvasView,
    pub selected: Option<NodeId>,
    selected_link: Option<usize>,
}

impl GraphDoc {
    pub fn new(graph: Graph) -> Self {
        Self {
            saved: graph.clone(),
            graph,
            undo: Vec::new(),
            redo: Vec::new(),
            before: None,
            view: CanvasView::default(),
            selected: None,
            selected_link: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.graph != self.saved || self.before.is_some()
    }

    pub fn mark_saved(&mut self) {
        self.commit();
        self.saved = self.graph.clone();
    }

    /// Records the edit in progress as one undo step.
    fn commit(&mut self) {
        if let Some(before) = self.before.take()
            && before != self.graph
        {
            self.undo.push(before);
            self.redo.clear();
        }
    }

    /// Applies a change as an undo step of its own.
    fn change(&mut self, f: impl FnOnce(&mut Graph)) {
        self.commit();
        let before = self.graph.clone();
        f(&mut self.graph);
        if before != self.graph {
            self.undo.push(before);
            self.redo.clear();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.before.is_some()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.commit();
        if let Some(previous) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.graph, previous));
            self.keep_selection_valid();
        }
    }

    pub fn redo(&mut self) {
        self.commit();
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.graph, next));
            self.keep_selection_valid();
        }
    }

    fn keep_selection_valid(&mut self) {
        if self
            .selected
            .is_some_and(|id| self.graph.node(id).is_none())
        {
            self.selected = None;
        }
        if self
            .selected_link
            .is_some_and(|i| i >= self.graph.links.len())
        {
            self.selected_link = None;
        }
    }

    /// Deletes the selected node or connection.
    pub fn delete_selected(&mut self) {
        if let Some(id) = self.selected.take() {
            self.change(|g| g.remove(id));
        } else if let Some(i) = self.selected_link.take() {
            self.change(|g| {
                if i < g.links.len() {
                    g.links.remove(i);
                }
            });
        }
    }

    /// Copies a node a little to the side, without its connections, and
    /// selects the copy.
    pub fn duplicate(&mut self, id: NodeId) {
        let Some(node) = self.graph.node(id).cloned() else {
            return;
        };
        let mut copy = None;
        self.change(|g| {
            copy = Some(g.add(node.kind, node.position + glam::Vec2::new(30.0, 30.0)));
        });
        self.selected = copy;
        self.selected_link = None;
    }

    /// Adds a node near the middle of what the canvas shows, and selects
    /// it.
    pub fn add_in_view(&mut self, kind: crate::graph::NodeKind) {
        let middle = (egui::vec2(400.0, 250.0) - self.view.pan) / self.view.zoom;
        let mut added = None;
        self.change(|g| added = Some(g.add(kind, glam::Vec2::new(middle.x, middle.y))));
        self.selected = added;
        self.selected_link = None;
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_link = None;
    }
}

impl Editor {
    /// Opens `doc` (read from `path`) in a new tab and makes it active.
    /// The tab's scene is empty: nothing is drawn behind the canvas.
    pub fn open_graph_tab(
        &mut self,
        path: PathBuf,
        doc: GraphDoc,
        scene: &mut Scene,
        camera: &FlyCamera,
    ) {
        self.open_tab(path, Scene::default(), scene, camera);
        self.graph = Some(doc);
    }

    /// Writes the open graph to its file.
    pub(super) fn save_graph(&mut self) {
        let Some(doc) = &mut self.graph else {
            return;
        };
        let path = self.project_dir.join(&self.scene_path);
        doc.commit();
        match doc.graph.save(&path) {
            Ok(()) => {
                doc.mark_saved();
                log::info!("saved {}", path.display());
                self.message = Some((format!("SAVED {}", self.scene_path.display()), false));
                self.index.refresh(&self.project_dir, true);
            }
            Err(err) => {
                log::error!("{err}");
                self.message = Some((err.to_string(), true));
            }
        }
    }

    /// The graph tab's panels; the canvas takes what's left.
    pub(super) fn graph_ui(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        self.index.refresh(&self.project_dir, false);
        let Some(snapshot) = self.graph.as_ref().map(|d| d.graph.clone()) else {
            return;
        };

        if self.show_scene {
            egui::Panel::left("graph_flags")
                .resizable(true)
                .default_size(260.0)
                .min_size(220.0)
                .show(ui, |ui| {
                    self.show_scene = !panel_header(ui, "FLAGS", Side::Left, "TAB", |_| {});
                    self.flags_panel(ui);
                });
        } else {
            self.show_scene = collapsed_panel(ui, "graph_flags_strip", Side::Left, "FLAGS", "TAB");
        }
        if self.show_inspector {
            egui::Panel::right("graph_inspector")
                .resizable(true)
                .default_size(340.0)
                .min_size(300.0)
                .show(ui, |ui| {
                    self.show_inspector =
                        !panel_header(ui, "INSPECTOR", Side::Right, "SHIFT+TAB", |_| {});
                    self.node_inspector(ui);
                });
        } else {
            self.show_inspector = collapsed_panel(
                ui,
                "graph_inspector_strip",
                Side::Right,
                "INSPECTOR",
                "SHIFT+TAB",
            );
        }
        egui::Panel::top("editor_tabs").show(ui, |ui| self.tab_strip(ui, output));

        let rect = ui.available_rect_before_wrap();
        output.viewport = rect;
        let Some(doc) = &mut self.graph else {
            return;
        };
        let marks = Marks {
            selected: doc.selected,
            selected_link: doc.selected_link,
            active: &[],
        };
        let events = graph_canvas::show(ui, rect, &mut doc.graph, &mut doc.view, &marks, true);
        if let Some(select) = events.select {
            doc.selected = select;
            doc.selected_link = None;
        }
        if let Some(link) = events.select_link {
            doc.selected_link = Some(link);
            doc.selected = None;
        }
        // Changes made by dragging and typing this frame join the edit in
        // progress; it becomes an undo step once nothing is held.
        if doc.graph != snapshot && doc.before.is_none() {
            doc.before = Some(snapshot);
        }
        if let Some((kind, at)) = events.add {
            let mut added = None;
            doc.change(|g| added = Some(g.add(kind, at)));
            doc.selected = added;
        }
        if let Some(id) = events.duplicate {
            doc.duplicate(id);
        }
        if let Some(id) = events.delete {
            doc.selected = Some(id);
            doc.delete_selected();
        }
        let interacting =
            ui.ctx().egui_is_using_pointer() || ui.ctx().memory(|m| m.focused().is_some());
        if !interacting {
            doc.commit();
        }
        if doc.graph.nodes.len() <= 1 && doc.graph.links.is_empty() {
            let hint = "RIGHT-CLICK TO ADD NODES  ·  DRAG FROM A NODE'S RIGHT DOT ONTO ANOTHER TO CONNECT  ·  WHEEL ZOOM  ·  DRAG THE BACKGROUND TO PAN";
            ui.painter().text(
                rect.left_bottom() + egui::vec2(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                hint,
                egui::FontId::proportional(11.0),
                theme::AMBER_DIM,
            );
        }
    }

    /// The selected node's settings.
    fn node_inspector(&mut self, ui: &mut Ui) {
        let names = self.index.names.clone();
        let Some(doc) = &mut self.graph else {
            return;
        };
        let Some(id) = doc.selected.filter(|&id| doc.graph.node(id).is_some()) else {
            if let Some(i) = doc.selected_link {
                section(ui, "CONNECTION");
                if ui.button("DELETE CONNECTION  DEL").clicked() {
                    doc.selected_link = Some(i);
                    doc.delete_selected();
                }
            } else {
                ui.label(dim("PICK A NODE TO EDIT IT"));
                ui.add_space(8.0);
                ui.label(dim("EVERY GRAPH BEGINS AT ITS START NODES."));
                ui.label(dim("EVENTS WAIT; ACTIONS ACT AND GO ON."));
                ui.label(dim("REPLIES CONNECTED TO A LINE ARE OFFERED"));
                ui.label(dim("TO THE PLAYER."));
            }
            return;
        };
        let (mut duplicate, mut delete) = (false, false);
        if let Some(node) = doc.graph.node_mut(id) {
            section(ui, node.kind.title());
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    inspector::fields(ui, &mut node.kind, &names);
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        duplicate = ui.button("DUPLICATE  CTRL+D").clicked();
                        delete = ui.button("DELETE  DEL").clicked();
                    });
                });
        }
        if duplicate {
            doc.duplicate(id);
        }
        if delete {
            doc.delete_selected();
        }
    }

    /// The project's flags (`flags.ron`): add, rename, set the starting
    /// value, delete. Saved as soon as a change is finished.
    fn flags_panel(&mut self, ui: &mut Ui) {
        let mut defs = self.index.flags.clone();
        let before = defs.clone();
        ui.horizontal(|ui| {
            ui.label(dim("ADD"));
            for (text, value) in [
                ("YES/NO", FlagValue::Bool(false)),
                ("NUMBER", FlagValue::Number(0.0)),
                ("TEXT", FlagValue::Text(String::new())),
            ] {
                if ui.small_button(text).clicked() {
                    let mut n = defs.flags.len() + 1;
                    while defs.flags.iter().any(|f| f.name == format!("flag_{n}")) {
                        n += 1;
                    }
                    defs.flags.push(FlagDef {
                        name: format!("flag_{n}"),
                        value,
                    });
                }
            }
        });
        ui.add_space(4.0);
        if defs.flags.is_empty() {
            ui.label(dim("NO FLAGS YET: THEY HOLD THE GAME'S STATE"));
            ui.label(dim("(ABILITIES, STORY PROGRESS, CHOICES)"));
        }
        let mut remove = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, flag) in defs.flags.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("X").on_hover_text("DELETE").clicked() {
                            remove = Some(i);
                        }
                        ui.add(egui::TextEdit::singleline(&mut flag.name).desired_width(110.0));
                        match &mut flag.value {
                            FlagValue::Bool(b) => {
                                let text = if *b { "[TRUE] " } else { "[FALSE]" };
                                if ui.small_button(text).clicked() {
                                    *b = !*b;
                                }
                            }
                            FlagValue::Number(n) => {
                                ui.add(egui::DragValue::new(n).speed(0.1));
                            }
                            FlagValue::Text(t) => {
                                ui.add(egui::TextEdit::singleline(t).desired_width(80.0));
                            }
                        }
                    })
                    .response
                    .on_hover_text(format!(
                        "{} · STARTS AS {}",
                        flag.value.kind(),
                        flag.value
                    ));
                }
            });
        if let Some(i) = remove {
            defs.flags.remove(i);
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new("STARTING VALUES; SAVED TO FLAGS.RON")
                .small()
                .color(theme::AMBER_DIM),
        );
        if defs != before {
            for flag in &mut defs.flags {
                flag.name = flag.name.replace(' ', "_");
            }
            self.index.flags = defs;
            self.index.flags_unsaved = true;
        }
        // Written once typing has finished.
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if !typing && self.index.flags_unsaved {
            self.index.flags_unsaved = false;
            match self.index.flags.save(&self.project_dir) {
                Ok(()) => {
                    self.index.refresh(&self.project_dir, true);
                    self.message = Some(("SAVED FLAGS.RON".into(), false));
                }
                Err(err) => self.message = Some((err.to_string(), true)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::NodeKind;

    #[test]
    fn graph_edits_undo_and_redo() {
        let mut doc = GraphDoc::new(Graph::starter());
        let mut added = None;
        doc.change(|g| added = Some(g.add(NodeKind::SaveGame, glam::Vec2::ZERO)));
        assert!(doc.is_dirty());
        doc.selected = added;
        doc.delete_selected();
        assert_eq!(doc.graph.nodes.len(), 1);
        doc.undo();
        assert_eq!(doc.graph.nodes.len(), 2);
        doc.undo();
        assert!(!doc.is_dirty());
        doc.redo();
        doc.duplicate(added.unwrap());
        assert_eq!(doc.graph.nodes.len(), 3);
        doc.mark_saved();
        assert!(!doc.is_dirty());
    }
}
