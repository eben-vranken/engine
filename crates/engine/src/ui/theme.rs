//! The amber terminal look (BRIEF.md → UI style): one amber accent on
//! near-black, a dimmer amber for secondary text and lines, one monospace
//! face, 1 px square frames, no shadows.

use egui::{Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle};

pub const AMBER: Color32 = Color32::from_rgb(255, 158, 44);
pub const AMBER_DIM: Color32 = Color32::from_rgb(150, 88, 24);
pub const BACKGROUND: Color32 = Color32::from_rgb(10, 8, 6);
pub const ALERT: Color32 = Color32::from_rgb(255, 64, 48);

pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, 10.0),
            (TextStyle::Body, 12.0),
            (TextStyle::Button, 12.0),
            (TextStyle::Monospace, 12.0),
            (TextStyle::Heading, 14.0),
        ]
        .into_iter()
        .map(|(text_style, size)| (text_style, FontId::new(size, FontFamily::Monospace)))
        .collect();

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(AMBER);
        v.weak_text_color = Some(AMBER_DIM);
        v.hyperlink_color = AMBER;
        v.warn_fg_color = AMBER;
        v.error_fg_color = ALERT;
        v.panel_fill = BACKGROUND;
        v.window_fill = BACKGROUND;
        v.window_stroke = Stroke::new(1.0, AMBER_DIM);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.extreme_bg_color = BACKGROUND;
        v.faint_bg_color = BACKGROUND;
        v.selection.bg_fill = AMBER_DIM;
        v.selection.stroke = Stroke::new(1.0, AMBER);

        let w = &mut v.widgets;
        for (state, fill, stroke) in [
            (&mut w.noninteractive, BACKGROUND, AMBER_DIM),
            (&mut w.inactive, BACKGROUND, AMBER_DIM),
            (&mut w.hovered, BACKGROUND, AMBER),
            (&mut w.active, AMBER_DIM, AMBER),
            (&mut w.open, BACKGROUND, AMBER),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, AMBER);
            state.corner_radius = CornerRadius::ZERO;
            state.expansion = 0.0;
        }
    });
}
