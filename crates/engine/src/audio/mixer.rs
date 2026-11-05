//! The audio thread's side. Every voice goes through Steam Audio's direct
//! effect (distance, air absorption, occlusion, transmission) and is placed
//! with HRTF (or plain panning); its reflections are convolved into a shared
//! ambisonic bus that is decoded once per frame. The mix goes through a
//! master limiter.
//!
//! It never blocks: voices arrive as commands over a channel, per-frame
//! parameters through a `try_lock`, reflection results through a lock-free
//! swap.

use super::Toggles;
use super::clip::Clip;
use audionimbus::wiring::{ReflectionsOutput, SharedSimulationOutput};
use audionimbus::{
    AmbisonicsDecodeEffect, AmbisonicsDecodeEffectParams, AudioBufferMut, AudioBufferRef,
    BinauralEffect, BinauralEffectParams, Convolution, CoordinateSystem, DirectEffect,
    DirectEffectParams, Direction, Equalizer, Hrtf, HrtfInterpolation, PanningEffect,
    PanningEffectParams, ReflectionEffect, Transmission,
};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

/// The mix never exceeds this (about -1 dBFS).
const CEILING: f32 = 0.9;
/// Seconds for the limiter to let go after a loud peak.
const RELEASE: f32 = 0.25;
/// First-order ambisonics: 4 channels.
pub const AMBISONIC_CHANNELS: usize = 4;

/// One sound the mixer can play, with its effects' state.
pub struct Voice {
    pub id: u32,
    /// `None` until a one-shot voice is given something to play.
    pub clip: Option<Clip>,
    pub position: usize,
    pub direct: DirectEffect,
    pub binaural: BinauralEffect,
    pub panning: PanningEffect,
    pub reflection: ReflectionEffect<Convolution>,
}

pub enum Command {
    Add(Voice),
    Remove(u32),
    /// Start a clip on a (one-shot) voice from the beginning.
    Play {
        id: u32,
        clip: Clip,
    },
}

/// Per-voice values the main thread computes each frame.
#[derive(Debug, Clone, Copy)]
pub struct VoiceParams {
    pub volume: f32,
    pub looping: bool,
    /// Toward the source, in the listener's space: x right, y up, z behind.
    pub direction: [f32; 3],
    pub distance_attenuation: f32,
    /// Low, mid and high band gains.
    pub air_absorption: [f32; 3],
    /// 1 = in plain view, 0 = fully blocked by geometry.
    pub occlusion: f32,
    /// How much gets through the blocking surfaces, per band.
    pub transmission: [f32; 3],
}

impl Default for VoiceParams {
    fn default() -> Self {
        Self {
            volume: 0.0,
            looping: true,
            direction: [0.0, 0.0, -1.0],
            distance_attenuation: 1.0,
            air_absorption: [1.0; 3],
            occlusion: 1.0,
            transmission: [1.0; 3],
        }
    }
}

/// What the main thread shares with the mixer each frame.
#[derive(Clone, Default)]
pub struct Shared {
    pub voices: Vec<(u32, VoiceParams)>,
    pub listener: CoordinateSystem,
    pub toggles: Toggles,
}

/// The ambisonic decoders for the reverb bus, one per HRTF setting.
pub struct Decoders {
    pub binaural: AmbisonicsDecodeEffect,
    pub panning: AmbisonicsDecodeEffect,
}

pub struct Mixer {
    hrtf: Hrtf,
    frame_size: usize,
    voices: Vec<Voice>,
    commands: Receiver<Command>,
    shared_in: Arc<Mutex<Shared>>,
    shared: Shared,
    reflections: SharedSimulationOutput<ReflectionsOutput<u32, Convolution>>,
    decoders: Decoders,
    /// The current frame (deinterleaved stereo) and how much of it is played.
    mix: Vec<f32>,
    cursor: usize,
    input: Vec<f32>,
    attenuated: Vec<f32>,
    spatial: Vec<f32>,
    reflected: Vec<f32>,
    reverb_bus: Vec<f32>,
    limiter: Limiter,
}

