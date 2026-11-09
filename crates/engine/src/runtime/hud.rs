//! What is drawn over the game: the debug HUD, the audio switches and
//! prompts.

use super::CameraMode;
use crate::audio::Toggles;
use crate::render::scale;
use crate::ui::theme;
use egui::{Align2, RichText};
use glam::Vec3;

/// The acoustic simulation's switches, top right of the view.
pub(super) fn draw_audio_panel(
    ui: &egui::Ui,
    image: egui::Rect,
    toggles: &Toggles,
    game_running: bool,
) {
    let state = |on: bool| if on { "[ON] " } else { "[OFF]" };
    let mut lines = vec![
        format!("F6 OCCLUSION   {}", state(toggles.occlusion)),
        format!("F7 REFLECTIONS {}", state(toggles.reflections)),
        format!("F8 HRTF        {}", state(toggles.hrtf)),
        format!("F9 AIR         {}", state(toggles.air_absorption)),
        format!("F10 BAKED      {}", state(toggles.baked)),
    ];
    if game_running {
        lines.push("Q CLAP   M MOVE SOUND HERE".into());
    }
    egui::Area::new(egui::Id::unique("audio_panel"))
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(image.right_top() + egui::vec2(-10.0, 10.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            for line in lines {
                let color = if line.contains("[OFF]") {
                    crate::ui::theme::ALERT
                } else {
                    crate::ui::theme::AMBER_DIM
                };
                ui.label(egui::RichText::new(line).small().color(color));
            }
        });
}

/// A quiet text prompt just below the middle of the view.
pub(super) fn draw_prompt(ui: &egui::Ui, image: egui::Rect, text: &str) {
    egui::Area::new(egui::Id::unique("prompt"))
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(image.center() + egui::vec2(0.0, 28.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.label(egui::RichText::new(text).color(crate::ui::theme::AMBER));
        });
}

/// A short message near the top of the view ("GAME SAVED").
pub(super) fn draw_toast(ui: &egui::Ui, image: egui::Rect, text: &str) {
    egui::Area::new(egui::Id::unique("toast"))
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(image.center_top() + egui::vec2(0.0, 24.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.label(egui::RichText::new(format!("[{text}]")).color(crate::ui::theme::AMBER));
        });
}

/// The simulation radius for the HUD: the active zones, or ALL.
pub(crate) fn active_text(zones: &[String]) -> String {
    if zones.is_empty() {
        "ALL".into()
    } else {
        zones.join(", ").to_uppercase()
    }
}

/// FPS averaged over half-second windows so the number is readable.
#[derive(Default)]
pub(super) struct FpsCounter {
    pub(super) frames: u32,
    pub(super) elapsed: f32,
    pub(super) fps: f32,
    pub(super) frame_ms: f32,
}

impl FpsCounter {
    pub(super) fn tick(&mut self, dt: f32) {
        self.frames += 1;
        self.elapsed += dt;
        if self.elapsed >= 0.5 {
            self.fps = self.frames as f32 / self.elapsed;
            self.frame_ms = self.elapsed * 1000.0 / self.frames as f32;
            self.frames = 0;
            self.elapsed = 0.0;
        }
    }
}

pub(super) struct PlayerReadout {
    pub(super) speed: f32,
    pub(super) velocity: Vec3,
    pub(super) grounded: bool,
    pub(super) air_time: f32,
    pub(super) crouching: bool,
}

pub(super) struct HudInfo {
    pub(super) fps: f32,
    pub(super) frame_ms: f32,
    /// GPU milliseconds per stage, if measured.
    pub(super) gpu_times: Option<[f32; crate::render::timings::STAGES.len()]>,
    pub(super) gpu: String,
    pub(super) output: [u32; 2],
    pub(super) internal: [u32; 2],
    pub(super) factor: u32,
    pub(super) auto: bool,
    pub(super) vsync: bool,
    pub(super) mode: CameraMode,
    pub(super) fly_speed: f32,
    pub(super) player: PlayerReadout,
    pub(super) mouse_free: bool,
    pub(super) cursor_on: bool,
    pub(super) zone: Option<String>,
    /// Current zone and neighbours; empty = everything runs.
    pub(super) active_zones: Vec<String>,
    /// Draws in view, out of all draws.
    pub(super) drawn: [usize; 2],
}

#[derive(Default)]
pub(super) struct HudActions {
    /// `Some(None)` = back to automatic.
    pub(super) scale: Option<Option<u32>>,
    pub(super) vsync: Option<bool>,
}

