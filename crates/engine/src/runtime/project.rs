//! Editor: files coming into and going out of the project (dropped
//! files, packages).

use super::Running;
use crate::camera::Camera;
use crate::editor::import;
use crate::physics::PhysicsWorld;
use crate::render::quality::Quality;
use glam::{Vec2, Vec3};
use std::path::{Path, PathBuf};

impl Running {
    /// Editor: leaves this project for the one in `dir` by starting the
    /// editor again on it (with the same command-line options) and closing
    /// this one. Unsaved changes were already dealt with.
    pub(super) fn open_project(&mut self, dir: &Path) {
        match start_editor(dir, self.quality, self.perf_log) {
            Ok(()) => self.exit_requested = true,
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err);
                }
            }
        }
    }

    /// Editor: copies dropped model files into the project and places each
    /// on the surface under the drop point. Dropped WAV files are copied to
    /// `audio/steps/` and, while Player Start is selected, added to the
    /// footsteps.
    pub(super) fn import_dropped(&mut self, camera: &Camera) {
        for (file, cursor) in std::mem::take(&mut self.dropped) {
            let Some(editor) = &mut self.editor else {
                return;
            };
            if crate::package::is_package(&file) {
                if let Some(dir) = editor.open_dropped_package(&file) {
                    self.open_project(&dir);
                }
                continue;
            }
            // A prefab from the PREFABS list: it's in the project already.
            let relative = file
                .strip_prefix(&self.game_dir)
                .ok()
                .map(Path::to_path_buf);
            if let Some(relative) = relative.filter(|p| crate::prefab::is_prefab(p)) {
                let path = crate::graph::key(&relative);
                let bounds = crate::prefab::bounds(&mut self.assets, &path)
                    .unwrap_or((Vec3::splat(-0.25), Vec3::splat(0.25)));
                let position = self.drop_position(camera, cursor, bounds);
                let Some(editor) = &mut self.editor else {
                    return;
                };
                if let Some(change) = editor.add_prefab(&mut self.scene, path, position) {
                    self.show_change(change);
                    self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
                }
                continue;
            }
            if import::is_sound(&file) {
                match import::copy_sound_into_project(&file, &self.game_dir) {
                    Ok(path) if editor.shows_player(&self.scene) => {
                        editor.show_message(format!("ADDED {path} TO THE FOOTSTEPS"));
                        editor.add_footstep(&self.scene, &mut self.settings, path);
                    }
                    Ok(path) => editor.show_message(format!(
                        "COPIED TO {path}; SELECT PLAYER START TO USE IT FOR FOOTSTEPS"
                    )),
                    Err(err) => {
                        log::error!("{err}");
                        editor.show_error(err.to_string());
                    }
                }
                continue;
            }
            let path = match import::copy_into_project(&file, &self.game_dir) {
                Ok(path) => path,
                Err(err) => {
                    log::error!("{err}");
                    editor.show_error(err.to_string());
                    continue;
                }
            };
            // It may have been cached as missing before the copy.
            self.assets.forget(&path);
            let bounds = self.assets.model(&path).map_or(
                (Vec3::splat(-0.25), Vec3::splat(0.25)), // the placeholder box
                |model| model.mesh.bounds(),
            );
            let position = self.drop_position(camera, cursor, bounds);
            let Some(editor) = &mut self.editor else {
                return;
            };
            if let Some(change) = editor.add_model(&mut self.scene, path, position) {
                self.show_change(change);
                self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
            }
        }
    }
    /// Where something dropped at `cursor` (or placed with `+`) goes: on
    /// the surface under it, or ahead of the camera; `bounds` around its
    /// origin.
    fn drop_position(&self, camera: &Camera, cursor: Vec2, bounds: (Vec3, Vec3)) -> Vec3 {
        // Dropped on a panel: aim through the middle of the view.
        let [x, y, w, h] = self.renderer.image_rect();
        let ndc = Vec2::new(
            (cursor.x - x) / w * 2.0 - 1.0,
            1.0 - (cursor.y - y) / h * 2.0,
        );
        let ndc = if ndc.abs().max_element() <= 1.0 {
            ndc
        } else {
            Vec2::ZERO
        };
        let (origin, direction) = camera.ray(ndc, w / h);
        let surface = self.physics.surface(origin, direction, camera.far);
        let ahead = origin + direction * crate::editor::NEW_OBJECT_DISTANCE;
        crate::editor::placement(bounds, surface, ahead)
    }

    /// Editor: makes a prefab of the model at `model` (relative to the
    /// project) in `prefabs/`, and opens it in a tab to be added to.
    pub(super) fn make_prefab(&mut self, model: &str) {
        let path = crate::prefab::new_path(&self.game_dir, model);
        let file = self.game_dir.join(&path);
        let written = std::fs::create_dir_all(file.parent().unwrap_or(&self.game_dir))
            .map_err(|e| e.to_string())
            .and_then(|()| {
                crate::prefab::of_model(model)
                    .save(&file)
                    .map_err(|e| e.to_string())
            });
        match written {
            Ok(()) => {
                log::info!("made the prefab {path}");
                self.open_tab(PathBuf::from(&path));
                if let Some(editor) = &mut self.editor {
                    editor.show_message(format!(
                        "MADE {path}: ADD A LIGHT OR ANYTHING ELSE, SAVE, THEN PLACE IT FROM PREFABS"
                    ));
                }
            }
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err);
                }
            }
        }
    }

    /// Editor: writes the project folder to a package at `out`.
    pub(super) fn export_package(&mut self, out: &Path) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match crate::package::export(&self.game_dir, &editor.game_name, out) {
            Ok(count) => {
                log::info!("exported {} ({count} files)", out.display());
                let unsaved = if editor.any_dirty() {
                    "; UNSAVED CHANGES ARE NOT IN IT"
                } else {
                    ""
                };
                editor.show_message(format!(
                    "EXPORTED {} ({count} FILES){unsaved}",
                    out.display()
                ));
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string());
            }
        }
    }

    /// Editor: writes the project as a game executable at `out`.
    pub(super) fn export_game(&mut self, out: &Path) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match crate::standalone::export(&self.game_dir, &editor.game_name, out) {
            Ok(count) => {
                log::info!("exported the game {} ({count} files)", out.display());
                let unsaved = if editor.any_dirty() {
                    "; UNSAVED CHANGES ARE NOT IN IT"
                } else {
                    ""
                };
                // The game runs on a copy of this engine as it was built.
                let debug = if cfg!(debug_assertions) {
                    "; THIS ENGINE IS A DEBUG BUILD: EXPORT FROM A RELEASE BUILD FOR PLAYERS"
                } else {
                    ""
                };
                editor.show_message(format!(
                    "EXPORTED THE GAME {} ({count} FILES){unsaved}{debug}",
                    out.display()
                ));
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string());
            }
        }
    }
}

/// Starts another editor process on the project in `dir`.
pub(crate) fn start_editor(dir: &Path, quality: Quality, perf_log: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("--editor")
        .arg("--game")
        .arg(dir)
        .arg("--quality")
        .arg(quality.name().to_lowercase());
    if perf_log {
        command.arg("--perf-log");
    }
    command.spawn().map_err(|e| e.to_string())?;
    log::info!("opening {} in a new editor", dir.display());
    Ok(())
}
