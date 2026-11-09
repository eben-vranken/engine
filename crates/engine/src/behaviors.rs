//! Behaviors: components that act while the game runs (in play mode, or
//! after Play in the editor). They change the scene; the editor's Stop
//! puts its snapshot back.

use crate::scene::{Component, Door, Floating, Scene, Switch, Transform};
use crate::zones::Active;
use std::collections::HashMap;
use std::f32::consts::TAU;

/// What the player did this frame that behaviors react to.
#[derive(Debug, Default, Clone, Copy)]
pub struct Interaction {
    /// The switch the player pressed E at.
    pub used: Option<usize>,
    /// The object under the player's feet.
    pub standing_on: Option<usize>,
}

pub struct Behaviors {
    /// Every object that moves.
    moving: Vec<Moving>,
    /// Per door: whether it's open (or opening), and how far, 0–1.
    doors: HashMap<usize, DoorState>,
    /// The pressure plate the player is standing on.
    plate: Option<usize>,
}

struct Moving {
    index: usize,
    /// Its transform when the game started.
    rest: Transform,
    /// Seconds it has run: its clock stops while it's outside the
    /// simulation radius, so it picks up where it left off.
    time: f32,
}

struct DoorState {
    open: bool,
    openness: f32,
}

impl Behaviors {
    pub fn start(scene: &Scene) -> Self {
        let mut moving = Vec::new();
        let mut doors = HashMap::new();
        for (index, object) in scene.objects.iter().enumerate() {
            let mut moves = false;
            for component in &object.components {
                match component {
                    Component::Floating(_) => moves = true,
                    Component::Door(door) => {
                        moves = true;
                        let open = door.starts_open;
                        let openness = if open { 1.0 } else { 0.0 };
                        doors.insert(index, DoorState { open, openness });
                    }
                    _ => {}
                }
            }
            if moves {
                moving.push(Moving {
                    index,
                    rest: object.transform,
                    time: 0.0,
                });
            }
        }
        Self {
            moving,
            doors,
            plate: None,
        }
    }

    /// Advances the active objects' behaviors by `dt` seconds; returns the
    /// objects that moved. Switches can still flip doors that wait outside
    /// the radius; those move once they're active.
    pub fn update(
        &mut self,
        scene: &mut Scene,
        dt: f32,
        interaction: &Interaction,
        active: &Active,
    ) -> Vec<usize> {
        self.flip_switches(scene, interaction);

        let mut moved = Vec::new();
        for moving in &mut self.moving {
            let (index, rest) = (moving.index, moving.rest);
            if !active.object(index) {
                continue;
            }
            moving.time += dt;
            let object = &mut scene.objects[index];
            let mut transform = rest;
            for component in &object.components {
                match component {
                    Component::Floating(floating) => {
                        transform.position.y += bob(floating, moving.time);
                    }
                    Component::Door(door) => {
                        if let Some(state) = self.doors.get_mut(&index) {
                            let step = dt / door.duration.max(0.01);
                            state.openness = if state.open {
                                (state.openness + step).min(1.0)
                            } else {
                                (state.openness - step).max(0.0)
                            };
                            open_door(&mut transform, &rest, door, smoothstep(state.openness));
                        }
                    }
                    _ => {}
                }
            }
            if transform != object.transform {
                object.transform = transform;
                moved.push(index);
            }
        }
        moved
    }

    /// Toggles the doors that switches target this frame.
    fn flip_switches(&mut self, scene: &Scene, interaction: &Interaction) {
        let mut flipped = Vec::new();
        if let Some(switch) = interaction.used.and_then(|i| switch(scene, i))
            && !switch.pressure_plate
        {
            flipped.push(switch);
        }
        // A plate flips its target when stepped on, and again when left.
        let plate = interaction
            .standing_on
            .filter(|&i| switch(scene, i).is_some_and(|s| s.pressure_plate));
        if plate != self.plate {
            flipped.extend(
                [self.plate, plate]
                    .into_iter()
                    .flatten()
                    .filter_map(|i| switch(scene, i)),
            );
            self.plate = plate;
        }
        for switch in flipped {
            let target = scene.objects.iter().position(|o| o.name == switch.target.0);
            if let Some(door) = target.and_then(|i| self.doors.get_mut(&i)) {
                door.open = !door.open;
            }
        }
    }
}

impl Behaviors {
    /// Each door's index and whether it is open (or opening).
    pub fn doors(&self) -> impl Iterator<Item = (usize, bool)> + '_ {
        self.doors.iter().map(|(&i, d)| (i, d.open))
    }

    /// Opens (`Some(true)`), closes or toggles (`None`) a door; it moves
    /// there over its duration.
    pub fn set_door(&mut self, index: usize, open: Option<bool>) {
        if let Some(door) = self.doors.get_mut(&index) {
            door.open = open.unwrap_or(!door.open);
        }
    }

    /// Puts a door open or closed at once (a loaded game).
    pub fn snap_door(&mut self, index: usize, open: bool) {
        if let Some(door) = self.doors.get_mut(&index) {
            door.open = open;
            door.openness = if open { 1.0 } else { 0.0 };
        }
    }
}

