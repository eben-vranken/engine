//! Undo/redo. Every change to the scene is an [`Edit`] that knows how to
//! apply and revert itself; the history keeps two stacks of them.

use crate::scene::{Object, Scene};

#[derive(Debug, Clone)]
pub enum Edit {
    /// Replace one object (any change to its fields or components).
    SetObject {
        index: usize,
        before: Box<Object>,
        after: Box<Object>,
    },
}

impl Edit {
    /// Applies the edit; returns the index of the object it changed.
    fn apply(&self, scene: &mut Scene) -> usize {
        match self {
            Edit::SetObject { index, after, .. } => {
                scene.objects[*index] = (**after).clone();
                *index
            }
        }
    }

    fn revert(&self, scene: &mut Scene) -> usize {
        match self {
            Edit::SetObject { index, before, .. } => {
                scene.objects[*index] = (**before).clone();
                *index
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    /// Records an edit that has already been applied to the scene.
    pub fn record(&mut self, edit: Edit) {
        self.undo.push(edit);
        self.redo.clear();
    }

    /// Reverts the last edit; returns the changed object's index.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<usize> {
        let edit = self.undo.pop()?;
        let index = edit.revert(scene);
        self.redo.push(edit);
        Some(index)
    }

    /// Re-applies the last undone edit; returns the changed object's index.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<usize> {
        let edit = self.redo.pop()?;
        let index = edit.apply(scene);
        self.undo.push(edit);
        Some(index)
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::test_scene;
    use glam::Vec3;

    fn moved(scene: &mut Scene, history: &mut History, x: f32) {
        let before = scene.objects[0].clone();
        scene.objects[0].transform.position.x = x;
        history.record(Edit::SetObject {
            index: 0,
            before: Box::new(before),
            after: Box::new(scene.objects[0].clone()),
        });
    }

    #[test]
    fn undo_and_redo_restore_exact_states() {
        let mut scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::ZERO)]);
        let mut history = History::default();
        moved(&mut scene, &mut history, 1.0);
        moved(&mut scene, &mut history, 2.0);

        assert_eq!(history.undo(&mut scene), Some(0));
        assert_eq!(scene.objects[0].transform.position.x, 1.0);
        history.undo(&mut scene);
        assert_eq!(scene.objects[0].transform.position.x, 0.0);
        assert_eq!(history.undo(&mut scene), None);

        history.redo(&mut scene);
        history.redo(&mut scene);
        assert_eq!(scene.objects[0].transform.position.x, 2.0);
        assert_eq!(history.redo(&mut scene), None);
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::ZERO)]);
        let mut history = History::default();
        moved(&mut scene, &mut history, 1.0);
        history.undo(&mut scene);
        moved(&mut scene, &mut history, 5.0);
        assert_eq!(history.redo_len(), 0);
        assert_eq!(history.undo_len(), 1);
    }
}
