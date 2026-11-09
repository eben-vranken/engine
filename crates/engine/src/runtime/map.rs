//! The map (Tab while playing): the zones visited in this scene, drawn
//! from above as the outlines of their zone volumes, with the doorways
//! between them and where the player stands. Zones not visited yet stay
//! hidden.

use crate::scene::{Component, Scene};
use crate::ui::theme;
use egui::{Pos2, Rect, Stroke, vec2};
use glam::{Vec2, Vec3};
use std::collections::BTreeSet;

/// What the map shows, in world meters seen from above (x, z).
pub(super) struct MapData {
    zones: Vec<MapZone>,
    doorways: Vec<Vec2>,
    player: Vec2,
    /// Radians; 0 looks toward -z (up on the map).
    yaw: f32,
}

struct MapZone {
    name: String,
    /// One outline per volume.
    outlines: Vec<[Vec2; 4]>,
    current: bool,
}

pub(super) fn map_data(
    scene: &Scene,
    visited: Option<&BTreeSet<String>>,
    current: Option<&str>,
    player: Vec3,
    yaw: f32,
) -> MapData {
    let seen = |zone: &str| visited.is_some_and(|v| v.contains(zone)) || current == Some(zone);
    let mut zones: Vec<MapZone> = Vec::new();
    let mut doorways = Vec::new();
    for object in &scene.objects {
        for component in &object.components {
            match component {
                Component::ZoneVolume(volume) if seen(&object.zone) => {
                    let m = object.transform.matrix();
                    let h = volume.size * 0.5;
                    let corner = |x: f32, z: f32| {
                        let p = m.transform_point3(Vec3::new(x * h.x, 0.0, z * h.z));
                        Vec2::new(p.x, p.z)
                    };
                    let outline = [
                        corner(-1.0, -1.0),
                        corner(1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, 1.0),
                    ];
                    match zones.iter_mut().find(|z| z.name == object.zone) {
                        Some(zone) => zone.outlines.push(outline),
                        None => zones.push(MapZone {
                            name: object.zone.clone(),
                            outlines: vec![outline],
                            current: current == Some(object.zone.as_str()),
                        }),
                    }
                }
                Component::Portal(portal) if seen(&object.zone) && seen(&portal.to.0) => {
                    let p = object.transform.position;
                    doorways.push(Vec2::new(p.x, p.z));
                }
                _ => {}
            }
        }
    }
    MapData {
        zones,
        doorways,
        player: Vec2::new(player.x, player.z),
        yaw,
    }
}

/// The map over the whole game view (M while playing).
pub(super) fn draw_map(ui: &egui::Ui, image: Rect, data: &MapData) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("map"),
    ));
    painter.rect_filled(image, 0.0, theme::BACKGROUND.gamma_multiply(0.9));
    painter.text(
        image.left_top() + vec2(16.0, 14.0),
        egui::Align2::LEFT_TOP,
        "MAP",
        egui::FontId::monospace(14.0),
        theme::AMBER,
    );
    painter.text(
        image.right_top() + vec2(-16.0, 14.0),
        egui::Align2::RIGHT_TOP,
        "M CLOSE",
        egui::FontId::monospace(11.0),
        theme::AMBER_DIM,
    );
    draw_map_in(&painter, image.shrink(image.height() * 0.1), data);
}

/// The map's zones, doorways and player, fitted into `area`.
pub(super) fn draw_map_in(painter: &egui::Painter, area: Rect, data: &MapData) {
    // Fit everything shown, north (-z) up.
    let points = data
        .zones
        .iter()
        .flat_map(|z| z.outlines.iter().flatten().copied())
        .chain([data.player]);
    let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in points {
        min = min.min(p);
        max = max.max(p);
    }
    let size = (max - min).max(Vec2::splat(10.0));
    let scale = (area.width() / size.x).min(area.height() / size.y);
    let middle = (min + max) * 0.5;
    let to_screen = |p: Vec2| -> Pos2 {
        let d = (p - middle) * scale;
        area.center() + vec2(d.x, d.y)
    };

    if data.zones.is_empty() {
        painter.text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "NOTHING MAPPED YET",
            egui::FontId::monospace(12.0),
            theme::AMBER_DIM,
        );
    }
    for zone in &data.zones {
        let color = if zone.current {
            theme::AMBER
        } else {
            theme::AMBER_DIM
        };
        for outline in &zone.outlines {
            let corners: Vec<Pos2> = outline.iter().map(|&p| to_screen(p)).collect();
            let fill = if zone.current {
                theme::AMBER.gamma_multiply(0.08)
            } else {
                egui::Color32::TRANSPARENT
            };
            painter.add(egui::Shape::convex_polygon(
                corners,
                fill,
                Stroke::new(1.0, color),
            ));
        }
        let center = zone.outlines[0].iter().copied().sum::<Vec2>() / 4.0;
        painter.text(
            to_screen(center),
            egui::Align2::CENTER_CENTER,
            zone.name.to_uppercase(),
            egui::FontId::monospace(11.0),
            color,
        );
    }
    for &doorway in &data.doorways {
        let at = to_screen(doorway);
        painter.rect_filled(
            Rect::from_center_size(at, vec2(5.0, 5.0)),
            0.0,
            theme::AMBER_DIM,
        );
    }
    // The player: a small arrow pointing the way they face.
    let at = to_screen(data.player);
    let forward = vec2(data.yaw.sin(), -data.yaw.cos());
    let side = vec2(-forward.y, forward.x);
    let tip = at + forward * 9.0;
    let left = at - forward * 5.0 + side * 5.0;
    let right = at - forward * 5.0 - side * 5.0;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, right, left],
        theme::AMBER,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Object, Portal, Transform, Zone, ZoneRef, ZoneVolume};

    fn volume(zone: &str, x: f32) -> Object {
        Object {
            id: 0,
            name: format!("{zone} volume"),
            zone: zone.into(),
            transform: Transform::at(Vec3::new(x, 0.0, 0.0)),
            components: vec![Component::ZoneVolume(ZoneVolume::default())],
            hidden: false,
        }
    }

    #[test]
    fn only_visited_zones_are_mapped() {
        let mut door = volume("a", 2.0);
        door.components = vec![Component::Portal(Portal {
            to: ZoneRef("b".into()),
        })];
        let scene = Scene {
            zones: vec![Zone::new("a"), Zone::new("b")],
            objects: vec![volume("a", 0.0), volume("b", 4.0), door],
            ..Default::default()
        };
        let mut visited = BTreeSet::new();
        visited.insert("a".to_string());
        let data = map_data(&scene, Some(&visited), Some("a"), Vec3::ZERO, 0.0);
        assert_eq!(data.zones.len(), 1);
        assert!(data.doorways.is_empty(), "b isn't known yet");
        visited.insert("b".to_string());
        let data = map_data(&scene, Some(&visited), None, Vec3::ZERO, 0.0);
        assert_eq!((data.zones.len(), data.doorways.len()), (2, 1));
    }
}
