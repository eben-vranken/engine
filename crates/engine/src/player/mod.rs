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
use rapier3d::control::{CharacterLength, KinematicCharacterController};
use rapier3d::parry::query::ShapeCastOptions;
use rapier3d::prelude::{Ball, Pose, Ray, SharedShape};
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
    /// View offset that eases out height changes from stairs and slopes.
    step_offset: f32,
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
            step_offset: 0.0,
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
        let was_grounded = self.grounded && !jumped_now;
        let fall_speed = -self.velocity.y;
        // Exact for constant acceleration, so jump height doesn't depend on frame rate.
        let dy = if was_grounded {
            0.0
        } else {
            let dy = self.velocity.y * dt + 0.5 * accel * dt * dt;
            self.velocity.y = (self.velocity.y + accel * dt).max(-jump.max_fall_speed);
            dy.max(-jump.max_fall_speed * dt)
        };

        // 1. Horizontal: move the floating capsule, sliding along walls. The
        // capsule never touches the ground, so any shortfall is a wall, and
        // the velocity keeps only what the wall let through.
        let wanted = Vec3::new(horizontal.x * dt, 0.0, horizontal.y * dt);
        let moved = self.move_capsule(physics, settings, height, wanted);
        self.feet += moved;
        if moved.xz().length() < wanted.xz().length() - 1e-6 {
            horizontal = moved.xz() / dt;
        }

        // 2. Vertical: follow the ground, or fall / rise through the air.
        let walkable = body.max_slope.to_radians().cos();
        let reach = if was_grounded {
            body.ground_snap
        } else {
            (-dy).max(0.0)
        };
        let ground = if self.velocity.y > 0.0 || jumped_now {
            None
        } else {
            self.probe_ground(physics, settings, height, reach + 0.01)
        };
        self.grounded = false;
        match ground {
            Some(ground) => {
                let before = self.feet.y;
                let follow = Vec3::Y * (ground.height - self.feet.y);
                self.feet += self.move_capsule(physics, settings, height, follow);
                self.velocity.y = 0.0;
                if ground.normal.y >= walkable {
                    if was_grounded {
                        // Stairs and slope crests: smooth the view over the height change.
                        self.step_offset -= self.feet.y - before;
                    } else {
                        self.land(fall_speed, settings);
                    }
                    self.grounded = true;
                    self.jumped = false;
                    self.time_since_grounded = 0.0;
                } else {
                    // Too steep: it holds you up, but you can't walk up it and you slide down.
                    let downhill = ground.normal.xz().normalize_or_zero();
                    let uphill_push = horizontal.dot(downhill);
                    if uphill_push < 0.0 {
                        horizontal -= downhill * uphill_push;
                    }
                    let slope_accel = jump.gravity()
                        * jump.fall_gravity_multiplier
                        * ground.normal.y
                        * (1.0 - ground.normal.y * ground.normal.y).max(0.0).sqrt();
                    horizontal += downhill * slope_accel * dt;
                }
            }
            None => {
                let moved = self.move_capsule(physics, settings, height, Vec3::Y * dy);
                self.feet += moved;
                // Bumped the ceiling, or came to rest on something the probe
                // didn't count as ground.
                if (dy > 0.0 && moved.y < dy - 1e-4) || (dy < 0.0 && moved.y > dy + 1e-4) {
                    self.velocity.y = moved.y / dt;
                }
            }
        }
        if !self.grounded {
            self.time_since_grounded += dt;
        }
        self.velocity.x = horizontal.x;
        self.velocity.z = horizontal.y;

        self.update_camera_feel(settings, height, dt);
    }

    /// Moves the collision capsule, sliding along whatever it hits.
    fn move_capsule(
        &self,
        physics: &PhysicsWorld,
        settings: &PlayerSettings,
        height: f32,
        translation: Vec3,
    ) -> Vec3 {
        // Climbing is the ground probe's job, so every surface the capsule
        // touches counts as a wall.
        let controller = KinematicCharacterController {
            offset: CharacterLength::Absolute(0.01),
            autostep: None,
            snap_to_ground: None,
            max_slope_climb_angle: 0.0,
            min_slope_slide_angle: 0.0,
            ..Default::default()
        };
        let (shape, pose) = body_capsule(self.feet, height, &settings.body);
        controller
            .move_shape(
                1.0,
                &physics.queries(),
                shape.as_ref(),
                &pose,
                translation,
                |_| {},
            )
            .translation
    }

    /// Finds the ground under the player with a small sphere cast from the
    /// bottom of the capsule down past the feet. Anything up to the step
    /// height above the feet counts (that's how steps are climbed); `reach`
    /// is how far below the feet to look.
    fn probe_ground(
        &self,
        physics: &PhysicsWorld,
        settings: &PlayerSettings,
        height: f32,
        reach: f32,
    ) -> Option<Ground> {
        let body = &settings.body;
        let step = step_height(height, body);
        let radius = body.radius * 0.5;
        let start = self.feet + Vec3::Y * (step + radius);
        let distance = step + reach;
        let queries = physics.queries();
        let (_, hit) = queries.cast_shape(
            &Pose::from_translation(start),
            Vec3::NEG_Y,
            &Ball::new(radius),
            ShapeCastOptions {
                max_time_of_impact: distance,
                target_distance: 0.0,
                stop_at_penetration: true,
                compute_impact_geometry_on_penetration: true,
            },
        )?;
        // The sphere's normal is diagonal on edges; the surface right under
        // the center decides whether the ground is walkable.
        let ray = Ray::new(start, Vec3::NEG_Y);
        let normal = queries
            .cast_ray_and_get_normal(&ray, radius + distance, true)
            .map_or(hit.normal1, |(_, ray_hit)| ray_hit.normal);
        Some(Ground {
            height: start.y - radius - hit.time_of_impact,
            normal,
        })
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

        self.step_offset *= (-dt / camera.step_smoothing.max(0.001)).exp();

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
        let (shape, pose) = body_capsule(self.feet, body.crouch_height, body);
        physics
            .queries()
            .cast_shape(
                &pose,
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
            position: self.feet
                + Vec3::Y * (self.eye_height - bob + self.dip_offset + self.step_offset),
            yaw: self.yaw,
            pitch: self.pitch,
            fov_y: camera.fov + camera.sprint_fov_kick * smoothstep(self.fov_kick),
            near: 0.05,
            far: 500.0,
        }
    }
}

