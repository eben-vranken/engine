//! Editor: files coming into and going out of the project (dropped
//! files, packages).

use super::Running;
use crate::camera::Camera;
use crate::editor::{Editor, import};
use crate::physics::PhysicsWorld;
use crate::render::quality::Quality;
use glam::{Vec2, Vec3};
use std::path::Path;

impl Running {
    /// Editor: leaves this project for the one in `dir` by starting the
    /// editor again on it (with the same command-line options) and closing
    /// this one. Unsaved changes were already dealt with.
    pub(super) fn open_project(&mut self, dir: &Path) {
        match start_editor(dir, self.quality, self.perf_log.is_some()) {
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
                open_package_window(&file, editor, self.quality, self.perf_log.is_some());
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
            let position = crate::editor::placement(bounds, surface, ahead);
            if let Some(change) = editor.add_model(&mut self.scene, path, position) {
                self.show_change(change);
                self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
            }
        }
    }
    /// Editor: writes the project folder to a package next to it.
    pub(super) fn export_package(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let out = crate::package::export_path(&self.game_dir);
        match crate::package::export(&self.game_dir, &editor.game_name, &out) {
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
}

/// A package dropped on the editor: unpack it next to itself and open it in
/// a second editor window (this one keeps its project).
pub(super) fn open_package_window(
    file: &Path,
    editor: &mut Editor,
    quality: Quality,
    perf_log: bool,
) {
    let opened = crate::package::unpack(file)
        .map_err(|e| e.to_string())
        .and_then(|(dir, _)| start_editor(&dir, quality, perf_log).map(|()| dir));
    match opened {
        Ok(dir) => editor.show_message(format!("UNPACKED INTO {}; OPENING IT", dir.display())),
        Err(err) => {
            log::error!("{err}");
            editor.show_error(err);
        }
    }
}

/// Starts another editor process on the project in `dir`.
fn start_editor(dir: &Path, quality: Quality, perf_log: bool) -> Result<(), String> {
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
