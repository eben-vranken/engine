//! The path tracer behind the light bake (CPU, all cores).
//!
//! Every static triangle goes into one parry triangle mesh, which keeps
//! its own bounding-volume hierarchy for fast ray casts. From a point on a
//! surface, rays leave in random directions (more of them near the normal,
//! matching how much light each direction contributes) and bounce around
//! the scene; what they pick up is the light arriving at that point.
//!
//! What's traced is only what the renderer can't do in real time: light
//! from the sky, from emissive surfaces, and everything that has bounced
//! at least once. Direct sun and lamp light at the point itself is left
//! out, because the renderer adds it live with shadows.

use glam::{Vec2, Vec3};
use rapier3d::parry::query::{Ray, RayCast};
use rapier3d::parry::shape::TriMesh;

/// Bounces followed after the first hit.
const BOUNCES: usize = 3;
/// How far ray origins are lifted off surfaces (meters).
const LIFT: f32 = 0.005;
const FAR: f32 = 10_000.0;
/// Step of the R2 sequence (from the plastic number): the most even 2D
/// low-discrepancy sequence, and every prefix of it is evenly spread.
const R2: Vec2 = Vec2::new(0.754_877_7, 0.569_840_3);

/// A light whose bounce is baked.
#[derive(Debug, Clone)]
pub struct BakeLight {
    pub position: Vec3,
    /// Color × intensity.
    pub color: Vec3,
    pub range: f32,
    /// Shadow rays stop this short of the light, so the light's own
    /// object (a lantern around it) doesn't block it.
    pub clearance: f32,
    /// Where a cone light shines, and its beam's cosines (`None`: every
    /// way).
    pub direction: Vec3,
    pub cone: Option<(f32, f32)>,
}

/// What a bake traces against.
pub struct World {
    mesh: Option<TriMesh>,
    /// Per triangle: normal, albedo, emissive.
    normals: Vec<Vec3>,
    albedo: Vec<Vec3>,
    emissive: Vec<Vec3>,
    pub sun_direction: Vec3,
    pub sun_color: Vec3,
    pub sky: Vec3,
    pub ground: Vec3,
    pub lights: Vec<BakeLight>,
}

/// One triangle for the world: corners, albedo, emissive.
pub type Triangle = ([Vec3; 3], Vec3, Vec3);

impl World {
    pub fn new(
        triangles: Vec<Triangle>,
        sun_direction: Vec3,
        sun_color: Vec3,
        sky: Vec3,
        ground: Vec3,
        lights: Vec<BakeLight>,
    ) -> Self {
        let mut vertices = Vec::with_capacity(triangles.len() * 3);
        let mut indices = Vec::with_capacity(triangles.len());
        let mut normals = Vec::with_capacity(triangles.len());
        let mut albedo = Vec::with_capacity(triangles.len());
        let mut emissive = Vec::with_capacity(triangles.len());
        for (corners, a, e) in triangles {
            let n = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
            if n.length_squared() < 1e-12 {
                continue;
            }
            let base = vertices.len() as u32;
            vertices.extend(corners);
            indices.push([base, base + 1, base + 2]);
            normals.push(n.normalize());
            albedo.push(a.clamp(Vec3::ZERO, Vec3::splat(0.95)));
            emissive.push(e);
        }
        let mesh = if indices.is_empty() {
            None
        } else {
            TriMesh::new(vertices, indices).ok()
        };
        Self {
            mesh,
            normals,
            albedo,
            emissive,
            sun_direction: sun_direction.normalize_or(Vec3::NEG_Y),
            sun_color,
            sky,
            ground,
            lights,
        }
    }

    /// First hit along a ray: distance and triangle.
    fn cast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<(f32, usize)> {
        let mesh = self.mesh.as_ref()?;
        let ray = Ray::new(origin, dir);
        mesh.cast_local_ray_and_get_normal(&ray, max, false)
            .map(|hit| (hit.time_of_impact, hit.subshape as usize))
    }

    fn blocked(&self, origin: Vec3, dir: Vec3, max: f32) -> bool {
        max > 0.0 && self.cast(origin, dir, max).is_some()
    }

    /// The sky's light from a direction (no hit).
    fn sky_light(&self, dir: Vec3) -> Vec3 {
        self.ground.lerp(self.sky, dir.y * 0.5 + 0.5)
    }

