//! The time step the game advances by each frame.
//!
//! With vsync, frames reach the screen on whole refresh intervals, but the
//! CPU measures them with jitter around that (a 6.9 ms frame measured as
//! 6.2, then 7.6). Moving the world by the measured time while the screen
//! shows it at a steady rate makes motion judder. So a frame time close to
//! a whole number of refresh intervals is snapped to it; the difference is
//! carried to the next frame, so game time never drifts from the clock.

/// Within this fraction of a refresh interval counts as on the interval.
const SNAP: f32 = 0.2;
/// The longest step, in seconds (a stall doesn't launch the player).
const MAX_DT: f32 = 0.1;

#[derive(Default)]
pub(super) struct Pacing {
    /// Measured time not yet given to the game (or given ahead), seconds.
    carry: f32,
}

impl Pacing {
    /// The step for a frame measured at `measured` seconds. `refresh` is the
    /// display's refresh interval in seconds; `vsync` whether frames wait
    /// for it (without it nothing is snapped).
    pub(super) fn step(&mut self, measured: f32, refresh: f32, vsync: bool) -> f32 {
        let dt = measured + self.carry;
        let intervals = (dt / refresh).round();
        if vsync && intervals >= 1.0 && (dt - intervals * refresh).abs() < refresh * SNAP {
            let snapped = intervals * refresh;
            self.carry = dt - snapped;
            snapped.min(MAX_DT)
        } else {
            self.carry = 0.0;
            dt.min(MAX_DT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ144: f32 = 1.0 / 144.0;

    #[test]
    fn jitter_around_the_refresh_becomes_steady() {
        let mut pacing = Pacing::default();
        let measured = [0.0062, 0.0076, 0.0066, 0.0073, 0.0070];
        for m in measured {
            assert_eq!(pacing.step(m, HZ144, true), HZ144);
        }
    }

    #[test]
    fn game_time_keeps_up_with_the_clock() {
        let mut pacing = Pacing::default();
        // Slightly slower than the refresh, every frame.
        let (mut game, mut clock) = (0.0, 0.0);
        for _ in 0..1000 {
            clock += 0.0072;
            game += pacing.step(0.0072, HZ144, true);
        }
        assert!((game - clock).abs() < HZ144, "{game} vs {clock}");
    }

    #[test]
    fn missed_refreshes_and_no_vsync_are_kept() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.step(2.0 * HZ144 + 0.0003, HZ144, true), 2.0 * HZ144);
        let mut pacing = Pacing::default();
        assert_eq!(pacing.step(0.0072, HZ144, false), 0.0072);
        assert_eq!(pacing.step(0.5, HZ144, false), MAX_DT);
    }
}
