//! Editor: scene tabs, from the runtime's side (loading files and
//! rebuilding what is drawn and simulated).

use super::Running;
use crate::editor::history::Change;
use crate::physics::PhysicsWorld;
use crate::scene::Scene;
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
                let selected = self
                    .editor
                    .as_ref()
                    .map(|e| e.selected_objects())
                    .unwrap_or_default();
                self.renderer.set_highlight(&selected);
                self.highlighted = selected;
                self.renderer.set_scene(&self.scene, &mut self.assets);
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

    /// Editor: opens a graph from the project in a new graph tab (or
    /// switches to it if it's open).
    pub(super) fn open_graph(&mut self, path: PathBuf) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if let Some(index) = editor.tab_of(&path) {
            self.switch_tab(index);
            return;
        }
        match crate::graph::Graph::load(&self.game_dir.join(&path)) {
            Ok(graph) => {
                let doc = crate::editor::GraphDoc::new(graph);
                editor.open_graph_tab(path, doc, &mut self.scene, &self.fly);
                self.scene_swapped();
            }
            Err(err) => {
                log::error!("{err}");
                editor.show_error(err.to_string());
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
        self.highlighted.clear();
        self.renderer.set_highlight(&[]);
        self.load_bake();
        self.load_lightmaps();
    }
}
