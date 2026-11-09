//! Audio: Steam Audio (through AudioNimbus) for spatial sound and room
//! acoustics, cpal for the output device. The listener is whatever camera
//! is active, so the editor previews sound too.
//!
//! Two simulators, each with its own copy of the acoustic scene:
//! - direct sound (distance, air absorption, occlusion, transmission) runs on
//!   the main thread every frame; its scene follows moving doors at once;
//! - reflections (echoes, reverb) run on a background thread (AudioNimbus'
//!   `wiring`), paused while nothing plays; its scene catches up between
//!   passes.
//!
//! The mixer on the audio thread (`mixer.rs`) renders both. Sounds come from
//! `SoundSource` components, plus one-shots (claps, footsteps) played from a
//! small pool of voices.

pub mod bake;
mod clip;
mod geometry;
mod mixer;
mod steps;
mod timing;

use crate::assets::Assets;
use crate::camera::Camera;
use crate::perf;
use crate::player::settings::{Footsteps, RandomOrder};
use crate::scene::{AcousticPreset, Component, Scene, SoundSource};
use crate::zones::Active;
use audionimbus::wiring::{ReflectionsFrame, ReflectionsSimulation, Simulation, SourceWithInputs};
use audionimbus::{
    AirAbsorptionModel, AmbisonicsDecodeEffect, AmbisonicsDecodeEffectSettings, AudioSettings,
    BinauralEffect, BinauralEffectSettings, Context, Convolution, ConvolutionParameters,
    ConvolutionSettings, CoordinateSystem, DefaultRayTracer, Direct, DirectEffect,
    DirectEffectSettings, DirectSimulationParameters, DirectSimulationSettings,
    DistanceAttenuationModel, Hrtf, HrtfSettings, Occlusion, OcclusionAlgorithm, PanningEffect,
    PanningEffectSettings, Parametric, ReflectionEffect, ReflectionEffectSettings, Reflections,
    ReflectionsSharedInputs, Rendering, SimulationInputs, SimulationParameters, SimulationSettings,
    SimulationSharedInputs, Simulator, Source, SpeakerLayout, Transmission, TransmissionParameters,
    Vector3,
};
use clip::Clip;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use geometry::AcousticGeometry;
use glam::Vec3;
use mixer::{
    AMBISONIC_CHANNELS, BusEffects, Command, LISTENER_REVERB, Mixer, Shared, Stylized, Voice,
    VoiceParams,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
pub use timing::Report as TimingReport;
use timing::Timing;

/// Samples per channel processed at a time (about 11 ms at 48 kHz): small,
/// so sounds start soon after what triggers them.
const FRAME_SIZE: u32 = 512;
/// How often edits to static geometry are picked up (rebuilds run on a
/// worker thread, but not for every frame of a drag).
const GEOMETRY_CHECK: Duration = Duration::from_millis(250);
/// Rays per source for volumetric occlusion: more = smoother fades.
const OCCLUSION_SAMPLES: u32 = 32;
/// Seconds for occlusion changes to settle, so a door opening fades the
/// sound in rather than switching it.
const OCCLUSION_SMOOTHING: f32 = 0.08;
/// Longest reverb tail simulated (seconds); enough for a large stone hall.
const MAX_REVERB: f32 = 3.0;
/// Footsteps don't need to reserve the full hall tail: keeping this shorter
/// avoids choking the one-shot pool while sprinting.
const FOOTSTEP_TAIL: f32 = 1.0;
/// Seconds for a sound leaving (or re-entering) the simulation radius to
/// fade out (or in).
const RADIUS_FADE: f32 = 1.0;
/// Voices kept ready for one-shot sounds (claps, footsteps); enough for
/// sprinting footsteps that each ring out for the full reverb tail.
const ONE_SHOT_VOICES: usize = 24;

type DirectSimulator = Simulator<DefaultRayTracer, Direct>;
type ReflectionSimulator = Simulator<DefaultRayTracer, (), Reflections, (), Convolution>;
type ReflectionSource = Source<(), Reflections, (), Convolution>;

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
    /// The listener's reverb from baked probes (where there are some)
    /// instead of live ray tracing.
    pub baked: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            occlusion: true,
            reflections: true,
            hrtf: true,
            air_absorption: true,
            baked: true,
        }
    }
}

