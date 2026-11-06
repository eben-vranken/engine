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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Zone {
    pub name: String,
    /// Color grading while the camera is in this zone; `None` = the
    /// environment's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<Grade>,
    /// Fog inside this zone's volumes; `None` = the environment's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog: Option<Fog>,
}

/// Volumetric fog: a medium that light scatters through, so lit air glows
/// (light shafts, halos around lanterns) and distant things fade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Fog {
    /// How thick the fog is (per meter); 0 = clear air.
    pub density: f32,
    /// Tints the light the fog scatters; (1, 1, 1) = neutral grey-white.
    pub color: Vec3,
    /// World height (meters) up to which the fog is at full density.
    pub height: f32,
    /// How quickly it thins above `height` (per meter); 0 = not at all.
    pub falloff: f32,
    /// Drifting wisps in the fog: 0 = even (and cheapest), 1 = strong
    /// swirls.
    pub wisps: f32,
    /// Size of the wisps (meters). About 0.5–1.5 reads as smoke in a light
    /// beam; much bigger than the beam and it only dims or brightens it.
    pub wisp_size: f32,
    /// How fast they drift (meters per second).
    pub wisp_speed: f32,
    /// Dust motes floating in the air, glittering where light catches them
    /// (invisible in the dark): 0 = none, 1 = a lot.
    pub dust: f32,
    /// Size of a mote in internal pixels.
    pub dust_size: f32,
    /// How bright a lit mote is.
    pub dust_brightness: f32,
    /// How fast the motes drift (meters per second).
    pub dust_speed: f32,
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            density: 0.02,
            color: Vec3::ONE,
            height: 2.0,
            falloff: 0.15,
            // Off by default: the noise costs about 4 ms of fog time on
            // integrated graphics.
            wisps: 0.0,
            wisp_size: 1.0,
            wisp_speed: 0.15,
            dust: 0.0,
            dust_size: 1.0,
            dust_brightness: 4.0,
            dust_speed: 0.05,
        }
    }
}

impl Zone {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }
}

/// Color grading, applied after tone mapping. Blends over a second or so
/// when the camera moves into a zone with a different grade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Grade {
    /// Stops of brightness before tone mapping; 0 = as lit.
    pub exposure: f32,
    /// 1 = unchanged; higher = punchier.
    pub contrast: f32,
    /// 1 = unchanged, 0 = grey.
    pub saturation: f32,
    /// Multiplies the final color; (1, 1, 1) = none.
    pub tint: Vec3,
    /// Optional `.cube` LUT in the project (e.g. `grades/cold.cube`),
    /// applied after the values above. Empty = none.
    pub lut: String,
}

impl Default for Grade {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            tint: Vec3::ONE,
            lut: String::new(),
        }
    }
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
    LightSource(LightSource),
    FogVolume(FogVolume),
    Material(Material),
}

/// How the object's surfaces look: a texture for primitives (instead of the
/// grid) and how glossy they are. Without one, surfaces are fully matte
/// (and models use the roughness and metalness from their glTF file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Material {
    /// A PNG or JPEG in the project, e.g. `textures/stone.png`, point-sampled
    /// like the grid; empty = the grid. Primitives only.
    pub texture: String,
    /// Meters one repeat of the texture covers.
    pub meters: f32,
    /// 0 = mirror-smooth (polished marble, still water), 1 = fully matte.
    pub roughness: f32,
    /// 0 = stone, wood, plaster; 1 = bare metal (reflections take its color).
    pub metalness: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            texture: String::new(),
            meters: 2.0,
            roughness: 0.5,
            metalness: 0.0,
        }
    }
}

/// Extra fog in a box centered on the object (turned and scaled with it),
/// on top of the zone's: mist over water, smoke in a corner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct FogVolume {
    /// Meters, before the object's scale.
    pub size: Vec3,
    pub density: f32,
    pub color: Vec3,
    /// How quickly it thins from the bottom of the box upward (per meter).
    pub falloff: f32,
    /// 0 = hard box edges, 1 = fades in from the edges all the way to the
    /// center.
    pub softness: f32,
}

impl Default for FogVolume {
    fn default() -> Self {
        Self {
            size: Vec3::new(4.0, 2.0, 4.0),
            density: 0.3,
            color: Vec3::ONE,
            falloff: 0.5,
            softness: 0.5,
        }
    }
}

/// A point light: lantern, candle, brazier. Lights the scene in real time
/// and casts shadows; with `baked`, the light it bounces off surfaces is
/// also baked into the lightmaps (keep it for lights that never move).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct LightSource {
    /// Linear RGB.
    pub color: Vec3,
    /// Brightness 1 m away; it falls off with the square of the distance.
    pub intensity: f32,
    /// Meters; the light fades out completely by here.
    pub range: f32,
    /// 0 = steady, 1 = a nervous flame.
    pub flicker: f32,
    pub shadows: bool,
    /// Its bounce light goes into the lightmap (BAKE LIGHTING).
    pub baked: bool,
}

