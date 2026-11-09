//! Adding, duplicating and deleting objects, and where new ones go.

use super::gizmo::View;
use super::history::{Change, Edit};
use super::{Editor, markers};
use crate::render::mesh::Shape;
use crate::scene::{Component, Model, Object, Portal, Primitive, Scene, Transform, ZoneVolume};
use glam::{Vec2, Vec3};

impl Editor {
    /// Adds a primitive in front of the camera, in the selection's zone (or
    /// the first zone). `None` if the scene has no zones.
    pub(super) fn add_primitive(
        &mut self,
        scene: &mut Scene,
        kind: &str,
        shape: Shape,
        view: &View,
    ) -> Option<Change> {
        let half = shape.half_extents();
        let ahead = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        let object = Object {
            id: 0,
            hidden: false,
            name: unique_name(scene, kind),
            zone: self.new_object_zone(scene)?,
            transform: Transform::at(placement((-half, half), view.aim, ahead)),
            components: vec![Component::Primitive(Primitive {
                shape,
                color: Vec3::splat(0.8),
                emissive: Vec3::ZERO,
                collision: true,
            })],
        };
        Some(self.append(scene, object))
    }

    /// Adds the PlayerStart where the camera looks, facing the way the
    /// camera faces (snapped to 15°).
    pub(super) fn add_player_start(&mut self, scene: &mut Scene, view: &View) -> Option<Change> {
        let bounds = (
            Vec3::new(-0.3, 0.0, -0.3),
            Vec3::new(0.3, markers::POST_HEIGHT, 0.3),
        );
        let ahead = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        // A camera yaw turns right, an object's Y rotation turns left.
        let yaw = (-view.camera.yaw.to_degrees() / 15.0).round() * 15.0 + 0.0;
        let object = Object {
            id: 0,
            hidden: false,
            name: unique_name(scene, "player start"),
            zone: self.new_object_zone(scene)?,
            transform: Transform {
                position: placement(bounds, view.aim, ahead),
                rotation: Vec3::new(0.0, yaw, 0.0),
                ..Default::default()
            },
            components: vec![Component::PlayerStart],
        };
        Some(self.append(scene, object))
    }

    /// Adds a zone volume (in the selection's zone) standing on the surface
    /// the camera looks at.
    pub(super) fn add_zone_volume(&mut self, scene: &mut Scene, view: &View) -> Option<Change> {
        let volume = ZoneVolume::default();
        let half = volume.size / 2.0;
        let ahead = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        let zone = self.new_object_zone(scene)?;
        let object = Object {
            id: 0,
            hidden: false,
            name: unique_name(scene, &format!("{zone} volume")),
            zone,
            transform: Transform::at(placement((-half, half), view.aim, ahead)),
            components: vec![Component::ZoneVolume(volume)],
        };
        Some(self.append(scene, object))
    }

