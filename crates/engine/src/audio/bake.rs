//! Baked reverb: acoustic probes placed on the floor inside each zone's
//! volumes, with the room's reverb precomputed at each one. While the game
//! runs, the listener's reverb is read from the probes around it (Steam
//! Audio blends between them) instead of being ray traced live.
//!
//! The bake runs on a background thread. Its result is one file next to
//! the scene (`scenes/hall.ron` → `scenes/hall.reverb`), holding per zone
//! the probe positions and Steam Audio's serialized probe batch.

use super::geometry::{self, AcousticGeometry};
use super::{AudioError, MAX_REVERB, steam};
use crate::assets::Assets;
use crate::scene::{Component, Scene};
use audionimbus::{
    BakedDataIdentifier, BakedDataVariation, Context, ProbeArray, ProbeBatch,
    ProbeGenerationParams, ProgressCallback, ReflectionsBakeFlags, ReflectionsBakeParams,
    ReflectionsBaker, SerializedObject,
};
use glam::{Mat4, Vec3};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// The layer of baked data: reverb, with source and listener at the probe.
pub const REVERB: BakedDataIdentifier = BakedDataIdentifier::Reflections {
    variation: BakedDataVariation::Reverb,
};

/// File format version, at the start of every `.reverb` file.
const MAGIC: &[u8; 8] = b"REVERB01";

#[derive(Debug, thiserror::Error)]
pub enum BakeError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}: not a reverb bake (or an older format); bake again")]
    Format(PathBuf),
    #[error("no zone has a ZoneVolume to put probes in")]
    NoVolumes,
    #[error(transparent)]
    Audio(#[from] AudioError),
}

/// One zone's baked probes.
pub struct BakedZone {
    pub zone: String,
    /// Probe centers, for showing them in the editor.
    pub probes: Vec<Vec3>,
    pub batch: ProbeBatch,
}

pub struct Baked {
    /// What the scene looked like when baked (see [`signature`]).
    pub signature: u64,
    pub zones: Vec<BakedZone>,
}

/// Where a scene's bake lives.
pub fn path_for(scene_file: &Path) -> PathBuf {
    scene_file.with_extension("reverb")
}

