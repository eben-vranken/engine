//! Prefabs: reusable groups of objects. A prefab is an ordinary scene file
//! in the project's `prefabs/` folder (a model with a light at its front,
//! say), edited in a tab like any scene. A `Prefab` component places one:
//! its objects are drawn, lit and collided as part of that object
//! (`Object::pieces`), so moving, selecting and saving the placement is
//! moving, selecting and saving one object.
//!
//! The files are read here and kept, shared by every placement; `resolve`
//! hands a scene's placements their prefab's objects. Prefabs may place
//! other prefabs; one that places itself (directly or not) stops there.

use crate::assets::{Assets, Resolved};
use crate::render::mesh::MeshData;
use crate::scene::{Component, Geometry, Object, Parts, PrefabContents, Scene, Transform, Zone};
use glam::Vec3;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

/// The folder prefabs live in, inside the project.
pub const FOLDER: &str = "prefabs";

/// The zone every object of a prefab file is in (placed, they are in the
/// placement's zone).
pub const ZONE: &str = "prefab";

/// Prefab files as read, by path relative to the project.
#[derive(Default)]
pub struct Prefabs {
    cached: HashMap<String, Cached>,
}

struct Cached {
    objects: Option<Arc<Vec<Object>>>,
    /// The file's time when it was read.
    modified: Option<SystemTime>,
}

impl Prefabs {
    /// The objects of the prefab at `path`, read (with the prefabs inside
    /// it) on first use and kept until `changed` sees a file change. `None`
    /// if it can't be read (logged).
    pub fn get(&mut self, root: &Path, path: &str) -> Option<Arc<Vec<Object>>> {
        self.load(root, path, &mut Vec::new())
    }

    fn load(
        &mut self,
        root: &Path,
        path: &str,
        loading: &mut Vec<String>,
    ) -> Option<Arc<Vec<Object>>> {
        if path.is_empty() {
            return None;
        }
        if loading.iter().any(|p| p == path) {
            log::warn!(
                "{path} places itself (through {}); left out",
                loading.join(" > ")
            );
            return None;
        }
        if let Some(cached) = self.cached.get(path) {
            return cached.objects.clone();
        }
        let file = root.join(path);
        let modified = modified_time(&file);
        let objects = match Scene::load(&file) {
            Ok(mut scene) => {
                loading.push(path.to_owned());
                for object in &mut scene.objects {
                    for component in &mut object.components {
                        if let Component::Prefab(inner) = component {
                            let inner_path = inner.prefab.0.clone();
                            inner.contents = PrefabContents(self.load(root, &inner_path, loading));
                        }
                    }
                }
                loading.pop();
                Some(Arc::new(scene.objects))
            }
            Err(err) => {
                log::error!("{err}");
                None
            }
        };
        self.cached.insert(
            path.to_owned(),
            Cached {
                objects: objects.clone(),
                modified,
            },
        );
        objects
    }

    /// The prefabs whose file changed since it was read. Then every prefab
    /// is read again on next use (one may place the changed one). Call it
    /// periodically.
    pub fn changed(&mut self, root: &Path) -> Vec<String> {
        let changed: Vec<String> = self
            .cached
            .iter()
            .filter(|(path, cached)| modified_time(&root.join(path)) != cached.modified)
            .map(|(path, _)| path.clone())
            .collect();
        if !changed.is_empty() {
            self.cached.clear();
        }
        changed
    }
}

/// Gives every `Prefab` placement in the scene its prefab's objects. Cheap:
/// each file is read once and shared until it changes.
pub fn resolve(scene: &mut Scene, assets: &mut Assets) {
    let root = assets.root().to_path_buf();
    for object in &mut scene.objects {
        for component in &mut object.components {
            if let Component::Prefab(prefab) = component {
                let path = prefab.prefab.0.clone();
                prefab.contents = PrefabContents(assets.prefabs.get(&root, &path));
            }
        }
    }
}

/// The box around everything the prefab at `path` draws, around its
/// origin; `None` if it draws nothing or can't be read.
pub fn bounds(assets: &mut Assets, path: &str) -> Option<(Vec3, Vec3)> {
    let root = assets.root().to_path_buf();
    let objects = assets.prefabs.get(&root, path)?;
    objects
        .iter()
        .filter_map(|object| world_bounds(object, assets))
        .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
}

