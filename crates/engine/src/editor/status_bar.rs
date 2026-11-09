//! The status line along the bottom: the last message on the left; bake
//! notes and the frame stats on the right. And the key hints over the
//! viewport.

use super::{Editor, EditorOutput, EditorStats, dim};
use crate::ui::theme;
use egui::{Align, Layout, Rect, RichText, Ui};

impl Editor {
    pub(super) fn status_bar(&self, ui: &mut Ui, stats: &EditorStats, output: &mut EditorOutput) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Right to left: the stats always fit, the message gets the rest.
            let count = self.problem_count.0;
            let text = if count == 0 {
                dim("NO PROBLEMS")
            } else {
                egui::RichText::new(format!("PROBLEMS {count}")).color(crate::ui::theme::ALERT)
            };
            if ui.add(egui::Button::new(text).small()).clicked() {
                output.show_problems = true;
            }
            ui.add_space(12.0);
            if self.show_stats {
                ui.label(format!("{} / {}", stats.drawn[0], stats.drawn[1]));
                ui.label(dim("DRAWN"));
                ui.add_space(12.0);
                let zone = stats.zone.as_deref().unwrap_or("-").to_uppercase();
                ui.label(zone).on_hover_text(format!(
                    "ACTIVE: {}",
                    crate::runtime::active_text(&stats.active_zones)
                ));
                ui.label(dim("ZONE"));
                ui.add_space(12.0);
                let scale = format!(
                    "{}×{} ×{}{} ▾",
                    stats.internal[0],
                    stats.internal[1],
                    stats.factor,
                    if stats.scale_override.is_none() {
                        " AUTO"
                    } else {
                        ""
                    }
                );
                ui.menu_button(scale, |ui| {
                    if ui.button("AUTO").clicked() {
                        output.render_scale = Some(None);
                        ui.close();
                    }
                    for factor in 1..=crate::render::scale::MAX_FACTOR {
                        if ui.button(format!("×{factor}")).clicked() {
                            output.render_scale = Some(Some(factor));
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("RENDER SCALE: OUTPUT PIXELS PER INTERNAL PIXEL ([ ] 0)");
                ui.label(dim("RENDER"));
                ui.add_space(12.0);
                if let Some(gpu) = stats.gpu_ms {
                    ui.label(format!("{gpu:.2} MS"));
                    ui.label(dim("GPU"));
                    ui.add_space(12.0);
                }
                ui.label(format!("{:.0}  {:.2} MS", stats.fps, stats.frame_ms));
                ui.label(dim("FPS"));
                ui.add_space(24.0);
            }
            // Only when a bake is running, missing or out of date.
            let notes: Vec<String> = [("SOUND", &self.bake_status), ("LIGHT", &self.light_status)]
                .iter()
                .filter(|(_, status)| !status.is_empty() && status.as_str() != "BAKED")
                .map(|(name, status)| {
                    format!("{name} {}", status.replace("BAKE OUTDATED", "OUTDATED"))
                })
                .collect();
            if !notes.is_empty() {
                ui.label(RichText::new(notes.join("  ")).color(theme::AMBER));
                ui.add_space(24.0);
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if let Some((text, error)) = &self.message {
                    let color = if *error {
                        theme::ALERT
                    } else {
                        theme::AMBER_DIM
                    };
                    let text = RichText::new(text.to_uppercase()).color(color);
                    ui.add(egui::Label::new(text).truncate());
                }
            });
        });
    }

    /// Controls not shown in the menus, in the viewport's bottom-left corner.
    pub(super) fn hints(&self, ui: &Ui, viewport: Rect) {
        if !self.show_hints {
            return;
        }
        let text = if self.playing {
            "WASD MOVE · SPACE JUMP · E USE / TALK · M MAP · F3 TEST DRAWER · ESC FREE MOUSE · F2 FLY · R RESPAWN"
        } else {
            "CLICK SELECT · SHIFT+CLICK ADD · HOLD RMB + WASD FLY · Q/E DOWN/UP · CTRL+K SEARCH · ? ALL KEYS"
        };
        egui::Area::new(egui::Id::unique("editor_hints"))
            // Anchored at the bottom, so wrapped lines grow upward.
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(viewport.left_bottom() + egui::vec2(10.0, -8.0))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                ui.set_max_width(viewport.width() - 20.0);
                ui.label(RichText::new(text).small().color(theme::AMBER_DIM));
            });
    }
}