impl Mixer {
    pub fn new(
        hrtf: Hrtf,
        frame_size: usize,
        rate: u32,
        commands: Receiver<Command>,
        shared_in: Arc<Mutex<Shared>>,
        reflections: SharedSimulationOutput<ReflectionsOutput<u32, Convolution>>,
        decoders: Decoders,
    ) -> Self {
        Self {
            hrtf,
            frame_size,
            voices: Vec::new(),
            commands,
            shared_in,
            shared: Shared::default(),
            reflections,
            decoders,
            mix: vec![0.0; frame_size * 2],
            cursor: frame_size,
            input: vec![0.0; frame_size],
            attenuated: vec![0.0; frame_size],
            spatial: vec![0.0; frame_size * 2],
            reflected: vec![0.0; frame_size * AMBISONIC_CHANNELS],
            reverb_bus: vec![0.0; frame_size * AMBISONIC_CHANNELS],
            limiter: Limiter::new(rate),
        }
    }

    /// Fills an interleaved output buffer with `channels` channels; the mix
    /// goes to the first two.
    pub fn fill(&mut self, out: &mut [f32], channels: usize) {
        for frame in out.chunks_mut(channels) {
            if self.cursor >= self.frame_size {
                self.render();
                self.cursor = 0;
            }
            frame.fill(0.0);
            frame[0] = self.mix[self.cursor];
            if let Some(right) = frame.get_mut(1) {
                *right = self.mix[self.frame_size + self.cursor];
            }
            self.cursor += 1;
        }
    }

    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Add(voice) => self.voices.push(voice),
                Command::Remove(id) => self.voices.retain(|v| v.id != id),
                Command::Play { id, clip } => {
                    if let Some(voice) = self.voices.iter_mut().find(|v| v.id == id) {
                        voice.clip = Some(clip);
                        voice.position = 0;
                    }
                }
            }
        }
    }

    /// Renders the next frame into `mix`.
    fn render(&mut self) {
        self.apply_commands();
        if let Ok(shared) = self.shared_in.try_lock() {
            self.shared.clone_from(&shared);
        }
        let toggles = self.shared.toggles;
        let reflections = self.reflections.load();
        self.mix.fill(0.0);
        self.reverb_bus.fill(0.0);
        let mut reverb = false;

        for voice in &mut self.voices {
            let Some(params) = self
                .shared
                .voices
                .iter()
                .find(|(id, _)| *id == voice.id)
                .map(|(_, p)| *p)
            else {
                continue;
            };
            if !read(voice, &params, &mut self.input) {
                continue;
            }
            let Ok(input) = AudioBufferRef::try_from(&self.input[..]) else {
                continue;
            };
            let direct = DirectEffectParams {
                distance_attenuation: Some(params.distance_attenuation),
                air_absorption: toggles
                    .air_absorption
                    .then_some(Equalizer(params.air_absorption)),
                directivity: None,
                occlusion: toggles.occlusion.then_some(params.occlusion),
                transmission: toggles
                    .occlusion
                    .then_some(Transmission::FrequencyDependent(Equalizer(
                        params.transmission,
                    ))),
            };
            if let Ok(mut attenuated) = AudioBufferMut::try_new(&mut self.attenuated, 1) {
                let _ = voice.direct.apply(&direct, &input, &mut attenuated);
            }
            let [x, y, z] = params.direction;
            let direction = Direction::new(x, y, z);
            if let (Ok(attenuated), Ok(mut spatial)) = (
                AudioBufferRef::try_from(&self.attenuated[..]),
                AudioBufferMut::try_new(&mut self.spatial, 2),
            ) {
                let placed = if toggles.hrtf {
                    let params = BinauralEffectParams {
                        direction,
                        interpolation: HrtfInterpolation::Bilinear,
                        spatial_blend: 1.0,
                        hrtf: self.hrtf.clone(),
                        peak_delays: None,
                    };
                    voice
                        .binaural
                        .apply(&params, &attenuated, &mut spatial)
                        .is_ok()
                } else {
                    let params = PanningEffectParams { direction };
                    voice
                        .panning
                        .apply(&params, &attenuated, &mut spatial)
                        .is_ok()
                };
                if placed {
                    for (mix, sample) in self.mix.iter_mut().zip(&self.spatial) {
                        *mix += sample;
                    }
                }
            }

            // Reflections: the dry sound convolved with the room's response.
            if toggles.reflections
                && let Some(room) = reflections.sources.get(&voice.id)
                && let Ok(mut reflected) =
                    AudioBufferMut::try_new(&mut self.reflected, AMBISONIC_CHANNELS)
                && voice.reflection.apply(room, &input, &mut reflected).is_ok()
            {
                for (bus, sample) in self.reverb_bus.iter_mut().zip(&self.reflected) {
                    *bus += sample;
                }
                reverb = true;
            }
        }

        if reverb
            && let (Ok(bus), Ok(mut spatial)) = (
                AudioBufferRef::try_new(&self.reverb_bus, AMBISONIC_CHANNELS),
                AudioBufferMut::try_new(&mut self.spatial, 2),
            )
        {
            let params = AmbisonicsDecodeEffectParams {
                order: 1,
                hrtf: self.hrtf.clone(),
                orientation: self.shared.listener,
            };
            let decoder = if toggles.hrtf {
                &mut self.decoders.binaural
            } else {
                &mut self.decoders.panning
            };
            if decoder.apply(&params, &bus, &mut spatial).is_ok() {
                for (mix, sample) in self.mix.iter_mut().zip(&self.spatial) {
                    *mix += sample;
                }
            }
        }

        let (left, right) = self.mix.split_at_mut(self.frame_size);
        self.limiter.process(left, right);
    }
}