/// The box around everything the object draws, in the world (its
/// prefab's pieces too); `None` if it draws nothing.
pub fn world_bounds(object: &Object, assets: &mut Assets) -> Option<(Vec3, Vec3)> {
    let (mut min, mut max) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for (piece, placed) in object.pieces() {
        for component in &piece.components {
            let geometry = match component {
                Component::Primitive(p) => Geometry::Shape(p.shape),
                Component::Model(m) => Geometry::Model(&m.path, Parts::All),
                _ => continue,
            };
            let (low, high) = match assets.resolve(geometry) {
                Resolved::Shape(shape) => MeshData::from_shape(shape).bounds(),
                Resolved::Model(model) => model.mesh.bounds(),
            };
            let matrix = placed.matrix();
            for corner in 0..8 {
                let pick = |bit: usize, a: f32, b: f32| if corner & bit == 0 { a } else { b };
                let local = Vec3::new(
                    pick(1, low.x, high.x),
                    pick(2, low.y, high.y),
                    pick(4, low.z, high.z),
                );
                let world = matrix.transform_point3(local);
                min = min.min(world);
                max = max.max(world);
            }
        }
    }
    min.is_finite().then_some((min, max))
}

/// A new prefab of one model: the model at the origin in a zone of its
/// own. Its objects can be added to and moved around in its tab.
pub fn of_model(model_path: &str) -> Scene {
    let stem = stem(model_path);
    Scene {
        zones: vec![Zone::new(ZONE)],
        objects: vec![Object {
            name: stem.to_owned(),
            id: 0,
            zone: ZONE.into(),
            transform: Transform::default(),
            components: vec![Component::Model(crate::scene::Model {
                path: model_path.to_owned(),
                color: Vec3::ONE,
                emissive: Vec3::ZERO,
                collision: true,
                parts: Vec::new(),
            })],
            hidden: false,
            play: Default::default(),
        }],
        ..Default::default()
    }
}

/// Where a new prefab of `model_path` goes: `prefabs/<model name>.ron`, or
/// with a number after the name if that is taken.
pub fn new_path(root: &Path, model_path: &str) -> String {
    let stem = stem(model_path);
    let mut path = format!("{FOLDER}/{stem}.ron");
    let mut n = 2;
    while root.join(&path).exists() {
        path = format!("{FOLDER}/{stem}_{n}.ron");
        n += 1;
    }
    path
}

/// Whether a project path is a prefab file.
pub fn is_prefab(path: &Path) -> bool {
    path.starts_with(FOLDER) && path.extension().is_some_and(|e| e == "ron")
}

