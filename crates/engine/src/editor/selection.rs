//! Selecting several things at once (shift-click). The *primary* pick is
//! `Editor::selected` / `selected_part`, the one the inspector shows; the
//! rest are in `Editor::also`. An edit to the primary is copied to the
//! others of the same kind (objects to objects, model parts to parts),
//! field by field, as one undo step.

use super::history::Change;
use super::{Editor, EditorOutput, inspector, objects::unique_name};
use crate::scene::{Component, Model, ModelPart, Object, Scene};
use std::mem::discriminant;

/// One selected thing: an object, or one part of its model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub object: usize,
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

    /// The selected objects (an object with selected parts counts), each
    /// once, the primary's first.
    pub fn selected_objects(&self) -> Vec<usize> {
        let mut objects = Vec::new();
        for pick in self.picks() {
            if !objects.contains(&pick.object) {
                objects.push(pick.object);
            }
        }
        objects
    }

    /// Whether `object` (or its `part`) is selected.
    pub(super) fn is_picked(&self, object: usize, part: Option<&str>) -> bool {
        self.picks()
            .iter()
            .any(|p| p.object == object && p.part.as_deref() == part)
    }

    /// Shift-click: adds `pick` to the selection as the new primary, or
    /// takes it out if it was in. Returns true if an edit was committed.
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
            if let Some(target) = scene.objects.get_mut(pick.object) {
                copy_object_changes(before, &after, target);
                output.changed.push(Change::Modified(pick.object));
            }
        }
        output.changed.push(Change::Modified(index));
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
            let (Some(name), Some(target)) = (pick.part, scene.objects.get_mut(pick.object)) else {
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
        output.changed.push(Change::Modified(index));
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
        let mut doomed = self.selected_objects();
        doomed.retain(|&i| i < scene.objects.len());
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
        let originals: Vec<usize> = self
            .selected_objects()
            .into_iter()
            .filter(|&i| i < scene.objects.len())
            .collect();
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
        let change = self.push_scene(scene, after);
        self.selected = Some(first);
        self.selected_part = None;
        self.also = (first + 1..first + originals.len())
            .map(|object| Pick { object, part: None })
            .collect();
        self.reveal_selection = true;
        Some(change)
    }

    pub(super) fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_part = None;
        self.also.clear();
    }

    /// Keeps the other picks on the same objects when one is removed.
    pub(super) fn follow_removed(&mut self, index: usize) {
        self.also.retain(|p| p.object != index);
        for pick in &mut self.also {
            if pick.object > index {
                pick.object -= 1;
            }
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
        editor.select(&scene, Some(0));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        assert_eq!(editor.selected, Some(2), "the newest is the primary");
        assert_eq!(editor.selected_objects(), [2, 0]);
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        assert_eq!(editor.selected_objects(), [0]);
        editor.toggle_pick(
            &scene,
            Pick {
                object: 0,
                part: None,
            },
        );
        assert_eq!(editor.selected, None);
        // A plain click starts over.
        editor.toggle_pick(
            &scene,
            Pick {
                object: 1,
                part: None,
            },
        );
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        editor.select(&scene, Some(0));
        assert_eq!(editor.selected_objects(), [0]);
    }

    #[test]
    fn an_object_edit_reaches_every_selected_object_as_one_step() {
        let mut scene = boxes(3);
        let original = scene.clone();
        let mut editor = editor();
        editor.select(&scene, Some(0));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 1,
                part: None,
            },
        );
        assert_eq!(editor.selected, Some(1));
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
        let mut editor = editor();
        let flame = |object| Pick {
            object,
            part: Some("flame".into()),
        };
        editor.toggle_pick(&scene, flame(0));
        editor.toggle_pick(&scene, flame(1));
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
        editor.select(&scene, Some(1));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 3,
                part: None,
            },
        );
        editor.duplicate_all_selected(&mut scene);
        assert_eq!(scene.objects.len(), 6);
        assert_eq!(editor.selected_objects(), [4, 5]);
        assert_eq!(scene.objects[4].transform, original.objects[3].transform);
        editor.delete_all_selected(&mut scene);
        assert_eq!(scene, original);
        editor.select(&scene, Some(0));
        editor.toggle_pick(
            &scene,
            Pick {
                object: 2,
                part: None,
            },
        );
        editor.delete_all_selected(&mut scene);
        assert_eq!(scene.objects.len(), 2);
        assert_eq!(editor.picks(), []);
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);
    }
}
