//! Undo/redo. Every change to the scene (or the game's player settings) is
//! an [`Edit`] that knows how to apply and revert itself; the history keeps
//! two stacks of them.

use crate::player::settings::PlayerSettings;
use crate::scene::{Object, Scene};

#[derive(Debug, Clone)]
pub enum Edit {
    /// Replace one object (any change to its fields or components),
    /// found by its id wherever it is in the list by then.
    SetObject {
        id: u32,
        before: Box<Object>,
        after: Box<Object>,
    },
    /// Insert an object at `index` (new, duplicated); the position only
    /// keeps the list's order on redo.
    Add { index: usize, object: Box<Object> },
    /// Remove the object (found by its id); `index` is where it was, so
    /// undo puts it back in the same place.
    Remove { index: usize, object: Box<Object> },
    /// Replace the whole scene: zone edits (add, rename, delete), which can
    /// touch every object in the zone.
    SetScene {
        before: Box<Scene>,
        after: Box<Scene>,
    },
    /// Replace the player settings (the project's `player.ron`).
    SetPlayer {
        before: Box<PlayerSettings>,
        after: Box<PlayerSettings>,
    },
}

impl Edit {
    /// New objects get ids before the edit is recorded, so undo and redo
    /// bring back the same object (references to it keep working). A copy
    /// (duplicate, paste) arrives with its original's id and gets its own.
    fn give_ids(&mut self, scene: &Scene) {
        match self {
            Edit::Add { object, .. } => {
                if object.id == 0 || scene.objects.iter().any(|o| o.id == object.id) {
                    object.id = scene.next_id();
                }
            }
            Edit::SetScene { after, .. } => after.assign_ids(),
            Edit::SetObject { before, after, .. } => after.id = before.id,
            Edit::Remove { .. } | Edit::SetPlayer { .. } => {}
        }
    }
}

/// What applying or reverting an edit did to the scene's objects, by
/// object id. The editor's selection follows ids; what is drawn is looked
/// up by position when it is rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The object with this id changed; nothing moved in the list.
    Modified(u32),
    /// The object with this id was inserted; later objects moved up.
    Added(u32),
    /// The object with this id was removed; later objects moved down.
    Removed(u32),
    /// Anything may have changed, objects may have come or gone.
    All,
    /// Only zone or environment settings changed (fog, grade, sun, sky):
    /// no meshes need re-uploading.
    Settings,
    /// Only the player settings changed; nothing in the scene did.
    Player,
}

impl Edit {
    fn apply(&self, scene: &mut Scene, player: &mut PlayerSettings) -> Change {
        match self {
            Edit::SetObject { id, after, .. } => set(scene, *id, after),
            Edit::Add { index, object } => insert(scene, *index, object),
            Edit::Remove { index, object } => remove(scene, *index, object),
            Edit::SetScene { after, .. } => {
                *scene = (**after).clone();
                Change::All
            }
            Edit::SetPlayer { after, .. } => {
                *player = (**after).clone();
                Change::Player
            }
        }
    }

    fn revert(&self, scene: &mut Scene, player: &mut PlayerSettings) -> Change {
        match self {
            Edit::SetObject { id, before, .. } => set(scene, *id, before),
            Edit::Add { index, object } => remove(scene, *index, object),
            Edit::Remove { index, object } => insert(scene, *index, object),
            Edit::SetScene { before, .. } => {
                *scene = (**before).clone();
                Change::All
            }
            Edit::SetPlayer { before, .. } => {
                *player = (**before).clone();
                Change::Player
            }
        }
    }
}

fn set(scene: &mut Scene, id: u32, object: &Object) -> Change {
    match scene.object_mut(id) {
        Some(slot) => {
            *slot = object.clone();
            Change::Modified(id)
        }
        // Not there (it never should be): nothing to put it over.
        None => Change::All,
    }
}

fn insert(scene: &mut Scene, index: usize, object: &Object) -> Change {
    scene
        .objects
        .insert(index.min(scene.objects.len()), object.clone());
    Change::Added(object.id)
}

