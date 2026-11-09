//! ADD: one searchable dialog for everything that goes into a scene: shapes,
//! the project's models, behaviors for the selected object, and the world's
//! pieces (player start, zones, portals). Opened with A in BUILD or the
//! outliner's + ADD; the ADD menu lists the same commands.

use super::commands::Command;
use super::files::all_models;
use super::{Editor, EditorOutput, dim};
use crate::ui::theme;
use egui::{Key, RichText, Ui};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Filter {
    #[default]
    All,
    Shapes,
    Models,
    Behaviors,
    World,
}

impl Filter {
    const ALL: [Filter; 5] = [
        Filter::All,
        Filter::Shapes,
        Filter::Models,
        Filter::Behaviors,
        Filter::World,
    ];

    fn title(self) -> &'static str {
        match self {
            Filter::All => "ALL",
            Filter::Shapes => "SHAPES",
            Filter::Models => "MODELS",
            Filter::Behaviors => "BEHAVIORS",
            Filter::World => "WORLD",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct AddDialog {
    query: String,
    filter: Filter,
    /// The highlighted row (↑↓, ENTER adds it).
    chosen: usize,
}

enum Pick {
    Run(Command),
    Model(PathBuf),
}

struct Entry {
    name: String,
    kind: Filter,
    description: String,
    pick: Pick,
}

impl Editor {
    pub(super) fn open_add_dialog(&mut self) {
        self.add_dialog = Some(AddDialog::default());
    }

    fn add_entries(&self, scene: &crate::scene::Scene) -> Vec<Entry> {
        let mut entries = Vec::new();
        for (i, (name, _)) in super::objects::primitives().iter().enumerate() {
            entries.push(Entry {
                name: name.to_uppercase(),
                kind: Filter::Shapes,
                description: "A primitive shape; its type can be changed later.".into(),
                pick: Pick::Run(Command::AddPrimitive(i)),
            });
        }
        for path in all_models(&self.project_dir) {
            entries.push(Entry {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                kind: Filter::Models,
                description: path.display().to_string(),
                pick: Pick::Model(path),
            });
        }
        if self.selected.is_some_and(|i| i < scene.objects.len()) {
            for (i, (name, component)) in super::object_panel::new_components()
                .into_iter()
                .enumerate()
            {
                entries.push(Entry {
                    name: name.to_uppercase(),
                    kind: Filter::Behaviors,
                    description: super::inspector::doc_of(&component)
                        .unwrap_or("")
                        .to_owned(),
                    pick: Pick::Run(Command::AddBehavior(i)),
                });
            }
        }
        for (command, description) in [
            (Command::AddPlayerStart, "Where the player starts."),
            (
                Command::AddZone,
                "A named area: fog, grading, reverb, the map.",
            ),
            (Command::AddZoneVolume, "A box that says where a zone is."),
            (Command::AddPortal, "Joins two zones (a doorway)."),
        ] {
            entries.push(Entry {
                name: command.label().trim_start_matches("ADD ").to_owned(),
                kind: Filter::World,
                description: description.into(),
                pick: Pick::Run(command),
            });
        }
        entries
    }

    /// The dialog, while open. A pick becomes a command (or a model to
    /// place in front of the camera).
    pub(super) fn add_dialog_ui(
        &mut self,
        ui: &Ui,
        scene: &crate::scene::Scene,
        run: &mut Option<Command>,
        output: &mut EditorOutput,
    ) {
        if self.add_dialog.is_none() {
            return;
        }
        let entries = self.add_entries(scene);
        let zone = self
            .new_object_zone(scene)
            .unwrap_or_default()
            .to_uppercase();
        let Some(dialog) = &mut self.add_dialog else {
            return;
        };
        let words: Vec<String> = dialog
            .query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let shown: Vec<&Entry> = entries
            .iter()
            .filter(|e| dialog.filter == Filter::All || e.kind == dialog.filter)
            .filter(|e| {
                let text = format!("{} {}", e.name, e.description).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect();
        dialog.chosen = dialog.chosen.min(shown.len().saturating_sub(1));
        let (down, up, enter, escape) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if down {
            dialog.chosen = (dialog.chosen + 1).min(shown.len().saturating_sub(1));
        }
        if up {
            dialog.chosen = dialog.chosen.saturating_sub(1);
        }
        let mut picked = enter.then_some(dialog.chosen).filter(|_| !shown.is_empty());
        let mut close = escape;
        let modal = egui::Modal::new(egui::Id::unique("add_dialog")).show(ui.ctx(), |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("ADD TO {zone}")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(dim("A · OR + ADD"));
                });
            });
            let field = ui.add(
                egui::TextEdit::singleline(&mut dialog.query)
                    .hint_text("SEARCH")
                    .desired_width(f32::INFINITY),
            );
            field.request_focus();
            ui.horizontal(|ui| {
                for filter in Filter::ALL {
                    if ui
                        .selectable_label(dialog.filter == filter, filter.title())
                        .clicked()
                    {
                        dialog.filter = filter;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if shown.is_empty() {
                        ui.label(dim("NOTHING MATCHES"));
                    }
                    for (i, entry) in shown.iter().enumerate() {
                        let on = i == dialog.chosen;
                        let response = ui
                            .vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let name = RichText::new(&entry.name);
                                    ui.label(if on { name.strong() } else { name });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| ui.label(dim(entry.kind.title())),
                                    );
                                });
                                if !entry.description.is_empty() {
                                    ui.label(
                                        RichText::new(&entry.description)
                                            .small()
                                            .color(theme::AMBER_DIM),
                                    );
                                }
                            })
                            .response
                            .interact(egui::Sense::click());
                        if on {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() {
                            picked = Some(i);
                        }
                    }
                });
            ui.separator();
            ui.label(dim(
                "↑↓ PICK · ENTER ADDS IN FRONT OF THE CAMERA · ESC CLOSES",
            ));
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(entry) = picked.and_then(|i| shown.get(i)) {
            match &entry.pick {
                Pick::Run(command) => *run = Some(command.clone()),
                Pick::Model(path) => output.place_model = Some((path.clone(), None)),
            }
            close = true;
        }
        if close {
            self.add_dialog = None;
        }
    }
}
