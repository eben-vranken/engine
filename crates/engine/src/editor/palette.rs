//! CTRL+K: one search for everything. Actions, the scene's objects, scenes
//! and graphs to open, flags, settings sections, behaviors to add to the
//! selection and nodes to add to the open graph. `>` keeps only actions,
//! `@` only objects, `#` only flags. SPACE over a graph opens it on nodes.
//! Also the `?` list of every shortcut.

use super::commands::{Command, Workspace};
use super::files::{graph_files, scene_files};
use super::gizmo::View;
use super::{Editor, EditorOutput, dim};
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use crate::ui::theme;
use egui::{Key, RichText, Ui};

pub(super) struct Palette {
    query: String,
    chosen: usize,
    /// Only these groups (SPACE in a graph: NODES).
    only: Option<&'static str>,
    focus: bool,
}

struct Item {
    group: &'static str,
    text: String,
    detail: String,
    command: Command,
}

impl Editor {
    pub(super) fn open_palette(&mut self, only: Option<&'static str>) {
        self.palette = Some(Palette {
            query: String::new(),
            chosen: 0,
            only,
            focus: true,
        });
    }

    fn palette_items(&self, scene: &Scene) -> Vec<Item> {
        let mut items = Vec::new();
        for command in Command::all() {
            if self.can_run(&command, scene) {
                items.push(Item {
                    group: "ACTIONS",
                    text: command.label(),
                    detail: command.shortcut().into(),
                    command,
                });
            }
        }
        if self.workspace() == Workspace::Build && !self.playing {
            for (index, object) in scene.objects.iter().enumerate() {
                items.push(Item {
                    group: "OBJECTS",
                    text: object.name.clone(),
                    detail: format!(
                        "{} · {}",
                        object.zone.to_uppercase(),
                        super::hierarchy::tag(object)
                    ),
                    command: Command::Select(index),
                });
            }
            if self.selected.is_some() {
                for (i, (name, _)) in super::object_panel::new_components().iter().enumerate() {
                    items.push(Item {
                        group: "ADD BEHAVIOR",
                        text: format!("ADD {}", name.to_uppercase()),
                        detail: "TO THE SELECTION".into(),
                        command: Command::AddBehavior(i),
                    });
                }
            }
        }
        if self.graph.is_some() && !self.playing {
            for (i, kind) in crate::graph::NodeKind::all().iter().enumerate() {
                items.push(Item {
                    group: "NODES",
                    text: format!("ADD {}", kind.title()),
                    detail: kind.category().title().into(),
                    command: Command::AddNode(i),
                });
            }
        }
        if !self.playing {
            for path in scene_files(&self.project_dir) {
                items.push(Item {
                    group: "SCENES",
                    text: path.display().to_string(),
                    detail: "OPEN IN BUILD".into(),
                    command: Command::OpenScene(path),
                });
            }
            for path in graph_files(&self.project_dir) {
                let owner = self.index.owner(&crate::graph::key(&path));
                items.push(Item {
                    group: "GRAPHS",
                    text: path.display().to_string(),
                    detail: owner.group().into(),
                    command: Command::OpenGraph(path),
                });
            }
            for path in super::files::script_files(&self.project_dir) {
                items.push(Item {
                    group: "SCRIPTS",
                    text: path.display().to_string(),
                    detail: "OPEN IN LOGIC".into(),
                    command: Command::OpenGraph(path),
                });
            }
            for flag in &self.index.flags.flags {
                items.push(Item {
                    group: "FLAGS",
                    text: flag.name.clone(),
                    detail: format!("{} · STARTS {}", flag.value.kind(), flag.value),
                    command: Command::Go(Workspace::Logic),
                });
            }
        }
        items
    }