    /// Adds a portal (from the selection's zone, to be pointed at another)
    /// on the surface the camera looks at, facing the camera.
    pub(super) fn add_portal(&mut self, scene: &mut Scene, view: &View) -> Option<Change> {
        let half = markers::PORTAL_SIZE.x / 2.0;
        let bounds = (
            Vec3::new(-half, 0.0, -0.05),
            Vec3::new(half, markers::PORTAL_SIZE.y, 0.05),
        );
        let ahead = view.camera.position + view.camera.forward() * NEW_OBJECT_DISTANCE;
        let yaw = (-view.camera.yaw.to_degrees() / 15.0).round() * 15.0 + 0.0;
        let zone = self.new_object_zone(scene)?;
        let object = Object {
            id: 0,
            hidden: false,
            name: unique_name(scene, &format!("{zone} portal")),
            zone,
            transform: Transform {
                position: placement(bounds, view.aim, ahead),
                rotation: Vec3::new(0.0, yaw, 0.0),
                ..Default::default()
            },
            components: vec![Component::Portal(Portal::default())],
        };
        Some(self.append(scene, object))
    }
    /// Adds a model (path relative to the project) at `position`, in the
    /// selection's zone. `None` if the scene has no zones.
    pub fn add_model(&mut self, scene: &mut Scene, path: String, position: Vec3) -> Option<Change> {
        let stem = path.rsplit('/').next().unwrap_or(&path);
        let stem = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem);
        let object = Object {
            id: 0,
            hidden: false,
            name: unique_name(scene, stem),
            zone: self.new_object_zone(scene)?,
            transform: Transform::at(position),
            components: vec![Component::Model(Model {
                path: path.clone(),
                color: Vec3::ONE,
                emissive: Vec3::ZERO,
                collision: true,
                parts: Vec::new(),
            })],
        };
        self.message = Some((format!("ADDED {path}"), false));
        Some(self.append(scene, object))
    }

    /// New objects go into the selection's zone, or the first zone.
    pub(super) fn new_object_zone(&self, scene: &Scene) -> Option<String> {
        self.selected
            .and_then(|i| scene.objects.get(i))
            .map(|o| o.zone.clone())
            .or_else(|| scene.zones.first().map(|z| z.name.clone()))
    }

    pub(super) fn append(&mut self, scene: &mut Scene, object: Object) -> Change {
        let index = scene.objects.len();
        self.push(
            scene,
            Edit::Add {
                index,
                object: Box::new(object),
            },
        )
    }

    /// Removes the selected object.
    pub fn delete_selected(&mut self, scene: &mut Scene) -> Option<Change> {
        if let Some(doc) = &mut self.graph {
            doc.delete_selected();
            return None;
        }
        if self.selected_objects().len() > 1 {
            return self.delete_all_selected(scene);
        }
        let index = self.selected.filter(|&i| i < scene.objects.len())?;
        // Commit first so undo restores the object with its latest edit.
        self.commit_pending(scene);
        let object = Box::new(scene.objects[index].clone());
        Some(self.push(scene, Edit::Remove { index, object }))
    }

    /// Copies the selected object in place, right after it in the list, and
    /// selects the copy (move it with the gizmo).
    pub fn duplicate_selected(&mut self, scene: &mut Scene) -> Option<Change> {
        if let Some(doc) = &mut self.graph {
            if let Some(id) = doc.selected {
                doc.duplicate(id);
            }
            return None;
        }
        if self.selected_objects().len() > 1 {
            return self.duplicate_all_selected(scene);
        }
        let index = self.selected.filter(|&i| i < scene.objects.len())?;
        let mut object = scene.objects[index].clone();
        object.name = unique_name(scene, &object.name);
        Some(self.push(
            scene,
            Edit::Add {
                index: index + 1,
                object: Box::new(object),
            },
        ))
    }
}

/// How far in front of the camera new objects float when the camera isn't
/// looking at a surface, in meters.
pub const NEW_OBJECT_DISTANCE: f32 = 4.0;
/// The fine grid (same as Ctrl-snapping), in meters.
pub(super) const SNAP: f32 = 0.25;

/// Where a new object goes: resting on `surface` (point, normal) if there
/// is one, otherwise at `fallback`. `bounds` is the object's bounding box
/// (min, max corner) around its origin. Snapped to the fine grid along the
/// surface (so snapped moves stay aligned) but not into it.
pub fn placement(bounds: (Vec3, Vec3), surface: Option<(Vec3, Vec3)>, fallback: Vec3) -> Vec3 {
    let Some((point, normal)) = surface else {
        return (fallback / SNAP).round() * SNAP;
    };
    // The box's corner furthest against the normal touches the surface.
    let (min, max) = bounds;
    let corner = Vec3::select(normal.cmpgt(Vec3::ZERO), min, max);
    let position = point - normal * normal.dot(corner);
    let snapped = (position / SNAP).round() * SNAP;
    Vec3::select(normal.abs().cmpgt(Vec3::splat(0.5)), position, snapped)
}