/// True if `index` is a switch the player can press (not a pressure plate).
pub fn is_usable_switch(scene: &Scene, index: usize) -> bool {
    switch(scene, index).is_some_and(|s| !s.pressure_plate)
}

fn switch(scene: &Scene, index: usize) -> Option<&Switch> {
    scene
        .objects
        .get(index)?
        .components
        .iter()
        .find_map(|c| match c {
            Component::Switch(switch) => Some(switch),
            _ => None,
        })
}

/// Height above the rest position at `time`: a sine wave.
fn bob(floating: &Floating, time: f32) -> f32 {
    floating.height * (TAU * (floating.speed * time + floating.phase)).sin()
}

/// Moves a door `amount` (0 closed, 1 open) of the way open: turned around
/// its hinge, which stays put, and slid along its own axes.
fn open_door(transform: &mut Transform, rest: &Transform, door: &Door, amount: f32) {
    let hinge = transform.position + rest.quat() * door.hinge;
    transform.rotation.y += door.swing * amount;
    transform.position =
        hinge - transform.quat() * door.hinge + rest.quat() * (door.slide * amount);
}

/// Eases in and out, so doors don't start and stop abruptly.
fn smoothstep(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::{ObjectRef, test_scene};
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
        let nothing = Interaction::default();

        // A quarter cycle: at the top.
        assert_eq!(
            behaviors.update(&mut scene, 0.25, &nothing, &Active::default()),
            vec![0]
        );
        assert!((scene.objects[0].transform.position.y - 2.5).abs() < 1e-5);
        // Three quarters: at the bottom. The other box never moves.
        behaviors.update(&mut scene, 0.5, &nothing, &Active::default());
        assert!((scene.objects[0].transform.position.y - 1.5).abs() < 1e-5);
        assert_eq!(
            scene.objects[1].transform.position,
            Vec3::new(5.0, 0.0, 0.0)
        );
    }

    /// A door (0), a switch (1) and a pressure plate (2) both targeting it.
    fn door_scene() -> Scene {
        let mut scene = test_scene(vec![
            (
                Shape::Box {
                    size: Vec3::new(2.0, 2.0, 0.1),
                },
                Vec3::new(0.0, 1.0, 0.0),
            ),
            (
                Shape::Box {
                    size: Vec3::splat(0.3),
                },
                Vec3::new(2.0, 1.0, 0.0),
            ),
            (
                Shape::Box {
                    size: Vec3::new(1.0, 0.1, 1.0),
                },
                Vec3::new(0.0, 0.0, 3.0),
            ),
        ]);
        scene.objects[0].name = "door".into();
        scene.objects[0].components.push(Component::Door(Door {
            swing: 90.0,
            hinge: Vec3::new(-1.0, 0.0, 0.0),
            ..Default::default()
        }));
        for (i, pressure_plate) in [(1, false), (2, true)] {
            scene.objects[i].components.push(Component::Switch(Switch {
                target: ObjectRef("door".into()),
                pressure_plate,
            }));
        }
        scene
    }

    #[test]
    fn a_switch_swings_its_door_open_around_the_hinge() {
        let mut scene = door_scene();
        let mut behaviors = Behaviors::start(&scene);
        assert!(is_usable_switch(&scene, 1));
        assert!(
            !is_usable_switch(&scene, 2),
            "plates are stood on, not pressed"
        );

        let press = Interaction {
            used: Some(1),
            standing_on: None,
        };
        behaviors.update(&mut scene, 0.5, &press, &Active::default());
        // Halfway through its one-second swing; the hinge edge hasn't moved.
        let door = scene.objects[0].transform;
        assert!((door.rotation.y - 45.0).abs() < 1e-3);
        let hinge = door.position + door.quat() * Vec3::new(-1.0, 0.0, 0.0);
        assert!((hinge - Vec3::new(-1.0, 1.0, 0.0)).length() < 1e-5);

        behaviors.update(&mut scene, 1.0, &Interaction::default(), &Active::default());
        assert!((scene.objects[0].transform.rotation.y - 90.0).abs() < 1e-3);
        // Pressing again closes it.
        behaviors.update(&mut scene, 1.0, &press, &Active::default());
        assert!(scene.objects[0].transform.rotation.y.abs() < 1e-3);
    }

    #[test]
    fn a_pressure_plate_holds_its_door_open_while_stood_on() {
        let mut scene = door_scene();
        let mut behaviors = Behaviors::start(&scene);
        let on_plate = Interaction {
            used: None,
            standing_on: Some(2),
        };
        behaviors.update(&mut scene, 2.0, &on_plate, &Active::default());
        behaviors.update(&mut scene, 2.0, &on_plate, &Active::default());
        assert!((scene.objects[0].transform.rotation.y - 90.0).abs() < 1e-3);
        behaviors.update(&mut scene, 2.0, &Interaction::default(), &Active::default());
        assert!(scene.objects[0].transform.rotation.y.abs() < 1e-3);
    }

    #[test]
    fn a_missing_target_does_nothing() {
        let mut scene = door_scene();
        scene.objects[0].name = "renamed".into();
        let mut behaviors = Behaviors::start(&scene);
        let press = Interaction {
            used: Some(1),
            standing_on: None,
        };
        assert!(
            behaviors
                .update(&mut scene, 1.0, &press, &Active::default())
                .is_empty()
        );
    }
}
