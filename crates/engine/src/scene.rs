//! Scenes: the environment, the zones, and the objects in them. Stored as
//! RON in the game project's `scenes/` folder.
//!
//! An object is a name, the zone it belongs to, a transform, and a list of
//! components. Adding a component type means writing its struct and adding
//! one variant to [`Component`].

use crate::render::mesh::Shape;
use bevy_reflect::Reflect;
use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub environment: Environment,
    pub zones: Vec<Zone>,
    pub objects: Vec<Object>,
}

/// A region of the world: a room, an island, a building. Every object
/// belongs to exactly one. Where a zone is comes from its `ZoneVolume`
/// objects; how zones connect, from `Portal` objects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub name: String,
    /// Name of the zone this object belongs to.
    pub zone: String,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default)]
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Transform {
    /// Meters.
    pub position: Vec3,
    /// Degrees around X, Y and Z, applied yaw (Y) first, then pitch (X),
    /// then roll (Z).
    pub rotation: Vec3,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
        }
    }
}

impl Transform {
    pub fn at(position: Vec3) -> Self {
        Self {
            position,
            ..Default::default()
        }
    }

    pub fn quat(&self) -> Quat {
        let r = self.rotation * (std::f32::consts::PI / 180.0);
        Quat::from_euler(EulerRot::YXZ, r.y, r.x, r.z)
    }

    /// Sets the rotation from a quaternion (the inverse of `quat`).
    pub fn set_quat(&mut self, rotation: Quat) {
        let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
        self.rotation = Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI);
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.quat(), self.position)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
pub enum Component {
    Primitive(Primitive),
    Model(Model),
    /// Where the player spawns; faces the object's yaw.
    PlayerStart,
    Floating(Floating),
    Door(Door),
    Switch(Switch),
    PhysicsBody(PhysicsBody),
    SoundSource(SoundSource),
    AcousticMaterial(AcousticMaterial),
    ZoneVolume(ZoneVolume),
    Portal(Portal),
}

/// Another object in the scene, by name. The inspector shows a picker.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Reflect)]
#[serde(transparent)]
pub struct ObjectRef(pub String);

/// A zone in the scene, by name. The inspector shows a picker.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Reflect)]
#[serde(transparent)]
pub struct ZoneRef(pub String);

/// A box of space that belongs to the object's zone, centered on the
/// object (rotated and scaled with it). Whoever is inside it is in that
/// zone; a zone can have several. Where volumes overlap, the smallest wins,
/// so a room can sit inside a larger outdoor volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct ZoneVolume {
    /// Meters, before the object's scale.
    pub size: Vec3,
}

impl Default for ZoneVolume {
    fn default() -> Self {
        Self {
            size: Vec3::new(4.0, 3.0, 4.0),
        }
    }
}

/// A doorway or passage: connects the object's zone with another. Zones
/// that share a portal are neighbours.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Portal {
    pub to: ZoneRef,
}

/// A shape generated in code, drawn with the grid texture and a collider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
pub struct Primitive {
    pub shape: Shape,
    /// Linear RGB tint multiplied with the grid texture.
    #[serde(default = "white")]
    pub color: Vec3,
    /// Blocks the player. Off: walk-through, but still clickable in the editor.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub collision: bool,
}

/// A glTF model from the game project, drawn with its materials' base color
/// (texture × color, point-sampled) and given a matching triangle-mesh
/// collider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
pub struct Model {
    /// Path relative to the project folder, e.g. `models/arch.gltf`.
    pub path: String,
    /// Linear RGB tint.
    #[serde(default = "white")]
    pub color: Vec3,
    /// Blocks the player. Off: walk-through, but still clickable in the editor.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub collision: bool,
}

/// Bobs up and down around where it was placed while the game runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Floating {
    /// Meters above and below the rest position.
    pub height: f32,
    /// Bobs per second.
    pub speed: f32,
    /// Where in the bob it starts, 0–1, so neighbours can bob out of step.
    pub phase: f32,
}

