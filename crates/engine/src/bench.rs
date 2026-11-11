//! A route the player walks on its own (`--bench <route.ron>`), so the
//! same path can be measured before and after a change without anyone at
//! the keyboard. Each stop is walked to in a straight line (so routes go
//! through doorways), then the player waits there; it can look at a point
//! and press E (take a pickup, pull a lever). The game quits at the end.
//!
//! Routes can also check movement: a stop with `height` must be reached
//! with the feet at that height, and walking that stalls (the player
//! barely moving for half a second, e.g. snagged on a step) counts as a
//! failure. Failures are logged as errors and the app exits with code 1.
//!
//! ```ron
//! (stops: [
//!     (name: "start", to: (0.0, 6.0), wait: 10.0),
//!     (name: "pickup", to: (8.0, 18.0), look: Some((10.0, 0.4, 20.0)), use: true, wait: 5.0),
//! ])
//! ```

use crate::input::Input;
use crate::player::Player;
use glam::{Vec2, Vec3, Vec3Swizzles};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Route {
    pub stops: Vec<Stop>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Stop {
    pub name: String,
    /// Where to walk, on the ground plane (x, z).
    pub to: (f32, f32),
    /// Seconds to stand there.
    #[serde(default)]
    pub wait: f32,
    /// A point to look at on arrival.
    #[serde(default)]
    pub look: Option<(f32, f32, f32)>,
    /// Press E on arrival.
    #[serde(default, rename = "use")]
    pub press_use: bool,
    /// The feet must be at this height (meters, within 5 cm) on arrival.
    #[serde(default)]
    pub height: Option<f32>,
    /// How close counts as arrived (meters on the ground; default 0.4).
    #[serde(default)]
    pub within: Option<f32>,
}

/// Set when a bench check failed, for the app's exit code.
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a bench check failed (a missed height, a stall, a stop given up
/// on).
pub fn failed() -> bool {
    FAILED.load(std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    #[error("bench route {0}: {1}")]
    Read(String, std::io::Error),
    #[error("bench route {0}: {1}")]
    Parse(String, String),
}

impl Route {
    pub fn load(path: &Path) -> Result<Self, BenchError> {
        let name = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|e| BenchError::Read(name.clone(), e))?;
        ron::from_str(&text).map_err(|e| BenchError::Parse(name, e.to_string()))
    }
}

/// Closer than this (metres, on the ground) counts as arrived.
const ARRIVED: f32 = 0.4;
/// Slower than this along the ground (m/s) while walking counts as stalled
/// after `STALL` seconds; the first second of a walk (speeding up, turning)
/// is left out.
const STALLED_SPEED: f32 = 0.3;
const STALL: f32 = 0.5;
/// How far off a stop's `height` the feet may be (meters).
const HEIGHT_TOLERANCE: f32 = 0.05;

enum Phase {
    /// `limit`: seconds before giving up, set on the walk's first frame.
    Walking {
        time: f32,
        limit: Option<f32>,
    },
    Waiting {
        left: f32,
    },
}

/// Walks a `Route`.
pub struct Bench {
    route: Route,
    stop: usize,
    phase: Phase,
    /// How long the player has been stalled on this walk, and whether it
    /// was reported.
    stalled: (f32, bool),
    failures: Vec<String>,
}

impl Bench {
    pub fn new(route: Route) -> Self {
        if let Some(first) = route.stops.first() {
            log::info!("bench: walking to {}", first.name);
        }
        Self {
            route,
            stop: 0,
            phase: Phase::Walking {
                time: 0.0,
                limit: None,
            },
            stalled: (0.0, false),
            failures: Vec::new(),
        }
    }

    fn fail(&mut self, what: String) {
        log::error!("bench: FAILED: {what}");
        FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        self.failures.push(what);
    }

    /// This frame's input; turns the player toward the stop. `None` when
    /// the route is done.
    pub fn drive(
        &mut self,
        player: &mut Player,
        keys: &crate::player::settings::Keys,
        eye_height: f32,
        dt: f32,
    ) -> Option<Input> {
        let stop = self.route.stops.get(self.stop)?.clone();
        let mut input = Input::default();
        let feet = player.feet;
        match &mut self.phase {
            Phase::Walking { time, limit } => {
                *time += dt;
                let walking = *time > 1.0;
                let to = Vec2::new(stop.to.0, stop.to.1) - player.feet.xz();
                // A blocked path: give up after twice the walk at 4 m/s, plus 3 s.
                let limit = *limit.get_or_insert(3.0 + to.length() / 2.0);
                let stuck = *time > limit;
                if to.length() > stop.within.unwrap_or(ARRIVED) && !stuck {
                    player.yaw = to.x.atan2(-to.y);
                    player.pitch = 0.0;
                    input.press_bound(&keys.forward);
                    let speed = player.velocity.xz().length();
                    if walking && speed < STALLED_SPEED {
                        self.stalled.0 += dt;
                    } else {
                        self.stalled.0 = 0.0;
                    }
                    if self.stalled.0 > STALL && !self.stalled.1 {
                        self.stalled.1 = true;
                        self.fail(format!(
                            "stalled on the way to {} at ({:.2}, {:.2}, {:.2})",
                            stop.name, feet.x, feet.y, feet.z
                        ));
                    }
                    return Some(input);
                }
                log::info!(
                    "bench: {} {} at ({:.1}, {:.1}, {:.1})",
                    if stuck { "gave up on" } else { "reached" },
                    stop.name,
                    player.feet.x,
                    player.feet.y,
                    player.feet.z
                );
                if stuck {
                    self.fail(format!("never reached {}", stop.name));
                }
                if let Some(want) = stop.height
                    && (feet.y - want).abs() > HEIGHT_TOLERANCE
                {
                    self.fail(format!(
                        "{}: feet at {:.2} m, not {want:.2} m",
                        stop.name, feet.y
                    ));
                }
                if let Some(at) = stop.look {
                    let eye = player.feet + Vec3::Y * eye_height;
                    let d = Vec3::from(at) - eye;
                    player.yaw = d.x.atan2(-d.z);
                    player.pitch = d.y.atan2(d.xz().length());
                }
                if stop.press_use {
                    input.press_bound(&keys.interact);
                }
                self.phase = Phase::Waiting { left: stop.wait };
            }
            Phase::Waiting { left } => {
                *left -= dt;
                if *left <= 0.0 {
                    self.stop += 1;
                    self.phase = Phase::Walking {
                        time: 0.0,
                        limit: None,
                    };
                    self.stalled = (0.0, false);
                    match self.route.stops.get(self.stop) {
                        Some(next) => log::info!("bench: walking to {}", next.name),
                        None if self.failures.is_empty() => {
                            log::info!("bench: done, every check passed")
                        }
                        None => log::error!(
                            "bench: done, {} CHECKS FAILED: {}",
                            self.failures.len(),
                            self.failures.join("; ")
                        ),
                    }
                }
            }
        }
        Some(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_route_loads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/bench.ron");
        let route = Route::load(&path).unwrap();
        assert!(route.stops.len() > 3);
    }
}
