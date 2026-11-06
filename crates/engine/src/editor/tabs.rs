//! Scene tabs: several scenes open at once. Only the active tab's scene is
//! live (drawn, simulated, edited); the others are *parked* with everything
//! that belongs to them (undo history, selection, camera) and swapped back
//! in when their tab is picked.

use super::history::History;
use super::{Confirm, Editor, EditorOutput, SettingsTarget, dim, scene_files};
use crate::camera::FlyCamera;
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use crate::ui::theme;
use egui::{RichText, Ui};
use glam::Vec3;
use std::path::{Path, PathBuf};

/// The editor state that belongs to one scene.
#[derive(Default)]
pub(super) struct Document {
    pub scene_path: PathBuf,
    pub saved_state: Option<u64>,
    pub history: History,
    pub selected: Option<usize>,
    pub settings: Option<SettingsTarget>,
    pub search: String,
    pub probes: Vec<Vec3>,
}

/// A tab that isn't the active one.
pub(super) struct Parked {
    pub doc: Document,
    pub scene: Scene,
    pub camera: FlyCamera,
}

impl Parked {
    fn is_dirty(&self) -> bool {
        self.doc.history.state() != self.doc.saved_state
    }
}

impl Editor {
    /// Moves the active scene's editor state out (committing any edit in
    /// progress first).
    fn take_document(&mut self, scene: &Scene) -> Document {
        self.commit_pending(scene);
        Document {
            scene_path: std::mem::take(&mut self.scene_path),
            saved_state: self.saved_state.take(),
            history: std::mem::take(&mut self.history),
            selected: self.selected.take(),
            settings: self.settings.take(),
            search: std::mem::take(&mut self.search),
            probes: std::mem::take(&mut self.probes),
        }
    }

    fn restore_document(&mut self, doc: Document) {
        self.scene_path = doc.scene_path;
        self.saved_state = doc.saved_state;
        self.history = doc.history;
        self.selected = doc.selected;
        self.settings = doc.settings;
        self.search = doc.search;
        self.probes = doc.probes;
        self.pending = None;
        self.zone_pending = None;
        self.player_pending = None;
        self.renaming = None;
        self.reveal_selection = true;
    }

    /// Makes tab `index` the active one, swapping its scene and camera into
    /// `scene` and `camera`. Returns false if there was nothing to switch.
    pub fn switch_tab(&mut self, index: usize, scene: &mut Scene, camera: &mut FlyCamera) -> bool {
        if index == self.active_tab {
            return false;
        }
        let Some(target) = self.tabs.get_mut(index).and_then(Option::take) else {
            return false;
        };
        let doc = self.take_document(scene);
        self.tabs[self.active_tab] = Some(Parked {
            doc,
            scene: std::mem::replace(scene, target.scene),
            camera: std::mem::replace(camera, target.camera),
        });
        self.restore_document(target.doc);
        self.active_tab = index;
        true
    }

    /// Opens `loaded` (read from `path`) in a new tab after the others and
    /// makes it active; the camera stays where it is.
    pub fn open_tab(
        &mut self,
        path: PathBuf,
        loaded: Scene,
        scene: &mut Scene,
        camera: &FlyCamera,
    ) {
        let doc = self.take_document(scene);
        self.tabs[self.active_tab] = Some(Parked {
            doc,
            scene: std::mem::replace(scene, loaded),
            camera: camera.clone(),
        });
        self.tabs.push(None);
        self.active_tab = self.tabs.len() - 1;
        self.restore_document(Document::default());
        self.scene_loaded(path);
    }