struct Ground {
    /// World height the feet should stand at.
    height: f32,
    /// Surface normal under the player's center.
    normal: Vec3,
}

/// How far the collision capsule floats above the feet: the step height,
/// limited so the capsule always has room for its two half-spheres.
fn step_height(height: f32, body: &settings::Body) -> f32 {
    body.step_height
        .clamp(0.0, (height - 2.0 * body.radius - 0.01).max(0.0))
}

/// The collision capsule floats `step_height` above the feet, so steps and
/// small ledges pass underneath it and the ground probe lifts the feet onto
/// them. Only things taller than a step touch it.
fn body_capsule(feet: Vec3, height: f32, body: &settings::Body) -> (SharedShape, Pose) {
    let bottom = step_height(height, body);
    let length = height - bottom;
    let radius = body.radius.min(length * 0.5);
    let shape = SharedShape::capsule_y((length * 0.5 - radius).max(0.0), radius);
    let pose = Pose::from_translation(feet + Vec3::Y * (bottom + length * 0.5));
    (shape, pose)
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
    fn stays_on_the_floor_at_any_frame_rate() {
        let settings = PlayerSettings::default();
        for fps in [30.0, 60.0, 120.0, 144.0, 165.0, 240.0, 1000.0] {
            let world = flat_world();
            let mut player = Player::new(spawn(), &settings);
            let mut lowest = f32::MAX;
            for _ in 0..(3.0 * fps) as usize {
                player.update(&Input::default(), &settings, &world, 1.0 / fps);
                lowest = lowest.min(player.feet.y);
            }
            assert!(lowest > -0.01, "{fps} fps: sank to {lowest}");
        }
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

    fn world_with(mut objects: Vec<Object>) -> PhysicsWorld {
        objects.push(Object::new(
            Shape::Box {
                size: Vec3::new(200.0, 1.0, 200.0),
            },
            Vec3::new(0.0, -0.5, 0.0),
        ));
        PhysicsWorld::from_scene(&Scene {
            objects,
            ..Default::default()
        })
    }

    /// Holds `keys` for `seconds` and returns the lowest horizontal speed seen
    /// after the first half second (once acceleration is done).
    fn min_speed_while_holding(
        player: &mut Player,
        world: &PhysicsWorld,
        keys: &[KeyCode],
        seconds: f32,
        fps: f32,
    ) -> f32 {
        let settings = PlayerSettings::default();
        let mut input = Input::default();
        simulate(player, &mut input, world, 0.5, fps); // settle on the ground
        for &key in keys {
            input.key_down(key);
        }
        let mut min_speed = f32::MAX;
        let frames = (seconds * fps) as usize;
        for frame in 0..frames {
            player.update(&input, &settings, world, 1.0 / fps);
            input.end_frame();
            if frame as f32 > 0.5 * fps {
                min_speed = min_speed.min(player.horizontal_speed());
            }
        }
        min_speed
    }

    #[test]
    fn sprint_speed_holds_at_high_frame_rates() {
        let settings = PlayerSettings::default();
        for fps in [60.0, 144.0, 2000.0] {
            let world = flat_world();
            let mut player = Player::new(spawn(), &settings);
            let keys = [KeyCode::KeyW, KeyCode::ShiftLeft];
            let min = min_speed_while_holding(&mut player, &world, &keys, 4.0, fps);
            assert!(
                min > settings.movement.sprint_speed * 0.99,
                "{fps} fps: sprint dropped to {min} m/s"
            );
        }
    }

    #[test]
    fn walking_up_a_walkable_slope_keeps_speed() {
        let settings = PlayerSettings::default();
        for angle in [15.0f32, 30.0, 45.0] {
            let depth = 6.0 / angle.to_radians().tan();
            let world = world_with(vec![Object::new(
                Shape::Wedge {
                    size: Vec3::new(4.0, 6.0, depth),
                },
                Vec3::new(0.0, 3.0, -1.0 - depth * 0.5),
            )]);
            let mut player = Player::new(spawn(), &settings);
            let min = min_speed_while_holding(&mut player, &world, &[KeyCode::KeyW], 1.5, 144.0);
            assert!(
                min > settings.movement.walk_speed * 0.95,
                "{angle}°: speed dropped to {min} m/s"
            );
            assert!(
                player.feet.y > 0.5,
                "{angle}°: did not climb (y = {})",
                player.feet.y
            );
        }
    }

    #[test]
    fn too_steep_slope_blocks() {
        let settings = PlayerSettings::default();
        let depth = 6.0 / 60f32.to_radians().tan();
        let world = world_with(vec![Object::new(
            Shape::Wedge {
                size: Vec3::new(4.0, 6.0, depth),
            },
            Vec3::new(0.0, 3.0, -1.0 - depth * 0.5),
        )]);
        let mut player = Player::new(spawn(), &settings);
        min_speed_while_holding(&mut player, &world, &[KeyCode::KeyW], 2.0, 144.0);
        assert!(
            player.feet.y < 1.0,
            "climbed a 60° slope to y = {}",
            player.feet.y
        );
    }

    #[test]
    fn climbs_stairs_without_losing_speed() {
        let settings = PlayerSettings::default();
        let mut objects: Vec<Object> = (0..10)
            .map(|k| {
                let height = 0.2 * (k + 1) as f32;
                Object::new(
                    Shape::Box {
                        size: Vec3::new(3.0, height, 0.3),
                    },
                    Vec3::new(0.0, height * 0.5, -1.0 - 0.3 * (k as f32 + 0.5)),
                )
            })
            .collect();
        // Landing at the top.
        objects.push(Object::new(
            Shape::Box {
                size: Vec3::new(3.0, 2.0, 10.0),
            },
            Vec3::new(0.0, 1.0, -9.0),
        ));
        let world = world_with(objects);
        let mut player = Player::new(spawn(), &settings);
        let start_z = player.feet.z;
        let mut input = Input::default();
        simulate(&mut player, &mut input, &world, 0.5, 144.0);
        input.key_down(KeyCode::KeyW);
        simulate(&mut player, &mut input, &world, 1.5, 144.0);
        // 1.5 s at 4.5 m/s ≈ 6.75 m; allow a little for acceleration and steps.
        let travelled = start_z - player.feet.z;
        assert!(travelled > 6.0, "only travelled {travelled} m");
        assert!(
            player.feet.y > 1.9,
            "did not reach the top (y = {})",
            player.feet.y
        );
    }

    #[test]
    fn slides_along_a_wall_at_an_angle() {
        let settings = PlayerSettings::default();
        let world = world_with(vec![Object::new(
            Shape::Box {
                size: Vec3::new(40.0, 3.0, 0.5),
            },
            Vec3::new(0.0, 1.5, -2.0),
        )]);
        let mut player = Player::new(spawn(), &settings);
        // Look 45° to the right of the wall's normal and walk into it.
        player.yaw = 45f32.to_radians();
        let mut input = Input::default();
        simulate(&mut player, &mut input, &world, 0.5, 144.0);
        input.key_down(KeyCode::KeyW);
        simulate(&mut player, &mut input, &world, 2.0, 144.0);
        // Pressed against the wall, sliding along +X at the tangential part
        // of walk speed, with nothing left going into the wall.
        let expected = settings.movement.walk_speed * 45f32.to_radians().cos();
        assert!(
            player.velocity.z.abs() < 0.01,
            "still pushing into the wall"
        );
        assert!(
            (player.velocity.x - expected).abs() < 0.1,
            "sliding at {} m/s, expected {expected}",
            player.velocity.x
        );
    }
}
