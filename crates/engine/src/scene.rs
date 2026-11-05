//! What gets drawn. For now a flat list of primitive objects plus the
//! environment (light, ambient, fog); the RON scene format comes later.

use crate::render::mesh::Shape;
use glam::{Quat, Vec3};

#[derive(Debug, Clone)]
pub struct Object {
    pub shape: Shape,
    pub position: Vec3,
    pub rotation: Quat,
    /// Linear RGB tint multiplied with the grid texture.
    pub color: Vec3,
}

impl Object {
    pub fn new(shape: Shape, position: Vec3) -> Self {
        Self {
            shape,
            position,
            rotation: Quat::IDENTITY,
            color: Vec3::ONE,
        }
    }

    pub fn rotated(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn colored(mut self, color: Vec3) -> Self {
        self.color = color;
        self
    }
}

/// Global lighting and atmosphere. All colors are linear RGB; light colors
/// include intensity, so values above 1 are expected.
#[derive(Debug, Clone)]
pub struct Environment {
    /// Direction the sunlight travels (from the sun toward the ground).
    pub sun_direction: Vec3,
    pub sun_color: Vec3,
    /// Ambient light from above and below; normals blend between them.
    pub ambient_sky: Vec3,
    pub ambient_ground: Vec3,
    pub fog_color: Vec3,
    /// Fog per meter; 0 turns fog off.
    pub fog_density: f32,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            sun_direction: Vec3::new(-0.45, -1.0, -0.3).normalize(),
            sun_color: Vec3::new(1.0, 0.9, 0.78) * 2.6,
            ambient_sky: Vec3::new(0.16, 0.18, 0.24),
            ambient_ground: Vec3::new(0.05, 0.045, 0.04),
            fog_color: Vec3::new(0.045, 0.05, 0.065),
            fog_density: 0.035,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub objects: Vec<Object>,
    pub environment: Environment,
}
