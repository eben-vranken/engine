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
/// belongs to exactly one. Connections between zones come later.
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

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.quat(), self.position)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
pub enum Component {
    Primitive(Primitive),
    /// Where the player spawns; faces the object's yaw.
    PlayerStart,
}

/// A shape generated in code, drawn with the grid texture and a collider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
pub struct Primitive {
    pub shape: Shape,
    /// Linear RGB tint multiplied with the grid texture.
    #[serde(default = "white")]
    pub color: Vec3,
}

fn white() -> Vec3 {
    Vec3::ONE
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

    /// Every primitive with its object's index and world transform.
    pub fn primitives(&self) -> impl Iterator<Item = (usize, &Primitive, &Transform)> {
        self.objects.iter().enumerate().flat_map(|(index, object)| {
            object.components.iter().filter_map(move |c| match c {
                Component::Primitive(p) => Some((index, p, &object.transform)),
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
        assert_eq!(scene.primitives().count(), 1);
    }
}
