//! Sound clips: mono samples at the output's sample rate, loaded from WAV
//! files in the game project, or generated.

use std::path::Path;
use std::sync::Arc;

pub type Clip = Arc<Vec<f32>>;

#[derive(Debug, thiserror::Error)]
pub enum ClipError {
    #[error("{path}: {source}")]
    Wav { path: String, source: hound::Error },
    #[error("{0}: empty")]
    Empty(String),
}

/// Loads a WAV file, mixed down to mono and resampled to `rate`.
pub fn load_wav(path: &Path, rate: u32) -> Result<Clip, ClipError> {
    let wav_error = |source| ClipError::Wav {
        path: path.display().to_string(),
        source,
    };
    let reader = hound::WavReader::open(path).map_err(wav_error)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(wav_error)?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|s| s as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(wav_error)?
        }
    };
    let channels = spec.channels.max(1) as usize;
    let mono: Vec<f32> = samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    if mono.is_empty() {
        return Err(ClipError::Empty(path.display().to_string()));
    }
    Ok(Arc::new(resample(&mono, spec.sample_rate, rate)))
}

/// Linear-interpolation resampling; good enough for game sounds.
fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return samples.to_vec();
    }
    let step = from as f64 / to as f64;
    let len = (samples.len() as f64 / step) as usize;
    (0..len)
        .map(|i| {
            let t = i as f64 * step;
            let a = t as usize;
            let b = (a + 1).min(samples.len() - 1);
            let f = (t - a as f64) as f32;
            samples[a] * (1.0 - f) + samples[b] * f
        })
        .collect()
}

/// A short plucked-string phrase (Karplus–Strong), for sources without a
/// clip: sharp attacks and decaying tails make echoes and reverb easy to hear.
pub fn plucked_loop(rate: u32) -> Clip {
    // An E minor arpeggio, one note every 0.5 s, then a beat of rest.
    const NOTES: [f32; 6] = [164.81, 196.0, 246.94, 329.63, 246.94, 196.0];
    let note_len = rate as usize / 2;
    let mut out = vec![0.0; note_len * (NOTES.len() + 2)];
    let mut noise = 0x2545_f491_u32;
    for (i, frequency) in NOTES.iter().enumerate() {
        let period = (rate as f32 / frequency) as usize;
        let mut delay: Vec<f32> = (0..period)
            .map(|_| {
                // xorshift: deterministic noise to excite the string.
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                noise as f32 / u32::MAX as f32 * 2.0 - 1.0
            })
            .collect();
        let start = i * note_len;
        // Each note rings on under the next ones.
        for n in 0..note_len * 3 {
            let k = n % period;
            let next = delay[(k + 1) % period];
            let sample = delay[k];
            delay[k] = 0.996 * 0.5 * (sample + next);
            if let Some(slot) = out.get_mut(start + n) {
                *slot += sample * 0.3;
            }
        }
    }
    Arc::new(out)
}

/// Deterministic white noise in -1..1 (xorshift).
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

/// A hand clap: a few quick bursts of bright noise. Sharp enough to excite
/// a room's echoes clearly.
pub fn clap(rate: u32) -> Clip {
    let len = rate as usize / 5;
    let mut noise = Noise(0x9e37_79b9);
    let mut out = vec![0.0; len];
    let (mut low, mut previous) = (0.0, 0.0);
    for (i, sample) in out.iter_mut().enumerate() {
        let t = i as f32 / rate as f32;
        // Three bursts 8–9 ms apart, each decaying in a few milliseconds.
        let envelope: f32 = [0.0, 0.008, 0.017]
            .iter()
            .filter(|&&start| t >= start)
            .map(|start| (-(t - start) / 0.006).exp())
            .sum();
        let x = noise.next() * envelope;
        // High-pass: a clap has little low end.
        low += 0.12 * (x - low);
        let bright = x - low;
        *sample = 0.6 * (bright + previous) * 0.5;
        previous = bright;
    }
    Arc::new(out)
}

