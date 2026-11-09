//! Keyboard and mouse state collected from winit events, read once per frame.

use glam::Vec2;
use std::collections::HashSet;
use winit::keyboard::KeyCode;

#[derive(Debug, Default, Clone)]
pub struct Input {
    held: HashSet<KeyCode>,
    pressed: HashSet<KeyCode>,
    /// Raw mouse movement since last frame, in pixels.
    pub mouse_delta: Vec2,
    /// Scroll wheel steps since last frame.
    pub scroll: f32,
    /// True while the right mouse button holds the cursor for mouse look.
    pub looking: bool,
}

impl Input {
    pub fn held(&self, key: KeyCode) -> bool {
        self.held.contains(&key)
    }

    /// True only on the frame the key went down.
    pub fn pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }

    pub fn key_down(&mut self, key: KeyCode) {
        if self.held.insert(key) {
            self.pressed.insert(key);
        }
    }

    pub fn key_up(&mut self, key: KeyCode) {
        self.held.remove(&key);
    }

    /// Drop all held keys, e.g. when the window loses focus.
    pub fn release_all(&mut self) {
        self.held.clear();
        self.looking = false;
    }

    /// Clear per-frame state; call after the frame has used it.
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.mouse_delta = Vec2::ZERO;
        self.scroll = 0.0;
    }
}