/// The Add menu: each primitive with a starting size of about a meter.
pub(super) fn primitives() -> [(&'static str, Shape); 5] {
    [
        ("box", Shape::Box { size: Vec3::ONE }),
        (
            "plane",
            Shape::Plane {
                size: Vec2::splat(4.0),
            },
        ),
        (
            "wedge",
            Shape::Wedge {
                size: Vec3::new(1.0, 1.0, 2.0),
            },
        ),
        (
            "cylinder",
            Shape::Cylinder {
                radius: 0.5,
                height: 1.0,
                segments: 12,
            },
        ),
        (
            "sphere",
            Shape::Sphere {
                radius: 0.5,
                segments: 12,
                rings: 8,
            },
        ),
    ]
}

/// `base`, or `base 2`, `base 3`... if that name is taken. A trailing number
/// on `base` is replaced, so copies of "box 2" are "box 3", not "box 2 2".
pub(super) fn unique_name(scene: &Scene, base: &str) -> String {
    unique_among(scene.objects.iter().map(|o| o.name.as_str()), base)
}

/// `base`, or `base 2`, `base 3`… whichever isn't in `names`.
pub(super) fn unique_among<'a>(names: impl Iterator<Item = &'a str> + Clone, base: &str) -> String {
    let taken = |name: &str| names.clone().any(|n| n == name);
    if !taken(base) {
        return base.to_owned();
    }
    let stem = match base.rsplit_once(' ') {
        Some((stem, n)) if n.parse::<u32>().is_ok() => stem,
        _ => base,
    };
    (2..)
        .map(|n| format!("{stem} {n}"))
        .find(|name| !taken(name))
        .expect("some number is free")
}

impl Editor {
    /// PASTE: the copied objects, again (new ids, unique names), selected.
    pub(super) fn paste(&mut self, scene: &mut Scene) -> Option<Change> {
        if self.clipboard.is_empty() {
            return None;
        }
        let mut after = scene.clone();
        let first = after.objects.len();
        let zone = self.new_object_zone(scene);
        for object in &self.clipboard {
            let mut copy = object.clone();
            copy.name = unique_name(&after, &copy.name);
            copy.id = 0;
            // Into an existing zone (the copy may come from another scene).
            if !after.zones.iter().any(|z| z.name == copy.zone)
                && let Some(zone) = &zone
            {
                copy.zone = zone.clone();
            }
            after.objects.push(copy);
        }
        let count = self.clipboard.len();
        let change = self.push_scene(scene, after);
        self.selected = Some(first);
        self.selected_part = None;
        self.also = (first + 1..first + count)
            .map(|object| super::Pick { object, part: None })
            .collect();
        self.reveal_selection = true;
        Some(change)
    }

    /// MOVE TO ZONE: the selected objects into `zone` (one undo step).
    pub(super) fn move_to_zone(&mut self, scene: &mut Scene, zone: &str) -> Option<Change> {
        let picked = self.selected_objects();
        if picked.is_empty() || !scene.zones.iter().any(|z| z.name == zone) {
            return None;
        }
        let mut after = scene.clone();
        for i in picked {
            if let Some(object) = after.objects.get_mut(i) {
                object.zone = zone.to_owned();
            }
        }
        Some(self.push_scene(scene, after))
    }