pub(super) fn draw_hud(ui: &mut egui::Ui, hud: &HudInfo, actions: &mut HudActions) {
    let dim = |text: &str| RichText::new(text).color(theme::AMBER_DIM);
    egui::Area::new(egui::Id::unique("hud"))
        .anchor(Align2::LEFT_TOP, [12.0, 12.0])
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(theme::BACKGROUND.gamma_multiply(0.85))
                .stroke(egui::Stroke::new(1.0, theme::AMBER_DIM))
                .inner_margin(10)
                .show(ui, |ui| {
                    let cursor = if hud.cursor_on { "_" } else { " " };
                    ui.label(RichText::new(format!("ENGINE // RENDER{cursor}")).strong());
                    ui.label(RichText::new(&hud.gpu).small().color(theme::AMBER_DIM));
                    ui.add_space(6.0);

                    egui::Grid::new("hud_stats")
                        .num_columns(2)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(dim("FPS"));
                            ui.label(format!("{:>5.0}  {:>5.2} MS", hud.fps, hud.frame_ms));
                            ui.end_row();

                            if let Some(times) = hud.gpu_times {
                                ui.label(dim("GPU"));
                                ui.label(format!("{:>5.2} MS", times.iter().sum::<f32>()));
                                ui.end_row();
                                for (name, ms) in crate::render::timings::STAGES.iter().zip(times) {
                                    ui.label(dim(&format!("  {name}")));
                                    ui.label(format!("{ms:>5.2} MS"));
                                    ui.end_row();
                                }
                            }

                            ui.label(dim("OUTPUT"));
                            ui.label(format!("{}×{}", hud.output[0], hud.output[1]));
                            ui.end_row();

                            ui.label(dim("INTERNAL"));
                            ui.label(format!("{}×{}", hud.internal[0], hud.internal[1]));
                            ui.end_row();

                            ui.label(dim("SCALE"));
                            ui.horizontal(|ui| {
                                if ui.small_button("-").clicked() {
                                    actions.scale = Some(Some(hud.factor.saturating_sub(1).max(1)));
                                }
                                ui.label(format!("×{}", hud.factor));
                                if ui.small_button("+").clicked() {
                                    actions.scale =
                                        Some(Some((hud.factor + 1).min(scale::MAX_FACTOR)));
                                }
                                let auto = if hud.auto { "[AUTO]" } else { " AUTO " };
                                if ui.small_button(auto).clicked() {
                                    actions.scale = Some(None);
                                }
                            });
                            ui.end_row();

                            ui.label(dim("VSYNC"));
                            let vsync = if hud.vsync { "[ON] " } else { "[OFF]" };
                            if ui.small_button(vsync).clicked() {
                                actions.vsync = Some(!hud.vsync);
                            }
                            ui.end_row();
                        });

                    ui.add_space(6.0);
                    egui::Grid::new("hud_player")
                        .num_columns(2)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(dim("ZONE"));
                            ui.label(hud.zone.as_deref().unwrap_or("-").to_uppercase());
                            ui.end_row();
                            ui.label(dim("ACTIVE"));
                            ui.label(active_text(&hud.active_zones));
                            ui.end_row();
                            ui.label(dim("DRAWN"));
                            ui.label(format!("{} / {}", hud.drawn[0], hud.drawn[1]));
                            ui.end_row();
                            match hud.mode {
                                CameraMode::Player => {
                                    let p = &hud.player;
                                    ui.label(dim("MODE"));
                                    ui.label(if p.crouching {
                                        "PLAYER [CROUCH]"
                                    } else {
                                        "PLAYER"
                                    });
                                    ui.end_row();

                                    ui.label(dim("SPEED"));
                                    ui.label(format!("{:>5.2} M/S", p.speed));
                                    ui.end_row();

                                    ui.label(dim("VELOCITY"));
                                    ui.label(format!(
                                        "{:>6.2} {:>6.2} {:>6.2}",
                                        p.velocity.x, p.velocity.y, p.velocity.z
                                    ));
                                    ui.end_row();

                                    ui.label(dim("GROUNDED"));
                                    ui.label(if p.grounded { "[YES]" } else { "[NO] " });
                                    ui.end_row();

                                    ui.label(dim("AIR TIME"));
                                    ui.label(format!("{:>5.2} S", p.air_time));
                                    ui.end_row();
                                }
                                CameraMode::Fly => {
                                    ui.label(dim("MODE"));
                                    ui.label("FLY");
                                    ui.end_row();

                                    ui.label(dim("CAMERA"));
                                    ui.label(format!("{:.1} M/S", hud.fly_speed));
                                    ui.end_row();
                                }
                            }
                        });

                    ui.add_space(6.0);
                    let hints: &[&str] = match hud.mode {
                        CameraMode::Player => &[
                            "WASD MOVE  SPACE JUMP  SHIFT SPRINT",
                            "CTRL/C CROUCH  R RESPAWN  E USE",
                            "M MAP  F3 TEST DRAWER",
                            "F1 TUNING  F2 FLY  ESC FREE MOUSE",
                        ],
                        CameraMode::Fly => &[
                            "RMB LOOK  WASD MOVE  Q/E DOWN/UP",
                            "SHIFT FAST  WHEEL SPEED  F2 PLAYER",
                        ],
                    };
                    for line in hints.iter().chain(&["[ ] SCALE  0 AUTO  V VSYNC"]) {
                        ui.label(RichText::new(*line).small().color(theme::AMBER_DIM));
                    }
                    if hud.mouse_free {
                        ui.label(
                            RichText::new("CLICK TO CAPTURE MOUSE")
                                .small()
                                .color(theme::AMBER),
                        );
                    }
                });
        });
}
