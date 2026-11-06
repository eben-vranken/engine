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
