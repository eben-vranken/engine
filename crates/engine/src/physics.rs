//! rapier3d world built from the scene: colliders for everything solid
//! (the character controller queries them), and while the game runs,
//! dynamic bodies for objects with a `PhysicsBody`.

use crate::assets::{Assets, Resolved};
use crate::render::mesh::{MeshData, Shape};
use crate::scene::{Component, PhysicsBody, Scene, Transform};
use crate::zones::Active;
use glam::{Quat, Vec3};
use rapier3d::prelude::*;
use std::collections::HashMap;

/// The simulation advances in fixed steps of this many seconds.
const STEP: f32 = 1.0 / 60.0;
/// At most this much time is caught up per frame, so a hitch doesn't
/// trigger a burst of steps.
const MAX_CATCH_UP: f32 = 0.1;
/// For pushing: roughly the player's mass (kg).
const PLAYER_MASS: f32 = 80.0;

/// Collider groups: solid colliders are in every group (rapier's default);
/// walk-through ones only in their own, which gameplay queries skip.
const SOLID: Group = Group::GROUP_1;
const WALK_THROUGH: Group = Group::GROUP_2;

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
    /// Colliders without a body, with their object and pose relative to it.
    placed: Vec<(ColliderHandle, usize, Pose)>,
    /// Dynamic bodies and their objects.
    dynamic: Vec<(RigidBodyHandle, usize)>,
    /// Time not yet simulated (less than one step).
    accumulator: f32,
}

impl PhysicsWorld {
    /// Everything static: for the editor.
    pub fn from_scene(scene: &Scene, assets: &mut Assets) -> Self {
        Self::build(scene, assets, false)
    }

    /// For the running game: objects with a `PhysicsBody` get dynamic bodies.
    pub fn for_game(scene: &Scene, assets: &mut Assets) -> Self {
        Self::build(scene, assets, true)
    }

