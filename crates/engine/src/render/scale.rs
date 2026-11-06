//! Render scale: the scene is drawn at output ÷ an integer factor, then
//! upscaled with nearest-neighbor so every internal pixel becomes an exact
//! `factor × factor` block. Leftover output pixels become a black letterbox.

/// Internal heights above this get a bigger factor by default (HIGH
/// quality; presets can lower it).
pub const MAX_DEFAULT_HEIGHT: u32 = 720;

/// Highest factor the user can pick; beyond this the image is mush.
pub const MAX_FACTOR: u32 = 8;

/// The factor that puts the internal height at or just under 720 lines:
/// 1080p → 2 (540), 1440p → 2 (720), 4K → 3 (720).
pub fn default_factor(output_height: u32) -> u32 {
    factor_for_lines(output_height, MAX_DEFAULT_HEIGHT)
}

/// The smallest factor that keeps the internal height at or under
/// `max_lines`.
pub fn factor_for_lines(output_height: u32, max_lines: u32) -> u32 {
    output_height.div_ceil(max_lines.max(1)).max(1)
}

/// How the internal image maps onto the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaleLayout {
    pub factor: u32,
    /// Size of the internal render target.
    pub internal: [u32; 2],
    /// Top-left of the upscaled image on the output (letterbox offset).
    pub origin: [u32; 2],
}

impl ScaleLayout {
    pub fn new(output: [u32; 2], factor: u32) -> Self {
        let factor = factor.clamp(1, MAX_FACTOR);
        let internal = [(output[0] / factor).max(1), (output[1] / factor).max(1)];
        let origin = [
            output[0].saturating_sub(internal[0] * factor) / 2,
            output[1].saturating_sub(internal[1] * factor) / 2,
        ];
        Self {
            factor,
            internal,
            origin,
        }
    }

    /// Size of the upscaled image on the output.
    pub fn scaled(&self) -> [u32; 2] {
        [
            self.internal[0] * self.factor,
            self.internal[1] * self.factor,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_factor_matches_brief() {
        assert_eq!(default_factor(1080), 2);
        assert_eq!(default_factor(1440), 2);
        assert_eq!(default_factor(2160), 3);
        assert_eq!(default_factor(720), 1);
        assert_eq!(default_factor(1), 1);
    }

    #[test]
    fn even_output_has_no_letterbox() {
        let layout = ScaleLayout::new([1920, 1080], 2);
        assert_eq!(layout.internal, [960, 540]);
        assert_eq!(layout.origin, [0, 0]);
    }

    #[test]
    fn uneven_output_is_centered() {
        let layout = ScaleLayout::new([1001, 601], 2);
        assert_eq!(layout.internal, [500, 300]);
        assert_eq!(layout.scaled(), [1000, 600]);
        assert_eq!(layout.origin, [0, 0]);

        let layout = ScaleLayout::new([1005, 607], 3);
        assert_eq!(layout.internal, [335, 202]);
        assert_eq!(layout.origin, [0, 0]);

        let layout = ScaleLayout::new([1007, 607], 4);
        assert_eq!(layout.internal, [251, 151]);
        assert_eq!(layout.origin, [1, 1]);
    }

    #[test]
    fn tiny_window_still_has_a_pixel() {
        let layout = ScaleLayout::new([3, 3], 8);
        assert_eq!(layout.internal, [1, 1]);
    }
}
