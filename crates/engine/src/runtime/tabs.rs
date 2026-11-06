//! Editor: scene and model tabs, from the runtime's side (loading files
//! and rebuilding what is drawn and simulated).

use super::Running;
use crate::editor::history::Change;
use crate::physics::PhysicsWorld;
use crate::scene::Scene;
use glam::Vec3;
use std::path::PathBuf;

impl Running {
    /// Editor: redraws what an edit changed. Adding or removing objects
    /// shifts the indices the draws refer to, so those rebuild all draws.
    pub(super) fn show_change(&mut self, change: Change) {
        match change {
            Change::Modified(index) => {
                self.renderer
                    .update_object(&self.scene, index, &mut self.assets);
            }
            Change::Settings => self.renderer.set_zones(&self.scene, &self.assets),
            Change::Player => {}
            Change::Added(_) | Change::Removed(_) | Change::All => {
                let selected = self.editor.as_ref().and_then(|e| e.selected);
                self.renderer.set_highlight(selected);
                self.highlighted = selected;
                self.renderer.set_scene(&self.scene, &mut self.assets);
            }
        }
    }
    /// Editor: replace the scene with another file from the project.
    pub(super) fn open_scene(&mut self, path: PathBuf) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match Scene::load(&self.game_dir.join(&path)) {
            Ok(scene) => {
                self.scene = scene;
                editor.scene_loaded(path);
                self.scene_swapped();
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string());
            }
        }
    }

    /// Editor: opens a scene from the project in a new tab.
    pub(super) fn open_tab(&mut self, path: PathBuf) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match Scene::load(&self.game_dir.join(&path)) {
            Ok(loaded) => {
                editor.open_tab(path, loaded, &mut self.scene, &self.fly);
                self.scene_swapped();
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string());
            }
        }
    }

    /// Editor: opens a model from the project in a new model tab.
    pub(super) fn open_model(&mut self, path: PathBuf) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if let Some(index) = editor.tab_of(&path) {
            self.switch_tab(index);
            return;
        }
        match crate::modeler::glb::load(&self.game_dir.join(&path)) {
            Ok(mesh) => {
                self.models_opened += 1;
                // A cache key that is no file's path: the model as edited.
                let key = format!("<modeler {}>", self.models_opened);
                let data = crate::modeler::model_data(&mesh, &mut self.assets);
                self.assets.insert_model(&key, data);
                let doc = crate::editor::ModelDoc::new(mesh, key.clone());
                let stage = crate::editor::stage(&key);
                editor.open_model_tab(path, doc, stage, &mut self.scene, &self.fly);
                // Look at the model from the front, a little above.
                let eye = Vec3::new(1.8, 1.6, 2.6);
                let forward = -eye.normalize();
                let camera = &mut self.fly.camera;
                camera.position = eye;
                camera.yaw = forward.x.atan2(-forward.z);
                camera.pitch = forward.y.asin();
                self.scene_swapped();
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string().to_uppercase());
            }
        }
    }

    /// Editor: makes another tab the active one.
    pub(super) fn switch_tab(&mut self, index: usize) {
        if let Some(editor) = &mut self.editor
            && editor.switch_tab(index, &mut self.scene, &mut self.fly)
        {
            self.scene_swapped();
        }
    }

    /// Editor: closes a tab (its unsaved changes were already dealt with);
    /// closing the active one moves to its neighbour first.
    pub(super) fn close_tab(&mut self, index: usize) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.tab_count() < 2 {
            return;
        }
        if index == editor.active_tab() {
            let next = if index + 1 < editor.tab_count() {
                index + 1
            } else {
                index - 1
            };
            self.switch_tab(next);
        }
        if let Some(editor) = &mut self.editor {
            editor.remove_tab(index);
        }
    }
    /// A different scene is now in `self.scene`: rebuild what is made from
    /// it and load its bakes.
    pub(super) fn scene_swapped(&mut self) {
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
        self.zone.reset();
        self.highlighted = None;
        self.renderer.set_highlight(None);
        self.load_bake();
        self.load_lightmaps();
    }
}
