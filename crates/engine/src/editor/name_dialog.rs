//! NEW SCENE and NEW GRAPH: naming the new file in the editor. Also SAVE
//! SCENE AS once its file is picked.

use super::{Editor, EditorOutput, dialog_heading};
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use egui::RichText;
use std::path::{Path, PathBuf};

/// What the name being typed is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Naming {
    Scene,
    Graph,
}

/// The name dialog's state.
pub(super) struct NameDialog {
    pub purpose: Naming,
    pub text: String,
    /// Why the last name was refused.
    pub error: Option<String>,
}

/// Checks a typed scene name; returns its file relative to the project
/// (`scenes/<name>.ron`).
pub fn scene_file(project_dir: &Path, name: &str) -> Result<PathBuf, String> {
    project_file(project_dir, "scenes", name)
}

/// Checks a typed name for a new `<folder>/<name>.ron` in the project;
/// returns that path, relative to the project.
fn project_file(project_dir: &Path, folder: &str, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    let name = name.strip_suffix(".ron").unwrap_or(name);
    if name.is_empty() {
        return Err("TYPE A NAME".into());
    }
    if !crate::project::is_valid_name(name) {
        return Err("USE LETTERS, DIGITS, SPACES, - AND _ ONLY".into());
    }
    let relative = Path::new(folder).join(format!("{name}.ron"));
    if project_dir.join(&relative).exists() {
        return Err(format!("{} ALREADY EXISTS", relative.display()).to_uppercase());
    }
    Ok(relative)
}

impl Editor {
    pub(super) fn ask_name(&mut self, purpose: Naming) {
        self.naming = Some(NameDialog {
            purpose,
            text: String::new(),
            error: None,
        });
    }

    /// The name dialog, while one is open.
    pub(super) fn name_dialog(&mut self, ui: &egui::Ui, output: &mut EditorOutput) {
        let Some(dialog) = &mut self.naming else {
            return;
        };
        let mut accept = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("scene_name")).show(ui.ctx(), |ui| {
            let (title, hint) = match dialog.purpose {
                Naming::Scene => ("NEW SCENE", "CREATED IN SCENES/ AND OPENED IN A NEW TAB"),
                Naming::Graph => ("NEW GRAPH", "CREATED IN GRAPHS/ AND OPENED IN A NEW TAB"),
            };
            dialog_heading(ui, title, hint);
            let field = ui.add(egui::TextEdit::singleline(&mut dialog.text).hint_text("NAME"));
            field.request_focus();
            if field.changed() {
                dialog.error = None;
            }
            if let Some(error) = &dialog.error {
                ui.label(RichText::new(error).small().color(crate::ui::theme::ALERT));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                accept = ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                cancel =
                    ui.button("CANCEL").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if cancel {
            self.naming = None;
            return;
        }
        if !accept {
            return;
        }
        let result = match dialog.purpose {
            Naming::Scene => scene_file(&self.project_dir, &dialog.text)
                .and_then(|path| self.create_scene(&path, output)),
            Naming::Graph => project_file(&self.project_dir, "graphs", &dialog.text)
                .and_then(|path| self.create_graph(&path, output)),
        };
        match result {
            Ok(()) => self.naming = None,
            Err(error) => {
                if let Some(dialog) = &mut self.naming {
                    dialog.error = Some(error);
                }
            }
        }
    }

    /// Writes a graph with just a START node to `path` and opens it.
    fn create_graph(&mut self, path: &Path, output: &mut EditorOutput) -> Result<(), String> {
        let file = self.project_dir.join(path);
        crate::graph::Graph::starter()
            .save(&file)
            .map_err(|e| e.to_string())?;
        log::info!("created {}", file.display());
        self.index.refresh(&self.project_dir, true);
        output.open_graph = Some(path.to_owned());
        Ok(())
    }

    /// Writes a starter scene to `path` and opens it in a new tab.
    fn create_scene(&mut self, path: &Path, output: &mut EditorOutput) -> Result<(), String> {
        let file = self.project_dir.join(path);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        Scene::starter().save(&file).map_err(|e| e.to_string())?;
        log::info!("created {}", file.display());
        output.new_tab = Some(path.to_owned());
        Ok(())
    }

    /// Saves the scene under `path` (copying its bakes along) and makes the
    /// tab edit that file from now on.
    pub(super) fn save_as(
        &mut self,
        path: &Path,
        scene: &Scene,
        player: &PlayerSettings,
        output: &mut EditorOutput,
    ) -> Result<(), String> {
        let old = self.project_dir.join(&self.scene_path);
        let new = self.project_dir.join(path);
        let previous = std::mem::replace(&mut self.scene_path, path.to_owned());
        output.settled |= self.save(scene, player);
        if self.is_dirty() {
            self.scene_path = previous;
            return Err(self.message.clone().map(|(m, _)| m).unwrap_or_default());
        }
        for path_for in [crate::lightmap::path_for, crate::audio::bake::path_for] {
            let from = path_for(&old);
            if from.exists()
                && let Err(err) = std::fs::copy(&from, path_for(&new))
            {
                log::error!("{}: {err}", from.display());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_names_are_checked() {
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        assert_eq!(
            scene_file(&sandbox, " cellar 2 ").unwrap(),
            Path::new("scenes").join("cellar 2.ron")
        );
        assert_eq!(
            scene_file(&sandbox, "cellar.ron").unwrap(),
            Path::new("scenes").join("cellar.ron")
        );
        assert!(scene_file(&sandbox, "  ").is_err());
        assert!(scene_file(&sandbox, "../escape").is_err());
        assert!(scene_file(&sandbox, "test_level").is_err(), "taken");
    }
}
