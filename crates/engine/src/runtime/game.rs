//! The running game: behaviors, physics and the sounds the player makes,
//! and the editor's Play / Stop.

use super::{CameraMode, PlaySnapshot, Running};
use crate::behaviors::{Behaviors, Interaction, is_usable_switch};
use crate::physics::PhysicsWorld;
use crate::player::{Player, Spawn};
use crate::scene::{Component, Scene};
use glam::Vec3;
use winit::keyboard::KeyCode;

impl Running {
    /// Behaviors act on the scene; moved objects are redrawn and their
    /// colliders follow.
    pub(super) fn run_game(&mut self, dt: f32) {
        if self.behaviors.is_none() {
            return;
        }
        self.game_sounds();
        let interaction = self.interaction();
        let Some(behaviors) = &mut self.behaviors else {
            return;
        };
        let moved = behaviors.update(&mut self.scene, dt, &interaction, &self.active);
        for &index in &moved {
            self.renderer
                .update_object(&self.scene, index, &mut self.assets);
            self.physics
                .move_object(index, &self.scene.objects[index].transform);
        }

        if self.mode == CameraMode::Player {
            let body = &self.settings.body;
            let height = if self.player.crouching {
                body.crouch_height
            } else {
                body.height
            };
            // The capsule floats a step above the feet.
            let bottom = self.player.feet + Vec3::Y * body.step_height;
            self.physics.push_bodies(
                bottom,
                height - body.step_height,
                body.radius,
                self.player.velocity,
            );
        }
        self.physics.set_active(&self.active);
        self.physics.advance(dt);
        let poses: Vec<_> = self.physics.body_poses().collect();
        for (index, position, rotation) in poses {
            let transform = &mut self.scene.objects[index].transform;
            transform.position = position;
            transform.set_quat(rotation);
            self.renderer
                .update_object(&self.scene, index, &mut self.assets);
        }
    }

    /// Sounds the player makes: claps (Q), footsteps by floor material, and
    /// M to bring the looping sound to where the player stands.
    pub(super) fn game_sounds(&mut self) {
        let eye = match self.mode {
            CameraMode::Player => self.player.camera(&self.settings),
            CameraMode::Fly => self.fly.camera.clone(),
        };
        if self.input.pressed(KeyCode::KeyM) {
            let ahead = eye.forward().with_y(0.0).normalize_or(Vec3::NEG_Z);
            let looping = self.scene.objects.iter().position(|o| {
                o.components
                    .iter()
                    .any(|c| matches!(c, Component::SoundSource(s) if s.looping))
            });
            if let Some(index) = looping {
                self.scene.objects[index].transform.position = eye.position + ahead * 1.5;
                self.renderer
                    .update_object(&self.scene, index, &mut self.assets);
                self.rebuild_physics();
            }
        }
        let Some(audio) = &mut self.audio else {
            return;
        };
        if self.input.pressed(KeyCode::KeyQ) {
            audio.clap(eye.position);
        }
        if self.mode != CameraMode::Player {
            return;
        }
        let grounded = self.player.grounded;
        let landed = grounded && !self.was_grounded;
        self.was_grounded = grounded;
        // In step with the head bob: one footstep per bob, plus one on landing.
        if landed || self.player.stepped {
            let feet = self.player.feet;
            let floor = self.physics.object_below(feet + Vec3::Y * 0.1, 0.4);
            let material = floor
                .and_then(|i| self.scene.objects.get(i))
                .and_then(|o| {
                    o.components.iter().find_map(|c| match c {
                        Component::AcousticMaterial(m) => Some(m.preset),
                        _ => None,
                    })
                })
                .unwrap_or_default();
            if floor.is_some() {
                // Just above the floor, so the step comes from the surface.
                audio.footstep(material, feet + Vec3::Y * 0.15, &self.settings.footsteps);
            }
        }
    }

    /// What the player does that behaviors react to: pressing E at a switch
    /// in reach (a prompt shows while one is aimed at), standing on things.
    pub(super) fn interaction(&mut self) -> Interaction {
        self.prompt = None;
        if self.mode != CameraMode::Player {
            return Interaction::default();
        }
        let eye = self.player.camera(&self.settings);
        let aimed = self
            .physics
            .pick(eye.position, eye.forward(), REACH)
            .map(|(object, _)| object)
            .filter(|&object| is_usable_switch(&self.scene, object));
        if aimed.is_some() {
            self.prompt = Some("E  USE");
        }
        Interaction {
            used: aimed.filter(|_| self.input.pressed(KeyCode::KeyE)),
            standing_on: self
                .physics
                .object_below(self.player.feet + Vec3::Y * 0.1, 0.4),
        }
    }

    /// The physics world for the current state: dynamic bodies only while
    /// the game runs.
    pub(super) fn rebuild_physics(&mut self) {
        self.physics = if self.behaviors.is_some() {
            PhysicsWorld::for_game(&self.scene, &mut self.assets)
        } else {
            PhysicsWorld::from_scene(&self.scene, &mut self.assets)
        };
    }

    /// Editor: Play runs the scene from its PlayerStart; Stop puts back the
    /// scene and the editor camera as they were.
    pub(super) fn toggle_play(&mut self) {
        if self.play.is_some() {
            self.stop_play();
        } else {
            self.start_play();
        }
    }

    pub(super) fn start_play(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.commit_pending(&self.scene);
        editor.playing = true;
        self.play = Some(PlaySnapshot {
            scene: self.scene.clone(),
            camera: self.fly.camera.clone(),
        });
        self.behaviors = Some(Behaviors::start(&self.scene));
        self.rebuild_physics();
        self.player = Player::new(spawn_point(&self.scene), &self.settings);
        self.zone.reset();
        self.mode = CameraMode::Player;
        self.mouse_released = false;
        self.tuning_open = false;
    }

    pub(super) fn stop_play(&mut self) {
        let Some(snapshot) = self.play.take() else {
            return;
        };
        self.scene = snapshot.scene;
        self.fly.camera = snapshot.camera;
        self.behaviors = None;
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
        self.mode = CameraMode::Fly;
        self.tuning_open = false;
        self.zone.reset();
        if let Some(editor) = &mut self.editor {
            editor.playing = false;
        }
    }
}

/// Where the player starts: the scene's PlayerStart, or the origin.
pub(super) fn spawn_point(scene: &Scene) -> Spawn {
    scene.player_start().map_or_else(
        || {
            log::warn!("the scene has no PlayerStart; spawning at the origin");
            Spawn {
                position: Vec3::ZERO,
                yaw: 0.0,
            }
        },
        Spawn::from_transform,
    )
}

/// How far the player reaches to use a switch (meters).
pub(super) const REACH: f32 = 2.5;
