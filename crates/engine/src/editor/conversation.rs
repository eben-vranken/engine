//! The CONVERSATION view of a graph: the same nodes read as a script,
//! from START down. Lines show the name the player would see at that point
//! (RENAMEs above count), replies sit under their line, branches as IF /
//! OTHERWISE. Click an entry to edit it in the inspector. Beside it, the
//! line in the game's dialogue style, and TEST CONVERSATION to play it
//! through right here.

use super::dim;
use crate::dialogue::{DialogueStyle, Shown};
use crate::graph::flags::Flags;
use crate::graph::run::{GraphRunner, World};
use crate::graph::{Graph, NodeId, NodeKind};
use crate::ui::theme;
use egui::{RichText, Ui};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

/// A conversation being played in the editor.
pub(super) struct TestRun {
    runner: GraphRunner,
    choice: usize,
    /// What the graph did besides talking (DOOR, SHOW…).
    log: Vec<String>,
}

impl TestRun {
    pub fn new(dir: PathBuf, path: &str, graph: Graph, flags: Flags) -> Self {
        let mut runner = GraphRunner::new(dir, flags);
        runner.insert_graph(path, graph);
        runner.start(path);
        runner.update(&World::default(), 0.0);
        Self {
            runner,
            choice: 0,
            log: Vec::new(),
        }
    }
}

/// The script, from every START. `names` gives each character id's name
/// at the start; `selected` is set when an entry is clicked.
pub(super) fn script(
    ui: &mut Ui,
    graph: &Graph,
    names: &HashMap<String, String>,
    selected: &mut Option<NodeId>,
    current: Option<NodeId>,
) {
    let mut seen = HashSet::new();
    let starts: Vec<NodeId> = graph.starts().collect();
    if starts.is_empty() {
        ui.label(dim("NO START NODE: ADD ONE FROM THE CANVAS (SPACE)"));
    }
    for start in starts {
        let mut writer = Writer {
            graph,
            selected,
            current,
            seen: &mut seen,
        };
        writer.entry(ui, start, 0, names.clone());
    }
}

struct Writer<'a> {
    graph: &'a Graph,
    selected: &'a mut Option<NodeId>,
    current: Option<NodeId>,
    seen: &'a mut HashSet<NodeId>,
}

impl Writer<'_> {
    /// One clickable row, indented.
    fn row(&mut self, ui: &mut Ui, id: NodeId, depth: usize, text: RichText) {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 22.0);
            let chosen = *self.selected == Some(id) || self.current == Some(id);
            if ui.selectable_label(chosen, text).clicked() {
                *self.selected = Some(id);
            }
        });
    }

    fn next(
        &mut self,
        ui: &mut Ui,
        id: NodeId,
        output: u32,
        depth: usize,
        names: &HashMap<String, String>,
    ) {
        let targets: Vec<NodeId> = self.graph.targets(id, output).collect();
        for target in targets {
            self.entry(ui, target, depth, names.clone());
        }
    }

    fn entry(&mut self, ui: &mut Ui, id: NodeId, depth: usize, mut names: HashMap<String, String>) {
        let Some(node) = self.graph.node(id) else {
            return;
        };
        if !self.seen.insert(id) {
            self.row(
                ui,
                id,
                depth,
                dim(&format!("→ BACK TO {} (ABOVE)", node.kind.title())),
            );
            return;
        }
        match &node.kind {
            NodeKind::Start => {
                self.row(ui, id, depth, dim("START"));
                self.next(ui, id, 0, depth, &names);
            }
            NodeKind::Note { text } => {
                self.row(ui, id, depth, dim(&format!("NOTE · {text}")));
            }
            NodeKind::Branch { condition } => {
                self.row(
                    ui,
                    id,
                    depth,
                    RichText::new(format!("IF {}", condition.describe())).color(theme::AMBER_DIM),
                );
                self.next(ui, id, 0, depth + 1, &names);
                self.row(ui, id, depth, dim("OTHERWISE"));
                self.next(ui, id, 1, depth + 1, &names);
            }
            NodeKind::Line {
                speaker,
                text,
                sound,
            } => {
                let who = names
                    .get(&speaker.0)
                    .cloned()
                    .unwrap_or_else(|| speaker.0.clone())
                    .to_uppercase();
                let sound = if sound.0.is_empty() {
                    "SILENT".into()
                } else {
                    format!("SOUND {}", sound.0.rsplit('/').next().unwrap_or_default())
                };
                self.row(
                    ui,
                    id,
                    depth,
                    RichText::new(format!("{who:<10} {text}   · {sound}")),
                );
                let targets: Vec<NodeId> = self.graph.targets(id, 0).collect();
                let is_reply = |t: &NodeId| {
                    matches!(
                        self.graph.node(*t).map(|n| &n.kind),
                        Some(NodeKind::Reply { .. })
                    )
                };
                let (replies, others): (Vec<NodeId>, Vec<NodeId>) =
                    targets.into_iter().partition(is_reply);
                for other in others {
                    if let Some(n) = self.graph.node(other) {
                        self.row(
                            ui,
                            other,
                            depth + 1,
                            dim(&format!("WHEN LEFT · {}", n.kind.title())),
                        );
                    }
                    self.entry(ui, other, depth + 2, names.clone());
                }
                for reply in replies {
                    self.entry(ui, reply, depth + 1, names.clone());
                }
            }
            NodeKind::Reply { text, condition } => {
                let condition = if condition.flag.0.is_empty() {
                    String::new()
                } else {
                    format!("   IF {}", condition.describe())
                };
                self.row(
                    ui,
                    id,
                    depth,
                    RichText::new(format!("› {text}{condition}")).strong(),
                );
                self.next(ui, id, 0, depth + 1, &names);
            }
            NodeKind::Rename { character, name } => {
                self.row(
                    ui,
                    id,
                    depth,
                    RichText::new(format!("RENAME {} → \"{name}\"", character.0))
                        .color(theme::AMBER_DIM),
                );
                names.insert(character.0.clone(), name.clone());
                self.next(ui, id, 0, depth, &names);
            }
            kind => {
                let summary = kind.summary().join(" ");
                self.row(
                    ui,
                    id,
                    depth,
                    RichText::new(format!("→ {} {summary}", kind.title())).color(theme::AMBER_DIM),
                );
                self.next(ui, id, 0, depth, &names);
            }
        }
    }
}

