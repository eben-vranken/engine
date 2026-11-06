//! NEW SCENE and NEW MODEL: naming the new file in the editor. Also SAVE
//! SCENE AS once its file is picked.

use super::{Editor, EditorOutput, dim};
use crate::modeler::mesh::EditMesh;
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use egui::RichText;
use glam::Vec3;
use std::path::{Path, PathBuf};

/// What the name being typed is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Naming {
    New,
    /// A new model in `models/`.
    Model,
}

/// What a new model starts as.
const STARTS: [&str; 4] = ["BOX", "PLANE", "CYLINDER", "SPHERE"];

fn start_mesh(index: usize) -> EditMesh {
    match index {
        1 => EditMesh::plane(glam::Vec2::ONE),
        2 => EditMesh::cylinder(0.5, 1.0, 8),
        3 => EditMesh::sphere(0.5, 8, 6),
        _ => EditMesh::cube(Vec3::ONE),
    }
}

/// The name dialog's state.
pub(super) struct NameDialog {
    pub purpose: Naming,
    pub text: String,
    /// Why the last name was refused.
    pub error: Option<String>,
    /// New models: which of `STARTS`.
    pub start: usize,
}

/// Checks a typed scene name; returns its file relative to the project
/// (`scenes/<name>.ron`).
pub fn scene_file(project_dir: &Path, name: &str) -> Result<PathBuf, String> {
    project_file(project_dir, "scenes", "ron", name)
}

/// Checks a typed name for a new file `<folder>/<name>.<extension>` in the
/// project; returns that path, relative to the project.
fn project_file(
    project_dir: &Path,
    folder: &str,
    extension: &str,
    name: &str,
) -> Result<PathBuf, String> {
    let name = name.trim();
    let name = name.strip_suffix(&format!(".{extension}")).unwrap_or(name);
    if name.is_empty() {
        return Err("TYPE A NAME".into());
    }
    if !crate::project::is_valid_name(name) {
        return Err("USE LETTERS, DIGITS, SPACES, - AND _ ONLY".into());
    }
    let relative = Path::new(folder).join(format!("{name}.{extension}"));
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
            start: 0,
        });
    }

    /// The name dialog, while one is open.
    pub(super) fn name_dialog(&mut self, ui: &egui::Ui, output: &mut EditorOutput) {
        let Some(dialog) = &mut self.naming else {
            return;
        };
        let (title, hint) = match dialog.purpose {
            Naming::New => ("NEW SCENE", "CREATED IN SCENES/ AND OPENED IN A NEW TAB"),
            Naming::Model => ("NEW MODEL", "CREATED IN MODELS/ AND OPENED IN A NEW TAB"),
        };
        let mut accept = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("scene_name")).show(ui.ctx(), |ui| {
            ui.label(RichText::new(title).strong());
            ui.label(dim(hint));
            ui.add_space(6.0);
            let field = ui.add(egui::TextEdit::singleline(&mut dialog.text).hint_text("NAME"));
            field.request_focus();
            if field.changed() {
                dialog.error = None;
            }
            if dialog.purpose == Naming::Model {
                ui.horizontal(|ui| {
                    ui.label(dim("START FROM"));
                    for (i, start) in STARTS.iter().enumerate() {
                        if ui.selectable_label(dialog.start == i, *start).clicked() {
                            dialog.start = i;
                        }
                    }
                });
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
        let (purpose, start) = (dialog.purpose, dialog.start);
        let result = match purpose {
            Naming::Model => project_file(&self.project_dir, "models", "glb", &dialog.text)
                .and_then(|path| self.create_model(&path, start, output)),
            Naming::New => scene_file(&self.project_dir, &dialog.text)
                .and_then(|path| self.create_scene(&path, output)),
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

    /// Writes a new model to `path` and opens it in a model tab.
    fn create_model(
        &mut self,
        path: &Path,
        start: usize,
        output: &mut EditorOutput,
    ) -> Result<(), String> {
        let file = self.project_dir.join(path);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        crate::modeler::glb::save(&file, &start_mesh(start), &self.project_dir)
            .map_err(|e| e.to_string())?;
        log::info!("created {}", file.display());
        output.open_model = Some(path.to_owned());
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
        assert!(scene_file(&sandbox, "hall").is_err(), "taken");
    }
}
