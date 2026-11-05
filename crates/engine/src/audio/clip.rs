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

#[cfg(test)]
mod tests {
    use super::*;

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