pub struct Audio {
    _stream: cpal::Stream,
    /// How the output callback keeps up (for `--perf-log`).
    timing: Arc<Timing>,
    rate: u32,
    context: Context,
    settings: AudioSettings,
    hrtf: Hrtf,
    direct: DirectSimulator,
    direct_geometry: AcousticGeometry,
    reflection: ReflectionSimulator,
    reflection_geometry: AcousticGeometry,
    simulation: Simulation<u32, DefaultRayTracer, (), Reflections, (), Convolution>,
    reflections: ReflectionsSimulation<u32, (), (), Convolution>,
    reflections_paused: bool,
    /// A reflection source that follows the listener: the reverb of the
    /// room the listener is in.
    listener_reverb: ReflectionSource,
    /// Fingerprint the geometry was built from, and when it was last checked.
    geometry_signature: u64,
    geometry_checked: Instant,
    /// A rebuild running on a worker thread.
    geometry_job: Option<Receiver<Result<Rebuilt, AudioError>>>,
    last_update: Instant,
    voices: Vec<Entry>,
    next_id: u32,
    commands: Sender<Command>,
    shared: Arc<Mutex<Shared>>,
    /// Loaded clips by path; `None` = no file (or it failed, reported once).
    clips: HashMap<String, Option<Clip>>,
    project_dir: PathBuf,
    footstep_variant: u32,
    /// Which of the project's own footstep clips plays next.
    steps: steps::StepPicker,
    pub toggles: Toggles,
    /// The scene's reverb bake, if it has one; its probe batches are in the
    /// reflection simulator.
    baked: Option<bake::Baked>,
}

struct Entry {
    id: u32,
    kind: Kind,
    direct: Source<Direct>,
    reflection: ReflectionSource,
    position: Vec3,
    volume: f32,
    looping: bool,
    /// Size of the sound for occlusion (meters).
    radius: f32,
    /// Smoothed occlusion and transmission, so changes fade.
    occlusion: f32,
    transmission: [f32; 3],
    /// 1 inside the simulation radius, fading to 0 outside it.
    presence: f32,
    /// In the simulators. A scene sound outside the radius leaves them
    /// once it has faded out (and keeps its place in the clip, silently).
    simulated: bool,
}

