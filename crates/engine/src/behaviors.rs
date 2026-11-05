//! Behaviors: components that act while the game runs (in play mode, or
//! after Play in the editor). They change the scene; the editor's Stop
//! puts its snapshot back.

use crate::scene::{Component, Floating, Scene, Transform};
use std::f32::consts::TAU;

pub struct Behaviors {
    /// Seconds since the game started.
    time: f32,
    /// Each moving object's transform when the game started.
    rest: Vec<(usize, Transform)>,
}

impl Behaviors {
    pub fn start(scene: &Scene) -> Self {
        let rest = scene
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                o.components
                    .iter()
                    .any(|c| matches!(c, Component::Floating(_)))
            })
            .map(|(index, o)| (index, o.transform))
            .collect();
        Self { time: 0.0, rest }
    }

    /// Advances every behavior by `dt` seconds; returns the objects that moved.
    pub fn update(&mut self, scene: &mut Scene, dt: f32) -> Vec<usize> {
        self.time += dt;
        let mut moved = Vec::new();
        for &(index, rest) in &self.rest {
            let object = &mut scene.objects[index];
            let mut transform = rest;
            for component in &object.components {
                if let Component::Floating(floating) = component {
                    transform.position.y += bob(floating, self.time);
                }
            }
            if transform != object.transform {
                object.transform = transform;
                moved.push(index);
            }
        }
        moved
    }
}

/// Height above the rest position at `time`: a sine wave.
fn bob(floating: &Floating, time: f32) -> f32 {
    floating.height * (TAU * (floating.speed * time + floating.phase)).sin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::test_scene;
    use glam::Vec3;

    #[test]
    fn floating_objects_bob_around_where_they_were() {
        let mut scene = test_scene(vec![
            (Shape::Box { size: Vec3::ONE }, Vec3::new(0.0, 2.0, 0.0)),
            (Shape::Box { size: Vec3::ONE }, Vec3::new(5.0, 0.0, 0.0)),
        ]);
        scene.objects[0]
            .components
            .push(Component::Floating(Floating {
                height: 0.5,
                speed: 1.0,
                phase: 0.0,
            }));
        let mut behaviors = Behaviors::start(&scene);

        // A quarter cycle: at the top.
        assert_eq!(behaviors.update(&mut scene, 0.25), vec![0]);
        assert!((scene.objects[0].transform.position.y - 2.5).abs() < 1e-5);
        // Three quarters: at the bottom. The other box never moves.
        behaviors.update(&mut scene, 0.5);
        assert!((scene.objects[0].transform.position.y - 1.5).abs() < 1e-5);
        assert_eq!(
            scene.objects[1].transform.position,
            Vec3::new(5.0, 0.0, 0.0)
        );
    }
}