    /// Direct sun and baked lamp light reaching a point (as the renderer
    /// would add it: before multiplying by the surface's albedo).
    fn direct(&self, p: Vec3, n: Vec3) -> Vec3 {
        let origin = p + n * LIFT;
        let mut total = Vec3::ZERO;
        let facing = n.dot(-self.sun_direction);
        if facing > 0.0 && !self.blocked(origin, -self.sun_direction, FAR) {
            total += self.sun_color * facing;
        }
        for light in &self.lights {
            let to = light.position - p;
            let distance = to.length();
            if distance >= light.range || distance < 1e-4 {
                continue;
            }
            let l = to / distance;
            let facing = n.dot(l);
            if facing <= 0.0 {
                continue;
            }
            let beam = crate::scene::cone_falloff(light.cone, light.direction, -l);
            if beam <= 0.0 || self.blocked(origin, l, distance - light.clearance) {
                continue;
            }
            let ratio = distance / light.range;
            let window = (1.0 - ratio.powi(4)).clamp(0.0, 1.0);
            total +=
                light.color * (facing * beam * window * window / (distance * distance).max(0.01));
        }
        total
    }

    /// Light arriving at a point from `dir`. `None` if the ray first hits
    /// the back of a surface: the point is inside something.
    pub fn incoming(&self, origin: Vec3, mut dir: Vec3, rng: &mut Rng) -> Option<Vec3> {
        let mut origin = origin;
        let mut throughput = Vec3::ONE;
        let mut light = Vec3::ZERO;
        for bounce in 0..=BOUNCES {
            let Some((t, tri)) = self.cast(origin, dir, FAR) else {
                light += throughput * self.sky_light(dir);
                break;
            };
            let n = self.normals[tri];
            if n.dot(dir) > 0.0 {
                if bounce == 0 {
                    return None;
                }
                break;
            }
            let p = origin + dir * t;
            light += throughput * self.emissive[tri];
            light += throughput * self.albedo[tri] * self.direct(p, n);
            throughput *= self.albedo[tri];
            let keep = throughput.max_element().min(1.0);
            if keep < 0.01 {
                break;
            }
            // Russian roulette: past the first bounce a path goes on with
            // the chance of what it still carries, and the survivors count
            // for more, so the average stays the same but dim paths stop.
            if bounce > 0 {
                if rng.float() >= keep {
                    break;
                }
                throughput /= keep;
            }
            origin = p + n * LIFT;
            dir = rng.cosine(n);
        }
        Some(light)
    }

    /// The light seen along a ray: the sky, or the surface it hits as it
    /// looks lit (emissive + albedo × (direct + `samples` rays of bounce)).
    /// Black if it hits the back of a surface.
    pub fn radiance(&self, origin: Vec3, dir: Vec3, samples: u32, rng: &mut Rng) -> Vec3 {
        let Some((t, tri)) = self.cast(origin, dir, FAR) else {
            return self.sky_light(dir);
        };
        let n = self.normals[tri];
        if n.dot(dir) > 0.0 {
            return Vec3::ZERO;
        }
        let p = origin + dir * t;
        let bounce = self.gather(p, n, samples, rng).unwrap_or(Vec3::ZERO);
        self.emissive[tri] + self.albedo[tri] * (self.direct(p, n) + bounce)
    }

    /// Average light arriving at a surface point with normal `n`, from
    /// `samples` rays. `None` if most rays start inside geometry.
    pub fn gather(&self, p: Vec3, n: Vec3, samples: u32, rng: &mut Rng) -> Option<Vec3> {
        let origin = p + n * LIFT;
        let mut sum = Vec3::ZERO;
        let mut inside = 0;
        // First directions from a low-discrepancy sequence (R2), shifted
        // per point: spread evenly over the hemisphere instead of clumped,
        // so the same samples give a smoother result.
        let shift = Vec2::new(rng.float(), rng.float());
        for i in 0..samples {
            let u = (shift + R2 * i as f32).fract();
            match self.incoming(origin, cosine_at(n, u.x, u.y), rng) {
                Some(l) => sum += l,
                None => inside += 1,
            }
        }
        let valid = samples - inside;
        // Points under a wall or inside a box see mostly back faces.
        (inside * 4 < samples && valid > 0).then(|| sum / valid as f32)
    }
}