    /// The selected objects' center and a radius around them, for FRAME.
    pub(super) fn selection_bounds(&self, scene: &Scene) -> Option<(glam::Vec3, f32)> {
        let objects: Vec<&crate::scene::Object> = self
            .selected_objects()
            .into_iter()
            .filter_map(|i| scene.objects.get(i))
            .collect();
        if objects.is_empty() {
            return None;
        }
        let center = objects
            .iter()
            .map(|o| o.transform.position)
            .sum::<glam::Vec3>()
            / objects.len() as f32;
        let radius = objects
            .iter()
            .map(|o| o.transform.position.distance(center) + o.transform.scale.max_element())
            .fold(1.0, f32::max);
        Some((center, radius))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::editor::testing::*;
    use crate::player::settings::PlayerSettings;
    use egui::Rect;

    #[test]
    fn unique_names_count_up() {
        let mut scene = boxes(1);
        scene.objects[0].name = "box".into();
        assert_eq!(unique_name(&scene, "lamp"), "lamp");
        assert_eq!(unique_name(&scene, "box"), "box 2");
        scene.objects[0].name = "box 2".into();
        assert_eq!(unique_name(&scene, "box 2"), "box 3");
    }

    #[test]
    fn selection_follows_deletes_and_undo() {
        let mut scene = boxes(3);
        let mut editor = editor();

        // Deleting an earlier object keeps the same object selected.
        editor.selected = Some(0);
        editor.delete_selected(&mut scene);
        assert_eq!(editor.selected, None);
        editor.selected = Some(1);
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(editor.selected, Some(0), "restored object is selected");
        assert_eq!(scene.objects[2].name, "object 2");

        editor.redo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(editor.selected, None);
        assert_eq!(scene.objects.len(), 2);
    }

    #[test]
    fn duplicate_goes_after_the_original_and_undoes() {
        let mut scene = boxes(2);
        let original = scene.clone();
        let mut editor = editor();
        editor.selected = Some(0);
        assert_eq!(
            editor.duplicate_selected(&mut scene),
            Some(Change::Added(1))
        );
        assert_eq!(editor.selected, Some(1));
        assert_eq!(scene.objects[1].name, "object 2");
        assert_eq!(scene.objects[1].transform, scene.objects[0].transform);

        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);
        assert_eq!(editor.selected, None);
    }
    #[test]
    fn added_primitive_lands_in_the_selected_zone_on_the_grid() {
        let mut scene = boxes(1);
        scene.zones.push(crate::scene::Zone::new("other"));
        scene.objects[0].zone = "other".into();
        let mut editor = editor();
        editor.selected = Some(0);
        let camera = Camera {
            position: Vec3::new(0.1, 1.7, 0.0),
            yaw: 0.3,
            pitch: -0.2,
            fov_y: 70.0,
            near: 0.1,
            far: 100.0,
        };
        let view = View {
            camera: &camera,
            image: Rect::NOTHING,
            looking: false,
            aim: None,
        };
        let (kind, shape) = primitives()[0];
        let change = editor.add_primitive(&mut scene, kind, shape, &view);
        assert_eq!(change, Some(Change::Added(1)));
        let added = &scene.objects[1];
        assert_eq!(added.zone, "other");
        let p = added.transform.position;
        assert_eq!(p, (p / SNAP).round() * SNAP);
        assert!((p - camera.position).length() > 3.0);
    }

    #[test]
    fn player_start_stands_on_the_floor_facing_like_the_camera() {
        let mut scene = boxes(1);
        let mut editor = editor();
        let camera = Camera {
            position: Vec3::new(0.0, 1.7, 0.0),
            // Looking toward +X (yaw turns right), slightly off the 15° grid.
            yaw: 92f32.to_radians(),
            pitch: -0.4,
            fov_y: 70.0,
            near: 0.1,
            far: 100.0,
        };
        let view = View {
            camera: &camera,
            image: Rect::NOTHING,
            looking: false,
            aim: Some((Vec3::new(3.0, 0.0, 0.1), Vec3::Y)),
        };
        editor.add_player_start(&mut scene, &view);
        let start = scene.player_start().expect("added");
        assert_eq!(start.position, Vec3::new(3.0, 0.0, 0.0));
        let facing = start.quat() * Vec3::NEG_Z;
        assert!((facing - Vec3::X).length() < 1e-5, "faces +X, got {facing}");
    }

    #[test]
    fn placement_rests_on_the_surface() {
        let half = primitives()[3].1.half_extents();
        let cylinder = (-half, half);
        let floor = Some((Vec3::new(0.3, 0.1, -2.0), Vec3::Y));
        // On a floor at y = 0.1: bottom touches it, x/z on the grid.
        assert_eq!(
            placement(cylinder, floor, Vec3::ZERO),
            Vec3::new(0.25, 0.6, -2.0),
            "half height 0.5 above the floor"
        );
        // Against a wall facing +X at x = 1.1.
        let wall = Some((Vec3::new(1.1, 0.4, -2.0), Vec3::X));
        assert_eq!(
            placement(cylinder, wall, Vec3::ZERO),
            Vec3::new(1.6, 0.5, -2.0)
        );
        // A model with its origin at its base (as exported from Blender)
        // sits with the origin on the floor.
        let model = (Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 3.0, 1.0));
        assert_eq!(
            placement(model, floor, Vec3::ZERO),
            Vec3::new(0.25, 0.1, -2.0)
        );
        // Nothing hit: the fallback, on the grid.
        assert_eq!(
            placement(model, None, Vec3::new(0.3, 1.0, -4.1)),
            Vec3::new(0.25, 1.0, -4.0)
        );
    }
}