/// The voice's next frame of samples into `out`, scaled by its volume.
/// False when there's nothing to play: no clip, or a one-shot that has
/// finished (its reverb tail still rings out through the convolution, which
/// is fed silence).
fn read(voice: &mut Voice, params: &VoiceParams, out: &mut [f32]) -> bool {
    let Some(clip) = &voice.clip else {
        return false;
    };
    if clip.is_empty() {
        return false;
    }
    for sample in out.iter_mut() {
        if voice.position >= clip.len() {
            if !params.looping {
                *sample = 0.0;
                continue;
            }
            voice.position = 0;
        }
        *sample = clip[voice.position] * params.volume;
        voice.position += 1;
    }
    true
}

/// Keeps the mix under `CEILING`: turns the gain down instantly for a loud
/// peak, then eases it back up.
struct Limiter {
    gain: f32,
    recovery: f32,
}

impl Limiter {
    fn new(rate: u32) -> Self {
        Self {
            gain: 1.0,
            recovery: 1.0 - (-1.0 / (RELEASE * rate as f32)).exp(),
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let peak = l.abs().max(r.abs());
            let allowed = if peak > CEILING { CEILING / peak } else { 1.0 };
            self.gain = (self.gain + (1.0 - self.gain) * self.recovery).min(allowed);
            *l *= self.gain;
            *r *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_never_lets_the_mix_past_the_ceiling() {
        let mut limiter = Limiter::new(48_000);
        let mut left: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 3.0).collect();
        let mut right = left.clone();
        limiter.process(&mut left, &mut right);
        assert!(left.iter().chain(&right).all(|s| s.abs() <= CEILING + 1e-6));
        // Quiet audio afterwards comes back to full level (8 release times).
        let mut quiet = vec![0.1; 96_000];
        let mut quiet_r = quiet.clone();
        limiter.process(&mut quiet, &mut quiet_r);
        assert!((quiet[95_999] - 0.1).abs() < 1e-3);
    }
}
