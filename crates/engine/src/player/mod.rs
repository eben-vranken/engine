//! First-person character controller. rapier's kinematic character
//! controller resolves collisions (slide, steps, slopes, ground snap); this
//! module decides the desired motion: acceleration, jumping, gravity, crouch,
//! and the camera feel on top.

pub mod settings;
pub mod tuning;

use crate::camera::Camera;
use crate::input::Input;
use crate::physics::PhysicsWorld;
use glam::{Vec2, Vec3, Vec3Swizzles};
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::prelude::{Pose, SharedShape};
use settings::PlayerSettings;
use winit::keyboard::KeyCode;

/// Where the player starts and respawns.
#[derive(Debug, Clone, Copy)]
pub struct Spawn {
    /// Feet position.
    pub position: Vec3,
    /// Radians; 0 looks down -Z.
    pub yaw: f32,
}

pub struct Player {
    spawn: Spawn,
    /// Bottom of the capsule.
    pub feet: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
    /// Seconds since last on the ground (0 while grounded).
    pub time_since_grounded: f32,
    pub crouching: bool,
    pub sprinting: bool,
    jumped: bool,
    jump_buffer: f32,
    /// Smoothed eye height above the feet (crouch transitions).
    eye_height: f32,
    bob_phase: f32,
    bob_weight: f32,
    dip_offset: f32,
    dip_velocity: f32,
    fov_kick: f32,
}

impl Player {
    pub fn new(spawn: Spawn, settings: &PlayerSettings) -> Self {
        Self {
            spawn,
            feet: spawn.position,
            velocity: Vec3::ZERO,
            yaw: spawn.yaw,
            pitch: 0.0,
            grounded: false,
            time_since_grounded: 0.0,
            crouching: false,
            sprinting: false,
            jumped: false,
            jump_buffer: 0.0,
            eye_height: settings.body.height - settings.body.eye_offset,
            bob_phase: 0.0,
            bob_weight: 0.0,
            dip_offset: 0.0,
            dip_velocity: 0.0,
            fov_kick: 0.0,
        }
    }

    pub fn respawn(&mut self, settings: &PlayerSettings) {
        *self = Self::new(self.spawn, settings);
    }

    pub fn horizontal_speed(&self) -> f32 {
        self.velocity.xz().length()
    }