impl Default for LightSource {
    fn default() -> Self {
        Self {
            color: Vec3::new(1.0, 0.62, 0.3),
            intensity: 3.0,
            range: 8.0,
            flicker: 0.0,
            shadows: true,
            baked: true,
        }
    }
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
    /// Reverb probes (BAKE ACOUSTICS): meters between them across the
    /// floors inside the volume.
    pub probe_spacing: f32,
    /// Meters above the floor: about ear height.
    pub probe_height: f32,
}

impl Default for ZoneVolume {
    fn default() -> Self {
        Self {
            size: Vec3::new(4.0, 3.0, 4.0),
            probe_spacing: 2.0,
            probe_height: 1.5,
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
    /// Light the surface gives off (linear RGB, above 1 for bright glow);
    /// it lights its surroundings in the bake and glows in the fog.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub emissive: Vec3,
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
    /// Light the surface gives off; see `Primitive::emissive`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub emissive: Vec3,
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

fn is_zero(value: &Vec3) -> bool {
    *value == Vec3::ZERO
}

/// One drawable part of an object.
#[derive(Debug, Clone, Copy)]
pub struct Renderable<'a> {
    pub object: usize,
    pub geometry: Geometry<'a>,
    pub color: Vec3,
    pub emissive: Vec3,
    pub transform: &'a Transform,
    /// The object's `Material`, if it has one.
    pub material: Option<&'a Material>,
}

/// Global lighting and atmosphere. All colors are linear RGB; light colors
/// include intensity, so values above 1 are expected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Environment {
    /// Direction the sunlight travels (from the sun toward the ground).
    pub sun_direction: Vec3,
    pub sun_color: Vec3,
    /// Ambient light from above and below; normals blend between them.
    pub ambient_sky: Vec3,
    pub ambient_ground: Vec3,
    /// Fog outside zones that have their own.
    pub fog: Fog,
    /// How much fog scatters light forward (toward a camera looking at the
    /// light): 0 = evenly, 0.9 = strongly. Makes shafts brighter against
    /// the light.
    pub fog_anisotropy: f32,
    /// Meters of fog simulated in front of the camera; beyond, nothing
    /// changes any more.
    pub fog_distance: f32,
    /// Meters around the camera where the sun casts real-time shadows.
    pub shadow_distance: f32,
    /// How much bright light glows past its edges; 0 = off.
    pub bloom: f32,
    /// Film grain strength; 0 = off.
    pub grain: f32,
    /// Grading for zones without their own.
    pub grade: Grade,
    /// Lightmap texels per meter (BAKE LIGHTING); higher = finer bounce
    /// light and contact shadows, but longer bakes and more memory.
    pub lightmap_density: f32,
    /// Rays per lightmap texel; more = less blotchy, slower bake.
    pub bake_samples: u32,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            sun_direction: Vec3::new(-0.45, -1.0, -0.3).normalize(),
            sun_color: Vec3::new(1.0, 0.9, 0.78) * 2.6,
            ambient_sky: Vec3::new(0.16, 0.18, 0.24),
            ambient_ground: Vec3::new(0.05, 0.045, 0.04),
            fog: Fog::default(),
            fog_anisotropy: 0.35,
            fog_distance: 60.0,
            shadow_distance: 40.0,
            bloom: 0.05,
            grain: 0.03,
            grade: Grade::default(),
            lightmap_density: 4.0,
            bake_samples: 256,
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

    /// Everything visible.
    pub fn renderables(&self) -> impl Iterator<Item = Renderable<'_>> {
        self.objects.iter().enumerate().flat_map(|(index, object)| {
            let material = object.components.iter().find_map(|c| match c {
                Component::Material(m) => Some(m),
                _ => None,
            });
            object.components.iter().filter_map(move |c| {
                let (geometry, color, emissive) = match c {
                    Component::Primitive(p) => (Geometry::Shape(p.shape), p.color, p.emissive),
                    Component::Model(m) => (Geometry::Model(&m.path), m.color, m.emissive),
                    _ => return None,
                };
                Some(Renderable {
                    object: index,
                    geometry,
                    color,
                    emissive,
                    transform: &object.transform,
                    material,
                })
            })
        })
    }

    /// Every light: object index, the light, and where it is.
    pub fn lights(&self) -> impl Iterator<Item = (usize, &LightSource, Vec3)> {
        self.objects.iter().enumerate().flat_map(|(index, object)| {
            object.components.iter().filter_map(move |c| match c {
                Component::LightSource(light) => Some((index, light, object.transform.position)),
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
        zones: vec![Zone::new("test")],
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
                    emissive: Vec3::ZERO,
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
            zones: vec![Zone::new("hall")],
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
                        emissive: Vec3::ZERO,
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
