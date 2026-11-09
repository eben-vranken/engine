//! The LOGIC workspace: a graph open in a tab. On the left every graph,
//! grouped by owner (one per scene, one per character, others), and the
//! flags (global: every scene's graph reads and sets the same ones); in the middle the canvas or the CONVERSATION
//! view; on the right the graph's owner and what uses it, then the selected
//! node's settings (generated from reflection, like components). Every
//! change is an undo step of the tab's own.

use super::conversation::{self, TestRun};
use super::graph_canvas::{self, CanvasView, Marks};
use super::project_index::{Go, Owner};
use super::widgets::{Side, collapsed_panel, panel_header};
use super::{Editor, EditorOutput, dim, inspector, section};
use crate::graph::flags::{FlagDef, FlagValue};
use crate::graph::{Graph, NodeId};
use crate::ui::theme;
use egui::{RichText, Ui};
use std::collections::BTreeMap;
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
    /// The CONVERSATION view instead of the canvas.
    conversation: bool,
    /// FIND NODE's text.
    find: String,
    /// TEST CONVERSATION, while it runs.
    test: Option<TestRun>,
    /// PREVIEW AS: yes/no flags switched for the preview and the test.
    preview: BTreeMap<String, bool>,
}

impl GraphDoc {
    /// Links object references to the graph's scene. Not an edit: the
    /// saved copy is linked too, so the tab doesn't show as changed.
    pub fn link_objects(&mut self, objects: &[crate::scene::ObjectRef]) {
        self.graph.link_objects(objects);
        self.saved.link_objects(objects);
    }

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
            conversation: false,
            find: String::new(),
            test: None,
            preview: BTreeMap::new(),
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
    /// The tab's scene is empty: nothing is drawn behind the canvas. A
    /// scene's graph has its object references linked to that scene's
    /// objects (ids filled in, names brought up to date).
    pub fn open_graph_tab(&mut self, path: PathBuf, mut doc: GraphDoc) {
        self.index.refresh(&self.project_dir, false);
        if let Some(objects) = self.scene_objects_of(&crate::graph::key(&path)) {
            doc.link_objects(&objects);
        }
        // OPEN AT NODE (from Play's test drawer): select and show it.
        if let Some((_, node)) = self.select_on_open.take().filter(|(p, _)| *p == path) {
            doc.selected = Some(node);
            graph_canvas::center_on(&doc.graph, &mut doc.view, node, egui::vec2(900.0, 600.0));
        }
        self.open_logic(path, super::logic_docs::LogicDoc::Graph(doc));
    }

    /// The objects of the scene a graph belongs to (`None`: not a scene's
    /// graph, or the scene isn't known).
    fn scene_objects_of(&self, graph: &str) -> Option<Vec<crate::scene::ObjectRef>> {
        let scene = crate::graph::scene_of_graph(graph)?;
        self.index.scene_objects.get(&scene).cloned()
    }

    /// Writes the open graph to its file.
    pub(super) fn save_graph(&mut self) {
        let Some(doc) = &mut self.graph else {
            return;
        };
        let path = self.project_dir.join(&self.logic_path);
        doc.commit();
        match doc.graph.save(&path) {
            Ok(()) => {
                doc.mark_saved();
                log::info!("saved {}", path.display());
                self.message = Some((format!("SAVED {}", self.logic_path.display()), false));
                self.index.refresh(&self.project_dir, true);
            }
            Err(err) => {
                log::error!("{err}");
                self.message = Some((err.to_string(), true));
            }
        }
    }