/// A file's name without its folder and extension.
pub fn stem(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::{LightSource, Prefab, PrefabRef, Primitive};

    fn write(dir: &Path, path: &str, scene: &Scene) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        scene.save(&file).unwrap();
    }

    fn object(name: &str, at: Vec3, component: Component) -> Object {
        Object {
            name: name.into(),
            id: 0,
            zone: ZONE.into(),
            transform: Transform::at(at),
            components: vec![component],
            hidden: false,
            play: Default::default(),
        }
    }

    fn placement(path: &str, at: Vec3) -> Object {
        object(
            "placed",
            at,
            Component::Prefab(Prefab {
                prefab: PrefabRef(path.into()),
                contents: Default::default(),
            }),
        )
    }

    fn lamp() -> Scene {
        Scene {
            zones: vec![Zone::new(ZONE)],
            objects: vec![
                object(
                    "body",
                    Vec3::ZERO,
                    Component::Primitive(Primitive {
                        shape: Shape::Box { size: Vec3::ONE },
                        color: Vec3::ONE,
                        emissive: Vec3::ZERO,
                        collision: true,
                    }),
                ),
                object(
                    "beam",
                    Vec3::new(0.0, 0.0, -0.5),
                    Component::LightSource(LightSource {
                        cone: 40.0,
                        ..Default::default()
                    }),
                ),
            ],
            ..Default::default()
        }
    }

    fn temp_project(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("engine_prefab_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_placement_draws_and_lights_its_prefab_where_it_is() {
        let dir = temp_project("place");
        write(&dir, "prefabs/lamp.ron", &lamp());
        let mut scene = crate::scene::test_scene(Vec::new());
        let mut placed = placement("prefabs/lamp.ron", Vec3::new(5.0, 1.0, 0.0));
        placed.zone = scene.zones[0].name.clone();
        // Turned to face +X: its beam shines that way too.
        placed.transform.rotation.y = -90.0;
        scene.objects.push(placed);
        let mut assets = Assets::new(dir.clone());
        resolve(&mut scene, &mut assets);

        let drawn: Vec<_> = scene.renderables().collect();
        assert_eq!(drawn.len(), 1);
        assert!(
            drawn[0]
                .transform
                .position
                .distance(Vec3::new(5.0, 1.0, 0.0))
                < 1e-4
        );
        let lights = scene.lights(|_, _| None);
        assert_eq!(lights.len(), 1);
        assert!(lights[0].position.distance(Vec3::new(5.5, 1.0, 0.0)) < 1e-4);
        assert!(
            lights[0].direction.distance(Vec3::X) < 1e-4,
            "{}",
            lights[0].direction
        );
        // Its pieces collide as part of the placement.
        assert_eq!(scene.colliders().count(), 1);
        // Switched off, the light is gone; the body stays.
        scene.objects[0].play.lights_off = true;
        assert!(scene.lights(|_, _| None).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_holdable_prefab_makes_every_placement_holdable() {
        let dir = temp_project("holdable");
        let mut torch = lamp();
        torch.objects[0]
            .components
            .push(Component::Holdable(crate::scene::Holdable::default()));
        write(&dir, "prefabs/torch.ron", &torch);
        let mut scene = Scene {
            zones: vec![Zone::new(ZONE)],
            objects: vec![placement("prefabs/torch.ron", Vec3::ZERO)],
            ..Default::default()
        };
        assert!(scene.objects[0].holdable().is_none());
        resolve(&mut scene, &mut Assets::new(dir.clone()));
        assert!(scene.objects[0].holdable().is_some());
        assert!(scene.objects[0].is_dynamic());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_contents_are_not_saved_with_the_scene() {
        let dir = temp_project("save");
        write(&dir, "prefabs/lamp.ron", &lamp());
        let mut scene = Scene {
            zones: vec![Zone::new(ZONE)],
            objects: vec![placement("prefabs/lamp.ron", Vec3::ZERO)],
            ..Default::default()
        };
        resolve(&mut scene, &mut Assets::new(dir.clone()));
        let text = ron::to_string(&scene).unwrap();
        assert!(
            text.contains("prefabs/lamp.ron") && !text.contains("beam"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefabs_place_prefabs_but_not_themselves() {
        let dir = temp_project("nested");
        write(&dir, "prefabs/lamp.ron", &lamp());
        let pair = Scene {
            zones: vec![Zone::new(ZONE)],
            objects: vec![
                placement("prefabs/lamp.ron", Vec3::X),
                placement("prefabs/lamp.ron", -Vec3::X),
                // Itself: left out.
                placement("prefabs/pair.ron", Vec3::ZERO),
            ],
            ..Default::default()
        };
        write(&dir, "prefabs/pair.ron", &pair);
        let mut scene = Scene {
            zones: vec![Zone::new(ZONE)],
            objects: vec![placement("prefabs/pair.ron", Vec3::ZERO)],
            ..Default::default()
        };
        resolve(&mut scene, &mut Assets::new(dir.clone()));
        assert_eq!(scene.renderables().count(), 2);
        assert_eq!(scene.lights(|_, _| None).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let dir = temp_project("reload");
        write(&dir, "prefabs/lamp.ron", &lamp());
        let mut prefabs = Prefabs::default();
        assert_eq!(prefabs.get(&dir, "prefabs/lamp.ron").unwrap().len(), 2);
        let mut smaller = lamp();
        smaller.objects.pop();
        // Some file systems keep times to the second.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write(&dir, "prefabs/lamp.ron", &smaller);
        assert_eq!(prefabs.changed(&dir), ["prefabs/lamp.ron"]);
        assert_eq!(prefabs.get(&dir, "prefabs/lamp.ron").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sandbox_flashlight_is_a_holdable_cone_light() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let mut scene = Scene::load(&root.join("scenes/dark_room.ron")).unwrap();
        resolve(&mut scene, &mut Assets::new(root));
        let flashlight = scene
            .objects
            .iter()
            .find(|o| o.name == "flashlight")
            .unwrap();
        assert!(flashlight.holdable().is_some());
        let lights = scene.lights(|_, _| None);
        assert_eq!(lights.len(), 1);
        assert!(lights[0].light.cone_cosines().is_some());
    }

    #[test]
    fn new_prefabs_are_named_after_their_model() {
        let dir = temp_project("names");
        assert_eq!(new_path(&dir, "models/torch.glb"), "prefabs/torch.ron");
        write(&dir, "prefabs/torch.ron", &of_model("models/torch.glb"));
        assert_eq!(new_path(&dir, "models/torch.glb"), "prefabs/torch_2.ron");
        let scene = of_model("models/props/torch.glb");
        assert!(scene.validate().is_ok());
        assert_eq!(scene.objects[0].name, "torch");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
