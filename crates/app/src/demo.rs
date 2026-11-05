//! A hard-coded grey-box scene for testing the renderer. Replaced by RON
//! scenes in the game project once the scene format exists (milestone 3).

use engine::glam::{Quat, Vec2, Vec3};
use engine::render::mesh::Shape;
use engine::scene::{Object, Scene};

const GREY: Vec3 = Vec3::new(0.62, 0.62, 0.62);
const STONE: Vec3 = Vec3::new(0.55, 0.52, 0.48);
const ACCENT: Vec3 = Vec3::new(0.85, 0.45, 0.15);

fn cuboid(size: Vec3, center: Vec3) -> Object {
    Object::new(Shape::Box { size }, center).colored(GREY)
}

/// A box resting on the floor at (x, z).
fn block(size: Vec3, x: f32, z: f32) -> Object {
    cuboid(size, Vec3::new(x, size.y * 0.5, z))
}

pub fn scene() -> Scene {
    let mut objects = vec![
        // Floor: 80 × 80 m, top at y = 0.
        cuboid(Vec3::new(80.0, 0.5, 80.0), Vec3::new(0.0, -0.25, 0.0)),
        // Reference cubes: 1 m and 2 m.
        block(Vec3::ONE, -2.0, -4.0),
        block(Vec3::splat(2.0), 1.5, -5.0),
        cuboid(Vec3::ONE, Vec3::new(1.5, 2.5, -5.0))
            .colored(ACCENT)
            .rotated(Quat::from_rotation_y(0.4)),
        // A wall with a doorway.
        block(Vec3::new(6.0, 4.0, 0.5), -7.0, -12.0),
        block(Vec3::new(6.0, 4.0, 0.5), 1.0, -12.0),
        cuboid(Vec3::new(2.0, 1.0, 0.5), Vec3::new(-3.0, 3.5, -12.0)),
        // A 30° ramp, 2 m tall.
        Object::new(
            Shape::Wedge {
                size: Vec3::new(3.0, 2.0, 2.0 / 30f32.to_radians().tan()),
            },
            Vec3::new(8.0, 1.0, -4.0),
        )
        .colored(STONE),
        // A plinth with a sphere on it.
        Object::new(
            Shape::Cylinder {
                radius: 0.6,
                height: 1.0,
                segments: 8,
            },
            Vec3::new(-6.0, 0.5, -3.0),
        )
        .colored(STONE),
        Object::new(
            Shape::Sphere {
                radius: 0.5,
                segments: 10,
                rings: 6,
            },
            Vec3::new(-6.0, 1.5, -3.0),
        )
        .colored(ACCENT),
        // A thin plane hovering, to check single-sided quads.
        Object::new(
            Shape::Plane {
                size: Vec2::new(2.0, 2.0),
            },
            Vec3::new(4.0, 0.01, 2.0),
        )
        .colored(ACCENT),
    ];

    // Stairs: 20 cm steps, 30 cm deep.
    for i in 0..8 {
        let height = 0.2 * (i + 1) as f32;
        objects.push(block(
            Vec3::new(2.0, height, 0.3),
            12.5,
            -1.0 - 0.3 * i as f32,
        ));
    }

    // A colonnade of hexagonal pillars receding into the fog.
    for i in 0..10 {
        let z = -6.0 - 6.0 * i as f32;
        for x in [-14.0, 14.0] {
            objects.push(
                Object::new(
                    Shape::Cylinder {
                        radius: 0.5,
                        height: 6.0,
                        segments: 6,
                    },
                    Vec3::new(x, 3.0, z),
                )
                .colored(STONE),
            );
        }
    }

    Scene {
        objects,
        ..Default::default()
    }
}
