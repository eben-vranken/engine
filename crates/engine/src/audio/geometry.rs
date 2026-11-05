//! The scene's solid geometry as Steam Audio sees it, for occlusion and
//! transmission (and later reflections). Static objects are merged into one
//! mesh that is rebuilt when the scene is edited; moving objects (doors,
//! floating platforms, physics bodies) are instanced meshes whose transforms
//! follow the objects every frame.

use super::AudioError;
use crate::assets::{Assets, Resolved};
use crate::render::mesh::MeshData;
use crate::scene::{AcousticPreset, Component, Scene};
use audionimbus::{
    Context, DefaultRayTracer, InstancedMesh, InstancedMeshHandle, InstancedMeshSettings, Material,
    Matrix, Point, StaticMesh, StaticMeshSettings, Triangle,
};
use glam::Mat4;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Every preset, in declaration order (so `preset as usize` indexes it).
const PRESETS: [AcousticPreset; 13] = [
    AcousticPreset::Generic,
    AcousticPreset::Wood,
    AcousticPreset::Stone,
    AcousticPreset::Marble,
    AcousticPreset::Brick,
    AcousticPreset::Concrete,
    AcousticPreset::Plaster,
    AcousticPreset::Carpet,
    AcousticPreset::Velvet,
    AcousticPreset::Glass,
    AcousticPreset::Metal,
    AcousticPreset::Water,
    AcousticPreset::Gravel,
];

pub struct AcousticGeometry {
    pub scene: audionimbus::Scene,
    /// Moving objects: object index, instance, and its current transform.
    instances: Vec<(usize, InstancedMeshHandle, Mat4)>,
}

impl AcousticGeometry {
    pub fn build(
        context: &Context,
        scene: &Scene,
        assets: &mut Assets,
    ) -> Result<Self, AudioError> {
        let mut acoustic = audionimbus::Scene::try_new(context).map_err(steam)?;
        let materials: Vec<Material> = PRESETS.iter().map(|&p| material(p)).collect();
        let mut world = MeshBuilder::default();
        let mut instances = Vec::new();
        for (object, geometry, transform, solid) in scene.colliders() {
            // Walk-through objects (collision off) don't block sound either.
            if !solid {
                continue;
            }
            let resolved = assets.resolve(geometry);
            let shape_mesh;
            let mesh = match &resolved {
                Resolved::Shape(shape) => {
                    shape_mesh = MeshData::from_shape(*shape);
                    &shape_mesh
                }
                Resolved::Model(model) => &model.mesh,
            };
            let preset = preset_of(scene, object) as usize;
            if moves(scene, object) {
                let mut local = MeshBuilder::default();
                local.add(mesh, Mat4::IDENTITY, preset);
                let mut sub = audionimbus::Scene::try_new(context).map_err(steam)?;
                if let Some(static_mesh) = local.build(&sub, &materials)? {
                    sub.add_static_mesh(static_mesh);
                }
                sub.commit();
                let placed = transform.matrix();
                let instance = InstancedMesh::try_new(
                    &acoustic,
                    &InstancedMeshSettings {
                        sub_scene: sub,
                        transform: matrix(placed),
                    },
                )
                .map_err(steam)?;
                instances.push((object, acoustic.add_instanced_mesh(instance), placed));
            } else {
                world.add(mesh, transform.matrix(), preset);
            }
        }
        if let Some(static_mesh) = world.build(&acoustic, &materials)? {
            acoustic.add_static_mesh(static_mesh);
        }
        acoustic.commit();
        Ok(Self {
            scene: acoustic,
            instances,
        })
    }

    /// Moves the instanced meshes to where their objects are now. Returns
    /// true if any moved (the scene was committed again).
    pub fn follow(&mut self, scene: &Scene) -> bool {
        let mut moved = false;
        for (object, handle, placed) in &mut self.instances {
            let Some(now) = scene.objects.get(*object).map(|o| o.transform.matrix()) else {
                continue;
            };
            if now != *placed {
                self.scene
                    .update_instanced_mesh_transform(*handle, matrix(now));
                *placed = now;
                moved = true;
            }
        }
        if moved {
            self.scene.commit();
        }
        moved
    }
}