    /// The palette, while open.
    pub(super) fn palette_ui(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        view: &View,
        output: &mut EditorOutput,
    ) {
        if self.palette.is_none() {
            return;
        }
        let items = self.palette_items(scene);
        let Some(palette) = &mut self.palette else {
            return;
        };
        let raw = palette.query.trim().to_lowercase();
        let (group, query) = match raw.chars().next() {
            Some('>') => (Some("ACTIONS"), raw[1..].trim().to_owned()),
            Some('@') => (Some("OBJECTS"), raw[1..].trim().to_owned()),
            Some('#') => (Some("FLAGS"), raw[1..].trim().to_owned()),
            _ => (palette.only, raw.clone()),
        };
        let shown: Vec<&Item> = items
            .iter()
            .filter(|i| group.is_none_or(|g| i.group == g))
            .filter(|i| {
                query.is_empty()
                    || query.split_whitespace().all(|w| {
                        i.text.to_lowercase().contains(w) || i.detail.to_lowercase().contains(w)
                    })
            })
            .take(60)
            .collect();
        palette.chosen = palette.chosen.min(shown.len().saturating_sub(1));
        let (mut run, mut close) = (None, false);
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                palette.chosen = (palette.chosen + 1).min(shown.len().saturating_sub(1));
            }
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                palette.chosen = palette.chosen.saturating_sub(1);
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Enter) {
                run = shown.get(palette.chosen).map(|i| i.command.clone());
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                close = true;
            }
        });
        let title = palette.only.unwrap_or("SEARCH OR RUN ANYTHING");
        let modal = egui::Modal::new(egui::Id::unique("palette")).show(ui.ctx(), |ui| {
            ui.set_width(620.0);
            ui.label(dim(title));
            let field = ui.add(
                egui::TextEdit::singleline(&mut palette.query)
                    .hint_text("TYPE…   > ACTIONS   @ OBJECTS   # FLAGS")
                    .desired_width(f32::INFINITY),
            );
            if palette.focus {
                field.request_focus();
                palette.focus = false;
            }
            if field.changed() {
                palette.chosen = 0;
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let mut last_group = "";
                    for (i, item) in shown.iter().enumerate() {
                        if item.group != last_group {
                            last_group = item.group;
                            ui.add_space(4.0);
                            ui.label(RichText::new(item.group).small().color(theme::AMBER_DIM));
                        }
                        let picked = i == palette.chosen;
                        let response = ui
                            .horizontal(|ui| {
                                let r = ui.selectable_label(picked, &item.text);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| ui.label(dim(&item.detail)),
                                );
                                r
                            })
                            .inner;
                        if picked {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() {
                            run = Some(item.command.clone());
                        }
                    }
                    if shown.is_empty() {
                        ui.label(dim("NOTHING MATCHES"));
                    }
                });
            ui.add_space(6.0);
            ui.label(dim("↑↓ MOVE   ENTER RUN   ESC CLOSE"));
        });
        if modal.should_close() {
            close = true;
        }
        if run.is_some() || close {
            self.palette = None;
        }
        if let Some(command) = run {
            self.run(command, scene, player, view, output);
        }
    }

    /// `?`: every shortcut.
    pub(super) fn shortcuts_ui(&mut self, ui: &Ui) {
        if !self.shortcuts_open {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("shortcuts")).show(ui.ctx(), |ui| {
            ui.set_width(640.0);
            ui.label(RichText::new("SHORTCUTS").strong());
            ui.add_space(6.0);
            egui::Grid::new("shortcut_list")
                .num_columns(2)
                .spacing([24.0, 4.0])
                .show(ui, |ui| {
                    for (keys, what) in SHORTCUTS {
                        if keys.is_empty() {
                            ui.label("");
                            ui.label(RichText::new(*what).small().color(theme::AMBER_DIM));
                        } else {
                            ui.label(*keys);
                            ui.label(dim(what));
                        }
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            if ui.button("CLOSE").clicked() {
                ui.close();
            }
        });
        if modal.should_close() {
            self.shortcuts_open = false;
        }
    }
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("", "EVERYWHERE"),
    ("CTRL+K", "SEARCH OR RUN ANYTHING"),
    ("CTRL+S", "SAVE"),
    ("CTRL+Z / CTRL+Y", "UNDO / REDO"),
    ("F5", "PLAY / STOP"),
    ("SHIFT+F5", "PLAY IN A WINDOW, AS A PLAYER"),
    ("TAB / SHIFT+TAB", "LEFT PANEL / INSPECTOR"),
    ("?", "THIS LIST"),
    ("[ ] 0 / V", "RENDER SCALE / AUTO / VSYNC"),
    ("", "BUILD"),
    ("CLICK / SHIFT+CLICK", "SELECT / ADD TO THE SELECTION"),
    ("W E R", "MOVE / ROTATE / SCALE (CTRL SNAPS)"),
    ("HOLD RMB + WASD, Q/E", "FLY; SHIFT FAST, WHEEL SPEED"),
    ("CTRL+D / DEL", "DUPLICATE / DELETE"),
    ("H", "ZONE MARKERS"),
    ("ESC", "DESELECT"),
    ("", "LOGIC"),
    ("SPACE", "ADD A NODE"),
    ("DRAG A RIGHT DOT", "CONNECT TO ANOTHER NODE"),
    ("WHEEL / DRAG BACKGROUND", "ZOOM / PAN"),
    ("", "PLAYING"),
    ("WASD SPACE SHIFT CTRL", "MOVE, JUMP, SPRINT, CROUCH"),
    ("E", "USE, TALK, TAKE"),
    ("M", "MAP"),
    ("F3", "TEST DRAWER (LOGIC, FLAGS, AUDIO, FEEL, MAP, STATS)"),
    ("F6–F10 / Q", "AUDIO SWITCHES / CLAP"),
    ("F1 / F2 / R", "TUNING / FLY CAMERA / RESPAWN"),
    ("W/S, 1–9, E", "DIALOGUE: CHOOSE, PICK, GO ON"),
];