impl Default for Floating {
    fn default() -> Self {
        Self {
            height: 0.25,
            speed: 0.25,
            phase: 0.0,
        }
    }
}

/// Opens and closes when a switch targets it: slides, swings around a
/// hinge, or both. Placed closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Door {
    /// How far it slides when open, along its own axes (meters).
    pub slide: Vec3,
    /// Degrees it turns around its vertical axis when open.
    pub swing: f32,
    /// The point it swings around, along its own axes from its origin
    /// (meters); e.g. (-1, 0, 0) is the left edge of a 2 m wide box door.
    pub hinge: Vec3,
    /// Seconds to open or close.
    pub duration: f32,
    pub starts_open: bool,
}

impl Default for Door {
    fn default() -> Self {
        Self {
            slide: Vec3::ZERO,
            swing: 90.0,
            hinge: Vec3::ZERO,
            duration: 1.0,
            starts_open: false,
        }
    }
}

/// Opens or closes its target door: the player presses E while looking at
/// it, or, as a pressure plate, stands on it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Switch {
    pub target: ObjectRef,
    /// Toggles the target while the player stands on it, and back when
    /// they step off.
    pub pressure_plate: bool,
}

/// Falls, tumbles and can be pushed while the game runs. In the editor it
/// stays where it's placed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct PhysicsBody {
    /// kg; the player (about 80 kg) pushes heavy bodies slower.
    pub mass: f32,
    /// Hangs where it's placed until something touches it.
    pub starts_asleep: bool,
}

impl Default for PhysicsBody {
    fn default() -> Self {
        Self {
            mass: 20.0,
            starts_asleep: false,
        }
    }
}

/// Plays a sound from the object's position, heard through the acoustic
/// simulation. Plays in the editor too, so spaces can be listened to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct SoundSource {
    /// A WAV file in the project, e.g. `audio/drip.wav`; empty plays a
    /// built-in plucked-string phrase.
    pub clip: String,
    /// 1 = as recorded.
    pub volume: f32,
    pub looping: bool,
    /// How big the sound is for walls and doors (meters): a bigger sound
    /// fades in more gradually as a door opens or you round a corner.
    pub radius: f32,
}

impl Default for SoundSource {
    fn default() -> Self {
        Self {
            clip: String::new(),
            volume: 1.0,
            looping: true,
            radius: 1.0,
        }
    }
}

/// How the object's surfaces sound: what they absorb, scatter and let
/// through. Objects without one sound `Generic`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct AcousticMaterial {
    pub preset: AcousticPreset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Reflect)]
pub enum AcousticPreset {
    #[default]
    Generic,
    Wood,
    Stone,
    Marble,
    Brick,
    Concrete,
    Plaster,
    Carpet,
    /// Heavy curtains and soft furnishings.
    Velvet,
    Glass,
    Metal,
    Water,
    Gravel,
}

/// What an object looks like and collides as.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Geometry<'a> {
    Shape(Shape),
    Model(&'a str),
}

fn white() -> Vec3 {
    Vec3::ONE
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// Global lighting and atmosphere. All colors are linear RGB; light colors
/// include intensity, so values above 1 are expected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Environment {
    /// Direction the sunlight travels (from the sun toward the ground).
    pub sun_direction: Vec3,
    pub sun_color: Vec3,
    /// Ambient light from above and below; normals blend between them.
    pub ambient_sky: Vec3,
    pub ambient_ground: Vec3,
    pub fog_color: Vec3,
    /// Fog per meter; 0 turns fog off.
    pub fog_density: f32,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            sun_direction: Vec3::new(-0.45, -1.0, -0.3).normalize(),
            sun_color: Vec3::new(1.0, 0.9, 0.78) * 2.6,
            ambient_sky: Vec3::new(0.16, 0.18, 0.24),
            ambient_ground: Vec3::new(0.05, 0.045, 0.04),
            fog_color: Vec3::new(0.045, 0.05, 0.065),
            fog_density: 0.035,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<ron::error::SpannedError>,
    },
    #[error("could not serialize the scene: {0}")]
    Serialize(#[from] ron::Error),
    #[error("object \"{object}\" is in zone \"{zone}\", which the scene doesn't define")]
    UnknownZone { object: String, zone: String },
}

