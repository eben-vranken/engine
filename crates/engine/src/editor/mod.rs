//! The editor: the running game with panels on top. The scene hierarchy is
//! on the left (grouped by zone), the inspector on the right, and the 3D
//! viewport takes whatever space is left in the middle.
//!
//! Every change goes through [`history::Edit`] so it can be undone. While a
//! value is being dragged or typed, the change is applied live but held as
//! *pending*; it becomes one undo step when the interaction ends.

pub mod history;
pub mod inspector;

use crate::scene::{Object, Scene};
use crate::ui::theme;
use egui::{Align, Layout, Rect, RichText, Ui};
use history::{Edit, History};
use std::path::PathBuf;

pub struct Editor {
    /// Index into `Scene::objects`.
    pub selected: Option<usize>,
    /// Open the selection's zone and scroll to it in the hierarchy (set when
    /// it was picked in the viewport).
    pub reveal_selection: bool,
    /// The scene file being edited, for display.
    pub scene_path: PathBuf,
    pub game_name: String,
    history: History,
    /// An edit in progress: the object's index and how it was before.
    pending: Option<(usize, Object)>,
}

/// Numbers for the top bar.
pub struct EditorStats {
    pub fps: f32,
    pub frame_ms: f32,
    pub internal: [u32; 2],
    pub factor: u32,
}

/// What the editor did this frame, for the runtime to act on.
pub struct EditorOutput {
    /// Space left for the 3D viewport, in points.
    pub viewport: Rect,
    /// Objects whose data changed (redraw them).
    pub changed: Vec<usize>,
    /// An edit was committed, undone or redone (rebuild collision).
    pub settled: bool,
}

impl Editor {
    pub fn new(game_name: String, scene_path: PathBuf) -> Self {
        Self {
            selected: None,
            reveal_selection: false,
            scene_path,
            game_name,
            history: History::default(),
            pending: None,
        }
    }

    /// Draws the panels, applies inspector edits to `scene`, and reports
    /// what changed.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        stats: &EditorStats,
        blink: bool,
    ) -> EditorOutput {
        let mut output = EditorOutput {
            viewport: Rect::NOTHING,
            changed: Vec::new(),
            settled: false,
        };
        egui::Panel::top("editor_top")
            .show(ui, |ui| self.top_bar(ui, scene, stats, blink, &mut output));
        egui::Panel::left("editor_scene")
            .resizable(true)
            .default_size(260.0)
            .min_size(220.0)
            .show(ui, |ui| self.hierarchy(ui, scene));
        egui::Panel::right("editor_inspector")
            .resizable(true)
            .default_size(340.0)
            // Wide enough for its content, so selecting doesn't resize the viewport.
            .min_size(340.0)
            .show(ui, |ui| self.inspector(ui, scene, &mut output));

        // A drag or text entry has ended: the pending change becomes one undo step.
        let interacting =
            ui.ctx().egui_is_using_pointer() || ui.ctx().memory(|m| m.focused().is_some());
        if !interacting {
            output.settled |= self.commit_pending(scene);
        }

        output.viewport = ui.available_rect_before_wrap();
        hints(ui, output.viewport);
        output
    }

    /// Records the edit in progress, if any. Returns true if one was recorded.
    pub fn commit_pending(&mut self, scene: &Scene) -> bool {
        let Some((index, before)) = self.pending.take() else {
            return false;
        };
        let after = &scene.objects[index];
        if *after == before {
            return false;
        }
        self.history.record(Edit::SetObject {
            index,
            before: Box::new(before),
            after: Box::new(after.clone()),
        });
        true
    }

    /// Undoes the last edit; returns the object it changed.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<usize> {
        self.commit_pending(scene);
        self.history.undo(scene)
    }

    /// Redoes the last undone edit; returns the object it changed.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<usize> {
        self.commit_pending(scene);
        self.history.redo(scene)
    }

    /// Changes the selection, finishing any edit on the previous one.
    pub fn select(&mut self, scene: &Scene, selected: Option<usize>) -> bool {
        let settled = self.commit_pending(scene);
        self.selected = selected;
        settled
    }

    fn top_bar(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        stats: &EditorStats,
        blink: bool,
        output: &mut EditorOutput,
    ) {
        ui.horizontal(|ui| {
            let cursor = if blink { "_" } else { " " };
            ui.label(RichText::new(format!("ENGINE // EDITOR{cursor}")).strong());
            ui.add_space(12.0);
            ui.label(self.game_name.to_uppercase());
            ui.label(dim(&self.scene_path.display().to_string().to_uppercase()));
            ui.add_space(12.0);

            let undo = format!("UNDO [{}]", self.history.undo_len());
            if ui
                .add_enabled(self.history.undo_len() > 0, egui::Button::new(undo).small())
                .clicked()
                && let Some(index) = self.undo(scene)
            {
                output.changed.push(index);
                output.settled = true;
            }
            let redo = format!("REDO [{}]", self.history.redo_len());
            if ui
                .add_enabled(self.history.redo_len() > 0, egui::Button::new(redo).small())
                .clicked()
                && let Some(index) = self.redo(scene)
            {
                output.changed.push(index);
                output.settled = true;
            }

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
        let mut clicked = None;
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
                    let reveal = self.reveal_selection
                        && members.iter().any(|(i, _)| Some(*i) == self.selected);
                    egui::CollapsingHeader::new(header)
                        .id_salt(&zone.name)
                        .open(reveal.then_some(true))
                        .show(ui, |ui| {
                            for (index, object) in members {
                                let selected = self.selected == Some(index);
                                let response = ui.selectable_label(selected, &object.name);
                                if response.clicked() {
                                    clicked = Some(index);
                                }
                                if selected && reveal {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                            }
                        });
                }
            });
        self.reveal_selection = false;
        if let Some(index) = clicked {
            self.select(scene, Some(index));
        }
    }

    fn inspector(&mut self, ui: &mut Ui, scene: &mut Scene, output: &mut EditorOutput) {
        ui.add_space(4.0);
        ui.label(RichText::new("INSPECTOR").strong());
        ui.add_space(4.0);
        let Some(index) = self.selected.filter(|&i| i < scene.objects.len()) else {
            ui.label(dim("NOTHING SELECTED"));
            return;
        };

        let zones = &scene.zones;
        let object = &mut scene.objects[index];
        let before = object.clone();

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    inspector::field_label(ui, "NAME");
                    ui.text_edit_singleline(&mut object.name);
                });
                ui.horizontal(|ui| {
                    inspector::field_label(ui, "ZONE");
                    egui::ComboBox::from_id_salt("inspector_zone")
                        .selected_text(object.zone.to_uppercase())
                        .show_ui(ui, |ui| {
                            for zone in zones {
                                ui.selectable_value(
                                    &mut object.zone,
                                    zone.name.clone(),
                                    zone.name.to_uppercase(),
                                );
                            }
                        });
                });

                section(ui, "TRANSFORM");
                inspector::fields(ui, &mut object.transform);

                for component in &mut object.components {
                    section(ui, &inspector::title(component));
                    inspector::fields(ui, component);
                }
            });

        if *object != before {
            if self.pending.as_ref().is_none_or(|(i, _)| *i != index) {
                self.pending = Some((index, before));
            }
            output.changed.push(index);
        }
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
                RichText::new(
                    "CLICK SELECT  RMB + WASD/QE FLY  WHEEL SPEED  CTRL+Z/Y UNDO/REDO  ESC DESELECT",
                )
                .small()
                .color(theme::AMBER_DIM),
            );
        });
}
