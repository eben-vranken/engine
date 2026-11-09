//! The project's own settings as one document: `game.ron` (name, start
//! scene, render scale, dialogue look) and `flags.ron` (flags and abilities).
//! Edited on the PROJECT page and in LOGIC's FLAGS list; every finished
//! change is an undo step, and nothing is written until it is saved
//! (Ctrl+S on PROJECT, SAVE ALL), so leaving it is asked about like any
//! unsaved document.

use crate::graph::flags::FlagDefs;
use crate::project::{GameSettings, Project};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct State {
    pub game: GameSettings,
    pub flags: FlagDefs,
}

pub(super) struct ProjectDoc {
    /// As being edited.
    pub now: State,
    /// As on disk.
    saved: State,
    /// Where the last undo step ends (edits since are still in progress).
    committed: State,
    undo: Vec<State>,
    redo: Vec<State>,
}

impl ProjectDoc {
    pub fn load(dir: &Path) -> Option<Self> {
        let game = Project::open(dir).ok()?.settings;
        let state = State {
            game,
            flags: FlagDefs::load(dir),
        };
        Some(Self {
            now: state.clone(),
            saved: state.clone(),
            committed: state,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.now != self.saved
    }

    /// How many settings differ from the files.
    pub fn changes(&self) -> usize {
        let (a, b) = (&self.now.game, &self.saved.game);
        [
            a.name != b.name,
            a.start_scene != b.start_scene,
            a.render_scale != b.render_scale,
            a.dialogue != b.dialogue,
            self.now.flags != self.saved.flags,
        ]
        .iter()
        .filter(|c| **c)
        .count()
    }

    /// Ends the change in progress (typing finished, a click done): it
    /// becomes one undo step.
    pub fn settle(&mut self) {
        if self.now != self.committed {
            self.undo
                .push(std::mem::replace(&mut self.committed, self.now.clone()));
            self.redo.clear();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.now != self.committed
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.settle();
        if let Some(before) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.now, before));
            self.committed = self.now.clone();
        }
    }

    pub fn redo(&mut self) {
        if let Some(after) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.now, after));
            self.committed = self.now.clone();
        }
    }

    /// Writes `game.ron` and `flags.ron`.
    pub fn save(&mut self, dir: &Path) -> Result<(), String> {
        self.settle();
        self.now.game.name = self.now.game.name.trim().to_owned();
        if self.now.game.name.is_empty() {
            return Err("THE GAME NEEDS A NAME".into());
        }
        self.now.game.save(dir).map_err(|e| e.to_string())?;
        self.now
            .flags
            .save(dir)
            .map_err(|e| format!("could not write flags.ron: {e}"))?;
        self.saved = self.now.clone();
        self.committed = self.now.clone();
        Ok(())
    }

    /// Back to the files (one undo step).
    pub fn revert(&mut self) {
        self.settle();
        self.now = self.saved.clone();
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::flags::{FlagDef, FlagValue};

    fn doc() -> ProjectDoc {
        let state = State {
            game: GameSettings::default(),
            flags: FlagDefs::default(),
        };
        ProjectDoc {
            now: state.clone(),
            saved: state.clone(),
            committed: state,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    #[test]
    fn finished_changes_undo_and_redo() {
        let mut doc = doc();
        doc.now.game.name = "a hall".into();
        doc.settle();
        doc.now.flags.flags.push(FlagDef {
            name: "lit".into(),
            value: FlagValue::Bool(false),
            script: Default::default(),
        });
        assert!(doc.is_dirty());
        assert_eq!(doc.changes(), 2);
        doc.undo();
        assert!(doc.now.flags.flags.is_empty());
        assert_eq!(doc.now.game.name, "a hall");
        doc.undo();
        assert!(!doc.is_dirty());
        doc.redo();
        assert_eq!(doc.now.game.name, "a hall");
    }
}
