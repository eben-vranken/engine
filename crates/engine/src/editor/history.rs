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
    /// Insert an object at `index` (new, duplicated).
    Add { index: usize, object: Box<Object> },
    /// Remove the object at `index`; `object` is kept to put it back.
    Remove { index: usize, object: Box<Object> },
}

/// What applying or reverting an edit did to the scene's object list.
/// Selection, highlight and draws all refer to objects by index, so they
/// have to follow insertions and removals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The object at this index changed; no indices moved.
    Modified(usize),
    /// An object was inserted here; later objects moved up by one.
    Added(usize),
    /// The object here was removed; later objects moved down by one.
    Removed(usize),
}

impl Edit {
    fn apply(&self, scene: &mut Scene) -> Change {
        match self {
            Edit::SetObject { index, after, .. } => {
                scene.objects[*index] = (**after).clone();
                Change::Modified(*index)
            }
            Edit::Add { index, object } => insert(scene, *index, object),
            Edit::Remove { index, .. } => remove(scene, *index),
        }
    }

    fn revert(&self, scene: &mut Scene) -> Change {
        match self {
            Edit::SetObject { index, before, .. } => {
                scene.objects[*index] = (**before).clone();
                Change::Modified(*index)
            }
            Edit::Add { index, .. } => remove(scene, *index),
            Edit::Remove { index, object } => insert(scene, *index, object),
        }
    }
}

fn insert(scene: &mut Scene, index: usize, object: &Object) -> Change {
    scene.objects.insert(index, object.clone());
    Change::Added(index)
}

fn remove(scene: &mut Scene, index: usize) -> Change {
    scene.objects.remove(index);
    Change::Removed(index)
}

/// Edits carry an id so "has the scene changed since it was saved?" can be
/// answered exactly, even after undo and redo.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<(u64, Edit)>,
    redo: Vec<(u64, Edit)>,
    next_id: u64,
}

impl History {
    /// Applies an edit to the scene and records it.
    pub fn push(&mut self, scene: &mut Scene, edit: Edit) -> Change {
        let change = edit.apply(scene);
        self.record(edit);
        change
    }

    /// Records an edit that has already been applied to the scene.
    pub fn record(&mut self, edit: Edit) {
        self.next_id += 1;
        self.undo.push((self.next_id, edit));
        self.redo.clear();
    }

    /// Identifies the current state: the id of the newest applied edit.
    /// Equal ids mean identical scenes.
    pub fn state(&self) -> Option<u64> {
        self.undo.last().map(|(id, _)| *id)
    }

    /// Reverts the last edit.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<Change> {
        let (id, edit) = self.undo.pop()?;
        let change = edit.revert(scene);
        self.redo.push((id, edit));
        Some(change)
    }

    /// Re-applies the last undone edit.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<Change> {
        let (id, edit) = self.redo.pop()?;
        let change = edit.apply(scene);
        self.undo.push((id, edit));
        Some(change)
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

        assert_eq!(history.undo(&mut scene), Some(Change::Modified(0)));
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
    fn state_tracks_saved_point_through_undo_and_redo() {
        let mut scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::ZERO)]);
        let mut history = History::default();
        moved(&mut scene, &mut history, 1.0);
        let saved = history.state();
        moved(&mut scene, &mut history, 2.0);
        assert_ne!(history.state(), saved);
        history.undo(&mut scene);
        assert_eq!(history.state(), saved);
        // Undo past the save and make a different edit: not the saved state.
        history.undo(&mut scene);
        moved(&mut scene, &mut history, 3.0);
        assert_ne!(history.state(), saved);
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

    #[test]
    fn add_and_remove_undo_back_to_the_same_list() {
        let shapes = (0..3)
            .map(|i| (Shape::Box { size: Vec3::ONE }, Vec3::X * i as f32))
            .collect();
        let mut scene = test_scene(shapes);
        let original = scene.clone();
        let mut history = History::default();

        let mut copy = scene.objects[0].clone();
        copy.name = "copy".into();
        let change = history.push(
            &mut scene,
            Edit::Add {
                index: 1,
                object: Box::new(copy),
            },
        );
        assert_eq!(change, Change::Added(1));
        assert_eq!(scene.objects[1].name, "copy");
        assert_eq!(scene.objects[2].name, "object 1");

        let removed = Box::new(scene.objects[3].clone());
        let change = history.push(
            &mut scene,
            Edit::Remove {
                index: 3,
                object: removed,
            },
        );
        assert_eq!(change, Change::Removed(3));
        assert_eq!(scene.objects.len(), 3);

        assert_eq!(history.undo(&mut scene), Some(Change::Added(3)));
        assert_eq!(history.undo(&mut scene), Some(Change::Removed(1)));
        assert_eq!(scene, original);
        assert_eq!(history.redo(&mut scene), Some(Change::Added(1)));
        assert_eq!(history.redo(&mut scene), Some(Change::Removed(3)));
        assert_eq!(scene.objects.len(), 3);
        assert_eq!(scene.objects[1].name, "copy");
    }
}