impl Entry {
    /// Scene sounds can be blocked by walls. One-shots are the player's own
    /// sounds (claps, footsteps): right at the listener, never occluded.
    fn occluded(&self) -> bool {
        matches!(self.kind, Kind::Scene { .. })
    }
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
        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Device(e.to_string()))?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;

        let context = Context::default();
        let settings = AudioSettings {
            sampling_rate: rate,
            frame_size: FRAME_SIZE,
        };
        let hrtf = Hrtf::try_new(&context, &settings, &HrtfSettings::default()).map_err(steam)?;

        let empty = || AcousticGeometry::build(&context, &Scene::default(), &mut Assets::default());
        let direct_geometry = empty()?;
        let mut direct = Simulator::try_new(
            &context,
            &SimulationSettings::new(&settings).with_direct(DirectSimulationSettings {
                max_num_occlusion_samples: OCCLUSION_SAMPLES,
            }),
        )
        .map_err(steam)?;
        direct.set_scene(&direct_geometry.scene);
        direct.commit();

        let reflection_geometry = empty()?;
        let mut reflection = Simulator::try_new(
            &context,
            &SimulationSettings::new(&settings).with_reflections(ConvolutionSettings {
                max_num_rays: 4096,
                num_diffuse_samples: 32,
                max_duration: MAX_REVERB,
                max_num_sources: 16,
                num_threads: 2,
                max_order: 1,
            }),
        )
        .map_err(steam)?;
        reflection.set_scene(&reflection_geometry.scene);
        reflection.commit();
        let listener_reverb = ReflectionSource::try_new(&reflection).map_err(steam)?;
        reflection.add_source(&listener_reverb);
        reflection.commit();
        let mut simulation = Simulation::new::<u32>(reflection.clone());
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
        let buses = BusEffects {
            binaural: decoder(Rendering::Binaural)?,
            panning: decoder(Rendering::Panning)?,
            listener_room: [
                self::reverb_effect(&context, &settings)?,
                self::reverb_effect(&context, &settings)?,
            ],
            // The zone's stylized reverb: one channel, widened in the mixer.
            zone_reverb: ReflectionEffect::<Parametric>::try_new(
                &context,
                &settings,
                &ReflectionEffectSettings {
                    impulse_response_size: FRAME_SIZE,
                    num_channels: 1,
                },
            )
            .map_err(steam)?,
        };
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (commands, receive_commands) = channel();
        let mixer = Mixer::new(
            hrtf.clone(),
            FRAME_SIZE as usize,
            rate,
            receive_commands,
            shared.clone(),
            reflections.output(),
            buses,
        );
        let timing = Arc::new(Timing::default());
        let stream = open_stream(&device, &supported, channels, rate, mixer, timing.clone())?;
        log::info!("audio: {rate} Hz, {channels} channels");

        let mut audio = Self {
            _stream: stream,
            timing,
            rate,
            context,
            settings,
            hrtf,
            direct,
            direct_geometry,
            reflection,
            reflection_geometry,
            simulation,
            reflections,
            reflections_paused: true,
            listener_reverb,
            geometry_signature: geometry::signature(&Scene::default()),
            geometry_checked: Instant::now(),
            geometry_job: None,
            last_update: Instant::now(),
            voices: Vec::new(),
            next_id: 0,
            commands,
            shared,
            clips: HashMap::new(),
            project_dir,
            footstep_variant: 0,
            steps: steps::StepPicker::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(1, |d| d.subsec_nanos()),
            ),
            toggles: Toggles::default(),
            baked: None,
        };
        for _ in 0..ONE_SHOT_VOICES {
            audio.add_voice(Kind::OneShot { until: None }, None);
        }
        Ok(audio)
    }

    /// Call every frame: follows the scene's geometry and sound sources and
    /// the listener, and runs the direct simulation. Scene sounds outside
    /// the `active` zones fade out and aren't simulated. In a zone with
    /// baked probes, the listener's reverb comes from those; a zone with its
    /// own `reverb` adds that tail to every sound.
    /// The output callback's worst timings since the last call.
    pub fn take_timing(&self) -> TimingReport {
        self.timing.take()
    }

    pub fn update(
        &mut self,
        scene: &Scene,
        assets: &mut Assets,
        listener: &Camera,
        active: &Active,
        listener_zone: Option<&str>,
    ) {
        let now = Instant::now();
        let dt = (now - self.last_update).as_secs_f32().min(0.1);
        self.last_update = now;
        self.update_geometry(scene, assets);
        self.sync_scene_voices(scene);
        self.follow_radius(active, dt);

        // Which voices are simulated: scene sources, and one-shots still
        // ringing out.
        for entry in &mut self.voices {
            if let Kind::OneShot { until } = &mut entry.kind
                && until.is_some_and(|t| now >= t)
            {
                *until = None;
            }
        }
        let active = |e: &&Entry| e.simulated && !matches!(e.kind, Kind::OneShot { until: None });

        let right = listener.right();
        let ahead = listener.forward();
        let up = right.cross(ahead);
        let listener_frame = CoordinateSystem {
            right: vector(right),
            up: vector(up),
            ahead: vector(ahead),
            origin: vector(listener.position),
        };

        // Direct sound, here and now.
        let direct_shared = SimulationSharedInputs::new(listener_frame).with_direct();
        if let Err(err) = self.direct.set_shared_inputs(&direct_shared) {
            log::error!("audio listener: {err}");
            return;
        }
        for entry in self.voices.iter().filter(active) {
            let inputs = direct_inputs(entry.position, entry.occluded(), entry.radius);
            if let Err(err) = entry.direct.set_inputs(&inputs) {
                log::error!("audio source: {err}");
            }
        }
        crate::perf::time("RUN DIRECT", || self.direct.run_direct());

        // Reflections on their own thread, from the latest snapshot.
        let baked = self.toggles.baked
            && listener_zone.is_some_and(|zone| {
                self.baked
                    .as_ref()
                    .is_some_and(|b| b.zones.iter().any(|z| z.zone == zone))
            });
        self.simulation.update_sources(|sources| {
            sources.push((
                LISTENER_REVERB,
                SourceWithInputs {
                    source: self.listener_reverb.clone(),
                    simulation_inputs: reflection_inputs(listener.position, baked),
                },
            ));
            // The player's own sounds use the listener's room.
            for entry in self.voices.iter().filter(active).filter(|e| e.occluded()) {
                sources.push((
                    entry.id,
                    SourceWithInputs {
                        source: entry.reflection.clone(),
                        simulation_inputs: reflection_inputs(entry.position, false),
                    },
                ));
            }
        });
        self.reflections.set_input(ReflectionsFrame {
            sources: self.simulation.sources.clone(),
            shared_inputs: SimulationSharedInputs::new(listener_frame).with_reflections(
                ReflectionsSharedInputs {
                    num_rays: 4096,
                    num_bounces: 16,
                    duration: MAX_REVERB,
                    order: 1,
                    irradiance_min_distance: 1.0,
                },
            ),
        });
        let idle = !self.voices.iter().any(|e| active(&e));
        if idle != self.reflections_paused {
            if idle {
                self.reflections.pause();
            } else {
                self.reflections.resume();
            }
            self.reflections_paused = idle;
        }

        let fade = 1.0 - (-dt / OCCLUSION_SMOOTHING).exp();
        let mut voices = Vec::new();
        for entry in self.voices.iter_mut() {
            if !entry.simulated {
                // Silent, but the mixer keeps reading so the loop stays in time.
                voices.push((entry.id, VoiceParams::default()));
                continue;
            }
            if !active(&&*entry) {
                continue;
            }
            let toward = (entry.position - listener.position).normalize_or(ahead);
            let mut params = VoiceParams {
                volume: entry.volume * entry.presence,
                looping: entry.looping,
                direction: [toward.dot(right), toward.dot(up), -toward.dot(ahead)],
                ..Default::default()
            };
            if let Ok(outputs) = entry.direct.get_outputs() {
                let direct = outputs.direct();
                params.distance_attenuation = direct.distance_attenuation.unwrap_or(1.0);
                if let Some(air) = direct.air_absorption {
                    params.air_absorption = air.0;
                }
                if entry.occluded() {
                    let target = direct.occlusion.unwrap_or(1.0);
                    entry.occlusion += (target - entry.occlusion) * fade;
                    if let Some(
                        Transmission::FrequencyDependent(t) | Transmission::FrequencyIndependent(t),
                    ) = direct.transmission
                    {
                        for (smoothed, target) in entry.transmission.iter_mut().zip(t.0) {
                            *smoothed += (target - *smoothed) * fade;
                        }
                    }
                    params.occlusion = entry.occlusion;
                    params.transmission = entry.transmission;
                }
            }
            params.own_reverb = entry.occluded();
            params.through = if entry.occluded() {
                (1.0 - params.occlusion) * params.transmission.iter().sum::<f32>() / 3.0
            } else {
                1.0
            };
            voices.push((entry.id, params));
        }
        if let Ok(mut shared) = self.shared.lock() {
            shared.voices = voices;
            shared.listener = listener_frame;
            shared.toggles = self.toggles;
            shared.stylized = listener_zone
                .and_then(|name| scene.zones.iter().find(|z| z.name == name))
                .and_then(|z| z.reverb.as_ref())
                .map(|r| Stylized {
                    decays: r.band_decays(),
                    level: r.level,
                    replace: r.replace_simulated,
                });
        }
    }

    /// Fades scene sounds in and out of the simulation radius; one that has
    /// faded out leaves the simulators, and rejoins when its zone is active.
    fn follow_radius(&mut self, active: &Active, dt: f32) {
        let step = dt / RADIUS_FADE;
        let mut changed = false;
        for entry in &mut self.voices {
            let Kind::Scene { object, .. } = entry.kind else {
                continue;
            };
            let inside = active.object(object);
            entry.presence = if inside {
                (entry.presence + step).min(1.0)
            } else {
                (entry.presence - step).max(0.0)
            };
            let simulated = inside || entry.presence > 0.0;
            if simulated != entry.simulated {
                entry.simulated = simulated;
                if simulated {
                    self.direct.add_source(&entry.direct);
                    self.reflection.add_source(&entry.reflection);
                } else {
                    self.direct.remove_source(&entry.direct);
                    self.reflection.remove_source(&entry.reflection);
                }
                changed = true;
            }
        }
        if changed {
            self.direct.commit();
            self.simulation.request_simulator_commit();
        }
    }

    /// Starts baking the scene's reverb into `file` in the background.
    pub fn start_bake(
        &self,
        scene: &Scene,
        assets: &mut Assets,
        file: PathBuf,
    ) -> Result<bake::BakeJob, bake::BakeError> {
        bake::start(&self.context, scene, assets, file)
    }

    /// Replaces the reverb bake with the one in `file` (none if there's no
    /// file). Returns its signature and probe positions.
    pub fn load_bake(
        &mut self,
        file: &std::path::Path,
    ) -> Result<Option<(u64, Vec<Vec3>)>, bake::BakeError> {
        if let Some(old) = self.baked.take() {
            for zone in &old.zones {
                self.reflection.remove_probe_batch(&zone.batch);
            }
            self.simulation.request_simulator_commit();
        }
        let Some(baked) = bake::load(&self.context, file)? else {
            return Ok(None);
        };
        for zone in &baked.zones {
            self.reflection.add_probe_batch(&zone.batch);
        }
        self.simulation.request_simulator_commit();
        let probes = baked.zones.iter().flat_map(|z| z.probes.clone()).collect();
        let signature = baked.signature;
        log::info!("loaded {} ({} zones)", file.display(), baked.zones.len());
        self.baked = Some(baked);
        Ok(Some((signature, probes)))
    }

    /// A project sound (`audio/hum.wav`) once at `position`, through the
    /// acoustic simulation. A missing file is logged once and skipped.
    pub fn play_sound(&mut self, path: &str, position: Vec3, volume: f32) {
        if let Some(clip) = self.load(path) {
            self.play(clip, position, volume.max(0.0), MAX_REVERB);
        }
    }

    /// A hand clap at `position` (usually the listener).
    pub fn clap(&mut self, position: Vec3) {
        let rate = self.rate;
        let clip = self.clip_or("audio/clap.wav", || clip::clap(rate));
        self.play(clip, position, 1.0, MAX_REVERB);
    }

    /// A footstep on a surface of `material` at `position`: the project's
    /// own clips if `steps` lists some and `use_default` is off, otherwise
    /// the default step for the material (which a project can replace with
    /// `audio/footsteps/<material>.wav`).
    pub fn footstep(&mut self, material: AcousticPreset, position: Vec3, steps: &Footsteps) {
        let volume = 0.5 * steps.volume.max(0.0);
        let clips: Vec<&String> = if steps.use_default {
            Vec::new()
        } else {
            steps.clips.iter().filter(|c| !c.is_empty()).collect()
        };
        if !clips.is_empty() {
            let index = self.steps.pick(clips.len(), steps);
            if let Some(clip) = self.load(clips[index]) {
                let (clip, gain) = if steps.randomize && steps.random == RandomOrder::Smart {
                    let (pitch, gain) = self.steps.variation();
                    // Read as if recorded at a slightly different rate: higher and
                    // shorter, or lower and longer.
                    let from = (self.rate as f32 * pitch) as u32;
                    (Arc::new(clip::resample(&clip, from, self.rate)), gain)
                } else {
                    (clip, 1.0)
                };
                self.play(clip, position, volume * gain, FOOTSTEP_TAIL);
                return;
            }
            // A missing or broken file (logged once): the default step instead.
        }
        let name = format!("{material:?}").to_lowercase();
        let rate = self.rate;
        self.footstep_variant = (self.footstep_variant + 1) % 4;
        let variant = self.footstep_variant;
        let clip = self.clip_or(&format!("audio/footsteps/{name}.wav"), || {
            clip::footstep(&name, variant, rate)
        });
        self.play(clip, position, volume, FOOTSTEP_TAIL);
    }

    /// Plays a clip once from the one-shot pool: a free voice, or the one
    /// that has just finished ringing out.
    fn play(&mut self, clip: Clip, position: Vec3, volume: f32, tail: f32) {
        let now = Instant::now();
        let ring = Duration::from_secs_f32(clip.len() as f32 / self.rate as f32 + tail.max(0.0));
        let Some(entry) = self
            .voices
            .iter_mut()
            .filter_map(|e| match e.kind {
                Kind::OneShot { until } => Some((until, e)),
                Kind::Scene { .. } => None,
            })
            .find(|(until, _)| until.is_none_or(|t| t <= now))
            .map(|(_, e)| e)
        else {
            // All one-shot voices are still ringing: skip this event instead
            // of restarting an active tail, which sounds robotic.
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
        let mut wanted: Vec<(usize, &SoundSource, Vec3)> = Vec::new();
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
                if entry.simulated {
                    self.direct.remove_source(&entry.direct);
                    self.reflection.remove_source(&entry.reflection);
                }
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
            entry.radius = sound.radius.max(0.05);
        }
        if changed {
            self.direct.commit();
            self.simulation.request_simulator_commit();
        }
    }

    /// Creates a voice: a source in each simulator and the mixer's effects.
    fn add_voice(&mut self, kind: Kind, clip: Option<Clip>) -> bool {
        let sources = Source::try_new(&self.direct)
            .and_then(|direct| Ok((direct, ReflectionSource::try_new(&self.reflection)?)));
        let (direct, reflection) = match sources {
            Ok(sources) => sources,
            Err(err) => {
                log::error!("audio source: {err}");
                return false;
            }
        };
        let own_reverb = matches!(kind, Kind::Scene { .. });
        let voice = match self.effects(self.next_id, clip, own_reverb) {
            Ok(voice) => voice,
            Err(err) => {
                log::error!("audio: {err}");
                return false;
            }
        };
        self.direct.add_source(&direct);
        self.reflection.add_source(&reflection);
        let _ = self.commands.send(Command::Add(voice));
        self.voices.push(Entry {
            id: self.next_id,
            kind,
            direct,
            reflection,
            position: Vec3::ZERO,
            volume: 1.0,
            looping: false,
            radius: 1.0,
            occlusion: 1.0,
            transmission: [1.0; 3],
            presence: 1.0,
            simulated: true,
        });
        self.next_id += 1;
        true
    }

    fn effects(&self, id: u32, clip: Option<Clip>, own_reverb: bool) -> Result<Voice, AudioError> {
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
            reflection: if own_reverb {
                Some([self.reverb()?, self.reverb()?])
            } else {
                None
            },
        })
    }

    fn reverb(&self) -> Result<ReflectionEffect<Convolution>, AudioError> {
        ReflectionEffect::try_new(
            &self.context,
            &self.settings,
            &ReflectionEffectSettings {
                impulse_response_size: (MAX_REVERB * self.rate as f32) as u32,
                num_channels: AMBISONIC_CHANNELS as u32,
            },
        )
        .map_err(steam)
    }

    /// Rebuilds both acoustic scenes when static objects were edited, and
    /// moves the moving ones every frame: the direct scene at once, the
    /// reflection scene on its thread between passes.
    ///
    /// A rebuild runs on a worker thread: building takes milliseconds, and
    /// handing the reflection simulator a new scene waits for its current
    /// pass (up to hundreds of milliseconds while sources move).
    fn update_geometry(&mut self, scene: &Scene, assets: &mut Assets) {
        if let Some(job) = &self.geometry_job {
            match job.try_recv() {
                Ok(Ok(rebuilt)) => {
                    perf::time("ACOUSTIC SET SCENE", || {
                        self.direct_geometry = rebuilt.direct;
                        self.direct.set_scene(&self.direct_geometry.scene);
                        self.direct.commit();
                    });
                    // Already set on the worker; the thread commits it.
                    self.reflection_geometry = rebuilt.reflection;
                    self.simulation.request_simulator_commit();
                    self.geometry_signature = rebuilt.signature;
                    self.geometry_job = None;
                }
                Ok(Err(err)) => {
                    log::error!("acoustic geometry: {err}");
                    self.geometry_job = None;
                }
                Err(TryRecvError::Disconnected) => self.geometry_job = None,
                Err(TryRecvError::Empty) => {}
            }
        } else if self.geometry_checked.elapsed() >= GEOMETRY_CHECK {
            self.geometry_checked = Instant::now();
            let signature = geometry::signature(scene);
            if signature != self.geometry_signature {
                let source = perf::time("ACOUSTIC GATHER", || {
                    geometry::GeometrySource::gather(scene, assets)
                });
                self.geometry_job = Some(rebuild(
                    self.context.clone(),
                    self.reflection.clone(),
                    source,
                    signature,
                ));
            }
        }
        if self.direct_geometry.follow(scene) {
            perf::time("ACOUSTIC MOVE COMMIT", || {
                self.direct_geometry.scene.commit()
            });
        }
        if self.reflection_geometry.follow(scene) {
            self.simulation
                .request_scene_commits(std::slice::from_ref(&self.reflection_geometry.scene));
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

fn reverb_effect(
    context: &Context,
    settings: &AudioSettings,
) -> Result<ReflectionEffect<Convolution>, AudioError> {
    ReflectionEffect::try_new(
        context,
        settings,
        &ReflectionEffectSettings {
            impulse_response_size: (MAX_REVERB * settings.sampling_rate as f32) as u32,
            num_channels: AMBISONIC_CHANNELS as u32,
        },
    )
    .map_err(steam)
}

/// The output stream, asking the device for a buffer as small as one mixer
/// frame (lower latency) and falling back to its default size.
fn open_stream(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    channels: usize,
    rate: u32,
    mixer: Mixer,
    timing: Arc<Timing>,
) -> Result<cpal::Stream, AudioError> {
    let mixer = Arc::new(Mutex::new(mixer));
    let build = |config: cpal::StreamConfig| {
        let mixer = mixer.clone();
        let (timing, errors) = (timing.clone(), timing.clone());
        let mut previous = None;
        device.build_output_stream::<f32, _, _>(
            config,
            move |out, _| {
                let start = Instant::now();
                // Only this callback ever locks it.
                if let Ok(mut mixer) = mixer.lock() {
                    mixer.fill(out, channels);
                }
                timing.record(previous, start, out.len() / channels, rate);
                previous = Some(start);
            },
            move |err| {
                if err.kind() == cpal::ErrorKind::Xrun {
                    errors.underrun();
                }
                log::error!("audio output: {err}");
            },
            None,
        )
    };
    let mut config = supported.config();
    if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size() {
        config.buffer_size = cpal::BufferSize::Fixed(FRAME_SIZE.clamp(*min, *max));
    }
    let stream = build(config)
        .or_else(|err| {
            log::info!("audio: small buffer refused ({err}); using the device default");
            build(supported.config())
        })
        .map_err(|e| AudioError::Device(e.to_string()))?;
    stream
        .play()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    Ok(stream)
}

/// What each source's direct sound is simulated for; occlusion only if
/// `occluded`, with the sound modeled as a sphere of `radius`.
fn direct_inputs(position: Vec3, occluded: bool, radius: f32) -> SimulationInputs<Direct> {
    let mut direct = DirectSimulationParameters::new()
        .with_distance_attenuation(DistanceAttenuationModel::default())
        .with_air_absorption(AirAbsorptionModel::default());
    if occluded {
        direct = direct.with_occlusion(
            Occlusion::new(OcclusionAlgorithm::Volumetric {
                radius,
                num_occlusion_samples: OCCLUSION_SAMPLES,
            })
            .with_transmission(TransmissionParameters {
                num_transmission_rays: 3,
            }),
        );
    }
    SimulationInputs {
        source: at(position),
        parameters: SimulationParameters::new().with_direct(direct),
    }
}

/// Reflections for a source at `position`: ray traced live, or looked up
/// in the baked reverb probes.
fn reflection_inputs(position: Vec3, baked: bool) -> SimulationInputs<(), Reflections> {
    SimulationInputs {
        source: at(position),
        parameters: SimulationParameters::new().with_reflections(ConvolutionParameters {
            baked_data_identifier: baked.then_some(bake::REVERB),
        }),
    }
}

fn at(position: Vec3) -> CoordinateSystem {
    CoordinateSystem {
        origin: vector(position),
        ..Default::default()
    }
}

fn vector(v: Vec3) -> Vector3 {
    Vector3::new(v.x, v.y, v.z)
}

fn steam(e: audionimbus::SteamAudioError) -> AudioError {
    AudioError::SteamAudio(e.to_string())
}

/// Acoustic scenes built on a worker thread, the reflection one already
/// handed to its simulator.
struct Rebuilt {
    direct: AcousticGeometry,
    reflection: AcousticGeometry,
    signature: u64,
}

fn rebuild(
    context: Context,
    mut reflection: ReflectionSimulator,
    source: geometry::GeometrySource,
    signature: u64,
) -> Receiver<Result<Rebuilt, AudioError>> {
    let (send, receive) = channel();
    std::thread::spawn(move || {
        let built = perf::time("ACOUSTIC BUILD", || {
            Ok::<_, AudioError>((
                AcousticGeometry::from_source(&context, &source)?,
                AcousticGeometry::from_source(&context, &source)?,
            ))
        });
        let result = built.map(|(direct, reflection_geometry)| {
            // Waits for the reflection pass in progress: off the main thread.
            perf::time("ACOUSTIC WAIT FOR REFLECTIONS", || {
                reflection.set_scene(&reflection_geometry.scene)
            });
            Rebuilt {
                direct,
                reflection: reflection_geometry,
                signature,
            }
        });
        let _ = send.send(result);
    });
    receive
}
