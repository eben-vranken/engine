//! The running game: behaviors, physics and the sounds the player makes,
//! and the editor's Play / Stop.

use super::{CameraMode, PlaySnapshot, Running};
use crate::behaviors::{Behaviors, Interaction};
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
        let used = self.use_objects();
        let interaction = Interaction {
            used,
            standing_on: if self.mode == CameraMode::Player {
                self.physics
                    .object_below(self.player.feet + Vec3::Y * 0.1, 0.4)
            } else {
                None
            },
        };
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
                body.mass,
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
        self.update_story(dt);
    }

    /// Sounds the player makes: claps (Q), footsteps by floor material, and
    /// (from the test drawer) the looping sound brought to the player.
    pub(super) fn game_sounds(&mut self) {
        let eye = match self.mode {
            CameraMode::Player => self.player.camera(&self.settings),
            CameraMode::Fly => self.fly.camera.clone(),
        };
        if std::mem::take(&mut self.move_sound) {
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

    /// The physics world for the current state: dynamic bodies only while
    /// the game runs.
    pub(super) fn rebuild_physics(&mut self) {
        crate::perf::time("REBUILD PHYSICS", || {
            self.physics = if self.behaviors.is_some() {
                PhysicsWorld::for_game(&self.scene, &mut self.assets)
            } else {
                PhysicsWorld::from_scene(&self.scene, &mut self.assets)
            };
        });
    }

    /// Editor: Play runs the scene from its PlayerStart; Stop puts back the
    /// scene and the editor camera as they were.
    pub(super) fn toggle_play(&mut self) {
        // Play runs the scene on screen: only from BUILD (a script tab or
        // the PROJECT page would play an empty or hidden scene).
        if let Some(editor) = &mut self.editor
            && self.play.is_none()
            && !editor.shows_view()
        {
            editor.show_message("PLAY FROM BUILD (A SCENE TAB)".into());
            return;
        }
        if self.play.is_some() {
            self.stop_play();
        } else {
            self.start_play(false);
        }
    }

    /// Editor: opens the game in a window of its own, as a player gets it
    /// (`--player`): the project's start scene, or the saved game, with no
    /// editor or debug tools. It reads the files as saved.
    pub(super) fn play_in_window(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => {
                editor.show_error(format!("COULD NOT START THE GAME: {err}"));
                return;
            }
        };
        let started = std::process::Command::new(exe)
            .arg("--game")
            .arg(&self.game_dir)
            .arg("--player")
            .arg("--quality")
            .arg(self.renderer.quality().name().to_lowercase())
            .spawn();
        match started {
            Ok(_) => {
                let unsaved = if editor.any_dirty() {
                    ": UNSAVED CHANGES AREN'T IN IT, SAVE FIRST TO SEE THEM"
                } else {
                    ""
                };
                editor.show_message(format!("THE GAME OPENED IN ITS OWN WINDOW{unsaved}"));
                log::info!("started the game in its own window");
            }
            Err(err) => editor.show_error(format!("COULD NOT START THE GAME: {err}")),
        }
    }

    /// Play from the PlayerStart with the scene's graph from its start, or
    /// (`resume`) from the saved game.
    pub(super) fn start_play(&mut self, resume: bool) {
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
        self.player_zone.reset();
        self.mode = CameraMode::Player;
        self.mouse_released = false;
        self.tuning_open = false;
        self.start_story(resume);
    }

    pub(super) fn stop_play(&mut self) {
        let Some(snapshot) = self.play.take() else {
            return;
        };
        self.scene = snapshot.scene;
        self.fly.camera = snapshot.camera;
        self.behaviors = None;
        self.story = None;
        self.prompt = None;
        if self.play_scene.take().is_some() {
            // A graph loaded another scene: the edited one's bakes again.
            self.load_bake();
            self.load_lightmaps();
        }
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.physics = PhysicsWorld::from_scene(&self.scene, &mut self.assets);
        self.mode = CameraMode::Fly;
        self.tuning_open = false;
        self.zone.reset();
        self.player_zone.reset();
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
