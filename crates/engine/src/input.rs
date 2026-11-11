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

/// A key by its name in `player.ron` and in scripts: a letter or digit,
/// `space`, `shift`, `ctrl`, `alt`, `tab`, `enter`, `up` / `down` /
/// `left` / `right`.
pub fn key_named(name: &str) -> Option<KeyCode> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    const DIGITS: [KeyCode; 10] = [
        Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    ];
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "space" => Some(Space),
        "shift" => Some(ShiftLeft),
        "ctrl" => Some(ControlLeft),
        "alt" => Some(AltLeft),
        "tab" => Some(Tab),
        "enter" => Some(Enter),
        "up" => Some(ArrowUp),
        "down" => Some(ArrowDown),
        "left" => Some(ArrowLeft),
        "right" => Some(ArrowRight),
        one if one.len() == 1 => match one.as_bytes()[0] {
            c @ b'a'..=b'z' => Some(LETTERS[(c - b'a') as usize]),
            c @ b'0'..=b'9' => Some(DIGITS[(c - b'0') as usize]),
            _ => None,
        },
        _ => None,
    }
}

/// The keys of a binding: key names separated by spaces (`"ctrl c"`), any
/// of which does the job. Shift, Ctrl, Alt and Enter count on both sides
/// of the keyboard; names that aren't keys are skipped.
fn bound(binding: &str) -> impl Iterator<Item = KeyCode> + '_ {
    binding
        .split_whitespace()
        .filter_map(key_named)
        .flat_map(|key| {
            let twin = match key {
                KeyCode::ShiftLeft => Some(KeyCode::ShiftRight),
                KeyCode::ControlLeft => Some(KeyCode::ControlRight),
                KeyCode::AltLeft => Some(KeyCode::AltRight),
                KeyCode::Enter => Some(KeyCode::NumpadEnter),
                _ => None,
            };
            [Some(key), twin].into_iter().flatten()
        })
}

/// The names in a binding that aren't keys.
pub fn unknown_keys(binding: &str) -> impl Iterator<Item = &str> {
    binding
        .split_whitespace()
        .filter(|name| key_named(name).is_none())
}

/// A binding's first key as prompts show it (`E`, `SPACE`).
pub fn key_label(binding: &str) -> String {
    binding
        .split_whitespace()
        .next()
        .unwrap_or("?")
        .to_uppercase()
}

impl Input {
    /// Whether any key of the binding is held.
    pub fn bound_held(&self, binding: &str) -> bool {
        bound(binding).any(|key| self.held(key))
    }

    /// Whether any key of the binding went down this frame.
    pub fn bound_pressed(&self, binding: &str) -> bool {
        bound(binding).any(|key| self.pressed(key))
    }

    /// Presses the binding's first key.
    pub fn press_bound(&mut self, binding: &str) {
        if let Some(key) = bound(binding).next() {
            self.key_down(key);
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_key_of_a_binding_counts() {
        let mut input = Input::default();
        assert!(!input.bound_held("ctrl c"));
        input.key_down(KeyCode::KeyC);
        assert!(input.bound_held("ctrl c") && input.bound_pressed("CTRL C"));
        input.key_up(KeyCode::KeyC);
        input.key_down(KeyCode::ControlRight);
        assert!(input.bound_held("ctrl c"));
        assert!(!input.bound_held("space") && !input.bound_held(""));
    }

    #[test]
    fn names_that_are_not_keys_are_found() {
        assert_eq!(
            unknown_keys("w spacebar up").collect::<Vec<_>>(),
            ["spacebar"]
        );
        assert_eq!(key_label("e enter"), "E");
        let mut input = Input::default();
        input.press_bound("spacebar space");
        assert!(input.pressed(KeyCode::Space));
    }
}
