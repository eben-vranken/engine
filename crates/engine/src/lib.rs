//! Core engine runtime.
//!
//! Everything a game needs at runtime lives here; the `app` crate adds the
//! editor UI on top. Planned modules (add them only when a milestone needs
//! them, see `docs/ROADMAP.md`):
//!
//! - `render`    render scale, baked GI, volumetric fog, shadows, post
//! - `audio`     Steam Audio (via AudioNimbus), acoustic materials, limiter
//! - `music`     global key/scale/tempo, motifs, instrument layers
//! - `scene`     entities, reflected components, RON load/save
//! - `zones`     zone graph, culling, simulation radius
//! - `behaviors` Floating, Door, Switch, PhysicsBody, Singer, ...
//! - `player`    character controller, camera, abilities

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