/// A footstep on `material` (a preset name, lowercase). `variant` picks one
/// of several slightly different steps so walking doesn't sound looped.
pub fn footstep(material: &str, variant: u32, rate: u32) -> Clip {
    // brightness: one-pole low-pass amount (higher = brighter);
    // decay: seconds; thump: low "body" of the step; ring: metallic tone.
    let (brightness, decay, thump, ring, gain, crunch) = match material {
        "wood" => (0.15, 0.030, 0.5, 0.0, 0.8, false),
        "marble" | "glass" => (0.70, 0.015, 0.2, 0.0, 0.7, false),
        "carpet" | "velvet" => (0.05, 0.040, 0.25, 0.0, 0.4, false),
        "metal" => (0.60, 0.020, 0.3, 0.15, 0.7, false),
        "water" => (0.30, 0.120, 0.1, 0.0, 0.5, false),
        "gravel" => (0.50, 0.020, 0.2, 0.0, 0.7, true),
        // stone, concrete, brick, plaster, generic
        _ => (0.40, 0.020, 0.3, 0.0, 0.7, false),
    };
    let len = rate as usize * 3 / 10;
    let mut noise = Noise(0x1234_5678 ^ variant.wrapping_mul(0x85eb_ca6b).max(1));
    // Gravel: many tiny clicks over 120 ms instead of one impact.
    let clicks: Vec<f32> = if crunch {
        (0..24).map(|_| (noise.next() * 0.5 + 0.5) * 0.12).collect()
    } else {
        vec![0.0]
    };
    let pitch = 1.0 + 0.06 * noise.next();
    let mut out = vec![0.0; len];
    let mut low = 0.0;
    for (i, sample) in out.iter_mut().enumerate() {
        let t = i as f32 / rate as f32;
        let envelope: f32 = clicks
            .iter()
            .filter(|&&start| t >= start)
            .map(|start| (-(t - start) / decay).exp())
            .sum::<f32>()
            / clicks.len().max(1) as f32
            * if crunch { 6.0 } else { 1.0 };
        low += brightness * (noise.next() * envelope - low);
        let body = thump * (-t / 0.05).exp() * (std::f32::consts::TAU * 80.0 * pitch * t).sin();
        let tone = ring * (-t / 0.15).exp() * (std::f32::consts::TAU * 900.0 * pitch * t).sin();
        *sample = low * 2.0 + body + tone;
    }
    // Same loudness on every surface, apart from `gain` (soft floors quieter).
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
    for sample in &mut out {
        *sample *= gain / peak;
    }
    Arc::new(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_sounds_are_audible_and_in_range() {
        let peak = |clip: &Clip| clip.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((0.05..1.0).contains(&peak(&clap(48_000))));
        for material in [
            "wood", "stone", "marble", "carpet", "metal", "water", "gravel",
        ] {
            let step = footstep(material, 0, 48_000);
            assert!(
                (0.02..1.0).contains(&peak(&step)),
                "{material}: {}",
                peak(&step)
            );
        }
        assert_ne!(footstep("wood", 0, 48_000), footstep("wood", 1, 48_000));
    }

    #[test]
    fn resampling_keeps_the_duration() {
        let one_second = vec![0.5; 44_100];
        let out = resample(&one_second, 44_100, 48_000);
        assert_eq!(out.len(), 48_000);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn plucked_loop_is_audible_and_in_range() {
        let clip = plucked_loop(48_000);
        assert_eq!(clip.len(), 48_000 * 4);
        let peak = clip.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1 && peak < 1.0, "peak {peak}");
    }

    #[test]
    fn wav_files_load_as_mono_at_the_output_rate() {
        let path = std::env::temp_dir().join("engine-clip-test.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..24_000 {
            writer.write_sample(i16::MAX / 2).unwrap();
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        let clip = load_wav(&path, 48_000).unwrap();
        assert_eq!(clip.len(), 48_000);
        assert!((clip[100] - 0.25).abs() < 1e-3, "left 0.5 + right 0 → 0.25");
    }
}
