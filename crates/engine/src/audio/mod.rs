//! Audio: Steam Audio (through AudioNimbus) for spatial sound and room
//! acoustics, cpal for the output device. The listener is whatever camera
//! is active, so the editor previews sound too.
//!
//! - Direct sound (distance, air absorption, occlusion, transmission) is
//!   simulated on the main thread every frame; it's cheap.
//! - Reflections (echoes, reverb) are simulated on a background thread
//!   (AudioNimbus' `wiring`), paused while nothing plays.
//! - The mixer on the audio thread (`mixer.rs`) renders both.
//!
//! Sounds come from `SoundSource` components, plus one-shots (claps,
//! footsteps) played from a small pool of voices.

mod clip;
mod geometry;
mod mixer;

use crate::assets::Assets;
use crate::camera::Camera;
use crate::scene::{AcousticPreset, Component, Scene};
use audionimbus::wiring::{ReflectionsFrame, ReflectionsSimulation, Simulation, SourceWithInputs};
use audionimbus::{
    AirAbsorptionModel, AmbisonicsDecodeEffect, AmbisonicsDecodeEffectSettings, AudioSettings,
    BinauralEffect, BinauralEffectSettings, Context, Convolution, ConvolutionParameters,
    ConvolutionSettings, CoordinateSystem, DefaultRayTracer, Direct, DirectEffect,
    DirectEffectSettings, DirectSimulationParameters, DirectSimulationSettings,
    DistanceAttenuationModel, Hrtf, HrtfSettings, Occlusion, OcclusionAlgorithm, PanningEffect,
    PanningEffectSettings, ReflectionEffect, ReflectionEffectSettings, Reflections,
    ReflectionsSharedInputs, Rendering, SimulationInputs, SimulationParameters, SimulationSettings,
    SimulationSharedInputs, Simulator, Source, SpeakerLayout, Transmission, TransmissionParameters,
    Vector3,
};
use clip::Clip;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use geometry::AcousticGeometry;
use glam::Vec3;
use mixer::{AMBISONIC_CHANNELS, Command, Decoders, Mixer, Shared, Voice, VoiceParams};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Samples per channel processed at a time (21 ms at 48 kHz).
const FRAME_SIZE: u32 = 1024;
/// How often edits to static geometry are picked up (rebuilding is cheap,
/// but not every frame of a drag).
const GEOMETRY_CHECK: Duration = Duration::from_millis(250);
/// Sources are spheres this big for occlusion, so sound fades in and out
/// around edges instead of switching (meters).
const SOURCE_RADIUS: f32 = 0.4;
/// Longest reverb tail simulated (seconds); enough for a large stone hall.
const MAX_REVERB: f32 = 3.0;
/// Voices kept ready for one-shot sounds (claps, footsteps).
const ONE_SHOT_VOICES: usize = 6;

type AcousticSimulator = Simulator<DefaultRayTracer, Direct, Reflections, (), Convolution>;
type AcousticSource = Source<Direct, Reflections, (), Convolution>;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio output device")]
    NoDevice,
    #[error("audio output: {0}")]
    Device(String),
    #[error("Steam Audio: {0}")]
    SteamAudio(String),
}

/// Parts of the simulation that can be switched off to hear what they do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Toggles {
    pub occlusion: bool,
    pub reflections: bool,
    pub hrtf: bool,
    pub air_absorption: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            occlusion: true,
            reflections: true,
            hrtf: true,
            air_absorption: true,
        }
    }
}

pub struct Audio {
    _stream: cpal::Stream,
    rate: u32,
    context: Context,
    settings: AudioSettings,
    hrtf: Hrtf,
    simulator: AcousticSimulator,
    simulation: Simulation<u32, DefaultRayTracer, Direct, Reflections, (), Convolution>,
    reflections: ReflectionsSimulation<u32, Direct, (), Convolution>,
    reflections_paused: bool,
    geometry: AcousticGeometry,
    /// Fingerprint the geometry was built from, and when it was last checked.
    geometry_signature: u64,
    geometry_checked: Instant,
    voices: Vec<Entry>,
    next_id: u32,
    commands: Sender<Command>,
    shared: Arc<Mutex<Shared>>,
    /// Loaded clips by path; `None` = no file (or it failed, reported once).
    clips: HashMap<String, Option<Clip>>,
    project_dir: PathBuf,
    footstep_variant: u32,
    pub toggles: Toggles,
}

