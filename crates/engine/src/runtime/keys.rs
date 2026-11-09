//! Keys handled every frame: editor shortcuts, the game's keys and the
//! debug keys.

use super::hud::HudActions;
use super::{CameraMode, Running};
use crate::physics::PhysicsWorld;
use winit::keyboard::KeyCode;

impl Running {
    pub(super) fn handle_keys(&mut self, actions: &mut HudActions) {
        if self.editor.is_some() && self.input.pressed(KeyCode::F5) {
            self.toggle_play();
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
        if self.input.pressed(KeyCode::KeyM)
            && let Some(story) = &mut self.story
            && story.line.is_none()
        {
            story.map_open = !story.map_open;
        }
        // A/B comparison of the acoustic simulation, anywhere.
        if let Some(audio) = &mut self.audio {
            let toggles = &mut audio.toggles;
            for (key, flag) in [
                (KeyCode::F6, &mut toggles.occlusion),
                (KeyCode::F7, &mut toggles.reflections),
                (KeyCode::F8, &mut toggles.hrtf),
                (KeyCode::F9, &mut toggles.air_absorption),
                (KeyCode::F10, &mut toggles.baked),
            ] {
                if self.input.pressed(key) {
                    *flag = !*flag;
                }
            }
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
            // These keys act on what's on screen: the scene in BUILD, the
            // graph in a graph tab. The PROJECT page and script tabs handle
            // their own keys (a hidden scene must never be edited).
            let workspace = editor.workspace();
            let on_screen = workspace == crate::editor::Workspace::Build
                || (workspace == crate::editor::Workspace::Logic && editor.graph.is_some());
            if !on_screen {
                if workspace == crate::editor::Workspace::Logic {
                    let ctrl =
                        input.held(KeyCode::ControlLeft) || input.held(KeyCode::ControlRight);
                    if ctrl && input.pressed(KeyCode::KeyS) {
                        editor.save(&self.scene, &self.settings);
                    }
                }
                return;
            }
            let ctrl = input.held(KeyCode::ControlLeft) || input.held(KeyCode::ControlRight);
            let shift = input.held(KeyCode::ShiftLeft) || input.held(KeyCode::ShiftRight);
            let mut settled = false;
            let mut changed = None;
            if input.pressed(KeyCode::Escape) {
                settled = editor.select(&self.scene, None);
            }
            // Gizmo modes; while flying these keys move the camera instead.
            if !input.looking && !ctrl {
                for (key, kind) in [
                    (
                        KeyCode::KeyW,
                        transform_gizmo::config::GizmoModeKind::Translate,
                    ),
                    (
                        KeyCode::KeyE,
                        transform_gizmo::config::GizmoModeKind::Rotate,
                    ),
                    (KeyCode::KeyR, transform_gizmo::config::GizmoModeKind::Scale),
                ] {
                    if input.pressed(key) {
                        editor.gizmo.kind = kind;
                    }
                }
            }
            if ctrl && input.pressed(KeyCode::KeyZ) {
                changed = if shift {
                    editor.redo(&mut self.scene, &mut self.settings)
                } else {
                    editor.undo(&mut self.scene, &mut self.settings)
                };
            }
            if ctrl && input.pressed(KeyCode::KeyY) {
                changed = editor.redo(&mut self.scene, &mut self.settings);
            }
            if ctrl && input.pressed(KeyCode::KeyS) {
                settled |= editor.save(&self.scene, &self.settings);
            }
            if input.pressed(KeyCode::Delete) {
                changed = editor.delete_selected(&mut self.scene);
            }
            // Not while flying, where D strafes.
            if ctrl && !input.looking && input.pressed(KeyCode::KeyD) {
                changed = editor.duplicate_selected(&mut self.scene);
            }
            if let Some(change) = changed {
                self.show_change(change);
                settled = true;
            }
            if settled {
                self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
            }
            return;
        }
        if input.pressed(KeyCode::Escape) {
            self.mouse_released = true;
        }
        if input.pressed(KeyCode::F1) {
            self.tuning_open = !self.tuning_open;
        }
        if input.pressed(KeyCode::F2) {
            self.mode = match self.mode {
                CameraMode::Player => {
                    // Start flying from where the player is looking.
                    let view = self.player.camera(&self.settings);
                    self.fly.camera = view;
                    CameraMode::Fly
                }
                CameraMode::Fly => CameraMode::Player,
            };
        }
        if input.pressed(KeyCode::KeyR) {
            self.player.respawn(&self.settings);
        }
    }
}