/// Fingerprint of everything a bake depends on: the solid geometry and
/// materials, and the zone volumes. A different value means the bake is
/// outdated.
pub fn signature(scene: &Scene) -> u64 {
    let mut hasher = DefaultHasher::new();
    geometry::signature(scene).hash(&mut hasher);
    for (zone, volume) in volumes(scene) {
        zone.hash(&mut hasher);
        for v in volume.transform.to_cols_array() {
            v.to_bits().hash(&mut hasher);
        }
        volume.spacing.to_bits().hash(&mut hasher);
        volume.height.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// How a bake is going.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// The zone being baked, its number (from 1) and the zone count.
    pub zone: String,
    pub index: usize,
    pub count: usize,
    /// Of the current zone, 0–1.
    pub fraction: f32,
}

/// A bake running in the background.
pub struct BakeJob {
    progress: Arc<Mutex<Progress>>,
    thread: Option<JoinHandle<Result<PathBuf, BakeError>>>,
}

impl BakeJob {
    pub fn progress(&self) -> Progress {
        self.progress.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// The result once the bake is done (the file written); `None` while
    /// it runs. Returns it only once.
    pub fn finished(&mut self) -> Option<Result<PathBuf, BakeError>> {
        if !self.thread.as_ref()?.is_finished() {
            return None;
        }
        let result = self.thread.take()?.join().unwrap_or_else(|_| {
            Err(BakeError::Audio(AudioError::SteamAudio(
                "the bake crashed".into(),
            )))
        });
        Some(result)
    }
}

/// A zone volume as Steam Audio wants it: the unit cube 0..1 turned into
/// the volume, plus probe spacing and height above the floor.
struct ProbeVolume {
    transform: Mat4,
    spacing: f32,
    height: f32,
}

/// Every zone volume with its zone's name, in scene order.
fn volumes(scene: &Scene) -> Vec<(String, ProbeVolume)> {
    let mut out = Vec::new();
    for object in &scene.objects {
        for component in &object.components {
            if let Component::ZoneVolume(volume) = component {
                // Steam Audio starts its grid at the box's corner, which puts
                // a row of probes against the walls. Shrinking the box by
                // half a spacing on each side centres the grid in the room.
                let spacing = volume.probe_spacing.max(0.25);
                let extent = (volume.size * object.transform.scale).abs();
                let inset = Vec3::new(
                    (1.0 - spacing / extent.x.max(1e-3)).max(1e-3),
                    1.0,
                    (1.0 - spacing / extent.z.max(1e-3)).max(1e-3),
                );
                let transform = object.transform.matrix()
                    * Mat4::from_scale(volume.size * inset)
                    * Mat4::from_translation(Vec3::splat(-0.5));
                out.push((
                    object.zone.clone(),
                    ProbeVolume {
                        transform,
                        spacing,
                        height: volume.probe_height.max(0.0),
                    },
                ));
            }
        }
    }
    out
}

/// Starts baking `scene` (as it is now) into `file` on a background thread.
pub fn start(
    context: &Context,
    scene: &Scene,
    assets: &mut Assets,
    file: PathBuf,
) -> Result<BakeJob, BakeError> {
    let mut zones: Vec<(String, Vec<ProbeVolume>)> = Vec::new();
    for (zone, volume) in volumes(scene) {
        match zones.iter_mut().find(|(name, _)| *name == zone) {
            Some((_, list)) => list.push(volume),
            None => zones.push((zone, vec![volume])),
        }
    }
    if zones.is_empty() {
        return Err(BakeError::NoVolumes);
    }
    // Geometry as placed in the editor: doors closed, bodies where they start.
    let geometry = AcousticGeometry::build(context, scene, assets)?;
    let signature = signature(scene);
    let progress = Arc::new(Mutex::new(Progress {
        count: zones.len(),
        ..Default::default()
    }));
    let context = context.clone();
    let shared = progress.clone();
    let thread = std::thread::Builder::new()
        .name("reverb bake".into())
        .spawn(move || {
            let mut baked = Vec::new();
            for (index, (zone, volumes)) in zones.into_iter().enumerate() {
                if let Ok(mut p) = shared.lock() {
                    *p = Progress {
                        zone: zone.clone(),
                        index: index + 1,
                        count: p.count,
                        fraction: 0.0,
                    };
                }
                if let Some(zone) = bake_zone(&context, &geometry, zone, &volumes, &shared)? {
                    baked.push(zone);
                }
            }
            write(&context, &file, signature, &baked)?;
            Ok(file)
        })
        .map_err(|source| BakeError::Io {
            path: PathBuf::from("reverb bake thread"),
            source,
        })?;
    Ok(BakeJob {
        progress,
        thread: Some(thread),
    })
}

/// Places one zone's probes and bakes them. `None` if no probe found a
/// floor to stand on.
fn bake_zone(
    context: &Context,
    geometry: &AcousticGeometry,
    zone: String,
    volumes: &[ProbeVolume],
    progress: &Arc<Mutex<Progress>>,
) -> Result<Option<BakedZone>, BakeError> {
    let mut batch = ProbeBatch::try_new(context).map_err(steam)?;
    let mut probes = Vec::new();
    for volume in volumes {
        let mut array = ProbeArray::try_new(context).map_err(steam)?;
        array.generate_probes(
            &geometry.scene,
            &ProbeGenerationParams::UniformFloor {
                spacing: volume.spacing,
                height: volume.height,
                // Unlike instanced mesh transforms (row-major, see
                // `geometry::matrix`), probe generation reads its box
                // transposed: found by test, see the bake test below.
                transform: audionimbus::Matrix::new(volume.transform.to_cols_array_2d()),
            },
        );
        probes.extend(
            array
                .probes()
                .map(|s| Vec3::new(s.center.x, s.center.y, s.center.z)),
        );
        batch.add_probe_array(&array);
    }
    if probes.is_empty() {
        log::warn!("zone \"{zone}\": no floor inside its volumes, no probes");
        return Ok(None);
    }
    batch.commit();
    let shared = progress.clone();
    let callback = ProgressCallback::new(move |fraction| {
        if let Ok(mut p) = shared.lock() {
            p.fraction = fraction;
        }
    });
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get() as u32);
    ReflectionsBaker::<audionimbus::DefaultRayTracer>::new()
        .bake_with_progress_callback(
            context,
            &mut batch,
            &geometry.scene,
            ReflectionsBakeParams {
                identifier: REVERB,
                bake_flags: ReflectionsBakeFlags::BAKE_CONVOLUTION,
                num_rays: 8192,
                num_diffuse_samples: 256,
                num_bounces: 32,
                simulated_duration: MAX_REVERB,
                saved_duration: MAX_REVERB,
                order: 1,
                num_threads: threads,
                irradiance_min_distance: 1.0,
                bake_batch_size: 1,
            },
            callback,
        )
        .map_err(|e| AudioError::SteamAudio(e.to_string()))?;
    log::info!("baked zone \"{zone}\": {} probes", probes.len());
    Ok(Some(BakedZone {
        zone,
        probes,
        batch,
    }))
}

/// Writes the bake: magic, signature, zone count, then per zone its name,
/// probe positions and serialized probe batch. Little-endian throughout.
fn write(
    context: &Context,
    path: &Path,
    signature: u64,
    zones: &[BakedZone],
) -> Result<(), BakeError> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&signature.to_le_bytes());
    out.extend_from_slice(&(zones.len() as u32).to_le_bytes());
    for zone in zones {
        let data = zone.batch.try_save(context).map_err(steam)?.to_vec();
        out.extend_from_slice(&(zone.zone.len() as u32).to_le_bytes());
        out.extend_from_slice(zone.zone.as_bytes());
        out.extend_from_slice(&(zone.probes.len() as u32).to_le_bytes());
        for p in &zone.probes {
            for v in p.to_array() {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(&data);
    }
    std::fs::write(path, out).map_err(|source| BakeError::Io {
        path: path.to_owned(),
        source,
    })
}

/// Reads a bake written by [`write`]. `Ok(None)` if there is no file.
pub fn load(context: &Context, path: &Path) -> Result<Option<Baked>, BakeError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(BakeError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };
    let format = || BakeError::Format(path.to_owned());
    let mut reader = Reader(&bytes);
    if reader.take(MAGIC.len()).ok_or_else(format)? != MAGIC {
        return Err(format());
    }
    let signature = reader.u64().ok_or_else(format)?;
    let count = reader.u32().ok_or_else(format)?;
    let mut zones = Vec::new();
    for _ in 0..count {
        let name_len = reader.u32().ok_or_else(format)? as usize;
        let zone = String::from_utf8(reader.take(name_len).ok_or_else(format)?.to_vec())
            .map_err(|_| format())?;
        let probe_count = reader.u32().ok_or_else(format)?;
        let mut probes = Vec::new();
        for _ in 0..probe_count {
            let mut p = [0.0; 3];
            for v in &mut p {
                *v = f32::from_bits(reader.u32().ok_or_else(format)?);
            }
            probes.push(Vec3::from(p));
        }
        let data_len = reader.u64().ok_or_else(format)? as usize;
        let data = reader.take(data_len).ok_or_else(format)?.to_vec();
        let mut serialized = SerializedObject::try_with_buffer(context, data).map_err(steam)?;
        let batch = ProbeBatch::load(context, &mut serialized).map_err(steam)?;
        // Loading doesn't commit, and an uncommitted batch crashes the first
        // baked reflection pass (as in Valve's Unity plugin, commit after load).
        batch.commit();
        zones.push(BakedZone {
            zone,
            probes,
            batch,
        });
    }
    Ok(Some(Baked { signature, zones }))
}

