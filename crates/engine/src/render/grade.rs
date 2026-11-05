//! Color grading as a 3D lookup table (LUT): a small cube of colors where
//! the tone-mapped color is the coordinate and the graded color is what's
//! stored. Any chain of color tweaks becomes one texture lookup, and a
//! `.cube` file from a grading tool can be appended to the chain.

use crate::scene::Grade;
use glam::Vec3;
use std::path::{Path, PathBuf};

/// Texels per side of the LUT texture.
pub const LUT_SIZE: u32 = 32;

#[derive(Debug, thiserror::Error)]
pub enum GradeError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {reason}")]
    Parse { path: PathBuf, reason: String },
}

/// A 3D LUT read from a `.cube` file (Adobe/Resolve format).
#[derive(Debug, Clone, PartialEq)]
pub struct CubeLut {
    size: usize,
    domain: (Vec3, Vec3),
    /// Red changes fastest, then green, then blue.
    data: Vec<Vec3>,
}

impl CubeLut {
    pub fn load(path: &Path) -> Result<Self, GradeError> {
        let text = std::fs::read_to_string(path).map_err(|source| GradeError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text).map_err(|reason| GradeError::Parse {
            path: path.to_owned(),
            reason,
        })
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut size = None;
        let mut domain = (Vec3::ZERO, Vec3::ONE);
        let mut data = Vec::new();
        let vec3 = |words: &[&str]| -> Result<Vec3, String> {
            let v: Vec<f32> = words
                .iter()
                .map(|w| w.parse::<f32>().map_err(|e| format!("{w}: {e}")))
                .collect::<Result<_, _>>()?;
            match v[..] {
                [x, y, z] => Ok(Vec3::new(x, y, z)),
                _ => Err(format!("expected three numbers, got {}", v.len())),
            }
        };
        for line in text.lines() {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.first().copied() {
                None => {}
                Some(w) if w.starts_with('#') => {}
                Some("TITLE") | Some("LUT_1D_SIZE") | Some("LUT_1D_INPUT_RANGE") => {
                    if words[0] == "LUT_1D_SIZE" {
                        return Err("1D LUTs are not supported".into());
                    }
                }
                Some("LUT_3D_SIZE") => {
                    let n: usize = words
                        .get(1)
                        .and_then(|w| w.parse().ok())
                        .ok_or("bad LUT_3D_SIZE")?;
                    if !(2..=256).contains(&n) {
                        return Err(format!("LUT_3D_SIZE {n} out of range"));
                    }
                    size = Some(n);
                }
                Some("DOMAIN_MIN") => domain.0 = vec3(&words[1..])?,
                Some("DOMAIN_MAX") => domain.1 = vec3(&words[1..])?,
                Some(_) => data.push(vec3(&words)?),
            }
        }
        let size = size.ok_or("no LUT_3D_SIZE")?;
        if data.len() != size * size * size {
            return Err(format!(
                "expected {} entries, found {}",
                size * size * size,
                data.len()
            ));
        }
        Ok(Self { size, domain, data })
    }

    /// Trilinear lookup.
    pub fn sample(&self, color: Vec3) -> Vec3 {
        let range = (self.domain.1 - self.domain.0).max(Vec3::splat(1e-6));
        let max = (self.size - 1) as f32;
        let p = ((color - self.domain.0) / range).clamp(Vec3::ZERO, Vec3::ONE) * max;
        let i = p.floor().min(Vec3::splat(max - 1.0));
        let f = p - i;
        let (x, y, z) = (i.x as usize, i.y as usize, i.z as usize);
        let at = |dx, dy, dz| {
            self.data[(x + dx) + (y + dy) * self.size + (z + dz) * self.size * self.size]
        };
        let lerp = |a: Vec3, b: Vec3, t: f32| a + (b - a) * t;
        let c00 = lerp(at(0, 0, 0), at(1, 0, 0), f.x);
        let c10 = lerp(at(0, 1, 0), at(1, 1, 0), f.x);
        let c01 = lerp(at(0, 0, 1), at(1, 0, 1), f.x);
        let c11 = lerp(at(0, 1, 1), at(1, 1, 1), f.x);
        lerp(lerp(c00, c10, f.y), lerp(c01, c11, f.y), f.z)
    }
}

/// What one graded color becomes (input and output display-encoded, 0–1).
pub fn apply(grade: &Grade, lut: Option<&CubeLut>, color: Vec3) -> Vec3 {
    // Contrast pivots around a display-space mid grey.
    const PIVOT: f32 = 0.45;
    let mut c = (color - PIVOT) * grade.contrast + PIVOT;
    let luma = c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    c = Vec3::splat(luma) + (c - luma) * grade.saturation;
    c = (c * grade.tint).clamp(Vec3::ZERO, Vec3::ONE);
    match lut {
        Some(lut) => lut.sample(c).clamp(Vec3::ZERO, Vec3::ONE),
        None => c,
    }
}

/// The grade as RGBA8 texels of a [`LUT_SIZE`]³ 3D texture.
pub fn bake(grade: &Grade, lut: Option<&CubeLut>) -> Vec<u8> {
    let n = LUT_SIZE as usize;
    let max = (n - 1) as f32;
    let mut out = Vec::with_capacity(n * n * n * 4);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let input = Vec3::new(r as f32, g as f32, b as f32) / max;
                let c = apply(grade, lut, input);
                out.extend_from_slice(&[
                    (c.x * 255.0).round() as u8,
                    (c.y * 255.0).round() as u8,
                    (c.z * 255.0).round() as u8,
                    255,
                ]);
            }
        }
    }
    out
}

/// The grade's `.cube` file, if it names one and it loads (problems are
/// logged; the grade then works without it).
pub fn load_lut(grade: &Grade, project: &Path) -> Option<CubeLut> {
    if grade.lut.is_empty() {
        return None;
    }
    CubeLut::load(&project.join(&grade.lut))
        .inspect_err(|err| log::warn!("{err}"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY_2: &str = "TITLE \"identity\"\n# comment\nLUT_3D_SIZE 2\n\
        0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";

    #[test]
    fn default_grade_is_identity() {
        let grade = Grade::default();
        for c in [Vec3::ZERO, Vec3::ONE, Vec3::new(0.2, 0.5, 0.9)] {
            assert!((apply(&grade, None, c) - c).length() < 1e-5);
        }
    }

    #[test]
    fn cube_file_parses_and_samples() {
        let lut = CubeLut::parse(IDENTITY_2).unwrap();
        let c = Vec3::new(0.25, 0.5, 0.75);
        assert!((lut.sample(c) - c).length() < 1e-5);
    }

    #[test]
    fn bad_cube_files_are_errors() {
        assert!(CubeLut::parse("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(CubeLut::parse("0 0 0\n").is_err());
        assert!(CubeLut::parse("LUT_3D_SIZE 2\n0 0 zero\n").is_err());
    }

    #[test]
    fn zero_saturation_is_grey() {
        let grade = Grade {
            saturation: 0.0,
            ..Default::default()
        };
        let c = apply(&grade, None, Vec3::new(0.9, 0.2, 0.1));
        assert!((c.x - c.y).abs() < 1e-5 && (c.y - c.z).abs() < 1e-5);
    }

    #[test]
    fn baked_lut_has_every_texel() {
        let data = bake(&Grade::default(), None);
        assert_eq!(data.len(), (LUT_SIZE * LUT_SIZE * LUT_SIZE * 4) as usize);
        // Last texel = white.
        assert_eq!(&data[data.len() - 4..], &[255, 255, 255, 255]);
    }
}
