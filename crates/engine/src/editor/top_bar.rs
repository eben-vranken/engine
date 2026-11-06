//! The bar along the top of the editor, and the key hints over the
//! viewport.

use super::gizmo::View;
use super::objects::primitives;
use super::{Editor, EditorOutput, EditorStats, dim, new_scene, scene_files};
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use crate::ui::theme;
use egui::{Align, Layout, Rect, RichText, Ui};
use transform_gizmo::config::GizmoModeKind;

impl Editor {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn top_bar(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        stats: &EditorStats,
        blink: bool,
        view: &View,
        output: &mut EditorOutput,
    ) {
        ui.horizontal(|ui| {
            if ui
                .small_button(if self.show_scene { "[≡]" } else { " ≡ " })
                .on_hover_text("SHOW / HIDE THE SCENE PANEL")
                .clicked()
            {
                self.show_scene = !self.show_scene;
            }
            let cursor = if blink { "_" } else { " " };
            ui.label(RichText::new(format!("ENGINE // EDITOR{cursor}")).strong());
            ui.add_space(12.0);
            ui.label(self.game_name.to_uppercase());
            ui.add_space(12.0);
            let play = if self.playing {
                "[STOP F5]"
            } else {
                " PLAY F5 "
            };
            if ui.small_button(play).clicked() {
                output.toggle_play = true;
            }
            if self.playing {
                ui.label(RichText::new("[PLAYING]").color(theme::AMBER));
            }
            ui.add_space(12.0);
            // Editing waits until Stop: the scene is restored then.
            ui.add_enabled_ui(!self.playing, |ui| {
                egui::ComboBox::from_id_salt("scene_picker")
                    .selected_text(self.scene_path.display().to_string().to_uppercase())
                    .show_ui(ui, |ui| {
                        for path in scene_files(&self.project_dir) {
                            let current = path == self.scene_path;
                            let text = path.display().to_string().to_uppercase();
                            if ui.selectable_label(current, text).clicked() && !current {
                                self.open_scene(path, output);
                            }
                        }
                    });
                if self.is_dirty() {
                    ui.label(RichText::new("[MODIFIED]").color(theme::AMBER));
                }
                if ui.small_button("SAVE").clicked() {
                    output.settled |= self.save(scene, player);
                }
                if let Some((text, error)) = &self.message {
                    let color = if *error {
                        theme::ALERT
                    } else {
                        theme::AMBER_DIM
                    };
                    ui.label(RichText::new(text.to_uppercase()).small().color(color));
                }
                ui.add_space(12.0);

                let undo = format!("UNDO [{}]", self.history.undo_len());
                if ui
                    .add_enabled(self.history.undo_len() > 0, egui::Button::new(undo).small())
                    .clicked()
                    && let Some(change) = self.undo(scene, player)
                {
                    output.changed.push(change);
                    output.settled = true;
                }
                let redo = format!("REDO [{}]", self.history.redo_len());
                if ui
                    .add_enabled(self.history.redo_len() > 0, egui::Button::new(redo).small())
                    .clicked()
                    && let Some(change) = self.redo(scene, player)
                {
                    output.changed.push(change);
                    output.settled = true;
                }

                ui.add_space(12.0);
                ui.add_enabled_ui(!scene.zones.is_empty(), |ui| {
                    ui.menu_button("ADD", |ui| {
                        for (kind, shape) in primitives() {
                            if ui.button(kind.to_uppercase()).clicked()
                                && let Some(change) = self.add_primitive(scene, kind, shape, view)
                            {
                                output.changed.push(change);
                                output.settled = true;
                            }
                        }
                        ui.separator();
                        let has_start = scene.player_start().is_some();
                        if ui
                            .add_enabled(!has_start, egui::Button::new("PLAYER START"))
                            .on_disabled_hover_text("THE SCENE ALREADY HAS ONE")
                            .clicked()
                            && let Some(change) = self.add_player_start(scene, view)
                        {
                            output.changed.push(change);
                            output.settled = true;
                        }
                        ui.separator();
                        if ui.button("ZONE VOLUME").clicked()
                            && let Some(change) = self.add_zone_volume(scene, view)
                        {
                            output.changed.push(change);
                            output.settled = true;
                        }
                        if ui.button("PORTAL").clicked()
                            && let Some(change) = self.add_portal(scene, view)
                        {
                            output.changed.push(change);
                            output.settled = true;
                        }
                    });
                });

                ui.add_space(12.0);
                // Both bakes in one menu; the bar only says something when
                // a bake is running or missing.
                let bakes = [
                    (
                        "ACOUSTICS",
                        "SOUND",
                        &self.bake_status,
                        "REVERB PROBES IN EVERY ZONE VOLUME",
                    ),
                    (
                        "LIGHTING",
                        "LIGHT",
                        &self.light_status,
                        "BOUNCE, SKY AND EMISSIVE LIGHT INTO LIGHTMAPS AND PROBES",
                    ),
                ];
                let mut clicked = [false; 2];
                ui.menu_button("BAKE", |ui| {
                    for (i, (name, _, status, hint)) in bakes.iter().enumerate() {
                        let busy = status.starts_with("BAKING");
                        if ui
                            .add_enabled(!busy, egui::Button::new(format!("{name}  {status}")))
                            .on_hover_text(*hint)
                            .clicked()
                        {
                            clicked[i] = true;
                            ui.close();
                        }
                    }
                });
                output.bake |= clicked[0];
                output.bake_lighting |= clicked[1];
                let mut naming = None;
                ui.menu_button("PROJECT", |ui| {
                    if ui.button("NEW SCENE").clicked() {
                        naming = Some(new_scene::Naming::New);
                        ui.close();
                    }
                    if ui.button("SAVE SCENE AS").clicked() {
                        naming = Some(new_scene::Naming::SaveAs);
                        ui.close();
                    }
                    ui.separator();
                    let hint = "THE WHOLE PROJECT FOLDER IN ONE .GAMEPACK FILE NEXT TO IT,                                 TO OPEN IN ANOTHER EDITOR (SAVED FILES ONLY)";
                    if ui.button("EXPORT PACKAGE").on_hover_text(hint).clicked() {
                        output.export_package = true;
                        ui.close();
                    }
                });
                let notes: Vec<String> = bakes
                    .iter()
                    .filter(|(_, _, status, _)| !status.is_empty() && status.as_str() != "BAKED")
                    .map(|(_, short_name, status, _)| {
                        let short = status.replace("BAKE OUTDATED", "OUTDATED");
                        format!("{short_name} {short}")
                    })
                    .collect();
                if !notes.is_empty() {
                    ui.label(RichText::new(notes.join("  ")).small().color(theme::AMBER));
                }
                if let Some(purpose) = naming {
                    self.ask_name(purpose);
                }

                ui.add_space(12.0);
                for (kind, text) in [
                    (GizmoModeKind::Translate, "MOVE W"),
                    (GizmoModeKind::Rotate, "ROTATE E"),
                    (GizmoModeKind::Scale, "SCALE R"),
                ] {
                    let label = if self.gizmo.kind == kind {
                        format!("[{text}]")
                    } else {
                        format!(" {text} ")
                    };
                    if ui.small_button(label).clicked() {
                        self.gizmo.kind = kind;
                    }
                }
            });

            // Stats on the right, as many as fit: the rest are dropped
            // rather than drawn over the buttons.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button(if self.show_inspector {
                        "[≡]"
                    } else {
                        " ≡ "
                    })
                    .on_hover_text("SHOW / HIDE THE INSPECTOR")
                    .clicked()
                {
                    self.show_inspector = !self.show_inspector;
                }
                ui.add_space(8.0);
                let room = ui.available_width();
                let zone = stats.zone.as_deref().unwrap_or("-").to_uppercase();
                let fps = format!("{:.0}  {:.2} MS", stats.fps, stats.frame_ms);
                if room > 620.0 {
                    ui.label(format!("{} / {}", stats.drawn[0], stats.drawn[1]));
                    ui.label(dim("DRAWN"));
                    ui.add_space(12.0);
                }
                if room > 460.0 {
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
                }
                if room > 300.0
                    && let Some(gpu) = stats.gpu_ms
                {
                    ui.label(format!("{gpu:.2} MS"));
                    ui.label(dim("GPU"));
                    ui.add_space(12.0);
                }
                if room > 140.0 {
                    ui.label(fps);
                    ui.label(dim("FPS"));
                }
            });
        });
    }
}

/// Controls, in the viewport's bottom-left corner.
pub(super) fn hints(ui: &Ui, viewport: Rect, playing: bool) {
    let text = if playing {
        "F5 STOP  E USE  ESC FREE MOUSE  CLICK CAPTURE MOUSE  R RESPAWN  F1 TUNING  F2 FLY"
    } else {
        "CLICK SELECT  W/E/R GIZMO  CTRL SNAP  RMB+WASD FLY  CTRL+D DUPLICATE  DEL DELETE  CTRL+Z/Y UNDO/REDO  CTRL+S SAVE  F5 PLAY  TAB/SHIFT+TAB PANELS  H ZONES"
    };
    hint_line(ui, viewport, text);
}

/// A line of key hints along the bottom of the viewport.
pub(super) fn hint_line(ui: &Ui, viewport: Rect, text: &str) {
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
