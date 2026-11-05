//! The audio thread's side. Every voice goes through Steam Audio's direct
//! effect (distance attenuation, air absorption) and HRTF binaural
//! rendering, then the mix through a master limiter. It never blocks: new
//! voices arrive over a channel, new parameters through a `try_lock`.

use super::clip::Clip;
use audionimbus::{
    AudioBufferMut, AudioBufferRef, BinauralEffect, BinauralEffectParams, DirectEffect,
    DirectEffectParams, Direction, Equalizer, Hrtf, HrtfInterpolation,
};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

/// The mix never exceeds this (about -1 dBFS).
const CEILING: f32 = 0.9;
/// Seconds for the limiter to let go after a loud peak.
const RELEASE: f32 = 0.25;

/// One playing sound and its effects' state.
pub struct Voice {
    pub clip: Clip,
    pub position: usize,
    pub direct: DirectEffect,
    pub binaural: BinauralEffect,
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
}

impl Default for VoiceParams {
    fn default() -> Self {
        Self {
            volume: 0.0,
            looping: true,
            direction: [0.0, 0.0, -1.0],
            distance_attenuation: 1.0,
            air_absorption: [1.0; 3],
        }
    }
}

pub struct Mixer {
    hrtf: Hrtf,
    frame_size: usize,
    voices: Vec<Voice>,
    params: Vec<VoiceParams>,
    new_voices: Receiver<Vec<Voice>>,
    shared_params: Arc<Mutex<Vec<VoiceParams>>>,
    /// The current frame (deinterleaved stereo) and how much of it is played.
    mix: Vec<f32>,
    cursor: usize,
    input: Vec<f32>,
    attenuated: Vec<f32>,
    spatial: Vec<f32>,
    limiter: Limiter,
}

impl Mixer {
    pub fn new(
        hrtf: Hrtf,
        frame_size: usize,
        rate: u32,
        new_voices: Receiver<Vec<Voice>>,
        shared_params: Arc<Mutex<Vec<VoiceParams>>>,
    ) -> Self {
        Self {
            hrtf,
            frame_size,
            voices: Vec::new(),
            params: Vec::new(),
            new_voices,
            shared_params,
            mix: vec![0.0; frame_size * 2],
            cursor: frame_size,
            input: vec![0.0; frame_size],
            attenuated: vec![0.0; frame_size],
            spatial: vec![0.0; frame_size * 2],
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

    /// Renders the next frame into `mix`.
    fn render(&mut self) {
        while let Ok(voices) = self.new_voices.try_recv() {
            self.voices = voices;
        }
        if let Ok(params) = self.shared_params.try_lock() {
            self.params.clone_from(&params);
        }
        self.mix.fill(0.0);
        let n = self.frame_size;
        for (voice, params) in self.voices.iter_mut().zip(&self.params) {
            if !read(voice, params, &mut self.input) {
                continue;
            }
            let direct = DirectEffectParams {
                distance_attenuation: Some(params.distance_attenuation),
                air_absorption: Some(Equalizer(params.air_absorption)),
                directivity: None,
                occlusion: None,
                transmission: None,
            };
            let binaural = BinauralEffectParams {
                direction: Direction::new(
                    params.direction[0],
                    params.direction[1],
                    params.direction[2],
                ),
                interpolation: HrtfInterpolation::Bilinear,
                spatial_blend: 1.0,
                hrtf: self.hrtf.clone(),
                peak_delays: None,
            };
            let (Ok(input), Ok(mut attenuated), Ok(mut spatial)) = (
                AudioBufferRef::try_from(&self.input[..]),
                AudioBufferMut::try_new(&mut self.attenuated, 1),
                AudioBufferMut::try_new(&mut self.spatial, 2),
            ) else {
                continue;
            };
            if voice
                .direct
                .apply(&direct, &input, &mut attenuated)
                .is_err()
            {
                continue;
            }
            let Ok(attenuated) = AudioBufferRef::try_from(&self.attenuated[..]) else {
                continue;
            };
            if voice
                .binaural
                .apply(&binaural, &attenuated, &mut spatial)
                .is_err()
            {
                continue;
            }
            for (mix, sample) in self.mix.iter_mut().zip(&self.spatial[..2 * n]) {
                *mix += sample;
            }
        }
        let (left, right) = self.mix.split_at_mut(n);
        self.limiter.process(left, right);
    }
}

/// The voice's next frame of samples into `out`, scaled by its volume.
/// False once a one-shot clip has finished.
fn read(voice: &mut Voice, params: &VoiceParams, out: &mut [f32]) -> bool {
    let clip = &voice.clip;
    if clip.is_empty() || (!params.looping && voice.position >= clip.len()) {
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
