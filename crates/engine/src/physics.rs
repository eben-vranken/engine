//! rapier3d world. For now it only holds static colliders built from the
//! scene; the character controller queries it.

use crate::render::mesh::{MeshData, Shape};
use crate::scene::{Object, Scene};
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
    pub fn from_scene(scene: &Scene) -> Self {
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
        for object in &scene.objects {
            if let Some(collider) = collider_for(object) {
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

fn collider_for(object: &Object) -> Option<Collider> {
    let shape = match object.shape {
        Shape::Box { size } => SharedShape::cuboid(size.x * 0.5, size.y * 0.5, size.z * 0.5),
        // A thin slab just under the quad, so it can be stood on.
        Shape::Plane { size } => SharedShape::cuboid(size.x * 0.5, 0.005, size.y * 0.5),
        // Everything else is convex: wrap its render mesh, so collision matches what you see.
        shape => {
            let points: Vec<Vector> = MeshData::from_shape(shape)
                .vertices
                .iter()
                .map(|v| Vector::from(v.position))
                .collect();
            SharedShape::convex_hull(&points)?
        }
    };
    let offset = match object.shape {
        Shape::Plane { .. } => Vector::Y * -0.005,
        _ => Vector::ZERO,
    };
    Some(
        ColliderBuilder::new(shape)
            .position(Pose::from_parts(
                object.position + object.rotation * offset,
                object.rotation,
            ))
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec2, Vec3};

    #[test]
    fn ray_hits_static_box() {
        let scene = Scene {
            objects: vec![Object::new(
                Shape::Box {
                    size: Vec3::new(10.0, 1.0, 10.0),
                },
                Vec3::new(0.0, -0.5, 0.0),
            )],
            ..Default::default()
        };
        let world = PhysicsWorld::from_scene(&scene);
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y);
        let (_, toi) = world
            .queries()
            .cast_ray(&ray, 100.0, true)
            .expect("ray should hit the floor");
        assert!((toi - 5.0).abs() < 1e-3);
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
                collider_for(&Object::new(shape, Vec3::ZERO)).is_some(),
                "{shape:?}"
            );
        }
    }
}
