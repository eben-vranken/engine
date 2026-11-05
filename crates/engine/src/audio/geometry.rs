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
            if !blocks_sound(scene, object, solid) {
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
    /// true if any moved; the scene then needs a commit.
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
        moved
    }
}

/// A fingerprint of what the acoustic geometry is built from: which objects
/// are solid, their shapes and materials, and where the static ones are.
/// Moving objects' transforms are left out; `follow` handles those.
pub fn signature(scene: &Scene) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (object, geometry, transform, solid) in scene.colliders() {
        if !blocks_sound(scene, object, solid) {
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

/// Walk-through objects (collision off) don't block sound, and neither
/// does an object that is itself a sound source: its own shape would muffle
/// it.
fn blocks_sound(scene: &Scene, object: usize, solid: bool) -> bool {
    solid
        && !scene.objects[object]
            .components
            .iter()
            .any(|c| matches!(c, Component::SoundSource(_)))
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
/// Absorption and scattering are mostly Steam Audio's published values
/// (marble, velvet and water are ours). Transmission is tuned for games, not
/// physics: a closed wooden door muffles a sound but it stays audible; thick
/// stone mostly blocks it.
///
/// The transmission values below are what gets through a whole solid object.
/// A ray crosses two of its faces and Steam Audio combines them as t^1.5
/// (measured), so each face is given t^(2/3).
fn material(preset: AcousticPreset) -> Material {
    let (absorption, scattering, through): ([f32; 3], f32, [f32; 3]) = match preset {
        AcousticPreset::Generic => ([0.10, 0.20, 0.30], 0.05, [0.30, 0.15, 0.08]),
        AcousticPreset::Wood => ([0.11, 0.07, 0.06], 0.05, [0.30, 0.15, 0.06]),
        AcousticPreset::Stone => ([0.13, 0.20, 0.24], 0.05, [0.05, 0.02, 0.01]),
        AcousticPreset::Marble => ([0.01, 0.01, 0.02], 0.05, [0.04, 0.015, 0.008]),
        AcousticPreset::Brick => ([0.03, 0.04, 0.07], 0.05, [0.06, 0.03, 0.015]),
        AcousticPreset::Concrete => ([0.05, 0.07, 0.08], 0.05, [0.05, 0.02, 0.01]),
        AcousticPreset::Plaster => ([0.12, 0.06, 0.04], 0.05, [0.20, 0.10, 0.03]),
        AcousticPreset::Carpet => ([0.24, 0.69, 0.73], 0.05, [0.10, 0.05, 0.02]),
        AcousticPreset::Velvet => ([0.30, 0.75, 0.80], 0.30, [0.50, 0.30, 0.15]),
        AcousticPreset::Glass => ([0.06, 0.03, 0.02], 0.05, [0.35, 0.20, 0.08]),
        AcousticPreset::Metal => ([0.20, 0.07, 0.06], 0.05, [0.20, 0.06, 0.02]),
        AcousticPreset::Water => ([0.01, 0.01, 0.02], 0.05, [0.10, 0.05, 0.02]),
        AcousticPreset::Gravel => ([0.60, 0.70, 0.80], 0.05, [0.05, 0.02, 0.01]),
    };
    Material {
        absorption,
        scattering,
        transmission: through.map(|t| t.powf(2.0 / 3.0)),
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
        through_wall(wall_solid, AcousticPreset::Generic).0
    }

    /// Occlusion and mid-band transmission through a wall of `preset`.
    fn through_wall(wall_solid: bool, preset: AcousticPreset) -> (f32, f32) {
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
        scene.objects[0]
            .components
            .push(Component::AcousticMaterial(AcousticMaterial { preset }));
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
                    DirectSimulationParameters::new().with_occlusion(
                        Occlusion::new(OcclusionAlgorithm::Volumetric {
                            radius: 0.4,
                            num_occlusion_samples: 16,
                        })
                        .with_transmission(TransmissionParameters {
                            num_transmission_rays: 3,
                        }),
                    ),
                ),
            })
            .unwrap();
        simulator.run_direct();
        let direct = source.get_outputs().unwrap().direct();
        let transmission = match direct.transmission.unwrap() {
            Transmission::FrequencyDependent(t) | Transmission::FrequencyIndependent(t) => t.0[1],
        };
        (direct.occlusion.unwrap(), transmission)
    }

    #[test]
    fn wooden_walls_muffle_and_stone_mostly_blocks() {
        let (_, wood) = through_wall(true, AcousticPreset::Wood);
        let (_, stone) = through_wall(true, AcousticPreset::Stone);
        assert!(
            (0.1..0.2).contains(&wood),
            "wood lets about 15% of mids through: {wood}"
        );
        assert!(stone < 0.05 && stone < wood, "stone {stone}");
    }

    #[test]
    fn walls_occlude_unless_their_collision_is_off() {
        assert!(occlusion_through_wall(true) < 0.1, "behind a wall");
        assert!(occlusion_through_wall(false) > 0.9, "walk-through wall");
    }

    /// Energy of a click's reflections in a closed 6×4×6 m room made of
    /// `preset`, from 0.25 s after the click on (the reverb tail).
    fn reverb_tail(preset: AcousticPreset) -> f32 {
        use audionimbus::*;
        let room = |size: Vec3, at: Vec3| (Shape::Box { size }, at);
        let mut scene = test_scene(vec![
            room(Vec3::new(6.4, 0.2, 6.4), Vec3::new(0.0, -0.1, 0.0)),
            room(Vec3::new(6.4, 0.2, 6.4), Vec3::new(0.0, 4.1, 0.0)),
            room(Vec3::new(0.2, 4.0, 6.4), Vec3::new(-3.1, 2.0, 0.0)),
            room(Vec3::new(0.2, 4.0, 6.4), Vec3::new(3.1, 2.0, 0.0)),
            room(Vec3::new(6.4, 4.0, 0.2), Vec3::new(0.0, 2.0, -3.1)),
            room(Vec3::new(6.4, 4.0, 0.2), Vec3::new(0.0, 2.0, 3.1)),
        ]);
        for object in &mut scene.objects {
            object
                .components
                .push(Component::AcousticMaterial(AcousticMaterial { preset }));
        }
        let context = Context::default();
        let settings = AudioSettings::default();
        let simulator = Simulator::try_new(
            &context,
            &SimulationSettings::new(&settings).with_reflections(ConvolutionSettings {
                max_num_rays: 4096,
                num_diffuse_samples: 32,
                max_duration: 2.0,
                max_num_sources: 1,
                num_threads: 2,
                max_order: 1,
            }),
        );
        let mut simulator = simulator.unwrap();
        let geometry = AcousticGeometry::build(&context, &scene, &mut Assets::default()).unwrap();
        simulator.set_scene(&geometry.scene);
        let source = Source::<(), Reflections, (), Convolution>::try_new(&simulator).unwrap();
        simulator.add_source(&source);
        simulator.commit();
        let listener = CoordinateSystem {
            origin: Vector3::new(1.0, 1.7, 1.0),
            ..Default::default()
        };
        simulator
            .set_shared_inputs(&SimulationSharedInputs::new(listener).with_reflections(
                ReflectionsSharedInputs {
                    num_rays: 4096,
                    num_bounces: 16,
                    duration: 2.0,
                    order: 1,
                    irradiance_min_distance: 1.0,
                },
            ))
            .unwrap();
        source
            .set_inputs(&SimulationInputs {
                source: CoordinateSystem {
                    origin: Vector3::new(-1.5, 1.5, -1.0),
                    ..Default::default()
                },
                parameters: SimulationParameters::new().with_reflections(ConvolutionParameters {
                    baked_data_identifier: None,
                }),
            })
            .unwrap();
        simulator.run_reflections().unwrap();
        let room_response = source.get_outputs().unwrap().reflections();

        let mut effect = ReflectionEffect::<Convolution>::try_new(
            &context,
            &settings,
            &ReflectionEffectSettings {
                impulse_response_size: 2 * settings.sampling_rate,
                num_channels: 4,
            },
        )
        .unwrap();
        let frame = settings.frame_size as usize;
        let mut input = vec![0.0; frame];
        let mut output = vec![0.0; frame * 4];
        let quarter_second = settings.sampling_rate as usize / 4 / frame;
        let mut tail = 0.0;
        for i in 0..quarter_second * 3 {
            input.fill(0.0);
            if i == 0 {
                input[0] = 1.0;
            }
            let input_buffer = AudioBufferRef::try_from(&input[..]).unwrap();
            let mut output_buffer = AudioBufferMut::try_new(&mut output, 4).unwrap();
            effect
                .apply(&room_response, &input_buffer, &mut output_buffer)
                .unwrap();
            if i >= quarter_second {
                tail += output.iter().map(|s| s * s).sum::<f32>();
            }
        }
        tail
    }

    #[test]
    fn hard_rooms_ring_longer_than_soft_ones() {
        let marble = reverb_tail(AcousticPreset::Marble);
        let carpet = reverb_tail(AcousticPreset::Carpet);
        assert!(marble > 0.0, "a closed marble room rings");
        assert!(marble > carpet * 4.0, "marble {marble} vs carpet {carpet}");
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