    pub fn update(
        &mut self,
        input: &Input,
        settings: &PlayerSettings,
        physics: &PhysicsWorld,
        dt: f32,
    ) {
        if dt <= 0.0 {
            return;
        }
        let PlayerSettings {
            movement,
            jump,
            body,
            camera,
        } = settings;

        // Look.
        if input.looking {
            let sensitivity = camera.sensitivity.to_radians();
            self.yaw += input.mouse_delta.x * sensitivity;
            self.pitch = (self.pitch - input.mouse_delta.y * sensitivity)
                .clamp(-89f32.to_radians(), 89f32.to_radians());
        }

        // Crouch: going down is instant, standing up needs headroom.
        let wants_crouch = input.held(KeyCode::ControlLeft) || input.held(KeyCode::KeyC);
        if wants_crouch {
            self.crouching = true;
        } else if self.crouching && self.has_headroom(settings, physics) {
            self.crouching = false;
        }
        let height = if self.crouching {
            body.crouch_height
        } else {
            body.height
        };

        // Horizontal movement.
        let axis = |pos: KeyCode, neg: KeyCode| {
            input.held(pos) as i32 as f32 - input.held(neg) as i32 as f32
        };
        let forward_input = axis(KeyCode::KeyW, KeyCode::KeyS);
        let right_input = axis(KeyCode::KeyD, KeyCode::KeyA);
        let (sin, cos) = self.yaw.sin_cos();
        let forward = Vec2::new(sin, -cos);
        let right = Vec2::new(cos, sin);
        let wish = (forward * forward_input + right * right_input).normalize_or_zero();

        self.sprinting = input.held(KeyCode::ShiftLeft) && forward_input > 0.0 && !self.crouching;
        let speed = if self.crouching {
            movement.crouch_speed
        } else if self.sprinting {
            movement.sprint_speed
        } else {
            movement.walk_speed
        };
        let target = wish * speed;
        let mut horizontal = self.velocity.xz();
        if self.grounded {
            let rate = if wish == Vec2::ZERO {
                movement.ground_deceleration
            } else {
                movement.ground_acceleration
            };
            horizontal = move_towards(horizontal, target, rate * dt);
        } else {
            let delta =
                move_towards(horizontal, target, movement.air_acceleration * dt) - horizontal;
            // Speeding up along the current direction is free; steering and
            // braking are scaled by air control.
            let direction = horizontal.normalize_or_zero();
            let boost = direction * delta.dot(direction).max(0.0);
            let delta = if direction == Vec2::ZERO {
                delta
            } else {
                boost + (delta - boost) * movement.air_control
            };
            horizontal += delta;
        }

        // Jump: buffered presses, coyote time.
        if input.pressed(KeyCode::Space) {
            self.jump_buffer = jump.buffer_time;
        } else {
            self.jump_buffer = (self.jump_buffer - dt).max(0.0);
        }
        let can_jump =
            self.grounded || (!self.jumped && self.time_since_grounded <= jump.coyote_time);
        let mut jumped_now = false;
        if self.jump_buffer > 0.0 && can_jump {
            self.velocity.y = jump.takeoff_speed();
            self.jump_buffer = 0.0;
            self.jumped = true;
            self.grounded = false;
            jumped_now = true;
        }

        // Gravity: lighter on the way up while jump is held, heavier when
        // falling or when jump was released early.
        let gravity_scale = if self.velocity.y > 0.0 {
            if self.jumped && !input.held(KeyCode::Space) {
                jump.jump_cut_gravity_multiplier
            } else {
                1.0
            }
        } else {
            jump.fall_gravity_multiplier
        };
        let accel = -jump.gravity() * gravity_scale;
        // Exact for constant acceleration, so jump height doesn't depend on frame rate.
        let mut dy = self.velocity.y * dt + 0.5 * accel * dt * dt;
        self.velocity.y = (self.velocity.y + accel * dt).max(-jump.max_fall_speed);
        if self.velocity.y <= -jump.max_fall_speed {
            dy = dy.max(-jump.max_fall_speed * dt);
        }

        let desired = Vec3::new(horizontal.x * dt, dy, horizontal.y * dt);
        let movement_result = {
            let controller = KinematicCharacterController {
                offset: CharacterLength::Absolute(0.01),
                autostep: Some(CharacterAutostep {
                    max_height: CharacterLength::Absolute(body.step_height),
                    min_width: CharacterLength::Absolute(body.radius * 0.5),
                    include_dynamic_bodies: false,
                }),
                max_slope_climb_angle: body.max_slope.to_radians(),
                min_slope_slide_angle: body.max_slope.to_radians(),
                snap_to_ground: (!jumped_now)
                    .then_some(CharacterLength::Absolute(body.ground_snap)),
                ..Default::default()
            };
            let shape = capsule(height, body.radius);
            let queries = physics.queries();
            controller.move_shape(
                dt,
                &queries,
                shape.as_ref(),
                &capsule_pose(self.feet, height),
                desired,
                |_| {},
            )
        };
        let moved = movement_result.translation;
        self.feet += moved;

        // Walls and slopes take speed away; don't keep pushing into them.
        let actual = moved.xz() / dt;
        if actual.length() < horizontal.length() {
            horizontal = actual;
        }
        // Bumped the ceiling.
        if self.velocity.y > 0.0 && moved.y < dy - 1e-4 {
            self.velocity.y = 0.0;
        }

        let was_grounded = self.grounded;
        if movement_result.grounded && self.velocity.y <= 0.0 {
            if !was_grounded {
                self.land(-self.velocity.y, settings);
            }
            self.grounded = true;
            self.jumped = false;
            self.velocity.y = 0.0;
            self.time_since_grounded = 0.0;
        } else {
            self.grounded = false;
            self.time_since_grounded += dt;
        }
        self.velocity.x = horizontal.x;
        self.velocity.z = horizontal.y;

        self.update_camera_feel(settings, height, dt);
    }

    fn land(&mut self, impact_speed: f32, settings: &PlayerSettings) {
        // A velocity kick into a critically damped spring: with ω = 1 / t,
        // the dip bottoms out at depth v / (ω e) after t seconds.
        let camera = &settings.camera;
        let omega = 1.0 / camera.landing_dip_time.max(0.01);
        let depth = camera.landing_dip * (impact_speed / 10.0).clamp(0.0, 1.5);
        self.dip_velocity -= depth * omega * std::f32::consts::E;
    }

    fn update_camera_feel(&mut self, settings: &PlayerSettings, height: f32, dt: f32) {
        let camera = &settings.camera;

        let target_eye = height - settings.body.eye_offset;
        self.eye_height += (target_eye - self.eye_height) * (1.0 - (-14.0 * dt).exp());

        let omega = 1.0 / camera.landing_dip_time.max(0.01);
        let accel = -omega * omega * self.dip_offset - 2.0 * omega * self.dip_velocity;
        self.dip_velocity += accel * dt;
        self.dip_offset += self.dip_velocity * dt;

        let speed = self.horizontal_speed();
        let bobbing = camera.head_bob && self.grounded && speed > 0.5;
        let weight_target = if bobbing {
            (speed / settings.movement.walk_speed).min(1.5)
        } else {
            0.0
        };
        self.bob_weight += (weight_target - self.bob_weight) * (1.0 - (-10.0 * dt).exp());
        if self.grounded {
            self.bob_phase = (self.bob_phase + speed * dt / camera.bob_stride.max(0.1)).fract();
        }

        let kick_target = if self.sprinting && speed > settings.movement.walk_speed {
            1.0
        } else {
            0.0
        };
        let rate = dt / camera.fov_kick_time.max(0.01);
        self.fov_kick += (kick_target - self.fov_kick).clamp(-rate, rate);
    }