/// The line `id` with its replies, as the player would see it.
pub(super) fn preview_of(
    graph: &Graph,
    id: Option<NodeId>,
    names: &HashMap<String, String>,
    flags: &Flags,
) -> Option<(String, String, Vec<String>)> {
    let line = id
        .and_then(|id| graph.node(id))
        .filter(|n| matches!(n.kind, NodeKind::Line { .. }))
        .or_else(|| {
            graph
                .nodes
                .iter()
                .find(|n| matches!(n.kind, NodeKind::Line { .. }))
        })?;
    let NodeKind::Line { speaker, text, .. } = &line.kind else {
        return None;
    };
    let replies = graph
        .targets(line.id, 0)
        .filter_map(|t| match &graph.node(t)?.kind {
            NodeKind::Reply { text, condition } if condition.holds(flags) => Some(text.clone()),
            _ => None,
        })
        .collect();
    let who = names
        .get(&speaker.0)
        .cloned()
        .unwrap_or_else(|| speaker.0.clone());
    Some((who, text.clone(), replies))
}

/// The game's dialogue box over a stand-in view.
pub(super) fn draw_box(
    ui: &mut Ui,
    style: &DialogueStyle,
    who: &str,
    text: &str,
    replies: &[String],
    choice: usize,
) {
    let size = egui::vec2(ui.available_width(), 260.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(14, 11, 8));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, theme::LINE),
        egui::StrokeKind::Inside,
    );
    crate::dialogue::draw(
        ui,
        rect,
        style,
        false,
        &Shown {
            speaker: who,
            text,
            complete: true,
            replies,
            choice,
            go_key: "E",
            choose_keys: "W/S",
        },
    );
}

/// TEST CONVERSATION's controls under the box. Returns false once the
/// conversation has ended (the caller drops the run).
pub(super) fn test_controls(
    ui: &mut Ui,
    run: &mut TestRun,
    names: &HashMap<String, String>,
    style: &DialogueStyle,
) -> bool {
    let commands = run
        .runner
        .update(&World::default(), ui.input(|i| i.stable_dt));
    for command in commands {
        run.log.push(format!("{command:?}").to_uppercase());
    }
    let Some(line) = run.runner.line() else {
        ui.label(dim("THE CONVERSATION HAS ENDED"));
        return false;
    };
    let who = run
        .runner
        .names
        .get(&line.speaker)
        .or_else(|| names.get(&line.speaker))
        .cloned()
        .unwrap_or(line.speaker.clone());
    run.choice = run.choice.min(line.replies.len().saturating_sub(1));
    draw_box(ui, style, &who, &line.text, &line.replies, run.choice);
    ui.horizontal_wrapped(|ui| {
        if line.replies.is_empty() && ui.button("CONTINUE").clicked() {
            run.runner.answer(None);
        }
        for (i, reply) in line.replies.iter().enumerate() {
            if ui.button(format!("{}. {reply}", i + 1)).clicked() {
                run.runner.answer(Some(i));
                run.choice = 0;
            }
        }
    });
    if !run.log.is_empty() {
        ui.add_space(4.0);
        ui.label(dim("ALSO HAPPENED (NOT IN THE WORLD):"));
        for entry in run.log.iter().rev().take(4) {
            ui.label(RichText::new(entry).small().color(theme::AMBER_DIM));
        }
    }
    true
}

/// Yes/no flags the graph's conditions read, for PREVIEW AS.
pub(super) fn condition_flags(graph: &Graph) -> Vec<String> {
    let mut flags: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| {
            matches!(
                n.kind,
                NodeKind::Branch { .. } | NodeKind::Reply { .. } | NodeKind::WaitUntil { .. }
            )
        })
        .flat_map(|n| n.kind.flags().into_iter().map(str::to_owned))
        .collect();
    flags.sort();
    flags.dedup();
    flags
}

/// The project's starting flags with the preview's switches on top.
pub(super) fn preview_flags(start: &Flags, overrides: &BTreeMap<String, bool>) -> Flags {
    let mut flags = start.clone();
    for (name, on) in overrides {
        flags.set(name, if *on { "true" } else { "false" });
    }
    flags
}