/// Removes `object` by its id; `index` only if it has none.
fn remove(scene: &mut Scene, index: usize, object: &Object) -> Change {
    let at = scene
        .index_of(object.id)
        .or_else(|| (index < scene.objects.len()).then_some(index));
    match at {
        Some(at) => {
            scene.objects.remove(at);
            Change::Removed(object.id)
        }
        None => Change::All,
    }
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
    pub fn push(&mut self, scene: &mut Scene, mut edit: Edit) -> Change {
        edit.give_ids(scene);
        // Scene edits never touch the player settings.
        let change = edit.apply(scene, &mut PlayerSettings::default());
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
    pub fn undo(&mut self, scene: &mut Scene, player: &mut PlayerSettings) -> Option<Change> {
        let (id, edit) = self.undo.pop()?;
        let change = edit.revert(scene, player);
        self.redo.push((id, edit));
        Some(change)
    }

    /// Re-applies the last undone edit.
    pub fn redo(&mut self, scene: &mut Scene, player: &mut PlayerSettings) -> Option<Change> {
        let (id, edit) = self.redo.pop()?;
        let change = edit.apply(scene, player);
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
            id: scene.objects[0].id,
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

        assert_eq!(
            history.undo(&mut scene, &mut PlayerSettings::default()),
            Some(Change::Modified(1))
        );
        assert_eq!(scene.objects[0].transform.position.x, 1.0);
        history.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene.objects[0].transform.position.x, 0.0);
        assert_eq!(
            history.undo(&mut scene, &mut PlayerSettings::default()),
            None
        );

        history.redo(&mut scene, &mut PlayerSettings::default());
        history.redo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene.objects[0].transform.position.x, 2.0);
        assert_eq!(
            history.redo(&mut scene, &mut PlayerSettings::default()),
            None
        );
    }

    #[test]
    fn state_tracks_saved_point_through_undo_and_redo() {
        let mut scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::ZERO)]);
        let mut history = History::default();
        moved(&mut scene, &mut history, 1.0);
        let saved = history.state();
        moved(&mut scene, &mut history, 2.0);
        assert_ne!(history.state(), saved);
        history.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(history.state(), saved);
        // Undo past the save and make a different edit: not the saved state.
        history.undo(&mut scene, &mut PlayerSettings::default());
        moved(&mut scene, &mut history, 3.0);
        assert_ne!(history.state(), saved);
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::ZERO)]);
        let mut history = History::default();
        moved(&mut scene, &mut history, 1.0);
        history.undo(&mut scene, &mut PlayerSettings::default());
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
        assert_eq!(change, Change::Added(4));
        assert_eq!(scene.objects[1].name, "copy");
        // The copy got an id of its own.
        assert_ne!(scene.objects[1].id, scene.objects[0].id);
        assert_eq!(scene.objects[2].name, "object 1");

        let removed = Box::new(scene.objects[3].clone());
        let change = history.push(
            &mut scene,
            Edit::Remove {
                index: 3,
                object: removed,
            },
        );
        assert_eq!(change, Change::Removed(3), "object 2 has id 3");
        assert_eq!(scene.objects.len(), 3);

        assert_eq!(
            history.undo(&mut scene, &mut PlayerSettings::default()),
            Some(Change::Added(3))
        );
        assert_eq!(
            history.undo(&mut scene, &mut PlayerSettings::default()),
            Some(Change::Removed(4))
        );
        assert_eq!(scene, original);
        assert_eq!(
            history.redo(&mut scene, &mut PlayerSettings::default()),
            Some(Change::Added(4))
        );
        assert_eq!(
            history.redo(&mut scene, &mut PlayerSettings::default()),
            Some(Change::Removed(3))
        );
        assert_eq!(scene.objects.len(), 3);
        assert_eq!(scene.objects[1].name, "copy");
    }

    #[test]
    fn player_edits_undo_and_redo() {
        let mut scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::ZERO)]);
        let mut player = PlayerSettings::default();
        let mut history = History::default();
        let before = player.clone();
        player.movement.walk_speed = 9.0;
        history.record(Edit::SetPlayer {
            before: Box::new(before),
            after: Box::new(player.clone()),
        });
        assert_eq!(history.undo(&mut scene, &mut player), Some(Change::Player));
        assert_eq!(player, PlayerSettings::default());
        history.redo(&mut scene, &mut player);
        assert_eq!(player.movement.walk_speed, 9.0);
    }
}
