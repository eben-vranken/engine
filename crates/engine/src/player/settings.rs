//! Movement and camera properties, stored in the game project's `player.ron`.
//! Units: meters, seconds, degrees.
//!
//! The defaults here are neutral fallbacks so a missing or partial file still
//! works; each game tunes its own values in its `player.ron`.

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, Reflect)]
#[serde(default)]
pub struct PlayerSettings {
    pub movement: Movement,
    pub jump: Jump,
    pub body: Body,
    pub camera: CameraFeel,
    pub footsteps: Footsteps,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Movement {
    /// m/s
    pub walk_speed: f32,
    /// m/s
    pub sprint_speed: f32,
    /// m/s
    pub crouch_speed: f32,
    /// m/s² while on the ground with input held.
    pub ground_acceleration: f32,
    /// m/s² while on the ground with no input.
    pub ground_deceleration: f32,
    /// m/s² while airborne.
    pub air_acceleration: f32,
    /// 0–1: how much you can steer and brake in the air (speeding up along
    /// your current direction is always allowed).
    pub air_control: f32,
}

impl Default for Movement {
    fn default() -> Self {
        Self {
            walk_speed: 4.5,
            sprint_speed: 7.5,
            crouch_speed: 2.2,
            ground_acceleration: 50.0,
            ground_deceleration: 40.0,
            air_acceleration: 20.0,
            air_control: 0.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Jump {
    /// m: apex height of a full jump.
    pub height: f32,
    /// s: time from takeoff to apex. Gravity is derived from this and `height`.
    pub time_to_apex: f32,
    /// Gravity multiplier while falling.
    pub fall_gravity_multiplier: f32,
    /// Gravity multiplier while rising after jump is released (variable height).
    pub jump_cut_gravity_multiplier: f32,
    /// m/s
    pub max_fall_speed: f32,
    /// s: jumping is still allowed this long after walking off a ledge.
    pub coyote_time: f32,
    /// s: a jump pressed this long before landing still happens.
    pub buffer_time: f32,
}

impl Default for Jump {
    fn default() -> Self {
        Self {
            height: 1.25,
            time_to_apex: 0.38,
            fall_gravity_multiplier: 1.7,
            jump_cut_gravity_multiplier: 2.5,
            max_fall_speed: 45.0,
            coyote_time: 0.12,
            buffer_time: 0.12,
        }
    }
}

impl Jump {
    /// Rising gravity, m/s² (positive).
    pub fn gravity(&self) -> f32 {
        2.0 * self.height / (self.time_to_apex * self.time_to_apex).max(1e-4)
    }

    /// Takeoff speed, m/s.
    pub fn takeoff_speed(&self) -> f32 {
        2.0 * self.height / self.time_to_apex.max(1e-4)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Body {
    /// m: standing height.
    pub height: f32,
    /// m
    pub crouch_height: f32,
    /// m
    pub radius: f32,
    /// m: eyes are this far below the top of the head.
    pub eye_offset: f32,
    /// m: tallest step walked up without jumping.
    pub step_height: f32,
    /// degrees: steepest walkable slope.
    pub max_slope: f32,
    /// m: how far down the body sticks to the ground (stairs, slope crests).
    pub ground_snap: f32,
    /// kg: how hard the player shoves physics bodies (a body this heavy
    /// moves at half the player's speed).
    pub mass: f32,
    /// m: how far away the player can use things (E).
    pub reach: f32,
}

impl Default for Body {
    fn default() -> Self {
        Self {
            height: 1.8,
            crouch_height: 1.1,
            radius: 0.3,
            eye_offset: 0.12,
            step_height: 0.35,
            max_slope: 46.0,
            ground_snap: 0.3,
            mass: 80.0,
            reach: 2.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct CameraFeel {
    /// degrees, vertical.
    pub fov: f32,
    /// degrees per pixel of mouse movement.
    pub sensitivity: f32,
    pub head_bob: bool,
    /// m
    pub bob_amplitude: f32,
    /// m walked per step: one head bob down and up, and one footstep. Smaller
    /// = quicker steps.
    pub bob_stride: f32,
    /// m: how far the view dips on a hard landing (scaled by impact speed).
    pub landing_dip: f32,
    /// s: time to the bottom of the dip.
    pub landing_dip_time: f32,
    /// degrees added to the FOV while sprinting.
    pub sprint_fov_kick: f32,
    /// s: time to blend the FOV kick in and out.
    pub fov_kick_time: f32,
    /// s: how long the view takes to catch up with steps and slope crests.
    pub step_smoothing: f32,
}

impl Default for CameraFeel {
    fn default() -> Self {
        Self {
            fov: 75.0,
            sensitivity: 0.08,
            head_bob: true,
            bob_amplitude: 0.035,
            bob_stride: 1.9,
            landing_dip: 0.12,
            landing_dip_time: 0.09,
            sprint_fov_kick: 6.0,
            fov_kick_time: 0.25,
            step_smoothing: 0.06,
        }
    }
}

/// Which sounds the player's footsteps make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Footsteps {
    /// The engine's own steps, one per floor material (a project can replace
    /// them with `audio/footsteps/<material>.wav`).
    pub use_default: bool,
    /// Loudness of every step, default or own clips: 1 = as recorded (the
    /// default steps' level), 0 = silent, 2 = twice as loud.
    pub volume: f32,
    /// WAV files in the project (paths relative to it), used instead of the
    /// default steps when `use_default` is off.
    pub clips: Vec<String>,
    /// Off: the clips play in list order. On: in the order `random` picks.
    pub randomize: bool,
    pub random: RandomOrder,
}

impl Default for Footsteps {
    fn default() -> Self {
        Self {
            use_default: true,
            volume: 1.0,
            clips: Vec::new(),
            randomize: false,
            random: RandomOrder::Smart,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum RandomOrder {
    /// Any clip each step, so the same one can play twice in a row.
    True,
    /// Never one of the most recent clips (the last half of the list), and
    /// each step slightly different in pitch and loudness, so even a few
    /// clips don't sound like a loop.
    Smart,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<ron::error::SpannedError>,
    },
    #[error("could not serialize settings: {0}")]
    Serialize(#[from] ron::Error),
}

impl PlayerSettings {
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        let text = std::fs::read_to_string(path).map_err(|source| SettingsError::Read {
            path: path.to_owned(),
            source,
        })?;
        ron::from_str(&text).map_err(|source| SettingsError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;
        std::fs::write(path, text + "\n").map_err(|source| SettingsError::Write {
            path: path.to_owned(),
            source,
        })
    }
}

/// `player.ron` on disk, watched for changes by polling its modified time.
pub struct SettingsFile {
    pub path: PathBuf,
    modified: Option<SystemTime>,
}

impl SettingsFile {
    pub fn new(path: PathBuf) -> Self {
        let modified = modified_time(&path);
        Self { path, modified }
    }

    /// Loads the file; falls back to defaults if it doesn't exist yet.
    pub fn load(&self) -> Result<PlayerSettings, SettingsError> {
        if self.path.exists() {
            PlayerSettings::load(&self.path)
        } else {
            Ok(PlayerSettings::default())
        }
    }

    pub fn save(&mut self, settings: &PlayerSettings) -> Result<(), SettingsError> {
        settings.save(&self.path)?;
        self.modified = modified_time(&self.path);
        Ok(())
    }

    /// Returns the reloaded settings if the file changed since last time.
    pub fn poll(&mut self) -> Option<Result<PlayerSettings, SettingsError>> {
        let modified = modified_time(&self.path);
        if modified == self.modified || modified.is_none() {
            return None;
        }
        self.modified = modified;
        Some(PlayerSettings::load(&self.path))
    }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_ron() {
        let settings = PlayerSettings::default();
        let text = ron::ser::to_string_pretty(&settings, Default::default()).unwrap();
        let back: PlayerSettings = ron::from_str(&text).unwrap();
        assert_eq!(settings, back);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let settings: PlayerSettings = ron::from_str("(jump: (height: 2.0))").unwrap();
        assert_eq!(settings.jump.height, 2.0);
        assert_eq!(settings.jump.time_to_apex, Jump::default().time_to_apex);
        assert_eq!(settings.movement, Movement::default());
    }

    #[test]
    fn jump_reaches_its_height() {
        let jump = Jump::default();
        // v² = 2 g h  →  apex = v² / 2g
        let apex = jump.takeoff_speed().powi(2) / (2.0 * jump.gravity());
        assert!((apex - jump.height).abs() < 1e-4);
    }
}
