//! Frustum culling: skip drawing what the camera can't see. Each draw has a
//! bounding sphere; a sphere entirely outside one of the view's six planes
//! is off screen.

use glam::{Mat4, Vec3, Vec4};

/// The six planes of a view volume, normals pointing inward:
/// `normal · p + d >= 0` inside.
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// From a view-projection matrix with wgpu's clip space (x and y in
    /// -w..w, depth in 0..w). Each plane is a combination of the matrix's
    /// rows (the Gribb–Hartmann method).
    pub fn new(view_proj: Mat4) -> Self {
        let [x, y, z, w] = [0, 1, 2, 3].map(|i| view_proj.row(i));
        let planes = [w + x, w - x, w + y, w - y, z, w - z]
            .map(|p| p / p.truncate().length().max(f32::EPSILON));
        Self { planes }
    }

    /// False only if the sphere is certainly outside.
    pub fn sees_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| p.truncate().dot(center) + p.w >= -radius)
    }
}

/// A sphere around a mesh's bounding box (min, max), in its own space.
pub fn bounding_sphere((min, max): (Vec3, Vec3)) -> (Vec3, f32) {
    if min.cmpgt(max).any() {
        return (Vec3::ZERO, 0.0); // no vertices
    }
    ((min + max) / 2.0, (max - min).length() / 2.0)
}

/// A local sphere moved into the world by `model` (scaled by its largest
/// axis, so it still encloses the mesh).
pub fn world_sphere(model: &Mat4, (center, radius): (Vec3, f32)) -> (Vec3, f32) {
    let scale = [model.x_axis, model.y_axis, model.z_axis]
        .map(|axis| axis.truncate().length())
        .into_iter()
        .fold(0.0, f32::max);
    (model.transform_point3(center), radius * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    fn frustum() -> Frustum {
        // At the origin looking down -Z, 90° vertical FOV, square image.
        let camera = Camera {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 90.0,
            near: 0.1,
            far: 100.0,
            ortho: None,
        };
        Frustum::new(camera.view_projection(1.0))
    }

    #[test]
    fn sees_what_is_ahead_and_not_behind() {
        let f = frustum();
        assert!(f.sees_sphere(Vec3::new(0.0, 0.0, -10.0), 1.0));
        assert!(!f.sees_sphere(Vec3::new(0.0, 0.0, 10.0), 1.0));
        assert!(!f.sees_sphere(Vec3::new(0.0, 0.0, -200.0), 1.0), "past far");
    }

    #[test]
    fn spheres_poking_into_the_view_count() {
        let f = frustum();
        // 45° half-angle: at depth 10 the edge is at x = 10.
        assert!(!f.sees_sphere(Vec3::new(12.0, 0.0, -10.0), 1.0));
        assert!(f.sees_sphere(Vec3::new(12.0, 0.0, -10.0), 2.0));
        // Big objects around the camera are always drawn.
        assert!(f.sees_sphere(Vec3::new(0.0, 0.0, 5.0), 6.0));
    }

    #[test]
    fn world_spheres_follow_scale_and_position() {
        let model = Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 3.0, 1.0),
            glam::Quat::IDENTITY,
            Vec3::X * 5.0,
        );
        let local = bounding_sphere((Vec3::splat(-1.0), Vec3::ONE));
        let (center, radius) = world_sphere(&model, local);
        assert_eq!(center, Vec3::X * 5.0);
        assert!((radius - 3.0 * 3f32.sqrt()).abs() < 1e-5);
    }
}
