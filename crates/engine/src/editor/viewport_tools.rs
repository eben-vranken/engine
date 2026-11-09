//! Floating controls over the BUILD viewport: the gizmo tools and what
//! the view shows (top left), and the bakes' state with BAKE (top right).

use super::{Editor, EditorOutput, dim};
use crate::ui::theme;
use egui::{Align2, Rect, RichText, Ui, vec2};
use transform_gizmo::config::GizmoModeKind;

impl Editor {
    pub(super) fn viewport_tools(&mut self, ui: &Ui, viewport: Rect, output: &mut EditorOutput) {
        egui::Area::new(egui::Id::unique("viewport_tools"))
            .pivot(Align2::LEFT_TOP)
            .fixed_pos(viewport.left_top() + vec2(10.0, 10.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(theme::BACKGROUND)
                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                    .inner_margin(4)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (kind, text) in [
                                (GizmoModeKind::Translate, "MOVE W"),
                                (GizmoModeKind::Rotate, "ROTATE E"),
                                (GizmoModeKind::Scale, "SCALE R"),
                            ] {
                                let on = self.gizmo.kind == kind;
                                let label = if on {
                                    RichText::new(format!(" {text} "))
                                        .color(theme::BACKGROUND)
                                        .strong()
                                } else {
                                    RichText::new(format!(" {text} ")).color(theme::AMBER_DIM)
                                };
                                let mut button = egui::Button::new(label).small();
                                if on {
                                    button = button.fill(theme::AMBER);
                                }
                                if ui.add(button).clicked() {
                                    self.gizmo.kind = kind;
                                }
                            }
                            ui.label(dim("CTRL SNAPS"));
                            ui.menu_button("SHOW ▾", |ui| {
                                ui.set_min_width(200.0);
                                for (text, value) in [
                                    ("ZONE VOLUMES & PORTALS  H", &mut self.show_zones),
                                    ("TRIGGER BOXES", &mut self.show_triggers),
                                    ("REVERB PROBES", &mut self.show_probes),
                                    ("KEY HINTS", &mut self.show_hints),
                                ] {
                                    let mark = if *value { "[X]" } else { "[ ]" };
                                    if ui.button(format!("{mark} {text}")).clicked() {
                                        *value = !*value;
                                    }
                                }
                            });
                        });
                    });
            });

        egui::Area::new(egui::Id::unique("bake_chip"))
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(viewport.right_top() + vec2(-10.0, 10.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(theme::BACKGROUND)
                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                    .inner_margin(4)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (name, status) in
                                [("LIGHT", &self.light_status), ("SOUND", &self.bake_status)]
                            {
                                let text = if status.is_empty() {
                                    "NOT BAKED".to_owned()
                                } else {
                                    status.replace("BAKE OUTDATED", "OUTDATED")
                                };
                                let color = if text == "BAKED" {
                                    theme::AMBER_DIM
                                } else if text.starts_with("BAKING") {
                                    theme::AMBER
                                } else {
                                    theme::ALERT
                                };
                                ui.label(RichText::new(format!("{name} {text}")).color(color));
                            }
                            ui.menu_button("BAKE ▾", |ui| {
                                ui.set_min_width(260.0);
                                let lighting = !self.light_status.starts_with("BAKING");
                                if ui
                                    .add_enabled(lighting, egui::Button::new("LIGHTING"))
                                    .on_hover_text(
                                        "BOUNCE, SKY AND EMISSIVE LIGHT INTO LIGHTMAPS AND PROBES",
                                    )
                                    .clicked()
                                {
                                    output.bake_lighting = true;
                                    ui.close();
                                }
                                let acoustics = !self.bake_status.starts_with("BAKING");
                                if ui
                                    .add_enabled(acoustics, egui::Button::new("ACOUSTICS"))
                                    .on_hover_text("REVERB PROBES IN EVERY ZONE VOLUME")
                                    .clicked()
                                {
                                    output.bake = true;
                                    ui.close();
                                }
                            });
                        });
                    });
            });
    }
}
