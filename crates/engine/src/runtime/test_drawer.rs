//! PLAY's test drawer (F3): LOGIC (running graphs with their active nodes
//! lit, and what happened), FLAGS (flip them live), AUDIO (the acoustic
//! switches, clap, move the looping sound), FEEL (the player tuning, as
//! F1), MAP (visited zones) and STATS.

use super::map::{MapData, draw_map_in};
use super::story::Story;
use crate::audio::Toggles;
use crate::editor::graph_canvas::{self, Marks};
use crate::graph::flags::FlagValue;
use crate::player::settings::PlayerSettings;
use crate::player::tuning::{self, TuningAction};
use crate::ui::theme;
use egui::{RichText, Sense, Ui, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Tab {
    #[default]
    Logic,
    Flags,
    Audio,
    Feel,
    Map,
    Stats,
}

/// What the drawer asked for this frame.
#[derive(Default)]
pub(super) struct Actions {
    pub clap: bool,
    pub move_sound: bool,
    pub tuning: Option<TuningAction>,
}

pub(super) struct Inputs<'a> {
    pub toggles: Option<&'a mut Toggles>,
    pub settings: &'a mut PlayerSettings,
    pub map: Option<&'a MapData>,
    pub stats: &'a [(&'static str, String)],
}

fn dim(text: &str) -> RichText {
    RichText::new(text).color(theme::AMBER_DIM)
}

pub(super) fn drawer(ui: &mut Ui, story: &mut Story, inputs: Inputs) -> Actions {
    let mut actions = Actions::default();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("TEST DRAWER").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(dim("F3 HIDE"));
        });
    });
    ui.horizontal(|ui| {
        for (tab, name) in [
            (Tab::Logic, "LOGIC"),
            (Tab::Flags, "FLAGS"),
            (Tab::Audio, "AUDIO"),
            (Tab::Feel, "FEEL"),
            (Tab::Map, "MAP"),
            (Tab::Stats, "STATS"),
        ] {
            let text = if story.tab == tab {
                RichText::new(name).strong()
            } else {
                dim(name)
            };
            if ui.selectable_label(story.tab == tab, text).clicked() {
                story.tab = tab;
            }
        }
    });
    ui.separator();
    match story.tab {
        Tab::Logic => logic(ui, story),
        Tab::Flags => flags(ui, story),
        Tab::Audio => {
            match inputs.toggles {
                Some(toggles) => {
                    for (key, name, value) in [
                        ("F6", "OCCLUSION", &mut toggles.occlusion),
                        ("F7", "REFLECTIONS", &mut toggles.reflections),
                        ("F8", "HRTF", &mut toggles.hrtf),
                        ("F9", "AIR ABSORPTION", &mut toggles.air_absorption),
                        ("F10", "BAKED REVERB", &mut toggles.baked),
                    ] {
                        let mark = if *value { "[ON] " } else { "[OFF]" };
                        if ui.button(format!("{mark} {name}   {key}")).clicked() {
                            *value = !*value;
                        }
                    }
                }
                None => {
                    ui.label(dim("NO AUDIO DEVICE: THE GAME RUNS SILENT"));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                actions.clap = ui.button("CLAP  Q").clicked();
                actions.move_sound = ui
                    .button("MOVE THE LOOPING SOUND HERE")
                    .on_hover_text("BRINGS THE FIRST LOOPING SOUND SOURCE TO WHERE YOU STAND")
                    .clicked();
            });
        }
        Tab::Feel => {
            ui.label(dim("THE GAME'S PLAYER.RON, LIVE (THE SAME AS F1)"));
            egui::ScrollArea::vertical()
                .id_salt("drawer_feel")
                .max_height(ui.available_height() - 40.0)
                .show(ui, |ui| tuning::sections(ui, inputs.settings));
            ui.horizontal(|ui| {
                if ui.button("SAVE").clicked() {
                    actions.tuning = Some(TuningAction::Save);
                }
                if ui.button("RESET TO DEFAULTS").clicked() {
                    actions.tuning = Some(TuningAction::ResetToDefaults);
                }
            });
        }
        Tab::Map => match inputs.map {
            Some(map) => {
                let size = vec2(
                    ui.available_width(),
                    ui.available_width().min(ui.available_height()),
                );
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, theme::BACKGROUND);
                draw_map_in(&painter, rect.shrink(12.0), map);
                ui.label(dim("ZONES VISITED IN THIS SCENE · M SHOWS IT FULL SIZE"));
            }
            None => {
                ui.label(dim("NO MAP"));
            }
        },
        Tab::Stats => {
            egui::Grid::new("drawer_stats")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for (name, value) in inputs.stats {
                        ui.label(dim(name));
                        ui.label(value);
                        ui.end_row();
                    }
                });
        }
    }
    actions
}

fn logic(ui: &mut Ui, story: &mut Story) {
    let running = story.runner.running();
    if running.is_empty() {
        ui.label(dim("NO GRAPH IS RUNNING"));
    }
    story.debug_graph = story.debug_graph.min(running.len().saturating_sub(1));
    for (i, (path, _, active)) in running.iter().enumerate() {
        let text = format!(
            "{}  [{} ACTIVE]",
            path.trim_start_matches("graphs/").to_uppercase(),
            active.len()
        );
        if ui.selectable_label(i == story.debug_graph, text).clicked() {
            story.debug_graph = i;
        }
    }
    ui.add_space(4.0);
    let height = (ui.available_height() * 0.55).max(160.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    if let Some((_, graph, active)) = running.get(story.debug_graph) {
        let mut graph = (**graph).clone();
        let marks = Marks {
            selected: None,
            selected_link: None,
            active,
            found: &[],
        };
        graph_canvas::show(ui, rect, &mut graph, &mut story.debug_view, &marks, false);
    }
    ui.add_space(4.0);
    ui.label(dim("LIT = WAITING NOW"));
    ui.add_space(6.0);
    ui.label(RichText::new("EVENTS").strong());
    egui::ScrollArea::vertical()
        .id_salt("drawer_events")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if story.log.is_empty() {
                ui.label(dim("NOTHING YET"));
            }
            for (time, text) in story.log.iter().rev() {
                ui.horizontal(|ui| {
                    let minutes = (*time / 60.0) as u32;
                    let seconds = (*time % 60.0) as u32;
                    ui.label(dim(&format!("{minutes:02}:{seconds:02}")));
                    ui.label(RichText::new(text.to_uppercase()).small());
                });
            }
        });
}

fn flags(ui: &mut Ui, story: &mut Story) {
    ui.label(dim(
        "THE FLAGS NOW; CLICK A YES/NO FLAG TO FLIP IT FOR THIS RUN",
    ));
    ui.add_space(4.0);
    if story.runner.flags.0.is_empty() {
        ui.label(dim("NO FLAGS"));
    }
    egui::ScrollArea::vertical()
        .id_salt("drawer_flags")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (name, value) in story.runner.flags.0.iter_mut() {
                ui.horizontal(|ui| {
                    ui.label(name.as_str());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match value {
                            FlagValue::Bool(b) => {
                                let text = if *b { "[TRUE] " } else { "[FALSE]" };
                                if ui.small_button(text).clicked() {
                                    *b = !*b;
                                }
                            }
                            other => {
                                ui.label(other.to_string());
                            }
                        }
                    });
                });
            }
        });
}
