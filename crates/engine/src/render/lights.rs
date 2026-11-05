//! Lights for one frame: which of the scene's lights are used, how bright
//! each is right now (flicker), and which get shadow maps. Also the
//! matrices the shadow maps are rendered with.
//!
//! A point light's shadow map is six square depth images, one per cube
//! face, each a 90° perspective view along one axis. The shader picks the
//! face from the direction to the light and uses the same matrix it was
//! rendered with, so there's no cube-map orientation convention to match.

use crate::scene::LightSource;
use bytemuck::{Pod, Zeroable};
use glam::camera::rh;
use glam::{Mat4, Vec3, Vec4};

/// Most lights shaded at once (the nearest ones).
pub const MAX_LIGHTS: usize = 32;
/// Most lights with shadow maps at once (the nearest that want one).
pub const MAX_SHADOWED: usize = 8;
/// Near plane of the cube shadow faces, meters.
pub const SHADOW_NEAR: f32 = 0.05;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuLight {
    /// xyz: position, w: range.
    pub position: Vec4,
    /// rgb: color × intensity, w: shadow slot, or -1 for none.
    pub color: Vec4,
}

/// A scene light as used this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameLight {
    /// The object it belongs to; left out of its own shadow map.
    pub object: usize,
    pub position: Vec3,
    pub range: f32,
    /// Color × intensity × flicker.
    pub color: Vec3,
    pub shadow: Option<usize>,
}

impl FrameLight {
    pub fn gpu(&self) -> GpuLight {
        GpuLight {
            position: self.position.extend(self.range),
            color: self
                .color
                .extend(self.shadow.map_or(-1.0, |slot| slot as f32)),
        }
    }
}

/// Brightness multiplier of a flickering light at `time` seconds: a few
/// sines at unrelated rates, so it never visibly repeats. `seed` keeps
/// neighbouring lights out of step.
pub fn flicker(amount: f32, time: f32, seed: f32) -> f32 {
    if amount <= 0.0 {
        return 1.0;
    }
    let t = time + seed * 17.31;
    let wobble = 0.5
        + 0.22 * (t * 6.1).sin()
        + 0.16 * (t * 13.7 + 1.3).sin()
        + 0.12 * (t * 27.9 + 2.9).sin();
    1.0 - amount.min(1.0) * 0.55 * wobble
}

/// The lights to shade this frame, nearest to the camera first; the first
/// [`MAX_SHADOWED`] that want shadows get a shadow slot.
pub fn select<'a>(
    lights: impl Iterator<Item = (usize, &'a LightSource, Vec3)>,
    camera: Vec3,
    time: f32,
) -> Vec<FrameLight> {
    let mut candidates: Vec<(f32, usize, &LightSource, Vec3)> = lights
        .filter(|(_, l, _)| l.intensity > 0.0 && l.range > 0.0)
        .map(|(object, light, position)| {
            let reach = (position.distance(camera) - light.range).max(0.0);
            (reach, object, light, position)
        })
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut slots = 0;
    candidates
        .into_iter()
        .take(MAX_LIGHTS)
        .map(|(_, object, light, position)| {
            let shadow = (light.shadows && slots < MAX_SHADOWED).then(|| {
                slots += 1;
                slots - 1
            });
            FrameLight {
                object,
                position,
                range: light.range,
                color: light.color * light.intensity * flicker(light.flicker, time, object as f32),
                shadow,
            }
        })
        .collect()
}

/// Look direction and up vector of each cube face, in the order the shader
/// indexes them: +X, -X, +Y, -Y, +Z, -Z.
const FACES: [(Vec3, Vec3); 6] = [
    (Vec3::X, Vec3::NEG_Y),
    (Vec3::NEG_X, Vec3::NEG_Y),
    (Vec3::Y, Vec3::Z),
    (Vec3::NEG_Y, Vec3::NEG_Z),
    (Vec3::Z, Vec3::NEG_Y),
    (Vec3::NEG_Z, Vec3::NEG_Y),
];

/// View-projection of each cube face of a point light's shadow map.
pub fn cube_faces(position: Vec3, range: f32) -> [Mat4; 6] {
    let projection = rh::proj::directx::perspective(
        90f32.to_radians(),
        1.0,
        SHADOW_NEAR,
        range.max(SHADOW_NEAR * 2.0),
    );
    FACES.map(|(dir, up)| projection * rh::view::look_to_mat4(position, dir, up))
}

