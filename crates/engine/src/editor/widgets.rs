//! Small UI pieces shared by the editor panels.

use super::inspector;
use crate::ui::theme;
use egui::{RichText, Ui};

/// [ON]/[OFF] row for a setting.
pub(super) fn on_off(ui: &mut Ui, label: &str, value: &mut bool) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, label);
        let text = if *value { "[ON] " } else { "[OFF]" };
        if ui.small_button(text).clicked() {
            *value = !*value;
        }
    });
}

pub(super) fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong());
    ui.separator();
}

pub(super) fn dim(text: &str) -> RichText {
    RichText::new(text).color(theme::AMBER_DIM)
}

/// Which edge of the window a side panel sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Left,
    Right,
}

/// Width of a collapsed side panel, in points.
const STRIP_WIDTH: f32 = 26.0;

/// A side panel's title row: the title, then `extra` and the collapse
/// button at the right end. Returns true if the collapse button was
/// clicked; `key` is the shortcut that does the same.
pub(super) fn panel_header(
    ui: &mut Ui,
    title: &str,
    side: Side,
    key: &str,
    extra: impl FnOnce(&mut Ui),
) -> bool {
    ui.add_space(4.0);
    let collapse = ui
        .horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let arrow = match side {
                    Side::Left => "<",
                    Side::Right => ">",
                };
                let clicked = ui
                    .small_button(arrow)
                    .on_hover_text(format!("HIDE THIS PANEL ({key})"))
                    .clicked();
                extra(ui);
                clicked
            })
            .inner
        })
        .inner;
    ui.add_space(4.0);
    collapse
}

/// What is left of a collapsed side panel: a thin strip along its edge
/// with a button that brings it back. Returns true if that was clicked.
pub(super) fn collapsed_panel(ui: &mut Ui, id: &str, side: Side, title: &str, key: &str) -> bool {
    let (panel, arrow) = match side {
        Side::Left => (egui::Panel::left(id), ">"),
        Side::Right => (egui::Panel::right(id), "<"),
    };
    panel
        .resizable(false)
        .exact_size(STRIP_WIDTH)
        .show(ui, |ui| {
            ui.add_space(4.0);
            ui.vertical_centered(|ui| {
                ui.small_button(arrow)
                    .on_hover_text(format!("SHOW THE {title} ({key})"))
                    .clicked()
            })
            .inner
        })
        .inner
}

/// A dialog's title and the line under it.
pub(super) fn dialog_heading(ui: &mut Ui, title: &str, text: &str) {
    ui.label(RichText::new(title).strong());
    ui.label(dim(text));
    ui.add_space(8.0);
}
