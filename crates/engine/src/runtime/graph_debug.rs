//! F3 while playing: the running graphs, with their active nodes lit, and
//! the flags' values. For testing a game's logic as it plays.

use crate::editor::graph_canvas::{self, CanvasView, Marks};
use crate::graph::flags::Flags;
use crate::graph::{Graph, NodeId};
use crate::ui::theme;
use egui::{RichText, Sense, vec2};
use std::sync::Arc;

pub(super) type Running = Vec<(String, Arc<Graph>, Vec<NodeId>)>;

pub(super) fn window(
    ui: &egui::Ui,
    running: &Running,
    flags: &Flags,
    view: &mut CanvasView,
    index: &mut usize,
) {
    egui::Window::new(RichText::new("GRAPHS // RUNNING   F3 CLOSE").strong())
        .default_size([760.0, 440.0])
        .resizable(true)
        .collapsible(true)
        .show(ui.ctx(), |ui| {
            if running.is_empty() {
                ui.label(RichText::new("NO GRAPH IS RUNNING").color(theme::AMBER_DIM));
            }
            *index = (*index).min(running.len().saturating_sub(1));
            ui.horizontal_wrapped(|ui| {
                for (i, (path, _, active)) in running.iter().enumerate() {
                    let text = format!("{} [{}]", path.to_uppercase(), active.len());
                    ui.selectable_value(index, i, text);
                }
            });
            ui.separator();
            let available = ui.available_size();
            ui.horizontal_top(|ui| {
                let size = vec2((available.x - 220.0).max(240.0), available.y.max(240.0));
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                if let Some((_, graph, active)) = running.get(*index) {
                    let mut graph = (**graph).clone();
                    let marks = Marks {
                        selected: None,
                        selected_link: None,
                        active,
                    };
                    graph_canvas::show(ui, rect, &mut graph, view, &marks, false);
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new("FLAGS").strong());
                    if flags.0.is_empty() {
                        ui.label(RichText::new("NONE").color(theme::AMBER_DIM));
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("debug_flags")
                        .show(ui, |ui| {
                            for (name, value) in &flags.0 {
                                ui.label(RichText::new(format!("{name} = {value}")).small().color(
                                    if value.is_true() {
                                        theme::AMBER
                                    } else {
                                        theme::AMBER_DIM
                                    },
                                ));
                            }
                        });
                });
            });
        });
}
