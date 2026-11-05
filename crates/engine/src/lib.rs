//! Core engine runtime.
//!
//! Everything a game needs at runtime lives here, plus the `editor` module
//! (the same runtime with panels on top, opened with `--editor`); the `app`
//! crate is just the binary. Planned modules (add them only when a milestone
//! needs them, see `docs/ROADMAP.md`):
//!
//! - `render`    render scale, baked GI, volumetric fog, shadows, post
//! - `audio`     Steam Audio (via AudioNimbus), acoustic materials, limiter
//! - `music`     global key/scale/tempo, motifs, instrument layers
//! - `scene`     entities, reflected components, RON load/save
//! - `zones`     zone graph, culling, simulation radius
//! - `behaviors` Floating, Door, Switch, PhysicsBody, Singer, ...
//! - `player`    character controller, camera, abilities

pub mod assets;
pub mod camera;
pub mod editor;
pub mod input;
pub mod physics;
pub mod player;
pub mod project;
pub mod render;
pub mod runtime;
pub mod scene;
pub mod ui;

/// Re-exported so games and the app use the same math types as the engine.
pub use glam;
pub use runtime::{RunConfig, RunError, run};

/// Engine name, used in window titles and logs. Working name, may change.
pub const NAME: &str = "engine";

/// Engine version, taken from Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
