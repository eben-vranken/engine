//! rapier3d world. For now it only holds static colliders built from the
//! scene; the character controller queries it.

use crate::assets::{Assets, Resolved};
use crate::render::mesh::{MeshData, Shape};
use crate::scene::{Scene, Transform};
use glam::Vec3;
use rapier3d::prelude::*;

pub struct PhysicsWorld {
    pipeline: PhysicsPipeline,
    params: IntegrationParameters,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    soft_bodies: SoftBodySet,
    ccd: CCDSolver,
}

impl PhysicsWorld {
    pub fn from_scene(scene: &Scene, assets: &mut Assets) -> Self {
        let mut world = Self {
            pipeline: PhysicsPipeline::new(),
            params: IntegrationParameters::default(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            soft_bodies: SoftBodySet::new(),
            ccd: CCDSolver::new(),
        };
        for (object, geometry, _, transform) in scene.renderables() {
            let collider = match assets.resolve(geometry) {
                Resolved::Shape(shape) => collider_for(shape, transform),
                Resolved::Mesh(mesh) => mesh_collider(&mesh, transform),
            };
            if let Some(mut collider) = collider {
                collider.user_data = object as u128;
                world.colliders.insert(collider);
            }
        }
        // One step registers the colliders with the broad phase so queries see them.
        world.step(1.0 / 60.0);
        world
    }

    pub fn step(&mut self, dt: f32) {
        self.params.dt = dt;
        self.pipeline.step(
            Vector::Y * -9.81,
            &self.params,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            &mut self.ccd,
            &(),
            &(),
        );
    }

    /// The scene object (index into `Scene::objects`) the ray hits first,
    /// and the distance to it.
    pub fn pick(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<(usize, f32)> {
        let ray = Ray::new(origin, direction);
        let (handle, distance) = self.queries().cast_ray(&ray, max_distance, true)?;
        let object = self.colliders.get(handle)?.user_data as usize;
        Some((object, distance))
    }

    /// Where the ray first hits a surface, and the surface normal there.
    pub fn surface(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<(Vec3, Vec3)> {
        let ray = Ray::new(origin, direction);
        let (_, hit) = self
            .queries()
            .cast_ray_and_get_normal(&ray, max_distance, true)?;
        // Triangle meshes report the face's own normal, which can point away
        // from the ray (back faces, flipped winding); turn it toward the ray.
        let normal = if hit.normal.dot(direction) > 0.0 {
            -hit.normal
        } else {
            hit.normal
        };
        Some((ray.point_at(hit.time_of_impact), normal))
    }

    /// Scene queries (ray and shape casts) against everything in the world.
    pub fn queries(&self) -> QueryPipeline<'_> {
        self.broad_phase.as_query_pipeline(
            self.narrow_phase.query_dispatcher(),
            &self.bodies,
            &self.colliders,
            QueryFilter::default(),
        )
    }
}

/// A collider matching the primitive's render mesh. Scale is baked into the
/// shape, since colliders can't be scaled after the fact.
fn collider_for(shape: Shape, transform: &Transform) -> Option<Collider> {
    let scale = transform.scale.abs();
    let collider_shape = match shape {
        Shape::Box { size } => {
            let half = size * scale * 0.5;
            SharedShape::cuboid(half.x, half.y, half.z)
        }
        // A thin slab just under the quad, so it can be stood on.
        Shape::Plane { size } => {
            SharedShape::cuboid(size.x * scale.x * 0.5, 0.005, size.y * scale.z * 0.5)
        }
        // Everything else is convex: wrap its render mesh, so collision matches what you see.
        shape => {
            let points: Vec<Vector> = MeshData::from_shape(shape)
                .vertices
                .iter()
                .map(|v| Vector::from(v.position) * transform.scale)
                .collect();
            SharedShape::convex_hull(&points)?
        }
    };
    let rotation = transform.quat();
    let offset = match shape {
        Shape::Plane { .. } => Vector::Y * -0.005,
        _ => Vector::ZERO,
    };
    Some(
        ColliderBuilder::new(collider_shape)
            .position(Pose::from_parts(
                transform.position + rotation * offset,
                rotation,
            ))
            .build(),
    )
}

/// An exact triangle-mesh collider for a model (static only), scale baked in.
fn mesh_collider(mesh: &MeshData, transform: &Transform) -> Option<Collider> {
    let vertices: Vec<Vector> = mesh
        .vertices
        .iter()
        .map(|v| Vector::from(v.position) * transform.scale)
        .collect();
    let indices: Vec<[u32; 3]> = mesh
        .indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    let shape = SharedShape::trimesh(vertices, indices)
        .inspect_err(|err| log::error!("bad model mesh: {err}"))
        .ok()?;
    Some(
        ColliderBuilder::new(shape)
            .position(Pose::from_parts(transform.position, transform.quat()))
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::test_scene;
    use glam::{Vec2, Vec3};

    #[test]
    fn ray_hits_static_box() {
        let scene = test_scene(vec![(
            Shape::Box {
                size: Vec3::new(10.0, 1.0, 10.0),
            },
            Vec3::new(0.0, -0.5, 0.0),
        )]);
        let world = PhysicsWorld::from_scene(&scene, &mut Assets::default());
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y);
        let (_, toi) = world
            .queries()
            .cast_ray(&ray, 100.0, true)
            .expect("ray should hit the floor");
        assert!((toi - 5.0).abs() < 1e-3);
    }

    #[test]
    fn pick_returns_the_nearest_object() {
        let scene = test_scene(vec![
            (Shape::Box { size: Vec3::ONE }, Vec3::new(0.0, 0.0, -10.0)),
            (Shape::Box { size: Vec3::ONE }, Vec3::new(0.0, 0.0, -5.0)),
            (Shape::Box { size: Vec3::ONE }, Vec3::new(3.0, 0.0, -2.0)),
        ]);
        let world = PhysicsWorld::from_scene(&scene, &mut Assets::default());
        let (object, distance) = world.pick(Vec3::ZERO, Vec3::NEG_Z, 100.0).unwrap();
        assert_eq!(object, 1);
        assert!((distance - 4.5).abs() < 1e-3);
        assert!(world.pick(Vec3::ZERO, Vec3::Y, 100.0).is_none());
    }

    #[test]
    fn surface_normal_faces_the_ray() {
        // Hit a plane from above and below.
        let scene = test_scene(vec![(
            Shape::Plane {
                size: Vec2::splat(4.0),
            },
            Vec3::ZERO,
        )]);
        let world = PhysicsWorld::from_scene(&scene, &mut Assets::default());
        let (point, normal) = world.surface(Vec3::Y * 3.0, Vec3::NEG_Y, 10.0).unwrap();
        assert!(point.distance(Vec3::ZERO) < 1e-3);
        assert!(normal.distance(Vec3::Y) < 1e-3);
        let (_, normal) = world.surface(Vec3::NEG_Y * 3.0, Vec3::Y, 10.0).unwrap();
        assert!(normal.distance(Vec3::NEG_Y) < 1e-3);
    }

    #[test]
    fn every_shape_gets_a_collider() {
        let shapes = [
            Shape::Plane {
                size: Vec2::splat(2.0),
            },
            Shape::Wedge { size: Vec3::ONE },
            Shape::Cylinder {
                radius: 0.5,
                height: 1.0,
                segments: 6,
            },
            Shape::Sphere {
                radius: 0.5,
                segments: 8,
                rings: 4,
            },
        ];
        for shape in shapes {
            assert!(
                collider_for(shape, &Transform::default()).is_some(),
                "{shape:?}"
            );
        }
    }

    #[test]
    fn scale_is_baked_into_the_collider() {
        let mut scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::ZERO)]);
        scene.objects[0].transform.scale = Vec3::new(1.0, 4.0, 1.0);
        let world = PhysicsWorld::from_scene(&scene, &mut Assets::default());
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y);
        let (_, toi) = world.queries().cast_ray(&ray, 100.0, true).unwrap();
        // A unit box scaled ×4 in Y has its top at y = 2.
        assert!((toi - 3.0).abs() < 1e-3, "hit at y = {}", 5.0 - toi);
    }
}