impl Scene {
    pub fn load(path: &Path) -> Result<Self, SceneError> {
        let text = std::fs::read_to_string(path).map_err(|source| SceneError::Read {
            path: path.to_owned(),
            source,
        })?;
        let scene: Scene = ron::from_str(&text).map_err(|source| SceneError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })?;
        scene.validate()?;
        Ok(scene)
    }

    pub fn save(&self, path: &Path) -> Result<(), SceneError> {
        self.validate()?;
        // Objects stay one block each; small values (vectors, shapes) go on one line.
        let pretty = ron::ser::PrettyConfig::default().depth_limit(3);
        let text = ron::ser::to_string_pretty(self, pretty)?;
        std::fs::write(path, text + "\n").map_err(|source| SceneError::Write {
            path: path.to_owned(),
            source,
        })
    }

    /// Every object must belong to a zone the scene defines.
    pub fn validate(&self) -> Result<(), SceneError> {
        for object in &self.objects {
            if !self.zones.iter().any(|z| z.name == object.zone) {
                return Err(SceneError::UnknownZone {
                    object: object.name.clone(),
                    zone: object.zone.clone(),
                });
            }
        }
        Ok(())
    }

    /// Everything visible: object index, geometry, color and transform.
    pub fn renderables(&self) -> impl Iterator<Item = (usize, Geometry<'_>, Vec3, &Transform)> {
        self.objects.iter().enumerate().flat_map(|(index, object)| {
            object.components.iter().filter_map(move |c| match c {
                Component::Primitive(p) => {
                    Some((index, Geometry::Shape(p.shape), p.color, &object.transform))
                }
                Component::Model(m) => {
                    Some((index, Geometry::Model(&m.path), m.color, &object.transform))
                }
                _ => None,
            })
        })
    }

    /// Everything with a collider: object index, geometry, transform, and
    /// whether it blocks the player.
    pub fn colliders(&self) -> impl Iterator<Item = (usize, Geometry<'_>, &Transform, bool)> {
        self.objects.iter().enumerate().flat_map(|(index, object)| {
            object.components.iter().filter_map(move |c| match c {
                Component::Primitive(p) => Some((
                    index,
                    Geometry::Shape(p.shape),
                    &object.transform,
                    p.collision,
                )),
                Component::Model(m) => Some((
                    index,
                    Geometry::Model(&m.path),
                    &object.transform,
                    m.collision,
                )),
                _ => None,
            })
        })
    }

    /// Transform of the first object with a `PlayerStart`.
    pub fn player_start(&self) -> Option<&Transform> {
        self.objects
            .iter()
            .find(|o| o.components.contains(&Component::PlayerStart))
            .map(|o| &o.transform)
    }
}

