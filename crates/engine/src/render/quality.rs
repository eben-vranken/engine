//! Quality presets: how much GPU time goes into shadows and fog, and how
//! many lines the scene is rendered at. HIGH is the look as designed; LOW
//! keeps it smooth on integrated graphics at the cost of softer shadows and
//! coarser fog.

/// A preset, picked at startup (`--quality low`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    #[default]
    High,
    Low,
}

/// What a preset sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Texels per side of each sun shadow cascade.
    pub sun_shadow: u32,
    /// Texels per side of each point-light cube face.
    pub point_shadow: u32,
    /// Most point lights with shadows at once (the nearest that want
    /// one). Each takes six faces of memory; a moving one is re-rendered
    /// every frame.
    pub shadowed_lights: usize,
    /// Fog cells across, down and deep.
    pub fog_grid: [u32; 3],
    /// Fog samples per cell per frame.
    pub fog_samples: u32,
    /// The automatic render scale keeps the internal height at or under
    /// this many lines.
    pub max_lines: u32,
}

impl Quality {
    pub fn settings(self) -> Settings {
        match self {
            Quality::High => Settings {
                sun_shadow: 2048,
                point_shadow: 512,
                shadowed_lights: 16,
                fog_grid: [192, 108, 96],
                fog_samples: 4,
                max_lines: 720,
            },
            Quality::Low => Settings {
                sun_shadow: 1024,
                point_shadow: 256,
                shadowed_lights: 8,
                fog_grid: [128, 72, 64],
                fog_samples: 2,
                // Measured on integrated graphics: the scene itself is
                // cheap, so LOW keeps the resolution and saves on fog.
                max_lines: 720,
            },
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Quality::High => "HIGH",
            Quality::Low => "LOW",
        }
    }
}

impl std::str::FromStr for Quality {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "high" => Ok(Quality::High),
            "low" => Ok(Quality::Low),
            other => Err(format!("unknown quality \"{other}\" (high or low)")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_does_less_than_high() {
        let (h, l) = (Quality::High.settings(), Quality::Low.settings());
        assert!(l.sun_shadow < h.sun_shadow && l.point_shadow < h.point_shadow);
        assert!(l.shadowed_lights <= h.shadowed_lights);
        assert!(l.fog_grid.iter().product::<u32>() < h.fog_grid.iter().product::<u32>());
        assert!(l.max_lines <= h.max_lines);
        assert_eq!("LOW".parse::<Quality>(), Ok(Quality::Low));
        assert!("ultra".parse::<Quality>().is_err());
    }
}
