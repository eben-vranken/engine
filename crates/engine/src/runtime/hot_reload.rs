//! Hot reload: `player.ron` and model files changed on disk, and the
//! tuning panel's save / reset.

use super::Running;
use crate::player::settings::PlayerSettings;
use crate::player::tuning::TuningAction;
use std::time::{Duration, Instant};

/// How often `player.ron` is checked for changes.
pub(super) const RELOAD_INTERVAL: Duration = Duration::from_millis(250);

impl Running {
    /// Hot reload: picks up `player.ron` and model files changed on disk.
    pub(super) fn poll_files(&mut self) {
        if self.last_reload_check.elapsed() < RELOAD_INTERVAL {
            return;
        }
        self.last_reload_check = Instant::now();
        self.poll_settings_file();
        self.poll_model_files();
        self.check_bake();
        self.check_light_bake();
    }
    /// Models re-exported over their file: redraw and re-collide everything.
    pub(super) fn poll_model_files(&mut self) {
        let changed = self.assets.changed_models();
        if changed.is_empty() {
            return;
        }
        log::info!("reloaded {}", changed.join(", "));
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.rebuild_physics();
        if let Some(editor) = &mut self.editor {
            editor.show_message(format!("RELOADED {}", changed.join(", ")));
        }
    }

    pub(super) fn poll_settings_file(&mut self) {
        match self.settings_file.poll() {
            // The editor saving the same values isn't news.
            Some(Ok(settings)) if settings == self.settings => self.settings_error = None,
            Some(Ok(settings)) => {
                self.settings = settings;
                self.settings_error = None;
                self.settings_status = "RELOADED FROM DISK".into();
                log::info!("reloaded {}", self.settings_file.path.display());
            }
            Some(Err(err)) => {
                log::error!("{err}");
                self.settings_error = Some(err.to_string());
            }
            None => {}
        }
    }

    pub(super) fn apply_tuning(&mut self, action: TuningAction) {
        match action {
            TuningAction::Save => match self.settings_file.save(&self.settings) {
                Ok(()) => {
                    self.settings_error = None;
                    self.settings_status = format!(
                        "SAVED TO {}",
                        self.settings_file.path.display().to_string().to_uppercase()
                    );
                }
                Err(err) => self.settings_error = Some(err.to_string()),
            },
            TuningAction::ResetToDefaults => {
                self.settings = PlayerSettings::default();
                self.settings_status = "DEFAULTS RESTORED (NOT SAVED)".into();
            }
        }
    }
}
