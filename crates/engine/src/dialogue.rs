//! How dialogue looks: a per-game style (in `game.ron`, edited in PROJECT
//! SETTINGS) and the drawing of the line on screen with its replies. The
//! default is a text box in the amber UI style.

use bevy_reflect::Reflect;
use egui::{Color32, FontFamily, FontId, Rect, RichText, vec2};
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct DialogueStyle {
    pub layout: DialogueLayout,
    pub position: DialoguePosition,
    /// Fraction of the view's width the text may use, 0.2–1.
    pub width: f32,
    /// Text size in points.
    pub text_size: f32,
    /// sRGB, 0–1.
    pub text_color: Vec3,
    pub name_color: Vec3,
    /// The box's fill (box layout).
    pub background: Vec3,
    /// 0 = see-through, 1 = solid.
    pub background_opacity: f32,
    /// A thin frame around the box.
    pub frame: bool,
    /// Names and text in capitals.
    pub uppercase: bool,
    /// Characters per second as the text appears; 0 = all at once.
    pub typing_speed: f32,
    /// A `.ttf` or `.otf` in the project, e.g. `fonts/serif.ttf`; empty =
    /// the engine's monospace.
    pub font: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Reflect)]
pub enum DialogueLayout {
    /// A framed box with the speaker's name on top.
    #[default]
    TextBox,
    /// Centered lines, `NAME: text`, no box.
    Subtitles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Reflect)]
pub enum DialoguePosition {
    #[default]
    Bottom,
    Middle,
    Top,
}

const AMBER: Vec3 = Vec3::new(1.0, 0.62, 0.17);

impl Default for DialogueStyle {
    fn default() -> Self {
        Self {
            layout: DialogueLayout::TextBox,
            position: DialoguePosition::Bottom,
            width: 0.6,
            text_size: 16.0,
            text_color: AMBER,
            name_color: AMBER,
            background: Vec3::new(0.04, 0.03, 0.024),
            background_opacity: 0.85,
            frame: true,
            uppercase: true,
            typing_speed: 45.0,
            font: String::new(),
        }
    }
}

impl DialogueStyle {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    fn font(&self, size: f32, custom: bool) -> FontId {
        if custom {
            FontId::new(size, FontFamily::Name(FONT_FAMILY.into()))
        } else {
            FontId::monospace(size)
        }
    }
}

/// The egui font family a game's dialogue font is registered under.
pub const FONT_FAMILY: &str = "dialogue";

/// Registers the style's font file with egui, if it has one that reads.
/// Returns whether dialogue should use it.
pub fn load_font(
    ctx: &egui::Context,
    project_dir: &std::path::Path,
    style: &DialogueStyle,
) -> bool {
    if style.font.is_empty() {
        return false;
    }
    match std::fs::read(project_dir.join(&style.font)) {
        Ok(bytes) => {
            ctx.add_font(egui::FontInsert::new(
                FONT_FAMILY,
                egui::FontData::from_owned(bytes),
                vec![egui::InsertFontFamily {
                    family: FontFamily::Name(FONT_FAMILY.into()),
                    priority: egui::FontPriority::Highest,
                }],
            ));
            true
        }
        Err(err) => {
            log::error!("dialogue font {}: {err}", style.font);
            false
        }
    }
}

fn color(c: Vec3) -> Color32 {
    let c = (c.clamp(Vec3::ZERO, Vec3::ONE) * 255.0).round();
    Color32::from_rgb(c.x as u8, c.y as u8, c.z as u8)
}

/// What is on screen.
pub struct Shown<'a> {
    pub speaker: &'a str,
    /// The text as far as it has appeared.
    pub text: &'a str,
    /// Whether all of it has appeared.
    pub complete: bool,
    pub replies: &'a [String],
    /// The highlighted reply.
    pub choice: usize,
}

