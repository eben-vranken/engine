//! PROJECT ▸ PROJECT SETTINGS: the game's own settings (`game.ron`): its
//! name, the scene it starts in and the render scale it starts at. The
//! render scale shows live while the dialog is open; OK saves, CANCEL puts
//! everything back.

use super::files::graph_files;
use super::{Editor, EditorOutput, EditorStats, dialog_heading, dim, inspector, scene_files};
use crate::project::{GameSettings, Project};
use crate::render::scale::MAX_FACTOR;
use crate::ui::theme;
use egui::{RichText, Ui};

/// The dialog's state while it is open.
pub(super) struct SettingsDialog {
    settings: GameSettings,
    /// The render scale in use when it opened (back to it on CANCEL).
    before_scale: Option<u32>,
    error: Option<String>,
}

impl Editor {
    /// Opens PROJECT SETTINGS with what `game.ron` holds now.
    fn open_project_settings(&mut self, current_scale: Option<u32>) {
        match Project::open(&self.project_dir) {
            Ok(project) => {
                self.project_settings = Some(SettingsDialog {
                    settings: project.settings,
                    before_scale: current_scale,
                    error: None,
                });
            }
            Err(err) => self.show_error(err.to_string()),
        }
    }

    /// The dialog, while it is open.
    pub(super) fn project_settings_dialog(
        &mut self,
        ui: &Ui,
        stats: &EditorStats,
        output: &mut EditorOutput,
    ) {
        if std::mem::take(&mut self.want_project_settings) {
            self.open_project_settings(stats.scale_override);
        }
        let Some(dialog) = &mut self.project_settings else {
            return;
        };
        let scenes = scene_files(&self.project_dir);
        let graphs = graph_files(&self.project_dir);
        let settings = &mut dialog.settings;
        let scale_before = settings.render_scale;
        let (mut ok, mut cancel) = (false, false);
        egui::Modal::new(egui::Id::unique("project_settings")).show(ui.ctx(), |ui| {
            ui.set_min_width(420.0);
            dialog_heading(
                ui,
                "PROJECT SETTINGS",
                "THIS GAME'S OWN, IN GAME.RON: THE GAME STARTS WITH THEM TOO",
            );
            egui::Grid::new("project_settings_grid")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label(dim("NAME"));
                    ui.text_edit_singleline(&mut settings.name);
                    ui.end_row();

                    ui.label(dim("START SCENE"));
                    egui::ComboBox::from_id_salt("start_scene")
                        .selected_text(settings.start_scene.display().to_string().to_uppercase())
                        .show_ui(ui, |ui| {
                            for path in &scenes {
                                let text = path.display().to_string().to_uppercase();
                                ui.selectable_value(&mut settings.start_scene, path.clone(), text);
                            }
                        });
                    ui.end_row();

                    ui.label(dim("FLOW GRAPH"))
                        .on_hover_text("RUNS FROM ITS START WHEN THE GAME BEGINS");
                    let flow = settings
                        .flow
                        .as_ref()
                        .map_or("NONE".into(), |p| p.display().to_string().to_uppercase());
                    egui::ComboBox::from_id_salt("flow_graph")
                        .selected_text(flow)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut settings.flow, None, "NONE");
                            for path in &graphs {
                                let text = path.display().to_string().to_uppercase();
                                ui.selectable_value(&mut settings.flow, Some(path.clone()), text);
                            }
                        });
                    ui.end_row();

                    ui.label(dim("RENDER SCALE"));
                    ui.horizontal(|ui| {
                        let auto = settings.render_scale.is_none();
                        let label = if auto { "[AUTO]" } else { " AUTO " };
                        if ui
                            .small_button(label)
                            .on_hover_text("PICKED FROM THE WINDOW: ABOUT 720 LINES OR LESS")
                            .clicked()
                        {
                            settings.render_scale = if auto { Some(stats.factor) } else { None };
                        }
                        if let Some(factor) = &mut settings.render_scale {
                            if ui.small_button("-").clicked() {
                                *factor = factor.saturating_sub(1).max(1);
                            }
                            ui.label(format!("×{factor}"));
                            if ui.small_button("+").clicked() {
                                *factor = (*factor + 1).min(MAX_FACTOR);
                            }
                        }
                    });
                    ui.end_row();
                });
            ui.label(dim(&format!(
                "NOW ×{}: {}×{} INTERNAL. [ AND ] STILL CHANGE IT WHILE RUNNING;",
                stats.factor, stats.internal[0], stats.internal[1]
            )));
            ui.label(dim("THIS IS THE SCALE EVERY START BEGINS AT."));
            ui.add_space(8.0);
            egui::CollapsingHeader::new("DIALOGUE LOOK")
                .id_salt("dialogue_look")
                .show(ui, |ui| {
                    inspector::fields(ui, &mut settings.dialogue, &inspector::Names::default());
                    if ui.small_button("RESET TO DEFAULT").clicked() {
                        settings.dialogue = Default::default();
                    }
                });
            if let Some(error) = &dialog.error {
                ui.label(RichText::new(error).small().color(theme::ALERT));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ok = ui.button("OK").clicked();
                cancel =
                    ui.button("CANCEL").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if settings.render_scale != scale_before {
            output.render_scale = Some(settings.render_scale); // preview
        }
        if cancel {
            output.render_scale = Some(dialog.before_scale);
            self.project_settings = None;
            return;
        }
        if !ok {
            return;
        }
        settings.name = settings.name.trim().to_owned();
        if settings.name.is_empty() {
            dialog.error = Some("THE GAME NEEDS A NAME".into());
            return;
        }
        match settings.save(&self.project_dir) {
            Ok(()) => {
                log::info!("saved {}", self.project_dir.join("game.ron").display());
                self.game_name = settings.name.clone();
                output.rename = Some(settings.name.clone());
                self.project_settings = None;
                self.show_message("SAVED GAME.RON".into());
            }
            Err(err) => dialog.error = Some(err.to_string()),
        }
    }
}