/// Reads little-endian values off the front of a byte slice.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::{Object, Transform, ZoneVolume, test_scene};

    /// A closed 4 × 3 × 4 m room with a zone volume filling it.
    fn room() -> Scene {
        let slab = |size: Vec3, at: Vec3| (Shape::Box { size }, at);
        let mut scene = test_scene(vec![
            slab(Vec3::new(4.4, 0.2, 4.4), Vec3::new(0.0, -0.1, 0.0)),
            slab(Vec3::new(4.4, 0.2, 4.4), Vec3::new(0.0, 3.1, 0.0)),
            slab(Vec3::new(0.2, 3.0, 4.4), Vec3::new(-2.1, 1.5, 0.0)),
            slab(Vec3::new(0.2, 3.0, 4.4), Vec3::new(2.1, 1.5, 0.0)),
            slab(Vec3::new(4.4, 3.0, 0.2), Vec3::new(0.0, 1.5, -2.1)),
            slab(Vec3::new(4.4, 3.0, 0.2), Vec3::new(0.0, 1.5, 2.1)),
        ]);
        scene.objects.push(Object {
            name: "volume".into(),
            zone: "test".into(),
            transform: Transform::at(Vec3::new(0.0, 1.5, 0.0)),
            components: vec![Component::ZoneVolume(ZoneVolume {
                size: Vec3::new(4.0, 3.0, 4.0),
                ..Default::default()
            })],
        });
        scene
    }

    #[test]
    fn a_room_bakes_to_a_file_that_loads_back() {
        let context = Context::default();
        let scene = room();
        let file = std::env::temp_dir().join(format!("bake-test-{}.reverb", std::process::id()));
        let mut job = start(&context, &scene, &mut Assets::default(), file.clone()).unwrap();
        let written = loop {
            if let Some(result) = job.finished() {
                break result.unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(written, file);

        let baked = load(&context, &file).unwrap().expect("the file exists");
        std::fs::remove_file(&file).ok();
        assert_eq!(baked.signature, signature(&scene));
        assert_eq!(baked.zones.len(), 1);
        let zone = &baked.zones[0];
        assert_eq!(zone.zone, "test");
        // A 2 m grid centred in the 4 m room, 1.5 m above the floor.
        assert_eq!(zone.probes.len(), 4);
        for probe in &zone.probes {
            let expected = Vec3::new(probe.x.signum(), 1.5, probe.z.signum());
            assert!((*probe - expected).length() < 0.01, "probe at {probe}");
        }
        assert_eq!(zone.batch.num_probes(), zone.probes.len());

        // The game's use: a reflection source at the listener, reading the
        // baked reverb.
        let reverb = simulate_baked(&context, &scene, &baked);
        assert!(reverb, "a baked response at a probe");
    }

    /// Runs one reflection pass the way `Audio` does, with the listener's
    /// reverb read from `baked`. True if the source got a response.
    fn simulate_baked(context: &Context, scene: &Scene, baked: &Baked) -> bool {
        use audionimbus::*;
        let settings = AudioSettings {
            sampling_rate: 48_000,
            frame_size: 512,
        };
        let geometry = AcousticGeometry::build(context, scene, &mut Assets::default()).unwrap();
        let mut simulator: Simulator<DefaultRayTracer, (), Reflections, (), Convolution> =
            Simulator::try_new(
                context,
                &SimulationSettings::new(&settings).with_reflections(ConvolutionSettings {
                    max_num_rays: 4096,
                    num_diffuse_samples: 32,
                    max_duration: MAX_REVERB,
                    max_num_sources: 16,
                    num_threads: 2,
                    max_order: 1,
                }),
            )
            .unwrap();
        simulator.set_scene(&geometry.scene);
        for zone in &baked.zones {
            simulator.add_probe_batch(&zone.batch);
        }
        let source = Source::<(), Reflections, (), Convolution>::try_new(&simulator).unwrap();
        simulator.add_source(&source);
        simulator.commit();
        let listener = CoordinateSystem {
            origin: Vector3::new(1.0, 1.5, 1.0),
            ..Default::default()
        };
        simulator
            .set_shared_inputs(&SimulationSharedInputs::new(listener).with_reflections(
                ReflectionsSharedInputs {
                    num_rays: 4096,
                    num_bounces: 16,
                    duration: MAX_REVERB,
                    order: 1,
                    irradiance_min_distance: 1.0,
                },
            ))
            .unwrap();
        source
            .set_inputs(&super::super::reflection_inputs(
                Vec3::new(1.0, 1.5, 1.0),
                true,
            ))
            .unwrap();
        simulator.run_reflections().unwrap();
        source.get_outputs().is_ok()
    }

    #[test]
    fn moving_a_volume_outdates_the_bake_and_no_file_is_no_bake() {
        let mut scene = room();
        let before = signature(&scene);
        scene.objects.last_mut().unwrap().transform.position.x += 1.0;
        assert_ne!(signature(&scene), before);
        let missing = Path::new("no/such/scene.reverb");
        assert!(load(&Context::default(), missing).unwrap().is_none());
    }

    #[test]
    fn without_volumes_there_is_nothing_to_bake() {
        let scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::ZERO)]);
        let result = start(
            &Context::default(),
            &scene,
            &mut Assets::default(),
            PathBuf::from("unused.reverb"),
        );
        assert!(matches!(result, Err(BakeError::NoVolumes)));
    }
}
