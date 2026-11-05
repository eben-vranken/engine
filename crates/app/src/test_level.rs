//! A movement test level built from primitives in code. Replaced by RON
//! scenes in the game project once the scene format exists (milestone 3).
//!
//! The player spawns at the origin looking down -Z. Ahead: ramps and stairs.
//! Left: gap jumps and platforms of increasing height. Right: narrow beam,
//! low tunnel, tall tower. Behind: walls and corners. Colors run green →
//! yellow → orange → red from easiest to hardest.

use engine::glam::{Quat, Vec3};
use engine::player::Spawn;
use engine::render::mesh::Shape;
use engine::scene::{Object, Scene};

const FLOOR: Vec3 = Vec3::new(0.6, 0.6, 0.6);
const GREY: Vec3 = Vec3::new(0.5, 0.5, 0.52);
const STONE: Vec3 = Vec3::new(0.55, 0.52, 0.48);

/// Difficulty color for item `i` of `n`.
fn rank(i: usize, n: usize) -> Vec3 {
    let stops = [
        Vec3::new(0.25, 0.65, 0.3),
        Vec3::new(0.85, 0.75, 0.2),
        Vec3::new(0.9, 0.45, 0.12),
        Vec3::new(0.85, 0.18, 0.14),
    ];
    let t = if n <= 1 {
        0.0
    } else {
        i as f32 / (n - 1) as f32 * (stops.len() - 1) as f32
    };
    let lower = (t.floor() as usize).min(stops.len() - 2);
    stops[lower].lerp(stops[lower + 1], t - lower as f32)
}

/// Box from its center.
fn cuboid(size: Vec3, center: Vec3, color: Vec3) -> Object {
    Object::new(Shape::Box { size }, center).colored(color)
}

/// Box standing on the floor, centered at (x, z).
fn block(size: Vec3, x: f32, z: f32, color: Vec3) -> Object {
    cuboid(size, Vec3::new(x, size.y * 0.5, z), color)
}

pub fn spawn() -> Spawn {
    Spawn {
        position: Vec3::new(0.0, 0.0, 4.0),
        yaw: 0.0,
    }
}

pub fn scene() -> Scene {
    let mut objects = vec![cuboid(
        Vec3::new(160.0, 0.5, 160.0),
        Vec3::new(0.0, -0.25, -15.0),
        FLOOR,
    )];
    ramps(&mut objects);
    stairs(&mut objects);
    gap_jumps(&mut objects);
    platform_heights(&mut objects);
    beam(&mut objects);
    tunnel(&mut objects);
    tower(&mut objects);
    walls(&mut objects);
    Scene {
        objects,
        ..Default::default()
    }
}

/// 15°, 30°, 45°, 60° ramps up to 2 m platforms.
fn ramps(objects: &mut Vec<Object>) {
    let angles = [15.0f32, 30.0, 45.0, 60.0];
    let height = 2.0;
    for (i, angle) in angles.into_iter().enumerate() {
        let x = -9.0 + i as f32 * 4.0;
        let depth = height / angle.to_radians().tan();
        objects.push(
            Object::new(
                Shape::Wedge {
                    size: Vec3::new(3.0, height, depth),
                },
                Vec3::new(x, height * 0.5, -10.0 - depth * 0.5),
            )
            .colored(rank(i, angles.len())),
        );
        objects.push(block(
            Vec3::new(3.0, height, 3.0),
            x,
            -10.0 - depth - 1.5,
            GREY,
        ));
    }
}

/// Stairs with 10, 20, 30 and 40 cm steps, all rising 1.2 m.
fn stairs(objects: &mut Vec<Object>) {
    let step_heights = [0.1f32, 0.2, 0.3, 0.4];
    let rise = 1.2;
    let depth = 0.3;
    for (i, step) in step_heights.into_iter().enumerate() {
        let x = 8.0 + i as f32 * 3.5;
        let count = (rise / step).round() as usize;
        for k in 0..count {
            objects.push(block(
                Vec3::new(3.0, step * (k + 1) as f32, depth),
                x,
                -10.0 - depth * (k as f32 + 0.5),
                rank(i, step_heights.len()),
            ));
        }
        objects.push(block(
            Vec3::new(3.0, rise, 3.0),
            x,
            -10.0 - depth * count as f32 - 1.5,
            GREY,
        ));
    }
}

/// 1 m high platforms separated by gaps from 1 m to 5 m, along -Z.
fn gap_jumps(objects: &mut Vec<Object>) {
    let gaps = [1.0f32, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0];
    let (x, height) = (-20.0, 1.0);
    objects.push(block(Vec3::new(2.0, height, 4.0), x, 0.0, GREY));
    let mut z = -2.0; // far edge of the current platform
    for (i, gap) in gaps.into_iter().enumerate() {
        z -= gap;
        let color = rank(i, gaps.len());
        objects.push(block(Vec3::new(2.0, height, 2.0), x, z - 1.0, color));
        // A floor marker spanning the gap, same color, to read its width.
        objects.push(cuboid(
            Vec3::new(2.0, 0.02, gap),
            Vec3::new(x, 0.01, z + gap * 0.5),
            color * 0.6,
        ));
        z -= 2.0;
    }
    // Stairs up to the start platform.
    for k in 0..4 {
        objects.push(block(
            Vec3::new(2.0, 0.25 * (k + 1) as f32, 0.4),
            x,
            3.4 - 0.4 * k as f32,
            GREY,
        ));
    }
}