/// Draws the line over the game view `image`.
pub fn draw(ui: &egui::Ui, image: Rect, style: &DialogueStyle, custom_font: bool, shown: &Shown) {
    let caps = |s: &str| {
        if style.uppercase {
            s.to_uppercase()
        } else {
            s.to_owned()
        }
    };
    let size = style.text_size.max(6.0);
    let text_font = style.font(size, custom_font);
    let name_font = style.font(size * 0.85, custom_font);
    let width = image.width() * style.width.clamp(0.2, 1.0);
    let margin = image.height() * 0.06;
    let (pivot, anchor) = match style.position {
        DialoguePosition::Bottom => (
            egui::Align2::CENTER_BOTTOM,
            image.center_bottom() - vec2(0.0, margin),
        ),
        DialoguePosition::Middle => (egui::Align2::CENTER_CENTER, image.center()),
        DialoguePosition::Top => (
            egui::Align2::CENTER_TOP,
            image.center_top() + vec2(0.0, margin),
        ),
    };
    let text_color = color(style.text_color);
    let name_color = color(style.name_color);
    let dim = text_color.gamma_multiply(0.55);
    let hint = if !shown.complete {
        "E  ›"
    } else if shown.replies.is_empty() {
        "E  CONTINUE"
    } else {
        "W/S CHOOSE   E ANSWER"
    };

    egui::Area::new(egui::Id::unique("dialogue"))
        .pivot(pivot)
        .fixed_pos(anchor)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            ui.set_width(width);
            let replies = |ui: &mut egui::Ui, centered: bool| {
                if !shown.complete || shown.replies.is_empty() {
                    return;
                }
                ui.add_space(size * 0.5);
                for (i, reply) in shown.replies.iter().enumerate() {
                    let picked = i == shown.choice;
                    let mark = if picked { "›" } else { " " };
                    let text = RichText::new(format!("{mark} {}. {}", i + 1, caps(reply)))
                        .font(text_font.clone())
                        .color(if picked { text_color } else { dim });
                    if centered {
                        ui.vertical_centered(|ui| ui.label(text));
                    } else {
                        ui.label(text);
                    }
                }
            };
            match style.layout {
                DialogueLayout::TextBox => {
                    let mut frame = egui::Frame::NONE
                        .fill(
                            color(style.background)
                                .gamma_multiply(style.background_opacity.clamp(0.0, 1.0)),
                        )
                        .inner_margin(size * 0.9);
                    if style.frame {
                        frame = frame.stroke(egui::Stroke::new(1.0, dim));
                    }
                    frame.show(ui, |ui| {
                        ui.set_width(width - size * 1.8);
                        if !shown.speaker.is_empty() {
                            ui.label(
                                RichText::new(caps(shown.speaker))
                                    .font(name_font.clone())
                                    .color(name_color)
                                    .strong(),
                            );
                            ui.add_space(size * 0.3);
                        }
                        ui.label(
                            RichText::new(caps(shown.text))
                                .font(text_font.clone())
                                .color(text_color),
                        );
                        replies(ui, false);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(hint).font(FontId::monospace(10.0)).color(dim));
                        });
                    });
                }
                DialogueLayout::Subtitles => {
                    let line = if shown.speaker.is_empty() {
                        caps(shown.text)
                    } else {
                        format!("{}: {}", caps(shown.speaker), caps(shown.text))
                    };
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(line)
                                .font(text_font.clone())
                                .color(text_color),
                        );
                    });
                    replies(ui, true);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(hint).font(FontId::monospace(10.0)).color(dim));
                    });
                }
            }
        });
}

/// How much of `text` has appeared after `seconds` at `speed` characters
/// per second (0 = all of it), cut at a character boundary.
pub fn typed(text: &str, seconds: f32, speed: f32) -> (&str, bool) {
    if speed <= 0.0 {
        return (text, true);
    }
    let count = (seconds * speed) as usize;
    match text.char_indices().nth(count) {
        Some((end, _)) => (&text[..end], false),
        None => (text, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_types_out_at_its_speed() {
        assert_eq!(typed("héllo", 0.1, 20.0), ("hé", false));
        assert_eq!(typed("héllo", 1.0, 20.0), ("héllo", true));
        assert_eq!(typed("héllo", 0.0, 0.0), ("héllo", true));
    }

    #[test]
    fn the_default_style_isnt_written() {
        let style = DialogueStyle::default();
        assert!(style.is_default());
        let text = ron::to_string(&DialogueStyle {
            typing_speed: 0.0,
            ..Default::default()
        })
        .unwrap();
        let back: DialogueStyle = ron::from_str(&text).unwrap();
        assert_eq!(back.typing_speed, 0.0);
    }
}