struct Entry {
    id: u32,
    kind: Kind,
    source: AcousticSource,
    position: Vec3,
    volume: f32,
    looping: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// A `SoundSource` on a scene object.
    Scene { object: usize, clip: String },
    /// From the one-shot pool; busy (simulated) until the given time.
    OneShot { until: Option<Instant> },
}

impl Audio {
    pub fn new(project_dir: PathBuf) -> Result<Self, AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(AudioError::NoDevice)?;
        let config = device
            .default_output_config()
            .map_err(|e| AudioError::Device(e.to_string()))?;
        let rate = config.sample_rate();
        let channels = config.channels() as usize;

        let context = Context::default();
        let settings = AudioSettings {
            sampling_rate: rate,
            frame_size: FRAME_SIZE,
        };
        let hrtf = Hrtf::try_new(&context, &settings, &HrtfSettings::default()).map_err(steam)?;
        let simulation_settings = SimulationSettings::new(&settings)
            .with_direct(DirectSimulationSettings {
                max_num_occlusion_samples: 16,
            })
            .with_reflections(ConvolutionSettings {
                max_num_rays: 4096,
                num_diffuse_samples: 32,
                max_duration: MAX_REVERB,
                max_num_sources: 16,
                num_threads: 2,
                max_order: 1,
            });
        let mut simulator = Simulator::try_new(&context, &simulation_settings).map_err(steam)?;
        // Empty until the first update brings the scene's geometry.
        let geometry =
            AcousticGeometry::build(&context, &Scene::default(), &mut Assets::default())?;
        simulator.set_scene(&geometry.scene);
        simulator.commit();
        let mut simulation = Simulation::new::<u32>(simulator.clone());
        let reflections =
            simulation.spawn_reflections(|err| log::error!("audio reflections: {err}"));
        reflections.pause();

        let decoder = |rendering| {
            AmbisonicsDecodeEffect::try_new(
                &context,
                &settings,
                &AmbisonicsDecodeEffectSettings {
                    speaker_layout: SpeakerLayout::Stereo,
                    hrtf: hrtf.clone(),
                    max_order: 1,
                    rendering,
                },
            )
            .map_err(steam)
        };
        let decoders = Decoders {
            binaural: decoder(Rendering::Binaural)?,
            panning: decoder(Rendering::Panning)?,
        };
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (commands, receive_commands) = channel();
        let mut mixer = Mixer::new(
            hrtf.clone(),
            FRAME_SIZE as usize,
            rate,
            receive_commands,
            shared.clone(),
            reflections.output(),
            decoders,
        );
        let stream = device
            .build_output_stream::<f32, _, _>(
                config.config(),
                move |out, _| mixer.fill(out, channels),
                |err| log::error!("audio output: {err}"),
                None,
            )
            .map_err(|e| AudioError::Device(e.to_string()))?;
        stream
            .play()
            .map_err(|e| AudioError::Device(e.to_string()))?;
        log::info!("audio: {rate} Hz, {channels} channels");

