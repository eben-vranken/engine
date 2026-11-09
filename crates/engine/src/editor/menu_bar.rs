//! The top bar: the workspaces (PROJECT, BUILD, LOGIC), the menus (FILE,
//! EDIT, ADD, VIEW, all running the same commands as CTRL+K), where you are
//! (a breadcrumb), the search, undo/redo, the save state and PLAY. While
//! playing it shows what is playing and STOP.

use super::commands::{Command, Workspace};
use super::gizmo::View;
use super::project_page::Section;
use super::{Editor, EditorOutput, dim};
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use crate::ui::theme;
use egui::{Align, Layout, Response, RichText, Ui};

/// Menus are at least this wide (points), so shortcuts stand apart.
const MENU_WIDTH: f32 = 220.0;

impl Editor {
    pub(super) fn top_bar(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        blink: bool,
        view: &View,
        output: &mut EditorOutput,
    ) {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            let cursor = if blink { "_" } else { " " };
            ui.label(RichText::new(format!("ENGINE{cursor}")).strong());
            ui.add_space(8.0);
            if self.playing {
                self.playing_bar(ui, output);
                return;
            }
            self.workspace_switch(ui, output);
            ui.add_space(6.0);
            egui::MenuBar::new().ui(ui, |ui| {
                let mut run = None;
                self.menus(ui, scene, &mut run);
                if let Some(command) = run {
                    self.run(command, scene, player, view, output);
                }
            });
            ui.add_space(6.0);
            ui.label(self.breadcrumb());

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.right_controls(ui, scene, player, view, output);
                let width = (ui.available_width() - 16.0).clamp(120.0, 380.0);
                let search = ui.add_sized(
                    [width, 22.0],
                    egui::Button::new(dim("SEARCH OR RUN ANYTHING…        CTRL+K")),
                );
                if search.clicked() {
                    self.open_palette(None);
                }
            });
        });
        ui.add_space(3.0);
    }

    fn playing_bar(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        ui.label(
            RichText::new(format!(
                " [PLAYING] {} ",
                self.scene_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_uppercase())
                    .unwrap_or_default()
            ))
            .color(theme::BACKGROUND)
            .background_color(theme::AMBER)
            .strong(),
        );
        ui.label(dim("EDITING IS LOCKED; STOP PUTS THE SCENE BACK AS IT WAS"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(" STOP  F5 ")
                            .color(theme::BACKGROUND)
                            .strong(),
                    )
                    .fill(theme::AMBER),
                )
                .clicked()
            {
                output.toggle_play = true;
            }
            if ui
                .button("RESTART")
                .on_hover_text("STOP AND PLAY AGAIN FROM THE START")
                .clicked()
            {
                output.restart_play = true;
            }
            let drawer = if self.drawer_open {
                "[TEST DRAWER F3]"
            } else {
                " TEST DRAWER F3 "
            };
            if ui.small_button(drawer).clicked() {
                self.drawer_open = !self.drawer_open;
            }
        });
    }

    fn workspace_switch(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        let current = self.workspace();
        ui.spacing_mut().item_spacing.x = 0.0;
        for (workspace, text, hint) in [
            (
                Workspace::Project,
                "PROJECT",
                "START SCREEN AND THE PROJECT'S SETTINGS",
            ),
            (Workspace::Build, "BUILD", "SCENES"),
            (Workspace::Logic, "LOGIC", "GRAPHS AND FLAGS"),
        ] {
            let on = workspace == current;
            let text = if on {
                RichText::new(format!(" {text} "))
                    .color(theme::BACKGROUND)
                    .strong()
            } else {
                RichText::new(format!(" {text} ")).color(theme::AMBER_DIM)
            };
            let mut button = egui::Button::new(text);
            if on {
                button = button.fill(theme::AMBER);
            }
            if ui.add(button).on_hover_text(hint).clicked() && !on {
                self.go(workspace, output);
            }
        }
        ui.spacing_mut().item_spacing.x = 8.0;
    }

    fn breadcrumb(&self) -> RichText {
        let project = self.game_name.to_uppercase();
        let tail = match self.workspace() {
            Workspace::Project => format!(
                "SETTINGS › {}",
                self.project_page.as_ref().map_or("", |p| p.section.title())
            ),
            Workspace::Build => format!(
                "SCENES › {}",
                self.scene_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_uppercase())
                    .unwrap_or_default()
            ),
            Workspace::Logic => {
                let key = crate::graph::key(&self.scene_path);
                let owner = self.index.owner(&key);
                format!(
                    "LOGIC › {} › {}",
                    owner.group(),
                    self.scene_path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_uppercase())
                        .unwrap_or_default()
                )
            }
        };
        RichText::new(format!("{project} › {tail}")).color(theme::AMBER_DIM)
    }

    /// Undo, redo, the save state and PLAY (or SAVE GAME.RON on the
    /// project page), laid out right to left.
    fn right_controls(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        view: &View,
        output: &mut EditorOutput,
    ) {
        if self.workspace() == Workspace::Project {
            let changes = self.project_changes();
            if ui
                .add_enabled(changes > 0, egui::Button::new(" SAVE GAME.RON "))
                .clicked()
            {
                self.save_project_settings(output);
            }
            if ui
                .add_enabled(changes > 0, egui::Button::new("REVERT"))
                .clicked()
            {
                self.revert_project_settings(output);
            }
            let state = if changes == 0 {
                dim("[SAVED]")
            } else {
                RichText::new(format!("[{changes} CHANGES]"))
            };
            ui.label(state);
            return;
        }
        if self.workspace() == Workspace::Build {
            let saved = crate::save::exists(&self.project_dir);
            ui.menu_button("▾", |ui| {
                ui.set_min_width(MENU_WIDTH);
                if item(ui, "PLAY FROM START", "F5", true).clicked() {
                    output.toggle_play = true;
                }
                if item(ui, "PLAY FROM SAVE", "", saved)
                    .on_disabled_hover_text(
                        "NO SAVED GAME YET: SAVE POINTS AND SAVE GAME NODES MAKE ONE",
                    )
                    .clicked()
                {
                    output.resume_play = true;
                }
                if item(ui, "PLAY IN A WINDOW, AS A PLAYER", "SHIFT+F5", true)
                    .on_hover_text("THE GAME AS A PLAYER GETS IT: ITS OWN WINDOW, NO TUNING OR DEBUG TOOLS, FROM THE SAVED FILES")
                    .clicked()
                {
                    output.play_window = true;
                }
            })
            .response
            .on_hover_text("PLAY FROM THE SAVED GAME");
            let play = egui::Button::new(
                RichText::new(" PLAY  F5 ")
                    .color(theme::BACKGROUND)
                    .strong(),
            )
            .fill(theme::AMBER);
            if ui.add(play).clicked() {
                output.toggle_play = true;
            }
        } else {
            ui.label(dim("PLAY FROM BUILD"));
        }
        ui.add_space(6.0);
        let state = if self.any_dirty() {
            RichText::new("[UNSAVED]")
        } else {
            dim("[SAVED]")
        };
        ui.label(state);
        for command in [Command::Redo, Command::Undo] {
            let enabled = self.can_run(&command, scene);
            if ui
                .add_enabled(enabled, egui::Button::new(command.label()).small())
                .on_hover_text(command.shortcut())
                .clicked()
            {
                self.run(command, scene, player, view, output);
            }
        }
    }

    /// FILE, EDIT, ADD and VIEW. A picked command goes to `run`.
    fn menus(&mut self, ui: &mut Ui, scene: &Scene, run: &mut Option<Command>) {
        use Command::*;
        let file: Vec<Option<Command>> = vec![
            Some(NewProject),
            Some(OpenProject),
            Some(OpenPackage),
            Some(ExportPackage),
            None,
            Some(NewScene),
            Some(NewGraph),
            Some(NewScript),
            Some(Save),
            Some(SaveSceneAs),
            None,
            Some(Settings(Section::General)),
            None,
            Some(Quit),
        ];
        let edit = vec![
            Some(Undo),
            Some(Redo),
            None,
            Some(Duplicate),
            Some(Delete),
            None,
            Some(Search),
        ];
        let mut view = vec![
            Some(ToggleLeft),
            Some(ToggleRight),
            None,
            Some(ToggleZones),
            Some(ToggleTriggers),
            Some(ToggleProbes),
            None,
            Some(ToggleHints),
            Some(ToggleStats),
            None,
            Some(Shortcuts),
        ];
        if self.workspace() != Workspace::Build {
            view.retain(|c| {
                !matches!(
                    c,
                    Some(ToggleZones) | Some(ToggleTriggers) | Some(ToggleProbes)
                )
            });
        }
        for (title, list) in [("FILE", file), ("EDIT", edit)] {
            ui.menu_button(title, |ui| self.menu_items(ui, scene, &list, run));
        }
        match self.workspace() {
            Workspace::Build => {
                let mut add: Vec<Option<Command>> = (0..super::objects::primitives().len())
                    .map(|i| Some(AddPrimitive(i)))
                    .collect();
                add.extend([
                    None,
                    Some(AddPlayerStart),
                    None,
                    Some(AddZone),
                    Some(AddZoneVolume),
                    Some(AddPortal),
                    None,
                    Some(BakeLighting),
                    Some(BakeAcoustics),
                ]);
                ui.menu_button("ADD", |ui| self.menu_items(ui, scene, &add, run));
            }
            Workspace::Logic => {
                ui.menu_button("ADD", |ui| {
                    if let Some(kind) = super::graph_canvas::add_menu(ui)
                        && let Some(doc) = &mut self.graph
                    {
                        doc.add_in_view(kind);
                    }
                });
            }
            Workspace::Project => {}
        }
        ui.menu_button("VIEW", |ui| self.menu_items(ui, scene, &view, run));
    }

    fn menu_items(
        &self,
        ui: &mut Ui,
        scene: &Scene,
        list: &[Option<Command>],
        run: &mut Option<Command>,
    ) {
        ui.set_min_width(MENU_WIDTH);
        for entry in list {
            match entry {
                None => {
                    ui.separator();
                }
                Some(command) => {
                    let mut text = command.label();
                    if let Some(on) = self.toggle_state(command) {
                        text = format!("{} {text}", if on { "[X]" } else { "[ ]" });
                    }
                    if item(ui, &text, command.shortcut(), self.can_run(command, scene)).clicked() {
                        *run = Some(command.clone());
                    }
                }
            }
        }
    }

    /// For VIEW's toggles: whether they are on.
    fn toggle_state(&self, command: &Command) -> Option<bool> {
        Some(match command {
            Command::ToggleLeft => self.show_scene,
            Command::ToggleRight => self.show_inspector,
            Command::ToggleZones => self.show_zones,
            Command::ToggleTriggers => self.show_triggers,
            Command::ToggleProbes => self.show_probes,
            Command::ToggleHints => self.show_hints,
            Command::ToggleStats => self.show_stats,
            _ => return None,
        })
    }
}

/// A menu entry with its shortcut on the right; closes the menu when
/// clicked.
pub(super) fn item(ui: &mut Ui, text: &str, shortcut: &str, enabled: bool) -> Response {
    let button = egui::Button::new(text).shortcut_text(dim(shortcut));
    let response = ui.add_enabled(enabled, button);
    if response.clicked() {
        ui.close();
    }
    response
}