/// A one-zone scene of grey primitives, for tests.
#[cfg(test)]
pub(crate) fn test_scene(shapes: Vec<(Shape, Vec3)>) -> Scene {
    Scene {
        zones: vec![Zone {
            name: "test".into(),
        }],
        objects: shapes
            .into_iter()
            .enumerate()
            .map(|(i, (shape, position))| Object {
                name: format!("object {i}"),
                zone: "test".into(),
                transform: Transform::at(position),
                components: vec![Component::Primitive(Primitive {
                    shape,
                    color: Vec3::ONE,
                    collision: true,
                })],
            })
            .collect(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Scene {
        Scene {
            zones: vec![Zone {
                name: "hall".into(),
            }],
            objects: vec![
                Object {
                    name: "floor".into(),
                    zone: "hall".into(),
                    transform: Transform::at(Vec3::new(0.0, -0.25, 0.0)),
                    components: vec![Component::Primitive(Primitive {
                        shape: Shape::Box {
                            size: Vec3::new(10.0, 0.5, 10.0),
                        },
                        color: Vec3::splat(0.6),
                        collision: true,
                    })],
                },
                Object {
                    name: "start".into(),
                    zone: "hall".into(),
                    transform: Transform {
                        rotation: Vec3::new(0.0, 90.0, 0.0),
                        ..Default::default()
                    },
                    components: vec![Component::PlayerStart],
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_through_ron() {
        let scene = sample();
        let text = ron::ser::to_string_pretty(&scene, Default::default()).unwrap();
        let back: Scene = ron::from_str(&text).unwrap();
        assert_eq!(scene, back);
    }

    #[test]
    fn collision_is_on_unless_turned_off() {
        let scene: Scene = ron::from_str(
            r#"(zones: [(name: "a")], objects: [(name: "x", zone: "a", components: [
                Primitive((shape: Box(size: (1.0, 1.0, 1.0)))),
                Model((path: "models/m.glb", collision: false)),
            ])])"#,
        )
        .unwrap();
        let solid: Vec<bool> = scene.colliders().map(|(.., solid)| solid).collect();
        assert_eq!(solid, [true, false]);
        // Only the exception is written back.
        let text = ron::to_string(&scene).unwrap();
        assert_eq!(text.matches("collision").count(), 1);
    }

    #[test]
    fn every_sandbox_scene_loads() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/scenes");
        let mut count = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "ron") {
                Scene::load(&path).unwrap_or_else(|err| panic!("{err}"));
                count += 1;
            }
        }
        assert!(count >= 2);
    }

    #[test]
    fn unknown_zone_is_an_error() {
        let mut scene = sample();
        scene.objects[0].zone = "cellar".into();
        assert!(matches!(
            scene.validate(),
            Err(SceneError::UnknownZone { .. })
        ));
    }

    #[test]
    fn minimal_object_uses_defaults() {
        let scene: Scene =
            ron::from_str(r#"(zones: [(name: "a")], objects: [(name: "x", zone: "a")])"#).unwrap();
        assert_eq!(scene.objects[0].transform, Transform::default());
        assert!(scene.objects[0].components.is_empty());
    }

    #[test]
    fn rotation_round_trips_through_a_quaternion() {
        let mut t = Transform {
            rotation: Vec3::new(20.0, -135.0, 10.0),
            ..Default::default()
        };
        let q = t.quat();
        t.set_quat(q);
        assert!((t.quat().dot(q).abs() - 1.0).abs() < 1e-5);
        assert!((t.rotation - Vec3::new(20.0, -135.0, 10.0)).length() < 1e-3);
    }

    #[test]
    fn switch_targets_are_plain_names_in_ron() {
        let switch = Switch {
            target: ObjectRef("hall door".into()),
            pressure_plate: false,
        };
        let text = ron::to_string(&switch).unwrap();
        assert!(text.contains(r#"target:"hall door""#), "{text}");
        assert_eq!(ron::from_str::<Switch>(&text).unwrap(), switch);
    }

    #[test]
    fn positive_y_rotation_turns_left() {
        // Y rotation is counter-clockwise seen from above (right-handed), so
        // an object's forward (-Z) turns toward -X. The camera's yaw turns
        // the other way; see `Spawn::from_transform`.
        let t = Transform {
            rotation: Vec3::new(0.0, 90.0, 0.0),
            ..Default::default()
        };
        let forward = t.quat() * Vec3::NEG_Z;
        assert!((forward - Vec3::NEG_X).length() < 1e-5);
    }

    #[test]
    fn finds_player_start() {
        let scene = sample();
        assert_eq!(scene.player_start().unwrap().rotation.y, 90.0);
        assert_eq!(scene.renderables().count(), 1);
    }
}