/// Small fast random numbers (PCG), one per thread.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut rng = Self(
            seed.wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407),
        );
        rng.next();
        rng
    }

    fn next(&mut self) -> u32 {
        let old = self.0;
        self.0 = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let shifted = (((old >> 18) ^ old) >> 27) as u32;
        shifted.rotate_right((old >> 59) as u32)
    }

    /// Uniform in 0..1.
    pub fn float(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A direction around `n`, more likely near it (cosine-weighted), so
    /// averaging what the rays see gives the light a surface receives.
    pub fn cosine(&mut self, n: Vec3) -> Vec3 {
        let (u1, u2) = (self.float(), self.float());
        cosine_at(n, u1, u2)
    }
}

/// The cosine-weighted direction around `n` for two numbers in 0..1.
fn cosine_at(n: Vec3, u1: f32, u2: f32) -> Vec3 {
    let r = u1.sqrt();
    let phi = std::f32::consts::TAU * u2;
    let (x, y) = (r * phi.cos(), r * phi.sin());
    let z = (1.0 - u1).max(0.0).sqrt();
    let helper = if n.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let t = helper.cross(n).normalize();
    let b = n.cross(t);
    (t * x + b * y + n * z).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32, size: f32, up: bool) -> Vec<Triangle> {
        let (a, b, c, d) = (
            Vec3::new(-size, y, -size),
            Vec3::new(size, y, -size),
            Vec3::new(size, y, size),
            Vec3::new(-size, y, size),
        );
        let white = Vec3::splat(0.5);
        if up {
            vec![
                ([a, d, c], white, Vec3::ZERO),
                ([a, c, b], white, Vec3::ZERO),
            ]
        } else {
            vec![
                ([a, b, c], white, Vec3::ZERO),
                ([a, c, d], white, Vec3::ZERO),
            ]
        }
    }

    fn world(triangles: Vec<Triangle>) -> World {
        World::new(
            triangles,
            Vec3::NEG_Y,
            Vec3::ZERO,
            Vec3::ONE,
            Vec3::ZERO,
            Vec::new(),
        )
    }

    #[test]
    fn open_floor_sees_the_sky() {
        let w = world(quad(0.0, 10.0, true));
        let mut rng = Rng::new(1);
        let l = w.gather(Vec3::ZERO, Vec3::Y, 256, &mut rng).unwrap();
        // Cosine-weighted sky from white (up) to black (horizon side down):
        // the upper hemisphere averages to about 2/3 + bits.
        assert!(l.x > 0.5 && l.x < 0.9, "{l}");
    }

    #[test]
    fn a_roof_darkens_the_floor() {
        let mut triangles = quad(0.0, 10.0, true);
        triangles.extend(quad(0.5, 10.0, false));
        let w = world(triangles);
        let mut rng = Rng::new(2);
        let open = world(quad(0.0, 10.0, true))
            .gather(Vec3::ZERO, Vec3::Y, 256, &mut rng)
            .unwrap();
        let covered = w.gather(Vec3::ZERO, Vec3::Y, 256, &mut rng).unwrap();
        assert!(covered.x < open.x * 0.5, "{covered} vs {open}");
    }

    #[test]
    fn inside_geometry_is_invalid() {
        // A point below an upward-facing floor, looking up, sees its back.
        let w = world(quad(0.0, 10.0, true));
        let mut rng = Rng::new(3);
        assert!(
            w.gather(Vec3::new(0.0, -0.1, 0.0), Vec3::Y, 64, &mut rng)
                .is_none()
        );
    }

    #[test]
    fn an_emissive_ceiling_lights_the_floor() {
        let mut triangles = quad(0.0, 10.0, true);
        triangles.extend(
            quad(2.0, 10.0, false)
                .into_iter()
                .map(|(c, a, _)| (c, a, Vec3::splat(2.0))),
        );
        let w = World::new(
            triangles,
            Vec3::NEG_Y,
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::ZERO,
            Vec::new(),
        );
        let mut rng = Rng::new(4);
        let l = w.gather(Vec3::ZERO, Vec3::Y, 256, &mut rng).unwrap();
        assert!(l.x > 1.5, "{l}");
    }

    #[test]
    fn cosine_samples_stay_above_the_surface() {
        let mut rng = Rng::new(5);
        let n = Vec3::new(0.3, 0.9, -0.2).normalize();
        for _ in 0..1000 {
            assert!(rng.cosine(n).dot(n) >= 0.0);
        }
    }
}
