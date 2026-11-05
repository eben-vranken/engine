//! Zones in space: which zone a point is in (from `ZoneVolume` boxes) and
//! which zones are neighbours (from `Portal`s). Rebuilt from the scene when
//! needed; it's a pass over the objects, cheap at our object counts.

use crate::scene::{Component, Scene};
use glam::{Mat4, Vec3};

pub struct ZoneMap {
    volumes: Vec<Volume>,
    /// Per zone (index into `Scene::zones`): the zones it has portals to or
    /// from.
    neighbours: Vec<Vec<usize>>,
}

/// One `ZoneVolume`, as a transform into a unit cube.
struct Volume {
    zone: usize,
    /// World → the volume's box, where -0.5..0.5 on each axis is inside.
    to_box: Mat4,
    /// Cubic meters, so the smallest of overlapping volumes wins.
    size: f32,
}

impl ZoneMap {
    pub fn new(scene: &Scene) -> Self {
        let zone_index = |name: &str| scene.zones.iter().position(|z| z.name == name);
        let mut volumes = Vec::new();
        let mut neighbours = vec![Vec::new(); scene.zones.len()];
        for object in &scene.objects {
            let Some(zone) = zone_index(&object.zone) else {
                continue;
            };
            for component in &object.components {
                match component {
                    Component::ZoneVolume(volume) => {
                        let size = volume.size * object.transform.scale;
                        if size.abs().min_element() <= 0.0 {
                            continue;
                        }
                        let to_world = object.transform.matrix() * Mat4::from_scale(volume.size);
                        volumes.push(Volume {
                            zone,
                            to_box: to_world.inverse(),
                            size: size.abs().element_product(),
                        });
                    }
                    Component::Portal(portal) => {
                        if let Some(to) = zone_index(&portal.to.0)
                            && to != zone
                        {
                            for (a, b) in [(zone, to), (to, zone)] {
                                if !neighbours[a].contains(&b) {
                                    neighbours[a].push(b);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Self {
            volumes,
            neighbours,
        }
    }

    /// The zone whose volume contains `point` (the smallest, if several
    /// do); `None` outside every volume.
    pub fn zone_at(&self, point: Vec3) -> Option<usize> {
        self.volumes
            .iter()
            .filter(|v| {
                let local = v.to_box.transform_point3(point);
                local.abs().max_element() <= 0.5
            })
            .min_by(|a, b| a.size.total_cmp(&b.size))
            .map(|v| v.zone)
    }

    /// Zones that share a portal with `zone`.
    pub fn neighbours(&self, zone: usize) -> &[usize] {
        self.neighbours.get(zone).map_or(&[], Vec::as_slice)
    }
}

/// Which zone someone (the player, the camera) is in. Outside every volume
/// it keeps the last zone, so a gap between volumes or a jump above one
/// doesn't drop it.
#[derive(Debug, Default, Clone)]
pub struct ZoneTracker {
    /// By name, so it survives zones being added or reordered.
    current: Option<String>,
}

impl ZoneTracker {
    /// Updates from the position and returns the current zone's index.
    pub fn update(&mut self, scene: &Scene, map: &ZoneMap, position: Vec3) -> Option<usize> {
        if let Some(zone) = map.zone_at(position) {
            self.current = Some(scene.zones[zone].name.clone());
        }
        let current = self.current.as_deref()?;
        scene.zones.iter().position(|z| z.name == current)
    }

    /// Forget the last zone (a new scene, a respawn).
    pub fn reset(&mut self) {
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Object, Portal, Transform, Zone, ZoneRef, ZoneVolume};

    fn zone(name: &str) -> Zone {
        Zone { name: name.into() }
    }

    fn volume(zone: &str, position: Vec3, size: Vec3) -> Object {
        Object {
            name: format!("{zone} volume"),
            zone: zone.into(),
            transform: Transform::at(position),
            components: vec![Component::ZoneVolume(ZoneVolume { size })],
        }
    }

    fn portal(from: &str, to: &str) -> Object {
        Object {
            name: format!("{from} to {to}"),
            zone: from.into(),
            transform: Transform::default(),
            components: vec![Component::Portal(Portal {
                to: ZoneRef(to.into()),
            })],
        }
    }

    /// A hall with a small room inside its volume, and a corridor beside it.
    fn scene() -> Scene {
        Scene {
            zones: vec![zone("hall"), zone("room"), zone("corridor")],
            objects: vec![
                volume("hall", Vec3::ZERO, Vec3::splat(20.0)),
                volume("room", Vec3::new(5.0, 0.0, 0.0), Vec3::splat(2.0)),
                volume(
                    "corridor",
                    Vec3::new(15.0, 0.0, 0.0),
                    Vec3::new(10.0, 3.0, 2.0),
                ),
                portal("hall", "corridor"),
                portal("room", "hall"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn the_smallest_containing_volume_wins() {
        let map = ZoneMap::new(&scene());
        assert_eq!(map.zone_at(Vec3::new(-5.0, 0.0, 0.0)), Some(0));
        assert_eq!(map.zone_at(Vec3::new(5.5, 0.5, 0.0)), Some(1));
        assert_eq!(map.zone_at(Vec3::new(15.0, 1.0, 0.0)), Some(2));
        assert_eq!(map.zone_at(Vec3::new(0.0, 50.0, 0.0)), None);
    }

    #[test]
    fn volumes_turn_and_scale_with_their_object() {
        let mut scene = scene();
        let corridor = &mut scene.objects[2].transform;
        corridor.position = Vec3::new(0.0, 0.0, 30.0);
        corridor.rotation.y = 90.0; // its long side now runs along Z
        corridor.scale = Vec3::new(2.0, 1.0, 1.0);
        let map = ZoneMap::new(&scene);
        assert_eq!(map.zone_at(Vec3::new(0.0, 0.0, 39.0)), Some(2));
        assert_eq!(map.zone_at(Vec3::new(3.0, 0.0, 30.0)), None);
    }

    #[test]
    fn portals_make_neighbours_both_ways() {
        let map = ZoneMap::new(&scene());
        assert_eq!(map.neighbours(0), &[2, 1]);
        assert_eq!(map.neighbours(1), &[0]);
        assert_eq!(map.neighbours(2), &[0]);
    }

    #[test]
    fn the_tracker_keeps_the_last_zone_outside_every_volume() {
        let scene = scene();
        let map = ZoneMap::new(&scene);
        let mut tracker = ZoneTracker::default();
        assert_eq!(tracker.update(&scene, &map, Vec3::splat(100.0)), None);
        assert_eq!(
            tracker.update(&scene, &map, Vec3::new(15.0, 0.0, 0.0)),
            Some(2)
        );
        assert_eq!(tracker.update(&scene, &map, Vec3::splat(100.0)), Some(2));
    }
}