/// A fingerprint of what the acoustic geometry is built from: which objects
/// are solid, their shapes and materials, and where the static ones are.
/// Moving objects' transforms are left out; `follow` handles those.
pub fn signature(scene: &Scene) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (object, geometry, transform, solid) in scene.colliders() {
        if !solid {
            continue;
        }
        object.hash(&mut hasher);
        format!("{geometry:?}").hash(&mut hasher);
        (preset_of(scene, object) as usize).hash(&mut hasher);
        let moving = moves(scene, object);
        moving.hash(&mut hasher);
        if !moving {
            for v in [transform.position, transform.rotation, transform.scale] {
                v.to_array().map(f32::to_bits).hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// Objects whose geometry moves while the game runs.
fn moves(scene: &Scene, object: usize) -> bool {
    scene.objects[object].components.iter().any(|c| {
        matches!(
            c,
            Component::Door(_) | Component::Floating(_) | Component::PhysicsBody(_)
        )
    })
}

fn preset_of(scene: &Scene, object: usize) -> AcousticPreset {
    scene.objects[object]
        .components
        .iter()
        .find_map(|c| match c {
            Component::AcousticMaterial(m) => Some(m.preset),
            _ => None,
        })
        .unwrap_or_default()
}

/// How each preset absorbs (low, mid, high), scatters, and transmits sound.
/// Mostly Steam Audio's published material values; marble, velvet and water
/// are ours.
fn material(preset: AcousticPreset) -> Material {
    let (absorption, scattering, transmission) = match preset {
        AcousticPreset::Generic => ([0.10, 0.20, 0.30], 0.05, [0.100, 0.050, 0.030]),
        AcousticPreset::Wood => ([0.11, 0.07, 0.06], 0.05, [0.070, 0.014, 0.005]),
        AcousticPreset::Stone => ([0.13, 0.20, 0.24], 0.05, [0.015, 0.002, 0.001]),
        AcousticPreset::Marble => ([0.01, 0.01, 0.02], 0.05, [0.015, 0.002, 0.001]),
        AcousticPreset::Brick => ([0.03, 0.04, 0.07], 0.05, [0.015, 0.015, 0.015]),
        AcousticPreset::Concrete => ([0.05, 0.07, 0.08], 0.05, [0.015, 0.002, 0.001]),
        AcousticPreset::Plaster => ([0.12, 0.06, 0.04], 0.05, [0.056, 0.056, 0.004]),
        AcousticPreset::Carpet => ([0.24, 0.69, 0.73], 0.05, [0.020, 0.005, 0.003]),
        AcousticPreset::Velvet => ([0.30, 0.75, 0.80], 0.30, [0.400, 0.200, 0.100]),
        AcousticPreset::Glass => ([0.06, 0.03, 0.02], 0.05, [0.060, 0.044, 0.011]),
        AcousticPreset::Metal => ([0.20, 0.07, 0.06], 0.05, [0.200, 0.025, 0.010]),
        AcousticPreset::Water => ([0.01, 0.01, 0.02], 0.05, [0.020, 0.010, 0.005]),
        AcousticPreset::Gravel => ([0.60, 0.70, 0.80], 0.05, [0.031, 0.012, 0.008]),
    };
    Material {
        absorption,
        scattering,
        transmission,
    }
}

/// Triangles collected from several meshes into one Steam Audio mesh.
#[derive(Default)]
struct MeshBuilder {
    vertices: Vec<Point>,
    triangles: Vec<Triangle>,
    materials: Vec<usize>,
}

impl MeshBuilder {
    fn add(&mut self, mesh: &MeshData, transform: Mat4, material: usize) {
        let base = self.vertices.len() as i32;
        self.vertices.extend(mesh.vertices.iter().map(|v| {
            let p = transform.transform_point3(v.position.into());
            Point::new(p.x, p.y, p.z)
        }));
        for t in mesh.indices.chunks_exact(3) {
            self.triangles.push(Triangle::new(
                base + t[0] as i32,
                base + t[1] as i32,
                base + t[2] as i32,
            ));
            self.materials.push(material);
        }
    }

    fn build(
        &self,
        scene: &audionimbus::Scene,
        materials: &[Material],
    ) -> Result<Option<StaticMesh<DefaultRayTracer>>, AudioError> {
        if self.triangles.is_empty() {
            return Ok(None);
        }
        StaticMesh::try_new(
            scene,
            &StaticMeshSettings {
                vertices: &self.vertices,
                triangles: &self.triangles,
                material_indices: &self.materials,
                materials,
            },
        )
        .map(Some)
        .map_err(|e| AudioError::SteamAudio(e.to_string()))
    }
}

/// glam is column-major; Steam Audio wants rows.
fn matrix(m: Mat4) -> Matrix<f32, 4, 4> {
    Matrix::new(m.transpose().to_cols_array_2d())
}

fn steam(e: audionimbus::SteamAudioError) -> AudioError {
    AudioError::SteamAudio(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::{AcousticMaterial, test_scene};
    use glam::Vec3;

    /// Steam Audio's occlusion of a source 5 m ahead of a listener, with a
    /// wall halfway between them; 1 = clear, 0 = blocked.
    fn occlusion_through_wall(wall_solid: bool) -> f32 {
        use audionimbus::*;
        let mut scene = test_scene(vec![(
            Shape::Box {
                size: Vec3::new(4.0, 4.0, 0.2),
            },
            Vec3::new(0.0, 0.0, -2.5),
        )]);
        if let Component::Primitive(p) = &mut scene.objects[0].components[0] {
            p.collision = wall_solid;
        }
        let context = Context::default();
        let settings = AudioSettings::default();
        let mut simulator = Simulator::try_new(
            &context,
            &SimulationSettings::new(&settings).with_direct(DirectSimulationSettings {
                max_num_occlusion_samples: 16,
            }),
        )
        .unwrap();
        let geometry = AcousticGeometry::build(&context, &scene, &mut Assets::default()).unwrap();
        simulator.set_scene(&geometry.scene);
        let source = Source::try_new(&simulator).unwrap();
        simulator.add_source(&source);
        simulator.commit();
        simulator
            .set_shared_inputs(
                &SimulationSharedInputs::new(CoordinateSystem::default()).with_direct(),
            )
            .unwrap();
        source
            .set_inputs(&SimulationInputs {
                source: CoordinateSystem {
                    origin: Vector3::new(0.0, 0.0, -5.0),
                    ..Default::default()
                },
                parameters: SimulationParameters::new().with_direct(
                    DirectSimulationParameters::new().with_occlusion(Occlusion::new(
                        OcclusionAlgorithm::Volumetric {
                            radius: 0.4,
                            num_occlusion_samples: 16,
                        },
                    )),
                ),
            })
            .unwrap();
        simulator.run_direct();
        source.get_outputs().unwrap().direct().occlusion.unwrap()
    }

    #[test]
    fn walls_occlude_unless_their_collision_is_off() {
        assert!(occlusion_through_wall(true) < 0.1, "behind a wall");
        assert!(occlusion_through_wall(false) > 0.9, "walk-through wall");
    }

    #[test]
    fn presets_index_their_materials() {
        for (i, preset) in PRESETS.iter().enumerate() {
            assert_eq!(*preset as usize, i);
        }
    }

    #[test]
    fn signature_changes_with_static_geometry_but_not_moving_objects() {
        let mut scene = test_scene(vec![
            (Shape::Box { size: Vec3::ONE }, Vec3::ZERO),
            (Shape::Box { size: Vec3::ONE }, Vec3::X * 3.0),
        ]);
        scene.objects[1]
            .components
            .push(Component::Floating(Default::default()));
        let before = signature(&scene);

        scene.objects[1].transform.position.y = 2.0;
        assert_eq!(
            signature(&scene),
            before,
            "moving objects follow on their own"
        );

        scene.objects[0]
            .components
            .push(Component::AcousticMaterial(AcousticMaterial {
                preset: AcousticPreset::Marble,
            }));
        assert_ne!(signature(&scene), before, "a material change rebuilds");
        let with_marble = signature(&scene);
        scene.objects[0].transform.position.x = 1.0;
        assert_ne!(signature(&scene), with_marble, "moving a wall rebuilds");
    }

    #[test]
    fn mesh_builder_transforms_and_tags_triangles() {
        let mut builder = MeshBuilder::default();
        let mesh = MeshData::from_shape(Shape::Box { size: Vec3::ONE });
        builder.add(&mesh, Mat4::from_translation(Vec3::Y * 10.0), 3);
        assert_eq!(builder.triangles.len(), mesh.indices.len() / 3);
        assert!(builder.materials.iter().all(|&m| m == 3));
        assert!(builder.vertices.iter().all(|p| p.y >= 9.4 && p.y <= 10.6));
    }
}