        let mut audio = Self {
            _stream: stream,
            rate,
            context,
            settings,
            hrtf,
            simulator,
            simulation,
            reflections,
            reflections_paused: true,
            geometry,
            geometry_signature: geometry::signature(&Scene::default()),
            geometry_checked: Instant::now(),
            voices: Vec::new(),
            next_id: 0,
            commands,
            shared,
            clips: HashMap::new(),
            project_dir,
            footstep_variant: 0,
            toggles: Toggles::default(),
        };
        for _ in 0..ONE_SHOT_VOICES {
            audio.add_voice(Kind::OneShot { until: None }, None);
        }
        Ok(audio)
    }

    /// Call every frame: follows the scene's geometry and sound sources and
    /// the listener, and runs the direct simulation.
    pub fn update(&mut self, scene: &Scene, assets: &mut Assets, listener: &Camera) {
        self.update_geometry(scene, assets);
        self.sync_scene_voices(scene);

        // Which voices are simulated: scene sources, and one-shots still
        // ringing out.
        let now = Instant::now();
        for entry in &mut self.voices {
            if let Kind::OneShot { until } = &mut entry.kind
                && until.is_some_and(|t| now >= t)
            {
                *until = None;
            }
        }
        let active: Vec<&Entry> = self
            .voices
            .iter()
            .filter(|e| !matches!(e.kind, Kind::OneShot { until: None }))
            .collect();

        let right = listener.right();
        let ahead = listener.forward();
        let up = right.cross(ahead);
        let listener_frame = CoordinateSystem {
            right: vector(right),
            up: vector(up),
            ahead: vector(ahead),
            origin: vector(listener.position),
        };
        let shared_inputs = SimulationSharedInputs::new(listener_frame)
            .with_direct()
            .with_reflections(ReflectionsSharedInputs {
                num_rays: 4096,
                num_bounces: 16,
                duration: MAX_REVERB,
                order: 1,
                irradiance_min_distance: 1.0,
            });
        // Direct-only calls: they don't wait for a reflection pass in progress.
        if let Err(err) = self
            .simulator
            .set_shared_inputs_subset::<Direct, (), (), _, _, _>(&shared_inputs)
        {
            log::error!("audio listener: {err}");
            return;
        }
        for entry in &active {
            if let Err(err) = entry
                .source
                .set_inputs_subset::<Direct, (), (), _, _, _>(&inputs(entry.position))
            {
                log::error!("audio source: {err}");
            }
        }
        self.simulator.run_direct();

        // Reflections run on their own thread from the latest snapshot.
        self.simulation.update_sources(|sources| {
            for entry in &active {
                sources.push((
                    entry.id,
                    SourceWithInputs {
                        source: entry.source.clone(),
                        simulation_inputs: inputs(entry.position),
                    },
                ));
            }
        });
        self.reflections.set_input(ReflectionsFrame {
            sources: self.simulation.sources.clone(),
            shared_inputs,
        });
        let idle = active.is_empty();
        if idle != self.reflections_paused {
            if idle {
                self.reflections.pause();
            } else {
                self.reflections.resume();
            }
            self.reflections_paused = idle;
        }

        let voices = active
            .iter()
            .map(|entry| {
                let toward = (entry.position - listener.position).normalize_or(ahead);
                let mut params = VoiceParams {
                    volume: entry.volume,
                    looping: entry.looping,
                    direction: [toward.dot(right), toward.dot(up), -toward.dot(ahead)],
                    ..Default::default()
                };
                if let Ok(outputs) = entry.source.get_outputs_subset::<Direct, (), ()>() {
                    let direct = outputs.direct();
                    params.distance_attenuation = direct.distance_attenuation.unwrap_or(1.0);
                    if let Some(air) = direct.air_absorption {
                        params.air_absorption = air.0;
                    }
                    params.occlusion = direct.occlusion.unwrap_or(1.0);
                    if let Some(
                        Transmission::FrequencyDependent(t) | Transmission::FrequencyIndependent(t),
                    ) = direct.transmission
                    {
                        params.transmission = t.0;
                    }
                }
                (entry.id, params)
            })
            .collect();
        if let Ok(mut shared) = self.shared.lock() {
            shared.voices = voices;
            shared.listener = listener_frame;
            shared.toggles = self.toggles;
        }
    }

    /// A hand clap at `position` (usually the listener).
    pub fn clap(&mut self, position: Vec3) {
        let rate = self.rate;
        let clip = self.clip_or("audio/clap.wav", || clip::clap(rate));
        self.play(clip, position, 1.0);
    }

    /// A footstep on a surface of `material` at `position`. A project can
    /// supply its own as `audio/footsteps/<material>.wav`.
    pub fn footstep(&mut self, material: AcousticPreset, position: Vec3) {
        let name = format!("{material:?}").to_lowercase();
        let rate = self.rate;
        self.footstep_variant = (self.footstep_variant + 1) % 4;
        let variant = self.footstep_variant;
        let clip = self.clip_or(&format!("audio/footsteps/{name}.wav"), || {
            clip::footstep(&name, variant, rate)
        });
        self.play(clip, position, 0.5);
    }

    /// Plays a clip once from the one-shot pool: a free voice, or the one
    /// that has been busy longest.
    fn play(&mut self, clip: Clip, position: Vec3, volume: f32) {
        let now = Instant::now();
        let ring = Duration::from_secs_f32(clip.len() as f32 / self.rate as f32 + MAX_REVERB);
        let Some(entry) = self
            .voices
            .iter_mut()
            .filter_map(|e| match e.kind {
                Kind::OneShot { until } => Some((until, e)),
                Kind::Scene { .. } => None,
            })
            .min_by_key(|(until, _)| *until)
            .map(|(_, e)| e)
        else {
            return;
        };
        entry.kind = Kind::OneShot {
            until: Some(now + ring),
        };
        entry.position = position;
        entry.volume = volume;
        let _ = self.commands.send(Command::Play { id: entry.id, clip });
    }

    /// Adds and removes voices to match the scene's `SoundSource`s. Voices
    /// whose object and clip stay the same keep playing undisturbed.
    fn sync_scene_voices(&mut self, scene: &Scene) {
        let mut wanted: Vec<(usize, &crate::scene::SoundSource, Vec3)> = Vec::new();
        for (object, o) in scene.objects.iter().enumerate() {
            for component in &o.components {
                if let Component::SoundSource(sound) = component {
                    wanted.push((object, sound, o.transform.position));
                }
            }
        }
        let mut changed = false;
        let gone: Vec<u32> = self
            .voices
            .iter()
            .filter(|e| match &e.kind {
                Kind::Scene { object, clip } => !wanted
                    .iter()
                    .any(|(o, s, _)| o == object && &s.clip == clip),
                Kind::OneShot { .. } => false,
            })
            .map(|e| e.id)
            .collect();
        for id in gone {
            if let Some(i) = self.voices.iter().position(|e| e.id == id) {
                let entry = self.voices.remove(i);
                self.simulator.remove_source(&entry.source);
                let _ = self.commands.send(Command::Remove(id));
                changed = true;
            }
        }
        for (object, sound, position) in wanted {
            let kind = Kind::Scene {
                object,
                clip: sound.clip.clone(),
            };
            let existing = self.voices.iter_mut().find(|e| e.kind == kind);
            let entry = match existing {
                Some(entry) => entry,
                None => {
                    let clip = self.scene_clip(&sound.clip);
                    if !self.add_voice(kind, clip) {
                        continue;
                    }
                    changed = true;
                    self.voices.last_mut().expect("just added")
                }
            };
            entry.position = position;
            entry.volume = sound.volume;
            entry.looping = sound.looping;
        }
        if changed {
            self.simulation.request_simulator_commit();
        }
    }

    /// Creates a voice: a simulated source and the mixer's effects.
    fn add_voice(&mut self, kind: Kind, clip: Option<Clip>) -> bool {
        let source = match AcousticSource::try_new(&self.simulator) {
            Ok(source) => source,
            Err(err) => {
                log::error!("audio source: {err}");
                return false;
            }
        };
        let voice = match self.effects(self.next_id, clip) {
            Ok(voice) => voice,
            Err(err) => {
                log::error!("audio: {err}");
                return false;
            }
        };
        self.simulator.add_source(&source);
        let _ = self.commands.send(Command::Add(voice));
        self.voices.push(Entry {
            id: self.next_id,
            kind,
            source,
            position: Vec3::ZERO,
            volume: 1.0,
            looping: false,
        });
        self.next_id += 1;
        true
    }

    fn effects(&self, id: u32, clip: Option<Clip>) -> Result<Voice, AudioError> {
        let (context, settings) = (&self.context, &self.settings);
        Ok(Voice {
            id,
            clip,
            position: 0,
            direct: DirectEffect::try_new(
                context,
                settings,
                &DirectEffectSettings { num_channels: 1 },
            )
            .map_err(steam)?,
            binaural: BinauralEffect::try_new(
                context,
                settings,
                &BinauralEffectSettings {
                    hrtf: self.hrtf.clone(),
                },
            )
            .map_err(steam)?,
            panning: PanningEffect::try_new(
                context,
                settings,
                &PanningEffectSettings {
                    speaker_layout: SpeakerLayout::Stereo,
                },
            )
            .map_err(steam)?,
            reflection: ReflectionEffect::try_new(
                context,
                settings,
                &ReflectionEffectSettings {
                    impulse_response_size: (MAX_REVERB * self.rate as f32) as u32,
                    num_channels: AMBISONIC_CHANNELS as u32,
                },
            )
            .map_err(steam)?,
        })
    }

    /// Rebuilds the acoustic geometry when static objects were edited, and
    /// moves the moving ones every frame. Commits happen on the simulation
    /// thread, so the main thread never waits for a reflection pass.
    fn update_geometry(&mut self, scene: &Scene, assets: &mut Assets) {
        if self.geometry_checked.elapsed() >= GEOMETRY_CHECK {
            self.geometry_checked = Instant::now();
            let signature = geometry::signature(scene);
            if signature != self.geometry_signature {
                match AcousticGeometry::build(&self.context, scene, assets) {
                    Ok(geometry) => {
                        self.geometry = geometry;
                        self.simulator.set_scene(&self.geometry.scene);
                        self.simulation.request_simulator_commit();
                        // Direct sound runs here, so it needs the commit now.
                        self.simulator.commit();
                        self.geometry_signature = signature;
                    }
                    Err(err) => log::error!("acoustic geometry: {err}"),
                }
                return;
            }
        }
        if self.geometry.follow(scene) {
            self.simulation
                .request_scene_commits(std::slice::from_ref(&self.geometry.scene));
        }
    }

    /// A `SoundSource`'s clip; empty plays the built-in plucked phrase.
    fn scene_clip(&mut self, path: &str) -> Option<Clip> {
        if path.is_empty() {
            let rate = self.rate;
            return Some(self.clip_or("", || clip::plucked_loop(rate)));
        }
        self.load(path)
    }

    /// The project's WAV at `path` if there is one, else `fallback`.
    fn clip_or(&mut self, path: &str, fallback: impl FnOnce() -> Clip) -> Clip {
        if path.is_empty() || !self.project_dir.join(path).is_file() {
            return fallback();
        }
        self.load(path).unwrap_or_else(fallback)
    }

    fn load(&mut self, path: &str) -> Option<Clip> {
        let (rate, dir) = (self.rate, &self.project_dir);
        self.clips
            .entry(path.to_owned())
            .or_insert_with(|| {
                clip::load_wav(&dir.join(path), rate)
                    .inspect_err(|e| log::error!("{e}"))
                    .ok()
            })
            .clone()
    }
}

/// What each source is simulated for.
fn inputs(position: Vec3) -> SimulationInputs<Direct, Reflections, ()> {
    SimulationInputs {
        source: CoordinateSystem {
            origin: vector(position),
            ..Default::default()
        },
        parameters: SimulationParameters::new()
            .with_direct(
                DirectSimulationParameters::new()
                    .with_distance_attenuation(DistanceAttenuationModel::default())
                    .with_air_absorption(AirAbsorptionModel::default())
                    .with_occlusion(
                        Occlusion::new(OcclusionAlgorithm::Volumetric {
                            radius: SOURCE_RADIUS,
                            num_occlusion_samples: 16,
                        })
                        .with_transmission(TransmissionParameters {
                            num_transmission_rays: 3,
                        }),
                    ),
            )
            .with_reflections(ConvolutionParameters {
                baked_data_identifier: None,
            }),
    }
}

fn vector(v: Vec3) -> Vector3 {
    Vector3::new(v.x, v.y, v.z)
}

fn steam(e: audionimbus::SteamAudioError) -> AudioError {
    AudioError::SteamAudio(e.to_string())
}
