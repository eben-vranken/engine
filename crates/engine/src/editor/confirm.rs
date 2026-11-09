//! Asking about unsaved changes before they would be lost.

use super::{Editor, EditorOutput, dialog_heading};
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use egui::Ui;

/// What to do once unsaved changes are dealt with.
#[derive(Debug, Clone)]
pub(super) enum Confirm {
    /// Close this tab.
    CloseTab(usize),
    /// Close this LOGIC tab (a graph or script).
    CloseLogic(usize),
    /// Close the editor (every tab).
    Close,
    /// Leave this project for another one.
    Project(super::file_dialogs::Switch),
}

impl Editor {
    /// The window wants to close. Returns true if it may close now;
    /// otherwise asks about unsaved changes first.
    pub fn request_close(&mut self) -> bool {
        if self.any_dirty() || self.project_unsaved() {
            self.confirm = Some(Confirm::Close);
            false
        } else {
            true
        }
    }

    /// Save / discard / cancel, when unsaved changes are in the way.
    pub(super) fn confirm_dialog(
        &mut self,
        ui: &Ui,
        scene: &Scene,
        player: &PlayerSettings,
        output: &mut EditorOutput,
    ) {
        let Some(confirm) = self.confirm.clone() else {
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("unsaved_changes")).show(ui.ctx(), |ui| {
            let names = match &confirm {
                Confirm::CloseTab(index) => self.tab_label(*index),
                Confirm::CloseLogic(_) => "THIS TAB".into(),
                Confirm::Close | Confirm::Project(_) => {
                    let mut names = self.dirty_paths();
                    if self.project_unsaved() {
                        names.push("PROJECT SETTINGS".into());
                    }
                    names.join(", ")
                }
            };
            let verb = if names.contains(", ") { "HAVE" } else { "HAS" };
            let text = format!("{names} {verb} CHANGES THAT AREN'T SAVED.");
            dialog_heading(ui, "UNSAVED CHANGES", &text);
            ui.horizontal(|ui| {
                if ui.button("SAVE").clicked() {
                    decision = Some(true);
                }
                if ui.button("DISCARD").clicked() {
                    decision = Some(false);
                }
                if ui.button("CANCEL").clicked() {
                    self.confirm = None;
                }
            });
        });
        let Some(save) = decision else {
            return;
        };
        self.confirm = None;
        let leaves_project = matches!(confirm, Confirm::Close | Confirm::Project(_));
        if save {
            if leaves_project && self.project_unsaved() {
                self.save_project_settings(output);
                if self.project_unsaved() {
                    return; // game.ron wasn't written; the page shows why
                }
            }
            let saved = match &confirm {
                Confirm::CloseTab(index) => {
                    self.save_tab(*index, scene, player, &mut output.settled)
                }
                Confirm::CloseLogic(index) => self.save_logic_tab(*index),
                Confirm::Close | Confirm::Project(_) => {
                    self.save_all(scene, player, &mut output.settled)
                }
            };
            if !saved {
                return; // saving failed; the error is in the status bar
            }
        } else if leaves_project {
            // Discarded: the render scale preview goes back too.
            self.revert_project_settings(output);
        }
        match confirm {
            Confirm::CloseTab(index) => output.close_tab = Some(index),
            Confirm::CloseLogic(index) => self.remove_logic_tab(index),
            Confirm::Close => output.exit = true,
            Confirm::Project(switch) => self.switch_project(switch, output),
        }
    }
}

impl Editor {
    /// After Play: the player feel was changed while playing (tuning panel,
    /// player.ron edited). KEEP makes it one undo step; DISCARD puts it back.
    pub(super) fn keep_feel_dialog(&mut self, ui: &Ui, player: &mut PlayerSettings) {
        let Some((before, after)) = self.feel_changed.clone() else {
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("keep_feel")).show(ui.ctx(), |ui| {
            dialog_heading(
                ui,
                "PLAYER FEEL CHANGED WHILE PLAYING",
                "EVERYTHING ELSE WENT BACK WHEN PLAY STOPPED. KEEP THE NEW FEEL?",
            );
            ui.horizontal(|ui| {
                if ui.button("KEEP (UNDO-ABLE)").clicked() {
                    decision = Some(true);
                }
                if ui.button("DISCARD").clicked() {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                self.history
                    .record(super::history::Edit::SetPlayer { before, after });
                self.feel_changed = None;
            }
            Some(false) => {
                *player = *before;
                self.feel_changed = None;
            }
            None => {}
        }
    }
}
