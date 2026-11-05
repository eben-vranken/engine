//! Audio: Steam Audio (through AudioNimbus) for spatial sound, cpal for the
//! output device. The listener is whatever camera is active, so the editor
//! previews sound too.
//!
//! Each frame the main thread simulates every source (distance, air
//! absorption) and hands the results to the mixer on the audio thread
//! (`mixer.rs`), which renders them.

mod clip;
mod mixer;

use crate::camera::Camera;
use crate::scene::{Component, Scene, SoundSource};
use audionimbus::{
    AirAbsorptionModel, AudioSettings, BinauralEffect, BinauralEffectSettings, Context,
    CoordinateSystem, DefaultRayTracer, Direct, DirectEffect, DirectEffectSettings,
    DirectSimulationParameters, DirectSimulationSettings, DistanceAttenuationModel, Hrtf,
    HrtfSettings, SimulationInputs, SimulationParameters, SimulationSettings,
    SimulationSharedInputs, Simulator, Source, Vector3,
};
use clip::Clip;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glam::Vec3;
use mixer::{Mixer, Voice, VoiceParams};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

/// Samples per channel processed at a time (21 ms at 48 kHz).
const FRAME_SIZE: u32 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio output device")]
    NoDevice,
    #[error("audio output: {0}")]
    Device(String),
    #[error("Steam Audio: {0}")]
    SteamAudio(String),
}

pub struct Audio {
    _stream: cpal::Stream,
    rate: u32,
    context: Context,
    settings: AudioSettings,
    hrtf: Hrtf,
    simulator: Simulator<DefaultRayTracer, Direct>,
    /// One simulated source per playing voice, with what it plays.
    sources: Vec<(VoiceKey, Source<Direct>)>,
    params: Arc<Mutex<Vec<VoiceParams>>>,
    new_voices: Sender<Vec<Voice>>,
    /// Loaded clips by path; `None` = failed (reported once).
    clips: HashMap<String, Option<Clip>>,
    project_dir: PathBuf,
}

/// What a voice plays: the object and its clip. When these change the
/// voices are rebuilt; volume and position just update.
#[derive(Debug, Clone, PartialEq)]
struct VoiceKey {
    object: usize,
    clip: String,
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

        let steam = |e: audionimbus::SteamAudioError| AudioError::SteamAudio(e.to_string());
        let context = Context::default();
        let settings = AudioSettings {
            sampling_rate: rate,
            frame_size: FRAME_SIZE,
        };
        let hrtf = Hrtf::try_new(&context, &settings, &HrtfSettings::default()).map_err(steam)?;
        let simulation = SimulationSettings::new(&settings).with_direct(DirectSimulationSettings {
            max_num_occlusion_samples: 16,
        });
        let mut simulator = Simulator::try_new(&context, &simulation).map_err(steam)?;
        // No geometry yet: occlusion and reflections come next.
        let acoustic_scene = audionimbus::Scene::try_new(&context).map_err(steam)?;
        acoustic_scene.commit();
        simulator.set_scene(&acoustic_scene);
        simulator.commit();

        let params = Arc::new(Mutex::new(Vec::new()));
        let (new_voices, receive_voices) = channel();
        let mut mixer = Mixer::new(
            hrtf.clone(),
            FRAME_SIZE as usize,
            rate,
            receive_voices,
            params.clone(),
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

        Ok(Self {
            _stream: stream,
            rate,
            context,
            settings,
            hrtf,
            simulator,
            sources: Vec::new(),
            params,
            new_voices,
            clips: HashMap::new(),
            project_dir,
        })
    }