    fn build(scene: &Scene, assets: &mut Assets, game: bool) -> Self {
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
            placed: Vec::new(),
            dynamic: Vec::new(),
            accumulator: 0.0,
        };
        let mut object_bodies = HashMap::new();
        for (object, geometry, transform, solid) in scene.colliders() {
            let body = game.then(|| physics_body(scene, object)).flatten();
            let collider = match assets.resolve(geometry) {
                Resolved::Shape(shape) => collider_for(shape, transform),
                // Moving triangle meshes don't collide well; a hull does.
                Resolved::Model(model) => mesh_collider(&model.mesh, transform, body.is_some()),
            };
            let Some(mut collider) = collider else {
                continue;
            };
            collider.user_data = object as u128;
            let relative = object_pose(transform).inverse() * *collider.position();
            if !solid {
                collider
                    .set_collision_groups(InteractionGroups::all().with_memberships(WALK_THROUGH));
            }
            match body {
                Some(body) => {
                    let handle = *object_bodies.entry(object).or_insert_with(|| {
                        let rigid = RigidBodyBuilder::dynamic()
                            .pose(object_pose(transform))
                            .sleeping(body.starts_asleep)
                            .build();
                        let handle = world.bodies.insert(rigid);
                        world.dynamic.push((handle, object));
                        handle
                    });
                    collider.set_mass(body.mass.max(0.01));
                    let collider =
                        world
                            .colliders
                            .insert_with_parent(collider, handle, &mut world.bodies);
                    // Only takes effect once the collider has a parent.
                    world.colliders[collider].set_position_wrt_parent(relative);
                }
                None => {
                    let handle = world.colliders.insert(collider);
                    world.placed.push((handle, object, relative));
                }
            }
        }
        // One step registers the colliders with the broad phase so queries see them.
        world.step(1.0 / 60.0);
        world
    }

    /// Moves an object's colliders along with it (behaviors). The move
    /// shows in queries after the next `step`.
    pub fn move_object(&mut self, object: usize, transform: &Transform) {
        let pose = object_pose(transform);
        for &(handle, owner, relative) in &self.placed {
            if owner == object
                && let Some(collider) = self.colliders.get_mut(handle)
            {
                collider.set_position(pose * relative);
            }
        }
    }

    /// Simulates `dt` seconds in fixed steps; leftover time carries over.
    pub fn advance(&mut self, dt: f32) {
        self.accumulator = (self.accumulator + dt).min(MAX_CATCH_UP);
        while self.accumulator >= STEP {
            self.step(STEP);
            self.accumulator -= STEP;
        }
    }

    /// Dynamic bodies outside the simulation radius are disabled: frozen
    /// where they are (with their velocity) and out of collisions, until
    /// their zone is active again.
    pub fn set_active(&mut self, active: &Active) {
        for &(handle, object) in &self.dynamic {
            if let Some(body) = self.bodies.get_mut(handle)
                && body.is_enabled() != active.object(object)
            {
                body.set_enabled(active.object(object));
            }
        }
    }

    /// Where each awake dynamic body is now: its object, position and rotation.
    pub fn body_poses(&self) -> impl Iterator<Item = (usize, Vec3, Quat)> + '_ {
        self.dynamic.iter().filter_map(|&(handle, object)| {
            let body = self.bodies.get(handle)?;
            (body.is_enabled() && !body.is_sleeping())
                .then(|| (object, body.translation(), *body.rotation()))
        })
    }

    /// The player walking into dynamic bodies shoves them along at up to
    /// its own speed; heavier bodies move slower. The capsule runs from
    /// `bottom` up `height` meters.
    pub fn push_bodies(&mut self, bottom: Vec3, height: f32, radius: f32, velocity: Vec3) {
        let horizontal = velocity.with_y(0.0);
        if horizontal.length() < 0.01 || self.dynamic.is_empty() {
            return;
        }
        let reach = radius + 0.08;
        let center = bottom + Vec3::Y * height * 0.5;
        let shape = SharedShape::capsule_y((height * 0.5 - reach).max(0.01), reach);
        let touched: Vec<RigidBodyHandle> = self
            .queries()
            .intersect_shape(Pose::from_translation(center), shape.as_ref())
            .filter_map(|(_, collider)| collider.parent())
            .collect();
        for handle in touched {
            let Some(body) = self.bodies.get_mut(handle) else {
                continue;
            };
            let away = (body.translation() - center)
                .with_y(0.0)
                .normalize_or_zero();
            let push =
                horizontal.dot(away) * (2.0 * PLAYER_MASS / (PLAYER_MASS + body.mass())).min(1.0);
            let velocity = body.linvel();
            let along = velocity.dot(away);
            if push > along {
                body.set_linvel(velocity + away * (push - along), true);
            }
        }
    }

    /// The solid object right under `point`, at most `distance` below it.
    pub fn object_below(&self, point: Vec3, distance: f32) -> Option<usize> {
        let ray = Ray::new(point, Vec3::NEG_Y);
        let (handle, _) = self.queries().cast_ray(&ray, distance, true)?;
        Some(self.colliders.get(handle)?.user_data as usize)
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
        let (handle, distance) = self.editor_queries().cast_ray(&ray, max_distance, true)?;
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
            .editor_queries()
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

    /// Scene queries (ray and shape casts) for gameplay: only what blocks
    /// the player, not walk-through objects.
    pub fn queries(&self) -> QueryPipeline<'_> {
        let solid = InteractionGroups::all().with_filter(SOLID);
        self.query_pipeline(QueryFilter::default().groups(solid))
    }

    /// Queries against everything, walk-through objects included, for
    /// picking and placing in the editor.
    fn editor_queries(&self) -> QueryPipeline<'_> {
        self.query_pipeline(QueryFilter::default())
    }

    fn query_pipeline<'a>(&'a self, filter: QueryFilter<'a>) -> QueryPipeline<'a> {
        self.broad_phase.as_query_pipeline(
            self.narrow_phase.query_dispatcher(),
            &self.bodies,
            &self.colliders,
            filter,
        )
    }
}

fn physics_body(scene: &Scene, object: usize) -> Option<&PhysicsBody> {
    scene.objects[object]
        .components
        .iter()
        .find_map(|c| match c {
            Component::PhysicsBody(body) => Some(body),
            _ => None,
        })
}

