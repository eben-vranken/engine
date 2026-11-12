//! Selecting several things at once (shift-click). The *primary* pick is
//! `Editor::selected` / `selected_part`, the one the inspector shows; the
//! rest are in `Editor::also`. An edit to the primary is copied to the
//! others of the same kind (objects to objects, model parts to parts),
//! field by field, as one undo step.

use super::history::Change;
use super::{Editor, EditorOutput, inspector, objects::unique_name};
use crate::scene::{Component, Model, ModelPart, Object, Scene};
use std::mem::discriminant;

/// One selected thing: an object (by id), or one part of its model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub object: u32,
    pub part: Option<String>,
}

impl Editor {
    /// Everything selected, the primary first.
    pub fn picks(&self) -> Vec<Pick> {
        let primary = self.selected.map(|object| Pick {
            object,
            part: self.selected_part.clone(),
        });
        primary
            .into_iter()
            .chain(self.also.iter().cloned())
            .collect()
    }

    /// The selected objects' ids (an object with selected parts counts),
    /// each once, the primary's first.
    pub fn selected_objects(&self) -> Vec<u32> {
        let mut objects = Vec::new();
        for pick in self.picks() {
            if !objects.contains(&pick.object) {
                objects.push(pick.object);
            }
        }
        objects
    }

    /// Where the selected objects are in the list now, the primary's first;
    /// ids no object has (any more) are left out.
    pub fn selected_indices(&self, scene: &Scene) -> Vec<usize> {
        self.selected_objects()
            .into_iter()
            .filter_map(|id| scene.index_of(id))
            .collect()
    }

    /// Whether object `object` (an id), or its `part`, is selected.
    pub(super) fn is_picked(&self, object: u32, part: Option<&str>) -> bool {
        self.picks()
            .iter()
            .any(|p| p.object == object && p.part.as_deref() == part)
    }

    /// Shift-click: adds `pick` to the selection as the new primary, or
    /// takes it out if it was in. Returns true if an edit was committed.
    /// Box select: these objects (ids) become the selection (`add`: added
    /// to it). Returns whether an edit was settled.
    pub fn select_objects(&mut self, scene: &Scene, objects: &[u32], add: bool) -> bool {
        let mut picks: Vec<Pick> = if add { self.picks() } else { Vec::new() };
        for &object in objects {
            let pick = Pick { object, part: None };
            if !picks.contains(&pick) {
                picks.push(pick);
            }
        }
        let settled = self.select(scene, None);
        let mut picks = picks.into_iter();
        if let Some(first) = picks.next() {
            self.selected = Some(first.object);
            self.selected_part = first.part;
            self.also = picks.collect();
            self.settings = None;
        }
        self.reveal_selection = true;
        settled
    }

    pub fn toggle_pick(&mut self, scene: &Scene, pick: Pick) -> bool {
        let settled = self.commit_pending(scene);
        self.settings = None;
        let mut picks = self.picks();
        if let Some(i) = picks.iter().position(|p| *p == pick) {
            picks.remove(i);
            // The most recently added one becomes the primary.
            if let Some(last) = picks.pop() {
                picks.insert(0, last);
            }
        } else {
            picks.insert(0, pick);
        }
        let mut picks = picks.into_iter();
        match picks.next() {
            Some(first) => {
                self.selected = Some(first.object);
                self.selected_part = first.part;
            }
            None => {
                self.selected = None;
                self.selected_part = None;
            }
        }
        self.also = picks.collect();
        settled
    }

    /// Picks other than the primary, keeping those of the primary's kind:
    /// whole objects, or model parts.
    fn others(&self, parts: bool) -> Vec<Pick> {
        let primary = self.selected;
        self.also
            .iter()
            .filter(|p| p.part.is_some() == parts)
            .filter(|p| parts || Some(p.object) != primary)
            .cloned()
            .collect()
    }

    /// Whether an edit to the primary is copied to other selected things.
    pub(super) fn edits_several(&self) -> bool {
        !self.others(self.selected_part.is_some()).is_empty()
    }

    /// The primary object (index `index`) was edited from `before`: copies
    /// the change to the other selected objects, as one undo step.
    pub(super) fn spread_object_edit(
        &mut self,
        scene: &mut Scene,
        index: usize,
        before: &Object,
        output: &mut EditorOutput,
    ) {
        self.begin_scene_edit(scene, index, before);
        let after = scene.objects[index].clone();
        for pick in self.others(false) {
            if let Some(target) = scene.object_mut(pick.object) {
                copy_object_changes(before, &after, target);
                output.changed.push(Change::Modified(pick.object));
            }
        }
        output.changed.push(Change::Modified(after.id));
    }