/// Which cube face sees the direction `d` (from the light): the axis it
/// points along most. Mirrors `cube_face` in the shader.
pub fn face_for(d: Vec3) -> usize {
    let a = d.abs();
    if a.x >= a.y && a.x >= a.z {
        if d.x >= 0.0 { 0 } else { 1 }
    } else if a.y >= a.z {
        if d.y >= 0.0 { 2 } else { 3 }
    } else if d.z >= 0.0 {
        4
    } else {
        5
    }
}

/// The sun's shadow map: an orthographic view along the sunlight covering
/// `radius` meters around `center`, `depth` meters deep either way. The
/// view is snapped to whole shadow texels so the shadows don't crawl as
/// the camera moves.
pub fn sun_matrix(center: Vec3, sun_direction: Vec3, radius: f32, resolution: u32) -> Mat4 {
    const DEPTH: f32 = 200.0;
    let dir = sun_direction.normalize_or(Vec3::NEG_Y);
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let view = rh::view::look_to_mat4(Vec3::ZERO, dir, up);
    let c = view.transform_point3(center);
    let texel = 2.0 * radius / resolution as f32;
    let (x, y) = ((c.x / texel).floor() * texel, (c.y / texel).floor() * texel);
    // View space looks down -Z: distance in front is -z.
    let projection = rh::proj::directx::orthographic(
        x - radius,
        x + radius,
        y - radius,
        y + radius,
        -c.z - DEPTH,
        -c.z + DEPTH,
    );
    projection * view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(range: f32, shadows: bool) -> LightSource {
        LightSource {
            range,
            shadows,
            ..Default::default()
        }
    }

    #[test]
    fn nearest_lights_come_first_and_get_shadows() {
        let lights = [light(2.0, true), light(2.0, true), light(2.0, false)];
        let positions = [Vec3::X * 50.0, Vec3::X * 5.0, Vec3::ZERO];
        let picked = select(
            lights
                .iter()
                .zip(positions)
                .enumerate()
                .map(|(i, (l, p))| (i, l, p)),
            Vec3::ZERO,
            0.0,
        );
        let order: Vec<usize> = picked.iter().map(|l| l.object).collect();
        assert_eq!(order, [2, 1, 0]);
        let slots: Vec<Option<usize>> = picked.iter().map(|l| l.shadow).collect();
        assert_eq!(slots, [None, Some(0), Some(1)]);
    }

    #[test]
    fn shadow_slots_run_out() {
        let lights: Vec<LightSource> = (0..MAX_SHADOWED + 3).map(|_| light(4.0, true)).collect();
        let picked = select(
            lights
                .iter()
                .enumerate()
                .map(|(i, l)| (i, l, Vec3::X * i as f32)),
            Vec3::ZERO,
            0.0,
        );
        assert_eq!(
            picked.iter().filter(|l| l.shadow.is_some()).count(),
            MAX_SHADOWED
        );
    }

    #[test]
    fn flicker_stays_in_range() {
        assert_eq!(flicker(0.0, 3.0, 1.0), 1.0);
        for i in 0..500 {
            let f = flicker(1.0, i as f32 * 0.037, 2.0);
            assert!((0.3..=1.0).contains(&f), "{f}");
        }
    }

    #[test]
    fn every_direction_lands_inside_its_cube_face() {
        let position = Vec3::new(1.0, 2.0, 3.0);
        let faces = cube_faces(position, 10.0);
        for d in [
            Vec3::new(1.0, 0.3, -0.2),
            Vec3::new(-0.9, 0.8, 0.1),
            Vec3::new(0.1, 1.0, 0.95),
            Vec3::new(0.2, -1.0, -0.4),
            Vec3::new(-0.5, 0.5, 1.0),
            Vec3::new(0.7, -0.6, -1.0),
        ] {
            let face = face_for(d);
            let ndc = faces[face].project_point3(position + d.normalize() * 3.0);
            assert!(ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0, "{d} {face} {ndc}");
            assert!((0.0..=1.0).contains(&ndc.z), "{ndc}");
        }
    }

    #[test]
    fn sun_view_contains_the_area_around_the_camera() {
        let camera = Vec3::new(12.0, 3.0, -40.0);
        let m = sun_matrix(camera, Vec3::new(-0.4, -1.0, -0.3), 20.0, 2048);
        for offset in [Vec3::ZERO, Vec3::X * 15.0, Vec3::new(0.0, -10.0, 12.0)] {
            let ndc = m.project_point3(camera + offset);
            assert!(ndc.x.abs() < 1.0 && ndc.y.abs() < 1.0, "{ndc}");
            assert!((0.0..1.0).contains(&ndc.z), "{ndc}");
        }
        // Closer to the sun = smaller depth.
        let high = m.project_point3(camera + Vec3::Y * 5.0).z;
        let low = m.project_point3(camera).z;
        assert!(high < low);
    }
}
