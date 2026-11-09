//! LOGIC's open documents (graphs and scripts), apart from BUILD's scenes.
//! Opening a graph doesn't touch the scene: the 3D view, its selection and
//! the inspector stay as they were, ready when BUILD comes back. The active
//! document is `Editor::graph` or `Editor::script` (at `logic_path`); the
//! others wait in `logic_tabs`.

use super::{Confirm, Editor, EditorOutput, GraphDoc, ScriptDoc, Workspace, dim};
use crate::ui::theme;
use egui::{RichText, Ui};
use std::path::{Path, PathBuf};

/// A LOGIC document that isn't the active one (or the active one, put
/// away while BUILD is in front).
pub(super) struct LogicTab {
    pub path: PathBuf,
    pub graph: Option<GraphDoc>,
    pub script: Option<ScriptDoc>,
}

impl LogicTab {
    fn is_dirty(&self) -> bool {
        self.graph.as_ref().is_some_and(GraphDoc::is_dirty)
            || self.script.as_ref().is_some_and(ScriptDoc::is_dirty)
    }
}

/// What to open.
pub(super) enum LogicDoc {
    Graph(GraphDoc),
    Script(ScriptDoc),
}

impl Editor {
    /// Puts the active LOGIC document back in its slot.
    fn park_logic(&mut self) {
        let Some(active) = self.logic_active else {
            return;
        };
        if self.graph.is_none() && self.script.is_none() {
            return;
        }
        if let Some(slot) = self.logic_tabs.get_mut(active) {
            *slot = Some(LogicTab {
                path: self.logic_path.clone(),
                graph: self.graph.take(),
                script: self.script.take(),
            });
        }
    }

    /// Makes slot `index` the active document.
    fn unpark_logic(&mut self, index: usize) {
        let Some(tab) = self.logic_tabs.get_mut(index).and_then(Option::take) else {
            return;
        };
        self.logic_path = tab.path;
        self.graph = tab.graph;
        self.script = tab.script;
        self.logic_active = Some(index);
    }

    /// Opens a graph or script in LOGIC (in front).
    pub(super) fn open_logic(&mut self, path: PathBuf, doc: LogicDoc) {
        if let Some(index) = self.logic_tab_of(&path) {
            self.switch_logic(index);
            return;
        }
        self.project_page = None;
        if self.workspace == Workspace::Logic {
            self.park_logic();
        } else {
            self.park_logic();
            self.workspace = Workspace::Logic;
        }
        self.logic_tabs.push(None);
        self.logic_active = Some(self.logic_tabs.len() - 1);
        self.logic_path = path;
        match doc {
            LogicDoc::Graph(graph) => self.graph = Some(graph),
            LogicDoc::Script(script) => self.script = Some(script),
        }
    }

    /// The LOGIC tab that has `path` open.
    pub fn logic_tab_of(&self, path: &Path) -> Option<usize> {
        (0..self.logic_tabs.len()).find(|&i| self.logic_tab_path(i) == path)
    }

    fn logic_tab_path(&self, index: usize) -> &Path {
        match &self.logic_tabs[index] {
            Some(tab) => &tab.path,
            None => &self.logic_path,
        }
    }

    fn logic_tab_dirty(&self, index: usize) -> bool {
        match &self.logic_tabs[index] {
            Some(tab) => tab.is_dirty(),
            None => {
                self.graph.as_ref().is_some_and(GraphDoc::is_dirty)
                    || self.script.as_ref().is_some_and(ScriptDoc::is_dirty)
            }
        }
    }

    /// LOGIC documents with unsaved changes, as the dialogs show them.
    pub(super) fn dirty_logic_paths(&self) -> Vec<String> {
        (0..self.logic_tabs.len())
            .filter(|&i| self.logic_tab_dirty(i))
            .map(|i| self.logic_tab_path(i).display().to_string().to_uppercase())
            .collect()
    }

    /// Switches to LOGIC tab `index` (into LOGIC if BUILD is in front).
    pub fn switch_logic(&mut self, index: usize) {
        if index >= self.logic_tabs.len() {
            return;
        }
        self.project_page = None;
        if self.workspace == Workspace::Logic && self.logic_active == Some(index) {
            return;
        }
        self.park_logic();
        self.workspace = Workspace::Logic;
        self.unpark_logic(index);
    }

    /// Back to LOGIC's last document. Returns false if none is open.
    pub(super) fn resume_logic(&mut self) -> bool {
        match self.logic_active {
            Some(index) if index < self.logic_tabs.len() => {
                self.workspace = Workspace::Logic;
                self.project_page = None;
                if self.graph.is_none() && self.script.is_none() {
                    self.unpark_logic(index);
                }
                true
            }
            _ => false,
        }
    }

    /// Leaves LOGIC for BUILD: the active document waits in its slot.
    pub(super) fn leave_logic(&mut self) {
        self.park_logic();
        self.workspace = Workspace::Build;
    }

    /// Saves LOGIC tab `index`. Returns whether it was saved.
    pub(super) fn save_logic_tab(&mut self, index: usize) -> bool {
        if self.logic_tabs.get(index).is_some_and(Option::is_none) {
            // The active one (in front, or waiting).
            let was = self.workspace;
            self.unpark_if_waiting(index);
            if self.graph.is_some() {
                self.save_graph();
            } else if self.script.is_some() {
                self.save_script();
            }
            if was == Workspace::Build {
                self.park_logic();
            }
            return !self.logic_tab_dirty(index);
        }
        let dir = self.project_dir.clone();
        let Some(Some(tab)) = self.logic_tabs.get_mut(index) else {
            return false;
        };
        let path = dir.join(&tab.path);
        let result = match (&mut tab.graph, &mut tab.script) {
            (Some(doc), _) => doc.graph.save(&path).map_err(|e| e.to_string()).map(|()| {
                doc.mark_saved();
            }),
            (_, Some(doc)) => std::fs::write(&path, &doc.text)
                .map_err(|e| format!("could not write {}: {e}", path.display()))
                .map(|()| doc.mark_saved()),
            _ => Ok(()),
        };
        match result {
            Ok(()) => true,
            Err(err) => {
                self.show_error(err);
                false
            }
        }
    }