    /// The primary part (of object `index`) was edited: copies the change
    /// to the other selected parts, as one undo step.
    pub(super) fn spread_part_edit(
        &mut self,
        scene: &mut Scene,
        index: usize,
        before: &Object,
        output: &mut EditorOutput,
    ) {
        let Some(part) = self.selected_part.clone() else {
            return;
        };
        self.begin_scene_edit(scene, index, before);
        // A part without settings of its own counts as the model's (what
        // GIVE IT ITS OWN starts from), so changes made at once still copy.
        let own_before = model_of(before).map(|m| {
            m.part(&part)
                .cloned()
                .unwrap_or_else(|| own_settings(&mut m.clone(), &part).clone())
        });
        let had_own = model_of(before).is_some_and(|m| m.part(&part).is_some());
        let own_after = model_of(&scene.objects[index])
            .and_then(|m| m.part(&part))
            .cloned();
        for pick in self.others(true) {
            let (Some(name), Some(target)) = (pick.part, scene.object_mut(pick.object)) else {
                continue;
            };
            let Some(model) = model_of_mut(target) else {
                continue;
            };
            match (&own_before, &own_after) {
                (Some(b), Some(a)) => {
                    let own = own_settings(model, &name);
                    let keep = own.name.clone();
                    inspector::copy_changes(b, a, own);
                    own.name = keep;
                }
                // USE THE MODEL'S SETTINGS
                (_, None) if had_own => model.parts.retain(|p| p.name != name),
                _ => {}
            }
            output.changed.push(Change::Modified(pick.object));
        }
        output
            .changed
            .push(Change::Modified(scene.objects[index].id));
    }

    /// Starts (or continues) an edit of several objects, held as one undo
    /// step; `before` is object `index` as it was before this frame.
    fn begin_scene_edit(&mut self, scene: &Scene, index: usize, before: &Object) {
        if self.zone_pending.is_none() {
            let mut was = scene.clone();
            was.objects[index] = before.clone();
            self.commit_pending(&was);
            self.zone_pending = Some(Box::new(was));
        }
    }

    /// A gizmo drag of several objects: held as one undo step.
    pub(super) fn begin_group_move(&mut self, scene: &Scene) {
        if self.zone_pending.is_none() {
            self.commit_pending(scene);
            self.zone_pending = Some(Box::new(scene.clone()));
        }
    }

    /// Removes every selected object, as one undo step.
    pub(super) fn delete_all_selected(&mut self, scene: &mut Scene) -> Option<Change> {
        let mut doomed = self.selected_indices(scene);
        if doomed.is_empty() {
            return None;
        }
        let mut after = scene.clone();
        doomed.sort_unstable_by(|a, b| b.cmp(a));
        for i in doomed {
            after.objects.remove(i);
        }
        self.clear_selection();
        Some(self.push_scene(scene, after))
    }

    /// Copies every selected object (to the end of the list) and selects
    /// the copies, as one undo step.
    pub(super) fn duplicate_all_selected(&mut self, scene: &mut Scene) -> Option<Change> {
        let originals = self.selected_indices(scene);
        if originals.is_empty() {
            return None;
        }
        let mut after = scene.clone();
        let first = after.objects.len();
        for &i in &originals {
            let mut copy = after.objects[i].clone();
            copy.name = unique_name(&after, &copy.name);
            after.objects.push(copy);
        }
        // The copies get ids of their own when the edit is recorded.
        let change = self.push_scene(scene, after);
        let mut copies = scene.objects[first..].iter().map(|o| o.id);
        self.selected = copies.next();
        self.selected_part = None;
        self.also = copies.map(|object| Pick { object, part: None }).collect();
        self.reveal_selection = true;
        Some(change)
    }

    pub(super) fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_part = None;
        self.also.clear();
    }

    /// Drops picks of objects the scene no longer has (deleted, or undone
    /// away); the primary goes too, and the inspector with it.
    pub(super) fn drop_missing(&mut self, scene: &Scene) {
        self.also.retain(|p| scene.index_of(p.object).is_some());
        if self.selected.is_some_and(|id| scene.index_of(id).is_none()) {
            self.selected = None;
            self.selected_part = None;
        }
        if let Some((id, _)) = &self.pending
            && scene.index_of(*id).is_none()
        {
            self.pending = None;
        }
    }
}

/// Copies what changed on one object to another: its zone, transform and
/// components' fields; components added or removed are added to or
/// removed from it too. The name stays its own.
fn copy_object_changes(before: &Object, after: &Object, target: &mut Object) {
    if before.zone != after.zone {
        target.zone = after.zone.clone();
    }
    inspector::copy_changes(&before.transform, &after.transform, &mut target.transform);
    for component in &after.components {
        let kind = discriminant(component);
        let was = before.components.iter().find(|c| discriminant(*c) == kind);
        let theirs = target
            .components
            .iter_mut()
            .find(|c| discriminant(&**c) == kind);
        match (was, theirs) {
            // Only changed fields are copied, so a model keeps its own file
            // (and its parts' settings, which are edited per part).
            (Some(was), Some(theirs)) => inspector::copy_changes(was, component, theirs),
            (None, None) => target.components.push(component.clone()),
            _ => {}
        }
    }
    for component in &before.components {
        let kind = discriminant(component);
        if !after.components.iter().any(|c| discriminant(c) == kind) {
            target.components.retain(|c| discriminant(c) != kind);
        }
    }
}

