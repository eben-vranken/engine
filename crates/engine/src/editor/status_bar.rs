//! The status line along the bottom: the last message on the left; bake
//! notes and the frame stats on the right. And the key hints over the
//! viewport.

use super::{Editor, EditorStats, dim};
use crate::ui::theme;
use egui::{Align, Layout, Rect, RichText, Ui};

impl Editor {
    pub(super) fn status_bar(&self, ui: &mut Ui, stats: &EditorStats) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Right to left: the stats always fit, the message gets the rest.
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
                ui.label(format!(
                    "{}×{} ×{}",
                    stats.internal[0], stats.internal[1], stats.factor
                ));
                ui.label(dim("INTERNAL"));
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
            "F5 STOP  E USE / TALK  TAB MAP  F3 GRAPHS  ESC FREE MOUSE  CLICK CAPTURE MOUSE  R RESPAWN  F1 TUNING  F2 FLY"
        } else {
            "CLICK SELECT  ESC DESELECT  HOLD RMB: WASD FLY  Q/E DOWN/UP  SHIFT FAST  WHEEL SPEED"
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