    /// The active slot's document is parked while BUILD is in front; bring
    /// it into `graph` / `script` to save it.
    fn unpark_if_waiting(&mut self, index: usize) {
        if self.logic_active == Some(index) && self.graph.is_none() && self.script.is_none() {
            self.unpark_logic(index);
        }
    }

    /// Saves every LOGIC document with unsaved changes.
    pub(super) fn save_all_logic(&mut self) -> bool {
        (0..self.logic_tabs.len())
            .filter(|&i| self.logic_tab_dirty(i))
            .collect::<Vec<_>>()
            .into_iter()
            .all(|i| self.save_logic_tab(i))
    }

    /// Closes LOGIC tab `index` (unsaved changes already dealt with).
    pub(super) fn remove_logic_tab(&mut self, index: usize) {
        if index >= self.logic_tabs.len() {
            return;
        }
        if self.logic_active == Some(index) {
            self.graph = None;
            self.script = None;
            self.logic_tabs.remove(index);
            self.logic_active = None;
            if !self.logic_tabs.is_empty() {
                let next = index.min(self.logic_tabs.len() - 1);
                if self.workspace == Workspace::Logic {
                    self.unpark_logic(next);
                } else {
                    self.logic_active = Some(next);
                }
            } else if self.workspace == Workspace::Logic {
                self.workspace = Workspace::Build;
            }
        } else {
            self.logic_tabs.remove(index);
            if let Some(active) = &mut self.logic_active
                && index < *active
            {
                *active -= 1;
            }
        }
    }

    /// LOGIC's row of tabs, and `+ OPEN`.
    pub(super) fn logic_tab_strip(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        ui.horizontal(|ui| {
            let mut switch = None;
            let mut close = None;
            for index in 0..self.logic_tabs.len() {
                let active = self.logic_active == Some(index);
                let path = self.logic_tab_path(index).to_path_buf();
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_uppercase())
                    .unwrap_or_default();
                let name = if path.starts_with("scripts") {
                    format!("{stem}.RHAI")
                } else if path.starts_with("graphs/scenes") {
                    format!("{stem} GRAPH")
                } else {
                    stem
                };
                let mark = if self.logic_tab_dirty(index) {
                    "*"
                } else {
                    " "
                };
                let text = RichText::new(format!("{name}{mark}"));
                let text = if active {
                    text.color(theme::AMBER).strong()
                } else {
                    text.color(theme::AMBER_DIM)
                };
                let tab = ui
                    .selectable_label(active, text)
                    .on_hover_text(path.display().to_string());
                if tab.clicked() && !active {
                    switch = Some(index);
                }
                if tab.middle_clicked()
                    || ui
                        .small_button("X")
                        .on_hover_text("CLOSE THIS TAB")
                        .clicked()
                {
                    close = Some(index);
                }
                ui.add_space(6.0);
            }
            ui.menu_button("+ OPEN", |ui| {
                ui.label(dim("OPEN A GRAPH OR SCRIPT"));
                let mut files = super::files::graph_files(&self.project_dir);
                files.extend(super::files::script_files(&self.project_dir));
                for path in files {
                    let open = self.logic_tab_of(&path);
                    let mut text = path.display().to_string().to_uppercase();
                    if open.is_some() {
                        text.push_str("  (OPEN)");
                    }
                    if ui.button(text).clicked() {
                        match open {
                            Some(index) => switch = Some(index),
                            None => output.open_graph = Some(path),
                        }
                        ui.close();
                    }
                }
            });
            if let Some(index) = switch {
                self.switch_logic(index);
            }
            if let Some(index) = close {
                if self.logic_tab_dirty(index) {
                    self.confirm = Some(Confirm::CloseLogic(index));
                } else {
                    self.remove_logic_tab(index);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;

    /// LOGIC's documents come and go without touching BUILD's selection or
    /// history; BUILD and LOGIC each come back as they were left.
    #[test]
    fn logic_documents_leave_build_alone() {
        let mut editor = Editor::new("test".into(), PathBuf::new(), "scenes/a.ron".into());
        editor.selected = Some(3);
        editor.open_graph_tab("graphs/talk.ron".into(), GraphDoc::new(Graph::default()));
        assert_eq!(editor.workspace(), Workspace::Logic);
        assert_eq!(editor.scene_path, Path::new("scenes/a.ron"));
        assert_eq!(editor.selected, Some(3));
        editor.open_script_tab("scripts/a.rhai".into(), ScriptDoc::new(String::new()));
        assert_eq!(editor.logic_tabs.len(), 2);
        assert!(editor.script.is_some() && editor.graph.is_none());

        editor.leave_logic();
        assert_eq!(editor.workspace(), Workspace::Build);
        assert!(editor.graph.is_none() && editor.script.is_none());
        assert_eq!(editor.selected, Some(3));

        assert!(editor.resume_logic());
        assert!(editor.script.is_some(), "back to the last one");
        editor.switch_logic(0);
        assert!(editor.graph.is_some());
        editor.remove_logic_tab(0);
        assert!(editor.script.is_some());
        assert_eq!(editor.logic_tabs.len(), 1);
    }
}