fn model_of(object: &Object) -> Option<&Model> {
    object.components.iter().find_map(|c| match c {
        Component::Model(m) => Some(m),
        _ => None,
    })
}

fn model_of_mut(object: &mut Object) -> Option<&mut Model> {
    object.components.iter_mut().find_map(|c| match c {
        Component::Model(m) => Some(m),
        _ => None,
    })
}

/// The part's own settings, made from the model's if it has none yet.
pub(super) fn own_settings<'a>(model: &'a mut Model, part: &str) -> &'a mut ModelPart {
    let index = match model.parts.iter().position(|p| p.name == part) {
        Some(index) => index,
        None => {
            model.parts.push(ModelPart {
                name: part.to_owned(),
                color: model.color,
                emissive: model.emissive,
                collision: model.collision,
                light: None,
            });
            model.parts.len() - 1
        }
    };
    &mut model.parts[index]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::testing::*;
    use crate::player::settings::PlayerSettings;
    use crate::scene::{Floating, LightSource, Transform};
    use glam::Vec3;

    fn candle(name: &str, x: f32) -> Object {
        Object {
            id: 0,
            hidden: false,
            play: Default::default(),
            name: name.into(),
            zone: "test".into(),
            transform: Transform::at(Vec3::X * x),
            components: vec![Component::Model(Model {
                path: "models/candle.glb".into(),
                color: Vec3::ONE,
                emissive: Vec3::ZERO,
                collision: true,
                parts: Vec::new(),
            })],
        }
    }

    #[test]
    fn shift_clicks_add_and_remove_picks() {
        let scene = boxes(3);
        let mut editor = editor();
        editor.select(&scene, Some(1));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 3,
                part: None,
            },
        );
        assert_eq!(editor.selected, Some(3), "the newest is the primary");
        assert_eq!(editor.selected_objects(), [3, 1]);
        editor.toggle_pick(
            &scene,
            Pick {
                object: 3,
                part: None,
            },
        );
        assert_eq!(editor.selected_objects(), [1]);
        editor.toggle_pick(
            &scene,
            Pick {
                object: 1,
                part: None,
            },
        );
        assert_eq!(editor.selected, None);
        // A plain click starts over.
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        editor.toggle_pick(
            &scene,
            Pick {
                object: 3,
                part: None,
            },
        );
        editor.select(&scene, Some(1));
        assert_eq!(editor.selected_objects(), [1]);
    }

    #[test]
    fn an_object_edit_reaches_every_selected_object_as_one_step() {
        let mut scene = boxes(3);
        let original = scene.clone();
        let mut editor = editor();
        editor.select(&scene, Some(1));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        assert_eq!(editor.selected, Some(2));
        assert!(editor.edits_several());

        // As the inspector does: the primary changes, then it's spread.
        let before = scene.objects[1].clone();
        scene.objects[1].transform.scale.y = 3.0;
        scene.objects[1]
            .components
            .push(Component::Floating(Floating::default()));
        let mut output = crate::editor::testing::output();
        editor.spread_object_edit(&mut scene, 1, &before, &mut output);
        let other = &scene.objects[0];
        assert_eq!(other.transform.scale, Vec3::new(1.0, 3.0, 1.0));
        assert_eq!(
            other.transform.position,
            original.objects[0].transform.position
        );
        assert!(matches!(
            other.components.last(),
            Some(Component::Floating(_))
        ));
        assert_eq!(scene.objects[2], original.objects[2], "not selected");
        assert_eq!(other.name, original.objects[0].name);

        assert!(editor.commit_pending(&scene));
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);
    }

    #[test]
    fn a_part_edit_reaches_the_same_part_of_other_candles() {
        let mut scene = boxes(0);
        scene.objects = vec![
            candle("candle", 0.0),
            candle("candle 2", 1.0),
            candle("candle 3", 2.0),
        ];
        scene.assign_ids();
        let mut editor = editor();
        let flame = |object| Pick {
            object,
            part: Some("flame".into()),
        };
        editor.toggle_pick(&scene, flame(1));
        editor.toggle_pick(&scene, flame(2));
        assert_eq!(editor.selected_part.as_deref(), Some("flame"));

        // GIVE IT ITS OWN, then a glow and a light: every selected flame.
        let before = scene.objects[1].clone();
        let Component::Model(m) = &mut scene.objects[1].components[0] else {
            unreachable!()
        };
        let own = own_settings(m, "flame");
        own.emissive = Vec3::new(4.0, 2.0, 0.5);
        own.light = Some(LightSource::default());
        let mut output = crate::editor::testing::output();
        editor.spread_part_edit(&mut scene, 1, &before, &mut output);
        let Component::Model(other) = &scene.objects[0].components[0] else {
            unreachable!()
        };
        let part = other.part("flame").expect("given its own");
        assert_eq!(part.emissive, Vec3::new(4.0, 2.0, 0.5));
        assert!(part.light.is_some());
        let Component::Model(third) = &scene.objects[2].components[0] else {
            unreachable!()
        };
        assert!(third.parts.is_empty(), "not selected");
    }

    #[test]
    fn several_objects_are_deleted_or_duplicated_as_one_step() {
        let mut scene = boxes(4);
        let original = scene.clone();
        let mut editor = editor();
        editor.select(&scene, Some(2));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 4,
                part: None,
            },
        );
        editor.duplicate_all_selected(&mut scene);
        assert_eq!(scene.objects.len(), 6);
        // The copies have new ids (5, 6), selected.
        assert_eq!(editor.selected_objects(), [5, 6]);
        assert_eq!(scene.objects[4].transform, original.objects[3].transform);
        editor.delete_all_selected(&mut scene);
        assert_eq!(scene, original);
        editor.select(&scene, Some(1));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 3,
                part: None,
            },
        );
        editor.delete_all_selected(&mut scene);
        assert_eq!(scene.objects.len(), 2);
        assert_eq!(editor.picks(), []);
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);
    }

    fn ids(scene: &Scene) -> Vec<u32> {
        scene.objects.iter().map(|o| o.id).collect()
    }

    /// Undoing a delete of several objects (one whole-scene step) puts
    /// objects back in front of the selected one: it stays selected, not
    /// whatever now sits at its old position.
    #[test]
    fn the_selection_stays_on_its_object_when_undo_shifts_the_list() {
        let mut scene = boxes(3);
        let mut editor = editor();
        editor.select(&scene, Some(1));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        editor.delete_all_selected(&mut scene);
        assert_eq!(ids(&scene), [3]);
        editor.select(&scene, Some(3));
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(ids(&scene), [1, 2, 3]);
        assert_eq!(editor.selected, Some(3));
        assert_eq!(editor.selected_indices(&scene), [2]);
    }

    /// Undoing a duplicate of several objects takes the copies away; the
    /// picks of the copies go with them instead of pointing past the end
    /// (or at other objects).
    #[test]
    fn picks_of_objects_undone_away_are_dropped() {
        let mut scene = boxes(2);
        let mut editor = editor();
        editor.select(&scene, Some(1));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        editor.duplicate_all_selected(&mut scene);
        assert_eq!(editor.selected_objects(), [3, 4]);
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(editor.picks(), []);
        assert_eq!(editor.selected_index(&scene), None);
    }

    /// An edit in progress is recorded on the object it started on, even
    /// if the list moved under it, and undo reverts that object only.
    #[test]
    fn a_pending_edit_lands_on_its_object_after_a_reorder() {
        let mut scene = boxes(3);
        let original = scene.clone();
        let mut editor = editor();
        editor.select(&scene, Some(3));
        let before = scene.objects[2].clone();
        scene.objects[2].transform.position.y = 5.0;
        editor.begin_edit(&before);
        // Something else reorders the list meanwhile.
        scene.objects.reverse();
        assert!(editor.commit_pending(&scene));
        assert_eq!(editor.selected_indices(&scene), [0]);
        editor.undo(&mut scene, &mut PlayerSettings::default());
        scene.objects.reverse();
        assert_eq!(scene, original);
    }

    /// Deleting an earlier object (and undoing it) moves the others in
    /// the list; picks and the primary follow their objects.
    #[test]
    fn picks_follow_their_objects_through_delete_and_undo() {
        let mut scene = boxes(4);
        let mut editor = editor();
        editor.select(&scene, Some(1));
        editor.delete_selected(&mut scene);
        editor.select(&scene, Some(4));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        editor.toggle_pick(
            &scene,
            Pick {
                object: 3,
                part: Some("lid".into()),
            },
        );
        assert_eq!(editor.selected_indices(&scene), [1, 0, 2]);
        // Undo brings object 1 back first in the list (and selects it);
        // redo deletes it again: nothing else is lost along the way.
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(editor.selected, Some(1));
        editor.select(&scene, Some(4));
        assert_eq!(editor.selected_indices(&scene), [3]);
        editor.redo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(editor.selected, Some(4));
        assert_eq!(editor.selected_indices(&scene), [2]);
    }
}
