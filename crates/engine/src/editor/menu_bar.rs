//! The menu bar along the top: PROJECT, EDIT, ADD, BAKE and VIEW, then the
//! controls used all the time (Play, the gizmo modes) and the project name.
//! Model tabs use the same bar without the scene-only parts.

use super::file_dialogs::Purpose;
use super::gizmo::View;
use super::new_scene::Naming;
use super::objects::primitives;
use super::{Editor, EditorOutput, dim};
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use crate::ui::theme;
use egui::{Align, Layout, Response, RichText, Ui};
use transform_gizmo::config::GizmoModeKind;

impl Editor {
    pub(super) fn menu_bar(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        blink: bool,
        view: &View,
        output: &mut EditorOutput,
    ) {
        let modeling = self.model.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            let cursor = if blink { "_" } else { " " };
            let mode = if modeling { "MODELER" } else { "EDITOR" };
            ui.label(RichText::new(format!("ENGINE // {mode}{cursor}")).strong());
            ui.add_space(12.0);
            // Editing waits until Stop: the scene is restored then.
            ui.add_enabled_ui(!self.playing, |ui| {
                self.project_menu(ui, scene, player, output);
                self.edit_menu(ui, scene, player, output);
                if !modeling {
                    self.add_menu(ui, scene, view, output);
                    self.bake_menu(ui, output);
                }
            });
            self.view_menu(ui);

            ui.add_space(24.0);
            if !modeling {
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
            }
            ui.add_enabled_ui(!self.playing, |ui| {
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
                ui.label(dim("CTRL SNAP"));
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(dim(&self.game_name.to_uppercase()))
                    .on_hover_text(self.project_dir.display().to_string());
            });
        });
    }

    fn project_menu(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        player: &PlayerSettings,
        output: &mut EditorOutput,
    ) {
        let modeling = self.model.is_some();
        let mut naming = None;
        let waiting = self.file_dialog_open();
        let mut ask = None;
        ui.menu_button("PROJECT", |ui| {
            ui.set_min_width(MENU_WIDTH);
            let hint = "A NEW, EMPTY GAME: PICK WHERE ITS FOLDER GOES AND NAME IT";
            if item(ui, "NEW PROJECT…", "", !waiting)
                .on_hover_text(hint)
                .clicked()
            {
                ask = Some(Purpose::NewProject);
            }
            let hint = "ANOTHER GAME: PICK ITS GAME.RON";
            if item(ui, "OPEN PROJECT…", "", !waiting)
                .on_hover_text(hint)
                .clicked()
            {
                ask = Some(Purpose::OpenProject);
            }
            let hint = "A .GAMEPACK FILE: PICK IT, THEN THE FOLDER TO UNPACK IT INTO";
            if item(ui, "OPEN PACKAGE…", "", !waiting)
                .on_hover_text(hint)
                .clicked()
            {
                ask = Some(Purpose::OpenPackage);
            }
            let hint = "THE WHOLE PROJECT FOLDER IN ONE .GAMEPACK FILE, \
                        TO OPEN IN ANOTHER EDITOR (SAVED FILES ONLY)";
            if item(ui, "EXPORT PACKAGE…", "", !waiting)
                .on_hover_text(hint)
                .clicked()
            {
                ask = Some(Purpose::ExportPackage);
            }
            ui.separator();
            if item(ui, "NEW SCENE…", "", true).clicked() {
                naming = Some(Naming::New);
            }
            let save = if modeling { "SAVE MODEL" } else { "SAVE SCENE" };
            if item(ui, save, "CTRL+S", true).clicked() {
                output.settled |= self.save(scene, player);
            }
            let hint = "A COPY OF THIS SCENE (AND ITS BAKES) IN SCENES/; THE TAB SWITCHES TO IT";
            if item(ui, "SAVE SCENE AS…", "", !modeling && !waiting)
                .on_hover_text(hint)
                .clicked()
            {
                ask = Some(Purpose::SaveSceneAs);
            }
            ui.separator();
            if item(ui, "NEW MODEL…", "", true).clicked() {
                naming = Some(Naming::Model);
            }
            ui.separator();
            if item(ui, "QUIT", "", true).clicked() && self.request_close() {
                output.exit = true;
            }
        });
        if let Some(purpose) = naming {
            self.ask_name(purpose);
        }
        if let Some(purpose) = ask {
            self.ask_file(purpose);
        }
    }

    fn edit_menu(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        output: &mut EditorOutput,
    ) {
        let (can_undo, can_redo) = match &self.model {
            Some(model) => (!model.undo.is_empty(), !model.redo.is_empty()),
            None => (self.history.undo_len() > 0, self.history.redo_len() > 0),
        };
        let selected = match &self.model {
            Some(model) => model.selection_count() > 0,
            None => self.selected.is_some_and(|i| i < scene.objects.len()),
        };
        ui.menu_button("EDIT", |ui| {
            ui.set_min_width(MENU_WIDTH);
            let mut change = None;
            if item(ui, "UNDO", "CTRL+Z", can_undo).clicked() {
                change = self.undo(scene, player);
            }
            if item(ui, "REDO", "CTRL+Y", can_redo).clicked() {
                change = self.redo(scene, player);
            }
            ui.separator();
            let scene_selected = selected && self.model.is_none();
            if item(ui, "DUPLICATE", "CTRL+D", scene_selected).clicked() {
                change = self.duplicate_selected(scene);
            }
            if item(ui, "DELETE", "DEL", selected).clicked() {
                change = self.delete_selected(scene);
            }
            if let Some(change) = change {
                output.changed.push(change);
                output.settled = true;
            }
        });
    }

    fn add_menu(&mut self, ui: &mut Ui, scene: &mut Scene, view: &View, output: &mut EditorOutput) {
        ui.add_enabled_ui(!scene.zones.is_empty(), |ui| {
            ui.menu_button("ADD", |ui| {
                ui.set_min_width(MENU_WIDTH);
                let mut change = None;
                for (kind, shape) in primitives() {
                    if item(ui, &kind.to_uppercase(), "", true).clicked() {
                        change = self.add_primitive(scene, kind, shape, view);
                    }
                }
                ui.separator();
                let has_start = scene.player_start().is_some();
                if item(ui, "PLAYER START", "", !has_start)
                    .on_disabled_hover_text("THE SCENE ALREADY HAS ONE")
                    .clicked()
                {
                    change = self.add_player_start(scene, view);
                }
                ui.separator();
                if item(ui, "ZONE VOLUME", "", true).clicked() {
                    change = self.add_zone_volume(scene, view);
                }
                if item(ui, "PORTAL", "", true).clicked() {
                    change = self.add_portal(scene, view);
                }
                if let Some(change) = change {
                    output.changed.push(change);
                    output.settled = true;
                }
            });
        });
    }

    fn bake_menu(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        let bakes = [
            (
                "ACOUSTICS",
                &self.bake_status,
                "REVERB PROBES IN EVERY ZONE VOLUME",
            ),
            (
                "LIGHTING",
                &self.light_status,
                "BOUNCE, SKY AND EMISSIVE LIGHT INTO LIGHTMAPS AND PROBES",
            ),
        ];
        let mut clicked = [false; 2];
        ui.menu_button("BAKE", |ui| {
            ui.set_min_width(MENU_WIDTH);
            for (i, (name, status, hint)) in bakes.iter().enumerate() {
                let busy = status.starts_with("BAKING");
                if item(ui, &format!("{name}  {status}"), "", !busy)
                    .on_hover_text(*hint)
                    .clicked()
                {
                    clicked[i] = true;
                }
            }
        });
        output.bake |= clicked[0];
        output.bake_lighting |= clicked[1];
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        let modeling = self.model.is_some();
        ui.menu_button("VIEW", |ui| {
            ui.set_min_width(MENU_WIDTH);
            let (left, right) = if modeling {
                ("MODEL PANEL", "TOOLS PANEL")
            } else {
                ("SCENE PANEL", "INSPECTOR")
            };
            toggle(ui, left, "TAB", &mut self.show_scene);
            toggle(ui, right, "SHIFT+TAB", &mut self.show_inspector);
            if !modeling {
                toggle(ui, "ZONE MARKERS", "H", &mut self.show_zones);
            }
            ui.separator();
            toggle(ui, "KEY HINTS", "", &mut self.show_hints);
            toggle(ui, "STATS", "", &mut self.show_stats);
        });
    }
}

/// Menus are at least this wide (points), so shortcuts stand apart.
const MENU_WIDTH: f32 = 200.0;

/// A menu entry with its shortcut on the right; closes the menu when
/// clicked.
fn item(ui: &mut Ui, text: &str, shortcut: &str, enabled: bool) -> Response {
    let button = egui::Button::new(text).shortcut_text(dim(shortcut));
    let response = ui.add_enabled(enabled, button);
    if response.clicked() {
        ui.close();
    }
    response
}

/// A menu entry that shows and flips a setting.
fn toggle(ui: &mut Ui, text: &str, shortcut: &str, value: &mut bool) {
    let mark = if *value { "[X]" } else { "[ ]" };
    if item(ui, &format!("{mark} {text}"), shortcut, true).clicked() {
        *value = !*value;
    }
}
