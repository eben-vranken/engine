//! Keys handled every frame: editor shortcuts, the game's keys and the
//! debug keys.

use super::Running;
use super::hud::HudActions;
use winit::keyboard::KeyCode;

impl Running {
    pub(super) fn handle_keys(&mut self, actions: &mut HudActions) {
        if self.editor.is_some() && self.input.pressed(KeyCode::F5) {
            let shift = self.input.held(KeyCode::ShiftLeft) || self.input.held(KeyCode::ShiftRight);
            if shift {
                self.play_in_window();
            } else {
                self.toggle_play();
            }
        }
        // As a player gets the game: only its own keys (map, clap, use,
        // dialogue; Esc frees the mouse). No debug keys.
        if self.player_view {
            if self.input.bound_pressed(&self.settings.keys.map)
                && let Some(story) = &mut self.story
                && story.line.is_none()
            {
                story.map_open = !story.map_open;
            }
            if self.input.pressed(KeyCode::Escape) {
                self.mouse_released = true;
            }
            return;
        }
        // The test drawer (running graphs, flags, audio, feel, map).
        if self.input.pressed(KeyCode::F3) {
            match (&mut self.editor, &mut self.story) {
                (Some(editor), _) => editor.drawer_open = !editor.drawer_open,
                (None, Some(story)) => story.debug_open = !story.debug_open,
                _ => {}
            }
        }
        // The map while the game runs.
        if self.input.bound_pressed(&self.settings.keys.map)
            && let Some(story) = &mut self.story
            && story.line.is_none()
        {
            story.map_open = !story.map_open;
        }
        // While playing in the editor, the game's keys apply (below).
        let playing = self.play.is_some();
        let input = &self.input;
        let factor = self.renderer.layout().factor;
        if input.pressed(KeyCode::BracketLeft) {
            actions.scale = Some(Some(factor.saturating_sub(1).max(1)));
        }
        if input.pressed(KeyCode::BracketRight) {
            actions.scale = Some(Some(factor + 1));
        }
        if input.pressed(KeyCode::Digit0) {
            actions.scale = Some(None);
        }
        if input.pressed(KeyCode::KeyV) {
            actions.vsync = Some(!self.renderer.vsync());
        }
        if !playing && let Some(editor) = &mut self.editor {
            // Editor keys go through the binding table and the commands.
            editor.press_keys(input, input.looking);
            return;
        }
        let [escape, f1, f2, f6, f7] = [
            KeyCode::Escape,
            KeyCode::F1,
            KeyCode::F2,
            KeyCode::F6,
            KeyCode::F7,
        ]
        .map(|key| input.pressed(key));
        if escape {
            self.mouse_released = true;
        }
        // F1: the player feel, in the test drawer's FEEL tab.
        if f1 && let Some(story) = &mut self.story {
            story.tab = super::test_drawer::Tab::Feel;
            match &mut self.editor {
                Some(editor) => editor.drawer_open = true,
                None => story.debug_open = true,
            }
        }
        if f2 {
            self.toggle_free_camera();
        }
        // Editor Play: pause and step a frame.
        if let Some(editor) = &mut self.editor {
            if f6 {
                editor.play_controls.paused = !editor.play_controls.paused;
            }
            if f7 && editor.play_controls.paused {
                editor.play_controls.step = true;
            }
        }
        let input = &self.input;
        if input.pressed(KeyCode::KeyR) {
            self.player.respawn(&self.settings);
        }
    }
}
