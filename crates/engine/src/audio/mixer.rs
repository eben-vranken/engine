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
use std::ops::Deref;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

/// The mix never exceeds this (about -1 dBFS).
const CEILING: f32 = 0.9;
/// Seconds for the limiter to let go after a loud peak.
const RELEASE: f32 = 0.25;
/// First-order ambisonics: 4 channels.
pub const AMBISONIC_CHANNELS: usize = 4;
/// The reflection result for a source at the listener: the listener's room.
pub const LISTENER_REVERB: u32 = u32::MAX;
/// Seconds over which a new reflection result replaces the previous one.
/// Each pass traces random rays, so results differ a little; switching
/// instantly made the reverb jump.
const BLEND: f32 = 0.2;

/// A reflection result, kept alive while it's still being faded out.
type Room = Arc<dyn Deref<Target = ReflectionsOutput<u32, Convolution>> + Send + Sync>;

/// One sound the mixer can play, with its effects' state.
pub struct Voice {
    pub id: u32,
    /// `None` until a one-shot voice is given something to play.
    pub clip: Option<Clip>,
    pub position: usize,
    pub direct: DirectEffect,
    pub binaural: BinauralEffect,
    pub panning: PanningEffect,
    /// The sound's own room (scene sounds only). Two effects each, so a new
    /// room response can fade in while the previous one fades out.
    pub reflection: Option<[ReflectionEffect<Convolution>; 2]>,
    /// The listener's room, for sound that reaches it through walls and the
    /// player's own sounds.
    pub room: [ReflectionEffect<Convolution>; 2],
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
    /// Reverb from the source's own position (scene sounds).
    pub own_reverb: bool,
    /// How much of the sound rings in the listener's room instead: what
    /// comes through walls, or all of the player's own sounds.
    pub through: f32,
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
            own_reverb: true,
            through: 0.0,
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
    through: Vec<f32>,
    /// The latest reflection result and the one fading out; which of each
    /// voice's two effects plays the latest; how far the fade is (0–1).
    current: Option<Room>,
    previous: Option<Room>,
    slot: usize,
    blend: f32,
    blend_step: f32,
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
            through: vec![0.0; frame_size],
            current: None,
            previous: None,
            slot: 0,
            blend: 1.0,
            blend_step: frame_size as f32 / (BLEND * rate as f32),
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
        // A new reflection result fades in over `BLEND`; the next one waits
        // until that's done.
        let latest: Room = (*self.reflections.load()).clone();
        let is_new = self
            .current
            .as_ref()
            .is_none_or(|c| !std::ptr::addr_eq(Arc::as_ptr(c), Arc::as_ptr(&latest)));
        if is_new && self.blend >= 1.0 {
            self.previous = self.current.replace(latest);
            self.slot = 1 - self.slot;
            self.blend = 0.0;
        }
        let fade_start = self.blend;
        self.blend = (self.blend + self.blend_step).min(1.0);
        let fade = if self.previous.is_some() {
            (fade_start, self.blend)
        } else {
            (1.0, 1.0)
        };
        if fade_start >= 1.0 {
            self.previous = None;
        }
        let (current, previous, slot) = (self.current.clone(), self.previous.clone(), self.slot);
        let rooms = [
            current.as_deref().map(|r| &**r),
            previous.as_deref().map(|r| &**r),
        ];
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

            // Reflections: the sound convolved with its own room's response,
            // and the part reaching the listener's room (through walls, or the
            // player's own sounds) with that room's response.
            if toggles.reflections {
                if params.own_reverb
                    && let Some(effects) = voice.reflection.as_mut()
                {
                    reverb |= convolve(
                        effects,
                        slot,
                        rooms,
                        voice.id,
                        &self.input,
                        fade,
                        &mut self.reflected,
                        &mut self.reverb_bus,
                    );
                }
                let through = if params.own_reverb && !toggles.occlusion {
                    0.0
                } else {
                    params.through
                };
                if through > 0.001 {
                    for (out, sample) in self.through.iter_mut().zip(&self.input) {
                        *out = sample * through;
                    }
                    reverb |= convolve(
                        &mut voice.room,
                        slot,
                        rooms,
                        LISTENER_REVERB,
                        &self.through,
                        fade,
                        &mut self.reflected,
                        &mut self.reverb_bus,
                    );
                }
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

/// Adds `input` convolved with source `id`'s room response to the ambisonic
/// `bus`: the latest result (`rooms[0]`, on effect `slot`) with a gain going
/// from `fade.0` to `fade.1` over the frame, and the previous one (on the
/// other effect) fading out correspondingly. False if neither had a response.
#[allow(clippy::too_many_arguments)]
fn convolve(
    effects: &mut [ReflectionEffect<Convolution>; 2],
    slot: usize,
    rooms: [Option<&ReflectionsOutput<u32, Convolution>>; 2],
    id: u32,
    input: &[f32],
    fade: (f32, f32),
    scratch: &mut [f32],
    bus: &mut [f32],
) -> bool {
    let n = input.len();
    let mut any = false;
    for (k, room) in rooms.into_iter().enumerate() {
        let Some(response) = room.and_then(|r| r.sources.get(&id)) else {
            continue;
        };
        let effect = &mut effects[if k == 0 { slot } else { 1 - slot }];
        let applied = match (
            AudioBufferRef::try_from(input),
            AudioBufferMut::try_new(scratch, AMBISONIC_CHANNELS),
        ) {
            (Ok(input), Ok(mut out)) => effect.apply(response, &input, &mut out).is_ok(),
            _ => false,
        };
        if !applied {
            continue;
        }
        for (i, (bus, sample)) in bus.iter_mut().zip(scratch.iter()).enumerate() {
            let t = (i % n) as f32 / n as f32;
            let gain = fade.0 + (fade.1 - fade.0) * t;
            *bus += sample * if k == 0 { gain } else { 1.0 - gain };
        }
        any = true;
    }
    any
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