    /// Call every frame: follows the scene's sound sources and the listener.
    pub fn update(&mut self, scene: &Scene, listener: &Camera) {
        let wanted: Vec<(VoiceKey, &SoundSource, Vec3)> = scene
            .objects
            .iter()
            .enumerate()
            .flat_map(|(object, o)| {
                o.components.iter().filter_map(move |c| match c {
                    Component::SoundSource(s) => Some((
                        VoiceKey {
                            object,
                            clip: s.clip.clone(),
                        },
                        s,
                        o.transform.position,
                    )),
                    _ => None,
                })
            })
            .collect();
        let keys: Vec<&VoiceKey> = wanted.iter().map(|(k, ..)| k).collect();
        if keys != self.sources.iter().map(|(k, _)| k).collect::<Vec<_>>() {
            self.rebuild(wanted.iter().map(|(k, ..)| k.clone()).collect());
        }

        let right = listener.right();
        let ahead = listener.forward();
        let up = right.cross(ahead);
        let shared = SimulationSharedInputs::new(CoordinateSystem {
            right: vector(right),
            up: vector(up),
            ahead: vector(ahead),
            origin: vector(listener.position),
        })
        .with_direct();
        if let Err(err) = self.simulator.set_shared_inputs(&shared) {
            log::error!("audio listener: {err}");
            return;
        }
        for ((_, source), (_, _, position)) in self.sources.iter().zip(&wanted) {
            let inputs = SimulationInputs {
                source: CoordinateSystem {
                    origin: vector(*position),
                    ..Default::default()
                },
                parameters: SimulationParameters::new().with_direct(
                    DirectSimulationParameters::new()
                        .with_distance_attenuation(DistanceAttenuationModel::default())
                        .with_air_absorption(AirAbsorptionModel::default()),
                ),
            };
            if let Err(err) = source.set_inputs(&inputs) {
                log::error!("audio source: {err}");
            }
        }
        self.simulator.run_direct();

        let params: Vec<VoiceParams> = self
            .sources
            .iter()
            .zip(&wanted)
            .map(|((_, source), (_, sound, position))| {
                let toward = (*position - listener.position).normalize_or(ahead);
                let mut params = VoiceParams {
                    volume: sound.volume,
                    looping: sound.looping,
                    direction: [toward.dot(right), toward.dot(up), -toward.dot(ahead)],
                    ..Default::default()
                };
                if let Ok(outputs) = source.get_outputs() {
                    let direct = outputs.direct();
                    params.distance_attenuation = direct.distance_attenuation.unwrap_or(1.0);
                    if let Some(air) = direct.air_absorption {
                        params.air_absorption = air.0;
                    }
                }
                params
            })
            .collect();
        if let Ok(mut shared) = self.params.lock() {
            *shared = params;
        }
    }

    /// New set of playing sounds: fresh voices for the mixer and matching
    /// sources in the simulator.
    fn rebuild(&mut self, keys: Vec<VoiceKey>) {
        for (_, source) in self.sources.drain(..) {
            self.simulator.remove_source(&source);
        }
        let mut voices = Vec::new();
        for key in keys {
            let (Some(voice), Ok(source)) =
                (self.voice(&key.clip), Source::try_new(&self.simulator))
            else {
                continue;
            };
            self.simulator.add_source(&source);
            voices.push(voice);
            self.sources.push((key, source));
        }
        self.simulator.commit();
        if let Ok(mut params) = self.params.lock() {
            params.clear();
        }
        let _ = self.new_voices.send(voices);
    }

    fn voice(&mut self, clip: &str) -> Option<Voice> {
        let clip = self.clip(clip)?;
        let direct = DirectEffect::try_new(
            &self.context,
            &self.settings,
            &DirectEffectSettings { num_channels: 1 },
        )
        .inspect_err(|e| log::error!("audio: {e}"))
        .ok()?;
        let binaural = BinauralEffect::try_new(
            &self.context,
            &self.settings,
            &BinauralEffectSettings {
                hrtf: self.hrtf.clone(),
            },
        )
        .inspect_err(|e| log::error!("audio: {e}"))
        .ok()?;
        Some(Voice {
            clip,
            position: 0,
            direct,
            binaural,
        })
    }

    /// A clip by path (relative to the project); empty = the generated loop.
    fn clip(&mut self, path: &str) -> Option<Clip> {
        let (rate, dir) = (self.rate, &self.project_dir);
        self.clips
            .entry(path.to_owned())
            .or_insert_with(|| {
                if path.is_empty() {
                    return Some(clip::plucked_loop(rate));
                }
                clip::load_wav(&dir.join(path), rate)
                    .inspect_err(|e| log::error!("{e}"))
                    .ok()
            })
            .clone()
    }
}

fn vector(v: Vec3) -> Vector3 {
    Vector3::new(v.x, v.y, v.z)
}
