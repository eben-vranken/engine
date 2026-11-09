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
            // The menu bar takes the rest of the row, so everything else
            // goes inside it.
            egui::MenuBar::new().ui(ui, |ui| {
                let mut run = None;
                self.menus(ui, scene, &mut run);
                if let Some(command) = run {
                    self.run(command, scene, player, view, output);
                }
                ui.add_space(10.0);
                self.play_group(ui, output);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.right_controls(ui, scene, player, view, output);
                    let width = (ui.available_width() - 16.0).clamp(120.0, 300.0);
                    let search = ui.add_sized(
                        [width, 22.0],
                        egui::Button::new(dim("SEARCH OR RUN ANYTHING…   CTRL+K")),
                    );
                    if search.clicked() {
                        self.open_palette(None);
                    }
                });
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
        ui.label(dim("EDITING IS LOCKED · STOP PUTS EVERYTHING BACK"));
        ui.separator();
        let controls = &mut self.play_controls;
        let pause = if controls.paused {
            "▶ RESUME  F6"
        } else {
            "❚❚ PAUSE  F6"
        };
        if ui.button(pause).clicked() {
            controls.paused = !controls.paused;
        }
        if ui
            .add_enabled(controls.paused, egui::Button::new("STEP  F7"))
            .on_hover_text("ONE FRAME WHILE PAUSED")
            .clicked()
        {
            controls.step = true;
        }
        ui.label(dim("SPEED"));
        for (text, speed) in [("¼", 0.25), ("½", 0.5), ("1×", 1.0), ("2×", 2.0)] {
            if ui.selectable_label(controls.speed == speed, text).clicked() {
                controls.speed = speed;
            }
        }
        if ui
            .button("FREE CAMERA  F2")
            .on_hover_text("FLY AROUND; F2 AGAIN GOES BACK TO THE PLAYER")
            .clicked()
        {
            output.free_camera = true;
        }
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
                .add_enabled(changes > 0, egui::Button::new("REVERT"))
                .clicked()
            {
                self.revert_project_settings(output);
            }
            if ui
                .add_enabled(changes > 0, egui::Button::new(" SAVE  CTRL+S "))
                .clicked()
            {
                self.save_project_settings(output);
            }
            let state = if changes == 0 {
                dim("[PROJECT SAVED]")
            } else {
                RichText::new(format!("[PROJECT: {changes} CHANGES]"))
            };
            ui.label(state);
            return;
        }
        let unsaved = self.dirty_paths().len() + usize::from(self.project_unsaved());
        if ui
            .add_enabled(unsaved > 0, egui::Button::new("SAVE ALL"))
            .on_hover_text("CTRL+SHIFT+S")
            .clicked()
        {
            self.run(Command::SaveAll, scene, player, view, output);
        }
        let state = if unsaved == 0 {
            dim("[SAVED]")
        } else {
            RichText::new(format!("[{unsaved} UNSAVED]"))
        };
        ui.label(state);
    }

    /// PLAY (the scene open in BUILD), PLAY FROM HERE and CONTINUE SAVE.
    fn play_group(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        if self.workspace() != Workspace::Build {
            ui.label(dim("PLAY FROM BUILD"));
            return;
        }
        let play = egui::Button::new(
            RichText::new(" ▶ PLAY  F5 ")
                .color(theme::BACKGROUND)
                .strong(),
        )
        .fill(theme::AMBER);
        if ui.add(play).clicked() {
            output.toggle_play = true;
        }
        if ui
            .button("PLAY FROM HERE")
            .on_hover_text("THE PLAYER STARTS WHERE THE EDITOR CAMERA IS, FACING THE SAME WAY")
            .clicked()
        {
            output.play_from_here = true;
        }
        let saved = crate::save::exists(&self.project_dir);
        if ui
            .add_enabled(saved, egui::Button::new("CONTINUE SAVE"))
            .on_hover_text("PLAY FROM THE SAVED GAME")
            .on_disabled_hover_text("NO SAVED GAME YET: SAVE POINTS AND SAVE GAME NODES MAKE ONE")
            .clicked()
        {
            output.resume_play = true;
        }
        if ui
            .small_button("WINDOW")
            .on_hover_text("PLAY IN A WINDOW OF ITS OWN, AS A PLAYER GETS IT (SHIFT+F5)")
            .clicked()
        {
            output.play_window = true;
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
        // What the view shows (zones, triggers, probes) is in SHOW over it.
        let view = vec![
            Some(ToggleLeft),
            Some(ToggleRight),
            None,
            Some(ToggleVsync),
            Some(ToggleHints),
            Some(ToggleStats),
            None,
            Some(Shortcuts),
        ];
        for (title, list) in [("FILE", file), ("EDIT", edit)] {
            ui.menu_button(title, |ui| self.menu_items(ui, scene, &list, run));
        }
        match self.workspace() {
            Workspace::Build => {
                let mut add: Vec<Option<Command>> = vec![Some(AddDialog), None];
                add.extend((0..super::objects::primitives().len()).map(|i| Some(AddPrimitive(i))));
                add.extend([
                    None,
                    Some(AddPlayerStart),
                    None,
                    Some(AddZone),
                    Some(AddZoneVolume),
                    Some(AddPortal),
                ]);
                ui.menu_button("ADD", |ui| self.menu_items(ui, scene, &add, run));
            }
            Workspace::Logic => {
                ui.menu_button("ADD", |ui| {
                    let scene_graph =
                        crate::graph::scene_of_graph(&crate::graph::key(&self.logic_path))
                            .is_some();
                    if let Some(kind) = super::graph_canvas::add_menu(ui, scene_graph)
                        && let Some(doc) = &mut self.graph
                    {
                        doc.add_in_view(kind);
                    }
                });
            }
            Workspace::Project => {}
        }
        ui.menu_button("VIEW", |ui| self.menu_items(ui, scene, &view, run));
        if self.workspace() == Workspace::Build {
            let bake = vec![Some(BakeLighting), Some(BakeAcoustics)];
            ui.menu_button("BAKE", |ui| self.menu_items(ui, scene, &bake, run));
        }
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
            Command::ToggleVsync => self.vsync,
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