/// Free-standing platforms from 0.5 m to 2.5 m, to jump onto from the floor.
fn platform_heights(objects: &mut Vec<Object>) {
    let heights = [0.5f32, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5];
    for (i, height) in heights.into_iter().enumerate() {
        objects.push(block(
            Vec3::new(2.0, height, 2.0),
            -27.0,
            -(i as f32) * 4.5,
            rank(i, heights.len()),
        ));
    }
}

/// A 20 cm wide, 12 m long beam at 1 m height.
fn beam(objects: &mut Vec<Object>) {
    let (x, height) = (24.0, 1.0);
    objects.push(block(Vec3::new(3.0, height, 3.0), x, 0.0, GREY));
    objects.push(cuboid(
        Vec3::new(0.2, 0.2, 12.0),
        Vec3::new(x, height - 0.1, -7.5),
        rank(3, 4),
    ));
    objects.push(block(Vec3::new(3.0, height, 3.0), x, -15.0, GREY));
    for k in 0..4 {
        objects.push(block(
            Vec3::new(3.0, 0.25 * (k + 1) as f32, 0.4),
            x,
            2.9 - 0.4 * k as f32,
            GREY,
        ));
    }
}

/// A tunnel: 2.2 m clearance, then 1.3 m (crouch only).
fn tunnel(objects: &mut Vec<Object>) {
    let (x, width, wall) = (32.0, 2.5, 0.5);
    let sections = [(2.2f32, 0.0f32, 6.0f32), (1.3, -6.0, 8.0)];
    for (i, (clearance, start, length)) in sections.into_iter().enumerate() {
        let z = start - length * 0.5;
        for side in [-1.0, 1.0] {
            objects.push(block(
                Vec3::new(wall, 3.0, length),
                x + side * (width + wall) * 0.5,
                z,
                STONE,
            ));
        }
        objects.push(cuboid(
            Vec3::new(width + 2.0 * wall, 0.5, length),
            Vec3::new(x, clearance + 0.25, z),
            rank(i * 3, 4),
        ));
    }
}

/// A 15 m tower with a spiral of 30 cm steps around it.
fn tower(objects: &mut Vec<Object>) {
    let (cx, cz, height, rise) = (20.0, -32.0, 15.0, 0.3);
    objects.push(block(Vec3::new(4.0, height, 4.0), cx, cz, STONE));

    // The ring of 1 m cells around the 4 × 4 tower, walked counter-clockwise.
    let mut ring = Vec::new();
    ring.extend((0..6).map(|i| (-2.5 + i as f32, 2.5)));
    ring.extend((1..6).map(|i| (2.5, 2.5 - i as f32)));
    ring.extend((1..6).map(|i| (2.5 - i as f32, -2.5)));
    ring.extend((1..5).map(|i| (-2.5, -2.5 + i as f32)));

    let steps = (height / rise).round() as usize;
    for i in 0..steps {
        let (dx, dz) = ring[i % ring.len()];
        let top = rise * (i + 1) as f32;
        objects.push(cuboid(
            Vec3::new(1.0, rise, 1.0),
            Vec3::new(cx + dx, top - rise * 0.5, cz + dz),
            rank(i * 4 / steps, 4),
        ));
    }
}

/// Walls and corners behind the spawn: a long wall, a 90° corner, a 45°
/// corner, an outside corner and a round pillar.
fn walls(objects: &mut Vec<Object>) {
    let height = 3.0;
    objects.push(block(Vec3::new(24.0, height, 0.5), 0.0, 16.0, GREY));
    // 90° inside corner on the left end.
    objects.push(block(Vec3::new(0.5, height, 8.0), -11.75, 11.75, GREY));
    // 45° corner on the right end: a wall angled back into the long wall.
    let length = 8.0;
    let angle = 45f32.to_radians();
    let end = Vec3::new(11.75, 0.0, 15.75);
    let direction = Vec3::new(-angle.sin(), 0.0, -angle.cos());
    objects.push(
        cuboid(
            Vec3::new(0.5, height, length),
            end + direction * (length * 0.5) + Vec3::Y * (height * 0.5),
            GREY,
        )
        .rotated(Quat::from_rotation_y(angle)),
    );
    // Outside corner and a round pillar in the open.
    objects.push(block(Vec3::new(3.0, height, 3.0), -5.0, 10.0, STONE));
    objects.push(
        Object::new(
            Shape::Cylinder {
                radius: 0.6,
                height,
                segments: 8,
            },
            Vec3::new(4.0, height * 0.5, 10.0),
        )
        .colored(STONE),
    );
}
