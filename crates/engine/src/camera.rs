//! Cameras. Right-handed, Y up; yaw 0 looks down -Z.

use crate::input::Input;
use glam::camera::rh;
use glam::{Mat4, Vec3};
use winit::keyboard::KeyCode;

#[derive(Debug, Clone)]
pub struct Camera {
    pub position: Vec3,
    /// Radians; positive turns right.
    pub yaw: f32,
    /// Radians; positive looks up.
    pub pitch: f32,
    /// Vertical field of view in degrees.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, -cy * cp)
    }

    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cy, 0.0, sy)
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        // wgpu clip space: depth 0..1, Y up (glam calls this "directx").
        let projection =
            rh::proj::directx::perspective(self.fov_y.to_radians(), aspect, self.near, self.far);
        projection * rh::view::look_to_mat4(self.position, self.forward(), Vec3::Y)
    }
}

/// Free-fly debug camera: hold the right mouse button to look, WASD to move,
/// E/Space up, Q/Ctrl down, Shift for speed, scroll wheel to change speed.
#[derive(Debug, Clone)]
pub struct FlyCamera {
    pub camera: Camera,
    /// Meters per second.
    pub speed: f32,
    /// Radians per pixel of mouse movement.
    pub sensitivity: f32,
    /// Only move while looking (right mouse held), so the keys stay free
    /// for shortcuts otherwise. Used in the editor.
    pub move_requires_look: bool,
}

impl FlyCamera {
    pub fn new(position: Vec3, yaw: f32, pitch: f32) -> Self {
        Self {
            camera: Camera {
                position,
                yaw,
                pitch,
                fov_y: 70.0,
                near: 0.05,
                far: 500.0,
            },
            speed: 6.0,
            sensitivity: 0.0025,
            move_requires_look: false,
        }
    }

    pub fn update(&mut self, input: &Input, dt: f32) {
        if input.looking {
            let delta = input.mouse_delta;
            self.camera.yaw += delta.x * self.sensitivity;
            self.camera.pitch = (self.camera.pitch - delta.y * self.sensitivity)
                .clamp(-89f32.to_radians(), 89f32.to_radians());
        }
        if self.move_requires_look && !input.looking {
            return;
        }
        if input.scroll != 0.0 {
            self.speed = (self.speed * 1.2f32.powf(input.scroll)).clamp(0.5, 200.0);
        }

        let axis = |pos: &[KeyCode], neg: &[KeyCode]| {
            let held = |keys: &[KeyCode]| keys.iter().any(|&k| input.held(k));
            held(pos) as i32 as f32 - held(neg) as i32 as f32
        };
        let forward = axis(&[KeyCode::KeyW], &[KeyCode::KeyS]);
        let right = axis(&[KeyCode::KeyD], &[KeyCode::KeyA]);
        let up = axis(
            &[KeyCode::KeyE, KeyCode::Space],
            &[KeyCode::KeyQ, KeyCode::ControlLeft],
        );

        let direction =
            self.camera.forward() * forward + self.camera.right() * right + Vec3::Y * up;
        let boost = if input.held(KeyCode::ShiftLeft) {
            4.0
        } else {
            1.0
        };
        self.camera.position += direction.normalize_or_zero() * self.speed * boost * dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_is_perpendicular_to_forward() {
        for yaw in [0.0f32, 0.7, 2.0, -1.3] {
            let cam = FlyCamera::new(Vec3::ZERO, yaw, 0.3).camera;
            assert!(cam.forward().dot(cam.right()).abs() < 1e-5);
            // right × up should point backwards (right-handed, looking down -Z).
            assert!(cam.right().cross(Vec3::Y).dot(cam.forward()) < 0.0);
        }
    }

    #[test]
    fn yaw_zero_looks_down_negative_z() {
        let cam = FlyCamera::new(Vec3::ZERO, 0.0, 0.0).camera;
        assert!((cam.forward() - Vec3::NEG_Z).length() < 1e-6);
        assert!((cam.right() - Vec3::X).length() < 1e-6);
    }
}
