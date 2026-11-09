//! In-game tuning panel (F1): every player property, editable live.
//! Hand-written, with ranges and units; the editor shows the same rows
//! under Player Start.

use super::settings::PlayerSettings;
use crate::ui::theme;
use egui::{DragValue, RichText, Ui};
use std::ops::RangeInclusive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningAction {
    Save,
    ResetToDefaults,
}

/// Movement, jump, body and camera rows (also shown in the editor's
/// inspector under Player Start).
pub fn sections(ui: &mut Ui, s: &mut PlayerSettings) {
    section(ui, "MOVEMENT", |ui| {
        let m = &mut s.movement;
        row(ui, "WALK SPEED", &mut m.walk_speed, 0.0..=30.0, "M/S");
        row(ui, "SPRINT SPEED", &mut m.sprint_speed, 0.0..=30.0, "M/S");
        row(ui, "CROUCH SPEED", &mut m.crouch_speed, 0.0..=30.0, "M/S");
        row(
            ui,
            "GROUND ACCEL",
            &mut m.ground_acceleration,
            0.0..=500.0,
            "M/S²",
        );
        row(
            ui,
            "GROUND DECEL",
            &mut m.ground_deceleration,
            0.0..=500.0,
            "M/S²",
        );
        row(
            ui,
            "AIR ACCEL",
            &mut m.air_acceleration,
            0.0..=500.0,
            "M/S²",
        );
        row(ui, "AIR CONTROL", &mut m.air_control, 0.0..=1.0, "");
    });
    section(ui, "JUMP", |ui| {
        let j = &mut s.jump;
        row(ui, "HEIGHT", &mut j.height, 0.0..=10.0, "M");
        row(ui, "TIME TO APEX", &mut j.time_to_apex, 0.05..=2.0, "S");
        row(
            ui,
            "FALL GRAVITY ×",
            &mut j.fall_gravity_multiplier,
            0.1..=10.0,
            "",
        );
        row(
            ui,
            "JUMP CUT GRAVITY ×",
            &mut j.jump_cut_gravity_multiplier,
            0.1..=10.0,
            "",
        );
        row(
            ui,
            "MAX FALL SPEED",
            &mut j.max_fall_speed,
            1.0..=200.0,
            "M/S",
        );
        row(ui, "COYOTE TIME", &mut j.coyote_time, 0.0..=1.0, "S");
        row(ui, "JUMP BUFFER", &mut j.buffer_time, 0.0..=1.0, "S");
        let gravity = j.gravity();
        ui.label(
            RichText::new(format!(
                "GRAVITY {gravity:.1} M/S²  TAKEOFF {:.2} M/S",
                j.takeoff_speed()
            ))
            .small()
            .color(theme::AMBER_DIM),
        );
        ui.end_row();
    });
    section(ui, "BODY", |ui| {
        let b = &mut s.body;
        row(ui, "HEIGHT", &mut b.height, 0.5..=4.0, "M");
        row(ui, "CROUCH HEIGHT", &mut b.crouch_height, 0.3..=4.0, "M");
        row(ui, "RADIUS", &mut b.radius, 0.05..=1.5, "M");
        row(ui, "EYE OFFSET", &mut b.eye_offset, 0.0..=1.0, "M");
        row(ui, "STEP HEIGHT", &mut b.step_height, 0.0..=1.5, "M");
        row(ui, "MAX SLOPE", &mut b.max_slope, 0.0..=89.0, "°");
        row(ui, "GROUND SNAP", &mut b.ground_snap, 0.0..=2.0, "M");
        row(ui, "MASS", &mut b.mass, 1.0..=500.0, "KG");
        row(ui, "REACH", &mut b.reach, 0.5..=10.0, "M");
    });
    section(ui, "CAMERA", |ui| {
        let c = &mut s.camera;
        row(ui, "FOV", &mut c.fov, 30.0..=130.0, "°");
        row(ui, "SENSITIVITY", &mut c.sensitivity, 0.001..=1.0, "°/PX");
        toggle(ui, "HEAD BOB", &mut c.head_bob);
        row(ui, "BOB AMPLITUDE", &mut c.bob_amplitude, 0.0..=0.5, "M");
        row(ui, "STEP LENGTH", &mut c.step_length, 0.1..=10.0, "M");
        row(ui, "LANDING DIP", &mut c.landing_dip, 0.0..=1.0, "M");
        row(
            ui,
            "LANDING DIP TIME",
            &mut c.landing_dip_time,
            0.01..=1.0,
            "S",
        );
        row(
            ui,
            "SPRINT FOV KICK",
            &mut c.sprint_fov_kick,
            0.0..=40.0,
            "°",
        );
        row(ui, "FOV KICK TIME", &mut c.fov_kick_time, 0.01..=2.0, "S");
        row(ui, "STEP SMOOTHING", &mut c.step_smoothing, 0.0..=0.5, "S");
    });
}

fn section(ui: &mut Ui, title: &str, add_rows: impl FnOnce(&mut Ui)) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).strong());
    egui::Grid::new(title)
        .num_columns(2)
        .spacing([12.0, 3.0])
        // Same label width in every section so the values line up.
        .min_col_width(150.0)
        .show(ui, add_rows);
}

fn row(ui: &mut Ui, label: &str, value: &mut f32, range: RangeInclusive<f32>, unit: &str) {
    ui.label(RichText::new(label).color(theme::AMBER_DIM));
    let speed = ((range.end() - range.start()) / 300.0).max(0.001);
    let suffix = if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    };
    ui.add(
        DragValue::new(value)
            .range(range)
            .speed(speed)
            .max_decimals(3)
            .suffix(suffix),
    );
    ui.end_row();
}

fn toggle(ui: &mut Ui, label: &str, value: &mut bool) {
    ui.label(RichText::new(label).color(theme::AMBER_DIM));
    let text = if *value { "[ON] " } else { "[OFF]" };
    if ui.small_button(text).clicked() {
        *value = !*value;
    }
    ui.end_row();
}