    /// Drops a tab that isn't the active one (its changes are lost).
    pub fn remove_tab(&mut self, index: usize) {
        if index == self.active_tab || index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if index < self.active_tab {
            self.active_tab -= 1;
        }
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab(&self) -> usize {
        self.active_tab
    }

    /// The tab that has `path` open, if any.
    pub fn tab_of(&self, path: &Path) -> Option<usize> {
        (0..self.tabs.len()).find(|&i| self.tab_path(i) == path)
    }

    fn tab_path(&self, index: usize) -> &Path {
        match &self.tabs[index] {
            Some(parked) => &parked.doc.scene_path,
            None => &self.scene_path,
        }
    }

    fn tab_dirty(&self, index: usize) -> bool {
        match &self.tabs[index] {
            Some(parked) => parked.is_dirty(),
            None => self.is_dirty(),
        }
    }

    /// Whether any open scene has unsaved changes.
    pub fn any_dirty(&self) -> bool {
        (0..self.tabs.len()).any(|i| self.tab_dirty(i))
    }

    /// Paths of the scenes with unsaved changes.
    pub(super) fn dirty_paths(&self) -> Vec<String> {
        (0..self.tabs.len())
            .filter(|&i| self.tab_dirty(i))
            .map(|i| self.tab_label(i))
            .collect()
    }

    /// Tab `index`'s scene file, as the dialogs show it.
    pub(super) fn tab_label(&self, index: usize) -> String {
        self.tab_path(index).display().to_string().to_uppercase()
    }

    /// Saves tab `index` (the active scene is `scene`). Returns whether it
    /// was saved; `settled` is set if a pending edit was committed first.
    pub(super) fn save_tab(
        &mut self,
        index: usize,
        scene: &Scene,
        player: &PlayerSettings,
        settled: &mut bool,
    ) -> bool {
        let Some(parked) = &self.tabs[index] else {
            *settled |= self.save(scene, player);
            return !self.is_dirty();
        };
        let path = self.project_dir.join(&parked.doc.scene_path);
        let saved = parked
            .scene
            .save(&path)
            .map_err(|e| e.to_string())
            .and_then(|()| self.save_player(player));
        match saved {
            Ok(_) => {
                log::info!("saved {}", path.display());
                let name = parked.doc.scene_path.display().to_string();
                if let Some(parked) = &mut self.tabs[index] {
                    parked.doc.saved_state = parked.doc.history.state();
                }
                self.message = Some((format!("SAVED {name}"), false));
                true
            }
            Err(err) => {
                log::error!("{err}");
                self.message = Some((err, true));
                false
            }
        }
    }

    /// Saves every scene with unsaved changes. Returns whether all of them
    /// were saved.
    pub(super) fn save_all(
        &mut self,
        scene: &Scene,
        player: &PlayerSettings,
        settled: &mut bool,
    ) -> bool {
        (0..self.tabs.len())
            .filter(|&i| self.tab_dirty(i))
            .collect::<Vec<_>>()
            .into_iter()
            .all(|i| self.save_tab(i, scene, player, settled))
    }

    /// The row of tabs above the viewport, and `+` to open another scene.
    pub(super) fn tab_strip(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        ui.horizontal(|ui| {
            // Swapping scenes mid-play would lose the play snapshot.
            ui.add_enabled_ui(!self.playing, |ui| {
                let closable = self.tabs.len() > 1;
                for index in 0..self.tabs.len() {
                    let active = index == self.active_tab;
                    let name = self
                        .tab_path(index)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_uppercase())
                        .unwrap_or_default();
                    let mark = if self.tab_dirty(index) { "*" } else { " " };
                    let text = RichText::new(format!("{name}{mark}"));
                    let text = if active {
                        text.color(theme::AMBER).strong()
                    } else {
                        text.color(theme::AMBER_DIM)
                    };
                    let tab = ui
                        .selectable_label(active, text)
                        .on_hover_text(self.tab_path(index).display().to_string());
                    if tab.clicked() && !active {
                        output.switch_tab = Some(index);
                    }
                    if tab.middle_clicked() && closable {
                        self.close_tab(index, output);
                    }
                    if closable
                        && ui
                            .small_button("x")
                            .on_hover_text("CLOSE THIS TAB")
                            .clicked()
                    {
                        self.close_tab(index, output);
                    }
                    ui.add_space(6.0);
                }
                ui.menu_button("+", |ui| {
                    ui.label(dim("OPEN IN A NEW TAB"));
                    for path in scene_files(&self.project_dir) {
                        let open = self.tab_of(&path);
                        let mut text = path.display().to_string().to_uppercase();
                        if open.is_some() {
                            text.push_str("  (OPEN)");
                        }
                        if ui.button(text).clicked() {
                            match open {
                                Some(index) => output.switch_tab = Some(index),
                                None => output.new_tab = Some(path),
                            }
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("OPEN ANOTHER SCENE IN A NEW TAB");
            });
        });
    }

    /// Closes a tab, asking first if it has unsaved changes.
    fn close_tab(&mut self, index: usize, output: &mut EditorOutput) {
        if self.tab_dirty(index) {
            self.confirm = Some(Confirm::CloseTab(index));
        } else {
            output.close_tab = Some(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::test_scene;

    fn boxes(count: usize) -> Scene {
        test_scene(
            (0..count)
                .map(|i| (Shape::Box { size: Vec3::ONE }, Vec3::X * i as f32))
                .collect(),
        )
    }

    #[test]
    fn each_tab_keeps_its_scene_history_selection_and_camera() {
        let mut editor = Editor::new("test".into(), PathBuf::new(), "scenes/a.ron".into());
        let mut scene = boxes(1);
        let mut fly = FlyCamera::new(Vec3::ZERO, 0.0, 0.0);
        let mut player = PlayerSettings::default();

        // An edit in A, then open B and edit it too.
        editor.selected = Some(0);
        editor.duplicate_selected(&mut scene);
        editor.open_tab("scenes/b.ron".into(), boxes(3), &mut scene, &fly);
        assert_eq!((editor.tab_count(), editor.active_tab()), (2, 1));
        assert_eq!(editor.selected, None);
        assert!(!editor.is_dirty());
        fly.camera.position = Vec3::Y;
        editor.selected = Some(2);
        editor.delete_selected(&mut scene);
        assert_eq!(scene.objects.len(), 2);
        assert_eq!(editor.dirty_paths().len(), 2);

        // Back to A: its scene, selection, camera and undo come back.
        assert!(editor.switch_tab(0, &mut scene, &mut fly));
        assert_eq!(editor.scene_path, Path::new("scenes/a.ron"));
        assert_eq!(scene.objects.len(), 2);
        assert_eq!(editor.selected, Some(1));
        assert_eq!(fly.camera.position, Vec3::ZERO);
        editor.undo(&mut scene, &mut player);
        assert_eq!(scene.objects.len(), 1);
        assert!(!editor.is_dirty());

        // B still has its own edit to undo.
        assert!(editor.switch_tab(1, &mut scene, &mut fly));
        assert_eq!(fly.camera.position, Vec3::Y);
        editor.undo(&mut scene, &mut player);
        assert_eq!(scene.objects.len(), 3);
        assert!(!editor.any_dirty());
    }

    #[test]
    fn removing_a_tab_before_the_active_one_keeps_it_active() {
        let mut editor = Editor::new("test".into(), PathBuf::new(), "scenes/a.ron".into());
        let mut scene = boxes(1);
        let fly = FlyCamera::new(Vec3::ZERO, 0.0, 0.0);
        editor.open_tab("scenes/b.ron".into(), boxes(2), &mut scene, &fly);
        editor.open_tab("scenes/c.ron".into(), boxes(3), &mut scene, &fly);
        editor.remove_tab(2); // the active tab can't be removed
        assert_eq!(editor.tab_count(), 3);
        editor.remove_tab(0);
        assert_eq!((editor.tab_count(), editor.active_tab()), (2, 1));
        assert_eq!(editor.scene_path, Path::new("scenes/c.ron"));
        assert_eq!(editor.tab_of(Path::new("scenes/b.ron")), Some(0));
        assert_eq!(editor.tab_of(Path::new("scenes/a.ron")), None);
    }
}