    /// Can the standing capsule fit where the crouched one is?
    fn has_headroom(&self, settings: &PlayerSettings, physics: &PhysicsWorld) -> bool {
        let body = &settings.body;
        let rise = body.height - body.crouch_height;
        let shape = capsule(body.crouch_height, body.radius);
        physics
            .queries()
            .cast_shape(
                &capsule_pose(self.feet, body.crouch_height),
                Vec3::Y,
                shape.as_ref(),
                ShapeCastOptions {
                    max_time_of_impact: rise,
                    target_distance: 0.0,
                    stop_at_penetration: false,
                    compute_impact_geometry_on_penetration: false,
                },
            )
            .is_none()
    }

    /// The first-person camera, with bob, landing dip and FOV kick applied.
    pub fn camera(&self, settings: &PlayerSettings) -> Camera {
        let camera = &settings.camera;
        let bob = camera.bob_amplitude
            * self.bob_weight
            * (0.5 - 0.5 * (self.bob_phase * std::f32::consts::TAU).cos());
        Camera {
            position: self.feet + Vec3::Y * (self.eye_height - bob + self.dip_offset),
            yaw: self.yaw,
            pitch: self.pitch,
            fov_y: camera.fov + camera.sprint_fov_kick * smoothstep(self.fov_kick),
            near: 0.05,
            far: 500.0,
        }
    }
}

fn capsule(height: f32, radius: f32) -> SharedShape {
    let radius = radius.min(height * 0.5);
    SharedShape::capsule_y((height * 0.5 - radius).max(0.0), radius)
}

fn capsule_pose(feet: Vec3, height: f32) -> Pose {
    Pose::from_translation(feet + Vec3::Y * (height * 0.5))
}

fn move_towards(from: Vec2, to: Vec2, max_step: f32) -> Vec2 {
    let delta = to - from;
    let distance = delta.length();
    if distance <= max_step || distance < 1e-6 {
        to
    } else {
        from + delta / distance * max_step
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::{Object, Scene};

    fn flat_world() -> PhysicsWorld {
        PhysicsWorld::from_scene(&Scene {
            objects: vec![Object::new(
                Shape::Box {
                    size: Vec3::new(100.0, 1.0, 100.0),
                },
                Vec3::new(0.0, -0.5, 0.0),
            )],
            ..Default::default()
        })
    }

    fn spawn() -> Spawn {
        Spawn {
            position: Vec3::new(0.0, 0.5, 0.0),
            yaw: 0.0,
        }
    }

    /// Runs `seconds` of simulation at a fixed frame rate.
    fn simulate(
        player: &mut Player,
        input: &mut Input,
        world: &PhysicsWorld,
        seconds: f32,
        fps: f32,
    ) {
        let settings = PlayerSettings::default();
        for _ in 0..(seconds * fps) as usize {
            player.update(input, &settings, world, 1.0 / fps);
            input.end_frame();
        }
    }

    #[test]
    fn falls_and_lands_on_the_floor() {
        let world = flat_world();
        let mut player = Player::new(spawn(), &PlayerSettings::default());
        simulate(&mut player, &mut Input::default(), &world, 2.0, 120.0);
        assert!(player.grounded);
        assert!(player.feet.y.abs() < 0.05, "feet at {}", player.feet.y);
    }

    #[test]
    fn jump_height_is_independent_of_frame_rate() {
        let settings = PlayerSettings::default();
        for fps in [30.0, 60.0, 144.0, 500.0] {
            let world = flat_world();
            let mut player = Player::new(spawn(), &settings);
            let mut input = Input::default();
            simulate(&mut player, &mut input, &world, 1.0, fps);
            let ground = player.feet.y;

            input.key_down(KeyCode::Space);
            let mut apex = ground;
            for _ in 0..(fps as usize) {
                player.update(&input, &settings, &world, 1.0 / fps);
                input.end_frame();
                apex = apex.max(player.feet.y);
            }
            let height = apex - ground;
            assert!(
                (height - settings.jump.height).abs() < 0.05,
                "{fps} fps: jumped {height} m"
            );
        }
    }

    #[test]
    fn walks_at_walk_speed() {
        let world = flat_world();
        let settings = PlayerSettings::default();
        let mut player = Player::new(spawn(), &settings);
        let mut input = Input::default();
        simulate(&mut player, &mut input, &world, 1.0, 120.0);
        input.key_down(KeyCode::KeyW);
        simulate(&mut player, &mut input, &world, 1.0, 120.0);
        assert!((player.horizontal_speed() - settings.movement.walk_speed).abs() < 0.01);
        assert!(player.feet.z < -3.0, "should have walked toward -Z");
    }
}
