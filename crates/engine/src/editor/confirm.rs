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
    /// Close the editor (every tab).
    Close,
    /// Leave this project for another one.
    Project(super::file_dialogs::Switch),
}

impl Editor {
    /// The window wants to close. Returns true if it may close now;
    /// otherwise asks about unsaved changes first.
    pub fn request_close(&mut self) -> bool {
        if self.any_dirty() {
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
                Confirm::Close | Confirm::Project(_) => self.dirty_paths().join(", "),
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
        if save {
            let saved = match &confirm {
                Confirm::CloseTab(index) => {
                    self.save_tab(*index, scene, player, &mut output.settled)
                }
                Confirm::Close | Confirm::Project(_) => {
                    self.save_all(scene, player, &mut output.settled)
                }
            };
            if !saved {
                return; // saving failed; the error is in the status bar
            }
        }
        match confirm {
            Confirm::CloseTab(index) => output.close_tab = Some(index),
            Confirm::Close => output.exit = true,
            Confirm::Project(switch) => self.switch_project(switch, output),
        }
    }
}