fn object_pose(transform: &Transform) -> Pose {
    Pose::from_parts(transform.position, transform.quat())
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
/// A collider for a model: its exact triangles, or with `convex` its
/// convex hull (needed for bodies that move).
fn mesh_collider(mesh: &MeshData, transform: &Transform, convex: bool) -> Option<Collider> {
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
    let shape = if convex {
        SharedShape::convex_hull(&vertices)
    } else {
        SharedShape::trimesh(vertices, indices)
            .inspect_err(|err| log::error!("bad model mesh: {err}"))
            .ok()
    }?;
    Some(
        ColliderBuilder::new(shape)
            .position(Pose::from_parts(transform.position, transform.quat()))
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Component, test_scene};
    use crate::zones::ZoneMap;
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
    fn walk_through_objects_are_pickable_but_not_solid() {
        let mut scene = test_scene(vec![(Shape::Box { size: Vec3::ONE }, Vec3::NEG_Z * 5.0)]);
        if let Component::Primitive(p) = &mut scene.objects[0].components[0] {
            p.collision = false;
        }
        let world = PhysicsWorld::from_scene(&scene, &mut Assets::default());
        assert_eq!(
            world.pick(Vec3::ZERO, Vec3::NEG_Z, 100.0).map(|h| h.0),
            Some(0)
        );
        assert!(world.surface(Vec3::ZERO, Vec3::NEG_Z, 100.0).is_some());
        let ray = Ray::new(Vec3::ZERO, Vec3::NEG_Z);
        assert!(world.queries().cast_ray(&ray, 100.0, true).is_none());
    }

    #[test]
    fn moved_objects_are_hit_where_they_are_now() {
        let scene = test_scene(vec![(
            Shape::Plane {
                size: Vec2::splat(2.0),
            },
            Vec3::ZERO,
        )]);
        let mut world = PhysicsWorld::from_scene(&scene, &mut Assets::default());
        let down = |world: &PhysicsWorld| world.surface(Vec3::Y * 5.0, Vec3::NEG_Y, 10.0);
        assert!(down(&world).unwrap().0.y.abs() < 1e-3);

        world.move_object(0, &Transform::at(Vec3::Y * 2.0));
        world.step(1.0 / 60.0);
        let (point, _) = down(&world).unwrap();
        assert!(
            (point.y - 2.0).abs() < 1e-3,
            "plane now at 2 m, hit {point}"
        );
    }

    /// A floor (0) and a box (1) with a `PhysicsBody` 3 m above it.
    fn falling_box() -> Scene {
        let mut scene = test_scene(vec![
            (
                Shape::Box {
                    size: Vec3::new(10.0, 1.0, 10.0),
                },
                Vec3::new(0.0, -0.5, 0.0),
            ),
            (Shape::Box { size: Vec3::ONE }, Vec3::new(0.0, 3.5, 0.0)),
        ]);
        scene.objects[1]
            .components
            .push(Component::PhysicsBody(PhysicsBody::default()));
        scene
    }

    #[test]
    fn bodies_fall_in_the_game_but_not_in_the_editor() {
        let scene = falling_box();
        let mut editor = PhysicsWorld::from_scene(&scene, &mut Assets::default());
        editor.advance(0.1);
        assert_eq!(editor.body_poses().count(), 0);

        let mut game = PhysicsWorld::for_game(&scene, &mut Assets::default());
        for _ in 0..180 {
            game.advance(1.0 / 60.0);
        }
        // It may have fallen asleep after settling, so read the body itself.
        assert_eq!(game.dynamic.len(), 1);
        assert_eq!(game.dynamic[0].1, 1);
        let position = game.bodies[game.dynamic[0].0].translation();
        assert!(
            (position.y - 0.5).abs() < 0.05,
            "resting on the floor, at {position}"
        );
        assert_eq!(
            game.object_below(position + Vec3::Y, 2.0),
            Some(1),
            "the box"
        );
        let beside = position + Vec3::new(2.0, 0.0, 0.0);
        assert_eq!(game.object_below(beside, 2.0), Some(0), "the floor");
    }

    #[test]
    fn bodies_outside_the_simulation_radius_wait_in_the_air() {
        let mut scene = falling_box();
        scene.zones.push(crate::scene::Zone { name: "far".into() });
        scene.objects[1].zone = "far".into();
        let map = ZoneMap::new(&scene);
        let mut game = PhysicsWorld::for_game(&scene, &mut Assets::default());
        let height = |game: &PhysicsWorld| game.bodies[game.dynamic[0].0].translation().y;
        game.set_active(&Active::new(&scene, &map, Some(0)));
        let start = height(&game);
        for _ in 0..60 {
            game.advance(1.0 / 60.0);
        }
        assert_eq!(height(&game), start, "waiting in the air");
        assert_eq!(game.body_poses().count(), 0);

        game.set_active(&Active::new(&scene, &map, Some(1)));
        for _ in 0..60 {
            game.advance(1.0 / 60.0);
        }
        assert!(height(&game) < 3.0, "falling once its zone is active");
    }

    #[test]
    fn walking_into_a_body_pushes_it() {
        let scene = falling_box();
        let mut game = PhysicsWorld::for_game(&scene, &mut Assets::default());
        for _ in 0..180 {
            game.advance(1.0 / 60.0);
        }
        // A player standing right next to the box, walking toward +X into it.
        let start = game.bodies.iter().next().unwrap().1.translation();
        for _ in 0..30 {
            game.push_bodies(start - Vec3::new(0.85, 0.5, 0.0), 1.8, 0.35, Vec3::X * 3.0);
            game.advance(1.0 / 60.0);
        }
        let end = game.bodies.iter().next().unwrap().1.translation();
        assert!(end.x - start.x > 0.3, "moved from {start} to {end}");
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