    /// The LOGIC workspace's panels; the canvas takes what's left.
    pub(super) fn graph_ui(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        self.index.refresh(&self.project_dir, false);
        let Some(snapshot) = self.graph.as_ref().map(|d| d.graph.clone()) else {
            return;
        };
        let path = crate::graph::key(&self.logic_path);

        self.logic_side_panel(ui, &path, output);
        if self.show_inspector {
            egui::Panel::right("graph_inspector")
                .resizable(true)
                .default_size(340.0)
                .min_size(300.0)
                .show(ui, |ui| {
                    self.show_inspector =
                        !panel_header(ui, "INSPECTOR", Side::Right, "SHIFT+TAB", |_| {});
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.graph_card(ui, &path, output);
                            self.node_inspector(ui);
                        });
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
        let problems = self
            .graph
            .as_ref()
            .map(|d| self.index.problems(&path, &d.graph))
            .unwrap_or_default();
        egui::Panel::bottom("graph_problems").show(ui, |ui| {
            ui.horizontal(|ui| {
                let nodes = snapshot.nodes.len();
                let links = snapshot.links.len();
                if problems.is_empty() {
                    ui.label(dim(&format!(
                        "{nodes} NODES · {links} CONNECTIONS · NO PROBLEMS"
                    )));
                } else {
                    ui.label(
                        RichText::new(format!("{} PROBLEMS:", problems.len())).color(theme::ALERT),
                    )
                    .on_hover_text(problems.join("\n"));
                    ui.label(RichText::new(&problems[0]).color(theme::ALERT));
                }
            });
        });

        let index_names = self.index.character_names.clone();
        let start_flags = self.index.flags.start();
        let style = self.index.dialogue.clone();
        let project_dir = self.project_dir.clone();
        let Some(doc) = &mut self.graph else {
            return;
        };
        // The view switch, zoom and FIND over the middle.
        egui::Panel::top("graph_toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (on, text) in [(false, " CANVAS "), (true, " CONVERSATION ")] {
                    let chosen = doc.conversation == on;
                    let label = if chosen {
                        RichText::new(text).color(theme::BACKGROUND).strong()
                    } else {
                        RichText::new(text).color(theme::AMBER_DIM)
                    };
                    let mut button = egui::Button::new(label).small();
                    if chosen {
                        button = button.fill(theme::AMBER);
                    }
                    if ui.add(button).clicked() {
                        doc.conversation = on;
                    }
                }
                if !doc.conversation {
                    ui.add_space(12.0);
                    if ui
                        .small_button(format!("{:.0}%", doc.view.zoom * 100.0))
                        .on_hover_text("BACK TO 100%")
                        .clicked()
                    {
                        doc.view.zoom = 1.0;
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut doc.find)
                            .hint_text("FIND NODE")
                            .desired_width(180.0),
                    );
                }
            });
        });

        let rect = ui.available_rect_before_wrap();
        output.viewport = rect;
        let find = doc.find.trim().to_lowercase();
        let found: Vec<NodeId> = if find.is_empty() {
            Vec::new()
        } else {
            doc.graph
                .nodes
                .iter()
                .filter(|n| {
                    n.kind.title().to_lowercase().contains(&find)
                        || n.kind.summary().join(" ").to_lowercase().contains(&find)
                })
                .map(|n| n.id)
                .collect()
        };
        if doc.conversation {
            let flags = conversation::preview_flags(&start_flags, &doc.preview);
            ui.painter().rect_filled(rect, 0.0, theme::BACKGROUND);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)));
            child.columns(2, |cols| {
                egui::ScrollArea::vertical()
                    .id_salt("conversation_script")
                    .auto_shrink([false, false])
                    .show(&mut cols[0], |ui| {
                        ui.label(dim(
                            "THE SAME GRAPH, READ AS A SCRIPT · CLICK AN ENTRY TO EDIT IT",
                        ));
                        ui.add_space(6.0);
                        conversation::script(ui, &doc.graph, &index_names, &mut doc.selected, None);
                    });
                let ui = &mut cols[1];
                ui.label(RichText::new("PREVIEW · IN THE GAME'S STYLE").strong());
                ui.add_space(4.0);
                let mut ended = false;
                match &mut doc.test {
                    Some(run) => {
                        ended = !conversation::test_controls(ui, run, &index_names, &style);
                        if ui.button("STOP TEST").clicked() {
                            ended = true;
                        }
                    }
                    None => {
                        if let Some((who, text, replies)) =
                            conversation::preview_of(&doc.graph, doc.selected, &index_names, &flags)
                        {
                            conversation::draw_box(ui, &style, &who, &text, &replies, 0);
                        } else {
                            ui.label(dim("NO LINES YET: ADD A LINE NODE"));
                        }
                        if ui
                            .button(RichText::new("TEST CONVERSATION").strong())
                            .clicked()
                        {
                            doc.test = Some(TestRun::new(
                                project_dir.clone(),
                                &path,
                                doc.graph.clone(),
                                flags.clone(),
                            ));
                        }
                    }
                }
                if ended {
                    doc.test = None;
                }
                let used = conversation::condition_flags(&doc.graph);
                if !used.is_empty() {
                    ui.add_space(8.0);
                    ui.label(dim("PREVIEW AS (FLAGS SWITCHED FOR THE PREVIEW ONLY)"));
                    ui.horizontal_wrapped(|ui| {
                        for flag in used {
                            let on = doc
                                .preview
                                .get(&flag)
                                .copied()
                                .unwrap_or_else(|| start_flags.is_true(&flag));
                            let text = format!("{} {flag}", if on { "[X]" } else { "[ ]" });
                            if ui.small_button(text).clicked() {
                                doc.preview.insert(flag, !on);
                            }
                        }
                    });
                }
            });
        } else {
            let marks = Marks {
                selected: doc.selected,
                selected_link: doc.selected_link,
                active: &[],
                found: &found,
            };
            doc.view.scene_graph = crate::graph::scene_of_graph(&path).is_some();
            let events = graph_canvas::show(ui, rect, &mut doc.graph, &mut doc.view, &marks, true);
            if let Some(select) = events.select {
                doc.selected = select;
                doc.selected_link = None;
            }
            if let Some(link) = events.select_link {
                doc.selected_link = Some(link);
                doc.selected = None;
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
            if ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !found.is_empty()
                && ui.ctx().memory(|m| m.focused().is_some())
            {
                doc.selected = Some(found[0]);
                graph_canvas::center_on(&doc.graph, &mut doc.view, found[0], rect.size());
            }
            if doc.graph.nodes.len() <= 1 && doc.graph.links.is_empty() {
                let hint = "SPACE OR RIGHT-CLICK TO ADD NODES  ·  DRAG FROM A NODE'S RIGHT DOT ONTO ANOTHER TO CONNECT  ·  WHEEL ZOOM  ·  DRAG THE BACKGROUND TO PAN";
                ui.painter().text(
                    rect.left_bottom() + egui::vec2(10.0, -10.0),
                    egui::Align2::LEFT_BOTTOM,
                    hint,
                    egui::FontId::proportional(11.0),
                    theme::AMBER_DIM,
                );
            }
        }
        // Changes made by dragging and typing this frame join the edit in
        // progress; it becomes an undo step once nothing is held.
        if doc.graph != snapshot && doc.before.is_none() {
            doc.before = Some(snapshot);
        }
        let interacting =
            ui.ctx().egui_is_using_pointer() || ui.ctx().memory(|m| m.focused().is_some());
        if !interacting {
            doc.commit();
        }
        // SPACE: add a node (not while typing).
        if !interacting && ui.input(|i| i.key_pressed(egui::Key::Space)) {
            self.open_palette(Some("NODES"));
        }
    }

    /// Every graph in the project by owner, and each scene's slot for a
    /// graph of its own.
    /// LOGIC's left panel, for graph and script tabs alike: the graphs by
    /// owner, the scripts, and the flags.
    pub(super) fn logic_side_panel(
        &mut self,
        ui: &mut Ui,
        current: &str,
        output: &mut EditorOutput,
    ) {
        if !self.show_scene {
            self.show_scene =
                collapsed_panel(ui, "graph_browser_strip", Side::Left, "LOGIC", "TAB");
            return;
        }
        egui::Panel::left("graph_browser")
            .resizable(true)
            .default_size(280.0)
            .min_size(240.0)
            .show(ui, |ui| {
                self.show_scene = !panel_header(ui, "LOGIC", Side::Left, "TAB", |ui| {
                    if ui
                        .small_button("+ SCRIPT")
                        .on_hover_text("A SCRIPT: AN ABILITY, OR TO RUN ON AN OBJECT")
                        .clicked()
                    {
                        self.ask_name(super::name_dialog::Naming::Script);
                    }
                    if ui
                        .small_button("+ GRAPH")
                        .on_hover_text("A GRAPH STARTED BY RUN GRAPH (A SCENE'S OWN GRAPH: + ADD GRAPH BESIDE THE SCENE)")
                        .clicked()
                    {
                        self.ask_name(super::name_dialog::Naming::Graph);
                    }
                });
                // Graphs and scripts on top (up to about half the height),
                // flags below; each scrolls on its own.
                self.graph_browser(ui, current, output);
                ui.add_space(8.0);
                super::section(ui, "FLAGS · GLOBAL");
                self.flags_panel(ui);
            });
    }

    fn graph_browser(&mut self, ui: &mut Ui, current: &str, output: &mut EditorOutput) {
        let mut groups: BTreeMap<u8, Vec<(String, String)>> = BTreeMap::new();
        for (path, graph) in &self.index.graphs {
            let owner = self.index.owner(path);
            let order = match owner {
                Owner::Scene(_) => 1,
                Owner::Character { .. } => 2,
                Owner::Other => 3,
            };
            groups
                .entry(order)
                .or_default()
                .push((path.clone(), format!("{} NODES", graph.nodes.len())));
        }
        let scenes: Vec<String> = self.index.names.scenes.clone();
        let scripts: Vec<String> = self.index.names.scripts.clone();
        let mut create = None;
        egui::ScrollArea::vertical()
            .id_salt("graph_browser_list")
            .auto_shrink([false, true])
            .max_height(ui.available_height() * 0.55)
            .show(ui, |ui| {
                for (order, title, runs) in [
                    (1, "SCENES", "RUN WHILE LOADED"),
                    (2, "CHARACTERS", "RUN WHEN TALKED TO"),
                    (3, "OTHER", "STARTED BY RUN GRAPH"),
                ] {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("{title} · {runs}")).small().color(theme::AMBER_DIM));
                    let entries = groups.get(&order).cloned().unwrap_or_default();
                    if order == 1 {
                        // Every scene: its graph, or a button to make one.
                        for scene in &scenes {
                            let graph = crate::graph::scene_graph(scene);
                            let name = scene.trim_start_matches("scenes/").trim_end_matches(".ron");
                            ui.horizontal(|ui| {
                                if self.index.graphs.contains_key(&graph) {
                                    let chosen = graph == current;
                                    if ui.selectable_label(chosen, name.to_uppercase()).clicked() {
                                        output.open_graph = Some(PathBuf::from(&graph));
                                    }
                                } else {
                                    ui.label(dim(&name.to_uppercase()));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui
                                            .small_button("+ ADD GRAPH")
                                            .on_hover_text("A GRAPH OF THIS SCENE'S OWN: STARTS WHEN IT LOADS, CARRIES ON WHEN THE PLAYER RETURNS")
                                            .clicked()
                                        {
                                            create = Some(graph.clone());
                                        }
                                    });
                                }
                            });
                        }
                        continue;
                    }
                    if entries.is_empty() {
                        let hint = match order {
                            2 => "NONE: GIVE A CHARACTER A DIALOGUE",
                            _ => "NONE",
                        };
                        ui.label(dim(hint));
                    }
                    for (path, detail) in entries {
                        let chosen = path == current;
                        let name = path.trim_start_matches("graphs/").trim_end_matches(".ron").to_uppercase();
                        ui.horizontal(|ui| {
                            if ui.selectable_label(chosen, name).clicked() {
                                output.open_graph = Some(PathBuf::from(&path));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(detail).small().color(theme::AMBER_DIM));
                            });
                        });
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new("SCRIPTS · ABILITIES, OR ON OBJECTS")
                        .small()
                        .color(theme::AMBER_DIM),
                );
                if scripts.is_empty() {
                    ui.label(dim("NONE: + SCRIPT ABOVE"));
                }
                for path in &scripts {
                    let name = crate::graph::script_name(path).to_uppercase();
                    let uses = self.index.script_users(path).len();
                    ui.horizontal(|ui| {
                        if ui.selectable_label(path == current, name).clicked() {
                            output.open_graph = Some(PathBuf::from(path));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let text = if uses == 0 { "UNUSED".into() } else { format!("{uses} USE{}", if uses == 1 { "" } else { "S" }) };
                            ui.label(RichText::new(text).small().color(theme::AMBER_DIM));
                        });
                    });
                }
            });
        if let Some(graph) = create {
            match Graph::starter().save(&self.project_dir.join(&graph)) {
                Ok(()) => {
                    self.index.refresh(&self.project_dir, true);
                    output.open_graph = Some(PathBuf::from(graph));
                }
                Err(err) => self.show_error(err.to_string()),
            }
        }
    }

    /// The open graph: who owns it, when it runs, what uses it.
    fn graph_card(&mut self, ui: &mut Ui, path: &str, output: &mut EditorOutput) {
        let owner = self.index.owner(path);
        section(ui, "GRAPH");
        egui::Grid::new("graph_card")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label(dim("OWNER"));
                ui.label(owner.describe());
                ui.end_row();
                ui.label(dim("RUNS"));
                ui.add(egui::Label::new(dim(owner.runs())).wrap());
                ui.end_row();
                ui.label(dim("FILE"));
                ui.label(dim(path));
                ui.end_row();
            });
        let uses = self.index.used_by(path);
        section(ui, &format!("USED BY  {}", uses.len()));
        if uses.is_empty() {
            ui.label(dim("NOTHING USES THIS GRAPH YET:"));
            ui.label(dim("PICK IT AS A CHARACTER'S DIALOGUE"));
            ui.label(dim("OR IN A RUN GRAPH NODE."));
        }
        for u in uses {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(&u.text).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("OPEN").clicked() {
                        match &u.go {
                            Go::Scene(scene) => {
                                let scene = PathBuf::from(scene);
                                match self.tab_of(&scene) {
                                    Some(index) => output.switch_tab = Some(index),
                                    None => output.new_tab = Some(scene),
                                }
                            }
                            Go::Graph(graph) => output.open_graph = Some(PathBuf::from(graph)),
                        }
                    }
                });
            });
        }
    }

    /// The selected node's settings.
    fn node_inspector(&mut self, ui: &mut Ui) {
        let mut names = self.index.names.clone();
        // A scene's graph picks from that scene's objects, by id.
        let path = crate::graph::key(&self.logic_path);
        if let Some(mut objects) = self.scene_objects_of(&path) {
            objects.sort_by(|a, b| a.name.cmp(&b.name));
            names.objects = objects.iter().map(|o| o.name.clone()).collect();
            names.object_ids = objects.iter().map(|o| o.id).collect();
        }
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
                section(ui, "NODE");
                ui.label(dim("PICK A NODE TO EDIT IT; SPACE ADDS ONE."));
                ui.label(dim("EVERY GRAPH BEGINS AT ITS START NODES."));
                ui.label(dim("EVENTS WAIT; ACTIONS ACT AND GO ON."));
                ui.label(dim("REPLIES CONNECTED TO A LINE ARE OFFERED."));
            }
            return;
        };
        let (mut duplicate, mut delete) = (false, false);
        if let Some(node) = doc.graph.node_mut(id) {
            section(
                ui,
                &format!(
                    "NODE · {}  ({})",
                    node.kind.title(),
                    node.kind.category().tag()
                ),
            );
            ui.add(egui::Label::new(dim(node.kind.description())).wrap());
            ui.add_space(6.0);
            inspector::fields(ui, &mut node.kind, &names);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                duplicate = ui.button("DUPLICATE  CTRL+D").clicked();
                delete = ui.button("DELETE  DEL").clicked();
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
        let scripts = self.index.names.scripts.clone();
        let Some(doc) = &mut self.project else {
            return;
        };
        let mut defs = doc.now.flags.clone();
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
                        script: Default::default(),
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
        let mut rename_from = self.flag_rename_from.take();
        let mut renamed = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, flag) in defs.flags.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("X").on_hover_text("DELETE").clicked() {
                            remove = Some(i);
                        }
                        let name =
                            ui.add(egui::TextEdit::singleline(&mut flag.name).desired_width(110.0));
                        // A finished rename follows everywhere the flag is used.
                        if name.gained_focus() {
                            rename_from = Some((i, flag.name.clone()));
                        }
                        if name.lost_focus()
                            && let Some((at, old)) = rename_from.take()
                            && at == i
                            && old != flag.name
                        {
                            renamed = Some((old, flag.name.replace(' ', "_")));
                        }
                        match &mut flag.value {
                            FlagValue::Bool(b) => {
                                let text = if *b { "[TRUE] " } else { "[FALSE]" };
                                if ui.small_button(text).clicked() {
                                    *b = !*b;
                                }
                                // An ability: a script that runs while true.
                                let shown = if flag.script.0.is_empty() {
                                    "NO SCRIPT".to_owned()
                                } else {
                                    flag.script.0.trim_start_matches("scripts/").to_uppercase()
                                };
                                egui::ComboBox::from_id_salt(("flag_script", i))
                                    .selected_text(shown)
                                    .width(110.0)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut flag.script,
                                            Default::default(),
                                            "NO SCRIPT",
                                        );
                                        for script in &scripts {
                                            ui.selectable_value(
                                                &mut flag.script,
                                                crate::scene::ScriptRef(script.clone()),
                                                script.as_str(),
                                            );
                                        }
                                    })
                                    .response
                                    .on_hover_text(
                                        "AN ABILITY: THIS SCRIPT RUNS WHILE THE FLAG IS TRUE",
                                    );
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
            RichText::new(
                "STARTING VALUES · PART OF THE PROJECT'S SETTINGS · SAVE ALL WRITES FLAGS.RON",
            )
            .small()
            .color(theme::AMBER_DIM),
        );
        if defs != before {
            for flag in &mut defs.flags {
                flag.name = flag.name.replace(' ', "_");
            }
            // Part of the project's settings: an undo step on PROJECT,
            // written with Ctrl+S there or SAVE ALL.
            self.index.flags = defs.clone();
            self.index.flags_unsaved = true;
            if let Some(doc) = &mut self.project {
                doc.now.flags = defs;
            }
        }
        self.flag_rename_from = rename_from;
        if let Some((old, new)) = renamed {
            // In LOGIC the scenes are in parked tabs; there's no active one.
            let mut none = crate::scene::Scene::default();
            self.rename_references(super::refactor::Renamed::Flag, &old, &new, &mut none, None);
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
