//! The editor: the running game with panels on top. The scene hierarchy is
//! on the left (grouped by zone), the inspector on the right, and the 3D
//! viewport takes whatever space is left in the middle.

use crate::render::mesh::Shape;
use crate::scene::{Component, Object, Scene};
use crate::ui::theme;
use egui::{Align, Layout, Rect, RichText, Ui};
use glam::Vec3;
use std::path::PathBuf;

pub struct Editor {
    /// Index into `Scene::objects`.
    pub selected: Option<usize>,
    /// The scene file being edited, for display.
    pub scene_path: PathBuf,
    pub game_name: String,
}

/// Numbers for the top bar.
pub struct EditorStats {
    pub fps: f32,
    pub frame_ms: f32,
    pub internal: [u32; 2],
    pub factor: u32,
}

impl Editor {
    pub fn new(game_name: String, scene_path: PathBuf) -> Self {
        Self {
            selected: None,
            scene_path,
            game_name,
        }
    }

    /// Draws the panels and returns the rect left for the 3D viewport, in points.
    pub fn ui(&mut self, ui: &mut Ui, scene: &Scene, stats: &EditorStats, blink: bool) -> Rect {
        egui::Panel::top("editor_top").show(ui, |ui| self.top_bar(ui, stats, blink));
        egui::Panel::left("editor_scene")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| self.hierarchy(ui, scene));
        egui::Panel::right("editor_inspector")
            .resizable(true)
            .default_size(320.0)
            .show(ui, |ui| self.inspector(ui, scene));

        let viewport = ui.available_rect_before_wrap();
        hints(ui, viewport);
        viewport
    }

    fn top_bar(&self, ui: &mut Ui, stats: &EditorStats, blink: bool) {
        ui.horizontal(|ui| {
            let cursor = if blink { "_" } else { " " };
            ui.label(RichText::new(format!("ENGINE // EDITOR{cursor}")).strong());
            ui.add_space(12.0);
            ui.label(self.game_name.to_uppercase());
            ui.label(dim(&self.scene_path.display().to_string().to_uppercase()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(format!(
                    "{}×{} ×{}",
                    stats.internal[0], stats.internal[1], stats.factor
                ));
                ui.label(dim("INTERNAL"));
                ui.add_space(12.0);
                ui.label(format!("{:.0}  {:.2} MS", stats.fps, stats.frame_ms));
                ui.label(dim("FPS"));
            });
        });
    }

    fn hierarchy(&mut self, ui: &mut Ui, scene: &Scene) {
        ui.add_space(4.0);
        ui.label(RichText::new("SCENE").strong());
        ui.label(dim(&format!(
            "{} OBJECTS IN {} ZONES",
            scene.objects.len(),
            scene.zones.len()
        )));
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for zone in &scene.zones {
                    let members: Vec<(usize, &Object)> = scene
                        .objects
                        .iter()
                        .enumerate()
                        .filter(|(_, o)| o.zone == zone.name)
                        .collect();
                    let header = format!("{} [{}]", zone.name.to_uppercase(), members.len());
                    egui::CollapsingHeader::new(header)
                        .id_salt(&zone.name)
                        .show(ui, |ui| {
                            for (index, object) in members {
                                let selected = self.selected == Some(index);
                                if ui.selectable_label(selected, &object.name).clicked() {
                                    self.selected = Some(index);
                                }
                            }
                        });
                }
            });
    }

    fn inspector(&self, ui: &mut Ui, scene: &Scene) {
        ui.add_space(4.0);
        ui.label(RichText::new("INSPECTOR").strong());
        ui.add_space(4.0);
        let Some(object) = self.selected.and_then(|i| scene.objects.get(i)) else {
            ui.label(dim("NOTHING SELECTED"));
            return;
        };

        egui::Grid::new("inspector_object")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label(dim("NAME"));
                ui.label(&object.name);
                ui.end_row();
                ui.label(dim("ZONE"));
                ui.label(object.zone.to_uppercase());
                ui.end_row();
            });

        section(ui, "TRANSFORM");
        let t = &object.transform;
        egui::Grid::new("inspector_transform")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for (label, v) in [
                    ("POSITION", t.position),
                    ("ROTATION", t.rotation),
                    ("SCALE", t.scale),
                ] {
                    ui.label(dim(label));
                    ui.label(vec3(v));
                    ui.end_row();
                }
            });

        for component in &object.components {
            match component {
                Component::Primitive(primitive) => {
                    section(ui, "PRIMITIVE");
                    egui::Grid::new("inspector_primitive")
                        .num_columns(2)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            let (kind, rows) = shape_rows(primitive.shape);
                            ui.label(dim("SHAPE"));
                            ui.label(kind);
                            ui.end_row();
                            for (label, value) in rows {
                                ui.label(dim(label));
                                ui.label(value);
                                ui.end_row();
                            }
                            ui.label(dim("COLOR"));
                            ui.label(vec3(primitive.color));
                            ui.end_row();
                        });
                }
                Component::PlayerStart => section(ui, "PLAYER START"),
            }
        }
    }
}

fn vec3(v: Vec3) -> String {
    format!("{:>7.2} {:>7.2} {:>7.2}", v.x, v.y, v.z)
}

/// A shape's kind and its parameters as inspector rows.
fn shape_rows(shape: Shape) -> (&'static str, Vec<(&'static str, String)>) {
    match shape {
        Shape::Box { size } => ("BOX", vec![("SIZE", vec3(size))]),
        Shape::Plane { size } => (
            "PLANE",
            vec![("SIZE", format!("{:>7.2} {:>7.2}", size.x, size.y))],
        ),
        Shape::Wedge { size } => ("WEDGE", vec![("SIZE", vec3(size))]),
        Shape::Cylinder {
            radius,
            height,
            segments,
        } => (
            "CYLINDER",
            vec![
                ("RADIUS", format!("{radius:>7.2}")),
                ("HEIGHT", format!("{height:>7.2}")),
                ("SEGMENTS", format!("{segments:>7}")),
            ],
        ),
        Shape::Sphere {
            radius,
            segments,
            rings,
        } => (
            "SPHERE",
            vec![
                ("RADIUS", format!("{radius:>7.2}")),
                ("SEGMENTS", format!("{segments:>7}")),
                ("RINGS", format!("{rings:>7}")),
            ],
        ),
    }
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong());
    ui.separator();
}

fn dim(text: &str) -> RichText {
    RichText::new(text).color(theme::AMBER_DIM)
}

/// Controls, in the viewport's bottom-left corner.
fn hints(ui: &Ui, viewport: Rect) {
    egui::Area::new(egui::Id::unique("editor_hints"))
        .fixed_pos(viewport.left_bottom() + egui::vec2(10.0, -26.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.label(
                RichText::new("RMB + WASD/QE FLY  WHEEL SPEED  ESC DESELECT")
                    .small()
                    .color(theme::AMBER_DIM),
            );
        });
}
